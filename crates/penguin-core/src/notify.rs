//! New-mail notifications: which new messages are worth one and what it
//! says. Shared by the Mac (apps/desktop/src-tauri/src/notify.rs, which
//! batches `SyncObserver::messages_added` and shows them) and the iPhone
//! (crates/penguin-ffi notify.rs, after a background refresh): in the inbox
//! and unread, not sent, a draft, spam or trash, not Promotions or Social,
//! not from one of my own accounts, with "only people I've emailed" from a
//! sender I've written to; one notification each, or from [`GROUP_AT`] in one
//! account one grouped "5 new messages in Northwind". Only the sender and
//! the subject, never the body.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{Address, Message, Store};

/// From this many new messages in one account and batch, one grouped
/// notification instead of one each.
pub const GROUP_AT: usize = 3;
/// Labels that never notify.
const QUIET_LABELS: &[&str] = &[
    "SENT",
    "DRAFT",
    "SPAM",
    "TRASH",
    "CATEGORY_PROMOTIONS",
    "CATEGORY_SOCIAL",
];

/// Bound on the per-account notification switches.
pub const MAX_NOTIFY_ACCOUNTS: usize = 100;

/// The settings (`Settings.notifications`, the same JSON on the Mac and the
/// iPhone): new-mail notifications, off by default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationSettings {
    /// "Notify me about new mail". Off by default.
    pub enabled: bool,
    /// Per-account switches (account id → on). An account not listed
    /// follows the default: on, except accounts hidden from All Inboxes.
    #[serde(deserialize_with = "lenient_bool_map")]
    pub accounts: BTreeMap<String, bool>,
    /// Only mail from people I've emailed before (from any account).
    pub known_senders_only: bool,
}

/// A map of booleans; entries that aren't booleans are skipped, and a value
/// that isn't an object reads as empty.
pub fn lenient_bool_map<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, bool>, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
                .collect()
        })
        .unwrap_or_default())
}

impl NotificationSettings {
    /// Account switches cleaned for storing: ids trimmed and lowercased,
    /// only addresses (≤320 characters), at most [`MAX_NOTIFY_ACCOUNTS`].
    pub fn normalize(&mut self) {
        let accounts = std::mem::take(&mut self.accounts);
        self.accounts = accounts
            .into_iter()
            .map(|(k, v)| (k.trim().to_lowercase(), v))
            .filter(|(k, _)| k.contains('@') && k.len() <= 320)
            .take(MAX_NOTIFY_ACCOUNTS)
            .collect();
    }

    /// Whether new mail in `account_id` may notify: the master switch, then
    /// the account's own switch or its default (on unless the account is in
    /// `hidden_from_all`).
    pub fn notifies_for(&self, account_id: &str, hidden_from_all: &[String]) -> bool {
        self.enabled
            && self
                .accounts
                .get(account_id)
                .copied()
                .unwrap_or_else(|| !hidden_from_all.iter().any(|h| h == account_id))
    }
}

/// `penguin://notification-open`: what a click opens. No `thread_id` = the
/// account's inbox (a grouped notification).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationOpen {
    pub account_id: String,
    pub thread_id: Option<String>,
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
) -> crate::Result<Collected> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_settings_read_leniently_clean_up_and_decide() {
        let mut n: NotificationSettings = serde_json::from_str(
            r#"{"enabled": true, "accounts": {" Bo@Example.com": false, "x": true, "a@x.example": 3, "k@x.example": true}}"#,
        )
        .unwrap();
        n.normalize();
        assert_eq!(
            n.accounts,
            BTreeMap::from([
                ("bo@example.com".to_string(), false),
                ("k@x.example".to_string(), true)
            ])
        );
        let hidden = vec!["h@x.example".to_string(), "k@x.example".to_string()];
        assert!(!n.notifies_for("bo@example.com", &hidden), "its own switch");
        assert!(n.notifies_for("a@x.example", &hidden), "on by default");
        assert!(
            !n.notifies_for("h@x.example", &hidden),
            "hidden from All: off"
        );
        assert!(
            n.notifies_for("k@x.example", &hidden),
            "hidden, switched on"
        );
        n.enabled = false;
        assert!(!n.notifies_for("k@x.example", &hidden), "the master switch");
        let junk: NotificationSettings =
            serde_json::from_str(r#"{"accounts": [1, 2], "enabled": false}"#).unwrap();
        assert!(junk.accounts.is_empty());
        let mut many = NotificationSettings::default();
        many.accounts = (0..150)
            .map(|i| (format!("a{i}@x.example"), true))
            .collect();
        many.normalize();
        assert_eq!(many.accounts.len(), MAX_NOTIFY_ACCOUNTS);
    }

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
            .upsert_account(&crate::Account {
                id: ME.into(),
                email: ME.into(),
                nickname: Some("Northwind".into()),
                color: "#123456".into(),
                ..crate::Account::default()
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
