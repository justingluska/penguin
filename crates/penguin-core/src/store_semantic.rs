//! Reads for the search-by-meaning indexer (penguin-semantic; the app's
//! `semantic` module drives it). Read-only: vectors live in their own
//! database (`semantic.db`), so nothing here touches the mail schema.
//!
//! The indexer walks `messages` by rowid, newest first (rowid = date ×
//! 1024 + seq), compares each page with what `semantic.db` holds, and
//! embeds what is missing or changed. [`SemanticRow::sig`] is what
//! "changed" means: a headers-only message that later got its body.

use rusqlite::params;

use super::{Body, Store, F_BODY_PENDING, F_DRAFT, F_NEWSLETTER, F_SPAM};
use crate::Result;

/// Messages never embedded: spam, and drafts (they change on every save).
const SKIP: i64 = F_SPAM | F_DRAFT;

/// One message as the indexer's sweep sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticRow {
    pub rowid: i64,
    pub account_id: String,
    pub id: String,
    /// 1 while only headers are stored (the snippet stands in for the body),
    /// else 0. A change means "embed again".
    pub sig: i64,
}

/// What gets chunked and embedded for one message.
#[derive(Debug, Clone)]
pub struct SemanticText {
    pub rowid: i64,
    pub account_id: String,
    pub id: String,
    pub thread_id: String,
    pub date: i64,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_email: String,
    /// Quote-stripped body (`text::split_quoted`), or the snippet when only
    /// headers are stored.
    pub authored: String,
    /// Names of real (non-inline) attachments.
    pub filenames: Vec<String>,
    /// Newsletter / promotions / updates / forums mail.
    pub bulk: bool,
    pub sig: i64,
}

fn sig(flags: i64) -> i64 {
    i64::from(flags & F_BODY_PENDING != 0)
}

impl Store {
    /// Up to `limit` embeddable messages with `rowid < before`, newest
    /// first. Page with `before` = the last rowid returned.
    pub fn semantic_rows(&self, before: i64, limit: usize) -> Result<Vec<SemanticRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT rowid, account_id, id, flags FROM messages
                 WHERE rowid < ?1 AND flags & ?2 = 0 ORDER BY rowid DESC LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![before, SKIP, limit as i64], |r| {
                    Ok(SemanticRow {
                        rowid: r.get(0)?,
                        account_id: r.get(1)?,
                        id: r.get(2)?,
                        sig: sig(r.get(3)?),
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Messages the indexer should hold vectors for (the status total).
    pub fn semantic_eligible_count(&self) -> Result<u64> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM messages WHERE flags & ?1 = 0",
                [SKIP],
                |r| r.get::<_, i64>(0),
            )? as u64)
        })
    }

    /// The text of these messages (missing rowids are skipped; spam and
    /// drafts too, in case they changed since the sweep listed them).
    pub fn semantic_texts(&self, rowids: &[i64]) -> Result<Vec<SemanticText>> {
        self.read(|c| {
            let mut msg = c.prepare_cached(
                "SELECT m.account_id, m.id, t.thread_id, m.date, m.flags, m.from_name, m.from_email,
                        m.subject, m.snippet, b.body_text
                 FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
                 LEFT JOIN message_bodies b ON b.rowid = m.rowid
                 WHERE m.rowid = ?1",
            )?;
            let mut files = c.prepare_cached(
                "SELECT filename FROM attachments WHERE message_rowid = ?1 AND inline = 0 ORDER BY ord",
            )?;
            let mut out = Vec::with_capacity(rowids.len());
            for &rowid in rowids {
                let row = msg
                    .query_row([rowid], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, String>(6)?,
                            r.get::<_, String>(7)?,
                            r.get::<_, String>(8)?,
                            r.get::<_, Option<Body>>(9)?,
                        ))
                    })
                    .map(Some)
                    .or_else(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Ok(None),
                        e => Err(e),
                    })?;
                let Some((account_id, id, thread_id, date, flags, from_name, from_email, subject, snippet, body)) =
                    row
                else {
                    continue;
                };
                if flags & SKIP != 0 {
                    continue;
                }
                let authored = match body {
                    Some(Body(text)) if flags & F_BODY_PENDING == 0 => crate::text::split_quoted(&text).0,
                    _ => snippet,
                };
                let filenames = files
                    .query_map([rowid], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<_>>()?;
                out.push(SemanticText {
                    rowid,
                    account_id,
                    id,
                    thread_id,
                    date,
                    subject,
                    from_name,
                    from_email,
                    authored,
                    filenames,
                    bulk: flags & F_NEWSLETTER != 0,
                    sig: sig(flags),
                });
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{Address, AttachmentMeta, Message};
    use crate::Store;

    fn msg(id: &str, date: i64, labels: &[&str], body: &str) -> Message {
        Message {
            account_id: "acct@one.example".into(),
            id: id.into(),
            thread_id: format!("t-{id}"),
            date,
            from: Address { name: Some("Rosa Dalisay".into()), email: "rosa@harbor.example".into() },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: format!("Subject {id}"),
            snippet: format!("snippet {id}"),
            body_text: body.into(),
            body_html: None,
            label_ids: labels.iter().map(|s| s.to_string()).collect(),
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    fn att(id: &str, filename: &str, mime: &str, inline: bool) -> AttachmentMeta {
        AttachmentMeta {
            id: id.into(),
            filename: filename.into(),
            mime_type: mime.into(),
            size: 10,
            content_id: inline.then(|| "logo@x".to_string()),
            inline,
        }
    }

    #[test]
    fn lists_newest_first_skipping_spam_and_drafts_and_reads_authored_text() {
        let store = Store::open_in_memory().unwrap();
        let mut with_file = msg(
            "m1",
            1_000,
            &["INBOX"],
            "The lease is attached.\n\nOn Tue, Mar 3, 2026 at 10:00 AM Sam <sam@x.example> wrote:\n> old stuff",
        );
        with_file.attachments = vec![
            att("a1", "lease.pdf", "application/pdf", false),
            att("a2", "logo.png", "image/png", true),
        ];
        store
            .upsert_messages(&[
                with_file,
                msg("m2", 2_000, &["SPAM"], "buy now"),
                msg("m3", 3_000, &["DRAFT"], "draft text"),
                msg("m4", 4_000, &["INBOX", "CATEGORY_PROMOTIONS"], "sale"),
            ])
            .unwrap();
        let rows = store.semantic_rows(i64::MAX, 10).unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["m4", "m1"]);
        assert_eq!(store.semantic_eligible_count().unwrap(), 2);
        let page2 = store.semantic_rows(rows[0].rowid, 10).unwrap();
        assert_eq!(page2.len(), 1);
        let texts = store.semantic_texts(&[rows[1].rowid, rows[0].rowid, 12345]).unwrap();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0].authored, "The lease is attached.");
        assert_eq!(texts[0].filenames, vec!["lease.pdf".to_string()]);
        assert_eq!(texts[0].thread_id, "t-m1");
        assert!(!texts[0].bulk);
        assert!(texts[1].bulk);
        assert_eq!(texts[0].sig, 0);
    }
}
