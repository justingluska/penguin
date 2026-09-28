//! Unsubscribe memory (`unsubscribes`): senders you unsubscribed from, per
//! receiving account, so later mail from them shows "Unsubscribed". Also
//! the on-demand backfill of `List-Unsubscribe-Post` for messages stored
//! before it was kept (see `crate::unsubscribe`).

use rusqlite::{params, OptionalExtension, Transaction};

use super::{Extra, Store};
use crate::unsubscribe::{UnsubscribeMethod, UnsubscribeRecord};
use crate::Result;

pub(super) const SCHEMA_UNSUBSCRIBES: &str = r#"
CREATE TABLE unsubscribes(
    account_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    method TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY(account_id, sender)
) WITHOUT ROWID;
"#;

pub(super) fn remove_account(tx: &Transaction, account_id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM unsubscribes WHERE account_id = ?1",
        [account_id],
    )?;
    Ok(())
}

impl Store {
    /// Remember that `account_id` unsubscribed from `sender` (any case).
    pub fn record_unsubscribe(
        &self,
        account_id: &str,
        sender: &str,
        method: UnsubscribeMethod,
        at_ms: i64,
    ) -> Result<()> {
        let sender = sender.trim().to_lowercase();
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO unsubscribes(account_id, sender, method, at) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![account_id, sender, method.as_str(), at_ms])?;
            Ok(())
        })
    }

    /// When `account_id` unsubscribed from `sender`, if it did.
    pub fn unsubscribe_record(
        &self,
        account_id: &str,
        sender: &str,
    ) -> Result<Option<UnsubscribeRecord>> {
        let sender = sender.trim().to_lowercase();
        self.read(|c| {
            let row: Option<(String, i64)> = c
                .prepare_cached(
                    "SELECT method, at FROM unsubscribes WHERE account_id = ?1 AND sender = ?2",
                )?
                .query_row(params![account_id, sender], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            Ok(row.and_then(|(method, at)| {
                Some(UnsubscribeRecord {
                    at,
                    method: UnsubscribeMethod::parse(&method)?,
                })
            }))
        })
    }

    /// Store whether a message had `List-Unsubscribe-Post: One-Click`
    /// (learned from a header fetch). False when the message isn't stored.
    pub fn set_list_unsubscribe_post(
        &self,
        account_id: &str,
        message_id: &str,
        one_click: bool,
    ) -> Result<bool> {
        self.write(|tx| {
            let row: Option<(i64, String)> = tx
                .prepare_cached(
                    "SELECT m.rowid, b.extra FROM messages m JOIN message_bodies b ON b.rowid = m.rowid
                     WHERE m.account_id = ?1 AND m.id = ?2",
                )?
                .query_row(params![account_id, message_id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let Some((rowid, extra)) = row else {
                return Ok(false);
            };
            let mut extra: Extra = serde_json::from_str(&extra)?;
            extra.list_unsubscribe_post = Some(one_click);
            tx.prepare_cached("UPDATE message_bodies SET extra = ?1 WHERE rowid = ?2")?
                .execute(params![serde_json::to_string(&extra)?, rowid])?;
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::types::*;
    use crate::unsubscribe::UnsubscribeMethod;
    use crate::Store;

    fn msg() -> Message {
        Message {
            account_id: "me@example.com".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 1_700_000_000_000,
            from: Address {
                name: Some("Weekly".into()),
                email: "news@weekly.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "This week".into(),
            snippet: String::new(),
            body_text: "Hello".into(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: Some("<https://weekly.example/u/1>".into()),
            list_unsubscribe_post: None,
            sender_authenticated: true,
        }
    }

    #[test]
    fn remembers_per_account_and_sender() {
        let s = Store::open_in_memory().unwrap();
        let a = "me@example.com";
        assert!(s
            .unsubscribe_record(a, "news@weekly.example")
            .unwrap()
            .is_none());
        s.record_unsubscribe(a, "News@Weekly.example", UnsubscribeMethod::OneClick, 42)
            .unwrap();
        let r = s
            .unsubscribe_record(a, "news@weekly.example")
            .unwrap()
            .unwrap();
        assert_eq!((r.at, r.method), (42, UnsubscribeMethod::OneClick));
        assert!(s
            .unsubscribe_record("other@example.com", "news@weekly.example")
            .unwrap()
            .is_none());
        s.remove_account(a).unwrap();
        assert!(s
            .unsubscribe_record(a, "news@weekly.example")
            .unwrap()
            .is_none());
    }

    #[test]
    fn post_flag_round_trips_and_backfills() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_messages(&[msg()]).unwrap();
        let m = s.get_message("me@example.com", "m1").unwrap().unwrap();
        assert_eq!(m.list_unsubscribe_post, None);
        assert!(s
            .set_list_unsubscribe_post("me@example.com", "m1", true)
            .unwrap());
        let m = s.get_message("me@example.com", "m1").unwrap().unwrap();
        assert_eq!(m.list_unsubscribe_post, Some(true));
        assert_eq!(
            m.list_unsubscribe.as_deref(),
            Some("<https://weekly.example/u/1>")
        );
        assert!(!s
            .set_list_unsubscribe_post("me@example.com", "gone", true)
            .unwrap());
    }
}
