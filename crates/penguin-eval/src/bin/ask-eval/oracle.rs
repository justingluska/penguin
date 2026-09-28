//! Ground truth for the Ask question set, computed from the synthetic
//! corpus itself (its tags and the generated text), never from Penguin's
//! extractors or its Ask code. Each question carries a gold query in a
//! small DSL (`questions.rs`); `expect` turns it into the exact answer and
//! the conversations that support it.
//!
//! What counts, stated once (docs/ASK-QUESTIONS.md, "Definitions"):
//! - A round trip is two flights: SFO → city on the departure date and
//!   city → SFO on the return date. "Flights to X" are the ones landing in X.
//! - A trip is one booking.
//! - Orders are receipts and order confirmations. A refund isn't an order;
//!   in spending it counts negative. "Pay at pickup" isn't paid yet.
//! - Spending = receipts, subscription charges ("Amount charged"), hotel
//!   booking totals, refunds negative. Bills that are only due (autopay
//!   notices) are billed, not spent. Money is never converted.
//! - Bills are statements and invoices received with an amount due.
//! - People are the humans in the cast, not automated senders.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use penguin_eval::corpus::world::{CITIES, CLIENTS, COLLEAGUES, FRIENDS, PARTNERS};
use penguin_eval::corpus::{doc_id, Corpus};

pub fn local_day(ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(ms).single().unwrap().date_naive()
}

fn tag<'a>(tags: &'a BTreeSet<String>, key: &str) -> Option<&'a str> {
    let p = format!("{key}:");
    tags.iter().find_map(|t| t.strip_prefix(&p))
}

fn amount(s: &str) -> Option<f64> {
    s.replace(',', "").parse().ok()
}

/// "Fri, Oct 2" / "Friday, October 2" on or after `from`.
fn day_after(text: &str, from: NaiveDate) -> Option<NaiveDate> {
    let t = text.trim().trim_end_matches(['.', ',']);
    let parts: Vec<&str> = t.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
    // [weekday, month, day]
    let (m, d) = match parts.as_slice() {
        [_, m, d, ..] => (*m, *d),
        _ => return None,
    };
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|x| m.to_lowercase().starts_with(x))? as u32
        + 1;
    let day: u32 = d.parse().ok()?;
    for y in from.year()..=from.year() + 1 {
        if let Some(date) = NaiveDate::from_ymd_opt(y, month, day) {
            if date >= from {
                return Some(date);
            }
        }
    }
    None
}

/// The text after `label` up to the end of the line.
fn after<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    let i = text.find(label)? + label.len();
    Some(text[i..].lines().next().unwrap_or("").trim())
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub doc: String,
    /// When it happened (departure, check-in) or the email's day.
    pub day: NaiveDate,
    /// The first email's day.
    pub email_day: NaiveDate,
    /// The first email's time, to order things on one day.
    pub ms: i64,
    pub place: Option<String>,
    pub from_place: Option<String>,
    pub country: Option<String>,
    pub merchant: Option<String>,
    pub amount: Option<(f64, &'static str)>,
    pub nights: Option<u32>,
    pub booking: Option<String>,
    /// Confirmation, tracking or order number (uppercase).
    pub reference: Option<String>,
    pub flight_number: Option<String>,
    /// A bill's due date.
    pub due: Option<NaiveDate>,
}

#[derive(Debug, Clone)]
pub struct Mail {
    pub doc: String,
    pub day: NaiveDate,
    pub from: String,
    pub to: Vec<String>,
    pub sent: bool,
}

pub struct Truth {
    pub today: NaiveDate,
    pub flights: Vec<Unit>,
    pub stays: Vec<Unit>,
    pub orders: Vec<Unit>,
    pub parcels: Vec<Unit>,
    pub bills: Vec<Unit>,
    pub bookings: Vec<Unit>,
    /// Money actually paid (see the module docs).
    pub spending: Vec<Unit>,
    pub mail: Vec<Mail>,
    /// People in the cast: key → (name, email).
    pub people: BTreeMap<String, (String, String)>,
}

fn country_of(city: &str) -> Option<String> {
    CITIES.iter().find(|c| c.0 == city).map(|c| c.3.to_lowercase())
}

impl Truth {
    pub fn from_corpus(c: &Corpus) -> Truth {
        let today = local_day(c.now);
        let mut t = Truth {
            today,
            flights: vec![],
            stays: vec![],
            orders: vec![],
            parcels: vec![],
            bills: vec![],
            bookings: vec![],
            spending: vec![],
            mail: vec![],
            people: BTreeMap::new(),
        };
        for p in COLLEAGUES.iter().chain(PARTNERS).chain(FRIENDS).chain(CLIENTS) {
            t.people.insert(p.key.to_string(), (p.name.to_string(), p.email.to_string()));
        }
        let mut by_thread: BTreeMap<(String, String), Vec<&penguin_core::Message>> = BTreeMap::new();
        for m in &c.messages {
            by_thread.entry((m.account_id.clone(), m.thread_id.clone())).or_default().push(m);
            let sent = m.label_ids.iter().any(|l| l == "SENT");
            t.mail.push(Mail {
                doc: doc_id(&m.account_id, &m.thread_id),
                day: local_day(m.date),
                from: m.from.email.to_lowercase(),
                to: m.to.iter().chain(&m.cc).map(|a| a.email.to_lowercase()).collect(),
                sent,
            });
        }
        for th in &c.threads {
            let msgs = &by_thread[&(th.account_id.clone(), th.thread_id.clone())];
            let first = msgs[0];
            let body = &first.body_text;
            let email_day = local_day(th.first);
            let doc = th.doc_id();
            let unit = |day: NaiveDate| Unit {
                doc: doc.clone(),
                day,
                email_day,
                ms: th.first,
                place: None,
                from_place: None,
                country: None,
                merchant: tag(&th.tags, "merchant").map(String::from),
                amount: None,
                nights: None,
                booking: None,
                reference: None,
                flight_number: None,
                due: None,
            };
            match tag(&th.tags, "kind").unwrap_or("") {
                "flight" => {
                    let city = tag(&th.tags, "city").unwrap_or("").to_string();
                    let pnr = tag(&th.tags, "pnr").unwrap_or("").to_uppercase();
                    let airline = tag(&th.tags, "airline").map(String::from);
                    let (dep, ret, num, ret_num) = if th.has("plant:lisbon-flight") {
                        let dep = after(body, "Departs ").and_then(|s| day_after(s.split(" at ").next()?, email_day));
                        let ret = after(body, "Return SK 2211: ").and_then(|s| day_after(s, email_day));
                        (dep, ret, Some("SK2210".to_string()), Some("SK2211".to_string()))
                    } else {
                        let dep = after(body, "Depart: ").or(after(body, "Ida: ")).and_then(|s| day_after(s, email_day));
                        let ret = after(body, "Return: ").or(after(body, "Vuelta: ")).and_then(|s| day_after(s, dep.unwrap_or(email_day)));
                        let num = body
                            .split_whitespace()
                            .skip_while(|w| *w != "Flight" && *w != "Vuelo")
                            .nth(1)
                            .map(|n| n.trim_end_matches(':').to_string());
                        (dep, ret, num, None)
                    };
                    let dep = dep.expect("flight departure in the corpus text");
                    let mut out = unit(dep);
                    out.place = Some(city.clone());
                    out.from_place = Some("san-francisco".into());
                    out.country = country_of(&city);
                    out.merchant = airline.clone();
                    out.booking = Some(pnr.clone());
                    out.reference = Some(pnr.clone());
                    out.flight_number = num;
                    t.flights.push(out.clone());
                    if let Some(r) = ret {
                        let mut back = out.clone();
                        back.day = r;
                        back.place = Some("san-francisco".into());
                        back.from_place = Some(city.clone());
                        back.country = Some("usa".into());
                        back.flight_number = ret_num;
                        t.flights.push(back);
                    }
                }
                "hotel" => {
                    let city = tag(&th.tags, "city").unwrap_or("").to_string();
                    let (checkin, nights) = if th.has("plant:lisbon-stay") || body.contains("Arrive ") {
                        let a = after(body, "Arrive ").unwrap_or("");
                        let day = day_after(a, email_day);
                        let nights = a.split(", ").find_map(|p| p.strip_suffix(" nights")?.trim().parse().ok());
                        (day, nights)
                    } else {
                        let day = after(body, "Check-in: ").and_then(|s| day_after(s, email_day));
                        let nights = body.lines().find_map(|l| l.split(" nights").next().filter(|_| l.contains(" nights"))?.trim().parse().ok());
                        (day, nights)
                    };
                    let mut u = unit(checkin.expect("check-in in the corpus text"));
                    u.place = Some(city.clone());
                    u.country = country_of(&city);
                    u.nights = nights;
                    u.reference = tag(&th.tags, "hotelconf").map(|s| s.to_uppercase());
                    u.amount = after(body, "Total $").and_then(amount).map(|v| (v, "USD"));
                    t.stays.push(u.clone());
                    if let Some(a) = u.amount {
                        let mut s = u.clone();
                        s.day = email_day;
                        s.amount = Some(a);
                        t.spending.push(s);
                    }
                }
                "receipt" => {
                    // Tickets are an event booking; "pay at pickup" isn't paid.
                    if th.has("merchant:ticketeer") {
                        // "Sunday Nov 8, doors 7 pm"
                        let day = after(body, "Hall, ").and_then(|s| day_after(s, email_day)).expect("concert date");
                        let mut u = unit(day);
                        u.reference = tag(&th.tags, "order").map(|s| s.to_uppercase());
                        t.bookings.push(u);
                        continue;
                    }
                    if th.has("merchant:spokeandchain") {
                        continue;
                    }
                    let cur = if th.has("merchant:mercadosol") { "EUR" } else { "USD" };
                    let mut u = unit(email_day);
                    u.reference = tag(&th.tags, "order").map(|s| s.to_uppercase());
                    if th.has("topic:refund") {
                        let v = body.split('$').nth(1).and_then(|s| amount(s.split_whitespace().next()?)).expect("refund amount");
                        u.amount = Some((-v, cur));
                        t.spending.push(u);
                        continue;
                    }
                    u.amount = tag(&th.tags, "amount").and_then(amount).map(|v| (v, cur));
                    t.orders.push(u.clone());
                    t.spending.push(u);
                }
                "shipping" => {
                    let mut u = unit(email_day);
                    u.reference = tag(&th.tags, "tracking").map(|s| s.to_uppercase());
                    t.parcels.push(u);
                }
                "bill" => {
                    let Some(v) = tag(&th.tags, "amount").and_then(amount).or_else(|| {
                        body.split('$').nth(1).and_then(|s| amount(s.split_whitespace().next()?.trim_end_matches('.')))
                    }) else {
                        continue; // a renewal notice without a price
                    };
                    let mut u = unit(email_day);
                    u.amount = Some((v, "USD"));
                    u.due = after(body, "Due date: ").and_then(|s| day_after(&format!("x, {s}"), email_day));
                    // A subscription receipt ("Amount charged") is a charge
                    // already made: a receipt, not a bill to pay.
                    if body.contains("Amount charged") {
                        t.orders.push(u.clone());
                        t.spending.push(u);
                        continue;
                    }
                    t.bills.push(u);
                }
                "invoice" if th.has("direction:in") => {
                    let v = after(body, "Amount: $")
                        .or(body.split("for $").nth(1))
                        .and_then(|s| amount(s.split_whitespace().next()?.trim_end_matches('.')))
                        .expect("invoice amount");
                    let mut u = unit(email_day);
                    u.merchant = Some("crestline".into());
                    u.amount = Some((v, "USD"));
                    u.reference = tag(&th.tags, "invoice").map(|s| s.to_uppercase());
                    t.bills.push(u);
                }
                _ => {}
            }
            if th.has("merchant:lumiere") {
                // "confirmed for Saturday": the next Saturday after the email.
                let mut d = email_day + Duration::days(1);
                while d.weekday() != chrono::Weekday::Sat {
                    d += Duration::days(1);
                }
                t.bookings.push(unit(d));
            }
        }
        t
    }

    pub fn units(&self, s: Subject) -> &[Unit] {
        match s {
            Subject::Flights => &self.flights,
            Subject::Stays => &self.stays,
            Subject::Orders => &self.orders,
            Subject::Parcels => &self.parcels,
            Subject::Bills => &self.bills,
            Subject::Bookings => &self.bookings,
            Subject::Spending => &self.spending,
            Subject::Messages => &[],
        }
    }
}

// ------------------------------------------------------------------ gold

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Flights,
    Stays,
    Orders,
    Parcels,
    Bills,
    Bookings,
    Spending,
    Messages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
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
    /// A topic question: graded by the sources cited.
    Passage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    Items,
    Money,
    Nights,
    Trips,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Month,
    Year,
    Merchant,
    Place,
    Person,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Side {
    Range(NaiveDate, NaiveDate, String),
    Name(String),
}

#[derive(Debug, Clone)]
pub struct Gold {
    pub subject: Subject,
    pub op: Op,
    pub measure: Measure,
    pub group: Option<Group>,
    /// [from, to) local dates.
    pub range: Option<(NaiveDate, NaiveDate)>,
    /// The timeframe as the draft writes it (for the simulated model).
    pub timeframe: Option<String>,
    pub vs: Vec<Side>,
    pub place: Option<String>,
    pub dir: Option<&'static str>,
    pub merchant: Option<String>,
    pub person: Option<String>,
    pub field: Option<String>,
    pub future: Option<bool>,
    pub plant: Option<String>,
}

/// Resolve a symbolic range against `today`: `aug` (the latest August),
/// `2025`, `2025-03`, `thisyear`, `lastyear`, `lastmonth`, `lastsummer`,
/// `lastspring`, `q2`, `since-mar`, `last6m`, `last30d`, `next3m`,
/// `nextmonth`, `oct-next`.
pub fn range(s: &str, today: NaiveDate) -> (NaiveDate, NaiveDate, String) {
    let ymd = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
    let add_m = |d: NaiveDate, n: i32| {
        let t = d.year() * 12 + d.month0() as i32 + n;
        ymd(t.div_euclid(12), t.rem_euclid(12) as u32 + 1, 1)
    };
    let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let month_of = |m: &str| months.iter().position(|x| *x == m).map(|i| i as u32 + 1);
    let fmt = |a: NaiveDate, b: NaiveDate| {
        format!("{} to {}", a.format("%Y-%m-%d"), (b - Duration::days(1)).format("%Y-%m-%d"))
    };
    let (a, b) = match s {
        "thisyear" => (ymd(today.year(), 1, 1), ymd(today.year() + 1, 1, 1)),
        "lastyear" => (ymd(today.year() - 1, 1, 1), ymd(today.year(), 1, 1)),
        "lastmonth" => {
            let f = ymd(today.year(), today.month(), 1);
            (add_m(f, -1), f)
        }
        "thismonth" => {
            let f = ymd(today.year(), today.month(), 1);
            (f, add_m(f, 1))
        }
        "nextmonth" => {
            let f = add_m(ymd(today.year(), today.month(), 1), 1);
            (f, add_m(f, 1))
        }
        "lastsummer" => (ymd(2026, 6, 1), ymd(2026, 9, 1)),
        "lastspring" => (ymd(2026, 3, 1), ymd(2026, 6, 1)),
        "lastwinter" => (ymd(2025, 12, 1), ymd(2026, 3, 1)),
        "last6m" => (today.checked_sub_months(chrono::Months::new(6)).unwrap(), today + Duration::days(1)),
        "last3m" => (today.checked_sub_months(chrono::Months::new(3)).unwrap(), today + Duration::days(1)),
        "last30d" => (today - Duration::days(30), today + Duration::days(1)),
        "lastweek" => {
            let mon = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            (mon - Duration::days(7), mon)
        }
        "next3m" => (today, today + Duration::days(91)),
        "yesterday" => (today - Duration::days(1), today),
        _ if s.starts_with("last") && s.ends_with('d') && s[4..s.len() - 1].parse::<i64>().is_ok() => {
            let n: i64 = s[4..s.len() - 1].parse().unwrap();
            (today - Duration::days(n), today + Duration::days(1))
        }
        _ => {
            if let Some(q) = s.strip_prefix('q') {
                let (qn, y) = match q.split_once('-') {
                    Some((n, y)) => (n.parse::<u32>().unwrap(), y.parse().unwrap()),
                    None => {
                        let n: u32 = q.parse().unwrap();
                        let cur = (today.month() - 1) / 3 + 1;
                        (n, if n <= cur { today.year() } else { today.year() - 1 })
                    }
                };
                let f = ymd(y, (qn - 1) * 3 + 1, 1);
                (f, add_m(f, 3))
            } else if let Some(m) = s.strip_prefix("since-") {
                let m = month_of(m).unwrap();
                let y = if m <= today.month() { today.year() } else { today.year() - 1 };
                (ymd(y, m, 1), today + Duration::days(1))
            } else if let Some(m) = s.strip_suffix("-next") {
                let m = month_of(m).unwrap();
                let y = if m >= today.month() { today.year() } else { today.year() + 1 };
                let f = ymd(y, m, 1);
                (f, add_m(f, 1))
            } else if let Some(m) = month_of(s) {
                let y = if m <= today.month() { today.year() } else { today.year() - 1 };
                let f = ymd(y, m, 1);
                (f, add_m(f, 1))
            } else if s.len() == 4 {
                let y: i32 = s.parse().unwrap();
                (ymd(y, 1, 1), ymd(y + 1, 1, 1))
            } else if s.len() == 7 {
                let (y, m) = s.split_once('-').unwrap();
                let f = ymd(y.parse().unwrap(), m.parse().unwrap(), 1);
                (f, add_m(f, 1))
            } else {
                panic!("unknown range {s}")
            }
        }
    };
    (a, b, fmt(a, b))
}

/// `flights count @aug place=lisbon dir=to`
pub fn parse_gold(dsl: &str, today: NaiveDate) -> Gold {
    let mut it = dsl.split_whitespace();
    let subject = match it.next().unwrap() {
        "flights" => Subject::Flights,
        "stays" => Subject::Stays,
        "orders" => Subject::Orders,
        "parcels" => Subject::Parcels,
        "bills" => Subject::Bills,
        "bookings" => Subject::Bookings,
        "spending" => Subject::Spending,
        "messages" => Subject::Messages,
        "passage" => Subject::Messages,
        s => panic!("subject {s}"),
    };
    let passage = dsl.starts_with("passage");
    let op = if passage {
        Op::Passage
    } else {
        match it.next().unwrap() {
            "count" => Op::Count,
            "sum" => Op::Sum,
            "average" => Op::Average,
            "max" => Op::Max,
            "min" => Op::Min,
            "list" => Op::List,
            "first" => Op::First,
            "last" => Op::Last,
            "next" => Op::Next,
            "exists" => Op::Exists,
            o => panic!("op {o}"),
        }
    };
    let mut g = Gold {
        subject,
        op,
        measure: match op {
            Op::Sum => Measure::Money,
            _ => Measure::Items,
        },
        group: None,
        range: None,
        timeframe: None,
        vs: vec![],
        place: None,
        dir: None,
        merchant: None,
        person: None,
        field: None,
        future: None,
        plant: None,
    };
    for w in it {
        if let Some(r) = w.strip_prefix('@') {
            let (a, b, text) = range(r, today);
            g.range = Some((a, b));
            g.timeframe = Some(text);
            continue;
        }
        let (k, v) = w.split_once('=').unwrap_or((w, ""));
        let v = v.replace('_', " ");
        match k {
            "money" => g.measure = Measure::Money,
            "nights" => g.measure = Measure::Nights,
            "trips" => g.measure = Measure::Trips,
            "by" => {
                g.group = Some(match v.as_str() {
                    "month" => Group::Month,
                    "year" => Group::Year,
                    "merchant" => Group::Merchant,
                    "place" => Group::Place,
                    "person" => Group::Person,
                    x => panic!("group {x}"),
                })
            }
            "vs" => {
                for side in v.split('|') {
                    g.vs.push(match side.strip_prefix('@') {
                        Some(r) => {
                            let (a, b, t) = range(r, today);
                            Side::Range(a, b, t)
                        }
                        None => Side::Name(side.to_string()),
                    });
                }
            }
            "place" => g.place = Some(v),
            "dir" => g.dir = Some(if v == "to" { "to" } else { "from" }),
            "merchant" => g.merchant = Some(v),
            "person" => g.person = Some(v),
            "field" => g.field = Some(v),
            "future" => g.future = Some(true),
            "past" => g.future = Some(false),
            "plant" => g.plant = Some(v),
            x => panic!("gold key {x} in {dsl}"),
        }
    }
    g
}

// -------------------------------------------------------------- answers

#[derive(Debug, Clone, PartialEq)]
pub enum Expect {
    Count(u64),
    /// Per currency.
    Money(Vec<(String, f64)>),
    /// An average count or nights.
    Value(f64),
    /// One of these labels (ties).
    Winner(Vec<String>),
    /// Group rows: (label, value).
    Groups(Vec<(String, f64)>),
    /// Exactly these conversations.
    Set(BTreeSet<String>),
    Date(NaiveDate),
    Text(String),
    Yes(bool),
    /// A topic question: any of these conversations cited near the top.
    Sources(BTreeSet<String>),
}

#[derive(Debug, Clone)]
pub struct Expected {
    pub value: Expect,
    /// The conversations that support the answer (graded sources).
    pub docs: BTreeSet<String>,
}

fn place_ok(u: &Unit, g: &Gold, name: &str) -> bool {
    let name = name.to_lowercase().replace(' ', "-");
    let hit = |p: &Option<String>| p.as_deref() == Some(name.as_str());
    let country = u.country.as_deref() == Some(name.as_str());
    match (g.subject, g.dir) {
        (Subject::Flights, Some("from")) => hit(&u.from_place),
        (Subject::Flights, _) => hit(&u.place) || country,
        _ => hit(&u.place) || country,
    }
}

fn in_range(d: NaiveDate, r: Option<(NaiveDate, NaiveDate)>) -> bool {
    r.is_none_or(|(a, b)| d >= a && d < b)
}

fn money_of(us: &[&Unit]) -> Vec<(String, f64)> {
    let mut m: BTreeMap<String, f64> = BTreeMap::new();
    for u in us {
        if let Some((v, c)) = u.amount {
            *m.entry(c.to_string()).or_default() += v;
        }
    }
    m.into_iter().map(|(c, v)| (c, (v * 100.0).round() / 100.0)).collect()
}

fn main_value(us: &[&Unit], g: &Gold) -> f64 {
    match g.measure {
        Measure::Money => us.iter().filter_map(|u| u.amount).filter(|a| a.1 == "USD").map(|a| a.0).sum(),
        Measure::Nights => us.iter().filter_map(|u| u.nights).sum::<u32>() as f64,
        Measure::Trips => us.iter().map(|u| u.booking.clone().unwrap_or(u.doc.clone())).collect::<BTreeSet<_>>().len() as f64,
        Measure::Items => us.len() as f64,
    }
}

fn label_month(d: NaiveDate) -> String {
    d.format("%B %Y").to_string()
}

/// The exact answer to `g`, and the conversations behind it.
pub fn expect(t: &Truth, g: &Gold) -> Expected {
    if g.op == Op::Passage {
        let doc = t
            .mail
            .iter()
            .find(|m| m.doc.ends_with(&format!("/p-{}", g.plant.clone().unwrap())))
            .map(|m| m.doc.clone())
            .expect("planted thread");
        let s: BTreeSet<String> = [doc].into();
        return Expected { value: Expect::Sources(s.clone()), docs: s };
    }
    if g.subject == Subject::Messages {
        return expect_mail(t, g);
    }
    let event = matches!(g.subject, Subject::Flights | Subject::Stays | Subject::Bookings);
    // Money is spent when the email says so, not on the day of the trip.
    let day_of = |u: &Unit| if g.measure == Measure::Money { u.email_day } else { u.day };
    let base: Vec<&Unit> = t
        .units(g.subject)
        .iter()
        .filter(|u| g.merchant.as_deref().is_none_or(|m| u.merchant.as_deref() == Some(m)))
        .filter(|u| g.place.as_deref().is_none_or(|p| place_ok(u, g, p)))
        .filter(|u| g.measure != Measure::Money || u.amount.is_some())
        .filter(|u| g.measure != Measure::Nights || u.nights.is_some())
        .collect();
    let tense_ok = |u: &&Unit| match (g.op, g.future) {
        (Op::Next, _) => u.day >= t.today,
        (Op::Last, _) if event => u.day < t.today,
        (_, Some(true)) if event => u.day >= t.today,
        (_, Some(false)) if event => day_of(u) < t.today,
        _ => true,
    };
    let chosen: Vec<&Unit> = base.iter().copied().filter(|u| in_range(day_of(u), g.range)).filter(tense_ok).collect();
    let docs = |us: &[&Unit]| us.iter().map(|u| u.doc.clone()).collect::<BTreeSet<_>>();
    if !g.vs.is_empty() {
        let mut vals: Vec<(String, f64, Vec<&Unit>)> = Vec::new();
        for s in &g.vs {
            let (label, us): (String, Vec<&Unit>) = match s {
                Side::Range(a, b, _) => {
                    let us: Vec<&Unit> = base.iter().copied().filter(|u| in_range(day_of(u), Some((*a, *b)))).collect();
                    (range_label(*a, *b), us)
                }
                Side::Name(n) => {
                    let us: Vec<&Unit> = chosen
                        .iter()
                        .copied()
                        .filter(|u| place_ok(u, g, n) || u.merchant.as_deref() == Some(n.as_str()))
                        .collect();
                    (n.clone(), us)
                }
            };
            let v = main_value(&us, g);
            vals.push((label, v, us));
        }
        let best = vals.iter().map(|v| v.1).fold(f64::MIN, f64::max);
        let winners: Vec<String> = vals.iter().filter(|v| (v.1 - best).abs() < 0.005).map(|v| v.0.clone()).collect();
        let all: Vec<&Unit> = vals.iter().flat_map(|v| v.2.iter().copied()).collect();
        let w = if winners.len() > 1 { vec!["the same".to_string()] } else { winners };
        return Expected { value: Expect::Winner(w), docs: docs(&all) };
    }
    if let Some(gr) = g.group {
        let mut rows: BTreeMap<String, Vec<&Unit>> = BTreeMap::new();
        for u in &chosen {
            let key = match gr {
                Group::Month => label_month(NaiveDate::from_ymd_opt(day_of(u).year(), day_of(u).month(), 1).unwrap()),
                Group::Year => day_of(u).year().to_string(),
                Group::Merchant => u.merchant.clone().unwrap_or_default(),
                // Where you fly is away from home: returns aren't destinations.
                Group::Place => match u.place.clone() {
                    Some(p) if p != "san-francisco" => p,
                    _ => continue,
                },
                Group::Person => continue,
            };
            rows.entry(key).or_default().push(u);
        }
        let vals: Vec<(String, f64)> = rows.iter().map(|(k, v)| (k.clone(), main_value(v, g))).collect();
        let value = match g.op {
            Op::Max | Op::Min => {
                let pick = if g.op == Op::Max {
                    vals.iter().map(|v| v.1).fold(f64::MIN, f64::max)
                } else {
                    vals.iter().map(|v| v.1).fold(f64::MAX, f64::min)
                };
                Expect::Winner(vals.iter().filter(|v| (v.1 - pick).abs() < 0.005).map(|v| v.0.clone()).collect())
            }
            Op::Average => {
                let (a, b) = g.range.expect("average per period needs a range");
                let last = (b - Duration::days(1)).min(t.today);
                let n = match gr {
                    Group::Month => (last.year() - a.year()) * 12 + last.month() as i32 - a.month() as i32 + 1,
                    _ => last.year() - a.year() + 1,
                };
                Expect::Value(vals.iter().map(|v| v.1).sum::<f64>() / n.max(1) as f64)
            }
            _ => Expect::Groups(vals),
        };
        return Expected { value, docs: docs(&chosen) };
    }
    let value = match g.op {
        Op::List => Expect::Set(docs(&chosen)),
        Op::Count => Expect::Count(main_value(&chosen, g) as u64),
        Op::Sum => Expect::Money(money_of(&chosen)),
        Op::Average => match g.measure {
            Measure::Money => {
                let usd: Vec<f64> = chosen.iter().filter_map(|u| u.amount).filter(|a| a.1 == "USD").map(|a| a.0).collect();
                Expect::Money(vec![("USD".into(), usd.iter().sum::<f64>() / usd.len().max(1) as f64)])
            }
            _ => Expect::Value(main_value(&chosen, g) / chosen.len().max(1) as f64),
        },
        Op::Max | Op::Min => {
            let val = |u: &Unit| match g.measure {
                Measure::Nights => u.nights.unwrap_or(0) as f64,
                _ => u.amount.filter(|a| a.1 == "USD").map(|a| a.0).unwrap_or(f64::NAN),
            };
            let pool: Vec<&Unit> = chosen.iter().copied().filter(|u| !val(u).is_nan()).collect();
            let v = if g.op == Op::Max {
                pool.iter().map(|u| val(u)).fold(f64::MIN, f64::max)
            } else {
                pool.iter().map(|u| val(u)).fold(f64::MAX, f64::min)
            };
            match g.measure {
                Measure::Nights => Expect::Value(v),
                _ => Expect::Money(vec![("USD".into(), (v * 100.0).round() / 100.0)]),
            }
        }
        Op::First | Op::Last | Op::Next => {
            let mut s = chosen.clone();
            s.sort_by_key(|u| (u.day, u.ms));
            let u = match g.op {
                Op::First | Op::Next => s.first(),
                _ => s.last(),
            };
            match (u, g.field.as_deref()) {
                (None, _) => Expect::Yes(false),
                (Some(u), Some("confirmation" | "tracking" | "ordernumber")) => Expect::Text(u.reference.clone().unwrap_or_default()),
                (Some(u), Some("flightnumber")) => Expect::Text(u.flight_number.clone().unwrap_or_default()),
                (Some(u), Some("due")) => Expect::Date(u.due.unwrap_or(u.day)),
                (Some(u), Some("amount")) => Expect::Money(vec![(u.amount.unwrap().1.to_string(), u.amount.unwrap().0)]),
                (Some(u), _) => Expect::Date(u.day),
            }
        }
        Op::Exists => Expect::Yes(!chosen.is_empty()),
        Op::Passage => unreachable!(),
    };
    Expected { value, docs: docs(&chosen) }
}

pub fn range_label(a: NaiveDate, b: NaiveDate) -> String {
    if a.day() == 1 && (a.month() - 1) % 3 == 0 && b == a.checked_add_months(chrono::Months::new(3)).unwrap() {
        format!("Q{} {}", (a.month() - 1) / 3 + 1, a.year())
    } else if a.day() == 1 && b == a.checked_add_months(chrono::Months::new(1)).unwrap() {
        label_month(a)
    } else if a.day() == 1 && a.month() == 1 && b == NaiveDate::from_ymd_opt(a.year() + 1, 1, 1).unwrap() {
        a.year().to_string()
    } else {
        format!("{} – {}", a.format("%b %-d, %Y"), (b - Duration::days(1)).format("%b %-d, %Y"))
    }
}

fn expect_mail(t: &Truth, g: &Gold) -> Expected {
    let person = g.person.as_ref().map(|p| t.people[p].1.to_lowercase());
    let own = |e: &str| e.ends_with("@northwind.example") && e.starts_with("alex.moreno")
        || e == "alexmoreno@mailbox.example"
        || e == "hello@morenostudio.example";
    let mails: Vec<&Mail> = t
        .mail
        .iter()
        .filter(|m| match (&person, g.dir) {
            (Some(p), Some("from")) => !m.sent && &m.from == p,
            (Some(p), Some("to")) => m.sent && m.to.contains(p),
            (Some(p), _) => (!m.sent && &m.from == p) || (m.sent && m.to.contains(p)),
            (None, Some("from")) => !m.sent && !own(&m.from),
            (None, Some("to")) => m.sent,
            (None, _) => true,
        })
        .collect();
    let base = mails.clone();
    let chosen: Vec<&Mail> = mails.into_iter().filter(|m| in_range(m.day, g.range)).collect();
    let docs: BTreeSet<String> = chosen.iter().map(|m| m.doc.clone()).collect();
    if !g.vs.is_empty() {
        let vals: Vec<(String, usize)> = g
            .vs
            .iter()
            .map(|s| match s {
                Side::Range(a, b, _) => (range_label(*a, *b), base.iter().filter(|m| in_range(m.day, Some((*a, *b)))).count()),
                Side::Name(n) => (n.clone(), 0),
            })
            .collect();
        let best = vals.iter().map(|v| v.1).max().unwrap_or(0);
        let w: Vec<String> = vals.iter().filter(|v| v.1 == best).map(|v| v.0.clone()).collect();
        let w = if w.len() > 1 { vec!["the same".to_string()] } else { w };
        return Expected { value: Expect::Winner(w), docs };
    }
    if let Some(gr) = g.group {
        let names: BTreeMap<String, String> = t.people.values().map(|(n, e)| (e.to_lowercase(), n.clone())).collect();
        let mut rows: BTreeMap<String, usize> = BTreeMap::new();
        for m in &chosen {
            let key = match gr {
                Group::Month => label_month(NaiveDate::from_ymd_opt(m.day.year(), m.day.month(), 1).unwrap()),
                Group::Year => m.day.year().to_string(),
                Group::Person | Group::Merchant if g.dir == Some("to") => {
                    for r in &m.to {
                        if let Some(n) = names.get(r) {
                            *rows.entry(n.clone()).or_default() += 1;
                        }
                    }
                    continue;
                }
                Group::Person | Group::Merchant => match names.get(&m.from) {
                    Some(n) if !m.sent => n.clone(),
                    _ => continue,
                },
                Group::Place => continue,
            };
            *rows.entry(key).or_default() += 1;
        }
        let value = match g.op {
            Op::Max => {
                let best = rows.values().copied().max().unwrap_or(0);
                Expect::Winner(rows.iter().filter(|r| *r.1 == best).map(|r| r.0.clone()).collect())
            }
            Op::Min => {
                let best = rows.values().copied().min().unwrap_or(0);
                Expect::Winner(rows.iter().filter(|r| *r.1 == best).map(|r| r.0.clone()).collect())
            }
            _ => Expect::Groups(rows.into_iter().map(|(k, v)| (k, v as f64)).collect()),
        };
        return Expected { value, docs };
    }
    let value = match g.op {
        Op::Count | Op::List => Expect::Count(chosen.len() as u64),
        Op::Exists => Expect::Yes(!chosen.is_empty()),
        Op::First => Expect::Date(chosen.iter().map(|m| m.day).min().unwrap()),
        Op::Last => Expect::Date(chosen.iter().map(|m| m.day).max().unwrap()),
        o => panic!("messages op {o:?}"),
    };
    Expected { value, docs }
}
