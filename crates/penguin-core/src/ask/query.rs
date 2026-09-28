//! A question as a structured query over what Ask knows (docs/ASK.md,
//! "Questions as queries"). The grammar in `qparse.rs` turns a question into
//! an [`AskQuery`]; the on-device model can fill the same schema when the
//! grammar can't read a phrasing, and the user can edit it. Whoever wrote
//! it, the query is validated here and answered exactly from local data by
//! `query_exec.rs`. Nothing about the answer is generated.
//!
//! Mirrored in `apps/desktop/src/lib/types.ts` (AskQuery and friends).

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use super::intent;
use super::AskTotal;
use crate::dates::{self, DateRange};

/// What the question is about.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum QuerySubject {
    Flights,
    Stays,
    Orders,
    Parcels,
    Bills,
    Bookings,
    /// Money paid: receipts and orders, booking totals, bills marked paid.
    Spending,
    Messages,
}

/// What to compute over the matching things.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum QueryOp {
    Count,
    Sum,
    Average,
    Max,
    Min,
    List,
    First,
    Last,
    Next,
    Exists,
}

/// What is counted, summed or compared.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "camelCase")]
pub enum QueryMeasure {
    /// Each flight, stay, order… once.
    #[default]
    Items,
    /// Amounts of money, one total per currency (never converted).
    Money,
    /// Hotel nights.
    Nights,
    /// Bookings rather than flights: a round trip is one trip.
    Trips,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum QueryGroup {
    Month,
    Year,
    Merchant,
    Place,
    Person,
}

/// Messages from them or to them; flights to or from the place.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum QueryDirection {
    From,
    To,
}

/// The one detail a lookup asks for.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum QueryField {
    Date,
    Confirmation,
    FlightNumber,
    Tracking,
    OrderNumber,
    Amount,
    Address,
}

/// Which side of today the question means when it gives no dates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "camelCase")]
pub enum QueryTense {
    #[default]
    Any,
    Past,
    Future,
}

/// Who turned the question into the query.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "camelCase")]
pub enum QuerySource {
    /// Penguin's grammar.
    #[default]
    Grammar,
    /// The on-device model (Apple Intelligence), validated before use.
    Model,
    /// Edited by the user in the answer's chips.
    Edited,
}

/// A question as a query: subject, operation, filters and grouping.
/// Timeframes are kept as written ("august", "last summer", "2025",
/// "2026-08-01 to 2026-08-31") and read by Penguin's date grammar, so a
/// model or the user can't make up a range the grammar wouldn't show.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskQuery {
    pub subject: QuerySubject,
    pub op: QueryOp,
    #[serde(default)]
    pub measure: QueryMeasure,
    #[serde(default)]
    pub group_by: Option<QueryGroup>,
    #[serde(default)]
    pub timeframe: Option<String>,
    /// Two or more timeframes, or names, compared ("july", "august").
    #[serde(default)]
    pub compare: Vec<String>,
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub merchant: Option<String>,
    #[serde(default)]
    pub person: Option<String>,
    #[serde(default)]
    pub direction: Option<QueryDirection>,
    #[serde(default)]
    pub field: Option<QueryField>,
    #[serde(default)]
    pub tense: QueryTense,
}

impl AskQuery {
    pub fn new(subject: QuerySubject, op: QueryOp) -> AskQuery {
        AskQuery {
            subject,
            op,
            measure: QueryMeasure::Items,
            group_by: None,
            timeframe: None,
            compare: vec![],
            place: None,
            merchant: None,
            person: None,
            direction: None,
            field: None,
            tense: QueryTense::Any,
        }
    }
}

/// How the answer read the question, for the chips above it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskUnderstood {
    pub query: AskQuery,
    pub source: QuerySource,
    /// "flights · August 2026 · count".
    pub summary: String,
    /// The timeframe as read: "Aug 1 – Aug 31, 2026".
    pub range_label: Option<String>,
    /// Each compared side as read.
    pub compare_labels: Vec<String>,
}

/// One row of a grouped count or sum, or one side of a comparison.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskGroup {
    /// "August 2026", "Lisbon", "Swiftcab".
    pub label: String,
    pub count: usize,
    /// Money in this group, per currency.
    pub totals: Vec<AskTotal>,
    /// Hotel nights in this group, when nights are measured.
    pub nights: Option<u32>,
    /// The emails behind it.
    pub cites: Vec<super::AskCite>,
    /// Local midnight (unix ms) when the group is a period.
    pub start: Option<i64>,
    /// The answer to "which … the most" (or least).
    pub best: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AskResultKind {
    Count,
    Sum,
    Average,
    List,
    Groups,
    Compare,
    Exists,
    Item,
}

/// The answer's value, machine-readable (the headline says the same).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskResult {
    pub kind: AskResultKind,
    pub count: Option<u64>,
    /// Money, per currency.
    pub totals: Vec<AskTotal>,
    /// Nights, or an average count.
    pub value: Option<f64>,
    pub yes: Option<bool>,
    /// The group or side that answers "which" / "more".
    pub winner: Option<String>,
    /// The date of a single item's answer ("2026-10-02").
    pub date: Option<String>,
    /// The detail asked for (a confirmation code, a tracking number).
    pub text: Option<String>,
}

impl AskResult {
    pub(crate) fn new(kind: AskResultKind) -> AskResult {
        AskResult {
            kind,
            count: None,
            totals: vec![],
            value: None,
            yes: None,
            winner: None,
            date: None,
            text: None,
        }
    }
}

// ------------------------------------------------------------- timeframes

/// A timeframe as read, with its label.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Timeframe {
    pub range: DateRange,
    pub label: String,
}

/// Read a timeframe with Penguin's date grammar plus quarters ("q2",
/// "second quarter of 2025"), "next month/week/year" and "year to date".
/// A future-tense question moves a month that has passed to next year
/// ("flights in march" asked in September means next March).
pub(crate) fn read_timeframe(text: &str, today: NaiveDate, tense: QueryTense) -> Option<Timeframe> {
    let words = intent::normalize(text);
    let mut w: Vec<&str> = words.iter().map(String::as_str).collect();
    while w.len() > 1 && matches!(w[0], "in" | "during" | "for" | "over" | "within" | "the" | "of" | "en" | "durante" | "on") {
        // "in 2025" and "in march" are read by the grammar with "in".
        if w[0] == "in" && dates::parse(&w, today).is_some_and(|(n, _)| n == w.len()) {
            break;
        }
        w.remove(0);
    }
    if w.is_empty() {
        return None;
    }
    // "bills from last month" means last month, not "since" it; "from
    // March to May" stays a span.
    if w.len() > 1
        && w[0] == "from"
        && !w.iter().any(|x| matches!(*x, "to" | "until" | "till" | "through" | "thru" | "-"))
    {
        let rest = &w[1..];
        let closed = special(rest, today)
            .or_else(|| {
                let (n, r) = dates::parse(rest, today)?;
                (n == rest.len()).then_some(r)
            })
            .is_some_and(|r| r.from.is_some() && r.to.is_some());
        if closed {
            w.remove(0);
        }
    }
    let range = special(&w, today).or_else(|| {
        let (n, r) = dates::parse(&w, today)?;
        (n == w.len()).then_some(r)
    })?;
    let range = if tense == QueryTense::Future {
        forward(range, &w, today)
    } else {
        range
    };
    Some(Timeframe {
        label: label_of(range),
        range,
    })
}

fn quarter_word(w: &str) -> Option<u32> {
    Some(match w {
        "q1" | "first" | "1st" => 1,
        "q2" | "second" | "2nd" => 2,
        "q3" | "third" | "3rd" => 3,
        "q4" | "fourth" | "4th" | "last-quarter" => 4,
        _ => return None,
    })
}

fn special(w: &[&str], today: NaiveDate) -> Option<DateRange> {
    let ymd = |y: i32, m: u32| dates::ymd(y, m, 1);
    let quarter = |q: u32, y: i32| -> Option<DateRange> {
        let from = ymd(y, (q - 1) * 3 + 1)?;
        Some(DateRange {
            from: Some(from),
            to: dates::add_months(from, 3),
        })
    };
    // The most recent quarter q that has started.
    let recent_q = |q: u32| {
        let cur = (today.month() - 1) / 3 + 1;
        if q <= cur {
            today.year()
        } else {
            today.year() - 1
        }
    };
    let year_at = |i: usize| w.get(i).and_then(|x| dates::year_num(x));
    match w {
        ["this", "quarter"] => quarter((today.month() - 1) / 3 + 1, today.year()),
        ["last", "quarter"] => {
            let cur = (today.month() - 1) / 3 + 1;
            if cur == 1 {
                quarter(4, today.year() - 1)
            } else {
                quarter(cur - 1, today.year())
            }
        }
        // "q2", "q2 2025"
        [q] | [q, _] if q.starts_with('q') && quarter_word(q).is_some() => {
            let n = quarter_word(q)?;
            let y = if w.len() == 2 { year_at(1)? } else { recent_q(n) };
            quarter(n, y)
        }
        // "second quarter", "the second quarter of 2025"
        [q, "quarter"] | [q, "quarter", _] | [q, "quarter", "of", _] if quarter_word(q).is_some() => {
            let n = quarter_word(q)?;
            let y = match w.len() {
                2 => recent_q(n),
                3 => year_at(2)?,
                _ => year_at(3)?,
            };
            quarter(n, y)
        }
        ["next", unit] | ["the", "next", unit] => {
            let first = dates::first_of_month(today);
            match *unit {
                "week" => {
                    let m = dates::monday(today) + Duration::days(7);
                    Some(DateRange {
                        from: Some(m),
                        to: Some(m + Duration::days(7)),
                    })
                }
                "month" => {
                    let s = dates::add_months(first, 1)?;
                    Some(DateRange {
                        from: Some(s),
                        to: dates::add_months(s, 1),
                    })
                }
                "year" => Some(DateRange {
                    from: dates::ymd(today.year() + 1, 1, 1),
                    to: dates::ymd(today.year() + 2, 1, 1),
                }),
                _ => None,
            }
        }
        // "the next 3 months", "next 30 days": from today on.
        ["next", n, unit] => {
            let n: i64 = n.parse().ok().filter(|n| (1..=1000).contains(n))?;
            let days = match *unit {
                "day" | "days" => n,
                "week" | "weeks" => 7 * n,
                "month" | "months" => 30 * n,
                "year" | "years" => 365 * n,
                _ => return None,
            };
            Some(DateRange {
                from: Some(today),
                to: Some(today + Duration::days(days + 1)),
            })
        }
        ["year", "to", "date"] | ["ytd"] => Some(DateRange {
            from: dates::ymd(today.year(), 1, 1),
            to: Some(today + Duration::days(1)),
        }),
        // "this past summer" = "last summer".
        ["this", "past", rest @ ..] => {
            let mut v = vec!["last"];
            v.extend_from_slice(rest);
            let (n, r) = dates::parse(&v, today)?;
            (n == v.len()).then_some(r)
        }
        _ => None,
    }
}

/// A future-tense question about a month (or a month and day) that has
/// already passed this year means next year's.
fn forward(r: DateRange, w: &[&str], today: NaiveDate) -> DateRange {
    let explicit_year = w.iter().any(|x| dates::year_num(x).is_some());
    let relative = w
        .iter()
        .any(|x| matches!(*x, "last" | "past" | "ago" | "this" | "yesterday" | "today"));
    match (r.from, r.to) {
        (Some(f), Some(t)) if !explicit_year && !relative && t <= today => DateRange {
            from: f.checked_add_months(chrono::Months::new(12)),
            to: t.checked_add_months(chrono::Months::new(12)),
        },
        _ => r,
    }
}

/// "August 2026", "2025", "Q2 2026", or "Jun 1 – Aug 31, 2025".
pub(crate) fn label_of(r: DateRange) -> String {
    match (r.from, r.to) {
        (Some(f), Some(t)) => {
            let last = t - Duration::days(1);
            if f.day() == 1 && f.month() == 1 && t == dates::ymd(f.year() + 1, 1, 1).unwrap_or(t) {
                return f.year().to_string();
            }
            if f.day() == 1 && Some(t) == dates::add_months(f, 1) {
                return f.format("%B %Y").to_string();
            }
            if f.day() == 1 && (f.month() - 1) % 3 == 0 && Some(t) == dates::add_months(f, 3) {
                return format!("Q{} {}", (f.month() - 1) / 3 + 1, f.year());
            }
            if f == last {
                return f.format("%b %-d, %Y").to_string();
            }
            if f.year() == last.year() {
                format!("{} – {}", f.format("%b %-d"), last.format("%b %-d, %Y"))
            } else {
                format!("{} – {}", f.format("%b %-d, %Y"), last.format("%b %-d, %Y"))
            }
        }
        (Some(f), None) => format!("since {}", f.format("%b %-d, %Y")),
        (None, Some(t)) => format!("before {}", t.format("%b %-d, %Y")),
        (None, None) => "any time".into(),
    }
}

// ------------------------------------------------------------ the model

/// The model's output: every field a string, empty when not stated, the
/// shape `@Generable struct MailQuery` in the Swift bridge generates.
/// [`QueryDraft::into_query`] checks each one before anything runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct QueryDraft {
    pub subject: String,
    pub op: String,
    pub measure: String,
    pub group_by: String,
    pub timeframe: String,
    pub compare: Vec<String>,
    pub place: String,
    pub merchant: String,
    pub person: String,
    pub direction: String,
    pub field: String,
    pub tense: String,
}

/// The allowed values of each enum field, as the model sees them.
pub const DRAFT_SUBJECTS: &[&str] = &[
    "flights", "stays", "orders", "parcels", "bills", "bookings", "spending", "messages",
];
pub const DRAFT_OPS: &[&str] = &[
    "count", "sum", "average", "max", "min", "list", "first", "last", "next", "exists",
];
pub const DRAFT_MEASURES: &[&str] = &["items", "money", "nights", "trips"];
pub const DRAFT_GROUPS: &[&str] = &["none", "month", "year", "merchant", "place", "person"];
pub const DRAFT_DIRECTIONS: &[&str] = &["any", "from", "to"];
pub const DRAFT_TENSES: &[&str] = &["any", "past", "future"];
pub const DRAFT_FIELDS: &[&str] = &[
    "none", "date", "confirmation", "flightNumber", "tracking", "orderNumber", "amount", "address",
];

fn enum_of<T: for<'de> Deserialize<'de>>(field: &str, v: &str, allowed: &[&str]) -> Result<T, String> {
    let t = v.trim();
    let v = allowed
        .iter()
        .find(|a| a.eq_ignore_ascii_case(t))
        .map(|a| a.to_string())
        .unwrap_or_else(|| t.to_lowercase());
    if !allowed.contains(&v.as_str()) {
        return Err(format!("{field} \u{201c}{v}\u{201d} isn't one of {}", allowed.join(", ")));
    }
    serde_json::from_value(serde_json::Value::String(v)).map_err(|e| e.to_string())
}

fn text_of(v: &str) -> Option<String> {
    let t = v.trim();
    let lower = t.to_lowercase();
    (!t.is_empty() && !matches!(lower.as_str(), "none" | "null" | "any" | "n/a" | "all" | "-"))
        .then(|| t.to_string())
}

impl QueryDraft {
    /// Check every field against the schema (enums, lengths) and the date
    /// grammar. Entities (places, merchants, people) are checked against the
    /// mailbox when the query runs; an unknown one fails there.
    pub fn into_query(self, today: NaiveDate) -> Result<AskQuery, String> {
        let subject: QuerySubject = enum_of("subject", &self.subject, DRAFT_SUBJECTS)?;
        let op: QueryOp = enum_of("op", &self.op, DRAFT_OPS)?;
        let measure: QueryMeasure = if self.measure.trim().is_empty() {
            QueryMeasure::Items
        } else {
            enum_of("measure", &self.measure, DRAFT_MEASURES)?
        };
        let group_by = match self.group_by.trim().to_lowercase().as_str() {
            "" | "none" => None,
            g => Some(enum_of::<QueryGroup>("groupBy", g, DRAFT_GROUPS)?),
        };
        let direction = match self.direction.trim().to_lowercase().as_str() {
            "" | "any" => None,
            d => Some(enum_of::<QueryDirection>("direction", d, DRAFT_DIRECTIONS)?),
        };
        let field = match self.field.trim().to_lowercase().as_str() {
            "" | "none" => None,
            _ => Some(enum_of::<QueryField>("field", &self.field, DRAFT_FIELDS)?),
        };
        let tense = match self.tense.trim().to_lowercase().as_str() {
            "" => QueryTense::Any,
            t => enum_of::<QueryTense>("tense", t, DRAFT_TENSES)?,
        };
        let long = |s: &Option<String>| s.as_ref().is_some_and(|x| x.chars().count() > 80);
        let q = AskQuery {
            subject,
            op,
            measure,
            group_by,
            timeframe: text_of(&self.timeframe),
            compare: self.compare.iter().filter_map(|c| text_of(c)).take(4).collect(),
            place: text_of(&self.place),
            merchant: text_of(&self.merchant),
            person: text_of(&self.person),
            direction,
            field,
            tense,
        };
        if long(&q.timeframe) || long(&q.place) || long(&q.merchant) || long(&q.person) {
            return Err("a field is too long".into());
        }
        check(&q, today)?;
        Ok(q)
    }
}

/// What the on-device model is told when it reads a question into a
/// `MailQuery` (the Swift bridge's `@Generable` type, whose fields are
/// [`QueryDraft`]'s). Short and fixed: Apple asks for one to three
/// paragraphs, developer-written, with the person's words kept out of the
/// instructions and in the prompt (docs/ASK.md, "Reading questions with
/// the on-device model"). `today` is "Sun 2026-09-27".
pub fn model_instructions(today: &str) -> String {
    format!(
        "You turn a question about the user's own email into a query. Fill only what the question says; leave other text fields empty and use \"none\", \"any\" or \"items\" for the rest. Never answer the question. Today is {today}.\n\n\
subject: flights (flying, trips), stays (hotels, nights), orders (purchases, receipts, rides), parcels (packages, deliveries), bills (invoices, statements), bookings (reservations, tickets), spending (money paid), messages (emails).\n\
op: count, sum (totals of money), average, max, min, list, first, last, next, exists (yes/no).\n\
measure: money for amounts, nights for hotel nights, trips for round trips, else items.\n\
timeframe: the time words exactly as asked (\"august\", \"last summer\", \"2025\", \"since march\").\n\n\
Examples:\n\
\"how many times did I fly in august\" → subject flights, op count, timeframe \"august\", tense past.\n\
\"what did I spend the most on last month\" → subject spending, op max, measure money, groupBy merchant, timeframe \"last month\".\n\
\"was my power bill higher in July or August\" → subject bills, op sum, measure money, compare [\"july\", \"august\"], merchant \"power\".\n\
\"when did I last go to Lisbon\" → subject flights, op last, place \"Lisbon\", direction to."
    )
}

/// The prompt: the question, fenced so it reads as data (Apple: wrap
/// people's input in your own text in the prompt).
pub fn model_prompt(question: &str) -> String {
    let q: String = question.chars().filter(|c| !c.is_control()).take(500).collect();
    format!("Question from the user, to turn into a query:\n\"\"\"\n{q}\n\"\"\"")
}

/// Most tokens the model may write for a query (the schema is small).
pub const MODEL_RESPONSE_TOKENS: u32 = 160;

/// Checks any query must pass: the combination makes sense and every
/// timeframe reads as dates.
pub(crate) fn check(q: &AskQuery, today: NaiveDate) -> Result<(), String> {
    if let Some(t) = &q.timeframe {
        if read_timeframe(t, today, q.tense).is_none() {
            return Err(format!("\u{201c}{t}\u{201d} isn't a timeframe I can read"));
        }
    }
    if q.compare.len() == 1 {
        return Err("a comparison needs two sides".into());
    }
    match q.measure {
        QueryMeasure::Nights if q.subject != QuerySubject::Stays => {
            return Err("nights are counted for stays only".into())
        }
        QueryMeasure::Trips if q.subject != QuerySubject::Flights => {
            return Err("trips are counted for flights only".into())
        }
        QueryMeasure::Money if matches!(q.subject, QuerySubject::Parcels | QuerySubject::Messages) => {
            return Err("parcels and messages have no amounts".into())
        }
        _ => {}
    }
    if q.subject == QuerySubject::Messages && q.group_by == Some(QueryGroup::Place) {
        return Err("messages have no place".into());
    }
    if q.person.is_some() && q.subject != QuerySubject::Messages {
        return Err("a person applies to messages".into());
    }
    Ok(())
}

/// "flights · to Lisbon · August 2026 · count".
pub(crate) fn summary(q: &AskQuery, range_label: Option<&str>, compare: &[String]) -> String {
    let name = |v: &dyn erased::Named| v.name();
    let mut parts: Vec<String> = vec![name(&q.subject).to_string()];
    if let Some(p) = &q.place {
        parts.push(match q.direction {
            Some(QueryDirection::From) => format!("from {p}"),
            Some(QueryDirection::To) => format!("to {p}"),
            None => format!("in {p}"),
        });
    }
    if let Some(m) = &q.merchant {
        parts.push(format!("at {m}"));
    }
    if let Some(p) = &q.person {
        parts.push(match q.direction {
            Some(QueryDirection::From) => format!("from {p}"),
            Some(QueryDirection::To) => format!("to {p}"),
            None => format!("with {p}"),
        });
    }
    if let Some(r) = range_label {
        parts.push(r.to_string());
    }
    if !compare.is_empty() {
        parts.push(compare.join(" vs "));
    }
    let op = match (q.op, q.measure) {
        (QueryOp::Count, QueryMeasure::Nights) => "nights".to_string(),
        (QueryOp::Count, QueryMeasure::Trips) => "count trips".to_string(),
        (QueryOp::Sum, _) | (QueryOp::Count, QueryMeasure::Money) => "total".to_string(),
        (op, QueryMeasure::Money) if !matches!(op, QueryOp::Sum) => format!("{} amount", name(&op)),
        (op, _) => name(&op).to_string(),
    };
    parts.push(op);
    if let Some(g) = q.group_by {
        parts.push(format!("by {}", name(&g)));
    }
    parts.join(" \u{b7} ")
}

/// Lowercase names of the enums, as serialized.
mod erased {
    use serde::Serialize;

    pub trait Named {
        fn name(&self) -> String;
    }

    impl<T: Serialize> Named for T {
        fn name(&self) -> String {
            match serde_json::to_value(self) {
                Ok(serde_json::Value::String(s)) => s,
                _ => String::new(),
            }
        }
    }
}

pub(crate) use erased::Named;

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 27).unwrap()
    }

    fn tf(s: &str) -> Option<String> {
        read_timeframe(s, today(), QueryTense::Any).map(|t| t.label)
    }

    #[test]
    fn timeframes() {
        assert_eq!(tf("august").as_deref(), Some("August 2026"));
        assert_eq!(tf("in august").as_deref(), Some("August 2026"));
        assert_eq!(tf("october").as_deref(), Some("October 2025"));
        assert_eq!(tf("2025").as_deref(), Some("2025"));
        assert_eq!(tf("in 2025").as_deref(), Some("2025"));
        assert_eq!(tf("q2").as_deref(), Some("Q2 2026"));
        assert_eq!(tf("q4").as_deref(), Some("Q4 2025"));
        assert_eq!(tf("the second quarter of 2025").as_deref(), Some("Q2 2025"));
        assert_eq!(tf("last summer").as_deref(), Some("Jun 1 – Aug 31, 2026"));
        assert_eq!(tf("this year").as_deref(), Some("2026"));
        assert_eq!(tf("en agosto").as_deref(), Some("August 2026"));
        assert_eq!(tf("el año pasado").as_deref(), Some("2025"));
        assert_eq!(tf("next month").as_deref(), Some("October 2026"));
        assert_eq!(tf("purple"), None);
        let fut = read_timeframe("march", today(), QueryTense::Future).unwrap();
        assert_eq!(fut.label, "March 2027");
    }

    #[test]
    fn drafts_are_checked() {
        let ok = QueryDraft {
            subject: "flights".into(),
            op: "count".into(),
            timeframe: "august".into(),
            ..Default::default()
        };
        let q = ok.clone().into_query(today()).unwrap();
        assert_eq!(q.subject, QuerySubject::Flights);
        assert_eq!(q.timeframe.as_deref(), Some("august"));
        assert!(QueryDraft {
            subject: "vacations".into(),
            ..ok.clone()
        }
        .into_query(today())
        .is_err());
        assert!(QueryDraft {
            timeframe: "whenever I felt like it".into(),
            ..ok.clone()
        }
        .into_query(today())
        .is_err());
        assert!(QueryDraft {
            measure: "nights".into(),
            ..ok
        }
        .into_query(today())
        .is_err());
    }

    #[test]
    fn summaries() {
        let mut q = AskQuery::new(QuerySubject::Flights, QueryOp::Count);
        q.place = Some("Lisbon".into());
        q.direction = Some(QueryDirection::To);
        assert_eq!(
            summary(&q, Some("August 2026"), &[]),
            "flights \u{b7} to Lisbon \u{b7} August 2026 \u{b7} count"
        );
    }
}
