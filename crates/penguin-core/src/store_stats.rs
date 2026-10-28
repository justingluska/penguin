//! Settings → Stats: a year of sending and receiving, counted from the local
//! store. One scan of the year's message headers (a rowid range, since rowid
//! is date-ordered) feeds the day grid, the weekday × hour grid, streaks,
//! reply times and top senders; a second read parses the recipients of the
//! year's sent mail. Nothing here touches the network.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike};
use rusqlite::params_from_iter;

use super::{account_in, is_automated_address, Extra, Store, ROWID_SLOTS};
use super::{F_DRAFT, F_NEWSLETTER, F_SENT, F_SPAM, F_TRASH};
use crate::types::{AccountId, Address, MailStats, StatsDay, StatsPerson};
use crate::Result;

/// Days in the grid, today included.
pub const STATS_DAYS: usize = 365;
/// People in each top list.
const TOP_PEOPLE: usize = 8;
const DAY_MS: i64 = 86_400_000;

/// One message header from the year's scan.
struct Row {
    thread: i64,
    date: i64,
    flags: i64,
    from_name: Option<String>,
    from_email: String,
}

impl Store {
    /// The year ending today (local time) for `accounts` (None: every
    /// account). `local_offset_ms(t)` is the local time zone's offset from
    /// UTC at unix ms `t`, so days and hours follow daylight saving.
    pub fn mail_stats(
        &self,
        accounts: Option<&[AccountId]>,
        now_ms: i64,
        local_offset_ms: impl Fn(i64) -> i64,
    ) -> Result<MailStats> {
        let local = |t: i64| local_datetime(t + local_offset_ms(t));
        let today = local(now_ms).date();
        let first_day = today - Duration::days(STATS_DAYS as i64 - 1);
        // A day of slack on each side of the UTC range; exact days are
        // decided per message in local time below.
        let since_ms = now_ms - (STATS_DAYS as i64 + 1) * DAY_MS;
        let (acct_sql, acct_vals) = account_in("account_id", accounts);

        let (rows, own, oldest) = self.read(|c| {
            let sql = format!(
                "SELECT thread_rowid, date, flags, from_name, from_email FROM messages \
                 WHERE rowid >= ?{acct_sql}"
            );
            let mut stmt = c.prepare(&sql)?;
            let mut params = vec![rusqlite::types::Value::Integer(since_ms * ROWID_SLOTS)];
            params.extend(acct_vals.iter().cloned());
            let rows = stmt
                .query_map(params_from_iter(params.iter()), |r| {
                    Ok(Row {
                        thread: r.get(0)?,
                        date: r.get(1)?,
                        flags: r.get(2)?,
                        from_name: r.get(3)?,
                        from_email: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let own: HashSet<String> = c
                .prepare("SELECT lower(email) FROM accounts")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            let oldest_sql = format!("SELECT min(rowid) FROM messages WHERE 1{acct_sql}");
            let oldest: Option<i64> = c.query_row(&oldest_sql, params_from_iter(acct_vals.iter()), |r| r.get(0))?;
            Ok((rows, own, oldest.map(|r| r / ROWID_SLOTS)))
        })?;

        let mut days: Vec<StatsDay> = (0..STATS_DAYS)
            .map(|i| StatsDay {
                date: (first_day + Duration::days(i as i64)).format("%Y-%m-%d").to_string(),
                sent: 0,
                received: 0,
            })
            .collect();
        let mut by_hour = vec![vec![0u32; 24]; 7];
        let mut out = MailStats::default();
        let mut senders: HashMap<String, (Option<String>, u32)> = HashMap::new();
        // Per conversation, in date order: (date, sent) for reply times.
        let mut threads: HashMap<i64, Vec<(i64, bool)>> = HashMap::new();
        let mut sent_ids: Vec<i64> = Vec::new();

        for r in &rows {
            if r.flags & (F_DRAFT | F_SPAM | F_TRASH) != 0 {
                continue;
            }
            let at = local(r.date);
            let Some(slot) = day_index(first_day, at.date()) else { continue };
            let sent = r.flags & F_SENT != 0;
            if sent {
                days[slot].sent += 1;
                out.sent += 1;
                by_hour[at.weekday().num_days_from_monday() as usize][at.hour() as usize] += 1;
                sent_ids.push(r.date);
            } else {
                days[slot].received += 1;
                out.received += 1;
                let email = r.from_email.trim().to_lowercase();
                if r.flags & F_NEWSLETTER != 0 {
                    out.received_newsletters += 1;
                } else if !own.contains(&email) && !is_automated_address(&email) {
                    let e = senders.entry(email).or_insert((None, 0));
                    e.1 += 1;
                    if e.0.is_none() {
                        e.0 = r.from_name.clone().filter(|n| !n.trim().is_empty());
                    }
                }
            }
            threads.entry(r.thread).or_default().push((r.date, sent));
        }

        let (current, longest) = streaks(&days);
        out.active_days = days.iter().filter(|d| d.sent > 0).count() as u32;
        out.current_streak = current;
        out.longest_streak = longest;

        let mut waits = reply_waits(threads.into_values());
        out.replies = waits.len() as u32;
        out.replies_within_hour = waits.iter().filter(|&&w| w <= 60 * 60_000).count() as u32;
        out.median_reply_minutes = median(&mut waits).map(|ms| (ms / 60_000) as u32);

        out.top_senders = top(senders);
        out.top_recipients = top(self.recipients_since(since_ms, &acct_sql, &acct_vals, &own, first_day, &local)?);
        out.days = days;
        out.sent_by_hour = by_hour;
        out.oldest_message = oldest;
        Ok(out)
    }

    /// Who the year's sent mail went to, from the headers stored with each body.
    fn recipients_since(
        &self,
        since_ms: i64,
        acct_sql: &str,
        acct_vals: &[rusqlite::types::Value],
        own: &HashSet<String>,
        first_day: NaiveDate,
        local: &impl Fn(i64) -> chrono::NaiveDateTime,
    ) -> Result<HashMap<String, (Option<String>, u32)>> {
        let sql = format!(
            "SELECT m.date, b.extra FROM messages m JOIN message_bodies b ON b.rowid = m.rowid \
             WHERE m.rowid >= ? AND m.flags & {F_SENT} != 0 AND m.flags & {} = 0{}",
            F_DRAFT | F_SPAM | F_TRASH,
            acct_sql.replace("account_id", "m.account_id"),
        );
        self.read(|c| {
            let mut stmt = c.prepare(&sql)?;
            let mut params = vec![rusqlite::types::Value::Integer(since_ms * ROWID_SLOTS)];
            params.extend(acct_vals.iter().cloned());
            let mut rows = stmt.query(params_from_iter(params.iter()))?;
            let mut out: HashMap<String, (Option<String>, u32)> = HashMap::new();
            while let Some(r) = rows.next()? {
                if day_index(first_day, local(r.get(0)?).date()).is_none() {
                    continue;
                }
                let extra: Extra = serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default();
                let mut seen = HashSet::new();
                for a in extra.to.iter().chain(&extra.cc).chain(&extra.bcc) {
                    let email = a.email.trim().to_lowercase();
                    if email.is_empty() || own.contains(&email) || !seen.insert(email.clone()) {
                        continue;
                    }
                    let e = out.entry(email).or_insert((None, 0));
                    e.1 += 1;
                    if e.0.is_none() {
                        e.0 = name_of(a);
                    }
                }
            }
            Ok(out)
        })
    }
}

fn name_of(a: &Address) -> Option<String> {
    a.name.clone().filter(|n| !n.trim().is_empty())
}

fn local_datetime(ms: i64) -> chrono::NaiveDateTime {
    DateTime::from_timestamp_millis(ms).unwrap_or_default().naive_utc()
}

fn day_index(first_day: NaiveDate, day: NaiveDate) -> Option<usize> {
    let i = (day - first_day).num_days();
    (0..STATS_DAYS as i64).contains(&i).then_some(i as usize)
}

/// (current, longest) runs of days with a send. Today without a send yet
/// doesn't break the current run.
fn streaks(days: &[StatsDay]) -> (u32, u32) {
    let mut longest = 0;
    let mut run = 0;
    for d in days {
        run = if d.sent > 0 { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let skip_today = days.last().is_some_and(|d| d.sent == 0);
    let current = days
        .iter()
        .rev()
        .skip(usize::from(skip_today))
        .take_while(|d| d.sent > 0)
        .count() as u32;
    (current, longest)
}

/// For each reply: ms from the first message received since your previous
/// send in that conversation to your send.
fn reply_waits(threads: impl Iterator<Item = Vec<(i64, bool)>>) -> Vec<i64> {
    let mut waits = Vec::new();
    for mut msgs in threads {
        msgs.sort_unstable();
        let mut waiting_since = None;
        for (date, sent) in msgs {
            if sent {
                if let Some(t) = waiting_since.take() {
                    waits.push(date - t);
                }
            } else if waiting_since.is_none() {
                waiting_since = Some(date);
            }
        }
    }
    waits
}

fn median(v: &mut [i64]) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

fn top(people: HashMap<String, (Option<String>, u32)>) -> Vec<StatsPerson> {
    let mut v: Vec<StatsPerson> = people
        .into_iter()
        .map(|(email, (name, count))| StatsPerson { email, name, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.email.cmp(&b.email)));
    v.truncate(TOP_PEOPLE);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;

    const ME: &str = "ada@x.example";
    const NOW: i64 = 1_790_000_000_000; // a fixed "now"

    fn msg(id: &str, thread: &str, from: &str, to: &str, date: i64, labels: &[&str]) -> Message {
        Message {
            account_id: ME.into(),
            id: id.into(),
            thread_id: thread.into(),
            date,
            from: Address { name: None, email: from.into() },
            to: vec![Address { name: Some("Bea".into()), email: to.into() }],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "s".into(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: None,
            label_ids: labels.iter().map(|s| s.to_string()).collect(),
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: true,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    fn store(msgs: &[Message]) -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&crate::types::Account { id: ME.into(), email: ME.into(), color: "#000".into(), ..Default::default() })
            .unwrap();
        s.upsert_messages(msgs).unwrap();
        s
    }

    #[test]
    fn counts_days_streaks_replies_and_people() {
        let h = 3_600_000;
        let s = store(&[
            msg("r1", "t1", "bea@y.example", ME, NOW - 2 * DAY_MS, &["INBOX"]),
            msg("s1", "t1", ME, "bea@y.example", NOW - 2 * DAY_MS + h / 2, &["SENT"]),
            msg("s2", "t2", ME, "bea@y.example", NOW - DAY_MS, &["SENT"]),
            msg("r2", "t3", "news@shop.example", ME, NOW - h, &["INBOX", "CATEGORY_PROMOTIONS"]),
            msg("old", "t4", ME, "cy@y.example", NOW - 400 * DAY_MS, &["SENT"]),
            msg("spam", "t5", "x@spam.example", ME, NOW - h, &["SPAM"]),
        ]);
        let st = s.mail_stats(None, NOW, |_| 0).unwrap();
        assert_eq!(st.days.len(), STATS_DAYS);
        assert_eq!((st.sent, st.received, st.received_newsletters), (2, 2, 1));
        assert_eq!((st.current_streak, st.longest_streak, st.active_days), (2, 2, 2));
        assert_eq!((st.replies, st.replies_within_hour, st.median_reply_minutes), (1, 1, Some(30)));
        assert_eq!(st.top_recipients.len(), 1);
        assert_eq!((st.top_recipients[0].email.as_str(), st.top_recipients[0].count), ("bea@y.example", 2));
        assert_eq!(st.top_senders.iter().map(|p| p.email.as_str()).collect::<Vec<_>>(), ["bea@y.example"]);
        assert_eq!(st.sent_by_hour.iter().flatten().sum::<u32>(), 2);
        assert_eq!(st.oldest_message, Some(NOW - 400 * DAY_MS));
    }

    #[test]
    fn local_offset_moves_messages_across_midnight() {
        // 00:30 UTC on the last day is still the day before at UTC−5.
        let midnight = NOW - NOW.rem_euclid(DAY_MS);
        let s = store(&[msg("s", "t", ME, "bea@y.example", midnight + 30 * 60_000, &["SENT"])]);
        let now = midnight + 12 * 3_600_000;
        let utc = s.mail_stats(None, now, |_| 0).unwrap();
        let ny = s.mail_stats(None, now, |_| -5 * 3_600_000).unwrap();
        assert_eq!(utc.days.last().unwrap().sent, 1);
        assert_eq!(ny.days[STATS_DAYS - 2].sent, 1);
    }

    #[test]
    fn streak_survives_a_quiet_today() {
        let mut days = vec![StatsDay::default(); 5];
        for d in &mut days[1..4] {
            d.sent = 1;
        }
        assert_eq!(streaks(&days), (3, 3));
    }
}
