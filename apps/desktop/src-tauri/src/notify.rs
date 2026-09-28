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

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use penguin_core::{Address, Message, Store};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::CmdResult;
use crate::state::{blocking, AppState};
use crate::views::ThreadRef;

/// Payload `NotificationOpen`: a notification was clicked.
pub const EVENT_NOTIFICATION_OPEN: &str = "penguin://notification-open";

/// How long a burst may take to arrive before it's shown as one batch.
const BATCH_DELAY: Duration = Duration::from_millis(1500);
/// From this many new messages in one account and batch, one grouped
/// notification instead of one each.
pub const GROUP_AT: usize = 3;
/// Messages remembered as notified (so a replay doesn't notify twice).
const RECENT: usize = 1000;
/// Notifications waiting for a click at once (each holds a thread).
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MAX_WAITING: usize = 16;
/// Labels that never notify.
const QUIET_LABELS: &[&str] = &[
    "SENT",
    "DRAFT",
    "SPAM",
    "TRASH",
    "CATEGORY_PROMOTIONS",
    "CATEGORY_SOCIAL",
];

/// What the UI has on screen (`set_notify_context`): the accounts whose
/// inbox list is showing (empty when no inbox is), and the open or
/// previewed thread.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifyContext {
    pub inbox_accounts: Vec<String>,
    pub thread: Option<ThreadRef>,
}

/// `penguin://notification-open`: what a click opens. No `thread_id` = the
/// account's inbox (a grouped notification).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationOpen {
    pub account_id: String,
    pub thread_id: Option<String>,
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

/// One new message worth a notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub message_id: String,
    pub thread_id: String,
    pub from: Address,
    pub subject: String,
}

/// A notification to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub title: String,
    pub body: String,
    /// None: a test notification, nothing to open.
    pub open: Option<NotificationOpen>,
}

/// Whether `m` is new mail worth telling me about (before the settings and
/// screen checks): in the inbox, unread, not a quiet label, not from me.
pub fn is_candidate(m: &Message, own: &HashSet<String>) -> bool {
    let has = |l: &str| m.label_ids.iter().any(|x| x == l);
    has("INBOX")
        && has("UNREAD")
        && !QUIET_LABELS.iter().any(|l| has(l))
        && !own.contains(&m.from.email.trim().to_lowercase())
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

/// The sender as a notification shows it: the name, else the address.
pub fn sender_label(a: &Address) -> String {
    a.name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| a.email.trim().to_string())
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The notifications for one account's batch: one per message, oldest
/// first, or from `GROUP_AT` on one "N new messages in <account>" naming
/// the first senders.
pub fn plan(account_id: &str, account_label: &str, items: &[Item]) -> Vec<Note> {
    if items.len() >= GROUP_AT {
        let mut senders: Vec<String> = Vec::new();
        for i in items {
            let s = sender_label(&i.from);
            if !senders.contains(&s) {
                senders.push(s);
            }
        }
        let body = match senders.len() {
            1 => format!("From {}", senders[0]),
            2 => format!("From {} and {}", senders[0], senders[1]),
            n => format!(
                "From {}, {} and {} other{}",
                senders[0],
                senders[1],
                n - 2,
                if n == 3 { "" } else { "s" }
            ),
        };
        return vec![Note {
            title: one_line(
                &format!("{} new messages in {account_label}", items.len()),
                120,
            ),
            body: one_line(&body, 200),
            open: Some(NotificationOpen {
                account_id: account_id.to_string(),
                thread_id: None,
            }),
        }];
    }
    items
        .iter()
        .map(|i| Note {
            title: one_line(&sender_label(&i.from), 120),
            body: if i.subject.trim().is_empty() {
                "(no subject)".into()
            } else {
                one_line(&i.subject, 200)
            },
            open: Some(NotificationOpen {
                account_id: account_id.to_string(),
                thread_id: Some(i.thread_id.clone()),
            }),
        })
        .collect()
}

/// Per account (in order) the items to notify about, and every account's
/// display name by id.
pub type Collected = (Vec<(String, Vec<Item>)>, HashMap<String, String>);

/// Each account's messages from `pending` worth a notification (stored,
/// candidates, and with `known_only` from someone I've emailed), plus every
/// account's name for grouped titles. Blocking (store reads).
pub fn collect(
    store: &Store,
    pending: BTreeMap<String, Vec<String>>,
    known_only: bool,
) -> penguin_core::Result<Collected> {
    let accounts = store.list_accounts()?;
    let own: HashSet<String> = accounts.iter().map(|a| a.email.to_lowercase()).collect();
    let labels: HashMap<String, String> = accounts
        .iter()
        .map(|a| {
            let label = a
                .nickname
                .clone()
                .or_else(|| a.display_name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| a.email.clone());
            (a.id.clone(), label)
        })
        .collect();
    let mut emailed: HashMap<String, bool> = HashMap::new();
    let mut out: Vec<(String, Vec<Item>)> = Vec::new();
    for (account, ids) in pending {
        let mut items = Vec::new();
        for id in ids {
            let Some(m) = store.get_message(&account, &id)? else {
                continue;
            };
            if !is_candidate(&m, &own) {
                continue;
            }
            if known_only {
                let sender = m.from.email.trim().to_lowercase();
                let known = match emailed.get(&sender) {
                    Some(k) => *k,
                    None => {
                        let k = store.person_summary(&sender)?.messages_to > 0;
                        emailed.insert(sender, k);
                        k
                    }
                };
                if !known {
                    continue;
                }
            }
            if items.iter().any(|i: &Item| i.message_id == m.id) {
                continue;
            }
            items.push(Item {
                message_id: m.id,
                thread_id: m.thread_id,
                from: m.from,
                subject: m.subject,
            });
        }
        if !items.is_empty() {
            out.push((account, items));
        }
    }
    Ok((out, labels))
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

fn show(app: &AppHandle, note: Note) {
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

    fn addr(name: Option<&str>, email: &str) -> Address {
        Address {
            name: name.map(str::to_string),
            email: email.into(),
        }
    }

    fn msg(labels: &[&str], from: &str) -> Message {
        Message {
            account_id: "sam@northwind.example".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 0,
            from: addr(Some("Dana Reyes"), from),
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Scope of work v3".into(),
            snippet: String::new(),
            body_text: "secret body".into(),
            body_html: None,
            label_ids: labels.iter().map(|l| l.to_string()).collect(),
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: true,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    fn item(id: &str, name: Option<&str>, email: &str, subject: &str) -> Item {
        Item {
            message_id: id.into(),
            thread_id: format!("t-{id}"),
            from: addr(name, email),
            subject: subject.into(),
        }
    }

    #[test]
    fn only_unread_inbox_mail_from_others_notifies() {
        let own = HashSet::from(["sam@northwind.example".to_string()]);
        assert!(is_candidate(
            &msg(&["INBOX", "UNREAD"], "dana@acme.example"),
            &own
        ));
        assert!(is_candidate(
            &msg(
                &["INBOX", "UNREAD", "CATEGORY_UPDATES", "IMPORTANT"],
                "dana@acme.example"
            ),
            &own
        ));
        for labels in [
            &["UNREAD"][..],
            &["INBOX"],
            &["INBOX", "UNREAD", "SPAM"],
            &["INBOX", "UNREAD", "TRASH"],
            &["INBOX", "UNREAD", "DRAFT"],
            &["INBOX", "UNREAD", "SENT"],
            &["INBOX", "UNREAD", "CATEGORY_PROMOTIONS"],
            &["INBOX", "UNREAD", "CATEGORY_SOCIAL"],
        ] {
            assert!(
                !is_candidate(&msg(labels, "dana@acme.example"), &own),
                "{labels:?}"
            );
        }
        // Mail from one of my own accounts (a note to self, a send to myself).
        assert!(!is_candidate(
            &msg(&["INBOX", "UNREAD"], " Sam@Northwind.example"),
            &own
        ));
    }

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

    #[test]
    fn one_notification_each_until_many_arrive_at_once() {
        let two = plan(
            "sam@northwind.example",
            "Northwind",
            &[
                item(
                    "m1",
                    Some("Dana Reyes"),
                    "dana@acme.example",
                    "Scope of work v3",
                ),
                item("m2", None, "lee@partner.example", "  "),
            ],
        );
        assert_eq!(
            two,
            vec![
                Note {
                    title: "Dana Reyes".into(),
                    body: "Scope of work v3".into(),
                    open: Some(NotificationOpen {
                        account_id: "sam@northwind.example".into(),
                        thread_id: Some("t-m1".into())
                    }),
                },
                Note {
                    title: "lee@partner.example".into(),
                    body: "(no subject)".into(),
                    open: Some(NotificationOpen {
                        account_id: "sam@northwind.example".into(),
                        thread_id: Some("t-m2".into())
                    }),
                },
            ]
        );
        let five: Vec<Item> = [
            ("Dana Reyes", "dana@acme.example"),
            ("Lee Park", "lee@partner.example"),
            ("Dana Reyes", "dana@acme.example"),
            ("Ops", "ops@acme.example"),
            ("Kai", "kai@acme.example"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (n, e))| item(&format!("m{i}"), Some(n), e, "s"))
        .collect();
        let grouped = plan("sam@northwind.example", "Northwind", &five);
        assert_eq!(
            grouped,
            vec![Note {
                title: "5 new messages in Northwind".into(),
                body: "From Dana Reyes, Lee Park and 2 others".into(),
                open: Some(NotificationOpen {
                    account_id: "sam@northwind.example".into(),
                    thread_id: None
                }),
            }]
        );
        let three_from_one = plan("a", "A", &vec![five[0].clone(); 3]);
        assert_eq!(three_from_one[0].body, "From Dana Reyes");
        let three = plan("a", "A", &five[1..4]);
        assert_eq!(three[0].body, "From Lee Park, Dana Reyes and 1 other");
    }

    #[test]
    fn text_is_one_line_and_bounded() {
        let long = "word ".repeat(100);
        let n = plan(
            "a",
            "A",
            &[item("m1", Some("Dana\nReyes"), "d@x.example", &long)],
        );
        assert_eq!(n[0].title, "Dana Reyes");
        assert_eq!(n[0].body.chars().count(), 200);
        assert!(n[0].body.ends_with('…'));
    }

    #[test]
    fn collect_reads_the_store_and_honors_known_senders_only() {
        const ME: &str = "sam@northwind.example";
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_account(&penguin_core::Account {
                id: ME.into(),
                email: ME.into(),
                nickname: Some("Northwind".into()),
                color: "#123456".into(),
                ..penguin_core::Account::default()
            })
            .unwrap();
        let m = |id: &str, from: &str, labels: &[&str]| {
            let mut x = msg(labels, from);
            x.id = id.into();
            x.thread_id = format!("t-{id}");
            x.date = 1_700_000_000_000 + id.len() as i64;
            x
        };
        // I wrote to Dana once; Lee is a stranger.
        let mut sent = m("s1", ME, &["SENT"]);
        sent.to = vec![addr(None, "dana@acme.example")];
        store
            .upsert_messages(&[
                sent,
                m("m1", "dana@acme.example", &["INBOX", "UNREAD"]),
                m("m2", "lee@partner.example", &["INBOX", "UNREAD"]),
                m("m3", "lee@partner.example", &["INBOX"]),
            ])
            .unwrap();
        let pending = BTreeMap::from([(
            ME.to_string(),
            vec![
                "m1".into(),
                "m2".into(),
                "m3".into(),
                "m1".into(),
                "gone".into(),
            ],
        )]);
        let (all, labels) = collect(&store, pending.clone(), false).unwrap();
        assert_eq!(labels[ME], "Northwind");
        let ids: Vec<&str> = all[0].1.iter().map(|i| i.message_id.as_str()).collect();
        assert_eq!(
            ids,
            ["m1", "m2"],
            "read mail, unknown ids and repeats are skipped"
        );
        let (known, _) = collect(&store, pending, true).unwrap();
        let ids: Vec<&str> = known[0].1.iter().map(|i| i.message_id.as_str()).collect();
        assert_eq!(ids, ["m1"], "only the sender I've emailed");
    }

    #[test]
    fn the_open_event_is_what_the_ui_reads() {
        let v = serde_json::to_value(NotificationOpen {
            account_id: "a@x.example".into(),
            thread_id: None,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"accountId": "a@x.example", "threadId": null})
        );
    }
}
