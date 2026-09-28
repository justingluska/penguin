//! Read receipts that came back (`receipts.rs` parses them): one row per
//! receipt email, keyed by that email, naming the Message-ID it is about.
//! The sent message's view looks its receipts up by its own Message-ID.
//!
//! A feature-owned schema (`Store::migrate_provider_schema("receipts", …)`),
//! so it versions on its own and its rows go with the account. The partial
//! index over `attachments` lets the scanner find receipt emails without
//! reading the whole table.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{Store, F_SENT, ROWID_SLOTS};
use crate::receipts::Mdn;
use crate::types::AttachmentMeta;
use crate::Result;

pub const RECEIPTS_SCHEMA: &str = "receipts";

/// `original` is the bare Message-ID the receipt is about ('' when the part
/// didn't parse, so the scanner doesn't try again); `recipient` '' when unknown.
const RECEIPTS_V1: &str = r#"
CREATE TABLE receipts_mdn(
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    original TEXT NOT NULL,
    recipient TEXT NOT NULL,
    disposition TEXT NOT NULL,
    date INTEGER NOT NULL,
    PRIMARY KEY(account_id, message_id)
) WITHOUT ROWID;
CREATE INDEX receipts_mdn_original ON receipts_mdn(account_id, original) WHERE original != '';
CREATE INDEX attachments_mdn ON attachments(message_rowid) WHERE mime_type = 'message/disposition-notification';
"#;

/// A received email with a receipt part the scanner hasn't read yet.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptCandidate {
    pub account_id: String,
    pub message_id: String,
    pub date: i64,
    pub part: AttachmentMeta,
}

/// A receipt as shown on the message it is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadReceipt {
    /// Bare Message-ID of the sent message.
    pub original_message_id: String,
    /// Who sent the receipt (lowercased), when it says.
    pub recipient: Option<String>,
    /// `displayed` = read; also `deleted`, `dispatched`, `processed`.
    pub disposition: String,
    /// When the receipt email arrived (unix ms).
    pub date: i64,
    /// The receipt email itself.
    pub message_id: String,
}

impl Store {
    /// Create or update the receipts schema. Call once at startup (writes).
    pub fn migrate_receipts(&self) -> Result<()> {
        self.migrate_provider_schema(RECEIPTS_SCHEMA, &[RECEIPTS_V1], &["receipts_mdn"])
    }

    /// Emails with a `message/disposition-notification` part and no
    /// `receipts_mdn` row yet, newest first.
    pub fn receipt_scan_candidates(&self, limit: usize) -> Result<Vec<ReceiptCandidate>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT m.account_id, m.id, m.date, a.att_id, a.filename, a.mime_type, a.size, a.content_id, a.inline
                 FROM attachments a INDEXED BY attachments_mdn
                 JOIN messages m ON m.rowid = a.message_rowid
                 WHERE a.mime_type = 'message/disposition-notification'
                   AND NOT EXISTS (SELECT 1 FROM receipts_mdn r WHERE r.account_id = m.account_id AND r.message_id = m.id)
                 ORDER BY a.message_rowid DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit as i64], |r| {
                    Ok(ReceiptCandidate {
                        account_id: r.get(0)?,
                        message_id: r.get(1)?,
                        date: r.get(2)?,
                        part: AttachmentMeta {
                            id: r.get(3)?,
                            filename: r.get(4)?,
                            mime_type: r.get(5)?,
                            size: r.get::<_, i64>(6)? as u64,
                            content_id: r.get(7)?,
                            inline: r.get(8)?,
                        },
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            // One row per email even if it has two receipt parts.
            let mut seen = std::collections::HashSet::new();
            Ok(rows
                .into_iter()
                .filter(|c| seen.insert((c.account_id.clone(), c.message_id.clone())))
                .collect())
        })
    }

    /// Record what a receipt email says (None: nothing usable, don't retry).
    /// Returns the thread of the sent message it is about when that message
    /// is stored (a sent message of the same account from the year before
    /// the receipt), so the caller can refresh it.
    pub fn put_receipt(
        &self,
        account_id: &str,
        message_id: &str,
        date: i64,
        mdn: Option<&Mdn>,
    ) -> Result<Option<String>> {
        let (original, recipient, disposition) = match mdn {
            Some(m) => (
                m.original_message_id.as_str(),
                m.recipient.as_deref().unwrap_or(""),
                m.disposition.as_str(),
            ),
            None => ("", "", ""),
        };
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO receipts_mdn(account_id, message_id, original, recipient, disposition, date)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![account_id, message_id, original, recipient, disposition, date])?;
            Ok(())
        })?;
        if original.is_empty() {
            return Ok(None);
        }
        // The Message-ID lives in the body's JSON, so look only at this
        // account's sent mail from the year before the receipt: a rowid
        // range (rowid = date × ROWID_SLOTS + seq). Receipts are rare.
        let hi = date.max(0).saturating_add(1).saturating_mul(ROWID_SLOTS);
        let lo = (date - 366 * 24 * 3600 * 1000).max(0).saturating_mul(ROWID_SLOTS);
        self.read(|c| {
            let found = c
                .prepare_cached(
                    "SELECT t.thread_id FROM messages m
                     JOIN message_bodies b ON b.rowid = m.rowid
                     JOIN threads t ON t.rowid = m.thread_rowid
                     WHERE m.rowid BETWEEN ?1 AND ?2 AND m.account_id = ?3 AND (m.flags & ?4) != 0
                       AND json_extract(b.extra, '$.message_id_header') = ?5
                     LIMIT 1",
                )?
                .query_row(params![lo, hi, account_id, F_SENT, original], |r| r.get(0))
                .ok();
            Ok(found)
        })
    }

    /// Receipts for these sent messages (bare Message-IDs) of one account,
    /// oldest first.
    pub fn receipts_for(&self, account_id: &str, original_ids: &[String]) -> Result<Vec<ReadReceipt>> {
        if original_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT original, recipient, disposition, date, message_id FROM receipts_mdn
                 WHERE account_id = ?1 AND original = ?2 AND original != '' ORDER BY date",
            )?;
            let mut out = Vec::new();
            for id in original_ids {
                let rows = stmt
                    .query_map(params![account_id, id], |r| {
                        let recipient: String = r.get(1)?;
                        Ok(ReadReceipt {
                            original_message_id: r.get(0)?,
                            recipient: (!recipient.is_empty()).then_some(recipient),
                            disposition: r.get(2)?,
                            date: r.get(3)?,
                            message_id: r.get(4)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                out.extend(rows);
            }
            out.sort_by_key(|r| r.date);
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Address, Message};

    fn msg(id: &str, date: i64, labels: &[&str], mid: Option<&str>, parts: Vec<AttachmentMeta>) -> Message {
        Message {
            account_id: "sam@mail.example".into(),
            id: id.into(),
            thread_id: format!("t-{id}"),
            date,
            from: Address { name: None, email: "sam@mail.example".into() },
            to: vec![Address { name: None, email: "alex@harbor.example".into() }],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Plans".into(),
            snippet: String::new(),
            body_text: "hi".into(),
            body_html: None,
            label_ids: labels.iter().map(|s| s.to_string()).collect(),
            attachments: parts,
            message_id_header: mid.map(str::to_string),
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    fn mdn_part() -> AttachmentMeta {
        AttachmentMeta {
            id: "part:2".into(),
            filename: "untitled.bin".into(),
            mime_type: "message/disposition-notification".into(),
            size: 300,
            content_id: None,
            inline: false,
        }
    }

    #[test]
    fn scans_receipts_and_finds_the_sent_message() {
        let s = Store::open_in_memory().unwrap();
        s.migrate_receipts().unwrap();
        // Idempotent.
        s.migrate_receipts().unwrap();
        let day = 24 * 3600 * 1000;
        let sent_at = 1_790_000_000_000;
        s.upsert_messages(&[
            msg("sent1", sent_at, &["SENT"], Some("abc@mail.example"), vec![]),
            msg("r1", sent_at + day, &["INBOX", "UNREAD"], Some("r1@harbor.example"), vec![mdn_part()]),
            msg("plain", sent_at + day, &["INBOX"], None, vec![]),
        ])
        .unwrap();
        let c = s.receipt_scan_candidates(10).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].message_id, "r1");
        assert_eq!(c[0].part.id, "part:2");

        let mdn = Mdn {
            original_message_id: "abc@mail.example".into(),
            recipient: Some("alex@harbor.example".into()),
            disposition: "displayed".into(),
            action_mode: Some("manual-action".into()),
        };
        let thread = s.put_receipt("sam@mail.example", "r1", sent_at + day, Some(&mdn)).unwrap();
        assert_eq!(thread.as_deref(), Some("t-sent1"));
        assert!(s.receipt_scan_candidates(10).unwrap().is_empty());

        let got = s.receipts_for("sam@mail.example", &["abc@mail.example".into()]).unwrap();
        assert_eq!(
            got,
            vec![ReadReceipt {
                original_message_id: "abc@mail.example".into(),
                recipient: Some("alex@harbor.example".into()),
                disposition: "displayed".into(),
                date: sent_at + day,
                message_id: "r1".into(),
            }]
        );
        // Another account's receipts, or an unknown id: nothing.
        assert!(s.receipts_for("other@mail.example", &["abc@mail.example".into()]).unwrap().is_empty());
        assert!(s.receipts_for("sam@mail.example", &[]).unwrap().is_empty());
    }

    #[test]
    fn unusable_receipts_are_not_retried_and_match_nothing() {
        let s = Store::open_in_memory().unwrap();
        s.migrate_receipts().unwrap();
        s.upsert_messages(&[msg("r2", 1_790_000_000_000, &["INBOX"], None, vec![mdn_part()])]).unwrap();
        assert_eq!(s.put_receipt("sam@mail.example", "r2", 1_790_000_000_000, None).unwrap(), None);
        assert!(s.receipt_scan_candidates(10).unwrap().is_empty());
        assert!(s.receipts_for("sam@mail.example", &["".into()]).unwrap().is_empty());
    }

    #[test]
    fn a_receipt_for_mail_not_stored_is_kept_without_a_thread() {
        let s = Store::open_in_memory().unwrap();
        s.migrate_receipts().unwrap();
        let mdn = Mdn {
            original_message_id: "gone@mail.example".into(),
            recipient: None,
            disposition: "displayed".into(),
            action_mode: None,
        };
        assert_eq!(s.put_receipt("sam@mail.example", "r3", 1_790_000_000_000, Some(&mdn)).unwrap(), None);
        assert_eq!(s.receipts_for("sam@mail.example", &["gone@mail.example".into()]).unwrap().len(), 1);
    }
}
