//! Reply Later and Follow up: the two triage views under Inbox.
//!
//! - **Reply Later** is a real label named [`REPLY_LATER_LABEL`] in each
//!   account (a Gmail user label, an IMAP folder, a Microsoft category), so
//!   it shows in the providers' own apps too. The app creates it on first use
//!   and applies it with an archive (`reply_later` in src-tauri).
//!   `MailboxView::ReplyLater` merges every account's label view, newest
//!   first, the same way `list_threads` merges a profile's accounts. Nothing
//!   is stored here for it.
//! - **Follow up** lists conversations waiting on a reply: the latest message
//!   that counts (not a draft, not trashed or spam) is one you sent, between
//!   [`FOLLOW_UP_LOOKBACK_DAYS`] and N days ago. This is `is:awaiting`
//!   (search.rs), narrowed to what's worth a nudge: not only to your own or
//!   automated addresses ([`is_automated_address`]), not a calendar reply,
//!   not a reply into a thread of nothing but bulk mail, not snoozed, and not
//!   dismissed. A dismissal (`follow_up_dismissals`) remembers the date of
//!   the message it dismissed, so it lasts until something newer is sent in
//!   the thread. Oldest wait first, one page.

use std::collections::{HashMap, HashSet};

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Deserialize;

use super::{
    summaries_by_rowid, summary_row, Store, F_DRAFT, F_NEWSLETTER, F_SENT, F_SPAM, F_TRASH,
    KIND_CALENDAR, ROWID_SLOTS, ROWID_SPILL_LIMIT,
};
use crate::types::{
    AccountId, Address, ListQuery, ThreadSummary, TriageCount, FOLLOW_UP_DAYS_MAX,
    FOLLOW_UP_DAYS_MIN, FOLLOW_UP_LOOKBACK_DAYS, REPLY_LATER_LABEL,
};
use crate::Result;

/// Follow-up dismissals: `sent_at` is the date of the message that was
/// being waited on when it was dismissed.
pub(super) const SCHEMA_FOLLOW_UP: &str = r#"
CREATE TABLE follow_up_dismissals(
    account_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    sent_at INTEGER NOT NULL,
    PRIMARY KEY(account_id, thread_id)
) WITHOUT ROWID;
"#;

const DAY: i64 = 86_400_000;

/// Messages that never count as the latest in a thread.
const NOT_LIVE: i64 = F_DRAFT | F_TRASH | F_SPAM;

/// The columns `summary_row` reads, minus the trailing date.
const COLS: &str = "t.account_id, t.thread_id, t.subject, t.snippet, t.participants, t.message_count, t.flags, t.labels, t.otp,
    (SELECT s.wake_at FROM snoozes s WHERE s.account_id = t.account_id AND s.thread_id = t.thread_id AND s.woke_at IS NULL)";

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Addresses that are machines, not people: waiting on them for a reply is
/// pointless (no-reply senders, notification and list robots, bounce
/// handlers, Google Calendar's addresses).
pub fn is_automated_address(email: &str) -> bool {
    let email = email.trim().to_ascii_lowercase();
    let (local, domain) = email.rsplit_once('@').unwrap_or((email.as_str(), ""));
    if domain == "calendar.google.com" || domain.ends_with(".calendar.google.com") {
        return true;
    }
    let squashed: String = local
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if squashed.contains("noreply") || squashed.contains("donotreply") {
        return true;
    }
    const EXACT_OR_PREFIX: &[&str] = &[
        "notification",
        "notifications",
        "notify",
        "mailer-daemon",
        "postmaster",
        "bounce",
        "bounces",
        "newsletter",
        "newsletters",
        "news",
        "updates",
        "digest",
        "alert",
        "alerts",
        "automated",
        "auto-confirm",
        "calendar-notification",
        "invitations",
        "unsubscribe",
    ];
    EXACT_OR_PREFIX.iter().any(|p| {
        local == *p
            || local
                .strip_prefix(p)
                .is_some_and(|rest| rest.starts_with(['-', '+', '.', '_']))
    })
}

impl Store {
    /// The Reply Later label's id in one account (None: not created yet).
    pub fn reply_later_label(&self, account_id: &str) -> Result<Option<String>> {
        self.read(|c| {
            Ok(reply_later_labels(c, Some(&[account_id.to_string()]))?
                .into_iter()
                .next()
                .map(|(_, id)| id))
        })
    }

    /// `MailboxView::ReplyLater`: each account's label view (an index
    /// range scan, like a profile's inbox), merged newest first. Pages with
    /// `before` like any label.
    pub(super) fn list_reply_later(
        &self,
        scope: Option<&[AccountId]>,
        query: &ListQuery,
    ) -> Result<Vec<ThreadSummary>> {
        let limit = if query.limit == 0 {
            50
        } else {
            query.limit.min(1000)
        };
        let before = query.before.unwrap_or(i64::MAX);
        let unread = if query.unread_only {
            " AND unread = 1"
        } else {
            ""
        };
        self.read(|c| {
            let labels = reply_later_labels(c, scope)?;
            if labels.is_empty() {
                return Ok(Vec::new());
            }
            let mut args: Vec<Value> = vec![Value::Integer(before), Value::Integer(limit.into())];
            let mut parts = Vec::with_capacity(labels.len());
            for (i, (account_id, label_id)) in labels.into_iter().enumerate() {
                parts.push(format!(
                    "SELECT * FROM (SELECT thread_rowid, last_date FROM thread_views INDEXED BY thread_views_acct
                     WHERE account_id = ?{} AND view = ?{} AND last_date < ?1{unread} ORDER BY last_date DESC LIMIT ?2)",
                    2 * i + 3,
                    2 * i + 4
                ));
                args.push(Value::Text(account_id));
                args.push(Value::Text(label_id));
            }
            let sql = format!(
                "WITH top(thread_rowid, last_date) AS ({})
                 SELECT {COLS}, top.last_date FROM top JOIN threads t ON t.rowid = top.thread_rowid
                 ORDER BY top.last_date DESC LIMIT ?2",
                parts.join(" UNION ALL ")
            );
            let mut stmt = c.prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(&args), summary_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// `MailboxView::FollowUp` with a wait of `days` (clamped to 1–14):
    /// see the module docs. `last_date` is when the awaited message was
    /// sent; oldest first.
    pub fn list_follow_ups(
        &self,
        scope: Option<&[AccountId]>,
        days: u32,
        now: i64,
        unread_only: bool,
    ) -> Result<Vec<ThreadSummary>> {
        if scope.is_some_and(|s| s.is_empty()) {
            return Ok(Vec::new());
        }
        self.read(|c| {
            let found = follow_ups(c, scope, days, now)?;
            let rowids: Vec<i64> = found.iter().map(|f| f.thread_rowid).collect();
            let mut summaries = summaries_by_rowid(c, &rowids)?;
            let mut out = Vec::with_capacity(found.len());
            for f in found {
                let Some(mut s) = summaries.remove(&f.thread_rowid) else {
                    continue;
                };
                if unread_only && !s.unread {
                    continue;
                }
                s.last_date = f.sent_at;
                out.push(s);
            }
            Ok(out)
        })
    }

    /// Sidebar counts: every account in scope with the size of its Reply
    /// Later and Follow up views.
    pub fn triage_counts(
        &self,
        scope: Option<&[AccountId]>,
        days: u32,
        now: i64,
    ) -> Result<Vec<TriageCount>> {
        self.read(|c| {
            let accounts = scoped_accounts(c, scope)?;
            let mut counts: Vec<TriageCount> = accounts
                .iter()
                .map(|a| TriageCount {
                    account_id: a.clone(),
                    reply_later: 0,
                    follow_up: 0,
                })
                .collect();
            let index: HashMap<String, usize> = accounts
                .into_iter()
                .enumerate()
                .map(|(i, a)| (a, i))
                .collect();
            let mut count_view = c.prepare_cached(
                "SELECT count(*) FROM thread_views INDEXED BY thread_views_acct WHERE account_id = ?1 AND view = ?2",
            )?;
            for (account_id, label_id) in reply_later_labels(c, scope)? {
                if let Some(&i) = index.get(&account_id) {
                    let n: i64 = count_view.query_row(params![account_id, label_id], |r| r.get(0))?;
                    counts[i].reply_later = n as u32;
                }
            }
            for f in follow_ups(c, scope, days, now)? {
                if let Some(&i) = index.get(&f.account_id) {
                    counts[i].follow_up += 1;
                }
            }
            Ok(counts)
        })
    }

    /// Hide (`dismissed`) or show again conversations in Follow up. A
    /// dismissal holds until something newer is sent in the thread; a
    /// thread with nothing sent is left alone.
    pub fn dismiss_follow_ups(&self, threads: &[(String, String)], dismissed: bool) -> Result<()> {
        self.write(|tx| {
            for (account_id, thread_id) in threads {
                if !dismissed {
                    tx.prepare_cached(
                        "DELETE FROM follow_up_dismissals WHERE account_id = ?1 AND thread_id = ?2",
                    )?
                    .execute(params![account_id, thread_id])?;
                    continue;
                }
                let sent: Option<i64> = tx
                    .prepare_cached(
                        "SELECT max(m.date) FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
                         WHERE t.account_id = ?1 AND t.thread_id = ?2 AND m.flags & ?3 = ?4",
                    )?
                    .query_row(
                        params![account_id, thread_id, F_SENT | NOT_LIVE, F_SENT],
                        |r| r.get(0),
                    )?;
                if let Some(sent) = sent {
                    tx.prepare_cached(
                        "INSERT OR REPLACE INTO follow_up_dismissals(account_id, thread_id, sent_at) VALUES (?1, ?2, ?3)",
                    )?
                    .execute(params![account_id, thread_id, sent])?;
                }
            }
            Ok(())
        })
    }
}

pub(super) fn remove_account(c: &Connection, account_id: &str) -> Result<()> {
    c.execute(
        "DELETE FROM follow_up_dismissals WHERE account_id = ?1",
        [account_id],
    )?;
    Ok(())
}

/// (account, label id) for every account in scope that has a Reply Later
/// label, in account order.
fn reply_later_labels(
    c: &Connection,
    scope: Option<&[AccountId]>,
) -> Result<Vec<(String, String)>> {
    let rows: Vec<(String, String)> = c
        .prepare_cached(
            "SELECT account_id, min(id) FROM labels WHERE kind = 'user' AND name = ?1 COLLATE NOCASE
             GROUP BY account_id ORDER BY account_id",
        )?
        .query_map([REPLY_LATER_LABEL], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(match scope {
        None => rows,
        Some(s) => {
            let mut out: Vec<(String, String)> = Vec::new();
            for a in s {
                if let Some(r) = rows.iter().find(|(acct, _)| acct == a) {
                    if !out.iter().any(|(x, _)| x == a) {
                        out.push(r.clone());
                    }
                }
            }
            out
        }
    })
}

fn scoped_accounts(c: &Connection, scope: Option<&[AccountId]>) -> Result<Vec<String>> {
    Ok(match scope {
        Some(s) => {
            let mut out: Vec<String> = Vec::with_capacity(s.len());
            for a in s {
                if !out.contains(a) {
                    out.push(a.clone());
                }
            }
            out
        }
        None => c
            .prepare_cached("SELECT id FROM accounts ORDER BY added_at, id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?,
    })
}

struct FollowUp {
    account_id: String,
    thread_rowid: i64,
    sent_at: i64,
}

#[derive(Default, Deserialize)]
struct Recipients {
    #[serde(default)]
    to: Vec<Address>,
    #[serde(default)]
    cc: Vec<Address>,
    #[serde(default)]
    bcc: Vec<Address>,
}

/// The Follow up list (module docs), oldest wait first.
///
/// One statement per account reads every candidate thread (anything you
/// sent in the window) together with its messages, so a thread is judged
/// from one pass instead of a query per test. Most candidates got a reply,
/// and the thread row proves it without reading messages.
/// Dismissals, snoozes and invitation messages are small sets, read once.
fn follow_ups(
    c: &Connection,
    scope: Option<&[AccountId]>,
    days: u32,
    now: i64,
) -> Result<Vec<FollowUp>> {
    let days = days.clamp(FOLLOW_UP_DAYS_MIN, FOLLOW_UP_DAYS_MAX) as i64;
    let hi = now - days * DAY;
    let lo = now - FOLLOW_UP_LOOKBACK_DAYS as i64 * DAY;
    let own: HashSet<String> = c
        .prepare_cached("SELECT lower(email) FROM accounts")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let dismissed: HashMap<(String, String), i64> = c
        .prepare_cached("SELECT account_id, thread_id, sent_at FROM follow_up_dismissals")?
        .query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let snoozed: HashSet<(String, String)> = c
        .prepare_cached("SELECT account_id, thread_id FROM snoozes WHERE woke_at IS NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    // Messages with an invitation or RSVP from the window on, from the
    // `attachments_calendar` partial index (its WHERE must match).
    let invites: HashSet<i64> = c
        .prepare_cached(&format!(
            "SELECT message_rowid FROM attachments WHERE kind = {KIND_CALENDAR} AND message_rowid >= ?1"
        ))?
        .query_map([lo.saturating_mul(ROWID_SLOTS)], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    // Candidates: threads with something you sent in the window (SENT's
    // view date is the newest sent message), each with its messages. The
    // ORDER BY is the index's own order, so a thread's rows are adjacent
    // and nothing is sorted.
    //
    // Skipped from the thread row alone, as replied to: its newest live
    // message (`t.last_date`) is newer than the newest you sent
    // (`v.last_date`). Only without a draft, as `t.last_date` counts
    // drafts, and only past a gap of ROWID_SPILL_LIMIT, as "latest" is by
    // rowid and a rowid can sit up to that many milliseconds past its date
    // (when a millisecond's slots are full).
    let mut scan = c.prepare_cached(&format!(
        "SELECT v.thread_rowid, t.thread_id, m.rowid, m.flags, m.date
         FROM thread_views v INDEXED BY thread_views_acct
         JOIN threads t ON t.rowid = v.thread_rowid
         JOIN messages m ON m.thread_rowid = v.thread_rowid
         WHERE v.account_id = ?1 AND v.view = 'SENT' AND v.last_date >= ?2 AND v.last_date < ?3
           AND NOT (t.flags & {F_DRAFT} = 0 AND t.last_date > v.last_date + {ROWID_SPILL_LIMIT})
         ORDER BY v.last_date, v.thread_rowid"
    ))?;
    let mut extra = c.prepare_cached("SELECT extra FROM message_bodies WHERE rowid = ?1")?;

    let mut out = Vec::new();
    for account_id in scoped_accounts(c, scope)? {
        let mut threads: Vec<Candidate> = Vec::new();
        let mut rows = scan.query(params![account_id, lo, hi])?;
        while let Some(r) = rows.next()? {
            let trow: i64 = r.get(0)?;
            if threads.last().is_none_or(|t| t.rowid != trow) {
                threads.push(Candidate {
                    rowid: trow,
                    thread_id: r.get(1)?,
                    latest: None,
                    received: 0,
                    bulk: 0,
                });
            }
            let t = threads.last_mut().expect("pushed above");
            t.add(r.get(2)?, r.get(3)?, r.get(4)?);
        }
        for t in threads {
            // is:awaiting: the latest message that counts is yours.
            let Some((msg, flags, sent_at)) = t.latest else {
                continue;
            };
            if flags & F_SENT == 0 || sent_at >= hi || sent_at < lo {
                continue;
            }
            let key = (account_id.clone(), t.thread_id);
            if dismissed.get(&key).is_some_and(|&at| at >= sent_at) || snoozed.contains(&key) {
                continue;
            }
            // A reply into a thread of nothing but bulk mail.
            if t.received > 0 && t.bulk == t.received {
                continue;
            }
            // An RSVP isn't waiting on anyone.
            if invites.contains(&msg) {
                continue;
            }
            // Only to yourself, or only to machines. Unknown recipients
            // (nothing stored) don't rule it out.
            let r: Recipients = extra
                .query_row([msg], |r| r.get::<_, String>(0))
                .optional()?
                .and_then(|e| serde_json::from_str(&e).ok())
                .unwrap_or_default();
            let all: Vec<String> =
                r.to.iter()
                    .chain(&r.cc)
                    .chain(&r.bcc)
                    .map(|a| a.email.trim().to_ascii_lowercase())
                    .filter(|e| !e.is_empty())
                    .collect();
            if !all.is_empty()
                && all
                    .iter()
                    .filter(|e| !own.contains(*e))
                    .all(|e| is_automated_address(e))
            {
                continue;
            }
            out.push(FollowUp {
                account_id: account_id.clone(),
                thread_rowid: t.rowid,
                sent_at,
            });
        }
    }
    out.sort_by_key(|f| (f.sent_at, f.thread_rowid));
    Ok(out)
}

/// A Follow up candidate, built from its messages.
struct Candidate {
    rowid: i64,
    thread_id: String,
    /// The newest message that counts (by rowid): (rowid, flags, date).
    latest: Option<(i64, i64, i64)>,
    /// Live messages you didn't send, and how many of them are bulk.
    received: u32,
    bulk: u32,
}

impl Candidate {
    fn add(&mut self, rowid: i64, flags: i64, date: i64) {
        if flags & NOT_LIVE != 0 {
            return;
        }
        if flags & F_SENT == 0 {
            self.received += 1;
            self.bulk += u32::from(flags & F_NEWSLETTER != 0);
        }
        if self.latest.is_none_or(|(l, _, _)| rowid > l) {
            self.latest = Some((rowid, flags, date));
        }
    }
}

#[cfg(test)]
#[path = "store_triage_tests.rs"]
mod tests;
