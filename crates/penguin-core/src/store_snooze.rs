//! Snooze: local records for snoozed conversations (see `types::Snooze`).
//!
//! Gmail has no snooze API, so a snooze is an archive plus a row here. The
//! row's life:
//! 1. **Snoozed** (`woke_at` NULL): listed in the Snoozed view, due at
//!    `wake_at`. Inserted *before* the archive is applied, so the thread is
//!    never in the inbox with an active snooze except when something put it
//!    back (see 4).
//! 2. **Woken locally** (`woke_at` set, `pushed` 0): `wake_snooze` stamped
//!    `woke_at` and added INBOX + UNREAD in one transaction; the Gmail update
//!    is still owed, and `wake_at` now holds the next attempt time.
//! 3. **Woken** (`pushed` 1): Gmail has it. `woke_at` stays as the thread's
//!    Inbox sort key (Gmail shows an unsnoozed thread at the top) until the
//!    thread leaves the inbox, when `refresh_thread` drops the row.
//! 4. `refresh_thread` also ends a snooze early when the thread is back in
//!    the inbox before waking (new mail in it, or moved to the inbox here or
//!    on another client), and drops rows whose thread is gone, trashed or
//!    spam. The scheduler (penguin-gmail `snooze`) drives 1 → 2 → 3.

use std::collections::HashSet;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use super::{apply_label_delta, refresh_thread, Store, F_INBOX, F_SPAM, F_TRASH, F_UNREAD};
use crate::types::{AccountId, Snooze};
use crate::Result;

/// Schema: see the module docs. Partial index = rows the scheduler still
/// owes something (a wake or a Gmail push).
pub(super) const SCHEMA_SNOOZES: &str = r#"
CREATE TABLE snoozes(
    account_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    wake_at INTEGER NOT NULL,
    snoozed_at INTEGER NOT NULL,
    woke_at INTEGER,
    pushed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(account_id, thread_id)
) WITHOUT ROWID;
CREATE INDEX snoozes_due ON snoozes(wake_at) WHERE pushed = 0;
"#;

/// Labels a wake adds back (Gmail's unsnooze: back in the inbox, unread).
pub const WAKE_LABELS: [&str; 2] = ["INBOX", "UNREAD"];

/// A snooze the scheduler owes something at `now`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueSnooze {
    pub account_id: AccountId,
    pub thread_id: String,
    /// When it was (or is) due; for a retry, the retry time.
    pub wake_at: i64,
    /// Already woken locally; only the Gmail update is owed.
    pub woken: bool,
}

/// What `wake_snooze` did locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalWake {
    /// Back in the inbox, unread. `subject` is for the notification.
    Woke { subject: String },
    /// The thread is gone, trashed or spam: the snooze was dropped.
    Dropped,
    /// No active snooze (already woken, cancelled or ended early).
    NotSnoozed,
}

impl Store {
    /// Snooze `threads` until `wake_at`, replacing any snooze they had. Only
    /// the records: the caller archives the threads afterwards (in that
    /// order, see the module docs).
    pub fn snooze_threads(
        &self,
        threads: &[(AccountId, String)],
        wake_at: i64,
        now_ms: i64,
    ) -> Result<()> {
        self.write(|tx| {
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO snoozes(account_id, thread_id, wake_at, snoozed_at, woke_at, pushed)
                 VALUES (?1, ?2, ?3, ?4, NULL, 0)",
            )?;
            for (account, thread) in threads {
                ins.execute(params![account, thread, wake_at, now_ms])?;
            }
            Ok(())
        })
    }

    /// Drop a snooze in any state. Returns whether there was one.
    pub fn unsnooze(&self, account_id: &str, thread_id: &str) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM snoozes WHERE account_id = ?1 AND thread_id = ?2",
                params![account_id, thread_id],
            )? > 0)
        })
    }

    /// The thread's active snooze (not yet woken).
    pub fn get_snooze(&self, account_id: &str, thread_id: &str) -> Result<Option<Snooze>> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT account_id, thread_id, wake_at, snoozed_at FROM snoozes
                 WHERE account_id = ?1 AND thread_id = ?2 AND woke_at IS NULL",
            )?
            .query_row(params![account_id, thread_id], snooze_row)
            .optional()?)
        })
    }

    /// Active snoozes, soonest first. `accounts`: None = every account (see
    /// `account_scope`).
    pub fn list_snoozes(&self, accounts: Option<&[AccountId]>) -> Result<Vec<Snooze>> {
        if accounts.is_some_and(|a| a.is_empty()) {
            return Ok(Vec::new());
        }
        let filter = match accounts {
            None => String::new(),
            Some(a) => format!(" AND account_id IN ({})", vec!["?"; a.len()].join(",")),
        };
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT account_id, thread_id, wake_at, snoozed_at FROM snoozes
                 WHERE woke_at IS NULL{filter} ORDER BY wake_at, account_id, thread_id"
            ))?;
            let rows = stmt.query_map(params_from_iter(accounts.unwrap_or(&[])), snooze_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Snoozes due at `now_ms` (wakes and owed Gmail pushes), soonest first.
    pub fn due_snoozes(&self, now_ms: i64) -> Result<Vec<DueSnooze>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT account_id, thread_id, wake_at, woke_at IS NOT NULL FROM snoozes
                 WHERE pushed = 0 AND wake_at <= ?1 ORDER BY wake_at, account_id, thread_id",
            )?;
            let rows = stmt.query_map([now_ms], |r| {
                Ok(DueSnooze {
                    account_id: r.get(0)?,
                    thread_id: r.get(1)?,
                    wake_at: r.get(2)?,
                    woken: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Earliest time the scheduler owes a snooze anything.
    pub fn next_snooze_due(&self) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT min(wake_at) FROM snoozes WHERE pushed = 0",
                [],
                |r| r.get(0),
            )?)
        })
    }

    /// Wake an active snooze locally, in one transaction: stamp `woke_at`
    /// (the Inbox sort key), then add INBOX + UNREAD to every message. The
    /// row stays owed to Gmail (`pushed` 0), next attempt `now_ms`.
    pub fn wake_snooze(&self, account_id: &str, thread_id: &str, now_ms: i64) -> Result<LocalWake> {
        self.write(|tx| {
            let active: bool = tx
                .prepare_cached(
                    "SELECT EXISTS(SELECT 1 FROM snoozes WHERE account_id = ?1 AND thread_id = ?2 AND woke_at IS NULL)",
                )?
                .query_row(params![account_id, thread_id], |r| r.get(0))?;
            if !active {
                return Ok(LocalWake::NotSnoozed);
            }
            let thread: Option<(i64, String)> = tx
                .prepare_cached("SELECT rowid, subject FROM threads WHERE account_id = ?1 AND thread_id = ?2")?
                .query_row(params![account_id, thread_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let live: Vec<i64> = match &thread {
                Some((trow, _)) => tx
                    .prepare_cached("SELECT rowid FROM messages WHERE thread_rowid = ?1 AND (flags & ?2) = 0")?
                    .query_map(params![trow, F_TRASH | F_SPAM], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?,
                None => Vec::new(),
            };
            let Some((_, subject)) = thread.filter(|_| !live.is_empty()) else {
                delete_row(tx, account_id, thread_id)?;
                return Ok(LocalWake::Dropped);
            };
            // woke_at first: refresh_thread then sees a woken snooze in the
            // inbox and keeps it (with the bump) instead of ending it early.
            tx.prepare_cached(
                "UPDATE snoozes SET woke_at = ?3, wake_at = ?3, pushed = 0 WHERE account_id = ?1 AND thread_id = ?2",
            )?
            .execute(params![account_id, thread_id, now_ms])?;
            let add: Vec<String> = WAKE_LABELS.iter().map(|l| l.to_string()).collect();
            let mut touched = HashSet::new();
            let all: Vec<i64> = tx
                .prepare_cached(
                    "SELECT m.rowid FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
                     WHERE t.account_id = ?1 AND t.thread_id = ?2",
                )?
                .query_map(params![account_id, thread_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for rowid in all {
                apply_label_delta(tx, rowid, &add, &[], &mut touched)?;
            }
            for t in touched {
                refresh_thread(tx, t)?;
            }
            Ok(LocalWake::Woke { subject })
        })
    }

    /// Gmail has the wake: the row is now only the Inbox sort key.
    pub fn snooze_pushed(&self, account_id: &str, thread_id: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE snoozes SET pushed = 1 WHERE account_id = ?1 AND thread_id = ?2 AND woke_at IS NOT NULL",
                params![account_id, thread_id],
            )?;
            Ok(())
        })
    }

    /// Try the owed Gmail push of a woken snooze again at `at`.
    pub fn retry_snooze_push(&self, account_id: &str, thread_id: &str, at: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE snoozes SET wake_at = ?3 WHERE account_id = ?1 AND thread_id = ?2 AND woke_at IS NOT NULL AND pushed = 0",
                params![account_id, thread_id, at],
            )?;
            Ok(())
        })
    }
}

fn snooze_row(r: &rusqlite::Row) -> rusqlite::Result<Snooze> {
    Ok(Snooze {
        account_id: r.get(0)?,
        thread_id: r.get(1)?,
        wake_at: r.get(2)?,
        snoozed_at: r.get(3)?,
    })
}

fn delete_row(c: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
    c.prepare_cached("DELETE FROM snoozes WHERE account_id = ?1 AND thread_id = ?2")?
        .execute(params![account_id, thread_id])?;
    Ok(())
}

/// Called by `refresh_thread` with the thread's live (not trash/spam)
/// message flags, newest last. Keeps the snooze row in step with the thread
/// (module docs, step 3 and 4) and returns the Inbox sort key to apply:
/// `Some(woke_at)` for a woken thread still in the inbox.
pub(super) fn reconcile(
    c: &Connection,
    account_id: &str,
    thread_id: &str,
    live_flags: &[i64],
) -> Result<Option<i64>> {
    let row: Option<Option<i64>> = c
        .prepare_cached("SELECT woke_at FROM snoozes WHERE account_id = ?1 AND thread_id = ?2")?
        .query_row(params![account_id, thread_id], |r| r.get(0))
        .optional()?;
    let Some(woke_at) = row else { return Ok(None) };
    let in_inbox = live_flags.iter().any(|f| f & F_INBOX != 0);
    match woke_at {
        // Woken and still in the inbox: keep it at the top.
        Some(at) if in_inbox => Ok(Some(at)),
        // Snoozed and still out of the inbox (and not all trash/spam).
        None if !in_inbox && !live_flags.is_empty() => Ok(None),
        // Woken and since archived; ended early (back in the inbox before
        // waking); or trashed/spammed.
        _ => {
            delete_row(c, account_id, thread_id)?;
            Ok(None)
        }
    }
}

/// Drop the snooze of a thread whose last message was deleted.
pub(super) fn thread_deleted(c: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
    delete_row(c, account_id, thread_id)
}

pub(super) fn remove_account(c: &Connection, account_id: &str) -> Result<()> {
    c.execute("DELETE FROM snoozes WHERE account_id = ?1", [account_id])?;
    Ok(())
}

/// `list_threads` for `MailboxView::Snoozed`: every active snooze in scope,
/// soonest wake first (then newest thread), `snoozed_until` set. `before`
/// pages past the single page return nothing.
///
/// `CROSS JOIN` keeps snoozes as the outer loop (SQLite's documented way to
/// fix the join order): with no statistics for `snoozes` (an empty table
/// gets no `sqlite_stat1` row) and 140k analyzed threads, the planner
/// otherwise scanned every thread and probed snoozes for each: 28 ms p50
/// at 300k messages with nothing snoozed, 0.05 ms now. `is:snoozed` in
/// search.rs joins the same way.
pub(super) fn snoozed_list_sql(scope: Option<&[AccountId]>, unread_only: bool) -> String {
    let acct = match scope {
        None => String::new(),
        Some(a) => format!(
            " AND s.account_id IN ({})",
            (0..a.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    let unread = if unread_only {
        format!(" AND (t.flags & {F_UNREAD}) != 0")
    } else {
        String::new()
    };
    format!(
        "SELECT t.account_id, t.thread_id, t.subject, t.snippet, t.participants, t.message_count, t.flags, t.labels, t.otp,
                s.wake_at, t.last_date
         FROM snoozes s CROSS JOIN threads t ON t.account_id = s.account_id AND t.thread_id = s.thread_id
         WHERE s.woke_at IS NULL{acct}{unread}
         ORDER BY s.wake_at, t.last_date DESC LIMIT ?1"
    )
}

#[cfg(test)]
#[path = "store_snooze_tests.rs"]
mod tests;
