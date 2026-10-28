//! Outbox: the send-later schedule and remind-if-no-reply reminders.
//! The firing logic lives in penguin-provider's `outbox` module; this is
//! persistence, plus each schedule's and reminder's synced record
//! (penguin_sync::outbox; docs/CROSS-DEVICE-STATE.md, "The outbox").
//!
//! A user's intent (schedule, cancel, remind, replace) writes the record in
//! the same transaction as the row: [`Store::add_scheduled_send`],
//! [`Store::replace_scheduled_sends`], [`Store::delete_scheduled_send`] and
//! the other deletes, [`Store::add_reminder`], [`Store::delete_reminder`].
//! How it ended (sent, fired, gone) is a terminal phase:
//! [`Store::finish_scheduled_send`], [`Store::finish_reminder`]. A retry
//! only moves the local row ([`Store::upsert_scheduled_send`],
//! [`Store::upsert_reminder`]). Schedules from another device are rows with
//! an `owner`; before sending anything the scheduler asks
//! [`Store::scheduled_send_turn`] whose turn it is, and
//! [`Store::reminder_turn`] before firing a reminder.

use penguin_sync::outbox::{
    self as sync_outbox, Outcome, ReminderBody, ScheduledSendBody, Turn,
};
use penguin_sync::{thread_key, Kind, Record};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::{sync, triage, Store, F_DRAFT, F_SENT, F_SPAM};
use crate::types::{Reminder, ScheduledSend};
use crate::Result;

/// Schedules and reminders learn who sends them: `owner` is the device a
/// schedule from another device belongs to (NULL: this one); `wait_until`
/// holds a due row back while it isn't this device's turn.
pub(super) const SCHEMA_OUTBOX_SYNC: &str = r#"
ALTER TABLE scheduled_sends ADD COLUMN owner TEXT;
ALTER TABLE scheduled_sends ADD COLUMN wait_until INTEGER;
ALTER TABLE reminders ADD COLUMN wait_until INTEGER;
ALTER TABLE sync_state ADD COLUMN outbox_seeded INTEGER NOT NULL DEFAULT 0;
"#;

const SEND_COLS: &str = "id, account_id, draft_id, send_at, created_at, remind_after_ms, attempts, last_error, owner IS NOT NULL";
/// When a row is due: its time, or later while it waits for its turn.
const SEND_DUE: &str = "max(send_at, coalesce(wait_until, 0))";
/// `since` stores `Reminder::sent_at`.
const REMINDER_COLS: &str =
    "id, account_id, thread_id, since, remind_at, created_at, sent_message_id";
const REMINDER_DUE: &str = "max(remind_at, coalesce(wait_until, 0))";

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
        elsewhere: r.get(8)?,
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

    /// Insert or update (same id) a scheduled send's row without a synced
    /// record: a retry moving it on. Who owns it is kept, and it is due at
    /// its (new) time again. A new schedule is [`Store::add_scheduled_send`].
    pub fn upsert_scheduled_send(&self, s: &ScheduledSend) -> Result<()> {
        self.write(|tx| put_send_row(tx, s, None))
    }

    /// A new schedule made on this device: the row, and its record (this
    /// device owns it and sends it).
    pub fn add_scheduled_send(&self, s: &ScheduledSend) -> Result<()> {
        self.write(|tx| add_send(tx, s, triage::now_ms()))
    }

    /// `s` replaces every other schedule of its draft (rescheduling): the
    /// old ones are cancelled for every device and `s` is added, in one
    /// transaction.
    pub fn replace_scheduled_sends(&self, s: &ScheduledSend) -> Result<()> {
        self.write(|tx| {
            let now = triage::now_ms();
            let old: Vec<String> = tx
                .prepare_cached(
                    "SELECT id FROM scheduled_sends WHERE account_id = ?1 AND draft_id = ?2 AND id != ?3",
                )?
                .query_map(params![s.account_id, s.draft_id, s.id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in old {
                cancel_send(tx, &s.account_id, &id, now)?;
            }
            add_send(tx, s, now)
        })
    }

    /// Cancel a schedule (the user, or its draft was sent by hand or
    /// discarded): gone here and, through its record, everywhere. Returns
    /// whether it existed here.
    pub fn delete_scheduled_send(&self, account_id: &str, id: &str) -> Result<bool> {
        self.write(|tx| cancel_send(tx, account_id, id, triage::now_ms()))
    }

    /// How a schedule ended (sent, its draft gone, refused for good): the
    /// row goes and its record takes the terminal phase, which no other
    /// device's version can undo.
    pub fn finish_scheduled_send(&self, account_id: &str, id: &str, outcome: Outcome) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM scheduled_sends WHERE account_id = ?1 AND id = ?2",
                params![account_id, id],
            )?;
            finish(tx, Kind::ScheduledSend, account_id, id, outcome)
        })
    }

    /// Whether this device may send the schedule `id` now (see
    /// penguin_sync::outbox). A claim it has to make is written here (and
    /// sent by the transport); a row that must wait is held back until the
    /// time returned; a row that is gone (sent or cancelled elsewhere) is
    /// deleted. A schedule without a record (none was ever written) is this
    /// device's own and sends. Also used for schedules kept outside this
    /// table (the iPhone's outbox, [`Store::publish_scheduled_send`]).
    pub fn scheduled_send_turn(&self, account_id: &str, id: &str, now_ms: i64) -> Result<Turn> {
        self.write(|tx| {
            let key = thread_key(account_id, id);
            let Some(stored) = sync::load(tx, Kind::ScheduledSend, &key)? else {
                let foreign: bool = tx
                    .prepare_cached(
                        "SELECT coalesce((SELECT owner IS NOT NULL FROM scheduled_sends
                                          WHERE account_id = ?1 AND id = ?2), 0)",
                    )?
                    .query_row(params![account_id, id], |r| r.get(0))?;
                return if foreign {
                    hold_send(tx, account_id, id, now_ms + sync_outbox::IDLE_RECHECK_MS)
                } else {
                    Ok(Turn::Send)
                };
            };
            let me = sync::device(tx)?;
            let on_server = sync::on_server(tx, &stored.record.name())?;
            let takeover = gmail_account(tx, account_id)?;
            let sync_on = sync::enabled(tx)?;
            let turn =
                sync_outbox::send_turn(&stored.record, on_server, &me, now_ms, takeover, sync_on);
            match &turn {
                Turn::Send => {}
                Turn::Gone => {
                    tx.execute(
                        "DELETE FROM scheduled_sends WHERE account_id = ?1 AND id = ?2",
                        params![account_id, id],
                    )?;
                }
                Turn::Wait { until } => {
                    hold_send(tx, account_id, id, *until)?;
                }
                Turn::Claim { phase, body, until } => {
                    sync::emit_phase(tx, Kind::ScheduledSend, &key, *phase, now_ms, |_| {
                        Some(body.clone())
                    })?;
                    tracing::info!(account = %account_id, phase, "claimed a scheduled send");
                    hold_send(tx, account_id, id, *until)?;
                }
            }
            Ok(turn)
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

    /// Soonest first; `account_id = None` → all accounts. Includes
    /// schedules other devices own (`elsewhere`).
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

    /// Scheduled sends due at `now_ms`, soonest first (a row waiting for its
    /// turn is due when the wait ends).
    pub fn due_scheduled_sends(&self, now_ms: i64) -> Result<Vec<ScheduledSend>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {SEND_COLS} FROM scheduled_sends WHERE {SEND_DUE} <= ?1 ORDER BY send_at, id"
            ))?;
            let rows = stmt.query_map([now_ms], send_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Cancel by id alone (the UI's cancel). Returns whether it existed.
    pub fn delete_scheduled_send_by_id(&self, id: &str) -> Result<bool> {
        self.write(|tx| {
            let account: Option<String> = tx
                .prepare_cached("SELECT account_id FROM scheduled_sends WHERE id = ?1")?
                .query_row([id], |r| r.get(0))
                .optional()?;
            match account {
                Some(a) => cancel_send(tx, &a, id, triage::now_ms()),
                None => Ok(false),
            }
        })
    }

    /// Cancel a draft's schedules (it was sent by hand or discarded).
    /// Returns how many there were.
    pub fn delete_scheduled_sends_for_draft(
        &self,
        account_id: &str,
        draft_id: &str,
    ) -> Result<usize> {
        self.write(|tx| {
            let ids: Vec<String> = tx
                .prepare_cached(
                    "SELECT id FROM scheduled_sends WHERE account_id = ?1 AND draft_id = ?2",
                )?
                .query_map(params![account_id, draft_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let now = triage::now_ms();
            for id in &ids {
                cancel_send(tx, account_id, id, now)?;
            }
            Ok(ids.len())
        })
    }

    // ----- schedules kept outside this table (the iPhone's own outbox) -----

    /// Publish a schedule this device keeps elsewhere (the iPhone's file
    /// outbox) under its id: its record only, owned by this device, so
    /// other devices show it, can cancel it and, with a `draft_id`, take it
    /// over. Publishing it again (a new time) is a new intent.
    pub fn publish_scheduled_send(&self, id: &str, body: &ScheduledSendBody) -> Result<()> {
        self.write(|tx| {
            let mut b = body.clone();
            b.owner = sync::device(tx)?;
            b.claimed_by = None;
            b.claimed_at = None;
            b.outcome = None;
            sync::emit_intent(
                tx,
                Kind::ScheduledSend,
                &thread_key(&b.account_id, id),
                Some(b.to_value()),
                triage::now_ms(),
            )?;
            Ok(())
        })
    }

    /// Cancel a published schedule (taken back or deleted here).
    pub fn unpublish_scheduled_send(&self, account_id: &str, id: &str) -> Result<()> {
        self.write(|tx| tombstone(tx, Kind::ScheduledSend, account_id, id, triage::now_ms()))
    }

    /// The record of a published schedule, if any (tombstones and terminal
    /// phases included).
    pub fn scheduled_send_record(&self, account_id: &str, id: &str) -> Result<Option<Record>> {
        self.read(|c| {
            Ok(sync::load(c, Kind::ScheduledSend, &thread_key(account_id, id))?.map(|s| s.record))
        })
    }

    // ----- reminders -----

    /// Insert or update a reminder's row without a synced record (a retry
    /// moving it on). A new reminder is [`Store::add_reminder`].
    pub fn upsert_reminder(&self, r: &Reminder) -> Result<()> {
        self.write(|tx| put_reminder_row(tx, r))
    }

    /// A new reminder (set by the user, or armed by a send): the row and
    /// its record, so every device knows it.
    pub fn add_reminder(&self, r: &Reminder) -> Result<()> {
        self.write(|tx| add_reminder(tx, r, triage::now_ms()))
    }

    /// Cancel a reminder (the user, or a new one replaces it), here and
    /// everywhere. Returns whether it existed here.
    pub fn delete_reminder(&self, account_id: &str, id: &str) -> Result<bool> {
        self.write(|tx| cancel_reminder(tx, account_id, id, triage::now_ms()))
    }

    pub fn delete_reminder_by_id(&self, id: &str) -> Result<bool> {
        self.write(|tx| {
            let account: Option<String> = tx
                .prepare_cached("SELECT account_id FROM reminders WHERE id = ?1")?
                .query_row([id], |r| r.get(0))
                .optional()?;
            match account {
                Some(a) => cancel_reminder(tx, &a, id, triage::now_ms()),
                None => Ok(false),
            }
        })
    }

    /// `r` replaces the other reminders on its conversation, in one
    /// transaction (each cancelled everywhere).
    pub fn replace_reminders(&self, r: &Reminder) -> Result<()> {
        self.write(|tx| {
            let now = triage::now_ms();
            let old: Vec<String> = tx
                .prepare_cached(
                    "SELECT id FROM reminders WHERE account_id = ?1 AND thread_id = ?2 AND id != ?3",
                )?
                .query_map(params![r.account_id, r.thread_id, r.id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in old {
                cancel_reminder(tx, &r.account_id, &id, now)?;
            }
            add_reminder(tx, r, now)
        })
    }

    /// How a reminder ended (fired, answered, its thread gone): the row goes
    /// and its record takes the terminal phase.
    pub fn finish_reminder(&self, account_id: &str, id: &str, outcome: Outcome) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM reminders WHERE account_id = ?1 AND id = ?2",
                params![account_id, id],
            )?;
            finish(tx, Kind::Reminder, account_id, id, outcome)
        })
    }

    /// Whether to fire the due reminder `id` now: with sync on, only after
    /// a pull that completed after it was due (it may have been cancelled
    /// or fired elsewhere); until then the row waits and the caller should
    /// ask for a fetch. A reminder gone elsewhere is deleted here.
    pub fn reminder_turn(&self, account_id: &str, id: &str, now_ms: i64) -> Result<Turn> {
        self.write(|tx| {
            let key = thread_key(account_id, id);
            let Some(stored) = sync::load(tx, Kind::Reminder, &key)? else {
                return Ok(Turn::Send);
            };
            let remind_at = stored
                .record
                .body_as::<ReminderBody>()
                .map_or(now_ms, |b| b.remind_at);
            let fresh = sync::pull_fresh(tx, remind_at)?;
            let turn = sync_outbox::reminder_turn(&stored.record, now_ms, fresh);
            match &turn {
                Turn::Gone => {
                    tx.execute(
                        "DELETE FROM reminders WHERE account_id = ?1 AND id = ?2",
                        params![account_id, id],
                    )?;
                }
                Turn::Wait { until } => {
                    tx.execute(
                        "UPDATE reminders SET wait_until = ?3 WHERE account_id = ?1 AND id = ?2",
                        params![account_id, id, until],
                    )?;
                }
                _ => {}
            }
            Ok(turn)
        })
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
                "SELECT {REMINDER_COLS} FROM reminders WHERE {REMINDER_DUE} <= ?1 ORDER BY remind_at, id"
            ))?;
            let rows = stmt.query_map([now_ms], reminder_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Earliest time the outbox has something due (a send or a reminder,
    /// after any wait for its turn), for the scheduler.
    pub fn next_outbox_due(&self) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT min(t) FROM (SELECT min({SEND_DUE}) AS t FROM scheduled_sends
                                         UNION ALL SELECT min({REMINDER_DUE}) FROM reminders)"
                ),
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

/// Write a schedule's row. `owner`: Some for another device's schedule
/// (set), None to keep whatever owner the row has.
fn put_send_row(c: &Connection, s: &ScheduledSend, owner: Option<&str>) -> Result<()> {
    c.prepare_cached(
        "INSERT INTO scheduled_sends(id, account_id, draft_id, send_at, created_at, remind_after_ms,
                                     attempts, last_error, owner, wait_until)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)
         ON CONFLICT(id) DO UPDATE SET account_id = excluded.account_id, draft_id = excluded.draft_id,
           send_at = excluded.send_at, created_at = excluded.created_at,
           remind_after_ms = excluded.remind_after_ms, attempts = excluded.attempts,
           last_error = excluded.last_error, wait_until = NULL,
           owner = coalesce(excluded.owner, scheduled_sends.owner)",
    )?
    .execute(params![
        s.id,
        s.account_id,
        s.draft_id,
        s.send_at,
        s.created_at,
        s.remind_after_ms,
        s.attempts as i64,
        s.last_error,
        owner,
    ])?;
    Ok(())
}

fn add_send(c: &Connection, s: &ScheduledSend, now: i64) -> Result<()> {
    put_send_row(c, s, None)?;
    // A schedule made here is this device's, whatever the row said before.
    c.execute("UPDATE scheduled_sends SET owner = NULL WHERE id = ?1", [&s.id])?;
    let body = ScheduledSendBody {
        account_id: s.account_id.clone(),
        draft_id: s.draft_id.clone(),
        send_at: s.send_at,
        created_at: s.created_at,
        remind_after_ms: s.remind_after_ms,
        owner: sync::device(c)?,
        claimed_by: None,
        claimed_at: None,
        outcome: None,
    };
    sync::emit_intent(
        c,
        Kind::ScheduledSend,
        &thread_key(&s.account_id, &s.id),
        Some(body.to_value()),
        now,
    )?;
    Ok(())
}

/// The row goes; its record (if it has one) becomes a tombstone.
fn cancel_send(c: &Connection, account_id: &str, id: &str, now: i64) -> Result<bool> {
    let existed = c.execute(
        "DELETE FROM scheduled_sends WHERE account_id = ?1 AND id = ?2",
        params![account_id, id],
    )? > 0;
    tombstone(c, Kind::ScheduledSend, account_id, id, now)?;
    Ok(existed)
}

fn add_reminder(c: &Connection, r: &Reminder, now: i64) -> Result<()> {
    put_reminder_row(c, r)?;
    let body = ReminderBody {
        account_id: r.account_id.clone(),
        thread_id: r.thread_id.clone(),
        sent_at: r.sent_at,
        remind_at: r.remind_at,
        created_at: r.created_at,
        sent_message_id: r.sent_message_id.clone(),
        outcome: None,
    };
    sync::emit_intent(
        c,
        Kind::Reminder,
        &thread_key(&r.account_id, &r.id),
        Some(body.to_value()),
        now,
    )?;
    Ok(())
}

fn cancel_reminder(c: &Connection, account_id: &str, id: &str, now: i64) -> Result<bool> {
    let existed = c.execute(
        "DELETE FROM reminders WHERE account_id = ?1 AND id = ?2",
        params![account_id, id],
    )? > 0;
    tombstone(c, Kind::Reminder, account_id, id, now)?;
    Ok(existed)
}

/// A cancel for every device: a new generation with no body. Nothing to
/// cancel when it never had a record, or already ended.
fn tombstone(c: &Connection, kind: Kind, account_id: &str, id: &str, now: i64) -> Result<()> {
    let key = thread_key(account_id, id);
    if let Some(s) = sync::load(c, kind, &key)? {
        if !s.record.is_deleted() && !sync_outbox::is_terminal(&s.record) {
            sync::emit_intent(c, kind, &key, None, now)?;
        }
    }
    Ok(())
}

fn finish(c: &Connection, kind: Kind, account_id: &str, id: &str, outcome: Outcome) -> Result<()> {
    let key = thread_key(account_id, id);
    sync::emit_phase(
        c,
        kind,
        &key,
        sync_outbox::PHASE_TERMINAL,
        triage::now_ms(),
        |r: &Record| {
            if sync_outbox::is_terminal(r) {
                None
            } else {
                sync_outbox::finished_body(r, outcome)
            }
        },
    )?;
    Ok(())
}

fn hold_send(c: &Connection, account_id: &str, id: &str, until: i64) -> Result<Turn> {
    c.execute(
        "UPDATE scheduled_sends SET wait_until = ?3 WHERE account_id = ?1 AND id = ?2",
        params![account_id, id, until],
    )?;
    Ok(Turn::Wait { until })
}

fn put_reminder_row(c: &Connection, r: &Reminder) -> Result<()> {
    c.prepare_cached(
        "INSERT INTO reminders(id, account_id, thread_id, since, remind_at, created_at, sent_message_id, wait_until)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
         ON CONFLICT(id) DO UPDATE SET account_id = excluded.account_id, thread_id = excluded.thread_id,
           since = excluded.since, remind_at = excluded.remind_at, created_at = excluded.created_at,
           sent_message_id = excluded.sent_message_id, wait_until = NULL",
    )?
    .execute(params![
        r.id,
        r.account_id,
        r.thread_id,
        r.sent_at,
        r.remind_at,
        r.created_at,
        r.sent_message_id
    ])?;
    Ok(())
}

/// Whether another device could send this account's schedules too: a
/// provider whose send consumes a server draft (Gmail's drafts.send).
fn gmail_account(c: &Connection, account_id: &str) -> Result<bool> {
    Ok(c
        .prepare_cached("SELECT provider = 'gmail' FROM accounts WHERE id = ?1")?
        .query_row([account_id], |r| r.get(0))
        .optional()?
        .unwrap_or(false))
}

/// A schedule or reminder record that won here: make the tables match
/// (store_sync's `project`).
pub(super) fn project_remote(c: &Connection, account_id: &str, id: &str, rec: &Record) -> Result<()> {
    let live = !rec.is_deleted() && !sync_outbox::is_terminal(rec);
    match rec.kind {
        Kind::ScheduledSend => {
            let Some(b) = rec.body_as::<ScheduledSendBody>().filter(|_| live) else {
                c.execute(
                    "DELETE FROM scheduled_sends WHERE account_id = ?1 AND id = ?2",
                    params![account_id, id],
                )?;
                return Ok(());
            };
            if b.owner == sync::device(c)? {
                // Ours (another device claimed it, say): the row stays ours;
                // whose turn it is is read from the record when it's due.
                return Ok(());
            }
            let row = ScheduledSend {
                id: id.to_string(),
                account_id: account_id.to_string(),
                draft_id: b.draft_id.clone(),
                send_at: b.send_at,
                created_at: b.created_at,
                remind_after_ms: b.remind_after_ms,
                attempts: 0,
                last_error: None,
                elsewhere: true,
            };
            put_send_row(c, &row, Some(&b.owner))?;
            // Not due here before another device may take it over.
            c.execute(
                "UPDATE scheduled_sends SET wait_until = ?2 WHERE id = ?1",
                params![id, b.send_at + sync_outbox::TAKEOVER_AFTER_MS],
            )?;
        }
        Kind::Reminder => match rec.body_as::<ReminderBody>().filter(|_| live) {
            Some(b) => put_reminder_row(
                c,
                &Reminder {
                    id: id.to_string(),
                    account_id: account_id.to_string(),
                    thread_id: b.thread_id,
                    sent_message_id: b.sent_message_id,
                    sent_at: b.sent_at,
                    remind_at: b.remind_at,
                    created_at: b.created_at,
                },
            )?,
            None => {
                c.execute(
                    "DELETE FROM reminders WHERE account_id = ?1 AND id = ?2",
                    params![account_id, id],
                )?;
            }
        },
        _ => {}
    }
    Ok(())
}

/// Records for schedules and reminders from before they synced, once (at
/// open, like store_sync's `seed`).
pub(super) fn seed(c: &Connection, now: i64) -> Result<()> {
    let done: bool = c.query_row(
        "SELECT outbox_seeded FROM sync_state WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    if done {
        return Ok(());
    }
    let sends: Vec<ScheduledSend> = c
        .prepare(&format!(
            "SELECT {SEND_COLS} FROM scheduled_sends WHERE owner IS NULL"
        ))?
        .query_map([], send_row)?
        .collect::<rusqlite::Result<_>>()?;
    for s in sends {
        if sync::load(c, Kind::ScheduledSend, &thread_key(&s.account_id, &s.id))?.is_none() {
            add_send(c, &s, now)?;
        }
    }
    let reminders: Vec<Reminder> = c
        .prepare(&format!("SELECT {REMINDER_COLS} FROM reminders"))?
        .query_map([], reminder_row)?
        .collect::<rusqlite::Result<_>>()?;
    for r in reminders {
        if sync::load(c, Kind::Reminder, &thread_key(&r.account_id, &r.id))?.is_none() {
            add_reminder(c, &r, now)?;
        }
    }
    c.execute("UPDATE sync_state SET outbox_seeded = 1 WHERE id = 1", [])?;
    Ok(())
}

#[cfg(test)]
#[path = "store_outbox_tests.rs"]
mod tests;
