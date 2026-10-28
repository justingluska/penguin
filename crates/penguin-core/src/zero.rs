//! Get to zero: archive every inbox conversation older than a day, a week…
//! or all of it, in one split or the whole inbox, keeping unread or starred
//! ones if you like. This is the deciding half (which conversations a run
//! covers); archiving them is the app's usual archive with Undo.
//!
//! The ages and the keep rules are the Mac's (apps/desktop/src/features/
//! zero/zero.ts, which its dialog still uses in TypeScript); the iPhone
//! asks this module through penguin-ffi.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::types::{AccountId, ListQuery, MailboxView, SplitFilter};
use crate::{Result, Store};

const DAY_MS: i64 = 86_400_000;
/// Rows read per page while collecting.
const PAGE: u32 = 1000;

/// "Older than…": the choices, in the order the dialog shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ZeroAge {
    Day,
    Week,
    TwoWeeks,
    Month,
    ThreeMonths,
    /// Everything in the inbox (or split).
    All,
}

impl ZeroAge {
    pub const ALL: [ZeroAge; 6] = [
        ZeroAge::Day,
        ZeroAge::Week,
        ZeroAge::TwoWeeks,
        ZeroAge::Month,
        ZeroAge::ThreeMonths,
        ZeroAge::All,
    ];

    /// "Older than two weeks".
    pub fn label(self) -> &'static str {
        match self {
            ZeroAge::Day => "Older than a day",
            ZeroAge::Week => "Older than a week",
            ZeroAge::TwoWeeks => "Older than two weeks",
            ZeroAge::Month => "Older than a month",
            ZeroAge::ThreeMonths => "Older than three months",
            ZeroAge::All => "Everything",
        }
    }

    /// Conversations whose latest inbox message is before this (unix ms)
    /// are covered; None = all of them.
    pub fn cutoff(self, now_ms: i64) -> Option<i64> {
        let days = match self {
            ZeroAge::Day => 1,
            ZeroAge::Week => 7,
            ZeroAge::TwoWeeks => 14,
            ZeroAge::Month => 30,
            ZeroAge::ThreeMonths => 91,
            ZeroAge::All => return None,
        };
        Some(now_ms - days * DAY_MS)
    }
}

/// What a Get to zero run would archive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroPlan {
    /// (account, thread), newest first, no duplicates.
    pub targets: Vec<(AccountId, String)>,
    /// Old enough but kept (unread or starred, as asked).
    pub kept: u32,
}

impl Store {
    /// The inbox conversations (of `split`, or the whole inbox) in `scope`
    /// whose latest inbox message is older than `age`, minus the unread or
    /// starred ones when `keep_unread` / `keep_starred`. Local reads only.
    pub fn get_to_zero(
        &self,
        split: Option<&SplitFilter>,
        scope: Option<&[AccountId]>,
        age: ZeroAge,
        now_ms: i64,
        keep_unread: bool,
        keep_starred: bool,
    ) -> Result<ZeroPlan> {
        let mut plan = ZeroPlan::default();
        let mut seen = HashSet::new();
        let mut before = age.cutoff(now_ms);
        loop {
            let page = self.list_threads(&ListQuery {
                view: MailboxView::Inbox,
                tab: None,
                account_id: None,
                account_ids: scope.map(<[AccountId]>::to_vec),
                limit: PAGE,
                before,
                unread_only: false,
                split: split.cloned(),
            })?;
            for t in &page {
                if (keep_unread && t.unread) || (keep_starred && t.starred) {
                    plan.kept += 1;
                    continue;
                }
                if seen.insert((t.account_id.clone(), t.thread_id.clone())) {
                    plan.targets
                        .push((t.account_id.clone(), t.thread_id.clone()));
                }
            }
            let Some(last) = page.last().map(|t| t.last_date) else {
                break;
            };
            // A short page is the end; a page of one timestamp can't move on.
            if page.len() < PAGE as usize || before == Some(last) {
                break;
            }
            before = Some(last);
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Address, Message};

    const A: &str = "ada@example.com";
    const NOW: i64 = 1_800_000_000_000;

    fn msg(id: &str, thread: &str, days_ago: i64, labels: &[&str]) -> Message {
        Message {
            account_id: A.into(),
            id: id.into(),
            thread_id: thread.into(),
            date: NOW - days_ago * DAY_MS - 1,
            from: Address {
                name: Some("Priya Raman".into()),
                email: "priya@northwind.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: format!("Subject {thread}"),
            snippet: String::new(),
            body_text: "hello".into(),
            body_html: None,
            label_ids: labels.iter().map(|l| l.to_string()).collect(),
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&crate::types::Account {
            id: A.into(),
            email: A.into(),
            ..Default::default()
        })
        .unwrap();
        s.upsert_messages(&[
            msg("m1", "fresh", 0, &["INBOX"]),
            msg("m2", "week", 8, &["INBOX"]),
            msg("m3", "unread", 20, &["INBOX", "UNREAD"]),
            msg("m4", "starred", 40, &["INBOX", "STARRED"]),
            msg("m5", "old", 100, &["INBOX"]),
            msg("m6", "done", 100, &[]),
        ])
        .unwrap();
        s
    }

    fn threads(p: &ZeroPlan) -> Vec<&str> {
        p.targets.iter().map(|t| t.1.as_str()).collect()
    }

    #[test]
    fn ages_cover_older_conversations_and_keep_what_was_asked() {
        let s = store();
        let p = s
            .get_to_zero(None, None, ZeroAge::Week, NOW, false, false)
            .unwrap();
        assert_eq!(threads(&p), ["week", "unread", "starred", "old"]);
        assert_eq!(p.kept, 0);

        let p = s
            .get_to_zero(None, None, ZeroAge::Week, NOW, true, true)
            .unwrap();
        assert_eq!(threads(&p), ["week", "old"]);
        assert_eq!(p.kept, 2);

        let p = s
            .get_to_zero(None, None, ZeroAge::ThreeMonths, NOW, false, true)
            .unwrap();
        assert_eq!(threads(&p), ["old"]);

        let all = s
            .get_to_zero(None, None, ZeroAge::All, NOW, false, false)
            .unwrap();
        assert_eq!(all.targets.len(), 5, "everything in the inbox, not Done");

        let none = s
            .get_to_zero(None, Some(&[]), ZeroAge::All, NOW, false, false)
            .unwrap();
        assert!(none.targets.is_empty());
    }

    #[test]
    fn a_split_covers_only_its_own_conversations() {
        let s = store();
        let starred = SplitFilter {
            include: Some("is:starred".into()),
            exclude: vec![],
        };
        let p = s
            .get_to_zero(Some(&starred), None, ZeroAge::Day, NOW, false, false)
            .unwrap();
        assert_eq!(threads(&p), ["starred"]);
        let other = SplitFilter {
            include: None,
            exclude: vec!["is:starred".into()],
        };
        let p = s
            .get_to_zero(Some(&other), None, ZeroAge::Day, NOW, false, false)
            .unwrap();
        assert_eq!(threads(&p), ["week", "unread", "old"]);
    }

    #[test]
    fn cutoffs_match_the_mac() {
        assert_eq!(ZeroAge::Day.cutoff(NOW), Some(NOW - DAY_MS));
        assert_eq!(ZeroAge::Month.cutoff(NOW), Some(NOW - 30 * DAY_MS));
        assert_eq!(ZeroAge::ThreeMonths.cutoff(NOW), Some(NOW - 91 * DAY_MS));
        assert_eq!(ZeroAge::All.cutoff(NOW), None);
        assert_eq!(ZeroAge::ALL.len(), 6);
    }
}
