//! The system sign-in sheet on macOS: ASWebAuthenticationSession.
//!
//! AppKit objects live on the main thread only. The app supplies
//! `run_on_main` (Tauri's `run_on_main_thread`) and an `anchor` closure that,
//! on the main thread, returns the window to present from (the `NSWindow*`
//! that `ASPresentationAnchor` is on macOS). Live
//! sessions are kept in a main-thread-local table until they complete, since
//! the session only holds its presentation provider weakly.
//!
//! The callback scheme is passed to the session itself, so no Info.plist
//! `CFBundleURLTypes` registration is needed: the session intercepts the
//! redirect before it ever reaches Launch Services.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    available, define_class, msg_send, AllocAnyThread, DefinedClass, MainThreadMarker,
    MainThreadOnly,
};
use objc2_authentication_services::{
    ASPresentationAnchor, ASWebAuthenticationPresentationContextProviding,
    ASWebAuthenticationSession, ASWebAuthenticationSessionCallback,
    ASWebAuthenticationSessionErrorCode, ASWebAuthenticationSessionErrorDomain,
};
use objc2_foundation::{NSError, NSString, NSURL};
use tokio::sync::oneshot;

use super::sheet::{SheetError, SheetFuture, WebAuthSheet};

type RunOnMain = Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> Result<(), String> + Send + Sync>;
type Anchor = Arc<dyn Fn() -> Option<NonNull<c_void>> + Send + Sync>;
type Reply = Arc<Mutex<Option<oneshot::Sender<Result<String, SheetError>>>>>;

/// [`WebAuthSheet`] backed by ASWebAuthenticationSession.
pub struct AppleWebAuthSheet {
    run_on_main: RunOnMain,
    anchor: Anchor,
}

impl AppleWebAuthSheet {
    /// `run_on_main` must run its closure on the main thread (or return why it
    /// can't, e.g. the event loop is gone). `anchor` is
    /// called there and returns the `NSWindow*` the sheet
    /// attaches to (the main window), or `None` if there is no window to
    /// present from.
    pub fn new(
        run_on_main: impl Fn(Box<dyn FnOnce() + Send>) -> Result<(), String> + Send + Sync + 'static,
        anchor: impl Fn() -> Option<NonNull<c_void>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            run_on_main: Arc::new(run_on_main),
            anchor: Arc::new(anchor),
        }
    }
}

impl WebAuthSheet for AppleWebAuthSheet {
    fn authenticate(&self, url: String, callback_scheme: String) -> SheetFuture {
        let run_on_main = self.run_on_main.clone();
        let anchor = self.anchor.clone();
        Box::pin(async move {
            static NEXT_ID: AtomicU64 = AtomicU64::new(1);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            let reply: Reply = Arc::new(Mutex::new(Some(tx)));
            let start_reply = reply.clone();
            let finish = run_on_main.clone();
            run_on_main(Box::new(move || {
                start(id, &url, &callback_scheme, &anchor, start_reply, finish)
            }))
            .map_err(SheetError::Unavailable)?;
            let mut guard = CancelOnDrop {
                id,
                run_on_main,
                armed: true,
            };
            let result = rx
                .await
                .unwrap_or_else(|_| Err(SheetError::Failed("the sign-in sheet went away".into())));
            guard.armed = false;
            result
        })
    }
}

/// Dismisses the sheet if the sign-in future is dropped early (the user hit
/// Cancel in Penguin, or a newer sign-in replaced this one).
struct CancelOnDrop {
    id: u64,
    run_on_main: RunOnMain,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let id = self.id;
            let cancel = (self.run_on_main)(Box::new(move || {
                if let Some(live) = take_session(id) {
                    // SAFETY: on the main thread; the session is alive.
                    unsafe { live.session.cancel() };
                }
            }));
            if let Err(e) = cancel {
                tracing::warn!(error = %e, "could not dismiss the sign-in sheet");
            }
        }
    }
}

struct LiveSession {
    session: Retained<ASWebAuthenticationSession>,
    _provider: Retained<AnchorProvider>,
}

thread_local! {
    /// Main thread only: sessions between `start` and completion.
    static SESSIONS: RefCell<HashMap<u64, LiveSession>> = RefCell::new(HashMap::new());
}

fn take_session(id: u64) -> Option<LiveSession> {
    SESSIONS.with(|s| s.borrow_mut().remove(&id))
}

fn send(reply: &Reply, result: Result<String, SheetError>) {
    if let Some(tx) = reply.lock().unwrap_or_else(|p| p.into_inner()).take() {
        let _ = tx.send(result);
    }
}

/// Runs on the main thread.
fn start(id: u64, url: &str, scheme: &str, anchor: &Anchor, reply: Reply, finish: RunOnMain) {
    let Some(mtm) = MainThreadMarker::new() else {
        send(
            &reply,
            Err(SheetError::Unavailable(
                "not called on the main thread".into(),
            )),
        );
        return;
    };
    let Some(window) = anchor() else {
        send(
            &reply,
            Err(SheetError::Unavailable(
                "no Penguin window to attach to".into(),
            )),
        );
        return;
    };
    // SAFETY: `anchor` returns a live NSWindow*, and we're on the
    // main thread where the UI framework keeps it alive; retaining it keeps it
    // valid for the session.
    let Some(window) = (unsafe { Retained::retain(window.as_ptr().cast::<NSObject>()) }) else {
        send(
            &reply,
            Err(SheetError::Unavailable(
                "no Penguin window to attach to".into(),
            )),
        );
        return;
    };
    let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        send(
            &reply,
            Err(SheetError::Failed("invalid authorization URL".into())),
        );
        return;
    };

    let done = reply.clone();
    let completion = RcBlock::new(move |callback: *mut NSURL, error: *mut NSError| {
        // SAFETY: the session passes either a valid callback URL or a valid
        // error, each live for the duration of this call.
        let result = unsafe { outcome(callback.as_ref(), error.as_ref()) };
        send(&done, result);
        // Release the session on the main thread, where it lives.
        if MainThreadMarker::new().is_some() {
            drop(take_session(id));
        } else if let Err(e) = finish(Box::new(move || drop(take_session(id)))) {
            tracing::warn!(error = %e, "could not release a finished sign-in sheet");
        }
    });

    let scheme = NSString::from_str(scheme);
    // SAFETY: all arguments are valid; the block pointer outlives the call
    // and the session copies it.
    let session = unsafe {
        if available!(macos = 14.4) {
            let callback = ASWebAuthenticationSessionCallback::callbackWithCustomScheme(&scheme);
            ASWebAuthenticationSession::initWithURL_callback_completionHandler(
                ASWebAuthenticationSession::alloc(),
                &ns_url,
                &callback,
                RcBlock::as_ptr(&completion),
            )
        } else {
            // The only initializer before macOS 14.4.
            #[allow(deprecated)]
            ASWebAuthenticationSession::initWithURL_callbackURLScheme_completionHandler(
                ASWebAuthenticationSession::alloc(),
                &ns_url,
                Some(&scheme),
                RcBlock::as_ptr(&completion),
            )
        }
    };

    let provider = AnchorProvider::new(mtm, window);
    // SAFETY: main thread; the provider is kept alive in SESSIONS because the
    // session holds it weakly.
    let started = unsafe {
        session.setPresentationContextProvider(Some(ProtocolObject::from_ref(&*provider)));
        // Reuse the Safari session so an already-signed-in Google account
        // doesn't have to type its password again.
        session.setPrefersEphemeralWebBrowserSession(false);
        session.start()
    };
    if !started {
        send(
            &reply,
            Err(SheetError::Unavailable(
                "the system refused to start the sign-in sheet".into(),
            )),
        );
        return;
    }
    SESSIONS.with(|s| {
        s.borrow_mut().insert(
            id,
            LiveSession {
                session,
                _provider: provider,
            },
        )
    });
}

/// # Safety
/// Both references must be valid for the call (they come from the session).
unsafe fn outcome(callback: Option<&NSURL>, error: Option<&NSError>) -> Result<String, SheetError> {
    if let Some(url) = callback.and_then(|u| u.absoluteString()) {
        return Ok(url.to_string());
    }
    let Some(error) = error else {
        return Err(SheetError::Failed("no callback URL and no error".into()));
    };
    let domain = error.domain();
    // SAFETY: an immutable framework constant.
    let session_domain = unsafe { ASWebAuthenticationSessionErrorDomain };
    let cancelled = error.code() == ASWebAuthenticationSessionErrorCode::CanceledLogin.0
        && &*domain == session_domain;
    if cancelled {
        Err(SheetError::Cancelled)
    } else {
        Err(SheetError::Failed(error.localizedDescription().to_string()))
    }
}

struct AnchorIvars {
    window: Retained<ASPresentationAnchor>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PenguinWebAuthAnchorProvider"]
    #[ivars = AnchorIvars]
    struct AnchorProvider;

    unsafe impl NSObjectProtocol for AnchorProvider {}

    unsafe impl ASWebAuthenticationPresentationContextProviding for AnchorProvider {
        #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
        fn presentation_anchor(
            &self,
            _session: &ASWebAuthenticationSession,
        ) -> Retained<ASPresentationAnchor> {
            self.ivars().window.clone()
        }
    }
);

impl AnchorProvider {
    fn new(mtm: MainThreadMarker, window: Retained<ASPresentationAnchor>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AnchorIvars { window });
        // SAFETY: NSObject's init on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}
