//! Headers-only ("body pending") messages for the sync window. Mail older
//! than the window is stored from Gmail's metadata (headers, labels,
//! snippet) with `F_BODY_PENDING`; its FTS row indexes the snippet where the
//! body would go. A later full upsert (opening the message, growing the
//! window) replaces the row and re-indexes the real body.

use std::collections::HashSet;

use rusqlite::{params, params_from_iter, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{
    bump_count, compress_body, format_addr, ingest_order, insert_message, refresh_thread, Extra,
    Store, F_BODY_PENDING, F_DRAFT, ROWID_SLOTS,
};
use crate::types::{AccountId, Message};
use crate::Result;

/// Messages whose body is dropped per write transaction, so a large
/// "free up space" never holds the writer for long.
const DROP_CHUNK: usize = 1000;

/// Full vs headers-only message counts for one account.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BodyCoverage {
    pub account_id: AccountId,
    pub full: u64,
    pub headers_only: u64,
}

impl Store {
    /// Store headers-only messages (body pending). Ids already stored are
    /// left alone, so a full message is never downgraded by a late metadata
    /// fetch. Returns how many were inserted.
    pub fn insert_header_messages(&self, messages: &[Message]) -> Result<usize> {
        if messages.is_empty() {
            return Ok(0);
        }
        self.write(|tx| {
            let mut touched = HashSet::new();
            let mut inserted = 0usize;
            let mut per_account: Vec<(&str, i64)> = Vec::new();
            for i in ingest_order(messages) {
                let m = &messages[i];
                let exists: bool = tx
                    .prepare_cached(
                        "SELECT EXISTS(SELECT 1 FROM messages WHERE account_id = ?1 AND id = ?2)",
                    )?
                    .query_row(params![m.account_id, m.id], |r| r.get(0))?;
                if exists {
                    continue;
                }
                insert_message(tx, m, true, &mut touched)?;
                inserted += 1;
                match per_account.iter_mut().find(|(a, _)| *a == m.account_id) {
                    Some((_, n)) => *n += 1,
                    None => per_account.push((&m.account_id, 1)),
                }
            }
            for (acct, n) in per_account {
                bump_count(tx, acct, n)?;
            }
            for t in touched {
                refresh_thread(tx, t)?;
            }
            Ok(inserted)
        })
    }

    /// The ids in `ids` that still need a full download: not stored at all,
    /// or stored headers-only. Order is preserved.
    pub fn ids_needing_body(&self, account_id: &str, ids: &[String]) -> Result<Vec<String>> {
        let full: HashSet<String> = self.read(|c| {
            let mut out = HashSet::new();
            for chunk in ids.chunks(500) {
                let placeholders = vec!["?"; chunk.len()].join(",");
                let sql = format!(
                    "SELECT id FROM messages WHERE account_id = ? AND id IN ({placeholders})
                     AND flags & {F_BODY_PENDING} = 0"
                );
                let mut stmt = c.prepare(&sql)?;
                let args = std::iter::once(account_id).chain(chunk.iter().map(|s| s.as_str()));
                let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
                for r in rows {
                    out.insert(r?);
                }
            }
            Ok(out)
        })?;
        Ok(ids
            .iter()
            .filter(|id| !full.contains(*id))
            .cloned()
            .collect())
    }

    /// Headers-only messages of one thread (oldest first).
    pub fn pending_message_ids(&self, account_id: &str, thread_id: &str) -> Result<Vec<String>> {
        self.read(|c| {
            let rows = c
                .prepare_cached(&format!(
                    "SELECT m.id FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
                     WHERE t.account_id = ?1 AND t.thread_id = ?2 AND m.flags & {F_BODY_PENDING} != 0
                     ORDER BY m.date, m.rowid"
                ))?
                .query_map(params![account_id, thread_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(rows)
        })
    }

    /// Whether one message is stored headers-only (None = not stored).
    pub fn is_body_pending(&self, account_id: &str, message_id: &str) -> Result<Option<bool>> {
        self.read(|c| {
            let flags: Option<i64> = c
                .prepare_cached("SELECT flags FROM messages WHERE account_id = ?1 AND id = ?2")?
                .query_row(params![account_id, message_id], |r| r.get(0))
                .optional()?;
            Ok(flags.map(|f| f & F_BODY_PENDING != 0))
        })
    }

    /// Full vs headers-only counts for every account with messages. Uses
    /// the pending partial index plus `message_counts`, so it's cheap.
    pub fn body_coverage(&self) -> Result<Vec<BodyCoverage>> {
        self.read(|c| {
            let mut pending = std::collections::HashMap::<String, u64>::new();
            {
                let mut stmt = c.prepare_cached(&format!(
                    "SELECT account_id, count(*) FROM messages INDEXED BY messages_body_pending
                     WHERE flags & {F_BODY_PENDING} != 0 GROUP BY account_id"
                ))?;
                let rows =
                    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
                for row in rows {
                    let (a, n) = row?;
                    pending.insert(a, n.max(0) as u64);
                }
            }
            let mut stmt =
                c.prepare_cached("SELECT account_id, n FROM message_counts ORDER BY account_id")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            let mut out = Vec::new();
            for row in rows {
                let (account_id, n) = row?;
                let headers_only = pending.get(&account_id).copied().unwrap_or(0);
                out.push(BodyCoverage {
                    full: (n.max(0) as u64).saturating_sub(headers_only),
                    headers_only,
                    account_id,
                });
            }
            Ok(out)
        })
    }

    /// Full (not headers-only) messages of an account dated in
    /// `[after_ms, before_ms)` (None = no upper bound). A rowid range scan.
    pub fn count_full_between(
        &self,
        account_id: &str,
        after_ms: i64,
        before_ms: Option<i64>,
    ) -> Result<u64> {
        let before = before_ms.unwrap_or(i64::MAX / ROWID_SLOTS);
        self.read(|c| {
            let n: i64 = c
                .prepare_cached(&format!(
                    "SELECT count(*) FROM messages WHERE rowid >= ?2 AND rowid < ?3 AND account_id = ?1
                     AND flags & {F_BODY_PENDING} = 0"
                ))?
                .query_row(
                    params![account_id, rowid_bound(after_ms), rowid_bound(before)],
                    |r| r.get(0),
                )?;
            Ok(n.max(0) as u64)
        })
    }

    /// Full messages of an account dated before `before_ms` (drafts
    /// excluded): what [`Store::drop_bodies_before`] would free.
    pub fn count_full_before(&self, account_id: &str, before_ms: i64) -> Result<u64> {
        self.read(|c| {
            let n: i64 = c
                .prepare_cached(&format!(
                    "SELECT count(*) FROM messages WHERE rowid < ?2 AND account_id = ?1
                     AND date < ?3 AND flags & {} = 0",
                    F_BODY_PENDING | F_DRAFT
                ))?
                .query_row(
                    params![account_id, rowid_bound(before_ms), before_ms],
                    |r| r.get(0),
                )?;
            Ok(n.max(0) as u64)
        })
    }

    /// Full messages of an account dated before `before_ms` (drafts
    /// excluded) and the stored (compressed) size of their bodies: what
    /// [`Store::drop_bodies_before`] would remove. O(messages in range).
    pub fn full_bodies_before(&self, account_id: &str, before_ms: i64) -> Result<(u64, u64)> {
        self.read(|c| {
            let (n, bytes): (i64, i64) = c
                .prepare_cached(&format!(
                    "SELECT count(*), coalesce(sum(length(b.body_text) + coalesce(length(b.body_html), 0)), 0)
                     FROM messages m JOIN message_bodies b ON b.rowid = m.rowid
                     WHERE m.rowid < ?2 AND m.account_id = ?1 AND m.date < ?3 AND m.flags & {} = 0",
                    F_BODY_PENDING | F_DRAFT
                ))?
                .query_row(
                    params![account_id, rowid_bound(before_ms), before_ms],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
            Ok((n.max(0) as u64, bytes.max(0) as u64))
        })
    }

    /// "Free up space": drop the stored bodies of an account's messages
    /// dated before `before_ms`, keeping headers, labels, attachment
    /// metadata and the snippet (they become headers-only, and search keeps
    /// finding them by subject, people and snippet). Drafts are kept.
    /// Returns how many messages were trimmed. The file shrinks after
    /// [`Store::vacuum`]; until then SQLite reuses the pages.
    pub fn drop_bodies_before(&self, account_id: &str, before_ms: i64) -> Result<u64> {
        let mut total = 0u64;
        loop {
            let trimmed = self.write(|tx| {
                let mut rowids: Vec<i64> = tx
                    .prepare_cached(&format!(
                        "SELECT rowid FROM messages WHERE rowid < ?2 AND account_id = ?1
                         AND date < ?3 AND flags & {} = 0 LIMIT ?4",
                        F_BODY_PENDING | F_DRAFT
                    ))?
                    .query_map(
                        params![account_id, rowid_bound(before_ms), before_ms, DROP_CHUNK as i64],
                        |r| r.get(0),
                    )?
                    .collect::<rusqlite::Result<_>>()?;
                rowids.sort_unstable();
                let empty = compress_body("")?;
                // Each message's index row is rewritten with the snippet in
                // place of the body: all deletes, then all inserts, both in
                // rowid order, so FTS5 doesn't flush a segment per message
                // (see `ingest_order`).
                let mut fts_rows = Vec::with_capacity(rowids.len());
                for &rowid in &rowids {
                    let (subject, snippet, from_name, from_email, extra): (
                        String,
                        String,
                        Option<String>,
                        String,
                        String,
                    ) = tx
                        .prepare_cached(
                            "SELECT m.subject, m.snippet, m.from_name, m.from_email, b.extra
                             FROM messages m JOIN message_bodies b ON b.rowid = m.rowid WHERE m.rowid = ?1",
                        )?
                        .query_row([rowid], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                        })?;
                    let extra: Extra = serde_json::from_str(&extra).unwrap_or_default();
                    let filenames: Vec<String> = tx
                        .prepare_cached(
                            "SELECT filename FROM attachments WHERE message_rowid = ?1 AND inline = 0 ORDER BY ord",
                        )?
                        .query_map([rowid], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    let mut sender = String::new();
                    format_addr(
                        &crate::types::Address {
                            name: from_name,
                            email: from_email,
                        },
                        &mut sender,
                    );
                    let mut recipients = String::new();
                    for a in extra.to.iter().chain(&extra.bcc) {
                        format_addr(a, &mut recipients);
                    }
                    let mut cc = String::new();
                    for a in &extra.cc {
                        format_addr(a, &mut cc);
                    }
                    let mut files = String::new();
                    for f in &filenames {
                        files.push_str(f);
                        files.push(' ');
                    }
                    fts_rows.push((rowid, subject, sender, recipients, cc, snippet, files));
                    tx.prepare_cached(
                        "UPDATE message_bodies SET body_text = ?2, body_html = NULL WHERE rowid = ?1",
                    )?
                    .execute(params![rowid, empty])?;
                    tx.prepare_cached(&format!(
                        "UPDATE messages SET flags = flags | {F_BODY_PENDING} WHERE rowid = ?1"
                    ))?
                    .execute([rowid])?;
                }
                for &rowid in &rowids {
                    tx.prepare_cached("DELETE FROM messages_fts WHERE rowid = ?1")?
                        .execute([rowid])?;
                }
                for (rowid, subject, sender, recipients, cc, snippet, files) in &fts_rows {
                    tx.prepare_cached(
                        "INSERT INTO messages_fts(rowid, subject, sender, recipients, cc, body, quoted, filenames)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', ?7)",
                    )?
                    .execute(params![rowid, subject, sender, recipients, cc, snippet, files])?;
                }
                Ok(rowids.len())
            })?;
            total += trimmed as u64;
            if trimmed < DROP_CHUNK {
                return Ok(total);
            }
        }
    }

    /// Rewrite the database file to return freed pages to the OS. The
    /// full-text index is merged first, in steps that let other writes
    /// through (`merge_fts`); VACUUM itself holds the writer for the
    /// duration (seconds on a large mailbox). Readers keep working from the
    /// WAL.
    pub fn vacuum(&self) -> Result<()> {
        self.merge_fts("messages_fts", super::MERGE_STEP_PAGES)?;
        self.inner.lock_writer().execute_batch("VACUUM;")?;
        Ok(())
    }
}

/// First rowid dated at or after `before_ms` (rowids are date-ordered).
fn rowid_bound(before_ms: i64) -> i64 {
    before_ms.max(0).saturating_mul(ROWID_SLOTS)
}

#[cfg(test)]
#[path = "store_window_tests.rs"]
mod tests;
