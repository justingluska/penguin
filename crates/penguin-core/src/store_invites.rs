//! Calendar invitations in mail: what the invite scanner parsed from each
//! message's calendar part (`invites`), the answers Penguin sent
//! (`invite_responses`), and the row chip built from both plus the synced
//! calendar (`attach_invites`). Parsing itself (iCalendar) happens in
//! penguin-gmail `ics`; this module only stores `InviteSnapshot`s.
//!
//! Everything here is local: list rows never wait on the network.

use rusqlite::{params, OptionalExtension, Transaction};

use super::{Store, KIND_CALENDAR, ROWID_SLOTS};
use crate::types::*;
use crate::Result;

/// `invites.data` is '' for a message whose calendar part had nothing
/// usable (or was your own reply), so the scanner doesn't retry it.
/// `uid` '' and `recurrence_id` 0 mean none.
pub(super) const SCHEMA_INVITES: &str = r#"
CREATE TABLE invites(
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    uid TEXT NOT NULL,
    recurrence_id INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    date INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY(account_id, message_id)
) WITHOUT ROWID;
CREATE INDEX invites_thread ON invites(account_id, thread_id, date);
CREATE INDEX invites_uid ON invites(account_id, uid, recurrence_id, date);
CREATE TABLE invite_responses(
    account_id TEXT NOT NULL,
    uid TEXT NOT NULL,
    recurrence_id INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY(account_id, uid, recurrence_id)
) WITHOUT ROWID;
CREATE INDEX attachments_calendar ON attachments(message_rowid) WHERE kind = 6;
"#;

pub(super) fn remove_account(tx: &Transaction, account_id: &str) -> Result<()> {
    tx.execute("DELETE FROM invites WHERE account_id = ?1", [account_id])?;
    tx.execute(
        "DELETE FROM invite_responses WHERE account_id = ?1",
        [account_id],
    )?;
    Ok(())
}

/// A message with a calendar part the scanner hasn't parsed yet.
#[derive(Debug, Clone, PartialEq)]
pub struct InviteCandidate {
    pub account_id: String,
    pub message_id: String,
    pub thread_id: String,
    pub date: i64,
    /// Its calendar parts (text/calendar, application/ics, *.ics).
    pub parts: Vec<AttachmentMeta>,
}

/// A stored snapshot with where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredInvite {
    pub message_id: String,
    pub thread_id: String,
    pub date: i64,
    pub invite: InviteSnapshot,
}

fn rid(r: Option<i64>) -> i64 {
    r.unwrap_or(0)
}

/// Your answer to show: what Penguin sent (unless a newer version of the
/// invite came since, or the synced calendar changed after it), else the
/// synced event's, else the invite's own PARTSTAT for you. None when you
/// aren't a guest and never answered.
pub fn effective_response(
    invite: &InviteSnapshot,
    synced: Option<&CalendarEvent>,
    sent: Option<&InviteResponse>,
) -> Option<String> {
    let sent = sent.filter(|s| s.sequence >= invite.sequence);
    match (
        sent,
        synced.and_then(|e| e.my_response.as_ref().map(|r| (r, e.updated))),
    ) {
        (Some(s), Some((_, updated))) if s.at >= updated => Some(s.response.clone()),
        (_, Some((r, _))) => Some(r.clone()),
        (Some(s), None) => Some(s.response.clone()),
        (None, None) => invite.me().map(|a| a.response.clone()),
    }
}

/// Whether Yes / Maybe / No make sense for this invite at `now_ms`.
pub fn answerable(invite: &InviteSnapshot, now_ms: i64) -> bool {
    let organizer_is_me = invite.organizer.as_ref().is_some_and(|o| {
        invite
            .attendees
            .iter()
            .any(|a| a.is_self && a.email == o.email)
    }) || invite.me().is_some_and(|a| a.organizer);
    invite.method == "request"
        && invite.uid.is_some()
        && invite.organizer.is_some()
        && !organizer_is_me
        && (is_series(invite) || invite.end.max(invite.start) > now_ms)
}

/// An invitation to a whole recurring series (not one instance of it).
pub fn is_series(invite: &InviteSnapshot) -> bool {
    invite.recurring && invite.recurrence_id.is_none()
}

/// When to show it: a series' next instance from the synced calendar when
/// there is one, else the invite's own time.
pub fn shown_time(invite: &InviteSnapshot, synced: Option<&CalendarEvent>) -> (i64, i64) {
    match synced {
        Some(e) if is_series(invite) => (e.start, e.end),
        _ => (invite.start, invite.end),
    }
}

impl Store {
    /// Messages dated `since_ms` or later with a calendar part and no
    /// `invites` row yet, newest first. A partial-index scan.
    pub fn invite_scan_candidates(
        &self,
        since_ms: i64,
        limit: usize,
    ) -> Result<Vec<InviteCandidate>> {
        let lo = since_ms.max(0).saturating_mul(ROWID_SLOTS);
        self.read(|c| {
            let rows: Vec<(i64, String, String, String, i64)> = c
                .prepare_cached(
                    "SELECT DISTINCT m.rowid, m.account_id, m.id, t.thread_id, m.date
                     FROM attachments a INDEXED BY attachments_calendar
                     JOIN messages m ON m.rowid = a.message_rowid
                     JOIN threads t ON t.rowid = m.thread_rowid
                     WHERE a.kind = 6 AND a.message_rowid >= ?1
                       AND NOT EXISTS (SELECT 1 FROM invites i WHERE i.account_id = m.account_id AND i.message_id = m.id)
                     ORDER BY m.rowid DESC LIMIT ?2",
                )?
                .query_map(params![lo, limit as i64], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let mut parts = c.prepare_cached(
                "SELECT att_id, filename, mime_type, size, content_id, inline FROM attachments
                 WHERE message_rowid = ?1 AND kind = ?2 ORDER BY ord",
            )?;
            let mut out = Vec::with_capacity(rows.len());
            for (rowid, account_id, message_id, thread_id, date) in rows {
                let parts = parts
                    .query_map(params![rowid, KIND_CALENDAR], |r| {
                        Ok(AttachmentMeta {
                            id: r.get(0)?,
                            filename: r.get(1)?,
                            mime_type: r.get(2)?,
                            size: r.get::<_, i64>(3)? as u64,
                            content_id: r.get(4)?,
                            inline: r.get(5)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                out.push(InviteCandidate {
                    account_id,
                    message_id,
                    thread_id,
                    date,
                    parts,
                });
            }
            Ok(out)
        })
    }

    /// Store what a message's calendar part says (None = nothing usable).
    pub fn put_invite(
        &self,
        account_id: &str,
        message_id: &str,
        thread_id: &str,
        date: i64,
        invite: Option<&InviteSnapshot>,
    ) -> Result<()> {
        let data = match invite {
            Some(i) => serde_json::to_string(i)?,
            None => String::new(),
        };
        let (uid, recurrence, sequence) = invite
            .map(|i| {
                (
                    i.uid.clone().unwrap_or_default(),
                    rid(i.recurrence_id),
                    i.sequence,
                )
            })
            .unwrap_or_default();
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO invites(account_id, message_id, thread_id, uid, recurrence_id, sequence, date, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![account_id, message_id, thread_id, uid, recurrence, sequence, date, data])?;
            Ok(())
        })
    }

    /// The stored snapshot of one message (None: not scanned or nothing usable).
    pub fn stored_invite(
        &self,
        account_id: &str,
        message_id: &str,
    ) -> Result<Option<StoredInvite>> {
        self.read(|c| {
            let row: Option<(String, i64, String)> = c
                .prepare_cached(
                    "SELECT thread_id, date, data FROM invites WHERE account_id = ?1 AND message_id = ?2",
                )?
                .query_row(params![account_id, message_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            Ok(row.and_then(|(thread_id, date, data)| {
                Some(StoredInvite {
                    message_id: message_id.to_string(),
                    thread_id,
                    date,
                    invite: serde_json::from_str(&data).ok()?,
                })
            }))
        })
    }

    /// The newest stored invitation of a thread whose message still exists.
    pub fn thread_invite(&self, account_id: &str, thread_id: &str) -> Result<Option<StoredInvite>> {
        self.read(|c| {
            let row: Option<(String, i64, String)> = c
                .prepare_cached(
                    "SELECT i.message_id, i.date, i.data FROM invites i
                     JOIN messages m ON m.account_id = i.account_id AND m.id = i.message_id
                     WHERE i.account_id = ?1 AND i.thread_id = ?2 AND i.data != ''
                     ORDER BY i.date DESC LIMIT 1",
                )?
                .query_row(params![account_id, thread_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            Ok(row.and_then(|(message_id, date, data)| {
                Some(StoredInvite {
                    message_id,
                    thread_id: thread_id.to_string(),
                    date,
                    invite: serde_json::from_str(&data).ok()?,
                })
            }))
        })
    }

    /// The version of the same event (UID and instance) you had before
    /// `invite`: the newest request/publish stored for it that is older by
    /// SEQUENCE, or as new but from an earlier message. For "What changed".
    pub fn previous_invite(
        &self,
        account_id: &str,
        invite: &InviteSnapshot,
        message_id: &str,
        date: i64,
    ) -> Result<Option<InviteSnapshot>> {
        let Some(uid) = invite.uid.as_deref() else {
            return Ok(None);
        };
        self.read(|c| {
            let rows: Vec<String> = c
                .prepare_cached(
                    "SELECT data FROM invites
                     WHERE account_id = ?1 AND uid = ?2 AND recurrence_id = ?3 AND data != ''
                       AND message_id != ?4
                       AND (sequence < ?5 OR (sequence = ?5 AND date < ?6))
                     ORDER BY sequence DESC, date DESC LIMIT 8",
                )?
                .query_map(
                    params![
                        account_id,
                        uid,
                        rid(invite.recurrence_id),
                        message_id,
                        invite.sequence,
                        date
                    ],
                    |r| r.get(0),
                )?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows
                .iter()
                .filter_map(|d| serde_json::from_str::<InviteSnapshot>(d).ok())
                .find(|s| matches!(s.method.as_str(), "request" | "publish")))
        })
    }

    /// Threads of the account that carry this UID (to refresh their rows).
    pub fn invite_threads(&self, account_id: &str, uid: &str) -> Result<Vec<String>> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT DISTINCT thread_id FROM invites WHERE account_id = ?1 AND uid = ?2 LIMIT 50",
            )?
            .query_map(params![account_id, uid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Remember an answer Penguin sent.
    pub fn record_invite_response(
        &self,
        account_id: &str,
        uid: &str,
        recurrence_id: Option<i64>,
        response: &InviteResponse,
    ) -> Result<()> {
        let data = serde_json::to_string(response)?;
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO invite_responses(account_id, uid, recurrence_id, data) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![account_id, uid, rid(recurrence_id), data])?;
            Ok(())
        })
    }

    /// The answer Penguin sent for this event (instance), if any. An answer
    /// to the whole series counts for its instances too.
    pub fn invite_response(
        &self,
        account_id: &str,
        uid: &str,
        recurrence_id: Option<i64>,
    ) -> Result<Option<InviteResponse>> {
        self.read(|c| {
            let mut q = c.prepare_cached(
                "SELECT data FROM invite_responses WHERE account_id = ?1 AND uid = ?2 AND recurrence_id = ?3",
            )?;
            let mut get = |r: i64| -> Result<Option<InviteResponse>> {
                let d: Option<String> = q.query_row(params![account_id, uid, r], |r| r.get(0)).optional()?;
                Ok(d.and_then(|d| serde_json::from_str(&d).ok()))
            };
            let own = get(rid(recurrence_id))?;
            let series = if recurrence_id.is_some() { get(0)? } else { None };
            Ok(match (own, series) {
                (Some(a), Some(b)) => Some(if a.at >= b.at { a } else { b }),
                (a, b) => a.or(b),
            })
        })
    }

    /// Fill `invite` on rows that carry an invitation (only rows with
    /// attachments are looked at: a primary-key lookup each).
    pub fn attach_invites(&self, rows: &mut [ThreadSummary], now_ms: i64) -> Result<()> {
        for t in rows.iter_mut().filter(|t| t.has_attachments) {
            let Some(stored) = self.thread_invite(&t.account_id, &t.thread_id)? else {
                continue;
            };
            t.invite = Some(self.invite_chip(&t.account_id, &stored, now_ms)?);
        }
        Ok(())
    }

    /// The synced calendar copy of an invitation in this account: the
    /// instance it is about, or for a series the next one that hasn't ended.
    pub fn synced_copy(
        &self,
        account_id: &str,
        invite: &InviteSnapshot,
        now_ms: i64,
    ) -> Result<Option<CalendarEvent>> {
        let Some(uid) = invite.uid.as_deref() else {
            return Ok(None);
        };
        let near = invite.recurrence_id.unwrap_or(if is_series(invite) {
            invite.start.max(now_ms)
        } else {
            invite.start
        });
        let candidates: Vec<CalendarEvent> = self
            .events_by_uid(uid, Some(account_id), Some(near))?
            .into_iter()
            .filter(|e| e.account_id == account_id)
            .collect();
        Ok(if is_series(invite) {
            candidates
                .iter()
                .filter(|e| e.end > now_ms)
                .min_by_key(|e| e.start)
                .or(candidates.first())
                .cloned()
        } else {
            candidates.into_iter().next()
        })
    }

    /// The chip for one stored invitation.
    pub fn invite_chip(
        &self,
        account_id: &str,
        stored: &StoredInvite,
        now_ms: i64,
    ) -> Result<InviteChip> {
        let inv = &stored.invite;
        let synced = self.synced_copy(account_id, inv, now_ms)?;
        let sent = match inv.uid.as_deref() {
            Some(uid) => self.invite_response(account_id, uid, inv.recurrence_id)?,
            None => None,
        };
        let can_respond = answerable(inv, now_ms);
        let (start, end) = shown_time(inv, synced.as_ref());
        let updated = inv.method == "request"
            && (inv.sequence > 0
                || self
                    .previous_invite(account_id, inv, &stored.message_id, stored.date)?
                    .is_some());
        let (conflict, conflicts) = if can_respond && !inv.all_day {
            let c = self.event_conflicts(start, end, inv.uid.as_deref(), None)?;
            (c.first().map(|e| e.summary.clone()), c.len() as u32)
        } else {
            (None, 0)
        };
        let replier = if matches!(inv.method.as_str(), "reply" | "counter") {
            inv.attendees.iter().find(|a| !a.is_self).cloned()
        } else {
            None
        };
        Ok(InviteChip {
            message_id: stored.message_id.clone(),
            uid: inv.uid.clone(),
            method: inv.method.clone(),
            updated,
            summary: inv.summary.clone(),
            start,
            end,
            all_day: inv.all_day,
            start_date: inv.start_date.clone(),
            end_date: inv.end_date.clone(),
            recurring: inv.recurring,
            response: if matches!(inv.method.as_str(), "request" | "publish") {
                effective_response(inv, synced.as_ref(), sent.as_ref())
            } else {
                None
            },
            replier,
            can_respond,
            conflict,
            conflicts,
        })
    }
}

#[cfg(test)]
#[path = "store_invites_tests.rs"]
mod tests;
