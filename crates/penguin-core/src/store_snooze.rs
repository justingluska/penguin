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
//!
//! Cross-device (store_sync.rs, docs/CROSS-DEVICE-STATE.md): every change
//! here also writes the thread's `Kind::Snooze` record in the same
//! transaction. Snoozing and unsnoozing are user intents (a new
//! generation); waking (2 → 3, only once the provider has it) and ending
//! by itself (4) advance the current one. Waking locally (1 → 2) writes no
//! record: it may be undone if another device turns out to have re-snoozed
//! the thread meanwhile, so it remembers what it changed (`wake_undo`).
//! A snooze that arrived from another device before this device saw the
//! archive is `remote_pending`: it is not "back in the inbox" until the
//! archive has been seen here once.

use std::collections::HashSet;

use penguin_sync::{thread_key, Kind, Record, SnoozeBody, SnoozeState};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{
    apply_label_delta, refresh_thread, sync, triage::now_ms, Store, F_INBOX, F_SPAM, F_TRASH,
    F_UNREAD,
};
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
                sync::emit_intent(
                    tx,
                    Kind::Snooze,
                    &thread_key(account, thread),
                    Some(SnoozeBody::snoozed(wake_at, now_ms).to_value()),
                    now_ms,
                )?;
            }
            Ok(())
        })
    }

    /// Drop a snooze in any state: the user unsnoozed it (or the snooze
    /// failed), on every device. Returns whether there was one here.
    pub fn unsnooze(&self, account_id: &str, thread_id: &str) -> Result<bool> {
        self.write(|tx| {
            let had = tx.execute(
                "DELETE FROM snoozes WHERE account_id = ?1 AND thread_id = ?2",
                params![account_id, thread_id],
            )? > 0;
            let key = thread_key(account_id, thread_id);
            let live = sync::load(tx, Kind::Snooze, &key)?.is_some_and(|s| !s.record.is_deleted());
            if had || live {
                sync::emit_intent(tx, Kind::Snooze, &key, None, now_ms())?;
            }
            Ok(had)
        })
    }

    /// The snooze ended by itself (the provider no longer has the thread):
    /// drop it here and mark it ended everywhere, without overriding a newer
    /// snooze made on another device.
    pub fn end_snooze(&self, account_id: &str, thread_id: &str) -> Result<()> {
        self.write(|tx| {
            emit_end(tx, account_id, thread_id, SnoozeState::Ended, now_ms())?;
            delete_row(tx, account_id, thread_id)
        })
    }

    /// Whether this device may tell the provider about its local wake of
    /// the thread: it has pulled every synced change made up to the wake,
    /// so no other device re-snoozed or unsnoozed it meanwhile (sync off:
    /// always). Otherwise the wake waits for a pull.
    pub fn snooze_wake_committable(&self, account_id: &str, thread_id: &str) -> Result<bool> {
        self.read(|c| {
            let woke: Option<Option<i64>> = c
                .prepare_cached(
                    "SELECT woke_at FROM snoozes WHERE account_id = ?1 AND thread_id = ?2",
                )?
                .query_row(params![account_id, thread_id], |r| r.get(0))
                .optional()?;
            match woke.flatten() {
                Some(at) => sync::pull_fresh(c, at),
                None => Ok(false),
            }
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
    /// row stays owed to Gmail (`pushed` 0), next attempt `now_ms`. What it
    /// added is remembered (`wake_undo`) until the wake is pushed.
    pub fn wake_snooze(&self, account_id: &str, thread_id: &str, now_ms: i64) -> Result<LocalWake> {
        self.write(|tx| wake_local(tx, account_id, thread_id, now_ms))
    }

    /// Gmail has the wake: the row is now only the Inbox sort key, and the
    /// snooze is woken on every device.
    pub fn snooze_pushed(&self, account_id: &str, thread_id: &str) -> Result<()> {
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE snoozes SET pushed = 1, wake_undo = NULL
                 WHERE account_id = ?1 AND thread_id = ?2 AND woke_at IS NOT NULL",
                params![account_id, thread_id],
            )?;
            if n > 0 {
                emit_end(tx, account_id, thread_id, SnoozeState::Woken, now_ms())?;
            }
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

/// What a local wake changed, to undo it if another device turns out to
/// have re-snoozed the thread first: the messages it put in the inbox and
/// the ones it marked unread (rowids).
#[derive(Debug, Default, Serialize, Deserialize)]
struct WakeUndo {
    #[serde(default)]
    inbox: Vec<i64>,
    #[serde(default)]
    unread: Vec<i64>,
}

fn wake_local(
    tx: &Connection,
    account_id: &str,
    thread_id: &str,
    now_ms: i64,
) -> Result<LocalWake> {
    let active: bool = tx
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM snoozes WHERE account_id = ?1 AND thread_id = ?2 AND woke_at IS NULL)",
        )?
        .query_row(params![account_id, thread_id], |r| r.get(0))?;
    if !active {
        return Ok(LocalWake::NotSnoozed);
    }
    let thread: Option<(i64, String)> = tx
        .prepare_cached(
            "SELECT rowid, subject FROM threads WHERE account_id = ?1 AND thread_id = ?2",
        )?
        .query_row(params![account_id, thread_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let all: Vec<(i64, i64)> = match &thread {
        Some((trow, _)) => tx
            .prepare_cached("SELECT rowid, flags FROM messages WHERE thread_rowid = ?1")?
            .query_map([trow], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?,
        None => Vec::new(),
    };
    let live = all.iter().any(|(_, f)| f & (F_TRASH | F_SPAM) == 0);
    let Some((_, subject)) = thread.filter(|_| live) else {
        // Gone here (or not synced to this device yet): local only, the
        // record stays for devices that have the thread.
        delete_row(tx, account_id, thread_id)?;
        return Ok(LocalWake::Dropped);
    };
    let undo = WakeUndo {
        inbox: all
            .iter()
            .filter(|(_, f)| f & F_INBOX == 0)
            .map(|(r, _)| *r)
            .collect(),
        unread: all
            .iter()
            .filter(|(_, f)| f & F_UNREAD == 0)
            .map(|(r, _)| *r)
            .collect(),
    };
    // woke_at first: refresh_thread then sees a woken snooze in the
    // inbox and keeps it (with the bump) instead of ending it early.
    tx.prepare_cached(
        "UPDATE snoozes SET woke_at = ?3, wake_at = ?3, pushed = 0, remote_pending = 0, wake_undo = ?4
         WHERE account_id = ?1 AND thread_id = ?2",
    )?
    .execute(params![account_id, thread_id, now_ms, serde_json::to_string(&undo)?])?;
    let add: Vec<String> = WAKE_LABELS.iter().map(|l| l.to_string()).collect();
    let mut touched = HashSet::new();
    for (rowid, _) in all {
        apply_label_delta(tx, rowid, &add, &[], &mut touched)?;
    }
    for t in touched {
        refresh_thread(tx, t)?;
    }
    Ok(LocalWake::Woke { subject })
}

/// Undo a local wake that was never pushed (see [`WakeUndo`]). The caller
/// has already updated or deleted the row.
fn revert_wake(tx: &Connection, undo: Option<String>) -> Result<()> {
    let Some(undo) = undo.and_then(|u| serde_json::from_str::<WakeUndo>(&u).ok()) else {
        return Ok(());
    };
    let mut touched = HashSet::new();
    for (rowids, label) in [(&undo.inbox, "INBOX"), (&undo.unread, "UNREAD")] {
        for &rowid in rowids {
            let exists: bool = tx
                .prepare_cached("SELECT EXISTS(SELECT 1 FROM messages WHERE rowid = ?1)")?
                .query_row([rowid], |r| r.get(0))?;
            if exists {
                apply_label_delta(tx, rowid, &[], &[label.to_string()], &mut touched)?;
            }
        }
    }
    for t in touched {
        refresh_thread(tx, t)?;
    }
    Ok(())
}

/// Move the thread's synced snooze on to `state` (Woken / Ended), if it is
/// still waiting. A snooze made since on another device (a newer
/// generation) is not affected: phases only order within a generation.
fn emit_end(
    c: &Connection,
    account_id: &str,
    thread_id: &str,
    state: SnoozeState,
    now: i64,
) -> Result<()> {
    sync::emit_phase(
        c,
        Kind::Snooze,
        &thread_key(account_id, thread_id),
        state.phase(),
        now,
        |current: &Record| {
            let body: SnoozeBody = current.body_as()?;
            (body.state == SnoozeState::Snoozed).then(|| {
                SnoozeBody {
                    state,
                    ended_at: Some(now),
                    ..body
                }
                .to_value()
            })
        },
    )?;
    Ok(())
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
    let row: Option<(Option<i64>, bool)> = c
        .prepare_cached(
            "SELECT woke_at, remote_pending FROM snoozes WHERE account_id = ?1 AND thread_id = ?2",
        )?
        .query_row(params![account_id, thread_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let Some((woke_at, remote_pending)) = row else {
        return Ok(None);
    };
    let in_inbox = live_flags.iter().any(|f| f & F_INBOX != 0);
    match woke_at {
        // Woken and still in the inbox: keep it at the top.
        Some(at) if in_inbox => Ok(Some(at)),
        // Snoozed and out of the inbox (and not all trash/spam). A snooze
        // from another device is now confirmed archived here.
        None if !in_inbox && !live_flags.is_empty() => {
            if remote_pending {
                c.prepare_cached(
                    "UPDATE snoozes SET remote_pending = 0 WHERE account_id = ?1 AND thread_id = ?2",
                )?
                .execute(params![account_id, thread_id])?;
            }
            Ok(None)
        }
        // Snoozed on another device and the archive not seen here yet:
        // still being in the inbox here is not "back in the inbox".
        None if remote_pending && !live_flags.is_empty() => Ok(None),
        // Woken and since archived: only the sort key goes.
        Some(_) => {
            delete_row(c, account_id, thread_id)?;
            Ok(None)
        }
        // Ended early (back in the inbox before waking), or trashed /
        // spammed: ended on every device.
        None => {
            emit_end(c, account_id, thread_id, SnoozeState::Ended, now_ms())?;
            delete_row(c, account_id, thread_id)?;
            Ok(None)
        }
    }
}

/// Drop the snooze of a thread whose last message was deleted.
pub(super) fn thread_deleted(c: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
    let active: bool = c
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM snoozes WHERE account_id = ?1 AND thread_id = ?2 AND woke_at IS NULL)",
        )?
        .query_row(params![account_id, thread_id], |r| r.get(0))?;
    if active {
        emit_end(c, account_id, thread_id, SnoozeState::Ended, now_ms())?;
    }
    delete_row(c, account_id, thread_id)
}

/// Apply a snooze record from another device that won the merge (the
/// account exists here).
pub(super) fn project_remote(
    c: &Connection,
    account_id: &str,
    thread_id: &str,
    rec: &Record,
    now: i64,
) -> Result<()> {
    let row: Option<(Option<i64>, bool, Option<String>)> = c
        .prepare_cached(
            "SELECT woke_at, pushed, wake_undo FROM snoozes WHERE account_id = ?1 AND thread_id = ?2",
        )?
        .query_row(params![account_id, thread_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    // A local wake not pushed yet: undone unless the record says woken.
    let unpushed_undo = row
        .as_ref()
        .filter(|(woke, pushed, _)| woke.is_some() && !pushed)
        .and_then(|(_, _, undo)| undo.clone());
    let Some(body) = rec.body_as::<SnoozeBody>() else {
        // Unsnoozed there (a move to the inbox, if any, comes through the
        // provider): only the row goes.
        return delete_row(c, account_id, thread_id);
    };
    match body.state {
        SnoozeState::Snoozed => {
            let thread: Option<i64> = c
                .prepare_cached(
                    "SELECT rowid FROM threads WHERE account_id = ?1 AND thread_id = ?2",
                )?
                .query_row(params![account_id, thread_id], |r| r.get(0))
                .optional()?;
            let msgs: Vec<(i64, i64)> = match thread {
                Some(trow) => c
                    .prepare_cached("SELECT flags, date FROM messages WHERE thread_rowid = ?1")?
                    .query_map([trow], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?,
                None => Vec::new(),
            };
            let live: Vec<&(i64, i64)> = msgs
                .iter()
                .filter(|(f, _)| f & (F_TRASH | F_SPAM) == 0)
                .collect();
            let gone = thread.is_some() && live.is_empty();
            // New mail in the inbox since it was snoozed ended it already
            // (the snoozing device sees the same mail and ends it there).
            let replied = live
                .iter()
                .any(|(f, date)| f & F_INBOX != 0 && *date > body.snoozed_at);
            if gone || replied {
                delete_row(c, account_id, thread_id)?;
                return revert_wake(c, unpushed_undo);
            }
            c.prepare_cached(
                "INSERT INTO snoozes(account_id, thread_id, wake_at, snoozed_at, woke_at, pushed, remote_pending, wake_undo)
                 VALUES (?1, ?2, ?3, ?4, NULL, 0, 1, NULL)
                 ON CONFLICT(account_id, thread_id) DO UPDATE SET wake_at = excluded.wake_at,
                   snoozed_at = excluded.snoozed_at, woke_at = NULL, pushed = 0, remote_pending = 1,
                   wake_undo = NULL",
            )?
            .execute(params![account_id, thread_id, body.wake_at, body.snoozed_at])?;
            revert_wake(c, unpushed_undo)?;
            // Settle remote_pending from what this device sees now.
            if let Some(trow) = thread {
                refresh_thread(c, trow)?;
            }
            Ok(())
        }
        SnoozeState::Woken => {
            match row {
                // Still snoozed here: wake it locally; the waking device
                // already told the provider.
                Some((None, _, _)) => {
                    let at = body.ended_at.unwrap_or(now);
                    if let LocalWake::Woke { .. } = wake_local(c, account_id, thread_id, at)? {
                        mark_pushed(c, account_id, thread_id)?;
                    }
                }
                // Woken here too: nothing left to push.
                Some((Some(_), _, _)) => mark_pushed(c, account_id, thread_id)?,
                None => {}
            }
            Ok(())
        }
        SnoozeState::Ended => {
            delete_row(c, account_id, thread_id)?;
            revert_wake(c, unpushed_undo)
        }
    }
}

fn mark_pushed(c: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
    c.prepare_cached(
        "UPDATE snoozes SET pushed = 1, wake_undo = NULL WHERE account_id = ?1 AND thread_id = ?2",
    )?
    .execute(params![account_id, thread_id])?;
    Ok(())
}

/// Records for snoozes from before sync (store_sync::seed).
pub(super) fn seed(c: &Connection, now: i64) -> Result<()> {
    let rows: Vec<(String, String, i64, i64)> = c
        .prepare(
            "SELECT account_id, thread_id, wake_at, snoozed_at FROM snoozes WHERE woke_at IS NULL",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (a, t, wake_at, snoozed_at) in rows {
        let key = thread_key(&a, &t);
        if sync::load(c, Kind::Snooze, &key)?.is_none() {
            let body = SnoozeBody::snoozed(wake_at, snoozed_at).to_value();
            sync::emit_intent(c, Kind::Snooze, &key, Some(body), now)?;
        }
    }
    Ok(())
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
