//! Smart views: optional sidebar lists built from what is already indexed.
//! Nothing here touches the network or stores anything new; every view is a
//! read over existing tables, and each row is an ordinary [`ThreadSummary`]
//! (so the thread list, j/k, archive and swipes work as anywhere) with a
//! [`SmartRow`] saying what the row is about.
//!
//! - **From extracted facts** (`extracted`, see store_extracted.rs and
//!   docs/ASK.md): Receipts (orders and receipts), Travel (flights, stays,
//!   rental cars, trains, buses), Packages (shipments), Bills (invoices and
//!   bills; paid the way Ask decides it), Reservations (restaurants, events,
//!   tickets) and Subscriptions (recurring receipts, detected here).
//! - **From other indexes**: Invites (`invites`, answerable and not
//!   answered), Codes (`messages.otp`, last 48 h), Files (the attachment
//!   flag bits, over a partial index), Newsletters (the inbox's
//!   newsletters view), People you know (inbox mail from someone you've
//!   written to: `first_contacts`).
//! - **Saved searches** pinned as views (`MailboxView::Query`) run the
//!   search language through `Store::match_query`.
//!
//! Dates in facts are local wall times as written in the mail; "today" is
//! the local day at `now` with `utc_offset_secs`. Facts from mail in Trash
//! or Spam are left out. Views over facts are one page (`before` returns
//! nothing); Newsletters and People you know page like the inbox.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use super::{
    summaries_by_rowid, summary_row, Store, F_ATTACH, F_DOC, F_DRAFT, F_IMAGE, F_NEWSLETTER, F_PDF,
    F_PRES, F_SENT, F_SHEET, F_SPAM, F_TRASH, F_UNREAD,
};
use crate::structured::{airport_by_code, clean_org, Extracted, Money};
use crate::types::{
    AccountId, InboxTab, ListQuery, MailboxView, Otp, SmartCount, SmartRow, SmartStat,
    SmartViewInfo, ThreadSummary, SMART_FILE_KINDS, SMART_VIEWS,
};
use crate::Result;

/// Files walks messages with attachments newest first: a partial index
/// keeps that proportional to the mail that has files, not the mailbox.
pub(super) const SCHEMA_SMART_VIEWS: &str = r#"
CREATE INDEX messages_attach ON messages(date) WHERE flags & 512 != 0;
"#;

/// The same index, covering what Files reads (type bits, thread, account),
/// so its newest-first walk never reads the message rows.
pub(super) const SCHEMA_FILES_COVERING: &str = r#"
DROP INDEX messages_attach;
CREATE INDEX messages_attach ON messages(date, flags, thread_rowid, account_id) WHERE flags & 512 != 0;
"#;

const DAY: i64 = 86_400_000;
/// Mail that never shows in a smart view.
const HIDDEN: i64 = F_TRASH | F_SPAM | F_DRAFT;
/// Facts read per kind (newest first); far more than a year of any kind.
const MAX_FACTS: i64 = 5000;
/// Rows in a one-page view.
const MAX_ROWS: usize = 500;
/// How far back each view looks.
const RECEIPTS_DAYS: i64 = 400;
const TRAVEL_DAYS: i64 = 400;
const PACKAGES_DAYS: i64 = 60;
const BILLS_DAYS: i64 = 365;
const RESERVATIONS_DAYS: i64 = 400;
const SUBSCRIPTION_DAYS: i64 = 800;
const INVITE_DAYS: i64 = 60;
const CODE_HOURS: i64 = 48;
/// A parcel with no news for this long isn't "on the way" any more.
const PACKAGE_STALE_DAYS: i64 = 21;

/// The fixed columns `summary_row` reads (the date is appended).
const COLS: &str = "t.account_id, t.thread_id, t.subject, t.snippet, t.participants, t.message_count, t.flags, t.labels, t.otp,
    (SELECT s.wake_at FROM snoozes s WHERE s.account_id = t.account_id AND s.thread_id = t.thread_id AND s.woke_at IS NULL)";

/// This Mac's UTC offset now (what "today" means for the views).
pub(super) fn local_offset_secs() -> i32 {
    chrono::Local::now().offset().local_minus_utc()
}

/// Whether `id` names a smart view ("files:pdf" included).
pub fn is_smart_view(id: &str) -> bool {
    match id.split_once(':') {
        Some(("files", k)) => SMART_FILE_KINDS.contains(&k),
        Some(_) => false,
        None => SMART_VIEWS.contains(&id),
    }
}

/// One row of a view, before it becomes a thread summary.
struct Item {
    thread: i64,
    /// The row's date (the email the fact came from).
    date: i64,
    row: SmartRow,
    /// Counts toward the sidebar badge (upcoming, on the way, unpaid …).
    active: bool,
}

/// A stored fact and the message it came from.
#[derive(Clone)]
struct Fact {
    msg: i64,
    thread: i64,
    date: i64,
    key: String,
    reference: Option<String>,
    from_name: Option<String>,
    from_email: String,
    fact: Extracted,
}

/// Everything the builders need to know about "now".
#[derive(Clone, Copy)]
struct Now {
    ms: i64,
    off: i32,
    today: NaiveDate,
}

impl Now {
    fn new(ms: i64, off: i32) -> Now {
        Now {
            ms,
            off,
            today: day_of(ms, off),
        }
    }
    fn iso(&self) -> String {
        self.today.format("%Y-%m-%d").to_string()
    }
}

fn day_of(ms: i64, off: i32) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(ms + i64::from(off) * 1000)
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

/// "2026-10-02T19:05" or "2026-10-02" → the date.
fn parse_day(at: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(at.get(..10)?, "%Y-%m-%d").ok()
}

/// "Oct 2" or "Oct 2, 2027".
fn short_day(d: NaiveDate, today: NaiveDate) -> String {
    if d.year() == today.year() {
        d.format("%b %-d").to_string()
    } else {
        d.format("%b %-d, %Y").to_string()
    }
}

/// "Oct 2–9", "Oct 30 – Nov 2", "Oct 2".
fn day_span(a: NaiveDate, b: NaiveDate, today: NaiveDate) -> String {
    if b <= a {
        return short_day(a, today);
    }
    if a.month() == b.month() && a.year() == b.year() {
        format!("{}\u{2013}{}", short_day(a, today), b.day())
    } else {
        format!("{} \u{2013} {}", short_day(a, today), short_day(b, today))
    }
}

/// `AND col IN (?n, …)` for a scope, appending the ids to `args`.
fn in_scope(col: &str, scope: Option<&[AccountId]>, args: &mut Vec<Value>) -> String {
    let Some(ids) = scope else {
        return String::new();
    };
    let marks: Vec<String> = ids
        .iter()
        .map(|a| {
            args.push(Value::Text(a.clone()));
            format!("?{}", args.len())
        })
        .collect();
    format!(" AND {col} IN ({})", marks.join(", "))
}

/// People you know (`Store::list_people_you_know`): the index and WHERE of
/// its inbox threads with mail from someone you've written to, and their
/// arguments (?1 before, ?2 limit, then the scope). The EXISTS reads
/// `v.thread_rowid`, so a count needs no thread rows.
fn people_you_know_where(
    scope: Option<&[AccountId]>,
    before: Option<i64>,
    limit: u32,
    unread_only: bool,
) -> (&'static str, String, Vec<Value>) {
    let mut args = vec![
        Value::Integer(before.unwrap_or(i64::MAX)),
        Value::Integer(i64::from(limit.clamp(1, 1000))),
    ];
    let scoped = in_scope("v.account_id", scope, &mut args);
    let (idx, unread) = if unread_only {
        ("thread_views_unread_all", " AND v.unread = 1")
    } else {
        ("thread_views_all", "")
    };
    let not_people = F_SENT | F_DRAFT | F_NEWSLETTER | F_TRASH | F_SPAM;
    let cond = format!(
        "v.view = 'INBOX' AND v.last_date < ?1{unread}{scoped}
           AND EXISTS (SELECT 1 FROM messages m JOIN first_contacts f ON f.email = lower(m.from_email) AND f.dir = 1
                       WHERE m.thread_rowid = v.thread_rowid AND m.flags & {not_people} = 0)"
    );
    (idx, cond, args)
}

/// Facts of one kind dated `since` or later, newest first, in scope.
fn load_facts(
    c: &Connection,
    kind: &str,
    since: i64,
    scope: Option<&[AccountId]>,
) -> Result<Vec<Fact>> {
    let mut args = vec![Value::Text(kind.into()), Value::Integer(since)];
    let scoped = in_scope("x.account_id", scope, &mut args);
    let sql = format!(
        "SELECT x.msg, m.thread_rowid, x.date, x.key, x.ref, m.from_name, m.from_email, x.data
         FROM extracted x INDEXED BY extracted_kind JOIN messages m ON m.rowid = x.msg
         WHERE x.kind = ?1 AND x.date >= ?2 AND m.flags & {HIDDEN} = 0{scoped}
         ORDER BY x.date DESC, x.msg DESC, x.ord LIMIT {MAX_FACTS}"
    );
    let mut stmt = c.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| {
        let data: String = r.get(7)?;
        let Ok(fact) = serde_json::from_str::<Extracted>(&data) else {
            return Ok(None);
        };
        Ok(Some(Fact {
            msg: r.get(0)?,
            thread: r.get(1)?,
            date: r.get(2)?,
            key: r.get(3)?,
            reference: r.get(4)?,
            from_name: r.get(5)?,
            from_email: r.get(6)?,
            fact,
        }))
    })?;
    let mut out = Vec::new();
    for row in rows {
        if let Some(f) = row? {
            out.push(f);
        }
    }
    Ok(out)
}

/// A display name for whoever a fact is from: the name it carries, else
/// the sender's name without "Receipts"/"Billing", else the lookup key.
fn who(named: Option<&str>, f: &Fact) -> String {
    if let Some(n) = named.map(clean_org).filter(|n| !n.is_empty()) {
        return n;
    }
    if let Some(n) = f
        .from_name
        .as_deref()
        .map(clean_org)
        .filter(|n| !n.is_empty())
    {
        return n;
    }
    let k = if f.key.is_empty() {
        f.from_email.split('@').next().unwrap_or("").to_string()
    } else {
        f.key.clone()
    };
    let mut ch = k.chars();
    match ch.next() {
        Some(first) => first.to_uppercase().chain(ch).collect(),
        None => k,
    }
}

fn norm_ref(r: &str) -> String {
    r.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_uppercase()
}

// ------------------------------------------------------------------ receipts

/// One purchase: its confirmation, receipt and shipping notices (same
/// merchant and order number) count once; a refund is its own entry with a
/// negative amount (as Ask counts spending).
struct Purchase {
    newest: Fact,
    merchant: String,
    key: String,
    amount: Option<Money>,
    status: Option<String>,
    items: Vec<String>,
    order_number: Option<String>,
}

fn purchases(c: &Connection, scope: Option<&[AccountId]>, since: i64) -> Result<Vec<Purchase>> {
    let facts = load_facts(c, "order", since, scope)?;
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<Fact>> = HashMap::new();
    for f in facts {
        let Extracted::Order(o) = &f.fact else {
            continue;
        };
        let refund = o.total.as_ref().is_some_and(|m| m.value < 0.0)
            || o.status.as_deref() == Some("refunded");
        let k = match f
            .reference
            .as_deref()
            .map(norm_ref)
            .filter(|r| !r.is_empty())
        {
            Some(r) => format!("{}|{r}|{refund}", f.key),
            None => format!("#{}", f.msg),
        };
        if !groups.contains_key(&k) {
            order.push(k.clone());
        }
        groups.entry(k).or_default().push(f);
    }
    let mut out = Vec::with_capacity(order.len());
    for k in order {
        let Some(g) = groups.remove(&k) else { continue };
        let newest = g[0].clone();
        let orders: Vec<&crate::structured::Order> = g
            .iter()
            .filter_map(|f| match &f.fact {
                Extracted::Order(o) => Some(o),
                _ => None,
            })
            .collect();
        let merchant_name = orders.iter().find_map(|o| o.merchant.clone());
        let amount = orders.iter().find_map(|o| o.total.clone());
        let status = orders.iter().find_map(|o| o.status.clone());
        let items = orders
            .iter()
            .find(|o| !o.items.is_empty())
            .map(|o| o.items.clone())
            .unwrap_or_default();
        let order_number = orders.iter().find_map(|o| o.order_number.clone());
        out.push(Purchase {
            merchant: who(merchant_name.as_deref(), &newest),
            key: newest.key.clone(),
            newest,
            amount,
            status,
            items,
            order_number,
        });
    }
    Ok(out)
}

fn receipts(c: &Connection, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let list = purchases(c, scope, now.ms - RECEIPTS_DAYS * DAY)?;
    let month = (now.today.year(), now.today.month());
    Ok(list
        .into_iter()
        .map(|p| {
            let d = day_of(p.newest.date, now.off);
            let status = p
                .status
                .filter(|s| matches!(s.as_str(), "refunded" | "cancelled" | "returned"));
            Item {
                thread: p.newest.thread,
                date: p.newest.date,
                active: (d.year(), d.month()) == month,
                row: SmartRow {
                    kind: "receipt".into(),
                    title: p.merchant,
                    amount: p.amount,
                    at: None,
                    end: None,
                    reference: p.order_number,
                    status,
                    detail: (!p.items.is_empty()).then(|| p.items.join(", ")),
                    group: Some(d.format("%B %Y").to_string()),
                },
            }
        })
        .collect())
}

/// Per-currency sums, largest first ("never converted").
fn totals<'a>(amounts: impl Iterator<Item = &'a Money>) -> Vec<Money> {
    let mut by: BTreeMap<String, f64> = BTreeMap::new();
    for m in amounts {
        *by.entry(m.currency.clone()).or_default() += m.value;
    }
    let mut out: Vec<Money> = by
        .into_iter()
        .map(|(currency, value)| Money {
            value: (value * 100.0).round() / 100.0,
            currency,
        })
        .collect();
    out.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs()));
    out
}

fn receipts_info(items: &[Item], now: Now) -> Vec<SmartStat> {
    let this: Vec<&Item> = items.iter().filter(|i| i.active).collect();
    let last_month = now
        .today
        .with_day(1)
        .and_then(|d| d.pred_opt())
        .map(|d| (d.year(), d.month()));
    let last: Vec<&Item> = items
        .iter()
        .filter(|i| {
            let d = day_of(i.date, now.off);
            Some((d.year(), d.month())) == last_month
        })
        .collect();
    let mut stats = vec![SmartStat {
        label: "This month".into(),
        value: this.is_empty().then(|| "Nothing yet".into()),
        amounts: totals(this.iter().filter_map(|i| i.row.amount.as_ref())),
        tone: None,
    }];
    stats.push(SmartStat {
        label: "Receipts".into(),
        value: Some(this.len().to_string()),
        amounts: vec![],
        tone: None,
    });
    if !last.is_empty() {
        stats.push(SmartStat {
            label: "Last month".into(),
            value: None,
            amounts: totals(last.iter().filter_map(|i| i.row.amount.as_ref())),
            tone: None,
        });
    }
    stats
}

// -------------------------------------------------------------- subscriptions

/// How often a subscription charges: a name and its nominal length.
const CADENCES: &[(&str, f64, f64, f64)] = &[
    // (name, nominal days, min, max) for the median gap between charges.
    ("Weekly", 7.0, 6.0, 8.0),
    ("Monthly", 30.44, 26.0, 35.0),
    ("Quarterly", 91.3, 84.0, 98.0),
    ("Yearly", 365.25, 350.0, 380.0),
];

/// Recurring charges: the same merchant and currency, charged at least
/// three times (twice for yearly) at a steady cadence. See `docs/ARCHITECTURE.md`
/// (Smart views) for the rule.
struct Subscription {
    merchant: String,
    last: Fact,
    amount: Money,
    cadence: &'static str,
    nominal: f64,
    count: usize,
    /// Every charge counted: (message rowid, unix ms, amount), oldest first.
    charges: Vec<(i64, i64, f64)>,
}

fn detect_subscriptions(list: Vec<Purchase>) -> Vec<Subscription> {
    let mut by: HashMap<(String, String), Vec<Purchase>> = HashMap::new();
    for p in list {
        let Some(m) = p.amount.as_ref().filter(|m| m.value > 0.0) else {
            continue;
        };
        by.entry((p.key.clone(), m.currency.clone()))
            .or_default()
            .push(p);
    }
    let mut out = Vec::new();
    for (_, mut charges) in by {
        charges.sort_by_key(|p| p.newest.date);
        // Two emails about one charge a day apart (a receipt and an invoice)
        // are one charge.
        charges.dedup_by(|b, a| (b.newest.date - a.newest.date).abs() < 3 * DAY);
        if charges.len() < 2 {
            continue;
        }
        let gaps: Vec<f64> = charges
            .windows(2)
            .map(|w| (w[1].newest.date - w[0].newest.date) as f64 / DAY as f64)
            .collect();
        let mut sorted = gaps.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let Some(&(name, nominal, lo, hi)) = CADENCES
            .iter()
            .find(|(_, _, lo, hi)| median >= *lo && median <= *hi)
        else {
            continue;
        };
        // Yearly needs two charges; the rest three (two gaps agreeing).
        let need = if name == "Yearly" { 2 } else { 3 };
        if charges.len() < need {
            continue;
        }
        // Steady: nearly every gap near the cadence (one skipped or doubled
        // period at most).
        let steady = gaps
            .iter()
            .filter(|g| **g >= lo * 0.85 && **g <= hi * 1.15)
            .count();
        if (steady as f64) < (gaps.len() as f64 * 0.75).ceil() {
            continue;
        }
        // Similar amounts: the last three within 25% of their median (a
        // price change once is fine; a shop you buy from monthly isn't).
        let recent: Vec<f64> = charges
            .iter()
            .rev()
            .take(3)
            .filter_map(|p| p.amount.as_ref().map(|m| m.value))
            .collect();
        let mut r = recent.clone();
        r.sort_by(f64::total_cmp);
        let mid = r[r.len() / 2];
        if recent.iter().any(|v| (v - mid).abs() > mid * 0.25) {
            continue;
        }
        let history: Vec<(i64, i64, f64)> = charges
            .iter()
            .map(|p| (p.newest.msg, p.newest.date, p.amount.as_ref().map_or(0.0, |m| m.value)))
            .collect();
        let last = charges.pop().expect("at least two charges");
        out.push(Subscription {
            merchant: last.merchant.clone(),
            amount: last.amount.clone().expect("filtered on amount"),
            last: last.newest,
            cadence: name,
            nominal,
            count: charges.len() + 1,
            charges: history,
        });
    }
    out
}

fn subscriptions(c: &Connection, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let found = detect_subscriptions(purchases(c, scope, now.ms - SUBSCRIPTION_DAYS * DAY)?);
    let mut items: Vec<(bool, f64, Item)> = found
        .into_iter()
        .map(|s| {
            let next = s.last.date + (s.nominal * DAY as f64) as i64;
            // Active until a charge is clearly overdue (half a period late,
            // and at least a week).
            let grace = ((s.nominal * 0.5).max(7.0) * DAY as f64) as i64;
            let active = now.ms <= next + grace;
            let monthly = s.amount.value * 30.44 / s.nominal;
            (
                active,
                monthly,
                Item {
                    thread: s.last.thread,
                    date: s.last.date,
                    active,
                    row: SmartRow {
                        kind: "subscription".into(),
                        title: s.merchant,
                        amount: Some(s.amount),
                        at: Some(day_of(s.last.date, now.off).format("%Y-%m-%d").to_string()),
                        end: active.then(|| day_of(next, now.off).format("%Y-%m-%d").to_string()),
                        reference: None,
                        status: Some(if active { "active" } else { "stopped" }.into()),
                        detail: Some(format!("{} \u{b7} {} charges", s.cadence, s.count)),
                        group: Some(if active { "Active" } else { "Stopped?" }.into()),
                    },
                },
            )
        })
        .collect();
    // Active first, the costliest first; then the stopped, latest first.
    items.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            if a.0 {
                b.1.total_cmp(&a.1)
            } else {
                b.2.date.cmp(&a.2.date)
            }
        })
    });
    Ok(items.into_iter().map(|(_, _, i)| i).collect())
}

/// A recurring charge as Ask shows it ("what subscriptions do I pay for"):
/// the Subscriptions view's rule, with every charge it counted.
pub(crate) struct Recurring {
    pub merchant: String,
    /// The latest charge.
    pub amount: Money,
    /// "Monthly", "Yearly"…
    pub cadence: &'static str,
    /// The latest charge as a month's worth (a yearly one divided by 12).
    pub per_month: f64,
    /// (message rowid, unix ms, amount), oldest first.
    pub charges: Vec<(i64, i64, f64)>,
    /// Charged recently enough to still be running.
    pub active: bool,
    /// Past due but within one more period: a charge may just be late (or
    /// its email missing); not yet clearly stopped.
    pub late: bool,
    /// When the next charge is due (unix ms).
    pub next: i64,
}

/// Recurring charges over the Subscriptions view's window: active first,
/// the costliest a month first; then the stopped ones, latest first.
pub(crate) fn recurring_charges(
    c: &Connection,
    scope: Option<&[AccountId]>,
    now_ms: i64,
) -> Result<Vec<Recurring>> {
    let found = detect_subscriptions(purchases(c, scope, now_ms - SUBSCRIPTION_DAYS * DAY)?);
    let mut out: Vec<Recurring> = found
        .into_iter()
        .map(|s| {
            let next = s.last.date + (s.nominal * DAY as f64) as i64;
            let grace = ((s.nominal * 0.5).max(7.0) * DAY as f64) as i64;
            Recurring {
                per_month: s.amount.value * 30.44 / s.nominal,
                merchant: s.merchant,
                amount: s.amount,
                cadence: s.cadence,
                charges: s.charges,
                active: now_ms <= next + grace,
                late: now_ms > next + grace && now_ms <= next + grace + (s.nominal * DAY as f64) as i64,
                next,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.active.cmp(&a.active).then_with(|| {
            if a.active {
                b.per_month.total_cmp(&a.per_month)
            } else {
                b.next.cmp(&a.next)
            }
        })
    });
    Ok(out)
}

fn subscriptions_info(items: &[Item]) -> Vec<SmartStat> {
    let active: Vec<&Item> = items.iter().filter(|i| i.active).collect();
    // A month's worth of each, per currency.
    let monthly: Vec<Money> = active
        .iter()
        .filter_map(|i| {
            let m = i.row.amount.as_ref()?;
            let cadence = i.row.detail.as_deref().unwrap_or("");
            let nominal = CADENCES
                .iter()
                .find(|(n, ..)| cadence.starts_with(n))
                .map_or(30.44, |c| c.1);
            Some(Money {
                value: m.value * 30.44 / nominal,
                currency: m.currency.clone(),
            })
        })
        .collect();
    vec![
        SmartStat {
            label: "Active".into(),
            value: Some(active.len().to_string()),
            amounts: vec![],
            tone: None,
        },
        SmartStat {
            label: "About a month".into(),
            value: monthly.is_empty().then(|| "\u{2014}".into()),
            amounts: totals(monthly.iter()),
            tone: None,
        },
    ]
}

// -------------------------------------------------------------------- travel

/// A trip leg, stay or rental: its start date, its end, and the city it
/// takes you to (flights only).
struct Leg {
    item: Item,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    to_city: Option<String>,
}

fn airport_label(code: Option<&str>, name: Option<&str>) -> Option<String> {
    code.map(str::to_string)
        .or_else(|| name.map(str::to_string))
        .filter(|s| !s.is_empty())
}

fn travel_leg(f: &Fact) -> Option<Leg> {
    let (kind, title, detail, reference, amount, status, at, end, to_city) = match &f.fact {
        Extracted::Flight(x) => {
            let from = airport_label(x.depart_airport.as_deref(), x.depart_name.as_deref());
            let to = airport_label(x.arrive_airport.as_deref(), x.arrive_name.as_deref());
            let title = match (&from, &to) {
                (Some(a), Some(b)) => format!("{a} \u{2192} {b}"),
                (None, Some(b)) => format!("To {b}"),
                (Some(a), None) => format!("From {a}"),
                (None, None) => x.flight_number.clone().unwrap_or_else(|| "Flight".into()),
            };
            let city = x
                .arrive_airport
                .as_deref()
                .and_then(airport_by_code)
                .map(|(_, city)| city.to_string())
                .or_else(|| x.arrive_name.clone());
            let detail = [x.flight_number.clone(), x.airline.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            (
                "flight",
                title,
                detail,
                x.confirmation.clone(),
                x.total.clone(),
                x.status.clone(),
                x.depart_time.clone(),
                x.arrive_time.clone(),
                city,
            )
        }
        Extracted::Lodging(x) => (
            "stay",
            x.name.clone().unwrap_or_else(|| "Stay".into()),
            x.address.clone().unwrap_or_default(),
            x.confirmation.clone(),
            x.total.clone(),
            x.status.clone(),
            x.checkin.clone(),
            x.checkout.clone(),
            None,
        ),
        Extracted::Reservation(x)
            if matches!(x.category.as_str(), "rentalCar" | "train" | "bus") =>
        {
            let kind = match x.category.as_str() {
                "rentalCar" => "car",
                "train" => "train",
                _ => "bus",
            };
            let fallback = match kind {
                "car" => "Rental car",
                "train" => "Train",
                _ => "Bus",
            };
            (
                kind,
                x.name.clone().unwrap_or_else(|| fallback.into()),
                x.venue
                    .clone()
                    .or_else(|| x.address.clone())
                    .unwrap_or_default(),
                x.confirmation.clone(),
                x.total.clone(),
                x.status.clone(),
                x.start.clone(),
                x.end.clone(),
                None,
            )
        }
        _ => return None,
    };
    let start = at.as_deref().and_then(parse_day);
    let end_day = end.as_deref().and_then(parse_day);
    Some(Leg {
        item: Item {
            thread: f.thread,
            date: f.date,
            active: false,
            row: SmartRow {
                kind: kind.into(),
                title,
                amount,
                at,
                end,
                reference,
                status: status.filter(|s| s == "cancelled"),
                detail: (!detail.is_empty()).then_some(detail),
                group: None,
            },
        },
        start,
        end: end_day,
        to_city,
    })
}

/// A booking re-sent (a change, a reminder) is one row: same kind,
/// reference and day, newest email first (the input is newest first).
fn dedupe_bookings(legs: Vec<Leg>) -> Vec<Leg> {
    let mut seen: HashSet<String> = HashSet::new();
    legs.into_iter()
        .filter(|l| {
            let Some(r) = l.item.row.reference.as_deref().map(norm_ref) else {
                return true;
            };
            let k = format!(
                "{}|{r}|{}|{}",
                l.item.row.kind,
                l.item
                    .row
                    .at
                    .as_deref()
                    .and_then(|a| a.get(..10))
                    .unwrap_or(""),
                l.item.row.title
            );
            seen.insert(k)
        })
        .collect()
}

/// Upcoming first, soonest first, grouped into trips (legs no more than
/// two days apart); then the past, latest first.
fn order_trips(legs: Vec<Leg>, now: Now, upcoming_label: &str) -> Vec<Item> {
    let (mut up, mut past): (Vec<Leg>, Vec<Leg>) = legs.into_iter().partition(|l| {
        l.start.is_some_and(|d| d >= now.today)
            || l.end
                .is_some_and(|d| d >= now.today && l.item.row.kind == "stay")
    });
    up.sort_by_key(|l| (l.start, l.item.row.at.clone()));
    past.sort_by(|a, b| b.start.cmp(&a.start).then(b.item.date.cmp(&a.item.date)));
    let mut out: Vec<Item> = Vec::with_capacity(up.len() + past.len());
    let mut i = 0;
    while i < up.len() {
        let first = up[i].start.unwrap_or(now.today);
        let mut last = up[i].end.unwrap_or(first).max(first);
        let mut j = i + 1;
        while j < up.len() {
            let s = up[j].start.unwrap_or(last);
            if s > last + Duration::days(2) {
                break;
            }
            last = last.max(up[j].end.unwrap_or(s)).max(s);
            j += 1;
        }
        let city = up[i..j].iter().find_map(|l| l.to_city.clone());
        let label = match &city {
            Some(c) => format!("Trip to {c} \u{b7} {}", day_span(first, last, now.today)),
            None => format!(
                "{upcoming_label} \u{b7} {}",
                day_span(first, last, now.today)
            ),
        };
        for l in &mut up[i..j] {
            l.item.row.group = Some(label.clone());
            l.item.active = l.item.row.status.as_deref() != Some("cancelled");
            if l.item.row.status.is_none() {
                l.item.row.status = Some("upcoming".into());
            }
        }
        i = j;
    }
    out.extend(up.into_iter().map(|l| l.item));
    out.extend(past.into_iter().map(|mut l| {
        l.item.row.group = Some("Past".into());
        if l.item.row.status.is_none() {
            l.item.row.status = Some("past".into());
        }
        l.item
    }));
    out
}

fn travel(c: &Connection, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let since = now.ms - TRAVEL_DAYS * DAY;
    let mut facts = load_facts(c, "flight", since, scope)?;
    facts.extend(load_facts(c, "lodging", since, scope)?);
    facts.extend(load_facts(c, "reservation", since, scope)?);
    facts.sort_by(|a, b| b.date.cmp(&a.date).then(b.msg.cmp(&a.msg)));
    let legs = dedupe_bookings(facts.iter().filter_map(travel_leg).collect());
    Ok(order_trips(legs, now, "Upcoming"))
}

fn upcoming_info(items: &[Item], what: &str) -> Vec<SmartStat> {
    let up: Vec<&Item> = items.iter().filter(|i| i.active).collect();
    let mut stats = vec![SmartStat {
        label: "Upcoming".into(),
        value: Some(up.len().to_string()),
        amounts: vec![],
        tone: None,
    }];
    if let Some(next) = up.first() {
        stats.push(SmartStat {
            label: format!("Next {what}"),
            value: Some(next.row.title.clone()),
            amounts: vec![],
            tone: None,
        });
        if let Some(g) = next.row.group.as_deref() {
            stats.push(SmartStat {
                label: "When".into(),
                value: Some(g.rsplit(" \u{b7} ").next().unwrap_or(g).to_string()),
                amounts: vec![],
                tone: None,
            });
        }
    }
    stats
}

// -------------------------------------------------------------- reservations

fn reservations(c: &Connection, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let facts = load_facts(c, "reservation", now.ms - RESERVATIONS_DAYS * DAY, scope)?;
    let legs: Vec<Leg> = facts
        .iter()
        .filter_map(|f| {
            let Extracted::Reservation(x) = &f.fact else {
                return None;
            };
            if matches!(x.category.as_str(), "rentalCar" | "train" | "bus") {
                return None;
            }
            let kind = match x.category.as_str() {
                "restaurant" => "restaurant",
                "event" => "event",
                _ => "reservation",
            };
            let mut detail: Vec<String> = Vec::new();
            if let Some(n) = x.party_size {
                detail.push(if kind == "restaurant" {
                    format!("Table for {n}")
                } else {
                    format!("{n} {}", if n == 1 { "ticket" } else { "tickets" })
                });
            }
            if let Some(v) = x.venue.clone().filter(|v| Some(v) != x.name.as_ref()) {
                detail.push(v);
            }
            let start = x.start.as_deref().and_then(parse_day);
            Some(Leg {
                item: Item {
                    thread: f.thread,
                    date: f.date,
                    active: false,
                    row: SmartRow {
                        kind: kind.into(),
                        title: x
                            .name
                            .clone()
                            .or_else(|| x.venue.clone())
                            .unwrap_or_else(|| who(None, f)),
                        amount: x.total.clone(),
                        at: x.start.clone(),
                        end: x.end.clone(),
                        reference: x.confirmation.clone(),
                        status: x.status.clone().filter(|s| s == "cancelled"),
                        detail: (!detail.is_empty()).then(|| detail.join(" \u{b7} ")),
                        group: None,
                    },
                },
                start,
                end: None,
                to_city: None,
            })
        })
        .collect();
    let legs = dedupe_bookings(legs);
    let (mut up, mut past): (Vec<Leg>, Vec<Leg>) = legs
        .into_iter()
        .partition(|l| l.start.is_some_and(|d| d >= now.today));
    up.sort_by_key(|l| l.item.row.at.clone());
    past.sort_by(|a, b| b.start.cmp(&a.start).then(b.item.date.cmp(&a.item.date)));
    let mut out = Vec::new();
    for mut l in up {
        l.item.active = l.item.row.status.as_deref() != Some("cancelled");
        l.item.row.group = Some("Upcoming".into());
        l.item.row.status.get_or_insert_with(|| "upcoming".into());
        out.push(l.item);
    }
    for mut l in past {
        l.item.row.group = Some("Past".into());
        l.item.row.status.get_or_insert_with(|| "past".into());
        out.push(l.item);
    }
    Ok(out)
}

// ------------------------------------------------------------------ packages

fn packages(c: &Connection, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let facts = load_facts(c, "shipment", now.ms - PACKAGES_DAYS * DAY, scope)?;
    // One parcel per tracking number (else per email); facts are newest first.
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<Fact>> = HashMap::new();
    for f in facts {
        let k = match f
            .reference
            .as_deref()
            .map(norm_ref)
            .filter(|r| !r.is_empty())
        {
            Some(r) => r,
            None => format!("#{}", f.msg),
        };
        if !groups.contains_key(&k) {
            order.push(k.clone());
        }
        groups.entry(k).or_default().push(f);
    }
    let mut active: Vec<(Option<String>, Item)> = Vec::new();
    let mut done: Vec<Item> = Vec::new();
    for k in order {
        let Some(g) = groups.remove(&k) else { continue };
        let newest = &g[0];
        let ships: Vec<&crate::structured::Shipment> = g
            .iter()
            .filter_map(|f| match &f.fact {
                Extracted::Shipment(s) => Some(s),
                _ => None,
            })
            .collect();
        let status = ships
            .iter()
            .find_map(|s| s.status.clone())
            .unwrap_or_else(|| "shipped".into());
        let expected = ships.iter().find_map(|s| s.expected.clone());
        let carrier = ships.iter().find_map(|s| s.carrier.clone());
        let merchant = ships.iter().find_map(|s| s.merchant.clone());
        let items = ships
            .iter()
            .find(|s| !s.items.is_empty())
            .map(|s| s.items.join(", "));
        let title = match (&merchant, &carrier) {
            (Some(m), _) => clean_org(m),
            (None, Some(c)) => c.clone(),
            (None, None) => who(None, newest),
        };
        let detail = [
            carrier
                .clone()
                .filter(|c| Some(c) != merchant.as_ref() && *c != title),
            items,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
        let stale = now.ms - newest.date > PACKAGE_STALE_DAYS * DAY;
        let on_the_way = status != "delivered" && !stale;
        let item = Item {
            thread: newest.thread,
            date: newest.date,
            active: on_the_way,
            row: SmartRow {
                kind: "parcel".into(),
                title,
                amount: None,
                at: expected.clone(),
                end: None,
                reference: ships.iter().find_map(|s| s.tracking_number.clone()),
                status: Some(status.clone()),
                detail: (!detail.is_empty()).then_some(detail),
                group: Some(
                    if on_the_way {
                        "On the way"
                    } else if status == "delivered" {
                        "Delivered"
                    } else {
                        "Earlier"
                    }
                    .into(),
                ),
            },
        };
        if on_the_way {
            active.push((expected, item));
        } else {
            done.push(item);
        }
    }
    // On the way: the soonest expected first (unknown last, newest first).
    active.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.1.date.cmp(&a.1.date),
    });
    // Delivered before stale, each latest first.
    done.sort_by(|a, b| {
        let rank = |i: &Item| u8::from(i.row.group.as_deref() != Some("Delivered"));
        rank(a).cmp(&rank(b)).then(b.date.cmp(&a.date))
    });
    Ok(active.into_iter().map(|(_, i)| i).chain(done).collect())
}

fn packages_info(items: &[Item], now: Now) -> Vec<SmartStat> {
    let on: Vec<&Item> = items.iter().filter(|i| i.active).collect();
    let today = now.iso();
    let arriving = on
        .iter()
        .filter(|i| {
            i.row.status.as_deref() == Some("outForDelivery")
                || i.row.at.as_deref().and_then(|a| a.get(..10)) == Some(today.as_str())
        })
        .count();
    let mut stats = vec![SmartStat {
        label: "On the way".into(),
        value: Some(on.len().to_string()),
        amounts: vec![],
        tone: None,
    }];
    if arriving > 0 {
        stats.push(SmartStat {
            label: "Arriving today".into(),
            value: Some(arriving.to_string()),
            amounts: vec![],
            tone: Some("good".into()),
        });
    }
    let delivered = items
        .iter()
        .filter(|i| i.row.status.as_deref() == Some("delivered") && now.ms - i.date < 7 * DAY)
        .count();
    if delivered > 0 {
        stats.push(SmartStat {
            label: "Delivered this week".into(),
            value: Some(delivered.to_string()),
            amounts: vec![],
            tone: None,
        });
    }
    stats
}

// --------------------------------------------------------------------- bills

/// Bills, paid the way Ask decides it: a bill is paid when it says so, or
/// when a later email from the same biller says a payment was received for
/// the same invoice number or the same amount.
fn bills(c: &Connection, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let facts = load_facts(c, "bill", now.ms - BILLS_DAYS * DAY, scope)?;
    let bill = |f: &Fact| match &f.fact {
        Extracted::Bill(b) => Some(b.clone()),
        _ => None,
    };
    let paid_facts: Vec<(&Fact, crate::structured::Bill)> = facts
        .iter()
        .filter_map(|f| bill(f).map(|b| (f, b)))
        .filter(|(_, b)| b.status.as_deref() == Some("paid"))
        .collect();
    // Payment emails that settle an earlier bill fold into it.
    let mut absorbed: HashSet<i64> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut rows: Vec<(u8, Option<String>, Item)> = Vec::new();
    let today = now.iso();
    let overdue_from = (now.today - Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    for f in &facts {
        let Some(b) = bill(f) else { continue };
        if b.status.as_deref() == Some("paid") {
            continue;
        }
        // A reminder for the same invoice is one row (the newest email).
        let dedupe = match b.invoice_number.as_deref().map(norm_ref) {
            Some(r) if !r.is_empty() => format!("{}|{r}", f.key),
            _ => format!("#{}", f.msg),
        };
        if !seen.insert(dedupe) {
            continue;
        }
        let payment = paid_facts.iter().find(|(p, pb)| {
            p.key == f.key
                && p.date >= f.date
                && ((pb.invoice_number.is_some() && pb.invoice_number == b.invoice_number)
                    || (pb.amount_due.is_some() && pb.amount_due == b.amount_due))
        });
        if let Some((p, _)) = payment {
            absorbed.insert(p.msg);
        }
        let due = b.due_date.clone();
        let (rank, status, group) = if payment.is_some() {
            (3, "paid", "Paid")
        } else {
            match due.as_deref() {
                Some(d) if d < today.as_str() && d >= overdue_from.as_str() => {
                    (0, "overdue", "Overdue")
                }
                Some(d) if d >= today.as_str() => (1, "due", "Due"),
                None if now.ms - f.date <= 45 * DAY => (1, "due", "Due"),
                // Long past due with no payment email: autopay, or paid
                // elsewhere. Listed, not flagged.
                _ => (4, "unpaid", "Earlier"),
            }
        };
        rows.push((
            rank,
            due.clone(),
            Item {
                thread: f.thread,
                date: f.date,
                active: rank < 2,
                row: SmartRow {
                    kind: "bill".into(),
                    title: who(b.biller.as_deref(), f),
                    amount: b.amount_due.clone(),
                    at: due,
                    end: None,
                    reference: b.invoice_number.clone(),
                    status: Some(status.into()),
                    detail: None,
                    group: Some(group.into()),
                },
            },
        ));
    }
    // Payment emails with no bill of their own here.
    for (p, pb) in &paid_facts {
        if absorbed.contains(&p.msg) {
            continue;
        }
        rows.push((
            3,
            None,
            Item {
                thread: p.thread,
                date: p.date,
                active: false,
                row: SmartRow {
                    kind: "bill".into(),
                    title: who(pb.biller.as_deref(), p),
                    amount: pb.amount_due.clone(),
                    at: pb.due_date.clone(),
                    end: None,
                    reference: pb.invoice_number.clone(),
                    status: Some("paid".into()),
                    detail: None,
                    group: Some("Paid".into()),
                },
            },
        ));
    }
    // Overdue and due by due date (none last); paid and earlier latest first.
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| {
            if a.0 < 2 {
                match (&a.1, &b.1) {
                    (Some(x), Some(y)) => x.cmp(y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => b.2.date.cmp(&a.2.date),
                }
            } else {
                b.2.date.cmp(&a.2.date)
            }
        })
    });
    Ok(rows.into_iter().map(|(_, _, i)| i).collect())
}

fn bills_info(items: &[Item], now: Now) -> Vec<SmartStat> {
    let unpaid: Vec<&Item> = items.iter().filter(|i| i.active).collect();
    let overdue = unpaid
        .iter()
        .filter(|i| i.row.status.as_deref() == Some("overdue"))
        .count();
    let mut stats = vec![SmartStat {
        label: "Unpaid".into(),
        value: unpaid.is_empty().then(|| "Nothing due".into()),
        amounts: totals(unpaid.iter().filter_map(|i| i.row.amount.as_ref())),
        tone: None,
    }];
    if overdue > 0 {
        stats.push(SmartStat {
            label: "Overdue".into(),
            value: Some(overdue.to_string()),
            amounts: vec![],
            tone: Some("warn".into()),
        });
    }
    if let Some(next) = unpaid
        .iter()
        .find(|i| i.row.status.as_deref() == Some("due") && i.row.at.is_some())
    {
        let d = next.row.at.as_deref().and_then(parse_day);
        stats.push(SmartStat {
            label: "Next due".into(),
            value: Some(match d {
                Some(d) => format!("{} \u{b7} {}", next.row.title, short_day(d, now.today)),
                None => next.row.title.clone(),
            }),
            amounts: vec![],
            tone: None,
        });
    }
    stats
}

// ------------------------------------------------------------------- invites

fn invites(store: &Store, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let rows: Vec<(String, String)> = store.read(|c| {
        let mut args = vec![Value::Integer(now.ms - INVITE_DAYS * DAY)];
        let scoped = in_scope("account_id", scope, &mut args);
        let mut stmt = c.prepare_cached(&format!(
            "SELECT account_id, thread_id FROM invites WHERE date >= ?1 AND data != ''{scoped}
             GROUP BY account_id, thread_id"
        ))?;
        let rows = stmt.query_map(params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })?;
    let mut out: Vec<(i64, Item)> = Vec::new();
    for (account_id, thread_id) in rows {
        let Some(stored) = store.thread_invite(&account_id, &thread_id)? else {
            continue;
        };
        let chip = store.invite_chip(&account_id, &stored, now.ms)?;
        let waiting = chip.can_respond
            && chip.method == "request"
            && matches!(chip.response.as_deref(), None | Some("needsAction"));
        if !waiting {
            continue;
        }
        let Some(thread) = thread_rowid(store, &account_id, &thread_id)? else {
            continue;
        };
        out.push((
            chip.start,
            Item {
                thread,
                date: stored.date,
                active: true,
                row: SmartRow {
                    kind: "invite".into(),
                    title: chip.summary.clone(),
                    group: Some(
                        if chip.start < now.ms + 7 * DAY {
                            "This week"
                        } else {
                            "Later"
                        }
                        .into(),
                    ),
                    ..SmartRow::default()
                },
            },
        ));
    }
    // Soonest event first.
    out.sort_by_key(|(start, _)| *start);
    Ok(out.into_iter().map(|(_, i)| i).collect())
}

fn thread_rowid(store: &Store, account_id: &str, thread_id: &str) -> Result<Option<i64>> {
    store.read(|c| {
        Ok(
            c.prepare_cached("SELECT rowid FROM threads WHERE account_id = ?1 AND thread_id = ?2")?
                .query_row(params![account_id, thread_id], |r| r.get(0))
                .optional()?,
        )
    })
}

// --------------------------------------------------------------------- codes

fn codes(c: &Connection, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
    let mut args = vec![Value::Integer(now.ms - CODE_HOURS * 3_600_000)];
    let scoped = in_scope("m.account_id", scope, &mut args);
    let mut stmt = c.prepare_cached(&format!(
        "SELECT m.thread_rowid, m.date, m.otp FROM messages m INDEXED BY messages_otp
         WHERE m.otp IS NOT NULL AND m.otp != '' AND m.date >= ?1 AND m.flags & {HIDDEN} = 0{scoped}
         ORDER BY m.date DESC LIMIT 1000"
    ))?;
    let rows = stmt.query_map(params_from_iter(args), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let (thread, date, otp) = row?;
        if !seen.insert(thread) {
            continue;
        }
        let Ok(otp) = serde_json::from_str::<Otp>(&otp) else {
            continue;
        };
        out.push(Item {
            thread,
            date,
            active: now.ms - date < 24 * 3_600_000,
            row: SmartRow {
                kind: match otp.kind {
                    crate::types::OtpKind::Code => "code",
                    crate::types::OtpKind::Link => "link",
                }
                .into(),
                ..SmartRow::default()
            },
        });
    }
    Ok(out)
}

// --------------------------------------------------------------------- files

fn file_bits(kind: Option<&str>) -> i64 {
    match kind {
        Some("pdf") => F_PDF,
        Some("images") => F_IMAGE,
        Some("docs") => F_DOC | F_PRES,
        Some("sheets") => F_SHEET,
        _ => F_ATTACH,
    }
}

/// The Files view's rows, newest first, one per thread (at most 300).
/// `titles`: read each row's file names for its title (the sidebar count
/// only needs the threads).
fn files(
    c: &Connection,
    scope: Option<&[AccountId]>,
    kind: Option<&str>,
    before: i64,
    titles: bool,
) -> Result<Vec<Item>> {
    let bits = file_bits(kind);
    let mut args = vec![Value::Integer(before)];
    let scoped = in_scope("m.account_id", scope, &mut args);
    // `flags & 512 != 0` names the partial index messages_attach, which
    // covers every column read here.
    let mut stmt = c.prepare_cached(&format!(
        "SELECT m.rowid, m.thread_rowid, m.date FROM messages m INDEXED BY messages_attach
         WHERE m.flags & {F_ATTACH} != 0 AND m.flags & {bits} != 0 AND m.flags & {HIDDEN} = 0 AND m.date < ?1{scoped}
         ORDER BY m.date DESC LIMIT 3000"
    ))?;
    let rows = stmt.query_map(params_from_iter(args), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut names = c.prepare_cached(
        "SELECT filename, mime_type FROM attachments WHERE message_rowid = ?1 AND inline = 0 ORDER BY ord",
    )?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let (msg, thread, date) = row?;
        if !seen.insert(thread) {
            continue;
        }
        if !titles {
            out.push(Item {
                thread,
                date,
                active: false,
                row: SmartRow::default(),
            });
            if out.len() >= 300 {
                break;
            }
            continue;
        }
        let files: Vec<(String, String)> = names
            .query_map([msg], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        // The file the filter is about first.
        let shown = files
            .iter()
            .find(|(n, m)| kind.is_none() || file_kind(n, m) == kind)
            .or(files.first());
        let title = shown.map(|(n, _)| n.clone()).unwrap_or_default();
        out.push(Item {
            thread,
            date,
            active: false,
            row: SmartRow {
                kind: "file".into(),
                title,
                detail: (files.len() > 1).then(|| format!("+{} more", files.len() - 1)),
                ..SmartRow::default()
            },
        });
        if out.len() >= 300 {
            break;
        }
    }
    Ok(out)
}

/// Which Files filter a file belongs to (by type, then extension).
fn file_kind(name: &str, mime: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let ext = ext.as_deref().unwrap_or("");
    if mime == "application/pdf" || ext == "pdf" {
        Some("pdf")
    } else if mime.starts_with("image/") {
        Some("images")
    } else if matches!(ext, "xls" | "xlsx" | "csv" | "numbers" | "ods")
        || mime.contains("spreadsheet")
    {
        Some("sheets")
    } else if matches!(
        ext,
        "doc" | "docx" | "pages" | "odt" | "rtf" | "txt" | "md" | "ppt" | "pptx" | "key" | "odp"
    ) || mime.contains("word")
        || mime.contains("presentation")
    {
        Some("docs")
    } else {
        None
    }
}

// ---------------------------------------------------------------- the store

impl Store {
    /// `MailboxView::Smart`: one smart view's rows, each with its
    /// [`SmartRow`]. `now_ms` and `utc_offset_secs` fix "today".
    pub fn list_smart(
        &self,
        view: &str,
        scope: Option<&[AccountId]>,
        query: &ListQuery,
        now_ms: i64,
        utc_offset_secs: i32,
    ) -> Result<Vec<ThreadSummary>> {
        if scope.is_some_and(<[AccountId]>::is_empty) || !is_smart_view(view) {
            return Ok(Vec::new());
        }
        let limit = if query.limit == 0 {
            50
        } else {
            query.limit.min(1000)
        };
        let paged = |view: MailboxView, tab: Option<InboxTab>| ListQuery {
            view,
            tab,
            account_id: None,
            account_ids: scope.map(<[AccountId]>::to_vec),
            limit,
            before: query.before,
            unread_only: query.unread_only,
            split: None,
        };
        match view {
            // The inbox's newsletters (List-Unsubscribe or a promotions/
            // updates category): a reading list, archived when read.
            "newsletters" => {
                return self.list_threads(&paged(MailboxView::Inbox, Some(InboxTab::Newsletters)))
            }
            "people" => {
                return self.list_people_you_know(scope, query.before, limit, query.unread_only)
            }
            _ => {}
        }
        let files_kind = view.strip_prefix("files:");
        if query.before.is_some() && !(view == "files" || files_kind.is_some()) {
            return Ok(Vec::new());
        }
        let items = if view == "files" || files_kind.is_some() {
            let before = query.before.unwrap_or(i64::MAX);
            self.read(|c| files(c, scope, files_kind, before, true))?
        } else {
            self.smart_items(view, scope, Now::new(now_ms, utc_offset_secs))?
        };
        self.summaries_for(items, query.unread_only)
    }

    fn smart_items(&self, view: &str, scope: Option<&[AccountId]>, now: Now) -> Result<Vec<Item>> {
        match view {
            "receipts" => self.read(|c| receipts(c, scope, now)),
            "travel" => self.read(|c| travel(c, scope, now)),
            "packages" => self.read(|c| packages(c, scope, now)),
            "bills" => self.read(|c| bills(c, scope, now)),
            "reservations" => self.read(|c| reservations(c, scope, now)),
            "subscriptions" => self.read(|c| subscriptions(c, scope, now)),
            "codes" => self.read(|c| codes(c, scope, now)),
            "invites" => invites(self, scope, now),
            _ => Ok(Vec::new()),
        }
    }

    /// Thread summaries for `items` in their order, one row per thread.
    fn summaries_for(&self, items: Vec<Item>, unread_only: bool) -> Result<Vec<ThreadSummary>> {
        self.read(|c| {
            // The first row of each thread; all of them may be needed to
            // find MAX_ROWS unread ones.
            let mut seen = HashSet::new();
            let items: Vec<Item> = items
                .into_iter()
                .filter(|it| seen.insert(it.thread))
                .take(if unread_only { usize::MAX } else { MAX_ROWS })
                .collect();
            let rowids: Vec<i64> = items.iter().map(|it| it.thread).collect();
            let mut summaries = summaries_by_rowid(c, &rowids)?;
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                let Some(mut s) = summaries.remove(&it.thread) else {
                    continue;
                };
                if unread_only && !s.unread {
                    continue;
                }
                s.last_date = it.date;
                s.smart = Some(it.row);
                out.push(s);
                if out.len() >= MAX_ROWS {
                    break;
                }
            }
            Ok(out)
        })
    }

    /// People you know: inbox conversations with a message from someone
    /// you've written to (not bulk mail), newest first. Pages with `before`.
    pub fn list_people_you_know(
        &self,
        scope: Option<&[AccountId]>,
        before: Option<i64>,
        limit: u32,
        unread_only: bool,
    ) -> Result<Vec<ThreadSummary>> {
        if scope.is_some_and(<[AccountId]>::is_empty) {
            return Ok(Vec::new());
        }
        let (idx, cond, args) = people_you_know_where(scope, before, limit, unread_only);
        let sql = format!(
            "SELECT {COLS}, v.last_date FROM thread_views v INDEXED BY {idx} JOIN threads t ON t.rowid = v.thread_rowid
             WHERE {cond} ORDER BY v.last_date DESC LIMIT ?2"
        );
        self.read(|c| {
            let mut stmt = c.prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(args), summary_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// How many rows `list_people_you_know` would return (at most
    /// `limit`), without reading the thread rows: the sidebar badge.
    fn count_people_you_know(
        &self,
        scope: Option<&[AccountId]>,
        limit: u32,
        unread_only: bool,
    ) -> Result<usize> {
        if scope.is_some_and(<[AccountId]>::is_empty) {
            return Ok(0);
        }
        let (idx, cond, args) = people_you_know_where(scope, None, limit, unread_only);
        let sql = format!(
            "SELECT count(*) FROM (SELECT 1 FROM thread_views v INDEXED BY {idx} WHERE {cond} LIMIT ?2)"
        );
        self.read(|c| {
            let n: i64 = c
                .prepare_cached(&sql)?
                .query_row(params_from_iter(args), |r| r.get(0))?;
            Ok(n as usize)
        })
    }

    /// `MailboxView::Query`: conversations matching a saved search, newest
    /// match first, one page. An unreadable query is `InvalidQuery`.
    pub fn list_saved_search(
        &self,
        q: &str,
        scope: Option<&[AccountId]>,
        query: &ListQuery,
    ) -> Result<Vec<ThreadSummary>> {
        if query.before.is_some() || scope.is_some_and(<[AccountId]>::is_empty) {
            return Ok(Vec::new());
        }
        let matches = self.match_query(q, scope, None, 5000)?;
        let mut seen = HashSet::new();
        let mut keys: Vec<(String, String, i64)> = Vec::new();
        for m in matches {
            if seen.insert((m.account_id.clone(), m.thread_id.clone())) {
                keys.push((m.account_id, m.thread_id, m.date));
            }
            if keys.len() >= MAX_ROWS {
                break;
            }
        }
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLS}, ?3 FROM threads t WHERE t.account_id = ?1 AND t.thread_id = ?2"
            ))?;
            let mut out = Vec::with_capacity(keys.len());
            for (a, t, d) in keys {
                if let Some(s) = stmt.query_row(params![a, t, d], summary_row).optional()? {
                    if !query.unread_only || s.unread {
                        out.push(s);
                    }
                }
            }
            Ok(out)
        })
    }

    /// The slim header over a smart view: its key figures, and a caveat
    /// while the fact scanner is still reading mail.
    pub fn smart_view_info(
        &self,
        view: &str,
        scope: Option<&[AccountId]>,
        now_ms: i64,
        utc_offset_secs: i32,
    ) -> Result<SmartViewInfo> {
        let mut info = SmartViewInfo {
            view: view.to_string(),
            ..SmartViewInfo::default()
        };
        if scope.is_some_and(<[AccountId]>::is_empty) || !is_smart_view(view) {
            return Ok(info);
        }
        let now = Now::new(now_ms, utc_offset_secs);
        let items = self.smart_items(view, scope, now)?;
        info.stats = match view {
            "receipts" => receipts_info(&items, now),
            "travel" => upcoming_info(&items, "trip"),
            "reservations" => upcoming_info(&items, "booking"),
            "packages" => packages_info(&items, now),
            "bills" => bills_info(&items, now),
            "subscriptions" => subscriptions_info(&items),
            "invites" => vec![SmartStat {
                label: "Waiting for your answer".into(),
                value: Some(items.len().to_string()),
                ..SmartStat::default()
            }],
            "codes" => vec![SmartStat {
                label: "Last 48 hours".into(),
                value: Some(items.len().to_string()),
                ..SmartStat::default()
            }],
            _ => vec![],
        };
        if matches!(
            view,
            "receipts" | "travel" | "packages" | "bills" | "reservations" | "subscriptions"
        ) {
            let pending = self.extract_remaining()?;
            if pending > 0 {
                info.note = Some(format!(
                    "Still reading {pending} {} for this list; it may be missing some.",
                    if pending == 1 { "email" } else { "emails" }
                ));
            }
        }
        Ok(info)
    }

    /// Sidebar counts for `views`: what's active in each (this month's
    /// receipts, upcoming trips and bookings, parcels on the way, unpaid
    /// bills, invitations waiting, codes from the last day, active
    /// subscriptions), or unread conversations for the plain lists (Files,
    /// Newsletters, People you know, a saved search as `query:<q>`).
    pub fn smart_counts(
        &self,
        views: &[String],
        scope: Option<&[AccountId]>,
        now_ms: i64,
        utc_offset_secs: i32,
    ) -> Result<Vec<SmartCount>> {
        let now = Now::new(now_ms, utc_offset_secs);
        let mut out = Vec::with_capacity(views.len());
        for view in views {
            let count = if scope.is_some_and(<[AccountId]>::is_empty) {
                0
            } else if let Some(q) = view.strip_prefix("query:") {
                // An unreadable saved search counts nothing (its list says why).
                let lq = ListQuery {
                    view: MailboxView::Query(q.to_string()),
                    tab: None,
                    account_id: None,
                    account_ids: None,
                    limit: 0,
                    before: None,
                    unread_only: true,
                    split: None,
                };
                self.list_saved_search(q, scope, &lq).map_or(0, |v| v.len())
            } else if !is_smart_view(view) {
                0
            } else if view == "files" || view.starts_with("files:") {
                // What the list shows, counted without reading file names
                // or building rows: unread threads among its threads.
                let kind = view.strip_prefix("files:");
                self.read(|c| {
                    let threads: Vec<i64> = files(c, scope, kind, i64::MAX, false)?
                        .iter()
                        .map(|i| i.thread)
                        .collect();
                    let n: i64 = c
                        .prepare_cached(&format!(
                            "SELECT count(*) FROM threads
                             WHERE rowid IN (SELECT value FROM json_each(?1)) AND flags & {F_UNREAD} != 0"
                        ))?
                        .query_row([serde_json::to_string(&threads)?], |r| r.get(0))?;
                    Ok(n as usize)
                })?
            } else if view == "people" {
                self.count_people_you_know(scope, 1000, true)?
            } else if view == "newsletters" {
                let q = ListQuery {
                    view: MailboxView::Smart(view.clone()),
                    tab: None,
                    account_id: None,
                    account_ids: None,
                    limit: 1000,
                    before: None,
                    unread_only: true,
                    split: None,
                };
                self.list_smart(view, scope, &q, now.ms, now.off)?.len()
            } else {
                self.smart_items(view, scope, now)?
                    .iter()
                    .filter(|i| i.active)
                    .map(|i| i.thread)
                    .collect::<HashSet<_>>()
                    .len()
            };
            out.push(SmartCount {
                view: view.clone(),
                count: count as u32,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "store_smart_tests.rs"]
mod tests;
