//! Outbox: the send-later schedule and remind-if-no-reply reminders.
//! OWNER: gmail-sync agent. The firing logic lives in penguin-gmail's
//! `outbox` module; this is only persistence.

use rusqlite::{params, OptionalExtension, Row};

use super::{Store, F_DRAFT, F_SENT, F_SPAM};
use crate::types::{Reminder, ScheduledSend};
use crate::Result;

const SEND_COLS: &str =
    "id, account_id, draft_id, send_at, created_at, remind_after_ms, attempts, last_error";
/// `since` stores `Reminder::sent_at`.
const REMINDER_COLS: &str =
    "id, account_id, thread_id, since, remind_at, created_at, sent_message_id";

fn send_row(r: &Row) -> rusqlite::Result<ScheduledSend> {
    Ok(ScheduledSend {
        id: r.get(0)?,
        account_id: r.get(1)?,
        draft_id: r.get(2)?,
        send_at: r.get(3)?,
        created_at: r.get(4)?,
        remind_after_ms: r.get(5)?,
        attempts: r.get::<_, i64>(6)? as u32,
        last_error: r.get(7)?,
    })
}

fn reminder_row(r: &Row) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: r.get(0)?,
        account_id: r.get(1)?,
        thread_id: r.get(2)?,
        sent_at: r.get(3)?,
        sent_message_id: r.get(6)?,
        remind_at: r.get(4)?,
        created_at: r.get(5)?,
    })
}

impl Store {
    // ----- scheduled sends -----

    /// Insert or replace (same id) a scheduled send.
    pub fn upsert_scheduled_send(&self, s: &ScheduledSend) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                &format!("INSERT OR REPLACE INTO scheduled_sends({SEND_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
                params![
                    s.id,
                    s.account_id,
                    s.draft_id,
                    s.send_at,
                    s.created_at,
                    s.remind_after_ms,
                    s.attempts as i64,
                    s.last_error
                ],
            )?;
            Ok(())
        })
    }

    /// Returns whether it existed.
    pub fn delete_scheduled_send(&self, account_id: &str, id: &str) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM scheduled_sends WHERE account_id = ?1 AND id = ?2",
                params![account_id, id],
            )? > 0)
        })
    }

    pub fn get_scheduled_send(&self, account_id: &str, id: &str) -> Result<Option<ScheduledSend>> {
        self.read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {SEND_COLS} FROM scheduled_sends WHERE account_id = ?1 AND id = ?2"
            ))?
            .query_row(params![account_id, id], send_row)
            .optional()?)
        })
    }

    /// Soonest first; `account_id = None` → all accounts.
    pub fn list_scheduled_sends(&self, account_id: Option<&str>) -> Result<Vec<ScheduledSend>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {SEND_COLS} FROM scheduled_sends WHERE ?1 IS NULL OR account_id = ?1 ORDER BY send_at, id"
            ))?;
            let rows = stmt.query_map([account_id], send_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Schedules for one draft (normally 0 or 1).
    pub fn scheduled_sends_for_draft(
        &self,
        account_id: &str,
        draft_id: &str,
    ) -> Result<Vec<ScheduledSend>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {SEND_COLS} FROM scheduled_sends WHERE account_id = ?1 AND draft_id = ?2 ORDER BY send_at"
            ))?;
            let rows = stmt.query_map(params![account_id, draft_id], send_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Scheduled sends due at `now_ms`, soonest first.
    pub fn due_scheduled_sends(&self, now_ms: i64) -> Result<Vec<ScheduledSend>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {SEND_COLS} FROM scheduled_sends WHERE send_at <= ?1 ORDER BY send_at, id"
            ))?;
            let rows = stmt.query_map([now_ms], send_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Delete by id alone (the UI's cancel). Returns whether it existed.
    pub fn delete_scheduled_send_by_id(&self, id: &str) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM scheduled_sends WHERE id = ?1", [id])? > 0))
    }

    /// Drop a draft's schedules (it was sent by hand or discarded). Returns
    /// how many there were.
    pub fn delete_scheduled_sends_for_draft(
        &self,
        account_id: &str,
        draft_id: &str,
    ) -> Result<usize> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM scheduled_sends WHERE account_id = ?1 AND draft_id = ?2",
                params![account_id, draft_id],
            )?)
        })
    }

    // ----- reminders -----

    pub fn upsert_reminder(&self, r: &Reminder) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                &format!("INSERT OR REPLACE INTO reminders({REMINDER_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
                params![r.id, r.account_id, r.thread_id, r.sent_at, r.remind_at, r.created_at, r.sent_message_id],
            )?;
            Ok(())
        })
    }

    pub fn delete_reminder(&self, account_id: &str, id: &str) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM reminders WHERE account_id = ?1 AND id = ?2",
                params![account_id, id],
            )? > 0)
        })
    }

    pub fn delete_reminder_by_id(&self, id: &str) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM reminders WHERE id = ?1", [id])? > 0))
    }

    /// Soonest first; `account_id = None` → all accounts.
    pub fn list_reminders(&self, account_id: Option<&str>) -> Result<Vec<Reminder>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {REMINDER_COLS} FROM reminders WHERE ?1 IS NULL OR account_id = ?1 ORDER BY remind_at, id"
            ))?;
            let rows = stmt.query_map([account_id], reminder_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn due_reminders(&self, now_ms: i64) -> Result<Vec<Reminder>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {REMINDER_COLS} FROM reminders WHERE remind_at <= ?1 ORDER BY remind_at, id"
            ))?;
            let rows = stmt.query_map([now_ms], reminder_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Earliest `send_at` / `remind_at` across the outbox, for the scheduler.
    pub fn next_outbox_due(&self) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT min(t) FROM (SELECT min(send_at) AS t FROM scheduled_sends
                                     UNION ALL SELECT min(remind_at) FROM reminders)",
                [],
                |r| r.get(0),
            )?)
        })
    }

    /// Whether the thread has a message newer than `after_ms` from someone
    /// other than the account. "Mine" is judged by flags, not From: replies
    /// sent from a send-as alias or another client carry SENT with a
    /// different address. Drafts and spam (auto-reply bots) don't count.
    pub fn has_reply_after(
        &self,
        account_id: &str,
        thread_id: &str,
        after_ms: i64,
    ) -> Result<bool> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
                  WHERE t.account_id = ?1 AND t.thread_id = ?2 AND m.date > ?3 AND (m.flags & ?4) = 0)",
            )?
            .query_row(params![account_id, thread_id, after_ms, F_SENT | F_DRAFT | F_SPAM], |r| r.get(0))?)
        })
    }
}
