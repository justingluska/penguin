//! Providers in the store (docs/PROVIDERS-IMPL.md): which backend each
//! account uses, the opaque per-provider sync state, moving messages
//! between threads (IMAP threading), and tables a provider owns.
//!
//! Provider-owned tables: a provider crate (IMAP, Microsoft) never edits
//! `MIGRATIONS`. It calls [`Store::migrate_provider_schema`] with its own
//! append-only list, named after itself, and reads and writes its tables
//! through [`Store::provider_read`] / [`Store::provider_write`]. Tables it
//! registers as per-account (an `account_id` column) are emptied for the
//! account by [`Store::remove_account`].

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::{ensure_thread, refresh_thread, Store};
use crate::{Error, Result};

/// `Account.provider` + `Account.provider_config`. Rows from before
/// providers existed are Gmail accounts with no config.
pub(super) const SCHEMA_ACCOUNT_PROVIDER: &str = r#"
ALTER TABLE accounts ADD COLUMN provider TEXT NOT NULL DEFAULT 'gmail';
ALTER TABLE accounts ADD COLUMN provider_config TEXT NOT NULL DEFAULT '{}';
"#;

/// The opaque provider cursor (`SyncCursor.provider_state`). Gmail's
/// history id and messages.list page token were columns; they move, as they
/// were, into Gmail's state JSON (`{"historyId", "backfillPageToken"}`, the
/// shape penguin-gmail's `GmailCursor` reads). Every cursor row at this
/// point belongs to a Gmail account: the provider column came one
/// migration earlier and nothing could store another kind before it.
/// backfill_done, failed_message_ids and window stay columns (generic).
pub(super) const SCHEMA_SYNC_STATE: &str = r#"
ALTER TABLE sync_cursors ADD COLUMN provider_state TEXT NOT NULL DEFAULT '';
UPDATE sync_cursors
   SET provider_state = json_object('historyId', history_id, 'backfillPageToken', backfill_page_token)
 WHERE history_id IS NOT NULL OR backfill_page_token IS NOT NULL;
ALTER TABLE sync_cursors DROP COLUMN history_id;
ALTER TABLE sync_cursors DROP COLUMN backfill_page_token;
"#;

/// Provider-owned schemas: one row per provider with the number of its
/// migrations applied and its per-account tables (space-separated).
pub(super) const SCHEMA_PROVIDER_TABLES: &str = r#"
CREATE TABLE provider_schema(
    name TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    account_tables TEXT NOT NULL DEFAULT ''
) WITHOUT ROWID;
"#;

/// A provider or table name: lowercase ASCII, digits and `_`, ≤ 48 bytes.
fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 48
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !s.as_bytes()[0].is_ascii_digit()
}

/// Empty every registered per-account provider table for `account_id`.
pub(super) fn remove_account(tx: &Transaction, account_id: &str) -> Result<()> {
    let lists: Vec<String> = tx
        .prepare("SELECT account_tables FROM provider_schema")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for table in lists.iter().flat_map(|l| l.split_whitespace()) {
        // Names were validated on registration; checked again because they
        // are spliced into SQL.
        if !valid_ident(table) {
            return Err(Error::Db(format!("bad provider table name {table:?}")));
        }
        tx.execute(
            &format!("DELETE FROM \"{table}\" WHERE account_id = ?1"),
            [account_id],
        )?;
    }
    Ok(())
}

impl Store {
    /// Apply a provider's own migrations (an append-only list, like the
    /// store's: never edit or reorder an entry that has shipped). Each runs
    /// in its own transaction and is recorded under `name`. Every table in
    /// `account_tables` must have an `account_id` column; its rows for an
    /// account are deleted with the account. Tables must be named
    /// `<name>_…`. Call once at startup; it is cheap when up to date.
    pub fn migrate_provider_schema(
        &self,
        name: &str,
        migrations: &[&str],
        account_tables: &[&str],
    ) -> Result<()> {
        if !valid_ident(name) {
            return Err(Error::Db(format!("bad provider schema name {name:?}")));
        }
        for t in account_tables {
            if !valid_ident(t) || !t.starts_with(&format!("{name}_")) {
                return Err(Error::Db(format!(
                    "provider table {t:?} must be named {name}_…"
                )));
            }
        }
        let tables = account_tables.join(" ");
        let applied: usize = self.read(|c| {
            Ok(
                c.prepare_cached("SELECT version FROM provider_schema WHERE name = ?1")?
                    .query_row([name], |r| r.get::<_, i64>(0))
                    .optional()?
                    .unwrap_or(0) as usize,
            )
        })?;
        if applied > migrations.len() {
            return Err(Error::Db(format!(
                "{name} schema v{applied} is newer than this build (v{})",
                migrations.len()
            )));
        }
        for (i, sql) in migrations.iter().enumerate().skip(applied) {
            self.write(|tx| {
                tx.execute_batch(sql)?;
                tx.execute(
                    "INSERT INTO provider_schema(name, version, account_tables) VALUES (?1, ?2, ?3)
                     ON CONFLICT(name) DO UPDATE SET version = excluded.version,
                       account_tables = excluded.account_tables",
                    params![name, (i + 1) as i64, tables],
                )?;
                Ok(())
            })?;
        }
        // The table list can grow without a new migration (a table created
        // by an earlier one registered late).
        self.write(|tx| {
            tx.execute(
                "UPDATE provider_schema SET account_tables = ?2 WHERE name = ?1",
                params![name, tables],
            )?;
            Ok(())
        })
    }

    /// Read a provider's own tables (a pooled read-only connection). Touch
    /// only tables your provider created; everything else goes through the
    /// typed Store API.
    pub fn provider_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.read(f)
    }

    /// Write a provider's own tables in one IMMEDIATE transaction (same
    /// rules as [`Store::provider_read`]).
    pub fn provider_write<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        self.write(f)
    }

    /// Move stored messages to thread `thread_id` (created when new), e.g.
    /// when IMAP threading learns that two conversations are one. Both the
    /// old and the new thread are recomputed; a thread left empty is
    /// removed (as a deletion would). Unknown ids are skipped. Returns the
    /// thread ids whose contents changed, for `mail-changed`.
    pub fn set_message_thread(
        &self,
        account_id: &str,
        message_ids: &[String],
        thread_id: &str,
    ) -> Result<Vec<String>> {
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let target = ensure_thread(tx, account_id, thread_id)?;
            let mut touched = HashSet::new();
            for id in message_ids {
                let found: Option<(i64, i64)> = tx
                    .prepare_cached(
                        "SELECT rowid, thread_rowid FROM messages WHERE account_id = ?1 AND id = ?2",
                    )?
                    .query_row(params![account_id, id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                let Some((rowid, from)) = found else { continue };
                if from == target {
                    continue;
                }
                tx.prepare_cached("UPDATE messages SET thread_rowid = ?2 WHERE rowid = ?1")?
                    .execute(params![rowid, target])?;
                touched.insert(from);
                touched.insert(target);
            }
            if touched.is_empty() {
                // Nothing moved: don't leave a new, empty thread row behind.
                refresh_thread(tx, target)?;
                return Ok(Vec::new());
            }
            let mut threads = Vec::new();
            for t in touched {
                let tid: Option<String> = tx
                    .prepare_cached("SELECT thread_id FROM threads WHERE rowid = ?1")?
                    .query_row([t], |r| r.get(0))
                    .optional()?;
                threads.extend(tid);
                refresh_thread(tx, t)?;
            }
            threads.sort();
            Ok(threads)
        })
    }
}

#[cfg(test)]
#[path = "store_providers_tests.rs"]
mod tests;
