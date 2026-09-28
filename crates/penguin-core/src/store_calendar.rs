//! Google Calendar events: persistence, the agenda/week queries, "meetings
//! with X", conflicts and event search. OWNER: calendar agent. Sync logic
//! lives in penguin-gmail's `calendar` module; this is only storage.
//!
//! Layout (`SCHEMA_CALENDAR`):
//! - `calendars` holds each account's calendar list plus the per-calendar
//!   sync cursor (Google `nextSyncToken` and the window it was taken for).
//!   `selected` is local: unselected calendars keep no events.
//! - `events` has one row per expanded instance (recurring series are
//!   fetched with `singleEvents=true`), keyed by (account, calendar, id).
//!   `start_ms`/`end_ms` drive every range query via `events_start`.
//! - `event_people` indexes the other people on an event (attendees and
//!   organizer, lowercased, minus the account itself and rooms) for the
//!   person card's last/next meeting.
//! - `events_fts` is contentless (text lives in `events`), rowid = events.rowid.

use std::collections::HashSet;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, Transaction};

use super::Store;
use crate::query::{Atom, Field, Folder, ParsedQuery};
use crate::types::{Address, CalendarEvent, CalendarInfo, EventAttendee, PersonMeetings};
use crate::Result;

pub(super) const SCHEMA_CALENDAR: &str = r#"
CREATE TABLE calendars(
    account_id TEXT NOT NULL,
    id TEXT NOT NULL,
    summary TEXT NOT NULL,
    color TEXT,
    selected INTEGER NOT NULL,
    is_primary INTEGER NOT NULL DEFAULT 0,
    access_role TEXT NOT NULL,
    time_zone TEXT,
    sync_token TEXT,
    window_start INTEGER,
    window_end INTEGER,
    synced_at INTEGER,
    PRIMARY KEY(account_id, id)
) WITHOUT ROWID;
CREATE TABLE events(
    rowid INTEGER PRIMARY KEY,
    account_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    ical_uid TEXT,
    status TEXT NOT NULL,
    summary TEXT NOT NULL,
    description TEXT NOT NULL,
    location TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    all_day INTEGER NOT NULL,
    start_date TEXT,
    end_date TEXT,
    organizer_email TEXT,
    organizer_name TEXT,
    attendees TEXT NOT NULL,
    my_response TEXT,
    html_link TEXT,
    conference_url TEXT,
    conference_kind TEXT,
    recurring_event_id TEXT,
    free INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    UNIQUE(account_id, calendar_id, event_id)
);
CREATE INDEX events_start ON events(start_ms);
CREATE INDEX events_uid ON events(ical_uid) WHERE ical_uid IS NOT NULL;
CREATE TABLE event_people(
    email TEXT NOT NULL,
    event_rowid INTEGER NOT NULL,
    declined INTEGER NOT NULL,
    PRIMARY KEY(email, event_rowid)
) WITHOUT ROWID;
CREATE INDEX event_people_event ON event_people(event_rowid);
CREATE VIRTUAL TABLE events_fts USING fts5(
    summary, description, location, organizer, attendees,
    content='', contentless_delete=1,
    tokenize='unicode61 remove_diacritics 2',
    prefix='3'
);
"#;

/// Per-calendar sync state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalendarCursor {
    /// Google `nextSyncToken`; None = a full sync is needed.
    pub sync_token: Option<String>,
    /// The window (unix ms) the stored events were fetched for.
    pub window_start: Option<i64>,
    pub window_end: Option<i64>,
    pub synced_at: Option<i64>,
}

/// Account-level numbers for Settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalendarCounts {
    pub events: u64,
    /// Oldest successful sync among selected calendars (unix ms).
    pub synced_at: Option<i64>,
}

const EVENT_COLS: &str =
    "e.account_id, e.calendar_id, e.event_id, e.ical_uid, e.status, e.summary, \
     e.description, e.location, e.start_ms, e.end_ms, e.all_day, e.start_date, e.end_date, \
     e.organizer_email, e.organizer_name, e.attendees, e.my_response, e.html_link, \
     e.conference_url, e.conference_kind, e.recurring_event_id, e.free, e.updated_ms";

/// Events only from calendars that are still selected.
const SELECTED_JOIN: &str = "JOIN calendars cal ON cal.account_id = e.account_id \
     AND cal.id = e.calendar_id AND cal.selected = 1";

/// Events in a search's "Calendar" group when mail results are shown too.
const MIXED_EVENT_LIMIT: usize = 8;
/// FTS candidates scored per event search.
const MAX_EVENT_CANDIDATES: usize = 400;

fn event_row(r: &Row) -> rusqlite::Result<CalendarEvent> {
    let attendees: String = r.get(15)?;
    let organizer_email: Option<String> = r.get(13)?;
    Ok(CalendarEvent {
        account_id: r.get(0)?,
        calendar_id: r.get(1)?,
        id: r.get(2)?,
        ical_uid: r.get(3)?,
        status: r.get(4)?,
        summary: r.get(5)?,
        description: r.get(6)?,
        location: r.get(7)?,
        start: r.get(8)?,
        end: r.get(9)?,
        all_day: r.get::<_, i64>(10)? != 0,
        start_date: r.get(11)?,
        end_date: r.get(12)?,
        organizer: organizer_email.map(|email| Address {
            email,
            name: r.get(14).ok().flatten(),
        }),
        attendees: serde_json::from_str(&attendees).unwrap_or_default(),
        my_response: r.get(16)?,
        html_link: r.get(17)?,
        conference_url: r.get(18)?,
        conference_kind: r.get(19)?,
        recurring_event_id: r.get(20)?,
        free: r.get::<_, i64>(21)? != 0,
        updated: r.get(22)?,
    })
}

fn calendar_row(r: &Row) -> rusqlite::Result<CalendarInfo> {
    Ok(CalendarInfo {
        account_id: r.get(0)?,
        id: r.get(1)?,
        summary: r.get(2)?,
        color: r.get(3)?,
        selected: r.get::<_, i64>(4)? != 0,
        primary: r.get::<_, i64>(5)? != 0,
        access_role: r.get(6)?,
    })
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// People on an event other than the account itself and rooms: (email,
/// declined).
fn event_people(e: &CalendarEvent) -> Vec<(String, bool)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for a in &e.attendees {
        if a.is_self || a.resource || a.email.is_empty() {
            continue;
        }
        let email = a.email.to_lowercase();
        if seen.insert(email.clone()) {
            out.push((email, a.response == "declined"));
        }
    }
    if let Some(o) = &e.organizer {
        let email = o.email.to_lowercase();
        let is_self = e.attendees.iter().any(|a| a.is_self && a.email == email);
        if !email.is_empty() && !is_self && seen.insert(email.clone()) {
            out.push((email, false));
        }
    }
    out
}

fn attendee_text(attendees: &[EventAttendee]) -> String {
    let mut s = String::new();
    for a in attendees {
        if let Some(n) = &a.name {
            s.push_str(n);
            s.push(' ');
        }
        s.push_str(&a.email);
        s.push('\n');
    }
    s
}

fn delete_event_rowid(tx: &Transaction, rowid: i64) -> Result<()> {
    tx.prepare_cached("DELETE FROM events_fts WHERE rowid = ?1")?
        .execute([rowid])?;
    tx.prepare_cached("DELETE FROM event_people WHERE event_rowid = ?1")?
        .execute([rowid])?;
    tx.prepare_cached("DELETE FROM events WHERE rowid = ?1")?
        .execute([rowid])?;
    Ok(())
}

fn upsert_event(tx: &Transaction, e: &CalendarEvent) -> Result<()> {
    let existing: Option<i64> = tx
        .prepare_cached(
            "SELECT rowid FROM events WHERE account_id = ?1 AND calendar_id = ?2 AND event_id = ?3",
        )?
        .query_row(params![e.account_id, e.calendar_id, e.id], |r| r.get(0))
        .optional()?;
    let attendees = serde_json::to_string(&e.attendees)?;
    let (org_email, org_name) = match &e.organizer {
        Some(o) => (Some(o.email.to_lowercase()), o.name.clone()),
        None => (None, None),
    };
    let values = params![
        e.account_id,
        e.calendar_id,
        e.id,
        e.ical_uid,
        e.status,
        e.summary,
        e.description,
        e.location,
        e.start,
        e.end,
        e.all_day as i64,
        e.start_date,
        e.end_date,
        org_email,
        org_name,
        attendees,
        e.my_response,
        e.html_link,
        e.conference_url,
        e.conference_kind,
        e.recurring_event_id,
        e.free as i64,
        e.updated,
    ];
    let rowid = match existing {
        Some(rowid) => {
            tx.prepare_cached("DELETE FROM events_fts WHERE rowid = ?1")?
                .execute([rowid])?;
            tx.prepare_cached("DELETE FROM event_people WHERE event_rowid = ?1")?
                .execute([rowid])?;
            tx.prepare_cached(
                "UPDATE events SET account_id = ?1, calendar_id = ?2, event_id = ?3, ical_uid = ?4,
                   status = ?5, summary = ?6, description = ?7, location = ?8, start_ms = ?9,
                   end_ms = ?10, all_day = ?11, start_date = ?12, end_date = ?13,
                   organizer_email = ?14, organizer_name = ?15, attendees = ?16, my_response = ?17,
                   html_link = ?18, conference_url = ?19, conference_kind = ?20,
                   recurring_event_id = ?21, free = ?22, updated_ms = ?23
                 WHERE rowid = ?24",
            )?
            .execute(rusqlite::params_from_iter(
                values.iter().copied().chain(std::iter::once(&rowid as _)),
            ))?;
            rowid
        }
        None => {
            tx.prepare_cached(
                "INSERT INTO events(account_id, calendar_id, event_id, ical_uid, status, summary,
                   description, location, start_ms, end_ms, all_day, start_date, end_date,
                   organizer_email, organizer_name, attendees, my_response, html_link,
                   conference_url, conference_kind, recurring_event_id, free, updated_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                   ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            )?
            .execute(values)?;
            tx.last_insert_rowid()
        }
    };
    let organizer = e
        .organizer
        .as_ref()
        .map(|o| format!("{} {}", o.name.as_deref().unwrap_or(""), o.email))
        .unwrap_or_default();
    tx.prepare_cached(
        "INSERT INTO events_fts(rowid, summary, description, location, organizer, attendees)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?
    .execute(params![
        rowid,
        e.summary,
        e.description,
        e.location,
        organizer,
        attendee_text(&e.attendees)
    ])?;
    let mut ins = tx.prepare_cached(
        "INSERT OR IGNORE INTO event_people(email, event_rowid, declined) VALUES (?1, ?2, ?3)",
    )?;
    for (email, declined) in event_people(e) {
        ins.execute(params![email, rowid, declined as i64])?;
    }
    Ok(())
}

/// Delete an event and, if it is a recurring series' parent, its instances.
fn delete_event(tx: &Transaction, account_id: &str, calendar_id: &str, id: &str) -> Result<usize> {
    let rowids: Vec<i64> = tx
        .prepare_cached(
            "SELECT rowid FROM events WHERE account_id = ?1 AND calendar_id = ?2
               AND (event_id = ?3 OR recurring_event_id = ?3)",
        )?
        .query_map(params![account_id, calendar_id, id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for &rowid in &rowids {
        delete_event_rowid(tx, rowid)?;
    }
    Ok(rowids.len())
}

fn delete_calendar_events(tx: &Transaction, account_id: &str, calendar_id: &str) -> Result<()> {
    let rowids: Vec<i64> = tx
        .prepare_cached("SELECT rowid FROM events WHERE account_id = ?1 AND calendar_id = ?2")?
        .query_map(params![account_id, calendar_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for rowid in rowids {
        delete_event_rowid(tx, rowid)?;
    }
    Ok(())
}

/// Every calendar row and event of an account (called from
/// `Store::remove_account`).
pub(super) fn remove_account(tx: &Transaction, account_id: &str) -> Result<()> {
    let cals: Vec<String> = tx
        .prepare("SELECT id FROM calendars WHERE account_id = ?1")?
        .query_map([account_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for cal in cals {
        delete_calendar_events(tx, account_id, &cal)?;
    }
    // Rows of calendars already gone from the list.
    let strays: Vec<i64> = tx
        .prepare("SELECT rowid FROM events WHERE account_id = ?1")?
        .query_map([account_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for rowid in strays {
        delete_event_rowid(tx, rowid)?;
    }
    tx.execute("DELETE FROM calendars WHERE account_id = ?1", [account_id])?;
    Ok(())
}

impl Store {
    // ----- calendars -----

    /// Replace the account's calendar list. New calendars start with the
    /// given `selected` (Google's own flag); known ones keep the local
    /// choice and their sync cursor. Calendars no longer listed are dropped
    /// with their events. `time_zones` is parallel to `calendars`.
    pub fn replace_calendars(
        &self,
        account_id: &str,
        calendars: &[CalendarInfo],
        time_zones: &[Option<String>],
    ) -> Result<()> {
        self.write(|tx| {
            let existing: Vec<String> = tx
                .prepare_cached("SELECT id FROM calendars WHERE account_id = ?1")?
                .query_map([account_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let keep: HashSet<&str> = calendars.iter().map(|c| c.id.as_str()).collect();
            for id in existing.iter().filter(|id| !keep.contains(id.as_str())) {
                delete_calendar_events(tx, account_id, id)?;
                tx.execute(
                    "DELETE FROM calendars WHERE account_id = ?1 AND id = ?2",
                    params![account_id, id],
                )?;
            }
            let mut up = tx.prepare_cached(
                "INSERT INTO calendars(account_id, id, summary, color, selected, is_primary, access_role, time_zone)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(account_id, id) DO UPDATE SET summary = excluded.summary,
                   color = excluded.color, is_primary = excluded.is_primary,
                   access_role = excluded.access_role, time_zone = excluded.time_zone",
            )?;
            for (i, c) in calendars.iter().enumerate() {
                up.execute(params![
                    account_id,
                    c.id,
                    c.summary,
                    c.color,
                    c.selected as i64,
                    c.primary as i64,
                    c.access_role,
                    time_zones.get(i).cloned().flatten(),
                ])?;
            }
            Ok(())
        })
    }

    /// Calendars of the given accounts (None = all), primary first.
    pub fn list_calendars(&self, accounts: Option<&[String]>) -> Result<Vec<CalendarInfo>> {
        self.read(|c| {
            let mut sql = String::from(
                "SELECT account_id, id, summary, color, selected, is_primary, access_role FROM calendars",
            );
            let mut vals: Vec<Value> = Vec::new();
            if let Some(ids) = accounts {
                sql.push_str(&format!(" WHERE account_id IN ({})", placeholders(ids.len())));
                vals.extend(ids.iter().map(|s| Value::Text(s.clone())));
            }
            sql.push_str(" ORDER BY account_id, is_primary DESC, summary COLLATE NOCASE");
            Ok(c.prepare(&sql)?
                .query_map(params_from_iter(vals), calendar_row)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Select or deselect a calendar. Deselecting drops its events and
    /// cursor (selecting again does a full sync). Returns whether it exists.
    pub fn set_calendar_selected(
        &self,
        account_id: &str,
        calendar_id: &str,
        selected: bool,
    ) -> Result<bool> {
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE calendars SET selected = ?3 WHERE account_id = ?1 AND id = ?2",
                params![account_id, calendar_id, selected as i64],
            )?;
            if n > 0 && !selected {
                delete_calendar_events(tx, account_id, calendar_id)?;
                tx.execute(
                    "UPDATE calendars SET sync_token = NULL, window_start = NULL, window_end = NULL,
                       synced_at = NULL WHERE account_id = ?1 AND id = ?2",
                    params![account_id, calendar_id],
                )?;
            }
            Ok(n > 0)
        })
    }

    pub fn calendar_cursor(&self, account_id: &str, calendar_id: &str) -> Result<CalendarCursor> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT sync_token, window_start, window_end, synced_at FROM calendars
                 WHERE account_id = ?1 AND id = ?2",
            )?
            .query_row(params![account_id, calendar_id], |r| {
                Ok(CalendarCursor {
                    sync_token: r.get(0)?,
                    window_start: r.get(1)?,
                    window_end: r.get(2)?,
                    synced_at: r.get(3)?,
                })
            })
            .optional()?
            .unwrap_or_default())
        })
    }

    /// A full sync's result: every event of the calendar is replaced.
    pub fn replace_calendar_events(
        &self,
        account_id: &str,
        calendar_id: &str,
        events: &[CalendarEvent],
        cursor: &CalendarCursor,
    ) -> Result<()> {
        self.write(|tx| {
            delete_calendar_events(tx, account_id, calendar_id)?;
            for e in events {
                upsert_event(tx, e)?;
            }
            set_cursor(tx, account_id, calendar_id, cursor)
        })
    }

    /// An incremental sync's result. Returns how many rows changed.
    pub fn apply_calendar_changes(
        &self,
        account_id: &str,
        calendar_id: &str,
        upserts: &[CalendarEvent],
        deletes: &[String],
        cursor: &CalendarCursor,
    ) -> Result<usize> {
        self.write(|tx| {
            let mut changed = 0;
            for id in deletes {
                changed += delete_event(tx, account_id, calendar_id, id)?;
            }
            for e in upserts {
                upsert_event(tx, e)?;
                changed += 1;
            }
            set_cursor(tx, account_id, calendar_id, cursor)?;
            Ok(changed)
        })
    }

    /// Drop events that ended before `before_ms` (the window slid past them).
    pub fn prune_events_before(&self, account_id: &str, before_ms: i64) -> Result<usize> {
        self.write(|tx| {
            let rowids: Vec<i64> = tx
                .prepare_cached("SELECT rowid FROM events WHERE account_id = ?1 AND end_ms < ?2")?
                .query_map(params![account_id, before_ms], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for &rowid in &rowids {
                delete_event_rowid(tx, rowid)?;
            }
            Ok(rowids.len())
        })
    }

    /// Replace one stored event (e.g. after an RSVP returned the new copy).
    pub fn upsert_calendar_event(&self, event: &CalendarEvent) -> Result<()> {
        self.write(|tx| upsert_event(tx, event))
    }

    pub fn calendar_counts(&self, account_id: &str) -> Result<CalendarCounts> {
        self.read(|c| {
            let events: i64 = c
                .prepare_cached(&format!(
                    "SELECT count(*) FROM events e {SELECTED_JOIN} WHERE e.account_id = ?1"
                ))?
                .query_row([account_id], |r| r.get(0))?;
            let synced_at: Option<i64> = c
                .prepare_cached(
                    "SELECT min(synced_at) FROM calendars WHERE account_id = ?1 AND selected = 1
                       AND access_role != 'freeBusyReader'",
                )?
                .query_row([account_id], |r| r.get(0))?;
            Ok(CalendarCounts {
                events: events as u64,
                synced_at,
            })
        })
    }

    // ----- reads -----

    /// Events overlapping [from, to), soonest first, from selected calendars
    /// of the given accounts (None = all).
    pub fn list_events(
        &self,
        from_ms: i64,
        to_ms: i64,
        accounts: Option<&[String]>,
        limit: usize,
    ) -> Result<Vec<CalendarEvent>> {
        self.read(|c| {
            // Events are short compared with the index span: bounding the
            // start from below by a generous lookback keeps this a range scan.
            let mut sql = format!(
                "SELECT {EVENT_COLS} FROM events e {SELECTED_JOIN}
                 WHERE e.start_ms < ? AND e.end_ms > ? AND e.start_ms > ?"
            );
            let mut vals = vec![
                Value::Integer(to_ms),
                Value::Integer(from_ms),
                Value::Integer(from_ms - LONGEST_EVENT_MS),
            ];
            push_accounts(&mut sql, &mut vals, accounts);
            sql.push_str(" ORDER BY e.all_day DESC, e.start_ms, e.end_ms LIMIT ?");
            vals.push(Value::Integer(limit as i64));
            Ok(c.prepare_cached(&sql)?
                .query_map(params_from_iter(vals), event_row)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn get_event(
        &self,
        account_id: &str,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<Option<CalendarEvent>> {
        self.read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {EVENT_COLS} FROM events e
                 WHERE e.account_id = ?1 AND e.calendar_id = ?2 AND e.event_id = ?3"
            ))?
            .query_row(params![account_id, calendar_id, event_id], event_row)
            .optional()?)
        })
    }

    /// Stored events with this iCalendar UID (an invite's `UID`), the
    /// account's own first, then by start. Recurring series share one UID;
    /// `near_ms` picks the instance closest to that time first.
    pub fn events_by_uid(
        &self,
        uid: &str,
        account_id: Option<&str>,
        near_ms: Option<i64>,
    ) -> Result<Vec<CalendarEvent>> {
        self.read(|c| {
            let mut rows: Vec<CalendarEvent> = c
                .prepare_cached(&format!(
                    "SELECT {EVENT_COLS} FROM events e {SELECTED_JOIN} WHERE e.ical_uid = ?1 LIMIT 500"
                ))?
                .query_map([uid], event_row)?
                .collect::<rusqlite::Result<_>>()?;
            rows.sort_by_key(|e| {
                (
                    account_id.is_some_and(|a| a != e.account_id),
                    near_ms.map_or(e.start, |n| (e.start - n).abs()),
                )
            });
            Ok(rows)
        })
    }

    /// Busy events overlapping [start, end) other than those in `except_uid`
    /// (the invite itself), across every account: declined, free and
    /// all-day events never conflict.
    pub fn event_conflicts(
        &self,
        start_ms: i64,
        end_ms: i64,
        except_uid: Option<&str>,
        accounts: Option<&[String]>,
    ) -> Result<Vec<CalendarEvent>> {
        let events = self.list_events(start_ms, end_ms, accounts, 50)?;
        let mut seen = HashSet::new();
        Ok(events
            .into_iter()
            .filter(|e| {
                !e.all_day
                    && !e.free
                    && e.my_response.as_deref() != Some("declined")
                    && (except_uid.is_none() || e.ical_uid.as_deref() != except_uid)
            })
            // The same meeting on two of your calendars is one conflict.
            .filter(|e| seen.insert(e.ical_uid.clone().unwrap_or_else(|| e.id.clone())))
            .collect())
    }

    /// Last and next event with `email` (who didn't decline) in selected
    /// calendars, relative to `now_ms`.
    pub fn person_meetings(&self, email: &str, now_ms: i64) -> Result<PersonMeetings> {
        let email = email.trim().to_lowercase();
        self.read(|c| {
            let pick = |order: &str, cmp: &str| -> Result<Option<CalendarEvent>> {
                Ok(c.prepare_cached(&format!(
                    "SELECT {EVENT_COLS} FROM event_people p
                     JOIN events e ON e.rowid = p.event_rowid {SELECTED_JOIN}
                     WHERE p.email = ?1 AND p.declined = 0 AND e.start_ms {cmp} ?2
                       AND (e.my_response IS NULL OR e.my_response != 'declined')
                     ORDER BY e.start_ms {order} LIMIT 1"
                ))?
                .query_row(params![email, now_ms], event_row)
                .optional()?)
            };
            let last = pick("DESC", "<")?;
            let next = pick("ASC", ">=")?;
            let count: i64 = c
                .prepare_cached(&format!(
                    "SELECT count(DISTINCT coalesce(e.ical_uid || e.start_ms, e.rowid))
                     FROM event_people p JOIN events e ON e.rowid = p.event_rowid {SELECTED_JOIN}
                     WHERE p.email = ?1 AND p.declined = 0"
                ))?
                .query_row([&email], |r| r.get(0))?;
            Ok(PersonMeetings {
                email: email.clone(),
                last,
                next,
                count: count as u32,
            })
        })
    }

    /// The newest thread in the account that carries an invitation for this
    /// event: a message with a calendar part whose subject names the event
    /// (Google: "Invitation: <title> @ <when>"). Local only.
    pub fn event_thread(&self, event: &CalendarEvent) -> Result<Option<(String, String)>> {
        let title = event.summary.trim();
        if title.is_empty() {
            return Ok(None);
        }
        // The subject phrase narrows through the FTS index first (newest
        // rowids first), then the exact substring and the calendar part are
        // checked per candidate through indexes.
        let words: Vec<String> = crate::text::tokens(title)
            .into_iter()
            .map(|t| t.2)
            .collect();
        if words.is_empty() {
            return Ok(None);
        }
        let phrase = format!("{{subject}} : \"{}\"", words.join(" "));
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT t.thread_id, m.subject FROM messages_fts
                 JOIN messages m ON m.rowid = messages_fts.rowid
                 JOIN threads t ON t.rowid = m.thread_rowid
                 WHERE messages_fts MATCH ?1 AND m.account_id = ?2
                   AND instr(lower(m.subject), lower(?3)) > 0
                   AND EXISTS (SELECT 1 FROM attachments a
                               WHERE a.message_rowid = m.rowid AND a.kind = 6)
                 ORDER BY messages_fts.rowid DESC LIMIT 1",
            )?
            .query_row(params![phrase, event.account_id, title], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?)
        })
    }
}

/// Longest event the range queries account for (a multi-week conference or
/// vacation block); longer ones only show from their first day.
const LONGEST_EVENT_MS: i64 = 62 * 24 * 3600 * 1000;

fn push_accounts(sql: &mut String, vals: &mut Vec<Value>, accounts: Option<&[String]>) {
    if let Some(ids) = accounts {
        sql.push_str(&format!(
            " AND e.account_id IN ({})",
            placeholders(ids.len())
        ));
        vals.extend(ids.iter().map(|s| Value::Text(s.clone())));
    }
}

fn set_cursor(
    tx: &Transaction,
    account_id: &str,
    calendar_id: &str,
    cursor: &CalendarCursor,
) -> Result<()> {
    tx.prepare_cached(
        "UPDATE calendars SET sync_token = ?3, window_start = ?4, window_end = ?5, synced_at = ?6
         WHERE account_id = ?1 AND id = ?2",
    )?
    .execute(params![
        account_id,
        calendar_id,
        cursor.sync_token,
        cursor.window_start,
        cursor.window_end,
        cursor.synced_at
    ])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// search

/// Does the query only make sense for mail (labels, folders, flags,
/// attachments)? Then the Calendar group stays empty.
fn mail_only(q: &ParsedQuery) -> bool {
    q.clauses.iter().flat_map(|c| &c.terms).any(|t| {
        !matches!(
            &t.atom,
            Atom::Text { .. } | Atom::Field { .. } | Atom::Near { .. } | Atom::In(Folder::Anywhere)
        )
    })
}

/// FTS5 expression for events: free text over every column, from: over the
/// organizer, to:/cc: over attendees, subject: over the title.
fn event_fts(q: &ParsedQuery, quote: &dyn Fn(&str, bool) -> String) -> Option<String> {
    let atom = |a: &Atom| -> Option<String> {
        match a {
            Atom::Text { text, prefix, .. } => Some(quote(text, *prefix)),
            Atom::Near { a, b, distance } => Some(format!("NEAR({} {}, {distance})", quote(a, false), quote(b, false))),
            Atom::Field {
                field,
                value,
                prefix,
            } => {
                let cols = match field {
                    Field::From => "organizer",
                    Field::To | Field::Cc | Field::Participant | Field::Domain => {
                        "organizer attendees"
                    }
                    Field::Subject => "summary",
                };
                Some(format!("{{{cols}}} : {}", quote(value, *prefix)))
            }
            _ => None,
        }
    };
    let mut pos = Vec::new();
    let mut neg = Vec::new();
    for clause in &q.clauses {
        let parts: Vec<(bool, String)> = clause
            .terms
            .iter()
            .filter_map(|t| atom(&t.atom).map(|s| (t.negated, s)))
            .collect();
        match parts.as_slice() {
            [] => {}
            [(true, s)] => neg.push(s.clone()),
            [(false, s)] => pos.push(s.clone()),
            many => {
                let ors: Vec<&str> = many
                    .iter()
                    .filter(|(n, _)| !n)
                    .map(|(_, s)| s.as_str())
                    .collect();
                if !ors.is_empty() {
                    pos.push(format!("({})", ors.join(" OR ")));
                }
            }
        }
    }
    if pos.is_empty() {
        return None;
    }
    let mut expr = pos.join(" AND ");
    for n in neg {
        expr = format!("({expr}) NOT {n}");
    }
    Some(expr)
}

fn event_key(e: &CalendarEvent) -> (String, String, String) {
    (e.account_id.clone(), e.calendar_id.clone(), e.id.clone())
}

/// The "Calendar" results group. `events_only` (type:event) lifts the small
/// mixed-results limit; with no text it lists upcoming events. Time filters
/// apply to the event's start.
pub(crate) fn search_events(
    c: &Connection,
    q: &ParsedQuery,
    accounts: Option<&[String]>,
    now_ms: i64,
    limit: usize,
    quote: &dyn Fn(&str, bool) -> String,
) -> Result<Vec<CalendarEvent>> {
    if mail_only(q) || q.empty_range() {
        return Ok(Vec::new());
    }
    let limit = if q.events_only {
        limit
    } else {
        MIXED_EVENT_LIMIT
    };
    let fts = event_fts(q, quote);
    if fts.is_none() && !q.events_only && q.after.is_none() && q.before.is_none() {
        // A bare `account:` or noise-only query: no calendar group.
        return Ok(Vec::new());
    }
    let mut sql = format!("SELECT {EVENT_COLS}");
    let mut vals: Vec<Value> = Vec::new();
    if let Some(expr) = &fts {
        sql.push_str(&format!(
            ", bm25(events_fts, 5.0, 1.0, 2.0, 2.0, 1.5) FROM events_fts
             JOIN events e ON e.rowid = events_fts.rowid {SELECTED_JOIN}
             WHERE events_fts MATCH ?"
        ));
        vals.push(Value::Text(expr.clone()));
    } else {
        sql.push_str(&format!(", 0.0 FROM events e {SELECTED_JOIN} WHERE 1"));
    }
    if let Some(a) = q.after {
        sql.push_str(" AND e.start_ms >= ?");
        vals.push(Value::Integer(a));
    }
    if let Some(b) = q.before {
        sql.push_str(" AND e.start_ms < ?");
        vals.push(Value::Integer(b));
    }
    let listing = fts.is_none() && q.after.is_none() && q.before.is_none();
    if listing {
        // type:event alone: what's coming up.
        sql.push_str(" AND e.end_ms > ?");
        vals.push(Value::Integer(now_ms));
    }
    push_accounts(&mut sql, &mut vals, accounts);
    let run = |order: &str, extra: Vec<Value>| -> Result<Vec<(CalendarEvent, f64)>> {
        let mut vals = vals.clone();
        vals.extend(extra);
        Ok(c.prepare(&format!("{sql} {order}"))?
            .query_map(params_from_iter(vals), |r| Ok((event_row(r)?, r.get(23)?)))?
            .collect::<rusqlite::Result<_>>()?)
    };
    let mut rows = if fts.is_some() {
        // Two bounded candidate pools, so a common word can't crowd out
        // either the best matches or the ones nearest today.
        let half = Value::Integer((MAX_EVENT_CANDIDATES / 2) as i64);
        let mut rows = run("ORDER BY 24 LIMIT ?", vec![half.clone()])?;
        let mut keys: HashSet<(String, String, String)> =
            rows.iter().map(|(e, _)| event_key(e)).collect();
        for row in run(
            "ORDER BY abs(e.start_ms - ?) LIMIT ?",
            vec![Value::Integer(now_ms), half],
        )? {
            if keys.insert(event_key(&row.0)) {
                rows.push(row);
            }
        }
        rows
    } else if listing {
        run(
            "ORDER BY e.start_ms LIMIT ?",
            vec![Value::Integer(limit as i64)],
        )?
    } else {
        run(
            "ORDER BY e.start_ms DESC LIMIT ?",
            vec![Value::Integer(limit as i64)],
        )?
    };
    if fts.is_some() {
        // bm25 is negative (lower = better); normalize against the best,
        // then favor events near today like mail favors recent messages.
        let best = rows.iter().map(|r| r.1).fold(0.0_f64, f64::min);
        let score = |e: &CalendarEvent, rank: f64| {
            let rel = if best < 0.0 { rank / best } else { 1.0 };
            let days = ((e.start - now_ms).abs() as f64) / 86_400_000.0;
            rel + 0.35 / (1.0 + days / 90.0)
        };
        rows.sort_by(|a, b| score(&b.0, b.1).total_cmp(&score(&a.0, a.1)));
    }
    // One entry per meeting even when it sits on several calendars.
    let mut seen = HashSet::new();
    Ok(rows
        .into_iter()
        .map(|r| r.0)
        .filter(|e| seen.insert((e.ical_uid.clone().unwrap_or_else(|| e.id.clone()), e.start)))
        .take(limit)
        .collect())
}

#[cfg(test)]
#[path = "store_calendar_tests.rs"]
mod tests;
