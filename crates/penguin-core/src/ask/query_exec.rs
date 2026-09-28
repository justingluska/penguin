//! Answering an [`AskQuery`] exactly: read every matching fact (or
//! message), filter, then count, sum, average, rank, group or compare. The
//! answer lists every item it used, so the number can be checked, and says
//! how it read the question (`AskUnderstood`, the chips in the UI).
//!
//! Units: one per real-world thing, not per email. A booking re-sent, an
//! order's confirmation and receipt, a parcel's three updates each count
//! once (the same keys as `facts::count_facts`).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, NaiveDate};

use super::facts::{folded, group, matches_thing, mentions, parse_at, related, Hit};
use super::*;
use crate::ask::query::{
    check, label_of, read_timeframe, summary, AskGroup, AskQuery, AskResult, AskResultKind,
    AskUnderstood, Named, QueryDirection, QueryField, QueryGroup, QueryMeasure, QueryOp,
    QuerySource, QuerySubject, QueryTense,
};
use crate::dates::DateRange;
use crate::structured::{airports_for_place, city_in, Extracted};

/// Most cards and items listed under one answer.
const MAX_LISTED: usize = 60;
/// Most cited emails listed (the UI folds them; a list answer cites every one).
const MAX_CITED: usize = 500;

/// One real-world thing (a flight, a stay, an order…) and the emails about it.
struct Unit<'h> {
    hits: Vec<&'h Hit>,
    /// The day it happened (departure, check-in, event) or the email's day.
    day: NaiveDate,
    /// The first email's time (unix ms), to order things on the same day.
    ms: i64,
    amount: Option<(f64, String)>,
    nights: Option<u32>,
    place: Option<String>,
    /// Where the flight leaves from (flights only).
    from_place: Option<String>,
    merchant: Option<String>,
    /// The booking a flight belongs to (trips count bookings).
    booking: Option<String>,
    /// A bill not marked paid (left out of spending).
    unpaid: bool,
}

impl Unit<'_> {
    fn first(&self) -> &Hit {
        self.hits[0]
    }
}

fn subject_kinds(s: QuerySubject) -> &'static [&'static str] {
    match s {
        QuerySubject::Flights => &["flight"],
        QuerySubject::Stays => &["lodging"],
        QuerySubject::Orders => &["order"],
        QuerySubject::Parcels => &["shipment"],
        QuerySubject::Bills => &["bill"],
        QuerySubject::Bookings => &["reservation"],
        QuerySubject::Spending => &["order", "reservation", "lodging", "flight", "bill"],
        QuerySubject::Messages => &[],
    }
}

/// "Flights", "Hotel stays", "Orders"…
fn noun(s: QuerySubject, measure: QueryMeasure, n: usize) -> &'static str {
    match (s, measure) {
        (QuerySubject::Stays, QueryMeasure::Nights) => {
            if n == 1 {
                "night"
            } else {
                "nights"
            }
        }
        (QuerySubject::Flights, QueryMeasure::Trips) => {
            if n == 1 {
                "trip"
            } else {
                "trips"
            }
        }
        _ => match (s, n == 1) {
            (QuerySubject::Flights, true) => "flight",
            (QuerySubject::Flights, false) => "flights",
            (QuerySubject::Stays, true) => "hotel stay",
            (QuerySubject::Stays, false) => "hotel stays",
            (QuerySubject::Orders, true) => "order",
            (QuerySubject::Orders, false) => "orders",
            (QuerySubject::Parcels, true) => "package",
            (QuerySubject::Parcels, false) => "packages",
            (QuerySubject::Bills, true) => "bill",
            (QuerySubject::Bills, false) => "bills",
            (QuerySubject::Bookings, true) => "reservation",
            (QuerySubject::Bookings, false) => "reservations",
            (QuerySubject::Spending, true) => "purchase",
            (QuerySubject::Spending, false) => "purchases",
            (QuerySubject::Messages, true) => "email",
            (QuerySubject::Messages, false) => "emails",
        },
    }
}

fn title(s: &str) -> String {
    capitalize(s)
}

fn day_iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn money_of(totals: &[AskTotal]) -> String {
    if totals.is_empty() {
        return money(0.0, "USD");
    }
    totals
        .iter()
        .map(|t| money(t.value, &t.currency))
        .collect::<Vec<_>>()
        .join(" + ")
}

fn add_total(totals: &mut Vec<AskTotal>, v: f64, cur: &str) {
    match totals.iter_mut().find(|t| t.currency == cur) {
        Some(t) => {
            t.value += v;
            t.count += 1;
        }
        None => totals.push(AskTotal {
            value: v,
            currency: cur.to_string(),
            count: 1,
        }),
    }
}

/// The main number of a set of units for ranking and comparing: money in
/// the most used currency, nights, trips, or the count.
fn score(units: &[&Unit], q: &AskQuery, cur: Option<&str>) -> f64 {
    match q.measure {
        QueryMeasure::Money => units
            .iter()
            .filter_map(|u| u.amount.as_ref())
            .filter(|(_, c)| cur.is_none_or(|x| x == c))
            .map(|(v, _)| *v)
            .sum(),
        QueryMeasure::Nights => units.iter().filter_map(|u| u.nights).sum::<u32>() as f64,
        QueryMeasure::Trips => trips(units) as f64,
        QueryMeasure::Items => units.len() as f64,
    }
}

fn trips(units: &[&Unit]) -> usize {
    let mut seen: Vec<String> = Vec::new();
    let mut n = 0;
    for u in units {
        match &u.booking {
            Some(b) => {
                if !seen.contains(b) {
                    seen.push(b.clone());
                    n += 1;
                }
            }
            None => n += 1,
        }
    }
    n
}

fn totals_of(units: &[&Unit]) -> Vec<AskTotal> {
    let mut t = Vec::new();
    for u in units {
        if let Some((v, c)) = &u.amount {
            add_total(&mut t, *v, c);
        }
    }
    t.sort_by_key(|x| std::cmp::Reverse(x.count));
    t
}

/// The currency most amounts are in.
fn main_currency(units: &[&Unit]) -> Option<String> {
    totals_of(units).first().map(|t| t.currency.clone())
}

fn nights_of(l: &crate::structured::Lodging) -> Option<u32> {
    let (a, _) = parse_at(l.checkin.as_deref()?)?;
    let (b, _) = parse_at(l.checkout.as_deref()?)?;
    let n = (b - a).num_days();
    (1..=365).contains(&n).then_some(n as u32)
}

fn short_org(s: &str) -> String {
    let c = crate::structured::clean_org(s);
    if c.is_empty() {
        s.to_string()
    } else {
        c
    }
}

/// Merchant label from the fact, else the sender's organization.
fn merchant_label(h: &Hit) -> String {
    let named = match &h.f.fact {
        Extracted::Order(o) => o.merchant.clone(),
        Extracted::Shipment(s) => s.merchant.clone(),
        Extracted::Bill(b) => b.biller.clone(),
        Extracted::Reservation(r) => r.venue.clone().or(r.name.clone()),
        Extracted::Flight(f) => f.airline.clone(),
        _ => None,
    };
    named
        .map(|n| short_org(&n))
        .or_else(|| h.row.from_name.as_deref().map(short_org))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| capitalize(&h.f.key))
}

fn flight_place(code: Option<&str>, name: Option<&str>) -> Option<String> {
    name.map(String::from).or_else(|| {
        code.map(|c| {
            crate::structured::airport_by_code(c)
                .map(|(_, city)| city.to_string())
                .unwrap_or_else(|| c.to_string())
        })
    })
}

/// Build the units for a subject from its facts.
fn units<'h>(cx: &Cx, subject: QuerySubject, hits: &'h [Hit], money_day: bool) -> Vec<Unit<'h>> {
    let mut out: Vec<Unit> = Vec::new();
    let owned: Vec<Hit> = Vec::new();
    let _ = owned;
    let groups = group(hits, |h| match &h.f.fact {
        Extracted::Flight(f) => {
            let day = f.depart_time.as_deref().and_then(|t| t.get(..10)).unwrap_or("");
            let id = f
                .flight_number
                .as_ref()
                .map(|n| n.replace(' ', ""))
                .or_else(|| f.confirmation.as_ref().map(|c| format!("conf:{c}")))?;
            Some(format!("flight|{id}|{day}"))
        }
        Extracted::Lodging(l) => l
            .confirmation
            .clone()
            .map(|c| format!("lodging|{c}"))
            .or_else(|| Some(format!("lodging|{}|{}", l.name.clone().unwrap_or_default(), l.checkin.clone().unwrap_or_default()))),
        Extracted::Shipment(s) => s
            .tracking_number
            .clone()
            .map(|t| format!("shipment|{t}"))
            .or_else(|| h.f.reference.clone().map(|r| format!("shipment|{}|{r}", h.f.key))),
        Extracted::Order(o) => {
            let refund = matches!(o.status.as_deref(), Some("refunded" | "returned"));
            h.f.reference
                .clone()
                .map(|r| format!("{}|{}|{r}|{refund}", h.f.kind, h.f.key))
        }
        _ => h.f.reference.clone().map(|r| format!("{}|{}|{r}", h.f.kind, h.f.key)),
    });
    for g in groups {
        let h = g[0];
        let email_day = local_date(g.iter().map(|x| x.row.date).min().unwrap_or(h.row.date), cx.off);
        let mut u = Unit {
            hits: g.clone(),
            day: email_day,
            ms: g.iter().map(|x| x.row.date).min().unwrap_or(h.row.date),
            amount: None,
            nights: None,
            place: None,
            from_place: None,
            merchant: Some(merchant_label(h)),
            booking: None,
            unpaid: false,
        };
        // The newest email with an amount has the final one.
        let amount = g.iter().find_map(|x| Some((x.f.amount?, x.f.currency.clone()?)));
        match &h.f.fact {
            Extracted::Flight(f) => {
                if f.status.as_deref() == Some("cancelled") {
                    continue;
                }
                if let Some((d, _)) = f.depart_time.as_deref().and_then(parse_at) {
                    u.day = d;
                }
                u.place = flight_place(f.arrive_airport.as_deref(), f.arrive_name.as_deref());
                u.from_place = flight_place(f.depart_airport.as_deref(), f.depart_name.as_deref());
                u.booking = f.confirmation.clone();
                u.amount = amount;
            }
            Extracted::Lodging(l) => {
                if l.status.as_deref() == Some("cancelled") {
                    continue;
                }
                if let Some((d, _)) = l.checkin.as_deref().and_then(parse_at) {
                    u.day = d;
                }
                u.nights = nights_of(l);
                let text = format!(
                    "{} {} {}",
                    l.name.clone().unwrap_or_default(),
                    l.address.clone().unwrap_or_default(),
                    h.row.subject
                );
                u.place = city_in(&text).map(|(_, c)| c.to_string());
                u.amount = amount;
            }
            Extracted::Order(o) => {
                let refund = matches!(o.status.as_deref(), Some("refunded" | "returned"))
                    || h.row.subject.to_lowercase().contains("refund");
                if refund && subject == QuerySubject::Orders {
                    continue;
                }
                u.amount = amount.map(|(v, c)| (if refund && v > 0.0 { -v } else { v }, c));
            }
            Extracted::Bill(b) => {
                u.unpaid = b.status.as_deref() != Some("paid");
                u.amount = amount;
            }
            Extracted::Reservation(r) => {
                if r.status.as_deref() == Some("cancelled") {
                    continue;
                }
                if let Some((d, _)) = r.start.as_deref().and_then(parse_at) {
                    u.day = d;
                }
                u.amount = amount;
            }
            Extracted::Shipment(_) => {}
            Extracted::Contact(_) => continue,
        }
        // Money is spent when the email says so, not when the trip is.
        if subject == QuerySubject::Spending || money_day {
            u.day = email_day;
        }
        out.push(u);
    }
    out
}

/// Does the unit match the place, by direction for flights?
fn matches_place(u: &Unit, place: &str, dir: Option<QueryDirection>, subject: QuerySubject) -> bool {
    let mut codes: Vec<&str> = airports_for_place(place);
    if let Some(c) = super::super::qparse::country(place) {
        codes.extend_from_slice(c);
    }
    let mut names: Vec<String> = vec![folded(place)];
    for c in &codes {
        names.extend(crate::structured::airport_names(c).into_iter().map(folded));
    }
    let hit = |p: &Option<String>, code: Option<&str>| {
        code.is_some_and(|c| codes.contains(&c)) || p.as_deref().is_some_and(|x| names.iter().any(|n| mentions(x, n)))
    };
    match &u.first().f.fact {
        Extracted::Flight(f) => {
            let to = hit(&u.place, f.arrive_airport.as_deref());
            let from = hit(&u.from_place, f.depart_airport.as_deref());
            match dir {
                Some(QueryDirection::From) => from,
                Some(QueryDirection::To) => to,
                None => to || (subject == QuerySubject::Flights && from && !to && false),
            }
        }
        Extracted::Lodging(l) => {
            let text = format!(
                "{} {} {}",
                l.name.clone().unwrap_or_default(),
                l.address.clone().unwrap_or_default(),
                u.first().row.subject
            );
            names.iter().any(|n| mentions(&text, n))
        }
        _ => u.hits.iter().any(|h| {
            let text = serde_json::to_string(&h.f.fact).unwrap_or_default();
            names.iter().any(|n| mentions(&text, n))
        }),
    }
}

/// A timeframe as a [lo, hi) unix-ms pair.
fn bounds(cx: &Cx, r: DateRange) -> (Option<i64>, Option<i64>) {
    (r.from.map(|d| day_ms(d, cx.off)), r.to.map(|d| day_ms(d, cx.off)))
}

fn in_range(d: NaiveDate, r: Option<DateRange>) -> bool {
    r.is_none_or(|r| r.from.is_none_or(|f| d >= f) && r.to.is_none_or(|t| d < t))
}

/// Why a query can't run: None for a grammar parse (the templates answer
/// instead), an answer saying so for the model's or the user's query.
fn refuse(cx: &mut Cx, source: QuerySource, q: &AskQuery, why: String) -> Result<Option<AskAnswer>> {
    if source == QuerySource::Grammar {
        return Ok(None);
    }
    let mut a = cx.answer(AskIntent::Query);
    a.headline = why;
    a.confidence = AskConfidence::None;
    a.understood = Some(AskUnderstood {
        summary: summary(q, None, &[]),
        query: q.clone(),
        source,
        range_label: None,
        compare_labels: vec![],
    });
    Ok(Some(cx.finish(a)))
}

/// Answer `q` exactly. None when the grammar's reading doesn't fit the
/// mailbox (an unknown merchant or place): the caller falls back to the
/// templates and passages.
pub(super) fn run_query(cx: &mut Cx, q: &AskQuery, source: QuerySource) -> Result<Option<AskAnswer>> {
    if let Err(e) = check(q, cx.today) {
        return refuse(cx, source, q, format!("I can't answer that as asked: {e}"));
    }
    // The query's own timeframe replaces the template's date scope.
    let mut tf = q.timeframe.as_deref().and_then(|t| read_timeframe(t, cx.today, q.tense));
    // "Since March", "the last 6 months": up to today, unless the question
    // is about what's coming.
    if let Some(t) = tf.as_mut() {
        if t.range.to.is_none() && q.tense != QueryTense::Future {
            t.range.to = Some(cx.today + chrono::Duration::days(1));
        }
    }
    let range = tf.as_ref().map(|t| t.range);
    let (lo, hi) = range.map(|r| bounds(cx, r)).unwrap_or((None, None));
    cx.lo = lo;
    cx.hi = hi;
    cx.q.range_text = None;
    let mut sides: Vec<(String, Option<DateRange>)> = Vec::new();
    let mut compare_time = false;
    if q.compare.len() >= 2 {
        let times: Vec<Option<crate::ask::query::Timeframe>> =
            q.compare.iter().map(|c| read_timeframe(c, cx.today, q.tense)).collect();
        if times.iter().all(Option::is_some) {
            compare_time = true;
            for t in times.into_iter().flatten() {
                sides.push((t.label.clone(), Some(t.range)));
            }
        } else {
            for c in &q.compare {
                sides.push((c.clone(), None));
            }
        }
    }
    let range_label = tf.as_ref().map(|t| {
        let open_end = t.range.to == Some(cx.today + chrono::Duration::days(1)) && t.label.starts_with("since");
        if open_end {
            t.label.clone()
        } else {
            label_of(t.range)
        }
    });
    if let Some(t) = &tf {
        cx.steps.push(format!(
            "Read \u{201c}{}\u{201d} as {}",
            q.timeframe.clone().unwrap_or_default(),
            match (t.range.from, t.range.to) {
                (Some(f), Some(to)) => format!("{} – {}", fmt_date(f), fmt_date(to - chrono::Duration::days(1))),
                _ => t.label.clone(),
            }
        ));
    }
    let understood = AskUnderstood {
        summary: summary(q, range_label.as_deref(), &sides.iter().map(|s| s.0.clone()).collect::<Vec<_>>()),
        query: q.clone(),
        source,
        range_label: range_label.clone(),
        compare_labels: sides.iter().map(|s| s.0.clone()).collect(),
    };
    if q.subject == QuerySubject::Messages {
        return messages(cx, q, source, range, &sides, compare_time, understood);
    }

    // ---- read the facts
    let mut hits: Vec<Hit> = Vec::new();
    let search = q.merchant.clone().or(q.place.clone());
    for kind in subject_kinds(q.subject) {
        hits.extend(cx.facts(kind, search.as_deref())?);
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.row.date));
    let all = units(cx, q.subject, &hits, q.measure == QueryMeasure::Money);

    // A carrier's email names the order, not the shop: parcels from a
    // merchant are the ones whose order number is one of its orders.
    let mut shop_orders: Vec<String> = Vec::new();
    if let (QuerySubject::Parcels, Some(m)) = (q.subject, &q.merchant) {
        let saved = cx.steps.len();
        for h in cx.facts("order", Some(m))? {
            if matches_thing(&h, m) {
                if let Some(r) = &h.f.reference {
                    shop_orders.push(r.to_uppercase());
                }
            }
        }
        cx.steps.truncate(saved);
    }
    let from_shop = |u: &Unit, m: &str| {
        u.hits.iter().any(|h| {
            matches_thing(h, m)
                || matches!(&h.f.fact, Extracted::Shipment(s) if s.order_number.as_ref().is_some_and(|o| shop_orders.contains(&o.to_uppercase())))
        })
    };
    // ---- entities must exist in the mail
    if let Some(m) = &q.merchant {
        if !all.iter().any(|u| from_shop(u, m)) {
            return refuse(cx, source, q, format!("Nothing in your {} is from \u{201c}{m}\u{201d}", noun(q.subject, QueryMeasure::Items, 2)));
        }
    }
    if let Some(p) = &q.place {
        if !all.iter().any(|u| matches_place(u, p, q.direction, q.subject)) {
            let known = !airports_for_place(p).is_empty() || super::super::qparse::country(p).is_some();
            if !known {
                return refuse(cx, source, q, format!("\u{201c}{p}\u{201d} isn't a place in your {}", noun(q.subject, QueryMeasure::Items, 2)));
            }
        }
    }
    if !compare_time && !sides.is_empty() {
        for (name, _) in &sides {
            let ok = all.iter().any(|u| {
                matches_place(u, name, q.direction, q.subject) || u.hits.iter().any(|h| matches_thing(h, name))
            });
            let place = !airports_for_place(name).is_empty() || super::super::qparse::country(name).is_some();
            if !ok && !place {
                return refuse(cx, source, q, format!("Nothing in your mail matches \u{201c}{name}\u{201d}"));
            }
        }
    }

    // ---- filter
    let today = cx.today;
    let side_match = |u: &Unit, name: &str| {
        matches_place(u, name, q.direction, q.subject) || u.hits.iter().any(|h| matches_thing(h, name))
    };
    // Money at a biller (a utility, a vendor's invoices) is what it billed:
    // nothing there is a receipt, so due bills count, and the answer says so.
    let billed_only = q.subject == QuerySubject::Spending
        && q.merchant.as_deref().is_some_and(|m| {
            let mine: Vec<&Unit> = all.iter().filter(|u| u.hits.iter().any(|h| matches_thing(h, m))).collect();
            !mine.is_empty() && mine.iter().all(|u| matches!(u.first().f.fact, Extracted::Bill(_)))
        });
    if billed_only {
        cx.steps.push("Only bills from them, none marked paid: summed what they billed".into());
    }
    let base: Vec<&Unit> = all
        .iter()
        .filter(|u| q.merchant.as_deref().is_none_or(|m| from_shop(u, m)))
        .filter(|u| q.place.as_deref().is_none_or(|p| matches_place(u, p, q.direction, q.subject)))
        .filter(|u| q.measure != QueryMeasure::Money || u.amount.is_some())
        .filter(|u| q.measure != QueryMeasure::Nights || u.nights.is_some())
        .filter(|u| billed_only || !(q.subject == QuerySubject::Spending && u.unpaid))
        .filter(|u| {
            // Spending counts bills only once paid; bill questions count all.
            !(q.subject == QuerySubject::Spending && matches!(u.first().f.fact, Extracted::Flight(_) | Extracted::Lodging(_) | Extracted::Reservation(_)) && u.amount.is_none())
        })
        .collect();
    let unpaid_left = if q.subject == QuerySubject::Spending && !billed_only {
        all.iter().filter(|u| u.unpaid && in_range(u.day, range)).count()
    } else {
        0
    };
    let event = matches!(q.subject, QuerySubject::Flights | QuerySubject::Stays | QuerySubject::Bookings);
    let tense_ok = |u: &Unit| match (q.tense, q.op) {
        (_, QueryOp::Next) => u.day >= today,
        (_, QueryOp::Last) if event => u.day < today,
        (QueryTense::Past, _) if event => u.day < today,
        (QueryTense::Future, _) if event => u.day >= today,
        _ => true,
    };
    let chosen: Vec<&Unit> = base
        .iter()
        .copied()
        .filter(|u| in_range(u.day, range))
        .filter(|u| tense_ok(u))
        .collect();
    let mut what = describe(q);
    if billed_only {
        what.money_title = "billed".into();
    }
    cx.steps.push(format!(
        "Found {} in your mail{}; counted each once (emails about the same booking, order or parcel merged)",
        plural(base.len(), noun(q.subject, QueryMeasure::Items, 1), noun(q.subject, QueryMeasure::Items, 2)),
        what.filters
    ));
    let coverage = cx.coverage(noun(q.subject, QueryMeasure::Items, 2))?;
    let mut a = cx.answer(AskIntent::Query);
    a.coverage = coverage;
    a.understood = Some(understood);
    a.search_query = Some(format!("{}{}", what.search, cx.range_ops()));
    let rng = range_label.as_ref().map(|r| in_label(r)).unwrap_or_default();
    let cur = main_currency(&chosen);
    let mut result;

    // ---- compare
    if !sides.is_empty() {
        let mut vals: Vec<(String, Vec<&Unit>)> = Vec::new();
        for (label, r) in &sides {
            let v: Vec<&Unit> = if compare_time {
                base.iter().copied().filter(|u| in_range(u.day, *r)).collect()
            } else {
                chosen.iter().copied().filter(|u| side_match(u, label)).collect()
            };
            vals.push((label.clone(), v));
        }
        let cur = main_currency(&vals.iter().flat_map(|v| v.1.iter().copied()).collect::<Vec<_>>());
        let scores: Vec<f64> = vals.iter().map(|v| score(&v.1, q, cur.as_deref())).collect();
        let fmt = |i: usize| -> String {
            match q.measure {
                QueryMeasure::Money => money_of(&totals_of(&vals[i].1)),
                m => plural(scores[i] as usize, noun(q.subject, m, 1), noun(q.subject, m, 2)),
            }
        };
        let best = (0..vals.len()).max_by(|&x, &y| scores[x].total_cmp(&scores[y])).unwrap_or(0);
        let tie = scores.iter().filter(|s| (**s - scores[best]).abs() < 0.005).count() > 1;
        let side_name = |i: usize| if compare_time { vals[i].0.clone() } else { title(&vals[i].0) };
        a.headline = if tie {
            format!("The same: {} each", fmt(best))
        } else {
            let others: Vec<String> = (0..vals.len())
                .filter(|&i| i != best)
                .map(|i| format!("{} {}", fmt(i), if compare_time { format!("in {}", side_name(i)) } else { format!("for {}", side_name(i)) }))
                .collect();
            format!(
                "More {} {}: {} vs {}",
                if compare_time { "in" } else { "for" },
                side_name(best),
                fmt(best),
                others.join(", ")
            )
        };
        let diff = scores[best] - scores.iter().enumerate().filter(|(i, _)| *i != best).map(|(_, s)| *s).fold(f64::MIN, f64::max);
        if !tie && vals.len() == 2 {
            a.detail = Some(match q.measure {
                QueryMeasure::Money => format!("{} more{}", money(diff, cur.as_deref().unwrap_or("USD")), what.filters),
                m => format!("{} more{}", plural(diff.round() as usize, noun(q.subject, m, 1), noun(q.subject, m, 2)), what.filters),
            });
        }
        for (i, (label, v)) in vals.iter().enumerate() {
            a.groups.push(AskGroup {
                label: if compare_time { label.clone() } else { title(label) },
                count: v.len(),
                totals: totals_of(v),
                nights: (q.measure == QueryMeasure::Nights).then(|| scores[i] as u32),
                cites: v.iter().map(|u| u.first().row.cite()).collect(),
                start: sides[i].1.and_then(|r| r.from).map(|d| day_ms(d, cx.off)),
                best: i == best && !tie,
            });
        }
        result = AskResult::new(AskResultKind::Compare);
        result.winner = (!tie).then(|| side_name(best));
        let listed: Vec<&Unit> = vals.iter().flat_map(|v| v.1.iter().copied()).collect();
        list_units(cx, &mut a, &listed, q, true);
        a.result = Some(result);
        a.confidence = confidence(&a);
        return Ok(Some(cx.finish(a)));
    }

    // ---- groups
    if let Some(g) = q.group_by {
        // Home is where most flights leave from; flying back there isn't a
        // destination.
        let home = if q.subject == QuerySubject::Flights && g == QueryGroup::Place {
            let mut n: HashMap<&str, usize> = HashMap::new();
            for u in &all {
                if let Some(p) = &u.from_place {
                    *n.entry(p.as_str()).or_default() += 1;
                }
            }
            n.into_iter().max_by_key(|x| x.1).map(|x| x.0.to_string())
        } else {
            None
        };
        if let Some(h) = &home {
            cx.steps.push(format!("Left out flights home to {h}: most flights leave from there"));
        }
        let mut groups: BTreeMap<String, (Option<NaiveDate>, Vec<&Unit>)> = BTreeMap::new();
        for u in &chosen {
            if home.is_some() && u.place == home {
                continue;
            }
            let (key, start) = match g {
                QueryGroup::Month => (u.day.format("%Y-%m").to_string(), Some(crate::dates::first_of_month(u.day))),
                QueryGroup::Year => (u.day.year().to_string(), crate::dates::ymd(u.day.year(), 1, 1)),
                QueryGroup::Merchant => (u.merchant.clone().unwrap_or_else(|| "Unknown".into()), None),
                QueryGroup::Place => match &u.place {
                    Some(p) => (p.clone(), None),
                    None => continue,
                },
                QueryGroup::Person => continue,
            };
            let e = groups.entry(key).or_insert((start, Vec::new()));
            e.1.push(u);
        }
        let label = |k: &str, start: Option<NaiveDate>| match (g, start) {
            (QueryGroup::Month, Some(d)) => d.format("%B %Y").to_string(),
            _ => k.to_string(),
        };
        let mut rows: Vec<(String, Option<NaiveDate>, Vec<&Unit>, f64)> = groups
            .into_iter()
            .map(|(k, (s, v))| {
                let sc = score(&v, q, cur.as_deref());
                (label(&k, s), s, v, sc)
            })
            .collect();
        let periods = matches!(g, QueryGroup::Month | QueryGroup::Year);
        if q.op == QueryOp::Average {
            // Average per period: over every period in the range, empty
            // ones included (a month with no rides counts as zero).
            let n_periods = if periods {
                let first = range.and_then(|r| r.from).unwrap_or_else(|| rows.first().and_then(|r| r.1).unwrap_or(today));
                let last = range.and_then(|r| r.to).map(|t| t - chrono::Duration::days(1)).unwrap_or(today).min(today);
                if g == QueryGroup::Month {
                    ((last.year() - first.year()) * 12 + last.month() as i32 - first.month() as i32 + 1).max(1) as usize
                } else {
                    (last.year() - first.year() + 1).max(1) as usize
                }
            } else {
                rows.len().max(1)
            };
            let total: f64 = rows.iter().map(|r| r.3).sum();
            let avg = total / n_periods as f64;
            let unit = match g {
                QueryGroup::Month => "month",
                QueryGroup::Year => "year",
                QueryGroup::Merchant => "merchant",
                QueryGroup::Place => "place",
                QueryGroup::Person => "person",
            };
            let shown = match q.measure {
                QueryMeasure::Money => money(avg, cur.as_deref().unwrap_or("USD")),
                m => format!("{avg:.1} {}", noun(q.subject, m, 2)),
            };
            a.headline = format!("{shown} per {unit} on average{rng}");
            a.detail = Some(format!(
                "{} over {}{}",
                match q.measure {
                    QueryMeasure::Money => money(total, cur.as_deref().unwrap_or("USD")),
                    m => plural(total as usize, noun(q.subject, m, 1), noun(q.subject, m, 2)),
                },
                plural(n_periods, unit, &format!("{unit}s")),
                what.filters
            ));
            result = AskResult::new(AskResultKind::Average);
            result.value = Some(avg);
            if q.measure == QueryMeasure::Money {
                result.totals = vec![AskTotal {
                    value: avg,
                    currency: cur.clone().unwrap_or_else(|| "USD".into()),
                    count: n_periods,
                }];
            }
        } else {
            if matches!(q.op, QueryOp::Max | QueryOp::Min) || !periods {
                rows.sort_by(|x, y| y.3.total_cmp(&x.3).then(x.0.cmp(&y.0)));
            }
            let pick = match q.op {
                QueryOp::Max => rows.iter().max_by(|x, y| x.3.total_cmp(&y.3).then(y.1.cmp(&x.1))).map(|r| r.0.clone()),
                QueryOp::Min => rows.iter().min_by(|x, y| x.3.total_cmp(&y.3).then(x.1.cmp(&y.1))).map(|r| r.0.clone()),
                _ => None,
            };
            let fmt_row = |v: &[&Unit], sc: f64| match q.measure {
                QueryMeasure::Money => money_of(&totals_of(v)),
                m => plural(sc as usize, noun(q.subject, m, 1), noun(q.subject, m, 2)),
            };
            if rows.is_empty() {
                a.headline = format!("No {}{rng}{}", noun(q.subject, q.measure, 2), what.filters);
                result = AskResult::new(AskResultKind::Groups);
            } else if let Some(p) = &pick {
                let r = rows.iter().find(|r| &r.0 == p).expect("picked");
                let tied: Vec<&String> = rows.iter().filter(|x| (x.3 - r.3).abs() < 0.005).map(|x| &x.0).collect();
                a.headline = format!(
                    "{}{}: {}{rng}",
                    if tied.len() > 1 {
                        tied.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" and ")
                    } else {
                        p.clone()
                    },
                    match q.op {
                        QueryOp::Max => " had the most",
                        _ => " had the least",
                    },
                    fmt_row(&r.2, r.3)
                );
                result = AskResult::new(AskResultKind::Groups);
                result.winner = Some(p.clone());
            } else {
                a.headline = format!(
                    "{} by {}{rng}: {}",
                    title(&what.title),
                    g.name(),
                    rows.iter().take(3).map(|r| format!("{} {}", r.0, fmt_row(&r.2, r.3))).collect::<Vec<_>>().join(", ")
                        + if rows.len() > 3 { ", …" } else { "" }
                );
                result = AskResult::new(AskResultKind::Groups);
            }
            for r in &rows {
                a.groups.push(AskGroup {
                    label: r.0.clone(),
                    count: r.2.len(),
                    totals: totals_of(&r.2),
                    nights: (q.measure == QueryMeasure::Nights).then(|| r.3 as u32),
                    cites: r.2.iter().map(|u| u.first().row.cite()).collect(),
                    start: r.1.map(|d| day_ms(d, cx.off)),
                    best: pick.as_ref() == Some(&r.0),
                });
            }
        }
        result.count = Some(chosen.len() as u64);
        list_units(cx, &mut a, &chosen, q, false);
        a.result = Some(result);
        a.confidence = confidence(&a);
        return Ok(Some(cx.finish(a)));
    }

    // ---- single values
    let n = chosen.len();
    match q.op {
        QueryOp::Count | QueryOp::List => {
            let v = score(&chosen, q, None) as usize;
            a.headline = format!("{}{}{rng}: {}", title(&what.title), what.filters, if q.measure == QueryMeasure::Money { money_of(&totals_of(&chosen)) } else { v.to_string() });
            if matches!(q.measure, QueryMeasure::Nights | QueryMeasure::Trips) {
                a.detail = Some(plural(n, noun(q.subject, QueryMeasure::Items, 1), noun(q.subject, QueryMeasure::Items, 2)));
            } else if q.subject == QuerySubject::Flights && trips(&chosen) != n && n > 0 {
                a.detail = Some(format!("On {}", plural(trips(&chosen), "booking", "bookings")));
            }
            let totals = totals_of(&chosen);
            if !totals.is_empty() && q.measure != QueryMeasure::Money && q.subject != QuerySubject::Flights {
                a.detail = Some(format!("{} across the ones with an amount", money_of(&totals)));
            }
            result = AskResult::new(if q.op == QueryOp::List { AskResultKind::List } else { AskResultKind::Count });
            result.count = Some(v as u64);
            if q.measure == QueryMeasure::Nights {
                result.value = Some(v as f64);
            }
            list_units(cx, &mut a, &chosen, q, false);
        }
        QueryOp::Sum => {
            let totals = totals_of(&chosen);
            a.headline = format!("{}{}{rng}: {}", title(&what.money_title), what.filters, money_of(&totals));
            a.detail = Some(format!(
                "Across {}{}",
                plural(n, noun(q.subject, QueryMeasure::Items, 1), noun(q.subject, QueryMeasure::Items, 2)),
                if unpaid_left > 0 {
                    format!("; {} not marked paid left out", plural(unpaid_left, "bill", "bills"))
                } else {
                    String::new()
                }
            ));
            if q.subject == QuerySubject::Bills {
                a.detail = Some(format!("Billed across {}", plural(n, "bill", "bills")));
            }
            a.sum = Some(AskSum {
                totals: totals.clone(),
                basis: format!("The total on each {}", noun(q.subject, QueryMeasure::Items, 1)),
                duplicates: 0,
                skipped: 0,
                unpaid: unpaid_left,
            });
            result = AskResult::new(AskResultKind::Sum);
            result.count = Some(n as u64);
            result.totals = totals;
            list_units(cx, &mut a, &chosen, q, false);
        }
        QueryOp::Average => {
            result = AskResult::new(AskResultKind::Average);
            match q.measure {
                QueryMeasure::Money => {
                    let totals: Vec<AskTotal> = totals_of(&chosen)
                        .into_iter()
                        .map(|t| AskTotal { value: t.value / t.count.max(1) as f64, ..t })
                        .collect();
                    a.headline = format!("Average {}{}{rng}: {}", noun(q.subject, QueryMeasure::Items, 1), what.filters, if totals.is_empty() { "none".into() } else { money_of(&totals) });
                    a.detail = Some(format!("Over {}", plural(n, noun(q.subject, QueryMeasure::Items, 1), noun(q.subject, QueryMeasure::Items, 2))));
                    result.totals = totals;
                }
                m => {
                    let v = if n == 0 { 0.0 } else { score(&chosen, q, None) / n as f64 };
                    a.headline = format!("Average {} per {}{}{rng}: {v:.1}", noun(q.subject, m, 2), noun(q.subject, QueryMeasure::Items, 1), what.filters);
                    result.value = Some(v);
                }
            }
            result.count = Some(n as u64);
            list_units(cx, &mut a, &chosen, q, false);
        }
        QueryOp::Max | QueryOp::Min => {
            let cur = cur.clone();
            let val = |u: &Unit| match q.measure {
                QueryMeasure::Nights => u.nights.unwrap_or(0) as f64,
                _ => u.amount.as_ref().filter(|(_, c)| cur.as_deref().is_none_or(|x| x == c)).map(|(v, _)| *v).unwrap_or(f64::NAN),
            };
            let pool: Vec<&Unit> = chosen.iter().copied().filter(|u| !val(u).is_nan()).collect();
            let pick = if q.op == QueryOp::Max {
                pool.iter().copied().max_by(|x, y| val(x).total_cmp(&val(y)))
            } else {
                pool.iter().copied().min_by(|x, y| val(x).total_cmp(&val(y)))
            };
            result = AskResult::new(AskResultKind::Item);
            match pick {
                None => {
                    a.headline = format!("No {} with {}{}{rng}", noun(q.subject, QueryMeasure::Items, 2), if q.measure == QueryMeasure::Nights { "dates" } else { "an amount" }, what.filters);
                }
                Some(u) => {
                    let shown = match q.measure {
                        QueryMeasure::Nights => plural(u.nights.unwrap_or(0) as usize, "night", "nights"),
                        _ => u.amount.as_ref().map(|(v, c)| money(*v, c)).unwrap_or_default(),
                    };
                    a.headline = format!(
                        "{} {}{}{rng}: {shown}, {}{}",
                        if q.op == QueryOp::Max { if q.measure == QueryMeasure::Nights { "Longest" } else { "Most expensive" } } else if q.measure == QueryMeasure::Nights { "Shortest" } else { "Cheapest" },
                        noun(q.subject, QueryMeasure::Items, 1),
                        what.filters,
                        u.merchant.clone().map(|m| format!("{m} on ")).unwrap_or_default(),
                        fmt_date(u.day)
                    );
                    result.date = Some(day_iso(u.day));
                    if let Some((v, c)) = &u.amount {
                        result.totals = vec![AskTotal { value: *v, currency: c.clone(), count: 1 }];
                    }
                    result.value = u.nights.map(f64::from);
                    list_units(cx, &mut a, &[u], q, false);
                    // The rest, for context.
                    let rest: Vec<&Unit> = pool.iter().copied().filter(|x| !std::ptr::eq(*x, u)).collect();
                    list_units(cx, &mut a, &rest, q, false);
                }
            }
            result.count = Some(pool.len() as u64);
        }
        QueryOp::First | QueryOp::Last | QueryOp::Next => {
            let mut sorted = chosen.clone();
            sorted.sort_by_key(|u| (u.day, u.ms));
            let pick = match q.op {
                QueryOp::First | QueryOp::Next => sorted.first().copied(),
                _ => sorted.last().copied(),
            };
            result = AskResult::new(AskResultKind::Item);
            let word = match q.op {
                QueryOp::First => "First",
                QueryOp::Next => "Next",
                _ => "Last",
            };
            match pick {
                None => {
                    a.headline = format!("No {}{}{rng}{}", noun(q.subject, QueryMeasure::Items, 2), what.filters, if q.op == QueryOp::Next { " coming up" } else { "" });
                }
                Some(u) => {
                    let field = field_value(u, q.field);
                    let rel = relative(u.day, today);
                    a.headline = match (&field, q.field) {
                        (Some(v), Some(f)) if f != QueryField::Date => format!("{} of your {} {}{}: {v}", field_name(f), word.to_lowercase(), noun(q.subject, QueryMeasure::Items, 1), what.filters),
                        _ => format!(
                            "{word} {}{}{rng}: {} ({rel}){}",
                            noun(q.subject, QueryMeasure::Items, 1),
                            what.filters,
                            fmt_date(u.day),
                            u.merchant.clone().filter(|_| !matches!(q.subject, QuerySubject::Flights | QuerySubject::Stays)).map(|m| format!(", {m}")).unwrap_or_default()
                        ),
                    };
                    result.date = Some(day_iso(u.day));
                    result.text = field;
                    list_units(cx, &mut a, &[u], q, false);
                }
            }
            result.count = Some(n as u64);
        }
        QueryOp::Exists => {
            result = AskResult::new(AskResultKind::Exists);
            result.yes = Some(n > 0);
            result.count = Some(n as u64);
            let mut sorted = chosen.clone();
            sorted.sort_by_key(|u| std::cmp::Reverse(u.day));
            a.headline = if n == 0 {
                format!("No: no {}{}{rng} in your mail", noun(q.subject, QueryMeasure::Items, 2), what.filters)
            } else {
                format!(
                    "Yes: {}{}{rng}, the latest on {}",
                    plural(n, noun(q.subject, QueryMeasure::Items, 1), noun(q.subject, QueryMeasure::Items, 2)),
                    what.filters,
                    fmt_date(sorted[0].day)
                )
            };
            list_units(cx, &mut a, &sorted, q, false);
        }
    }
    a.result = Some(result);
    a.confidence = confidence(&a);
    Ok(Some(cx.finish(a)))
}

/// " in August 2026", " since Mar 1, 2026", " in Jun 1 – Aug 31, 2026".
fn in_label(label: &str) -> String {
    if label.starts_with("since ") || label.starts_with("before ") {
        format!(" {label}")
    } else {
        format!(" in {label}")
    }
}

fn confidence(a: &AskAnswer) -> AskConfidence {
    if a.coverage.is_some() {
        AskConfidence::Medium
    } else {
        AskConfidence::High
    }
}

fn field_name(f: QueryField) -> &'static str {
    match f {
        QueryField::Date => "Date",
        QueryField::Confirmation => "Confirmation code",
        QueryField::FlightNumber => "Flight number",
        QueryField::Tracking => "Tracking number",
        QueryField::OrderNumber => "Order number",
        QueryField::Amount => "Amount",
        QueryField::Address => "Address",
    }
}

fn field_value(u: &Unit, f: Option<QueryField>) -> Option<String> {
    let f = f?;
    let fact = &u.first().f.fact;
    Some(match (f, fact) {
        (QueryField::Confirmation, Extracted::Flight(x)) => x.confirmation.clone()?,
        (QueryField::Confirmation, Extracted::Lodging(x)) => x.confirmation.clone()?,
        (QueryField::Confirmation, Extracted::Reservation(x)) => x.confirmation.clone()?,
        (QueryField::FlightNumber, Extracted::Flight(x)) => x.flight_number.clone()?,
        (QueryField::Tracking, Extracted::Shipment(x)) => x.tracking_number.clone()?,
        (QueryField::OrderNumber, Extracted::Order(x)) => x.order_number.clone()?,
        (QueryField::OrderNumber, Extracted::Shipment(x)) => x.order_number.clone()?,
        (QueryField::Address, Extracted::Lodging(x)) => x.address.clone()?,
        (QueryField::Amount, _) => u.amount.as_ref().map(|(v, c)| money(*v, c))?,
        (QueryField::Date, _) => fmt_date(u.day),
        _ => return None,
    })
}

/// Cards and cited items for the units used, newest first.
fn list_units(cx: &Cx, a: &mut AskAnswer, units: &[&Unit], q: &AskQuery, _compare: bool) {
    let mut sorted: Vec<&Unit> = units.to_vec();
    if !matches!(q.op, QueryOp::Max | QueryOp::Min | QueryOp::First | QueryOp::Last | QueryOp::Next) {
        sorted.sort_by_key(|u| u.day);
        if !(q.tense == QueryTense::Future || q.op == QueryOp::List) {
            sorted.reverse();
        }
    }
    for u in sorted {
        if a.items.len() >= MAX_CITED {
            break;
        }
        let h = u.first();
        // Both legs of a round trip can come from one email: one card
        // each, one cited email.
        let status = if u.unpaid && q.subject == QuerySubject::Bills { Some("Due".to_string()) } else { None };
        if a.cards.len() < MAX_LISTED {
            a.cards.push(cx.card(h, related(&u.hits), status));
        }
        if a.items.iter().any(|i| i.message_id == h.row.id && i.account_id == h.row.account_id) {
            continue;
        }
        let mut it = h.row.item(None);
        if let Some((v, c)) = &u.amount {
            it.amount = Some(AskAmount {
                value: *v,
                currency: c.clone(),
                source: String::new(),
            });
        }
        a.items.push(it);
    }
}

/// How the query reads in a headline.
struct Described {
    /// "flights", "orders".
    title: String,
    /// "spent on flights", "spending".
    money_title: String,
    /// " to Lisbon", " from Gearloft".
    filters: String,
    /// A Penguin search showing the mail.
    search: String,
}

fn describe(q: &AskQuery) -> Described {
    let mut filters = String::new();
    if let Some(p) = &q.place {
        filters.push_str(&match (q.subject, q.direction) {
            (QuerySubject::Flights, Some(QueryDirection::From)) => format!(" from {}", title(p)),
            (QuerySubject::Flights, _) => format!(" to {}", title(p)),
            _ => format!(" in {}", title(p)),
        });
    }
    if let Some(m) = &q.merchant {
        filters.push_str(&match q.subject {
            QuerySubject::Flights => format!(" with {}", title(m)),
            QuerySubject::Stays => format!(" at {}", title(m)),
            QuerySubject::Spending => format!(" at {}", title(m)),
            _ => format!(" from {}", title(m)),
        });
    }
    let title_s = match (q.subject, q.measure) {
        (QuerySubject::Stays, QueryMeasure::Nights) => "nights in hotels".to_string(),
        (QuerySubject::Flights, QueryMeasure::Trips) => "trips".to_string(),
        (s, _) => noun(s, QueryMeasure::Items, 2).to_string(),
    };
    let money_title = match q.subject {
        QuerySubject::Spending => "spent".to_string(),
        QuerySubject::Bills => "billed".to_string(),
        s => format!("spent on {}", noun(s, QueryMeasure::Items, 2)),
    };
    let search = match q.subject {
        QuerySubject::Flights => "flight",
        QuerySubject::Stays => "(hotel OR booking OR reservation)",
        QuerySubject::Orders | QuerySubject::Spending => "(order OR receipt)",
        QuerySubject::Parcels => "(shipped OR tracking OR delivered)",
        QuerySubject::Bills => "(bill OR invoice OR statement)",
        QuerySubject::Bookings => "(reservation OR tickets)",
        QuerySubject::Messages => "",
    };
    let search = match (&q.merchant, &q.place) {
        (Some(m), _) => format!("{search} {m}"),
        (None, Some(p)) => format!("{search} {p}"),
        _ => search.to_string(),
    };
    Described {
        title: title_s,
        money_title,
        filters,
        search,
    }
}

// ------------------------------------------------------------- messages

fn messages(
    cx: &mut Cx,
    q: &AskQuery,
    source: QuerySource,
    range: Option<DateRange>,
    sides: &[(String, Option<DateRange>)],
    compare_time: bool,
    understood: AskUnderstood,
) -> Result<Option<AskAnswer>> {
    let dir = match q.direction {
        Some(QueryDirection::From) => Dir::FromThem,
        Some(QueryDirection::To) => Dir::ToThem,
        None => Dir::Any,
    };
    // Rows over the widest range any side needs.
    let widest = |cx: &Cx| -> (Option<i64>, Option<i64>) {
        if compare_time {
            let lo = sides.iter().filter_map(|s| s.1.and_then(|r| r.from)).min().map(|d| day_ms(d, cx.off));
            let hi = sides.iter().filter_map(|s| s.1.and_then(|r| r.to)).max().map(|d| day_ms(d, cx.off));
            (lo, hi)
        } else {
            (cx.lo, cx.hi)
        }
    };
    let (lo, hi) = widest(cx);
    let mut label = "you".to_string();
    let rows: Vec<Row> = if let Some(p) = &q.person {
        let t = match resolve_who(cx, p, AskIntent::Query)? {
            Resolved::Found(t, _) => t,
            Resolved::Answer(a) => {
                return if source == QuerySource::Grammar { Ok(None) } else { Ok(Some(*a)) };
            }
        };
        label = t.label.clone();
        let rows = cx.store.read(|c| target_rows(cx, c, &t, lo, hi))?;
        rows.into_iter()
            .filter(|r| match dir {
                Dir::FromThem => r.from_them,
                Dir::ToThem => r.by_me,
                Dir::Any => r.from_them || r.by_me,
            })
            .collect()
    } else {
        let (rlo, rhi) = rowid_bounds(lo, hi);
        let scope = cx.scope.clone();
        cx.store.read(|c| {
            let own = own_addresses(c)?;
            let mut sql = format!(
                "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
                 WHERE m.rowid >= ? AND m.rowid < ? AND m.flags & ? = 0"
            );
            let mut ps: Vec<Value> = vec![Value::Integer(rlo), Value::Integer(rhi), Value::Integer(HIDDEN)];
            if let Some(s) = &scope {
                sql.push_str(&format!(" AND m.account_id IN ({})", vec!["?"; s.len()].join(",")));
                ps.extend(s.iter().map(|x| Value::Text(x.clone())));
            }
            sql.push_str(&format!(" ORDER BY m.rowid LIMIT {MAX_ROWS}"));
            let mut out = Vec::new();
            for r in c.prepare(&sql)?.query_map(params_from_iter(ps), read_row)? {
                let mut r = r?;
                r.by_me = r.flags & F_SENT != 0 || own.contains(&r.from_email);
                r.from_them = !r.by_me;
                let keep = match dir {
                    Dir::FromThem => r.from_them,
                    Dir::ToThem => r.by_me,
                    Dir::Any => true,
                };
                if keep {
                    out.push(r);
                }
            }
            Ok(out)
        })?
    };
    let who = match (&q.person, dir) {
        (Some(_), Dir::FromThem) => format!(" from {label}"),
        (Some(_), Dir::ToThem) => format!(" to {label}"),
        (Some(_), Dir::Any) => format!(" with {label}"),
        (None, Dir::FromThem) => " received".to_string(),
        (None, Dir::ToThem) => " sent".to_string(),
        (None, Dir::Any) => String::new(),
    };
    cx.steps.push(format!("Counted every stored message{who} (exact, not estimated)"));
    let mut a = cx.answer(AskIntent::Query);
    a.understood = Some(understood);
    let rng = range.map(|r| in_label(&label_of(r))).unwrap_or_default();
    let today = cx.today;
    let day = |r: &Row| local_date(r.date, cx.off);
    let mut result;
    if !sides.is_empty() {
        let mut vals: Vec<(String, Vec<&Row>)> = Vec::new();
        for (l, r) in sides {
            vals.push((l.clone(), rows.iter().filter(|x| compare_time && in_range(day(x), *r)).collect()));
        }
        let best = (0..vals.len()).max_by_key(|&i| vals[i].1.len()).unwrap_or(0);
        let tie = vals.iter().filter(|v| v.1.len() == vals[best].1.len()).count() > 1;
        a.headline = if tie {
            format!("The same: {} each", plural(vals[best].1.len(), "email", "emails"))
        } else {
            format!(
                "More in {}: {} vs {}",
                vals[best].0,
                plural(vals[best].1.len(), "email", "emails"),
                vals.iter().enumerate().filter(|(i, _)| *i != best).map(|(_, v)| format!("{} in {}", v.1.len(), v.0)).collect::<Vec<_>>().join(", ")
            )
        };
        for (i, (l, v)) in vals.iter().enumerate() {
            a.groups.push(AskGroup {
                label: l.clone(),
                count: v.len(),
                totals: vec![],
                nights: None,
                cites: v.iter().take(MAX_LISTED).map(|r| r.cite()).collect(),
                start: sides[i].1.and_then(|r| r.from).map(|d| day_ms(d, cx.off)),
                best: i == best && !tie,
            });
        }
        result = AskResult::new(AskResultKind::Compare);
        result.winner = (!tie).then(|| vals[best].0.clone());
    } else if let Some(g) = q.group_by {
        let mut groups: HashMap<String, (Option<NaiveDate>, Vec<&Row>)> = HashMap::new();
        let mut names: HashMap<String, String> = rows
            .iter()
            .filter_map(|r| Some((r.from_email.clone(), r.from_name.clone()?)))
            .collect();
        // "Who did I email the most": the people on the mail you sent.
        let sent_to: HashMap<i64, Vec<String>> = if matches!(g, QueryGroup::Person | QueryGroup::Merchant) && dir == Dir::ToThem {
            cx.store.read(|c| {
                let own = own_addresses(c)?;
                let mut m = HashMap::new();
                for r in rows.iter().filter(|r| r.by_me) {
                    let to: Vec<String> = recipients(c, r.rowid)?
                        .into_iter()
                        .filter(|e| !own.contains(e) && !automated(e))
                        .collect();
                    m.insert(r.rowid, to);
                }
                // Names for them, from mail they sent.
                Ok(m)
            })?
        } else {
            HashMap::new()
        };
        if !sent_to.is_empty() {
            let wanted: HashSet<&String> = sent_to.values().flatten().collect();
            let found: Vec<(String, String)> = cx.store.read(|c| {
                let mut out = Vec::new();
                let mut stmt = c.prepare_cached("SELECT from_name FROM messages WHERE from_email = ?1 COLLATE NOCASE AND from_name IS NOT NULL LIMIT 1")?;
                for e in &wanted {
                    if let Some(n) = stmt.query_row([e.as_str()], |r| r.get::<_, String>(0)).optional()? {
                        out.push(((*e).clone(), n));
                    }
                }
                Ok(out)
            })?;
            names.extend(found);
        }
        for r in &rows {
            let keys: Vec<(String, Option<NaiveDate>)> = match g {
                QueryGroup::Month => {
                    let d = crate::dates::first_of_month(day(r));
                    vec![(d.format("%Y-%m").to_string(), Some(d))]
                }
                QueryGroup::Year => vec![(day(r).year().to_string(), crate::dates::ymd(day(r).year(), 1, 1))],
                QueryGroup::Person | QueryGroup::Merchant if dir == Dir::ToThem => {
                    sent_to.get(&r.rowid).into_iter().flatten().map(|e| (e.clone(), None)).collect()
                }
                QueryGroup::Person | QueryGroup::Merchant => {
                    if r.by_me || automated(&r.from_email) || r.flags & F_NEWSLETTER != 0 {
                        continue;
                    }
                    vec![(r.from_email.clone(), None)]
                }
                QueryGroup::Place => continue,
            };
            for (key, start) in keys {
                groups.entry(key).or_insert((start, Vec::new())).1.push(r);
            }
        }
        let mut list: Vec<(String, Option<NaiveDate>, Vec<&Row>)> = groups
            .into_iter()
            .map(|(k, (s, v))| {
                let l = match (g, s) {
                    (QueryGroup::Month, Some(d)) => d.format("%B %Y").to_string(),
                    (QueryGroup::Person | QueryGroup::Merchant, _) => names.get(&k).cloned().unwrap_or(k),
                    _ => k,
                };
                (l, s, v)
            })
            .collect();
        list.sort_by(|x, y| y.2.len().cmp(&x.2.len()).then(x.0.cmp(&y.0)));
        let pick = match q.op {
            QueryOp::Max => list.first().map(|x| x.0.clone()),
            QueryOp::Min => list.last().map(|x| x.0.clone()),
            _ => None,
        };
        a.headline = match &pick {
            Some(p) => {
                let n = list.iter().find(|x| &x.0 == p).map(|x| x.2.len()).unwrap_or(0);
                format!("{p}{}: {}{rng}", if q.op == QueryOp::Max { " had the most" } else { " had the least" }, plural(n, "email", "emails"))
            }
            None => format!(
                "Emails{who} by {}{rng}: {}",
                g.name(),
                list.iter().take(3).map(|x| format!("{} {}", x.0, x.2.len())).collect::<Vec<_>>().join(", ")
            ),
        };
        for x in &list {
            a.groups.push(AskGroup {
                label: x.0.clone(),
                count: x.2.len(),
                totals: vec![],
                nights: None,
                cites: x.2.iter().rev().take(10).map(|r| r.cite()).collect(),
                start: x.1.map(|d| day_ms(d, cx.off)),
                best: pick.as_ref() == Some(&x.0),
            });
        }
        result = AskResult::new(AskResultKind::Groups);
        result.winner = pick;
    } else {
        let n = rows.len();
        match q.op {
            QueryOp::Exists => {
                a.headline = if n == 0 { format!("No: no emails{who}{rng}") } else { format!("Yes: {}{who}{rng}", plural(n, "email", "emails")) };
                result = AskResult::new(AskResultKind::Exists);
                result.yes = Some(n > 0);
            }
            QueryOp::First | QueryOp::Last | QueryOp::Next => {
                let r = if q.op == QueryOp::First { rows.iter().min_by_key(|r| r.date) } else { rows.iter().max_by_key(|r| r.date) };
                result = AskResult::new(AskResultKind::Item);
                match r {
                    Some(r) => {
                        a.headline = format!(
                            "{} email{who}{rng}: {} ({})",
                            if q.op == QueryOp::First { "First" } else { "Last" },
                            fmt_date(day(r)),
                            relative(day(r), today)
                        );
                        result.date = Some(day_iso(day(r)));
                        a.items.push(r.item(None));
                    }
                    None => a.headline = format!("No emails{who}{rng}"),
                }
            }
            _ => {
                a.headline = format!("Emails{who}{rng}: {n}");
                let threads: std::collections::HashSet<i64> = rows.iter().map(|r| r.thread_rowid).collect();
                a.detail = Some(format!("In {}", plural(threads.len(), "thread", "threads")));
                result = AskResult::new(if q.op == QueryOp::List { AskResultKind::List } else { AskResultKind::Count });
            }
        }
        result.count = Some(n as u64);
    }
    if a.items.is_empty() {
        let mut shown: Vec<&Row> = rows.iter().collect();
        shown.sort_by_key(|r| std::cmp::Reverse(r.date));
        a.items = shown.iter().take(MAX_ITEMS).map(|r| r.item(None)).collect();
    }
    a.result = Some(result);
    a.confidence = AskConfidence::High;
    Ok(Some(cx.finish(a)))
}
