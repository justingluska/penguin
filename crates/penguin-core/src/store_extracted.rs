//! Structured facts (`structured/`) stored per message, for Ask.
//!
//! `messages.extracted` is NULL until the message has been read by the
//! extractors, then the extractor version. New and re-stored messages start
//! NULL, so one background scanner covers the first backfill, every sync
//! and body downloads alike (`Store::extract_pending`, newest first, in
//! small batches that never hold the writer long). A trigger drops a
//! message's rows when the message goes.

use rusqlite::{params, Connection, OptionalExtension};

use super::{Body, Store, F_DRAFT, F_NEWSLETTER, F_SENT};
use crate::structured::{self, merchant_key, Extracted, Found, MailInput, EXTRACTOR_VERSION};
use crate::Result;

pub(super) const SCHEMA_EXTRACTED: &str = r#"
ALTER TABLE messages ADD COLUMN extracted INTEGER;
CREATE TABLE extracted(
    msg INTEGER NOT NULL,
    ord INTEGER NOT NULL,
    kind TEXT NOT NULL,
    account_id TEXT NOT NULL,
    date INTEGER NOT NULL,
    at TEXT,
    key TEXT NOT NULL,
    ref TEXT,
    amount REAL,
    currency TEXT,
    source INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY(msg, ord)
) WITHOUT ROWID;
CREATE INDEX extracted_kind ON extracted(kind, date);
CREATE INDEX extracted_at ON extracted(kind, at) WHERE at IS NOT NULL;
CREATE INDEX extracted_key ON extracted(key, kind);
CREATE INDEX extracted_ref ON extracted(ref) WHERE ref IS NOT NULL;
CREATE INDEX messages_unextracted ON messages(date) WHERE extracted IS NULL;
CREATE TABLE extract_state(version INTEGER NOT NULL);
INSERT INTO extract_state(version) VALUES (0);
CREATE TRIGGER extracted_gone AFTER DELETE ON messages BEGIN
    DELETE FROM extracted WHERE msg = old.rowid;
END;
"#;

/// Messages read per write transaction.
const BATCH: usize = 100;

/// One pass of [`Store::extract_pending`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractProgress {
    /// Messages read.
    pub scanned: usize,
    /// Facts stored.
    pub found: usize,
    /// Messages still waiting.
    pub remaining: i64,
}

/// A stored fact and the message it came from.
#[derive(Debug, Clone)]
pub(crate) struct StoredFact {
    pub kind: String,
    pub at: Option<String>,
    pub key: String,
    pub reference: Option<String>,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub source: i64,
    pub fact: Extracted,
}

struct Pending {
    rowid: i64,
    account_id: String,
    date: i64,
    flags: i64,
    from_email: String,
    from_name: Option<String>,
    subject: String,
    text: String,
    html: Option<String>,
}

impl Store {
    /// Read up to `max` not-yet-extracted messages (newest first) and store
    /// their facts. Cheap when nothing is pending (one partial-index probe).
    pub fn extract_pending(&self, max: usize) -> Result<ExtractProgress> {
        self.reset_extractions_if_outdated()?;
        let mut progress = ExtractProgress::default();
        while progress.scanned < max {
            let n = BATCH.min(max - progress.scanned);
            let pending: Vec<Pending> = self.read(|c| {
                let mut ids = c.prepare_cached(
                    "SELECT rowid FROM messages INDEXED BY messages_unextracted
                     WHERE extracted IS NULL ORDER BY date DESC LIMIT ?1",
                )?;
                let rowids: Vec<i64> = ids
                    .query_map([n as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut out = Vec::with_capacity(rowids.len());
                for rowid in rowids {
                    if let Some(p) = load_pending(c, rowid)? {
                        out.push(p);
                    }
                }
                Ok(out)
            })?;
            if pending.is_empty() {
                break;
            }
            // Extract outside the write lock.
            let results: Vec<(i64, String, i64, String, Vec<Found>)> = pending
                .into_iter()
                .map(|p| {
                    let (key, found) = run_extractors(&p);
                    (p.rowid, p.account_id, p.date, key, found)
                })
                .collect();
            let batch = results.len();
            let stored = self.write(|tx| {
                let mut mark = tx.prepare_cached(
                    "UPDATE messages SET extracted = ?1 WHERE rowid = ?2 AND extracted IS NULL",
                )?;
                let mut ins = tx.prepare_cached(
                    "INSERT OR REPLACE INTO extracted(msg, ord, kind, account_id, date, at, key, ref, amount, currency, source, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )?;
                let mut stored = 0;
                for (rowid, account, date, sender_key, found) in &results {
                    // Gone, or re-stored since it was read: the next pass has it.
                    if mark.execute(params![EXTRACTOR_VERSION, rowid])? == 0 {
                        continue;
                    }
                    for (ord, f) in found.iter().enumerate() {
                        let money = f.fact.money();
                        ins.execute(params![
                            rowid,
                            ord as i64,
                            f.fact.kind(),
                            account,
                            date,
                            f.fact.when(),
                            fact_key(&f.fact, sender_key),
                            f.fact.reference(),
                            money.map(|m| m.value),
                            money.map(|m| m.currency.as_str()),
                            f.source.code(),
                            serde_json::to_string(&f.fact)?,
                        ])?;
                        stored += 1;
                    }
                }
                Ok(stored)
            })?;
            progress.scanned += batch;
            progress.found += stored;
            if batch < n {
                break;
            }
        }
        progress.remaining = self.extract_remaining()?;
        Ok(progress)
    }

    /// Messages not yet read by the extractors. Counted on the partial
    /// index (O(pending), ~0 once caught up); named explicitly because the
    /// planner's statistics can be from before the scan (when every row
    /// was pending) and a table scan at 300k messages costs ~120 ms.
    pub fn extract_remaining(&self) -> Result<i64> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT count(*) FROM messages INDEXED BY messages_unextracted WHERE extracted IS NULL",
            )?
            .query_row([], |r| r.get(0))?)
        })
    }

    /// A new extractor version re-reads everything once.
    fn reset_extractions_if_outdated(&self) -> Result<()> {
        let v: i64 = self.read(|c| {
            Ok(c.prepare_cached("SELECT version FROM extract_state")?
                .query_row([], |r| r.get(0))?)
        })?;
        if v == EXTRACTOR_VERSION {
            return Ok(());
        }
        self.write(|tx| {
            tx.execute(
                "UPDATE messages SET extracted = NULL WHERE extracted IS NOT NULL",
                [],
            )?;
            tx.execute("DELETE FROM extracted", [])?;
            tx.execute("UPDATE extract_state SET version = ?1", [EXTRACTOR_VERSION])?;
            Ok(())
        })
    }
}

fn load_pending(c: &Connection, rowid: i64) -> Result<Option<Pending>> {
    Ok(c.prepare_cached(
        "SELECT m.account_id, m.date, m.flags, m.from_email, m.from_name, m.subject, m.snippet,
                b.body_text, b.body_html
         FROM messages m LEFT JOIN message_bodies b ON b.rowid = m.rowid WHERE m.rowid = ?1",
    )?
    .query_row([rowid], |r| {
        let snippet: String = r.get(6)?;
        let body: Option<Body> = r.get(7)?;
        let text = body.map(|b| b.0).filter(|t| !t.trim().is_empty());
        Ok(Pending {
            rowid,
            account_id: r.get(0)?,
            date: r.get(1)?,
            flags: r.get(2)?,
            from_email: r.get(3)?,
            from_name: r.get(4)?,
            subject: r.get(5)?,
            text: text.unwrap_or(snippet),
            html: r.get::<_, Option<Body>>(8)?.map(|b| b.0),
        })
    })
    .optional()?)
}

/// (sender key, facts) for one message.
fn run_extractors(p: &Pending) -> (String, Vec<Found>) {
    let (authored, _) = crate::text::split_quoted(&p.text);
    let input = MailInput {
        subject: &p.subject,
        from_email: &p.from_email,
        from_name: p.from_name.as_deref(),
        date: p.date,
        text: &authored,
        html: p.html.as_deref(),
        bulk: p.flags & F_NEWSLETTER != 0,
        sent: p.flags & (F_SENT | F_DRAFT) != 0,
    };
    let found = structured::extract(&input);
    (merchant_key(p.from_name.as_deref(), &p.from_email), found)
}

/// The facts of one message the scanner hasn't reached yet, computed now
/// and not stored (Ask stays correct during the first backfill). Empty
/// when the message is gone.
pub(crate) fn extract_now(c: &Connection, rowid: i64) -> Result<Vec<StoredFact>> {
    let Some(p) = load_pending(c, rowid)? else {
        return Ok(vec![]);
    };
    let (key, found) = run_extractors(&p);
    Ok(found
        .into_iter()
        .map(|f| {
            let money = f.fact.money().cloned();
            StoredFact {
                kind: f.fact.kind().to_string(),
                at: f.fact.when().map(String::from),
                key: fact_key(&f.fact, &key),
                reference: f.fact.reference().map(String::from),
                amount: money.as_ref().map(|m| m.value),
                currency: money.map(|m| m.currency),
                source: f.source.code(),
                fact: f.fact,
            }
        })
        .collect())
}

/// The lookup key: the merchant/airline/carrier/biller the fact names,
/// else the sender.
fn fact_key(f: &Extracted, sender_key: &str) -> String {
    let named = match f {
        Extracted::Flight(x) => x.airline.clone(),
        Extracted::Lodging(x) => x.name.clone(),
        Extracted::Order(x) => x.merchant.clone(),
        Extracted::Shipment(x) => x.merchant.clone().or_else(|| x.carrier.clone()),
        Extracted::Bill(x) => x.biller.clone(),
        Extracted::Reservation(x) => x.name.clone(),
        Extracted::Contact(_) => None,
    };
    named
        .map(|n| structured::clean_org(&n).to_lowercase())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| sender_key.to_string())
}

pub(crate) const FACT_COLS: &str =
    "x.msg, x.kind, x.account_id, x.date, x.at, x.key, x.ref, x.amount, x.currency, x.source, x.data";

pub(crate) fn read_fact(r: &rusqlite::Row) -> rusqlite::Result<Option<StoredFact>> {
    let data: String = r.get(10)?;
    let Ok(fact) = serde_json::from_str::<Extracted>(&data) else {
        return Ok(None);
    };
    // Columns 0, 2 and 3 (msg, account, date) come from the message row.
    Ok(Some(StoredFact {
        kind: r.get(1)?,
        at: r.get(4)?,
        key: r.get(5)?,
        reference: r.get(6)?,
        amount: r.get(7)?,
        currency: r.get(8)?,
        source: r.get(9)?,
        fact,
    }))
}

#[cfg(test)]
mod tests {
    use crate::types::*;
    use crate::Store;

    fn msg(
        id: &str,
        date: i64,
        from: (&str, &str),
        subject: &str,
        body: &str,
        html: Option<&str>,
    ) -> Message {
        Message {
            account_id: "me@example.com".into(),
            id: id.into(),
            thread_id: format!("t-{id}"),
            date,
            from: Address {
                name: Some(from.0.into()),
                email: from.1.into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: subject.into(),
            snippet: body.chars().take(100).collect(),
            body_text: body.into(),
            body_html: html.map(String::from),
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

    #[test]
    fn scans_incrementally_and_forgets_deleted_mail() {
        let store = Store::open_in_memory().unwrap();
        let t = 1_790_000_000_000;
        store
            .upsert_messages(&[
                msg(
                    "m1",
                    t,
                    ("Rydeo Receipts", "receipts@rydeo.example"),
                    "Your Tuesday trip with Rydeo",
                    "Thanks for riding\nTrip fare $18.20\nTotal $23.40",
                    None,
                ),
                msg(
                    "m2",
                    t + 1000,
                    ("Priya", "priya@linden.example"),
                    "Lunch",
                    "See you at noon.",
                    None,
                ),
            ])
            .unwrap();
        assert_eq!(store.extract_remaining().unwrap(), 2);
        let p = store.extract_pending(1000).unwrap();
        assert_eq!(p.scanned, 2);
        assert_eq!(p.found, 1);
        assert_eq!(p.remaining, 0);
        // Nothing left: a second pass reads nothing.
        assert_eq!(store.extract_pending(1000).unwrap().scanned, 0);
        let rows: Vec<(String, String, f64)> = store
            .read(|c| {
                Ok(c.prepare("SELECT kind, key, amount FROM extracted")?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(rows, vec![("order".into(), "rydeo".into(), 23.40)]);

        // New mail is picked up by the next pass.
        store
            .upsert_messages(&[msg(
                "m3",
                t + 2000,
                ("Rydeo Receipts", "receipts@rydeo.example"),
                "Your Friday trip with Rydeo",
                "Total $9.10",
                None,
            )])
            .unwrap();
        assert_eq!(store.extract_pending(1000).unwrap().found, 1);

        // Deleting a message drops its facts.
        store.remove_account("me@example.com").unwrap();
        let n: i64 = store
            .read(|c| Ok(c.query_row("SELECT count(*) FROM extracted", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 0);
    }
}
