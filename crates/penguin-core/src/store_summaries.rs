//! Cached thread summaries (`summaries`): the latest summary per thread,
//! with the thread version it was made from (`summary::version_key`). A
//! summary of the current version is shown straight from here, with no model
//! call; an older one is shown as stale until it's regenerated. One row per
//! thread: a new version replaces the old. Rows go with the account, and with
//! the thread when its last message is deleted.

use rusqlite::{params, Connection, OptionalExtension};

use super::Store;
use crate::types::AiSummary;
use crate::Result;

/// `data` is the `AiSummary` as JSON (`stale` is never stored: it's worked
/// out against the thread on every read).
pub(super) const SCHEMA_SUMMARIES: &str = r#"
CREATE TABLE summaries(
    account_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    version TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY(account_id, thread_id)
) WITHOUT ROWID;
"#;

pub(super) fn remove_account(c: &Connection, account_id: &str) -> Result<()> {
    c.execute("DELETE FROM summaries WHERE account_id = ?1", [account_id])?;
    Ok(())
}

pub(super) fn thread_deleted(c: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
    c.prepare_cached("DELETE FROM summaries WHERE account_id = ?1 AND thread_id = ?2")?
        .execute(params![account_id, thread_id])?;
    Ok(())
}

impl Store {
    /// The thread's cached summary, of whatever version; `stale` is set when
    /// `current_version` differs. One primary-key read.
    pub fn get_summary(
        &self,
        account_id: &str,
        thread_id: &str,
        current_version: &str,
    ) -> Result<Option<AiSummary>> {
        self.read(|c| {
            let row: Option<(String, String)> = c
                .prepare_cached(
                    "SELECT version, data FROM summaries WHERE account_id = ?1 AND thread_id = ?2",
                )?
                .query_row(params![account_id, thread_id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let Some((version, data)) = row else {
                return Ok(None);
            };
            let mut s: AiSummary = serde_json::from_str(&data)?;
            s.stale = version != current_version;
            Ok(Some(s))
        })
    }

    /// Store `summary` as the thread's summary, replacing any older one.
    pub fn put_summary(&self, summary: &AiSummary) -> Result<()> {
        let mut stored = summary.clone();
        stored.stale = false;
        let data = serde_json::to_string(&stored)?;
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT INTO summaries(account_id, thread_id, version, created_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(account_id, thread_id) DO UPDATE SET
                   version = excluded.version, created_at = excluded.created_at, data = excluded.data",
            )?
            .execute(params![
                stored.account_id,
                stored.thread_id,
                stored.version,
                stored.created_at,
                data
            ])?;
            Ok(())
        })
    }

    /// Forget the thread's summary.
    pub fn delete_summary(&self, account_id: &str, thread_id: &str) -> Result<()> {
        self.write(|tx| thread_deleted(tx, account_id, thread_id))
    }

    /// How many summaries are cached (Settings, diagnostics).
    pub fn count_summaries(&self) -> Result<u64> {
        self.read(|c| {
            Ok(c.query_row("SELECT COUNT(*) FROM summaries", [], |r| r.get::<_, i64>(0))? as u64)
        })
    }
}
