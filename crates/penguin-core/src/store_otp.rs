//! Verification-code backfill. New mail gets `messages.otp` at ingest
//! (insert_message); rows stored before the column existed are NULL and are
//! scanned here, newest window only, since only recent codes are useful.

use std::collections::HashSet;

use super::{load_messages, otp_column, refresh_thread, Store, ROWID_SLOTS};
use crate::types::AccountId;
use crate::Result;

/// Messages scanned per write transaction, so a sync batch never waits long.
const BATCH: usize = 200;

impl Store {
    /// Detect codes in not-yet-scanned messages dated `since_ms` or later and
    /// refresh the threads that gained one. Idempotent and cheap once done
    /// (a rowid range scan finding no NULLs), so it can run on every start.
    /// Returns the (account id, thread id) of threads that gained a code, for
    /// a mail-changed event.
    pub fn backfill_otp(&self, since_ms: i64) -> Result<Vec<(AccountId, String)>> {
        let lo = since_ms.max(0).saturating_mul(ROWID_SLOTS);
        let mut changed = Vec::new();
        loop {
            let n = self.write(|tx| {
                let rowids: Vec<i64> = tx
                    .prepare_cached(
                        "SELECT rowid FROM messages WHERE rowid >= ?1 AND otp IS NULL
                         ORDER BY rowid DESC LIMIT ?2",
                    )?
                    .query_map(rusqlite::params![lo, BATCH as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut touched = HashSet::new();
                let mut set = tx.prepare_cached("UPDATE messages SET otp = ?1 WHERE rowid = ?2")?;
                let mut thread_of =
                    tx.prepare_cached("SELECT thread_rowid FROM messages WHERE rowid = ?1")?;
                for &rowid in &rowids {
                    // A row load_messages can't join is marked scanned too.
                    let otp = load_messages(tx, &[rowid])?
                        .pop()
                        .and_then(|m| crate::otp::detect_message(&m));
                    set.execute(rusqlite::params![otp_column(otp.as_ref())?, rowid])?;
                    if otp.is_some() {
                        touched.insert(thread_of.query_row([rowid], |r| r.get::<_, i64>(0))?);
                    }
                }
                let mut ids = tx
                    .prepare_cached("SELECT account_id, thread_id FROM threads WHERE rowid = ?1")?;
                for t in touched {
                    refresh_thread(tx, t)?;
                    changed.push(ids.query_row([t], |r| Ok((r.get(0)?, r.get(1)?)))?);
                }
                Ok(rowids.len())
            })?;
            if n < BATCH {
                return Ok(changed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::types::*;
    use crate::Store;

    fn msg(id: &str, date: i64, subject: &str, body: &str) -> Message {
        Message {
            account_id: "me@example.com".into(),
            id: id.into(),
            thread_id: "t1".into(),
            date,
            from: Address {
                name: Some("Rydeo".into()),
                email: "no-reply@rydeo.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: subject.into(),
            snippet: body.chars().take(100).collect(),
            body_text: body.into(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: true,
        }
    }

    fn inbox_otp(store: &Store) -> Option<Otp> {
        let q = ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 10,
            before: None,
            unread_only: false,
            split: None,
        };
        store.list_threads(&q).unwrap().pop().unwrap().otp
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
    const DAY: i64 = 86_400_000;

    #[test]
    fn newest_message_code_is_on_the_thread() {
        let store = Store::open_in_memory().unwrap();
        let t = now() - 60_000;
        store
            .upsert_messages(&[msg(
                "m1",
                t,
                "0357",
                "A one-time Rydeo code has been created for you.",
            )])
            .unwrap();
        let otp = inbox_otp(&store).unwrap();
        assert_eq!(otp.code.as_deref(), Some("0357"));
        assert!(otp.verified);
        assert_eq!(otp.date, t);
        // A newer message without a code replaces it.
        store
            .upsert_messages(&[msg("m2", t + 1_000, "Thanks", "Your ride is on the way.")])
            .unwrap();
        assert_eq!(inbox_otp(&store), None);
    }

    #[test]
    fn a_trashed_newer_message_doesnt_hide_the_code() {
        let store = Store::open_in_memory().unwrap();
        let t = now() - 60_000;
        let mut newer = msg("m2", t + 1_000, "Thanks", "Your ride is on the way.");
        newer.label_ids = vec!["TRASH".into()];
        store
            .upsert_messages(&[
                msg("m1", t, "Your code", "Your verification code is 482913."),
                newer,
            ])
            .unwrap();
        assert_eq!(
            inbox_otp(&store).and_then(|o| o.code).as_deref(),
            Some("482913")
        );
    }

    #[test]
    fn old_mail_is_left_for_backfill_which_is_idempotent() {
        let store = Store::open_in_memory().unwrap();
        let (old, recent) = (now() - 40 * DAY, now() - 5 * DAY);
        store
            .upsert_messages(&[
                msg("old", old, "Your code", "Your verification code is 482913."),
                msg(
                    "new",
                    recent,
                    "Your code",
                    "Your verification code is 551902.",
                ),
            ])
            .unwrap();
        // Older than the ingest window: not scanned yet.
        assert_eq!(inbox_otp(&store), None);
        assert_eq!(
            store.backfill_otp(now() - 30 * DAY).unwrap(),
            vec![("me@example.com".to_string(), "t1".to_string())]
        );
        assert_eq!(
            inbox_otp(&store).and_then(|o| o.code).as_deref(),
            Some("551902")
        );
        assert!(store.backfill_otp(now() - 30 * DAY).unwrap().is_empty());
        let old: Option<String> = store
            .read(|c| {
                Ok(
                    c.query_row("SELECT otp FROM messages WHERE id = 'old'", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(old, None);
    }

    #[test]
    fn invoices_orders_phones_and_years_arent_codes() {
        let store = Store::open_in_memory().unwrap();
        let t = now() - 60_000;
        store
            .upsert_messages(&[msg(
                "m1",
                t,
                "Invoice 20931 for order 48213",
                "Invoice number: 20931. Order 482131 ships in 2026. Questions? Call (954) 555-0142.",
            )])
            .unwrap();
        assert_eq!(inbox_otp(&store), None);
    }
}
