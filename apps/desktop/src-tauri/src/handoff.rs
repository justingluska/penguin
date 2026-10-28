//! Handoff between the Mac and the iPhone (Apple's NSUserActivity Handoff,
//! not the window-closing handoffs in src/app/handoffs.ts). Reading a
//! conversation on one device offers it on the other; so does a draft once
//! the composer's autosave has made it a Gmail draft.
//!
//! The contract both apps share (the iPhone app reads exactly this; never
//! change it without the iPhone side):
//! - `co.gluska.penguin.conversation`: userInfo `{"v": 1, "account": <account
//!   id, the lowercased address>, "thread": <provider thread id, what
//!   get_thread takes>}`, title = the subject.
//! - `co.gluska.penguin.draft`: userInfo `{"v": 1, "account", "draftId": <Gmail
//!   draft id>, "thread"?: <the thread it answers>}`, title = the subject or
//!   "New message". The draft's content travels through Gmail, never here.
//!
//! Both types are declared in Info.plist (`NSUserActivityTypes`). Only ids
//! and a subject: never a body or a snippet. Gmail accounts only (the UI
//! decides, src/features/handoff/), since an IMAP or Microsoft account means
//! nothing to the iPhone app.
//!
//! Publishing: every window tells the backend what it shows
//! (`set_handoff_activity`): the main window its open or previewed
//! conversation or its composer's saved draft, a conversation window its
//! conversation, a compose window its draft, or nothing (hidden, minimized,
//! a new message not saved yet). The activity of the window last brought to
//! the front is the one advertised; the Swift bridge
//! (swift/PenguinAI/Sources/PenguinAI/Handoff.swift) keeps one
//! NSUserActivity current, updated in place while the type stays the same.
//!
//! Receiving: tao's app delegate already answers
//! `application:continueUserActivity:restorationHandler:`, but only for web
//! browsing activities (it turns those into opened URLs), and Tauri offers no
//! other hook. So the bridge wraps that method on the delegate's class once
//! at startup: Penguin's two types come here through a C callback, anything
//! else still goes to tao. `application:willContinueUserActivityWithType:`
//! stays tao's: it answers NO for these types, which lets macOS show its own
//! "opening…" indication. A continued activity is read and checked here,
//! kept as the pending open, Penguin comes to the front and the main window
//! gets `penguin://handoff-open`; it takes the pending open
//! (`take_handoff_open`, also once when it starts, for a launch by
//! Handoff) and opens the conversation or the draft. An unknown version or
//! a malformed field comes through as `unreadable` and the UI says so.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::error::{CmdError, CmdResult};

pub const TYPE_CONVERSATION: &str = "co.gluska.penguin.conversation";
pub const TYPE_DRAFT: &str = "co.gluska.penguin.draft";
/// No payload: the main window takes the open with `take_handoff_open`.
pub const EVENT_HANDOFF_OPEN: &str = "penguin://handoff-open";
/// The userInfo version both apps write and read.
pub const VERSION: u64 = 1;
const MAX_ID: usize = 512;
const MAX_TITLE: usize = 200;
const NO_SUBJECT: &str = "(no subject)";
const NEW_MESSAGE: &str = "New message";

/// What a window shows, as far as Handoff cares (`set_handoff_activity`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum HandoffActivity {
    #[serde(rename_all = "camelCase")]
    Conversation {
        account_id: String,
        thread_id: String,
        #[serde(default)]
        subject: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Draft {
        account_id: String,
        draft_id: String,
        /// The conversation the draft answers; none for a new message.
        #[serde(default)]
        thread_id: Option<String>,
        #[serde(default)]
        subject: Option<String>,
    },
    None,
}

/// The NSUserActivity to advertise: its type, title and userInfo.
#[derive(Debug, Clone, PartialEq)]
pub struct Advert {
    pub activity_type: &'static str,
    pub title: String,
    pub user_info: Value,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(char::is_control)
}

fn check_id(what: &str, id: &str) -> CmdResult<()> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(CmdError::invalid(format!(
            "set_handoff_activity: invalid {what}"
        )))
    }
}

/// One line, no control characters, at most 200 characters; `fallback` when empty.
fn clean_title(title: Option<&str>, fallback: &str) -> String {
    let t = title
        .unwrap_or("")
        .split(char::is_control)
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() {
        fallback.to_string()
    } else {
        t.chars().take(MAX_TITLE).collect()
    }
}

impl HandoffActivity {
    /// The activity to advertise for this (none for `None`), or why the
    /// request is malformed.
    pub fn advert(&self) -> CmdResult<Option<Advert>> {
        match self {
            HandoffActivity::None => Ok(None),
            HandoffActivity::Conversation {
                account_id,
                thread_id,
                subject,
            } => {
                check_id("account id", account_id)?;
                check_id("thread id", thread_id)?;
                Ok(Some(Advert {
                    activity_type: TYPE_CONVERSATION,
                    title: clean_title(subject.as_deref(), NO_SUBJECT),
                    user_info: json!({
                        "v": VERSION,
                        "account": account_id.to_lowercase(),
                        "thread": thread_id,
                    }),
                }))
            }
            HandoffActivity::Draft {
                account_id,
                draft_id,
                thread_id,
                subject,
            } => {
                check_id("account id", account_id)?;
                check_id("draft id", draft_id)?;
                if let Some(t) = thread_id {
                    check_id("thread id", t)?;
                }
                let mut info = Map::new();
                info.insert("v".into(), json!(VERSION));
                info.insert("account".into(), json!(account_id.to_lowercase()));
                info.insert("draftId".into(), json!(draft_id));
                if let Some(t) = thread_id {
                    info.insert("thread".into(), json!(t));
                }
                Ok(Some(Advert {
                    activity_type: TYPE_DRAFT,
                    title: clean_title(subject.as_deref(), NEW_MESSAGE),
                    user_info: Value::Object(info),
                }))
            }
        }
    }
}

/// Which of Penguin's activity types came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityKind {
    Conversation,
    Draft,
}

/// What a continued activity asks the UI to open (`take_handoff_open`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HandoffOpen {
    #[serde(rename_all = "camelCase")]
    Conversation {
        account_id: String,
        thread_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Draft {
        account_id: String,
        draft_id: String,
        thread_id: Option<String>,
    },
    /// Penguin's type, but an unknown version or a missing or malformed field.
    Unreadable { activity: ActivityKind },
}

/// Read a continued activity: None when the type isn't Penguin's, else what
/// to open (or `Unreadable`). `user_info` is its userInfo as JSON.
pub fn parse_incoming(activity_type: &str, user_info: &[u8]) -> Option<HandoffOpen> {
    let kind = match activity_type {
        TYPE_CONVERSATION => ActivityKind::Conversation,
        TYPE_DRAFT => ActivityKind::Draft,
        _ => return None,
    };
    Some(read_info(kind, user_info).unwrap_or(HandoffOpen::Unreadable { activity: kind }))
}

fn read_info(kind: ActivityKind, user_info: &[u8]) -> Option<HandoffOpen> {
    let info: Map<String, Value> = serde_json::from_slice(user_info).ok()?;
    if info.get("v")?.as_u64()? != VERSION {
        return None;
    }
    let id = |key: &str| -> Option<String> {
        let s = info.get(key)?.as_str()?;
        valid_id(s).then(|| s.to_string())
    };
    let account_id = id("account")?.to_lowercase();
    match kind {
        ActivityKind::Conversation => Some(HandoffOpen::Conversation {
            account_id,
            thread_id: id("thread")?,
        }),
        ActivityKind::Draft => {
            // Optional, but malformed when present.
            let thread_id = match info.get("thread") {
                None | Some(Value::Null) => None,
                Some(_) => Some(id("thread")?),
            };
            Some(HandoffOpen::Draft {
                account_id,
                draft_id: id("draftId")?,
                thread_id,
            })
        }
    }
}

/// What to tell the bridge.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Publish(Advert),
    Resign,
}

/// Each window's activity and which window is in front; what's advertised.
#[derive(Default)]
struct Windows {
    by_window: HashMap<String, Advert>,
    /// The window last brought to the front (the main window before any).
    front: Option<String>,
    shown: Option<Advert>,
}

impl Windows {
    fn wanted(&self) -> Option<&Advert> {
        let label = self.front.as_deref().unwrap_or(crate::windows::MAIN);
        self.by_window.get(label)
    }

    /// The change that makes the advertised activity the wanted one, if any.
    fn settle(&mut self) -> Option<Change> {
        let want = self.wanted().cloned();
        if want == self.shown {
            return None;
        }
        self.shown = want.clone();
        Some(match want {
            Some(a) => Change::Publish(a),
            None => Change::Resign,
        })
    }

    fn set(&mut self, label: &str, advert: Option<Advert>) -> Option<Change> {
        match advert {
            Some(a) => self.by_window.insert(label.to_string(), a),
            None => self.by_window.remove(label),
        };
        self.settle()
    }

    fn focused(&mut self, label: &str) -> Option<Change> {
        self.front = Some(label.to_string());
        self.settle()
    }

    fn gone(&mut self, label: &str) -> Option<Change> {
        self.by_window.remove(label);
        if self.front.as_deref() == Some(label) {
            self.front = None;
        }
        self.settle()
    }
}

/// Managed state.
#[derive(Default)]
pub struct Handoff {
    windows: Mutex<Windows>,
    pending: Mutex<Option<HandoffOpen>>,
}

impl Handoff {
    /// Under the lock, so the bridge sees changes in order.
    fn apply(&self, f: impl FnOnce(&mut Windows) -> Option<Change>) {
        let mut w = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        match f(&mut w) {
            Some(Change::Publish(a)) => {
                tracing::debug!(activity = a.activity_type, "handoff: advertising");
                bridge::publish(&a);
            }
            Some(Change::Resign) => {
                tracing::debug!("handoff: nothing to advertise");
                bridge::resign();
            }
            None => {}
        }
    }

    /// A window came to the front.
    pub fn window_focused(&self, label: &str) {
        self.apply(|w| w.focused(label));
    }

    /// A window closed for real.
    pub fn window_gone(&self, label: &str) {
        self.apply(|w| w.gone(label));
    }

    fn set_pending(&self, open: HandoffOpen) {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(open);
    }

    fn take_pending(&self) -> Option<HandoffOpen> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Start receiving continued activities (from setup, on the main thread,
/// once the app delegate exists).
pub fn install(app: &AppHandle) {
    if APP.set(app.clone()).is_err() {
        return;
    }
    bridge::install();
}

/// A continued activity of one of Penguin's types (from the bridge, on the
/// main thread).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn receive(activity_type: &str, user_info: &[u8]) {
    let Some(open) = parse_incoming(activity_type, user_info) else {
        return;
    };
    let Some(app) = APP.get() else { return };
    match &open {
        HandoffOpen::Unreadable { activity } => {
            tracing::warn!(
                ?activity,
                "handoff: couldn't read the activity from the other device"
            )
        }
        _ => tracing::info!(
            activity = activity_type,
            "handoff: continuing from another device"
        ),
    }
    if let Some(h) = app.try_state::<Handoff>() {
        h.set_pending(open);
    }
    crate::app_menu::bring_to_front(app);
    if let Err(e) = app.emit_to(crate::windows::MAIN, EVENT_HANDOFF_OPEN, ()) {
        tracing::warn!(error = %e, "could not emit handoff-open");
    }
}

/// `n` bytes at `p` (none for a null pointer or a length of 0 or less).
///
/// # Safety
/// `p` must point to `n` readable bytes that outlive the returned slice.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
unsafe fn bytes<'a>(p: *const u8, n: isize) -> &'a [u8] {
    if p.is_null() || n <= 0 {
        &[]
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}

/// The C callback the bridge calls (Handoff.swift
/// `PenguinHandoffContinued`): the activity type and its userInfo as JSON,
/// each valid for this call only. Never unwinds into Swift.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
extern "C" fn continued(kind: *const u8, kind_len: isize, info: *const u8, info_len: isize) {
    // SAFETY: Swift passes each buffer's length, valid for this call.
    let (kind, info) = unsafe { (bytes(kind, kind_len), bytes(info, info_len)) };
    let result = std::panic::catch_unwind(|| {
        if let Ok(kind) = std::str::from_utf8(kind) {
            receive(kind, info);
        }
    });
    if result.is_err() {
        tracing::error!("handoff: receiving an activity panicked");
    }
}

// ---------- commands ----------

/// What the calling window shows; the front window's is advertised.
#[tauri::command]
pub fn set_handoff_activity(
    window: WebviewWindow,
    handoff: State<'_, Handoff>,
    activity: HandoffActivity,
) -> CmdResult<()> {
    let advert = activity.advert()?;
    handoff.apply(|w| w.set(window.label(), advert));
    Ok(())
}

/// The conversation or draft another device handed over, once (null after
/// that, or when there's none).
#[tauri::command]
pub fn take_handoff_open(handoff: State<'_, Handoff>) -> Option<HandoffOpen> {
    handoff.take_pending()
}

// ---------- the Swift bridge ----------

#[cfg(target_os = "macos")]
mod bridge {
    type Continued = extern "C" fn(*const u8, isize, *const u8, isize);

    extern "C" {
        fn penguin_handoff_install(callback: Continued) -> i32;
        fn penguin_handoff_publish(
            kind: *const u8,
            kind_len: isize,
            title: *const u8,
            title_len: isize,
            info: *const u8,
            info_len: isize,
        );
        fn penguin_handoff_resign();
    }

    pub fn install() {
        // SAFETY: a plain function the bridge keeps for the app's lifetime.
        match unsafe { penguin_handoff_install(super::continued) } {
            0 => tracing::info!("handoff: receiving from the iPhone is on"),
            1 => tracing::error!("handoff: no app delegate; receiving from the iPhone is off"),
            2 => tracing::error!(
                "handoff: no type encoding for continueUserActivity; receiving from the iPhone is off"
            ),
            code => tracing::warn!(code, "handoff: receiving installed off the main thread"),
        }
    }

    pub fn publish(a: &super::Advert) {
        let info = a.user_info.to_string();
        // SAFETY: Swift copies each buffer before it returns.
        unsafe {
            penguin_handoff_publish(
                a.activity_type.as_ptr(),
                a.activity_type.len() as isize,
                a.title.as_ptr(),
                a.title.len() as isize,
                info.as_ptr(),
                info.len() as isize,
            )
        }
    }

    pub fn resign() {
        // SAFETY: no arguments; Swift hops to the main thread.
        unsafe { penguin_handoff_resign() }
    }
}

/// Not a Mac: no Handoff.
#[cfg(not(target_os = "macos"))]
mod bridge {
    pub fn install() {}
    pub fn publish(_: &super::Advert) {}
    pub fn resign() {}
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
