//! Repair of attachments stored as `inline` that the body can't draw.
//!
//! Before [`Message::settle_inline`](crate::types::Message::settle_inline),
//! providers marked a part inline from its disposition alone, so a PDF sent
//! as `Content-Disposition: inline` (Apple Mail's way) was stored inline: not
//! listed in the thread, not counted by `has:attachment`, and never drawn.
//! The migration queues every message holding such a part; the repair loads
//! each one, settles it and stores it again, which recomputes the attachment
//! rows, flags, full-text rows and thread aggregates like a fresh sync.

use std::collections::HashSet;

use rusqlite::OptionalExtension;

use super::{insert_message, load_messages, refresh_thread, Store, F_BODY_PENDING};
use crate::types::AccountId;
use crate::Result;

/// Queue of messages to repair. Only parts that can't be body images
/// (no Content-ID, or not an image) qualify, which is what the old rules got
/// wrong; inline images stay as they were.
pub(super) const SCHEMA_INLINE_REPAIR: &str = "
CREATE TABLE inline_repair(message_rowid INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO inline_repair(message_rowid)
    SELECT message_rowid FROM attachments
    WHERE inline = 1 AND (content_id IS NULL OR lower(mime_type) NOT LIKE 'image/%');
";

/// Messages stored again per write transaction, so a sync batch never waits long.
const BATCH: usize = 100;

impl Store {
    /// Work through the repair queue. Cheap once done (an empty table), so it
    /// can run on every start. Returns the (account id, thread id) of the
    /// threads that changed, for a mail-changed event.
    pub fn repair_inline_attachments(&self) -> Result<Vec<(AccountId, String)>> {
        let mut changed = Vec::new();
        loop {
            let n = self.write(|tx| {
                let rowids: Vec<i64> = tx
                    .prepare_cached("SELECT message_rowid FROM inline_repair LIMIT ?1")?
                    .query_map([BATCH as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut touched = HashSet::new();
                let mut flags_of =
                    tx.prepare_cached("SELECT flags FROM messages WHERE rowid = ?1")?;
                let mut done =
                    tx.prepare_cached("DELETE FROM inline_repair WHERE message_rowid = ?1")?;
                for &rowid in &rowids {
                    // A message deleted since the migration just leaves the queue.
                    let flags: Option<i64> =
                        flags_of.query_row([rowid], |r| r.get(0)).optional()?;
                    if let (Some(flags), Some(mut m)) = (flags, load_messages(tx, &[rowid])?.pop())
                    {
                        m.settle_inline();
                        insert_message(tx, &m, flags & F_BODY_PENDING != 0, &mut touched)?;
                    }
                    done.execute([rowid])?;
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
    use super::*;
    use crate::types::{Address, AttachmentMeta, Message};

    fn part(id: &str, name: &str, mime: &str, cid: Option<&str>, inline: bool) -> AttachmentMeta {
        AttachmentMeta {
            id: id.into(),
            filename: name.into(),
            mime_type: mime.into(),
            size: 1_000,
            content_id: cid.map(Into::into),
            inline,
        }
    }

    fn msg(id: &str, html: &str, attachments: Vec<AttachmentMeta>) -> Message {
        Message {
            account_id: "a".into(),
            id: id.into(),
            thread_id: format!("t-{id}"),
            date: 1_790_000_000_000,
            from: Address {
                name: Some("Ana Ruiz".into()),
                email: "ana@ruiz.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Lease".into(),
            snippet: "Signed copy attached".into(),
            body_text: "Signed copy attached".into(),
            body_html: Some(html.into()),
            label_ids: vec!["INBOX".into()],
            attachments,
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    fn apple_mail_pdf() -> Message {
        msg(
            "m1",
            r#"<div>Signed copy attached</div><img src="cid:pdf-1@ruiz.example"><img src="cid:logo@ruiz.example">"#,
            vec![
                part(
                    "p1",
                    "Lease_signed.pdf",
                    "application/pdf",
                    Some("pdf-1@ruiz.example"),
                    true,
                ),
                part(
                    "p2",
                    "logo.png",
                    "image/png",
                    Some("logo@ruiz.example"),
                    true,
                ),
            ],
        )
    }

    #[test]
    fn settle_lists_everything_the_body_cannot_draw() {
        let mut m = apple_mail_pdf();
        m.attachments
            .push(part("p3", "notes.txt", "text/plain", None, true));
        m.attachments
            .push(part("p4", "photo.jpg", "image/jpeg", None, true));
        m.settle_inline();
        let inline: Vec<_> = m
            .attachments
            .iter()
            .map(|a| (a.filename.as_str(), a.inline))
            .collect();
        assert_eq!(
            inline,
            [
                ("Lease_signed.pdf", false),
                ("logo.png", true),
                ("notes.txt", false),
                ("photo.jpg", false)
            ]
        );

        // Without `cid:` in the HTML nothing is drawn, so nothing stays inline.
        let mut m = apple_mail_pdf();
        m.body_html = Some("<p>no references</p>".into());
        m.settle_inline();
        assert!(m.attachments.iter().all(|a| !a.inline));

        // A part the provider called an attachment stays one.
        let mut m = msg(
            "m2",
            r#"<img src="cid:x">"#,
            vec![part("p1", "x.png", "image/png", Some("x"), false)],
        );
        m.settle_inline();
        assert!(!m.attachments[0].inline);
    }

    fn inbox_has_attachments(store: &Store) -> bool {
        let q = crate::types::ListQuery {
            view: crate::types::MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 10,
            before: None,
            unread_only: false,
            split: None,
        };
        store
            .list_threads(&q)
            .unwrap()
            .pop()
            .unwrap()
            .has_attachments
    }

    #[test]
    fn repair_lists_a_stored_inline_pdf() {
        let store = Store::open_in_memory().unwrap();
        // Stored the old way (the store keeps what it's given): the PDF
        // inline, so the thread shows no attachment.
        store.upsert_messages(&[apple_mail_pdf()]).unwrap();
        assert!(!inbox_has_attachments(&store));

        // What the migration does to a database holding it.
        let queue = SCHEMA_INLINE_REPAIR.replace("CREATE TABLE", "CREATE TABLE IF NOT EXISTS");
        store.write(|tx| Ok(tx.execute_batch(&queue)?)).unwrap();
        let changed = store.repair_inline_attachments().unwrap();
        assert_eq!(changed, vec![("a".to_string(), "t-m1".to_string())]);

        let m = store.get_message("a", "m1").unwrap().unwrap();
        let inline: Vec<_> = m
            .attachments
            .iter()
            .map(|a| (a.filename.as_str(), a.inline))
            .collect();
        assert_eq!(inline, [("Lease_signed.pdf", false), ("logo.png", true)]);
        assert!(inbox_has_attachments(&store));
        // Done: a second run finds an empty queue.
        assert!(store.repair_inline_attachments().unwrap().is_empty());
    }
}
