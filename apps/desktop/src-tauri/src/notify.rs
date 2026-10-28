//! New-mail notifications (Settings → General → Notifications). Off by
//! default.
//!
//! Trigger: the providers' `SyncObserver::messages_added`, which reports
//! mail that arrived through incremental sync only (never backfill or
//! reconcile). The observer only queues the ids (it must stay quick); after
//! a short pause, so a burst arrives as one batch, the queue is flushed:
//! each account's new messages are read from the store and kept when
//! - notifications are on and so is the account's switch (default on,
//!   except accounts hidden from All Inboxes: `Settings::notifies_for`);
//! - the message is in the inbox and unread, not sent, a draft, spam or
//!   trash, not Promotions or Social, and not from one of my own accounts;
//! - with "Only people I've emailed before", I've sent mail to the sender
//!   (from any account; `Store::person_summary`, indexed);
//! - it wasn't notified already (a crash replays `messages_added`);
//! - it isn't on screen: the window is focused and the UI reports (through
//!   `set_notify_context`) that the message's inbox or its thread is shown.
//!
//! Then per account: one notification per message (sender as the title,
//! subject as the body), or from `GROUP_AT` messages on one grouped
//! "5 new messages in Northwind". Only the sender and the subject, never the
//! body; neither is ever logged (ids and counts only).
//!
//! Clicking one (macOS and Linux) brings Penguin to the front and emits
//! `penguin://notification-open` {accountId, threadId}: the UI opens the
//! thread, or for a group that account's inbox. That goes through
//! notify-rust directly, the library tauri-plugin-notification itself
//! uses, because the plugin reports no clicks on desktop; each shown
//! notification waits for its click on a thread of its own (at most
//! `MAX_WAITING`; past that, notifications show without a click action).
//!
//! Permission: on macOS these are NSUserNotification notifications, which
//! need no prompt and can't report a refusal (the plugin answers
//! "granted"); if Notifications are off for Penguin in System Settings they
//! simply don't appear, so Settings says where to turn them on and offers a
//! test notification.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use penguin_core::notify::{collect, plan, Item, Note, NotificationOpen};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::CmdResult;
use crate::state::{blocking, AppState};
use crate::views::ThreadRef;

/// Payload `NotificationOpen`: a notification was clicked.
pub const EVENT_NOTIFICATION_OPEN: &str = "penguin://notification-open";

/// How long a burst may take to arrive before it's shown as one batch.
const BATCH_DELAY: Duration = Duration::from_millis(1500);
/// Messages remembered as notified (so a replay doesn't notify twice).
const RECENT: usize = 1000;
/// Notifications waiting for a click at once (each holds a thread).
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MAX_WAITING: usize = 16;

/// What the UI has on screen (`set_notify_context`): the accounts whose
/// inbox list is showing (empty when no inbox is), and the open or
/// previewed thread.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifyContext {
    pub inbox_accounts: Vec<String>,
    pub thread: Option<ThreadRef>,
}

/// Managed state: the batch being collected, what's on screen, what was
/// notified, and how many notifications wait for a click.
#[derive(Default)]
pub struct Notifier {
    inner: Mutex<Inner>,
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    waiting: AtomicUsize,
}

#[derive(Default)]
struct Inner {
    context: NotifyContext,
    pending: BTreeMap<String, Vec<String>>,
    flushing: bool,
    recent: VecDeque<(String, String)>,
}

/// Whether the message is on screen right now, so a notification would
/// only repeat it.
pub fn is_on_screen(ctx: &NotifyContext, focused: bool, account_id: &str, thread_id: &str) -> bool {
    focused
        && (ctx.inbox_accounts.iter().any(|a| a == account_id)
            || ctx
                .thread
                .as_ref()
                .is_some_and(|t| t.account_id == account_id && t.thread_id == thread_id))
}

/// The observer's hook: queue `message_ids` (new mail in `account_id`) for
/// the next batch. Quick; does nothing while notifications are off.
pub fn messages_added(app: &AppHandle, account_id: &str, message_ids: &[String]) {
    if message_ids.is_empty() {
        return;
    }
    let (Some(state), Some(notifier)) = (
        app.try_state::<Arc<AppState>>(),
        app.try_state::<Notifier>(),
    ) else {
        return;
    };
    if !state.settings.get().notifies_for(account_id) {
        return;
    }
    let start = {
        let mut inner = notifier.inner.lock().unwrap();
        inner
            .pending
            .entry(account_id.to_string())
            .or_default()
            .extend(message_ids.iter().cloned());
        !std::mem::replace(&mut inner.flushing, true)
    };
    if start {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(BATCH_DELAY).await;
            flush(&app).await;
        });
    }
}

/// Show what the batch holds (see the module docs for the rules).
async fn flush(app: &AppHandle) {
    let (Some(state), Some(notifier)) = (
        app.try_state::<Arc<AppState>>(),
        app.try_state::<Notifier>(),
    ) else {
        return;
    };
    let (pending, context) = {
        let mut inner = notifier.inner.lock().unwrap();
        inner.flushing = false;
        (std::mem::take(&mut inner.pending), inner.context.clone())
    };
    let settings = state.settings.get();
    let pending: BTreeMap<String, Vec<String>> = pending
        .into_iter()
        .filter(|(a, _)| settings.notifies_for(a))
        .collect();
    if pending.is_empty() {
        return;
    }
    let known_only = settings.notifications.known_senders_only;
    let store = state.store.clone();
    let found = blocking(move || Ok(collect(&store, pending, known_only)?)).await;
    let (found, labels) = match found {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e.message, "could not read new mail for notifications");
            return;
        }
    };
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    for (account, items) in found {
        let items: Vec<Item> = {
            let mut inner = notifier.inner.lock().unwrap();
            items
                .into_iter()
                .filter(|i| !is_on_screen(&context, focused, &account, &i.thread_id))
                .filter(|i| {
                    let key = (account.clone(), i.message_id.clone());
                    if inner.recent.contains(&key) {
                        return false;
                    }
                    inner.recent.push_back(key);
                    if inner.recent.len() > RECENT {
                        inner.recent.pop_front();
                    }
                    true
                })
                .collect()
        };
        if items.is_empty() {
            continue;
        }
        let label = labels
            .get(&account)
            .cloned()
            .unwrap_or_else(|| account.clone());
        let notes = plan(&account, &label, &items);
        tracing::info!(account = %account, messages = items.len(), notifications = notes.len(), "new-mail notifications");
        for note in notes {
            show(app, note);
        }
    }
}

/// Bring Penguin forward and tell the UI what to open.
fn open(app: &AppHandle, target: &NotificationOpen) {
    crate::app_menu::bring_to_front(app);
    if let Err(e) = app.emit(EVENT_NOTIFICATION_OPEN, target.clone()) {
        tracing::warn!(error = %e, "could not emit notification-open");
    }
}

pub(crate) fn show(app: &AppHandle, note: Note) {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let note = match show_clickable(app, note) {
        Ok(()) => return,
        Err(note) => note,
    };
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app
        .notification()
        .builder()
        .title(note.title)
        .body(note.body)
        .show()
    {
        tracing::warn!(error = %e, "could not show a notification");
    }
}

/// Show `note` on a thread that waits for its click. Gives it back when
/// too many already wait (or the thread can't start), for a plain one.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn show_clickable(app: &AppHandle, note: Note) -> Result<(), Note> {
    let Some(notifier) = app.try_state::<Notifier>() else {
        return Err(note);
    };
    if notifier.waiting.fetch_add(1, Ordering::SeqCst) >= MAX_WAITING {
        notifier.waiting.fetch_sub(1, Ordering::SeqCst);
        return Err(note);
    }
    let (title, body, target) = (note.title.clone(), note.body.clone(), note.open.clone());
    let app2 = app.clone();
    let spawned = std::thread::Builder::new()
        .name("penguin-notification".into())
        .spawn(move || {
            #[cfg(target_os = "macos")]
            {
                // What tauri-plugin-notification does before every show.
                let id = app2.config().identifier.clone();
                let _ = notify_rust::set_application(if tauri::is_dev() {
                    "com.apple.Terminal"
                } else {
                    &id
                });
            }
            let mut n = notify_rust::Notification::new();
            n.summary(&title).body(&body);
            #[cfg(target_os = "linux")]
            {
                n.appname("Penguin").action("default", "Open");
            }
            match n.show() {
                Ok(handle) => handle.wait_for_action(|action| {
                    if action == "default" {
                        if let Some(t) = &target {
                            open(&app2, t);
                        } else {
                            crate::app_menu::bring_to_front(&app2);
                        }
                    }
                }),
                Err(e) => tracing::warn!(error = %e, "could not show a notification"),
            }
            if let Some(n) = app2.try_state::<Notifier>() {
                n.waiting.fetch_sub(1, Ordering::SeqCst);
            }
        });
    if spawned.is_err() {
        notifier.waiting.fetch_sub(1, Ordering::SeqCst);
        return Err(note);
    }
    Ok(())
}

// ---------- commands ----------

/// What the UI has on screen (see `NotifyContext`). Sent when it changes.
#[tauri::command]
pub fn set_notify_context(notifier: tauri::State<'_, Notifier>, context: NotifyContext) {
    notifier.inner.lock().unwrap().context = context;
}

fn permission_name(p: tauri::plugin::PermissionState) -> &'static str {
    match p {
        tauri::plugin::PermissionState::Granted => "granted",
        tauri::plugin::PermissionState::Denied => "denied",
        _ => "prompt",
    }
}

/// "granted" | "denied" | "prompt" (macOS desktop builds always answer
/// granted: see the module docs).
#[tauri::command]
pub fn notification_permission(app: AppHandle) -> CmdResult<String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .permission_state()
        .map(|p| permission_name(p).to_string())
        .map_err(|e| crate::error::CmdError::other(e.to_string()))
}

/// Ask for permission (the first time notifications are turned on).
#[tauri::command]
pub fn request_notification_permission(app: AppHandle) -> CmdResult<String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .request_permission()
        .map(|p| permission_name(p).to_string())
        .map_err(|e| crate::error::CmdError::other(e.to_string()))
}

/// "Send a test notification": how new mail will look (clicking it just
/// brings Penguin forward).
#[tauri::command]
pub fn test_notification(app: AppHandle) {
    show(
        &app,
        Note {
            title: "Penguin".into(),
            body: "New mail will show up like this.".into(),
            open: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_only_when_focused_and_on_screen() {
        let ctx = NotifyContext {
            inbox_accounts: vec!["a@x.example".into()],
            thread: Some(ThreadRef {
                account_id: "b@x.example".into(),
                thread_id: "t9".into(),
            }),
        };
        assert!(
            is_on_screen(&ctx, true, "a@x.example", "t1"),
            "its inbox is showing"
        );
        assert!(
            is_on_screen(&ctx, true, "b@x.example", "t9"),
            "its thread is open"
        );
        assert!(
            !is_on_screen(&ctx, true, "b@x.example", "t1"),
            "another thread of b"
        );
        assert!(
            !is_on_screen(&ctx, false, "a@x.example", "t1"),
            "Penguin in the background"
        );
        assert!(!is_on_screen(
            &NotifyContext::default(),
            true,
            "a@x.example",
            "t1"
        ));
        // The UI's JSON.
        let c: NotifyContext = serde_json::from_str(
            r#"{"inboxAccounts":["a@x.example"],"thread":{"accountId":"a@x.example","threadId":"t1"}}"#,
        )
        .unwrap();
        assert_eq!(c.thread.unwrap().thread_id, "t1");
        let empty: NotifyContext = serde_json::from_str("{}").unwrap();
        assert!(empty.inbox_accounts.is_empty() && empty.thread.is_none());
    }
}
