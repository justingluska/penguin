//! Interactive Google sign-in for every flow that needs one (add account,
//! reconnect, contact photos, calendar): the system sheet when an iOS-type
//! OAuth client is configured, otherwise the loopback browser flow, which
//! brings Penguin back to the front once the browser shows its "signed in"
//! page. Cancellable through `cancel_sign_in`.
//!
//! The browser flow's link is kept while it waits ([`PendingSignIn`]) and
//! sent to the UI (`penguin://sign-in-url`), so the waiting screens can offer
//! "Open it again" (`reopen_sign_in`) and "Copy link" when no browser came
//! up, and show why when Penguin couldn't open one.

use std::sync::Mutex;

use penguin_gmail::auth::{SignInUi, SignedInAccount};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::error::{CmdError, CmdResult};
use crate::state::{blocking, AppState};
use crate::views::SignInLink;

/// Emitted with a [`SignInLink`] when a browser sign-in starts, and again
/// with its `error` set if the browser couldn't be opened.
pub const EVENT_SIGN_IN_URL: &str = "penguin://sign-in-url";

/// The link of the browser sign-in in flight, if any (managed state).
/// Each interactive sign-in is an attempt; a newer one replaces the older
/// one's link, and an older one finishing never clears the newer one's.
#[derive(Default)]
pub struct PendingSignIn(Mutex<Pending>);

#[derive(Default)]
struct Pending {
    attempt: u64,
    link: Option<SignInLink>,
}

impl PendingSignIn {
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Start a new attempt: forgets any older link.
    fn begin(&self) -> u64 {
        let mut p = self.lock();
        p.attempt += 1;
        p.link = None;
        p.attempt
    }

    /// Keep `link` for `attempt`. False (and ignored) once a newer attempt began.
    fn record(&self, attempt: u64, link: SignInLink) -> bool {
        let mut p = self.lock();
        if p.attempt != attempt {
            return false;
        }
        p.link = Some(link);
        true
    }

    /// `attempt` ended (signed in, failed or cancelled).
    fn finish(&self, attempt: u64) {
        let mut p = self.lock();
        if p.attempt == attempt {
            p.link = None;
        }
    }

    pub fn current(&self) -> Option<SignInLink> {
        self.lock().link.clone()
    }
}

/// Clears the attempt's link however the sign-in ends, including the
/// command's future being dropped.
struct AttemptGuard<'a> {
    pending: &'a PendingSignIn,
    attempt: u64,
}

impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        self.pending.finish(self.attempt);
    }
}

/// Record the link, tell the UI, then open the browser; if that fails, the
/// error is logged, recorded and sent to the UI too. `emit` runs only while
/// `attempt` is still the current one.
fn open_and_record(
    pending: &PendingSignIn,
    attempt: u64,
    url: String,
    open: impl FnOnce(&str) -> Result<(), String>,
    mut emit: impl FnMut(&SignInLink),
) {
    let link = SignInLink { url, error: None };
    if !pending.record(attempt, link.clone()) {
        return;
    }
    emit(&link);
    if let Err(error) = open(&link.url) {
        let failed = SignInLink {
            error: Some(error),
            ..link
        };
        if pending.record(attempt, failed.clone()) {
            emit(&failed);
        }
    }
}

/// Open `url` in the default browser. Blocking (on macOS it waits for
/// `/usr/bin/open`). Logs the outcome, never the URL.
fn open_in_browser(app: &AppHandle, url: &str) -> Result<(), String> {
    match app.opener().open_url(url, None::<&str>) {
        Ok(()) => {
            tracing::info!("browser opened for sign-in");
            Ok(())
        }
        Err(e) => {
            tracing::error!(error = %e, "could not open the browser for sign-in");
            Err(e.to_string())
        }
    }
}

/// The UI hooks for one sign-in: open the browser, present the system
/// sheet over the main window (macOS), focus Penguin after browser success.
fn sign_in_ui(app: &AppHandle, attempt: u64) -> SignInUi {
    let opener = app.clone();
    let mut ui = SignInUi::new(move |url| {
        // Off the async runtime: opening waits for the launcher to exit.
        let app = opener.clone();
        let url = url.to_owned();
        tauri::async_runtime::spawn_blocking(move || {
            let pending = app.state::<PendingSignIn>();
            open_and_record(
                &pending,
                attempt,
                url,
                |url| open_in_browser(&app, url),
                |link| {
                    if let Err(e) = app.emit(EVENT_SIGN_IN_URL, link) {
                        tracing::warn!(error = %e, "failed to emit the sign-in link");
                    }
                },
            );
        });
    });
    #[cfg(target_os = "macos")]
    {
        use std::sync::Arc;
        let main = app.clone();
        let win = app.clone();
        ui = ui.with_sheet(Arc::new(penguin_gmail::auth::AppleWebAuthSheet::new(
            move |f| main.run_on_main_thread(f).map_err(|e| e.to_string()),
            move || main_window_anchor(&win),
        )));
    }
    let focus = app.clone();
    ui.on_browser_success(move || crate::app_menu::bring_to_front(&focus))
}

/// The main window's `NSWindow*`, for the sign-in sheet. Main thread only.
#[cfg(target_os = "macos")]
fn main_window_anchor(app: &AppHandle) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
    app.get_webview_window("main")
        .and_then(|w| w.ns_window().ok())
        .and_then(std::ptr::NonNull::new)
}

/// Run an interactive sign-in (optionally preselecting `hint` and asking for
/// `extra_scopes`). `cancel_sign_in` (or a newer sign-in) aborts it;
/// dropping the future closes the loopback listener or dismisses the sheet.
pub async fn interactive_sign_in(
    app: &AppHandle,
    state: &AppState,
    hint: Option<&str>,
    extra_scopes: &[&str],
) -> CmdResult<SignedInAccount> {
    let services = state.services()?;
    let cancelled = state.begin_sign_in();
    let pending = app.state::<PendingSignIn>();
    let guard = AttemptGuard {
        pending: &pending,
        attempt: pending.begin(),
    };
    // The system sheet attaches to the main window: make sure it's shown and
    // frontmost first so the sheet never hangs off a hidden window. (Queued
    // on the main thread ahead of the sheet's own presentation.)
    if cfg!(target_os = "macos") && services.auth.ios_client().is_some() {
        crate::app_menu::bring_to_front(app);
    }
    let ui = sign_in_ui(app, guard.attempt);
    let sign_in = services.auth.sign_in_interactive(hint, extra_scopes, &ui);
    tokio::select! {
        signed = sign_in => Ok(signed?),
        _ = cancelled => {
            tracing::info!("sign-in cancelled");
            Err(CmdError::cancelled())
        }
    }
}

/// The browser sign-in waiting for the user, if any (the UI asks when its
/// waiting screen mounts, in case it missed `penguin://sign-in-url`).
#[tauri::command]
pub async fn sign_in_link(app: AppHandle) -> CmdResult<Option<SignInLink>> {
    Ok(app.state::<PendingSignIn>().current())
}

/// Open the waiting sign-in's link in the browser again ("Browser didn't
/// open?"). Rejects `invalidInput` when no browser sign-in is waiting, and
/// with the opener's error (also logged) when the browser can't be opened.
#[tauri::command]
pub async fn reopen_sign_in(app: AppHandle) -> CmdResult<()> {
    let link = app.state::<PendingSignIn>().current().ok_or_else(|| {
        CmdError::invalid("No sign-in is waiting for the browser. Start it again.")
    })?;
    tracing::info!("opening the sign-in link again");
    blocking(move || {
        open_in_browser(&app, &link.url)
            .map_err(|e| CmdError::other(format!("Penguin couldn't open your browser ({e}).")))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, error: Option<&str>) -> SignInLink {
        SignInLink {
            url: url.into(),
            error: error.map(Into::into),
        }
    }

    #[test]
    fn a_failed_browser_open_reaches_the_ui_with_its_error() {
        let pending = PendingSignIn::default();
        let attempt = pending.begin();
        let mut emitted = Vec::new();
        open_and_record(
            &pending,
            attempt,
            "https://accounts.example/auth?x=1".into(),
            |_| Err("launcher exited with 1".into()),
            |l| emitted.push(l.clone()),
        );
        assert_eq!(
            emitted,
            [
                link("https://accounts.example/auth?x=1", None),
                link(
                    "https://accounts.example/auth?x=1",
                    Some("launcher exited with 1")
                ),
            ]
        );
        assert_eq!(
            pending.current(),
            Some(link(
                "https://accounts.example/auth?x=1",
                Some("launcher exited with 1")
            ))
        );
    }

    #[test]
    fn an_opened_browser_keeps_the_link_for_open_again_and_copy() {
        let pending = PendingSignIn::default();
        let attempt = pending.begin();
        let mut emitted = Vec::new();
        let mut opened = Vec::new();
        open_and_record(
            &pending,
            attempt,
            "https://accounts.example/auth".into(),
            |u| {
                opened.push(u.to_owned());
                Ok(())
            },
            |l| emitted.push(l.clone()),
        );
        assert_eq!(opened, ["https://accounts.example/auth"]);
        assert_eq!(emitted, [link("https://accounts.example/auth", None)]);
        assert_eq!(
            pending.current(),
            Some(link("https://accounts.example/auth", None))
        );
        pending.finish(attempt);
        assert_eq!(pending.current(), None);
    }

    #[test]
    fn an_older_attempt_never_touches_a_newer_ones_link() {
        let pending = PendingSignIn::default();
        let old = pending.begin();
        assert!(pending.record(old, link("https://old.example", None)));
        let new = pending.begin();
        assert_eq!(
            pending.current(),
            None,
            "a new attempt forgets the old link"
        );
        assert!(pending.record(new, link("https://new.example", None)));
        // The old attempt's late open result and its cancellation are ignored.
        let mut emitted = Vec::new();
        open_and_record(
            &pending,
            old,
            "https://old.example".into(),
            |_| Ok(()),
            |l| emitted.push(l.clone()),
        );
        assert!(emitted.is_empty());
        pending.finish(old);
        assert_eq!(pending.current(), Some(link("https://new.example", None)));
    }

    #[test]
    fn the_guard_clears_the_link_when_the_sign_in_ends() {
        let pending = PendingSignIn::default();
        {
            let guard = AttemptGuard {
                pending: &pending,
                attempt: pending.begin(),
            };
            pending.record(guard.attempt, link("https://a.example", None));
            assert!(pending.current().is_some());
        }
        assert_eq!(pending.current(), None);
    }
}
