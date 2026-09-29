//! Answers from facts extracted at index time (`crate::structured`): the
//! next flight, a hotel stay, where a parcel is, what was ordered, which
//! bills are due, bookings, verification codes, a person's phone number,
//! and counts of those things. Exact: SQL over the `extracted` table, with
//! one card per fact and a citation per card.
//!
//! While the background scanner is still reading older mail, questions
//! that name something also extract, in memory, the unscanned messages a
//! search finds, and the answer says what it hasn't read yet.

use std::collections::HashMap;

use chrono::NaiveDate;

use super::*;
use crate::store::{extract_now, read_fact, Recurring, StoredFact, FACT_COLS};
use crate::structured::{airport_names, airports_for_place, Extracted, Source};

/// Most facts of one kind read per question.
const MAX_FACTS: i64 = 5_000;

/// A fact and the message it came from.
pub(super) struct Hit {
    pub f: StoredFact,
    pub row: Row,
}

pub(super) fn source_of(code: i64) -> Source {
    match code {
        1 => Source::JsonLd,
        2 => Source::Microdata,
        _ => Source::Pattern,
    }
}

pub(super) fn folded(s: &str) -> String {
    crate::text::tokens(s)
        .into_iter()
        .map(|t| t.2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Does `hay` contain every word of `needle` (folded, whole words)?
pub(super) fn mentions(hay: &str, needle: &str) -> bool {
    let h = format!(" {} ", folded(hay));
    let n = folded(needle);
    !n.is_empty() && n.split(' ').all(|w| h.contains(&format!(" {w} ")))
}

/// "2026-10-02T19:05" → (date, Some((19, 5))).
pub(super) fn parse_at(s: &str) -> Option<(NaiveDate, Option<(u32, u32)>)> {
    let d = NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?;
    let t = s.get(11..16).and_then(|hm| {
        let (h, m) = hm.split_once(':')?;
        Some((h.parse().ok()?, m.parse().ok()?))
    });
    Some((d, t))
}

fn clock12(h: u32, m: u32) -> String {
    let (h12, ap) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    if m == 0 {
        format!("{h12} {ap}")
    } else {
        format!("{h12}:{m:02} {ap}")
    }
}

/// "Fri, Oct 2 at 7:05 PM" (the year when it isn't this year).
pub(super) fn fmt_at(s: &str, today: NaiveDate) -> String {
    let Some((d, t)) = parse_at(s) else {
        return s.to_string();
    };
    let day = if d.year() == today.year() {
        d.format("%a, %b %-d").to_string()
    } else {
        d.format("%a, %b %-d, %Y").to_string()
    };
    match t {
        Some((h, m)) => format!("{day} at {}", clock12(h, m)),
        None => day,
    }
}

impl Cx<'_> {
    pub(super) fn today_iso(&self) -> String {
        self.today.format("%Y-%m-%d").to_string()
    }

    /// Messages the scanner hasn't read (asked once per question).
    pub(super) fn pending(&self) -> Result<i64> {
        if let Some(n) = self.pending.get() {
            return Ok(n);
        }
        let n = self.store.extract_remaining()?;
        self.pending.set(Some(n));
        Ok(n)
    }

    /// A caveat while the scanner hasn't read everything.
    pub(super) fn coverage(&self, what: &str) -> Result<Option<String>> {
        let n = self.pending()?;
        Ok((n > 0).then(|| {
            format!(
                "Still reading {} for {what}; this may be missing some.",
                plural(n as usize, "email", "emails")
            )
        }))
    }

    /// Facts of `kind`, newest email first, in scope. When the scanner is
    /// behind and `query` is given, unscanned messages a search for it
    /// finds are extracted now too.
    pub(super) fn facts(&mut self, kind: &str, query: Option<&str>) -> Result<Vec<Hit>> {
        let mut out: Vec<Hit> = self.store.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {FACT_COLS}, {ROW_COLS} FROM extracted x
                 JOIN messages m ON m.rowid = x.msg JOIN threads t ON t.rowid = m.thread_rowid
                 WHERE x.kind = ?1 AND m.flags & ?2 = 0 ORDER BY x.date DESC LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![kind, HIDDEN, MAX_FACTS], |r| {
                Ok((read_fact(r)?, read_row_at(r, 11)?))
            })?;
            let mut v = Vec::new();
            for x in rows {
                if let (Some(f), row) = x? {
                    v.push(Hit { f, row });
                }
            }
            Ok(v)
        })?;
        out.retain(|h| self.in_scope(&h.row.account_id));
        let stored = out.len();
        if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
            if self.pending()? > 0 {
                let hits = search(self, q, 40)?;
                // Every unscanned message of each thread found (a bill and
                // its "payment received" share one).
                let extra: Vec<Hit> = self.store.read(|c| {
                    let mut rows = c.prepare_cached(&format!(
                        "SELECT {ROW_COLS} FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
                         WHERE t.account_id = ?1 AND t.thread_id = ?2 AND m.extracted IS NULL AND m.flags & ?3 = 0
                         ORDER BY m.rowid DESC LIMIT 10"
                    ))?;
                    let mut v = Vec::new();
                    for h in &hits {
                        let found: Vec<Row> = rows
                            .query_map(params![h.account_id, h.thread_id, HIDDEN], read_row)?
                            .collect::<rusqlite::Result<_>>()?;
                        for r in found {
                            for f in extract_now(c, r.rowid)? {
                                if f.kind == kind {
                                    v.push(Hit { f, row: r.clone() });
                                }
                            }
                        }
                    }
                    Ok(v)
                })?;
                if !extra.is_empty() {
                    self.steps.push(format!(
                        "Also read {} not scanned yet, found by searching \u{201c}{q}\u{201d}",
                        plural(extra.len(), "fact", "facts")
                    ));
                }
                out.extend(extra);
                out.sort_by_key(|h| std::cmp::Reverse(h.row.date));
            }
        }
        let markup = out.iter().filter(|h| h.f.source != 3).count();
        self.steps.push(format!(
            "Read {} extracted from your mail{} ({} from schema.org markup, {} from the text)",
            plural(stored, &format!("{kind} fact"), &format!("{kind} facts")),
            if out.len() > stored {
                " and the unscanned matches"
            } else {
                ""
            },
            markup,
            out.len() - markup
        ));
        Ok(out)
    }

    pub(super) fn card(&self, h: &Hit, related: Vec<AskCite>, status: Option<String>) -> AskCard {
        AskCard {
            fact: h.f.fact.clone(),
            cite: h.row.cite(),
            from: Address {
                name: h.row.from_name.clone(),
                email: h.row.from_email.clone(),
            },
            subject: h.row.subject_or_none(),
            date: h.row.date,
            source: source_of(h.f.source),
            related,
            status,
        }
    }

    /// "In 6 days", "Tomorrow", "Today", "3 weeks ago".
    pub(super) fn when_label(&self, at: &str) -> Option<String> {
        let (d, _) = parse_at(at)?;
        Some(capitalize(&relative(d, self.today)))
    }
}

/// Group facts that describe the same thing (a booking re-sent, an order's
/// confirmation and receipt): newest email first in each group.
pub(super) fn group(hits: &[Hit], key: impl Fn(&Hit) -> Option<String>) -> Vec<Vec<&Hit>> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&Hit>> = HashMap::new();
    for (i, h) in hits.iter().enumerate() {
        let k = key(h).unwrap_or_else(|| format!("#{i}"));
        if !groups.contains_key(&k) {
            order.push(k.clone());
        }
        groups.entry(k).or_default().push(h);
    }
    order
        .into_iter()
        .filter_map(|k| groups.remove(&k))
        .map(|mut g| {
            g.sort_by_key(|h| std::cmp::Reverse(h.row.date));
            g
        })
        .collect()
}

pub(super) fn related(g: &[&Hit]) -> Vec<AskCite> {
    let mut v: Vec<AskCite> = Vec::new();
    for h in g.iter().skip(1) {
        let c = h.row.cite();
        if !v.contains(&c) && c != g[0].row.cite() {
            v.push(c);
        }
    }
    v
}

/// Airport codes and folded names for a place in a question.
fn place_keys(place: &str) -> (Vec<&'static str>, String) {
    (airports_for_place(place), folded(place))
}

// ------------------------------------------------------------- flights

pub(super) fn flight(
    cx: &mut Cx,
    place: Option<&str>,
    which: Which,
    field: Option<&'static str>,
) -> Result<AskAnswer> {
    let query = match place {
        Some(p) => format!("flight {p}"),
        None => "flight".into(),
    };
    let hits = cx.facts("flight", Some(&query))?;
    let (codes, pfold) = place.map(place_keys).unwrap_or_default();
    if let Some(p) = place {
        if codes.is_empty() {
            cx.steps.push(format!("\u{201c}{p}\u{201d} isn't an airport I know; matched it against airport and city names in the bookings"));
        } else {
            cx.steps
                .push(format!("\u{201c}{p}\u{201d} = {}", codes.join(", ")));
        }
    }
    let matches_place = |f: &crate::structured::Flight| -> bool {
        if place.is_none() {
            return true;
        }
        let arr = f
            .arrive_airport
            .as_deref()
            .is_some_and(|c| codes.contains(&c))
            || f.arrive_name
                .as_deref()
                .is_some_and(|n| mentions(n, &pfold));
        let dep = f
            .depart_airport
            .as_deref()
            .is_some_and(|c| codes.contains(&c))
            || f.depart_name
                .as_deref()
                .is_some_and(|n| mentions(n, &pfold));
        arr || dep
    };
    // A round trip's return leg leaves from the place: "my flight to
    // Lisbon" is the one landing there, and "from Lisbon" the one leaving.
    let words = format!(" {} ", cx.q.text);
    let leaving = place.is_some_and(|p| words.contains(&format!(" from {} ", p.to_lowercase())));
    let lands = |f: &crate::structured::Flight| {
        let (code, name) = if leaving {
            (&f.depart_airport, &f.depart_name)
        } else {
            (&f.arrive_airport, &f.arrive_name)
        };
        code.as_deref().is_some_and(|c| codes.contains(&c))
            || name.as_deref().is_some_and(|n| mentions(n, &pfold))
    };
    let sided = place.is_some()
        && hits
            .iter()
            .any(|h| matches!(&h.f.fact, Extracted::Flight(f) if lands(f) && f.depart_time.is_some()));
    let flights: Vec<&Hit> = hits
        .iter()
        .filter(|h| matches!(&h.f.fact, Extracted::Flight(f) if matches_place(f) && f.depart_time.is_some()))
        .collect();
    let owned: Vec<Hit> = flights
        .iter()
        .map(|h| Hit {
            f: h.f.clone(),
            row: h.row.clone(),
        })
        .collect();
    let groups = group(&owned, |h| match &h.f.fact {
        Extracted::Flight(f) => Some(format!(
            "{}|{}",
            f.flight_number.clone().unwrap_or_default(),
            f.depart_time
                .as_deref()
                .and_then(|t| t.get(..10))
                .unwrap_or("")
        )),
        _ => None,
    });
    let today = cx.today_iso();
    let depart = |g: &Vec<&Hit>| match &g[0].f.fact {
        Extracted::Flight(f) => f.depart_time.clone().unwrap_or_default(),
        _ => String::new(),
    };
    let mut upcoming: Vec<&Vec<&Hit>> = groups
        .iter()
        .filter(|g| depart(g).as_str() >= today.as_str())
        .collect();
    upcoming.sort_by_key(|g| depart(g));
    let mut past: Vec<&Vec<&Hit>> = groups
        .iter()
        .filter(|g| depart(g).as_str() < today.as_str())
        .collect();
    past.sort_by_key(|g| std::cmp::Reverse(depart(g)));
    // The answer is a flight landing there; the other legs of the booking
    // come after it.
    if sided {
        let landing = |g: &&Vec<&Hit>| matches!(&g[0].f.fact, Extracted::Flight(f) if lands(f));
        upcoming.sort_by_key(|g| !landing(g));
        past.sort_by_key(|g| !landing(g));
    }
    // The question's date scope ("in 2025") applies to the departure.
    let in_range = |g: &&Vec<&Hit>| {
        let Some((d, _)) = parse_at(&depart(g)) else {
            return false;
        };
        let ms = day_ms(d, cx.off);
        cx.lo.is_none_or(|lo| ms >= lo) && cx.hi.is_none_or(|hi| ms < hi)
    };
    upcoming.retain(in_range);
    past.retain(in_range);
    let chosen: Vec<&Vec<&Hit>> = match which {
        Which::Next => upcoming.clone(),
        Which::Last => past.clone(),
        Which::Any => {
            if upcoming.is_empty() {
                past.iter().take(1).copied().collect()
            } else {
                upcoming.clone()
            }
        }
    };
    let mut a = cx.answer(AskIntent::Flight);
    a.search_query = Some(query.clone());
    a.coverage = cx.coverage("bookings")?;
    let where_ = place
        .map(|p| format!(" to {}", capitalize(p)))
        .unwrap_or_default();
    let Some(first) = chosen.first() else {
        a.headline = match which {
            Which::Last => format!("I couldn't find a past flight{where_} in your mail"),
            _ if !past.is_empty() && place.is_some() => format!(
                "No upcoming flight{where_}; the last one was {}",
                fmt_at(&depart(past[0]), cx.today)
            ),
            _ => format!("I couldn't find an upcoming flight{where_} in your mail"),
        };
        a.confidence = AskConfidence::None;
        if let Some(g) = past.first().filter(|_| which != Which::Last) {
            a.cards.push(flight_card(cx, g));
            a.confidence = AskConfidence::Low;
        }
        if a.cards.is_empty() {
            a.items = search_items(cx, &query, 5)?;
            if !a.items.is_empty() {
                a.detail = Some(
                    "These emails mention it, but none reads as a flight booking I can use.".into(),
                );
            }
        }
        return Ok(cx.finish(a));
    };
    let Extracted::Flight(f) = &first[0].f.fact else {
        unreachable!("flight groups hold flights")
    };
    let number = f.flight_number.clone().unwrap_or_else(|| "Flight".into());
    let to = f
        .arrive_name
        .clone()
        .or(f.arrive_airport.clone())
        .map(|n| match &f.arrive_airport {
            Some(code) if !n.eq_ignore_ascii_case(code) => format!(" to {n} ({code})"),
            _ => format!(" to {n}"),
        })
        .unwrap_or_default();
    let from = f
        .depart_airport
        .clone()
        .map(|c| format!(" from {c}"))
        .unwrap_or_default();
    let when = f
        .depart_time
        .as_deref()
        .map(|t| fmt_at(t, cx.today))
        .unwrap_or_default();
    let rel = f
        .depart_time
        .as_deref()
        .and_then(parse_at)
        .map(|(d, _)| relative(d, cx.today))
        .unwrap_or_default();
    let cancelled = f.status.as_deref() == Some("cancelled");
    let prefix = if cancelled { "Cancelled: " } else { "" };
    a.headline = match field {
        Some("confirmation") => match &f.confirmation {
            Some(code) => format!("Confirmation code {code}: {number}{to}, {when}"),
            None => format!(
                "{number}{to}, {when}. The booking email has no confirmation code I can read."
            ),
        },
        Some("number") => format!("{prefix}{number}{to}, {when}{from}"),
        _ => format!("{prefix}{number}{to}: {when}{from} ({rel})"),
    };
    let mut detail = Vec::new();
    if let Some(al) = &f.airline {
        detail.push(al.clone());
    }
    if let (Some(code), Some("confirmation") | None) = (&f.confirmation, field) {
        if field.is_none() {
            detail.push(format!("confirmation {code}"));
        }
    }
    if let Some(t) = &f.arrive_time {
        detail.push(format!("arrives {}", fmt_at(t, cx.today)));
    }
    if !detail.is_empty() {
        a.detail = Some(capitalize(&detail.join(" · ")));
    }
    // The whole trip: other segments of the same booking, then the rest.
    // The whole trip (other legs of the same booking), then other
    // bookings that also match: two upcoming trips to Lisbon are both shown.
    let conf = f.confirmation.clone();
    let same = |g: &&Vec<&Hit>| matches!(&g[0].f.fact, Extracted::Flight(x) if x.confirmation.is_some() && x.confirmation == conf);
    let mut shown: Vec<&Vec<&Hit>> = vec![first];
    shown.extend(chosen.iter().skip(1).filter(|g| same(g)).take(3));
    let others: Vec<&Vec<&Hit>> = chosen
        .iter()
        .skip(1)
        .filter(|g| !same(g))
        .copied()
        .collect();
    if which != Which::Last {
        shown.extend(others.iter().take(4usize.saturating_sub(shown.len())));
    }
    let other_bookings = others.len();
    if other_bookings > 0 && which != Which::Last {
        let more = format!(
            "{} more upcoming{}",
            plural(other_bookings, "flight", "flights"),
            place
                .map(|p| format!(" to {}", capitalize(p)))
                .unwrap_or_default()
        );
        a.detail = Some(match a.detail.take() {
            Some(d) => format!("{d} · {more}"),
            None => capitalize(&more),
        });
    }
    for g in &shown {
        a.cards.push(flight_card(cx, g));
    }
    a.items = shown.iter().map(|g| g[0].row.item(None)).collect();
    a.confidence = if first[0].f.source != 3 {
        AskConfidence::High
    } else {
        AskConfidence::Medium
    };
    if let Some(dest) = f.arrive_name.clone().or(f.arrive_airport.clone()) {
        a.followups.push(suggest(
            &format!("Where am I staying in {dest}?"),
            format!("Where am I staying in {dest}"),
        ));
    }
    if field != Some("confirmation") && f.confirmation.is_some() {
        a.followups.push(suggest(
            "Confirmation code",
            format!("What's my confirmation code for the flight{where_}"),
        ));
    }
    a.followups.push(suggest(
        "All upcoming flights",
        "My upcoming flights".into(),
    ));
    Ok(cx.finish(a))
}

fn flight_card(cx: &Cx, g: &[&Hit]) -> AskCard {
    let status = match &g[0].f.fact {
        Extracted::Flight(f) if f.status.as_deref() == Some("cancelled") => {
            Some("Cancelled".into())
        }
        Extracted::Flight(f) => f.depart_time.as_deref().and_then(|t| cx.when_label(t)),
        _ => None,
    };
    cx.card(g[0], related(g), status)
}

// --------------------------------------------------------------- stays

pub(super) fn stay(cx: &mut Cx, place: Option<&str>, which: Which) -> Result<AskAnswer> {
    let query = match place {
        Some(p) => format!("(reservation OR booking OR hotel OR stay OR {p})"),
        None => "(reservation OR booking OR hotel OR stay OR checkin)".into(),
    };
    let hits = cx.facts("lodging", Some(&query))?;
    // A place matches the stay's name or address, in any of the city's
    // names ("Lisbon" ~ "Lisboa"), or a stay that starts when a flight to
    // that place lands (a guesthouse booking that never names the city).
    let mut names: Vec<String> = place.map(|p| vec![folded(p)]).unwrap_or_default();
    let codes = place.map(airports_for_place).unwrap_or_default();
    for code in &codes {
        names.extend(airport_names(code).into_iter().map(folded));
    }
    let mut arrivals: Vec<NaiveDate> = Vec::new();
    if !codes.is_empty() {
        let saved = cx.steps.len();
        for h in cx.facts("flight", None)? {
            if let Extracted::Flight(f) = &h.f.fact {
                if f.arrive_airport
                    .as_deref()
                    .is_some_and(|c| codes.contains(&c))
                {
                    for t in [&f.depart_time, &f.arrive_time].into_iter().flatten() {
                        if let Some((d, _)) = parse_at(t) {
                            arrivals.push(d);
                        }
                    }
                }
            }
        }
        cx.steps.truncate(saved);
        if !arrivals.is_empty() {
            cx.steps.push(format!(
                "Also matched stays starting within a day of {} to {}",
                plural(arrivals.len(), "flight date", "flight dates"),
                codes.join("/")
            ));
        }
    }
    let matches = |l: &crate::structured::Lodging| {
        place.is_none()
            || names.iter().any(|n| {
                l.name.as_deref().is_some_and(|x| mentions(x, n))
                    || l.address.as_deref().is_some_and(|x| mentions(x, n))
            })
            || l.checkin
                .as_deref()
                .and_then(parse_at)
                .is_some_and(|(d, _)| arrivals.iter().any(|a| (d - *a).num_days().abs() <= 1))
    };
    let owned: Vec<Hit> = hits
        .into_iter()
        .filter(|h| matches!(&h.f.fact, Extracted::Lodging(l) if matches(l) && l.checkin.is_some()))
        .collect();
    let groups = group(&owned, |h| match &h.f.fact {
        Extracted::Lodging(l) => l.confirmation.clone().or_else(|| {
            Some(format!(
                "{}|{}",
                l.name.clone().unwrap_or_default(),
                l.checkin.clone().unwrap_or_default()
            ))
        }),
        _ => None,
    });
    let today = cx.today_iso();
    let key = |g: &Vec<&Hit>| match &g[0].f.fact {
        Extracted::Lodging(l) => (
            l.checkin.clone().unwrap_or_default(),
            l.checkout.clone().unwrap_or_default(),
        ),
        _ => Default::default(),
    };
    // Current or upcoming: checkout today or later.
    let mut upcoming: Vec<&Vec<&Hit>> = groups
        .iter()
        .filter(|g| key(g).1.as_str() >= today.as_str())
        .collect();
    upcoming.sort_by_key(|g| key(g).0);
    let mut past: Vec<&Vec<&Hit>> = groups
        .iter()
        .filter(|g| key(g).1.as_str() < today.as_str())
        .collect();
    past.sort_by_key(|g| std::cmp::Reverse(key(g).0));
    let chosen = match which {
        Which::Last => past.clone(),
        Which::Next => upcoming.clone(),
        Which::Any if upcoming.is_empty() => past.iter().take(1).copied().collect(),
        Which::Any => upcoming.clone(),
    };
    let mut a = cx.answer(AskIntent::Stay);
    a.search_query = Some(query.clone());
    a.coverage = cx.coverage("bookings")?;
    let in_ = place
        .map(|p| format!(" in {}", capitalize(p)))
        .unwrap_or_default();
    let Some(first) = chosen.first() else {
        a.headline = format!("I couldn't find a hotel or rental booking{in_} in your mail");
        a.confidence = AskConfidence::None;
        a.items = search_items(cx, &query, 5)?;
        return Ok(cx.finish(a));
    };
    let Extracted::Lodging(l) = &first[0].f.fact else {
        unreachable!("lodging groups hold stays")
    };
    let name = l.name.clone().unwrap_or_else(|| "Your stay".into());
    let cin = l
        .checkin
        .as_deref()
        .map(|t| fmt_at(t, cx.today))
        .unwrap_or_default();
    let cout = l
        .checkout
        .as_deref()
        .map(|t| fmt_at(t, cx.today))
        .unwrap_or_default();
    let rel = l
        .checkin
        .as_deref()
        .and_then(parse_at)
        .map(|(d, _)| relative(d, cx.today))
        .unwrap_or_default();
    a.headline = if l.status.as_deref() == Some("cancelled") {
        format!("Cancelled: {name}, {cin} – {cout}")
    } else {
        format!("{name}: check in {cin}, check out {cout} ({rel})")
    };
    let mut detail = Vec::new();
    if let Some(ad) = &l.address {
        detail.push(ad.clone());
    }
    if let Some(c) = &l.confirmation {
        detail.push(format!("confirmation {c}"));
    }
    if !detail.is_empty() {
        a.detail = Some(capitalize(&detail.join(" · ")));
    }
    for g in chosen.iter().take(3) {
        let status = match &g[0].f.fact {
            Extracted::Lodging(l) if l.status.as_deref() == Some("cancelled") => {
                Some("Cancelled".into())
            }
            Extracted::Lodging(l) => l.checkin.as_deref().and_then(|t| cx.when_label(t)),
            _ => None,
        };
        a.cards.push(cx.card(g[0], related(g), status));
        a.items.push(g[0].row.item(None));
    }
    a.confidence = if first[0].f.source != 3 {
        AskConfidence::High
    } else {
        AskConfidence::Medium
    };
    a.followups
        .push(suggest("My flights", "My upcoming flights".into()));
    Ok(cx.finish(a))
}

// ------------------------------------------------------------- parcels

pub(super) fn matches_thing(h: &Hit, what: &str) -> bool {
    let w = folded(what);
    if w.is_empty() {
        return true;
    }
    let f = &h.f.fact;
    let mut hay = vec![
        h.f.key.clone(),
        h.row.from_email.clone(),
        h.row.subject.clone(),
    ];
    if let Some(n) = &h.row.from_name {
        hay.push(n.clone());
    }
    match f {
        Extracted::Shipment(s) => {
            hay.extend(s.merchant.clone());
            hay.extend(s.carrier.clone());
            hay.extend(s.items.clone());
            hay.extend(s.order_number.clone());
            hay.extend(s.tracking_number.clone());
        }
        Extracted::Order(o) => {
            hay.extend(o.merchant.clone());
            hay.extend(o.items.clone());
            hay.extend(o.order_number.clone());
        }
        Extracted::Bill(b) => {
            hay.extend(b.biller.clone());
            hay.extend(b.invoice_number.clone());
        }
        Extracted::Reservation(r) => {
            hay.extend(r.name.clone());
            hay.extend(r.venue.clone());
        }
        _ => {}
    }
    hay.iter()
        .any(|x| mentions(x, &w) || folded(x).split(' ').any(|t| t == w))
        || h.row
            .from_email
            .to_lowercase()
            .contains(&w.replace(' ', ""))
}

fn ship_label(status: Option<&str>) -> &'static str {
    match status {
        Some("delivered") => "Delivered",
        Some("outForDelivery") => "Out for delivery",
        Some("inTransit") => "In transit",
        Some("exception") => "Delivery problem",
        Some("shipped") => "Shipped",
        _ => "On its way",
    }
}

pub(super) fn package(cx: &mut Cx, what: Option<&str>) -> Result<AskAnswer> {
    let query = match what {
        Some(w) => format!("{w} (shipped OR tracking OR delivery OR delivered)"),
        None => "(shipped OR tracking OR delivery OR delivered OR package)".into(),
    };
    let hits = cx.facts("shipment", Some(&query))?;
    let owned: Vec<Hit> = hits
        .into_iter()
        .filter(|h| what.is_none_or(|w| matches_thing(h, w)))
        .collect();
    let groups = group(&owned, |h| h.f.reference.clone());
    let mut a = cx.answer(AskIntent::Package);
    a.search_query = Some(query.clone());
    a.coverage = cx.coverage("shipping emails")?;
    // Without a name: parcels still moving from the last 30 days, then
    // deliveries from the last week.
    let recent = cx.now - 30 * DAY;
    let mut shown: Vec<&Vec<&Hit>> = groups
        .iter()
        .filter(|g| what.is_some() || g[0].row.date >= recent)
        .collect();
    let delivered = |g: &&Vec<&Hit>| matches!(&g[0].f.fact, Extracted::Shipment(s) if s.status.as_deref() == Some("delivered"));
    if what.is_none() {
        shown.retain(|g| !delivered(g) || g[0].row.date >= cx.now - 7 * DAY);
    }
    shown.sort_by_key(|g| (delivered(g), std::cmp::Reverse(g[0].row.date)));
    let name = what.map(capitalize);
    let Some(first) = shown.first() else {
        // An order without a shipping email yet?
        if let Some(w) = what {
            let orders = cx.facts("order", Some(w))?;
            if let Some(o) = orders.iter().find(|h| matches_thing(h, w)) {
                let Extracted::Order(ord) = &o.f.fact else {
                    unreachable!()
                };
                a.headline = format!(
                    "No shipping email yet for your {} order{} from {}",
                    name.clone().unwrap_or_default(),
                    ord.order_number
                        .as_deref()
                        .map(|n| format!(" {n}"))
                        .unwrap_or_default(),
                    fmt_day(o.row.date, cx.off)
                );
                a.cards.push(cx.card(o, vec![], None));
                a.items.push(o.row.item(None));
                a.confidence = AskConfidence::Medium;
                return Ok(cx.finish(a));
            }
        }
        a.headline = match &name {
            Some(n) => format!("I couldn't find a shipment for \u{201c}{n}\u{201d} in your mail"),
            None => "No packages on the way in the last 30 days".into(),
        };
        a.confidence = AskConfidence::None;
        if what.is_some() {
            a.items = search_items(cx, &query, 5)?;
        }
        return Ok(cx.finish(a));
    };
    let Extracted::Shipment(s) = &first[0].f.fact else {
        unreachable!("shipment groups hold shipments")
    };
    let who = s
        .merchant
        .clone()
        .or(name.clone())
        .map(|m| format!("your {m} order"))
        .unwrap_or_else(|| "your package".into());
    let carrier = s.carrier.clone().unwrap_or_else(|| "the carrier".into());
    a.headline = match s.status.as_deref() {
        Some("delivered") => format!(
            "{}: {who} was delivered ({carrier}, {})",
            "Delivered",
            fmt_day(first[0].row.date, cx.off)
        ),
        Some("outForDelivery") => {
            format!("{} is out for delivery with {carrier}", capitalize(&who))
        }
        Some("exception") => format!("{} has a delivery problem ({carrier})", capitalize(&who)),
        _ => match &s.expected {
            Some(e) => format!(
                "{} is on its way with {carrier}, arriving {}",
                capitalize(&who),
                fmt_at(e, cx.today)
            ),
            None => format!(
                "{} shipped with {carrier} on {}",
                capitalize(&who),
                fmt_day(first[0].row.date, cx.off)
            ),
        },
    };
    if let Some(n) = &s.tracking_number {
        a.detail = Some(format!(
            "Tracking {n}{}",
            if s.verified {
                ""
            } else {
                " (as written in the email)"
            }
        ));
    }
    for g in shown.iter().take(if what.is_some() { 3 } else { 6 }) {
        let status = match &g[0].f.fact {
            Extracted::Shipment(x) => Some(match (&x.status.as_deref(), &x.expected) {
                (Some("delivered"), _) => "Delivered".to_string(),
                (_, Some(e)) if x.status.as_deref() != Some("outForDelivery") => {
                    format!("Arriving {}", fmt_at(e, cx.today))
                }
                (st, _) => ship_label(*st).to_string(),
            }),
            _ => None,
        };
        a.cards.push(cx.card(g[0], related(g), status));
        a.items.push(g[0].row.item(None));
    }
    if shown.len() > 1 && what.is_none() {
        let moving = shown.iter().filter(|g| !delivered(g)).count();
        a.headline = format!(
            "{} on the way{}; latest: {}",
            plural(moving, "package", "packages"),
            if shown.len() > moving {
                format!(", {} delivered this week", shown.len() - moving)
            } else {
                String::new()
            },
            a.headline
        );
    }
    a.confidence = if first[0].f.source != 3 || s.verified {
        AskConfidence::High
    } else {
        AskConfidence::Medium
    };
    Ok(cx.finish(a))
}

// -------------------------------------------------------------- orders

pub(super) fn orders(cx: &mut Cx, merchant: Option<&str>) -> Result<AskAnswer> {
    let q = merchant.map_or_else(
        || "(order OR receipt OR purchase)".to_string(),
        |m| m.to_string(),
    );
    let hits = cx.facts("order", Some(&q))?;
    let owned: Vec<Hit> = hits
        .into_iter()
        .filter(|h| merchant.is_none_or(|m| matches_thing(h, m)))
        .filter(|h| {
            cx.lo.is_none_or(|lo| h.row.date >= lo) && cx.hi.is_none_or(|hi| h.row.date < hi)
        })
        .collect();
    let groups = group(&owned, |h| {
        h.f.reference.clone().map(|r| format!("{}|{r}", h.f.key))
    });
    let mut a = cx.answer(AskIntent::Orders);
    a.coverage = cx.coverage("receipts")?;
    a.search_query = merchant.map(|m| format!("{m} order"));
    let label = merchant.map(capitalize);
    let Some(first) = groups.first() else {
        a.headline = match &label {
            Some(m) => format!(
                "I couldn't find an order from {m}{} in your mail",
                cx.range_phrase()
            ),
            None => format!("No orders or receipts{} in your mail", cx.range_phrase()),
        };
        a.confidence = AskConfidence::None;
        if let Some(m) = merchant {
            a.items = search_items(cx, &format!("{m} order"), 5)?;
        }
        return Ok(cx.finish(a));
    };
    let Extracted::Order(o) = &first[0].f.fact else {
        unreachable!("order groups hold orders")
    };
    let m = o
        .merchant
        .clone()
        .or(label.clone())
        .unwrap_or_else(|| "Your".into());
    let total = o
        .total
        .as_ref()
        .map(|t| format!(", {}", money(t.value, &t.currency)))
        .unwrap_or_default();
    let num = o
        .order_number
        .as_deref()
        .map(|n| format!(" {n}"))
        .unwrap_or_default();
    let items = if o.items.is_empty() {
        String::new()
    } else {
        format!(" ({})", o.items.join(", "))
    };
    a.headline = format!(
        "Latest {m} order{num}{items}{total} on {}",
        fmt_day(first[0].row.date, cx.off)
    );
    if groups.len() > 1 {
        a.detail = Some(format!(
            "{} from {}{}",
            plural(groups.len(), "order", "orders"),
            label.clone().unwrap_or_else(|| "all merchants".into()),
            cx.range_phrase()
        ));
    }
    for g in groups.iter().take(MAX_ITEMS) {
        let status = match &g[0].f.fact {
            Extracted::Order(o) => o.status.as_deref().map(capitalize),
            _ => None,
        };
        a.cards.push(cx.card(g[0], related(g), status));
        let mut it = g[0].row.item(None);
        if let (Some(v), Some(c)) = (g[0].f.amount, &g[0].f.currency) {
            it.amount = Some(AskAmount {
                value: v,
                currency: c.clone(),
                source: match &g[0].f.fact {
                    Extracted::Order(o) => o.total_source.clone().unwrap_or_default(),
                    _ => String::new(),
                },
            });
        }
        a.items.push(it);
    }
    a.confidence = AskConfidence::High;
    if let Some(mm) = merchant {
        a.followups
            .push(suggest("Where is it?", format!("Where is my {mm} order")));
        a.followups.push(suggest(
            "Total spent",
            format!("How much did I spend on {mm} this year"),
        ));
    }
    Ok(cx.finish(a))
}

// --------------------------------------------------------------- bills

pub(super) fn bills(cx: &mut Cx, what: Option<&str>) -> Result<AskAnswer> {
    let q = match what {
        Some(w) => format!("{w} (invoice OR bill OR statement OR due OR factura)"),
        None => "(invoice OR bill OR statement OR factura OR balance)".into(),
    };
    let hits = cx.facts("bill", Some(&q))?;
    let owned: Vec<Hit> = hits
        .into_iter()
        .filter(|h| what.is_none_or(|w| matches_thing(h, w)))
        .collect();
    // Paid: a payment email from the same biller for the same invoice
    // number or amount, sent after the bill.
    let paid_after = |h: &Hit| -> bool {
        let Extracted::Bill(b) = &h.f.fact else {
            return false;
        };
        if b.status.as_deref() == Some("paid") {
            return true;
        }
        owned.iter().any(|p| {
            let Extracted::Bill(pb) = &p.f.fact else {
                return false;
            };
            pb.status.as_deref() == Some("paid")
                && p.f.key == h.f.key
                && p.row.date >= h.row.date
                && ((pb.invoice_number.is_some() && pb.invoice_number == b.invoice_number)
                    || (pb.amount_due.is_some() && pb.amount_due == b.amount_due))
        })
    };
    let today = cx.today_iso();
    let mut due: Vec<&Hit> = owned
        .iter()
        .filter(|h| matches!(&h.f.fact, Extracted::Bill(b) if b.status.as_deref() != Some("paid")))
        .filter(|h| !paid_after(h))
        .collect();
    // Upcoming (or overdue in the last two weeks), soonest first; bills
    // without a due date go last, newest first.
    let cutoff = (cx.today - chrono::Duration::days(14))
        .format("%Y-%m-%d")
        .to_string();
    due.retain(|h| h.f.at.as_deref().is_none_or(|d| d >= cutoff.as_str()));
    if what.is_none() {
        due.retain(|h| h.f.at.is_some() || h.row.date >= cx.now - 45 * DAY);
    }
    due.sort_by(|x, y| match (&x.f.at, &y.f.at) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => y.row.date.cmp(&x.row.date),
    });
    // One per biller + invoice.
    let mut seen: Vec<String> = Vec::new();
    due.retain(|h| {
        let k = format!(
            "{}|{}",
            h.f.key,
            h.f.reference
                .clone()
                .unwrap_or_else(|| h.f.at.clone().unwrap_or_default())
        );
        if seen.contains(&k) {
            false
        } else {
            seen.push(k);
            true
        }
    });
    let mut a = cx.answer(AskIntent::Bills);
    a.coverage = cx.coverage("bills")?;
    a.search_query = Some(match what {
        Some(w) => format!("{w} invoice OR {w} bill"),
        None => "invoice OR bill OR statement".into(),
    });
    let describe = |h: &Hit| -> String {
        let Extracted::Bill(b) = &h.f.fact else {
            return String::new();
        };
        let who = b.biller.clone().unwrap_or_else(|| h.f.key.clone());
        let amt = b
            .amount_due
            .as_ref()
            .map(|m| format!(" {}", money(m.value, &m.currency)))
            .unwrap_or_default();
        match &b.due_date {
            Some(d) => {
                let (dd, _) = parse_at(d).unwrap_or((cx.today, None));
                let overdue = d.as_str() < today.as_str();
                format!(
                    "{who}:{amt} {} {} ({})",
                    if overdue { "was due" } else { "due" },
                    fmt_at(d, cx.today),
                    relative(dd, cx.today)
                )
            }
            None => format!("{who}:{amt} (no due date in the email)"),
        }
    };
    if due.is_empty() {
        // Maybe the named bill is already paid.
        if let Some(p) = what.and_then(|_| owned.iter().find(|h| paid_after(h))) {
            a.headline = format!("Paid: {}", describe(p));
            a.cards.push(cx.card(p, vec![], Some("Paid".into())));
            a.items.push(p.row.item(None));
            a.confidence = AskConfidence::Medium;
            return Ok(cx.finish(a));
        }
        a.headline = match what {
            Some(w) => format!("I couldn't find a bill from {} that's due", capitalize(w)),
            None => "No bills due in your mail".into(),
        };
        a.confidence = AskConfidence::None;
        if let Some(w) = what {
            a.items = search_items(cx, &format!("{w} invoice"), 5)?;
        }
        return Ok(cx.finish(a));
    }
    a.headline = if due.len() == 1 || what.is_some() {
        describe(due[0])
    } else {
        format!(
            "{} due; next: {}",
            plural(due.len(), "bill", "bills"),
            describe(due[0])
        )
    };
    let mut totals: Vec<(String, f64)> = Vec::new();
    for h in &due {
        if let (Some(v), Some(c)) = (h.f.amount, &h.f.currency) {
            match totals.iter_mut().find(|t| &t.0 == c) {
                Some(t) => t.1 += v,
                None => totals.push((c.clone(), v)),
            }
        }
        let status = match &h.f.fact {
            Extracted::Bill(b) => b.due_date.as_deref().map(|d| {
                if d < today.as_str() {
                    "Overdue".to_string()
                } else {
                    format!(
                        "Due {}",
                        relative(parse_at(d).map(|x| x.0).unwrap_or(cx.today), cx.today)
                    )
                }
            }),
            _ => None,
        };
        a.cards.push(cx.card(h, vec![], status));
        let mut it = h.row.item(None);
        if let (Some(v), Some(c)) = (h.f.amount, &h.f.currency) {
            it.amount = Some(AskAmount {
                value: v,
                currency: c.clone(),
                source: match &h.f.fact {
                    Extracted::Bill(b) => b.amount_source.clone().unwrap_or_default(),
                    _ => String::new(),
                },
            });
        }
        a.items.push(it);
    }
    if due.len() > 1 && !totals.is_empty() {
        a.detail = Some(format!(
            "{} in total",
            totals
                .iter()
                .map(|(c, v)| money(*v, c))
                .collect::<Vec<_>>()
                .join(" + ")
        ));
    }
    a.steps.push("A bill counts as paid when a later email from the same biller says so for the same invoice number or amount".into());
    a.confidence = AskConfidence::Medium;
    Ok(cx.finish(a))
}

// ------------------------------------------------------ "when is …"

/// "When is my flight / the dinner reservation / the Brightwave bill": the
/// date of an extracted booking or bill the words name. None when nothing
/// extracted matches (the caller reads dates out of the text instead).
pub(super) fn when_from_facts(cx: &mut Cx, words: &[String]) -> Result<Option<AskAnswer>> {
    const FLIGHT: &[&str] = &["flight", "flights", "fly", "plane", "vuelo"];
    const STAY: &[&str] = &["hotel", "stay", "checkin", "airbnb", "hotels"];
    const BILL: &[&str] = &[
        "bill",
        "bills",
        "invoice",
        "payment",
        "statement",
        "factura",
        "recibo",
    ];
    const BOOK: &[&str] = &[
        "dinner",
        "lunch",
        "brunch",
        "reservation",
        "table",
        "concert",
        "show",
        "game",
        "tickets",
        "ticket",
        "reserva",
        "cena",
    ];
    let has = |xs: &[&str]| words.iter().any(|w| xs.contains(&w.as_str()));
    let rest: Vec<String> = words
        .iter()
        .filter(|w| {
            ![FLIGHT, STAY, BILL, BOOK]
                .iter()
                .any(|xs| xs.contains(&w.as_str()))
        })
        .filter(|w| !matches!(w.as_str(), "next" | "upcoming" | "due" | "my" | "the"))
        .cloned()
        .collect();
    let rest_s = rest.join(" ");
    let what = (!rest_s.is_empty()).then_some(rest_s.as_str());
    let saved = cx.steps.clone();
    let a = if has(FLIGHT) {
        flight(cx, what, Which::Any, None)?
    } else if has(STAY) {
        stay(cx, what, Which::Any)?
    } else if has(BILL) {
        bills(cx, what)?
    } else if has(BOOK) {
        booking(cx, what)?
    } else if !words.is_empty() {
        // Any upcoming booking named by the words ("the Lanterns").
        let all = words.join(" ");
        booking(cx, Some(all.as_str()))?
    } else {
        return Ok(None);
    };
    if a.confidence == AskConfidence::None || (a.cards.is_empty() && a.intent != AskIntent::Bills) {
        cx.steps = saved;
        return Ok(None);
    }
    Ok(Some(a))
}

// ------------------------------------------------------------ bookings

pub(super) fn booking(cx: &mut Cx, what: Option<&str>) -> Result<AskAnswer> {
    // "When is my dinner reservation": any restaurant table, not one named
    // "dinner".
    let dining = what.is_some_and(|w| {
        matches!(w, "dinner" | "lunch" | "brunch" | "breakfast" | "restaurant" | "table")
    });
    let meal = what.filter(|_| dining);
    let what = what.filter(|_| !dining);
    let q = match what {
        Some(w) => format!("{w} (reservation OR tickets OR booked OR reserva)"),
        None => "(reservation OR tickets OR table OR reserva OR booked)".into(),
    };
    let hits = cx.facts("reservation", Some(&q))?;
    let owned: Vec<Hit> = hits
        .into_iter()
        .filter(|h| what.is_none_or(|w| matches_thing(h, w)))
        .filter(|h| {
            meal.is_none()
                || matches!(&h.f.fact, Extracted::Reservation(r) if r.category == "restaurant")
        })
        .collect();
    let groups = group(&owned, |h| {
        h.f.reference.clone().or_else(|| {
            Some(format!(
                "{}|{}",
                h.f.key,
                h.f.at.clone().unwrap_or_default()
            ))
        })
    });
    let today = cx.today_iso();
    let start = |g: &Vec<&Hit>| g[0].f.at.clone().unwrap_or_default();
    let mut upcoming: Vec<&Vec<&Hit>> = groups
        .iter()
        .filter(|g| start(g).as_str() >= today.as_str())
        .collect();
    upcoming.sort_by_key(|g| start(g));
    let mut a = cx.answer(AskIntent::Booking);
    a.coverage = cx.coverage("bookings")?;
    let chosen: Vec<&Vec<&Hit>> = if upcoming.is_empty() && (what.is_some() || meal.is_some()) {
        let mut past: Vec<&Vec<&Hit>> = groups.iter().collect();
        past.sort_by_key(|g| std::cmp::Reverse(start(g)));
        past.into_iter().take(1).collect()
    } else {
        upcoming
    };
    let Some(first) = chosen.first() else {
        a.headline = match what {
            Some(w) => format!(
                "I couldn't find a reservation or tickets for {}",
                capitalize(w)
            ),
            None => "No upcoming reservations or tickets in your mail".into(),
        };
        a.confidence = AskConfidence::None;
        if let Some(w) = what {
            a.items = search_items(cx, &format!("{w} reservation"), 5)?;
        }
        return Ok(cx.finish(a));
    };
    let Extracted::Reservation(r) = &first[0].f.fact else {
        unreachable!("reservation groups hold reservations")
    };
    let name = r.name.clone().unwrap_or_else(|| "Your reservation".into());
    let when = r
        .start
        .as_deref()
        .map(|t| fmt_at(t, cx.today))
        .unwrap_or_default();
    let rel = r
        .start
        .as_deref()
        .and_then(parse_at)
        .map(|(d, _)| relative(d, cx.today))
        .unwrap_or_default();
    let party = r
        .party_size
        .map(|n| format!(", {}", plural(n as usize, "person", "people")))
        .unwrap_or_default();
    a.headline = format!("{name}: {when} ({rel}){party}");
    let mut detail = Vec::new();
    if let Some(v) = &r.venue {
        if Some(v) != r.name.as_ref() {
            detail.push(v.clone());
        }
    }
    if let Some(ad) = &r.address {
        detail.push(ad.clone());
    }
    if let Some(c) = &r.confirmation {
        detail.push(format!("confirmation {c}"));
    }
    if !detail.is_empty() {
        a.detail = Some(detail.join(" · "));
    }
    for g in chosen.iter().take(5) {
        let status = g[0].f.at.as_deref().and_then(|t| cx.when_label(t));
        a.cards.push(cx.card(g[0], related(g), status));
        a.items.push(g[0].row.item(None));
    }
    a.confidence = if first[0].f.source != 3 {
        AskConfidence::High
    } else {
        AskConfidence::Medium
    };
    Ok(cx.finish(a))
}

// -------------------------------------------------------------- codes

/// The latest verification code (`messages.otp`, detected at ingest by
/// `crate::otp`). The code itself is only shown when the scope allows it
/// (the app; never the CLI or MCP).
pub(super) fn code(cx: &mut Cx, service: Option<&str>) -> Result<AskAnswer> {
    let since = (cx.now - 30 * DAY).saturating_mul(crate::store::ROWID_SLOTS);
    let rows: Vec<(Row, String)> = cx.store.read(|c| {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {ROW_COLS}, m.otp FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
             WHERE m.rowid >= ?1 AND m.otp IS NOT NULL AND m.otp != '' AND m.flags & ?2 = 0
             ORDER BY m.rowid DESC LIMIT 200"
        ))?;
        let v = stmt
            .query_map(params![since, HIDDEN], |r| {
                Ok((read_row(r)?, r.get::<_, String>(11)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    })?;
    let rows: Vec<(Row, crate::types::Otp)> = rows
        .into_iter()
        .filter(|(r, _)| cx.in_scope(&r.account_id))
        .filter(|(r, _)| {
            service.is_none_or(|s| {
                mentions(
                    &format!(
                        "{} {} {}",
                        r.from_name.clone().unwrap_or_default(),
                        r.from_email,
                        r.subject
                    ),
                    s,
                ) || r
                    .from_email
                    .to_lowercase()
                    .contains(&folded(s).replace(' ', ""))
            })
        })
        .filter_map(|(r, j)| serde_json::from_str(&j).ok().map(|o| (r, o)))
        .collect();
    cx.steps.push(format!(
        "Looked at verification codes detected in the last 30 days{}",
        service
            .map(|s| format!(" from \u{201c}{s}\u{201d}"))
            .unwrap_or_default()
    ));
    let mut a = cx.answer(AskIntent::Code);
    let Some((r, otp)) = rows.first() else {
        a.headline = match service {
            Some(s) => format!(
                "No verification code from {} in the last 30 days",
                capitalize(s)
            ),
            None => "No verification codes in the last 30 days".into(),
        };
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    };
    let who = r
        .from_name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| r.from_email.clone());
    let age = {
        let mins = (cx.now - r.date).max(0) / 60_000;
        match mins {
            0 => "just now".to_string(),
            1..=59 => format!("{mins} min ago"),
            60..=1439 => format!("{} h ago", mins / 60),
            _ => format!("{} ago", relative(local_date(r.date, cx.off), cx.today)),
        }
    };
    let code_text = otp.code.clone().filter(|_| cx.ask_scope.reveal_codes);
    a.headline = match (&code_text, &otp.kind) {
        (Some(code), _) => format!("{code} from {who} ({age})"),
        (None, crate::types::OtpKind::Link) => format!("A sign-in link from {who} ({age})"),
        (None, _) => format!("Your latest code is from {who} ({age}); open the email to see it"),
    };
    if !otp.verified {
        a.detail = Some("The sender couldn't be verified (no DMARC/DKIM pass). Check it's really them before using it.".into());
    } else if cx.now - r.date > 20 * 60_000 {
        a.detail = Some("It may have expired: most codes last 10–20 minutes.".into());
    }
    a.items = rows.iter().take(3).map(|(r, _)| r.item(None)).collect();
    a.confidence = AskConfidence::High;
    Ok(cx.finish(a))
}

// ------------------------------------------------------ contact details

/// One of a person's addresses and the mail you've exchanged at it (the
/// people index: every message stored, all accounts).
struct AddressUse {
    email: String,
    from_them: i64,
    to_them: i64,
    last: i64,
}

fn address_uses(c: &Connection, emails: &[String]) -> Result<Vec<AddressUse>> {
    let mut stmt = c.prepare_cached("SELECT from_count, sent_to_count, last_date FROM people WHERE email = ?1 COLLATE NOCASE")?;
    let mut out = Vec::new();
    for e in emails {
        if let Some((f, t, l)) = stmt.query_row([e], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).optional()? {
            if f + t > 0 {
                out.push(AddressUse { email: e.clone(), from_them: f, to_them: t, last: l });
            }
        }
    }
    out.sort_by(|a, b| (b.from_them + b.to_them).cmp(&(a.from_them + a.to_them)).then(b.last.cmp(&a.last)));
    Ok(out)
}

/// The latest message from one of `emails`, to cite.
fn latest_from(c: &Connection, emails: &[String]) -> Result<Option<Row>> {
    let mut stmt = c.prepare_cached(&format!(
        "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
         WHERE m.from_email = ?1 COLLATE NOCASE AND m.flags & ?2 = 0 ORDER BY m.date DESC LIMIT 1"
    ))?;
    let mut best: Option<Row> = None;
    for e in emails {
        if let Some(r) = stmt.query_row(params![e, HIDDEN], read_row).optional()? {
            if best.as_ref().is_none_or(|b| r.date > b.date) {
                best = Some(r);
            }
        }
    }
    Ok(best)
}

/// "What's Priya's email", "how do I reach Dana": the addresses you've
/// actually exchanged mail with for them (from the people index), and the
/// phone and postal address their signatures give. A name several people
/// share ("Mike") lists each of them instead of picking one.
fn contact_details(cx: &mut Cx, who: &str, field: &'static str) -> Result<AskAnswer> {
    let (t, candidates) = match resolve_who(cx, who, AskIntent::ContactInfo)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    let namesakes = std::mem::take(&mut cx.namesakes);
    let mut a = cx.answer(AskIntent::ContactInfo);
    a.candidates = candidates;
    let mut result = AskResult::new(AskResultKind::Item);
    if !namesakes.is_empty() && t.kind == TargetKind::Person {
        let people: Vec<&Target> = std::iter::once(&t).chain(namesakes.iter()).collect();
        let mut listed: Vec<String> = Vec::new();
        let mut all: Vec<String> = Vec::new();
        for p in &people {
            let uses = cx.store.read(|c| address_uses(c, &p.emails))?;
            let emails: Vec<String> = if uses.is_empty() { p.emails.iter().take(1).cloned().collect() } else { uses.iter().map(|u| u.email.clone()).collect() };
            let n: i64 = uses.iter().map(|u| u.from_them + u.to_them).sum();
            listed.push(format!("{} <{}>", p.label, emails.join(", ")));
            let mut f = cx.fact(&p.label, format!("{} \u{b7} {}", emails.join(", "), plural(n as usize, "email", "emails")));
            if let Some(r) = cx.store.read(|c| latest_from(c, &p.emails))? {
                f.cite = Some(r.cite());
                f.date = Some(r.date);
                a.items.push(r.item(None));
            }
            a.facts.push(f);
            all.extend(emails);
        }
        let typed = who.split_whitespace().map(capitalize).collect::<Vec<_>>().join(" ");
        a.headline = format!(
            "{} people match \u{201c}{typed}\u{201d}: {}",
            people.len(),
            listed.join(" and ")
        );
        // A chip for each, the first one too (nothing was picked).
        let key = t.emails.first().cloned().unwrap_or_default();
        let label = format!("{} <{key}>", t.label);
        if !a.candidates.iter().any(|c| c.label == label) {
            let q = capitalize(&cx.q.text.replacen(who, &key, 1));
            a.candidates.insert(0, AskSuggestion { label, question: q });
        }
        a.detail = Some("Pick one below to see only theirs.".into());
        a.confidence = AskConfidence::Medium;
        result.text = Some(all.join(", "));
        a.result = Some(result);
        return Ok(cx.finish(a));
    }
    a.person = Some(person_of(&t));
    a.followups = person_followups(&t, AskIntent::ContactInfo);
    let name = t.name.clone().unwrap_or(t.label.clone());
    let emails: Vec<String> = if t.kind == TargetKind::Company {
        // A company: the people there you've written with.
        t.emails.iter().take(40).cloned().collect()
    } else {
        t.emails.clone()
    };
    let uses = cx.store.read(|c| address_uses(c, &emails))?;
    let latest = cx.store.read(|c| latest_from(c, &emails))?;
    // Phone and postal address, as their signatures write them.
    let mut phone: Option<(String, Row)> = None;
    let mut postal: Option<(String, Row)> = None;
    if t.kind == TargetKind::Person {
        for h in contact_facts(cx, &t)? {
            let Extracted::Contact(c) = &h.f.fact else {
                continue;
            };
            if phone.is_none() {
                phone = c.phones.first().map(|p| (p.clone(), h.row.clone()));
            }
            if postal.is_none() {
                postal = c.addresses.first().map(|p| (p.clone(), h.row.clone()));
            }
        }
    }
    if uses.is_empty() {
        a.headline = format!("You haven't exchanged mail with {name} in your local mail");
        a.detail = Some(format!("Known as {}.", list_emails(&t.emails)));
        a.confidence = AskConfidence::Low;
        result.text = Some(t.emails.join(", "));
        a.result = Some(result);
        return Ok(cx.finish(a));
    }
    let shown: Vec<&AddressUse> = uses.iter().take(if t.kind == TargetKind::Company { 5 } else { 4 }).collect();
    let list: Vec<&str> = shown.iter().map(|u| u.email.as_str()).collect();
    a.headline = match (field, list.len(), &phone) {
        ("contact", _, Some((p, _))) => format!("Reach {name} at {} or {p}", list[0]),
        ("contact", _, None) => format!("Reach {name} at {}", list[0]),
        (_, 1, _) => format!("{name}'s email: {}", list[0]),
        _ => format!("{name}'s email addresses: {}", list.join(", ")),
    };
    let u = shown[0];
    a.detail = Some(format!(
        "You've exchanged {} at {} ({} from them, {} from you), the last on {}{}",
        plural((u.from_them + u.to_them) as usize, "email", "emails"),
        if shown.len() > 1 { "the first" } else { "this address" },
        u.from_them,
        u.to_them,
        fmt_day(u.last, cx.off),
        if shown.len() > 1 { "; the others are below" } else { "" }
    ));
    for u in &shown {
        let mut f = cx.fact(
            "Email",
            format!(
                "{} \u{b7} {} from them \u{b7} {} from you \u{b7} last {}",
                u.email,
                u.from_them,
                u.to_them,
                fmt_day(u.last, cx.off)
            ),
        );
        if let Some(r) = latest.as_ref().filter(|r| r.from_email.eq_ignore_ascii_case(&u.email)) {
            f.cite = Some(r.cite());
            f.date = Some(r.date);
        }
        a.facts.push(f);
    }
    if let Some((p, r)) = &phone {
        let mut f = cx.date_fact("Phone", r);
        f.value = format!("{p} \u{b7} signature, {}", fmt_day(r.date, cx.off));
        a.facts.push(f);
    }
    if let Some((ad, r)) = &postal {
        let mut f = cx.date_fact("Address", r);
        f.value = ad.clone();
        a.facts.push(f);
    }
    if let Some(r) = &latest {
        a.items.push(r.item(None));
    }
    let mut text: Vec<String> = list.iter().map(|e| e.to_string()).collect();
    if field == "contact" {
        text.extend(phone.as_ref().map(|p| p.0.clone()));
    }
    result.text = Some(text.join(", "));
    result.count = Some(uses.iter().map(|u| (u.from_them + u.to_them) as u64).sum());
    a.result = Some(result);
    a.search_query = Some(list.iter().map(|e| format!("from:{e}")).collect::<Vec<_>>().join(" OR "));
    a.confidence = if t.loose || !a.candidates.is_empty() { AskConfidence::Medium } else { AskConfidence::High };
    Ok(cx.finish(a))
}

pub(super) fn contact_info(cx: &mut Cx, who: &str, field: &'static str) -> Result<AskAnswer> {
    if matches!(field, "email" | "contact") {
        return contact_details(cx, who, field);
    }
    let (t, candidates) = match resolve_who(cx, who, AskIntent::ContactInfo)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    let found = contact_facts(cx, &t)?;
    let mut a = cx.answer(AskIntent::ContactInfo);
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.followups = person_followups(&t, AskIntent::ContactInfo);
    // Most recent first; count how often each value was written.
    let mut values: Vec<(String, usize, &Hit)> = Vec::new();
    for h in &found {
        let Extracted::Contact(c) = &h.f.fact else {
            continue;
        };
        let list = if field == "address" {
            &c.addresses
        } else {
            &c.phones
        };
        for v in list {
            let norm: String = v
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            match values.iter_mut().find(|x| {
                x.0.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase()
                    == norm
            }) {
                Some(x) => x.1 += 1,
                None => values.push((v.clone(), 1, h)),
            }
        }
    }
    let what = if field == "address" {
        "address"
    } else {
        "phone number"
    };
    let Some((v, n, h)) = values.first() else {
        a.headline = format!("I couldn't find a {what} for {} in their emails", t.label);
        a.detail = Some("Looked in the signatures of the emails they sent you.".into());
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    };
    a.headline = format!(
        "{}'s {what}: {v}",
        t.name.clone().unwrap_or(t.label.clone())
    );
    a.detail = Some(format!(
        "From their signature, {}{}",
        fmt_day(h.row.date, cx.off),
        if *n > 1 {
            format!(" (in {} emails)", n)
        } else {
            String::new()
        }
    ));
    for (v, n, h) in values.iter().skip(1).take(3) {
        let mut f = cx.date_fact(
            if field == "address" {
                "Also"
            } else {
                "Other number"
            },
            &h.row,
        );
        f.value = format!(
            "{v} · {} · {}",
            plural(*n, "email", "emails"),
            fmt_day(h.row.date, cx.off)
        );
        a.facts.push(f);
    }
    a.cards.push(cx.card(h, vec![], None));
    a.items.push(h.row.item(None));
    a.confidence = if *n > 1 {
        AskConfidence::High
    } else {
        AskConfidence::Medium
    };
    Ok(cx.finish(a))
}

/// "What subscriptions do I pay for": recurring charges by the smart
/// view's rule (the same merchant and currency, charged at least three
/// times, twice a year, at a steady cadence and a similar amount). Without
/// dates: the ones still running and what they come to a month. With dates
/// ("this year"): every charge of theirs in them, added up.
pub(super) fn subscriptions(cx: &mut Cx) -> Result<AskAnswer> {
    let scope = cx.scope.clone();
    let (now, lo, hi) = (cx.now, cx.lo, cx.hi);
    let dated = lo.is_some() || hi.is_some();
    let in_range = |ms: i64| lo.is_none_or(|l| ms >= l) && hi.is_none_or(|h| ms < h);
    let (subs, rows) = cx.store.read(|c| {
        let subs = crate::store::recurring_charges(c, scope.as_deref(), now)?;
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid WHERE m.rowid = ?1"
        ))?;
        let mut rows: HashMap<i64, Row> = HashMap::new();
        for s in &subs {
            let last = s.charges.last().map(|x| x.0);
            for (msg, date, _) in &s.charges {
                if (dated && in_range(*date)) || Some(*msg) == last {
                    if let Some(r) = stmt.query_row([msg], read_row).optional()? {
                        rows.insert(*msg, r);
                    }
                }
            }
        }
        Ok((subs, rows))
    })?;
    cx.steps.push(format!(
        "Found {} in your receipts: the same merchant charging at least three times (twice for yearly) at a steady weekly, monthly, quarterly or yearly cadence, the last three within 25% of each other",
        plural(subs.len(), "recurring charge", "recurring charges")
    ));
    let mut a = cx.answer(AskIntent::Subscriptions);
    let mut result = AskResult::new(if dated { AskResultKind::Sum } else { AskResultKind::List });
    let add = |totals: &mut Vec<AskTotal>, v: f64, cur: &str| match totals.iter_mut().find(|t| t.currency == cur) {
        Some(t) => {
            t.value += v;
            t.count += 1;
        }
        None => totals.push(AskTotal { value: v, currency: cur.to_string(), count: 1 }),
    };
    let fmt_totals = |totals: &[AskTotal]| totals.iter().map(|t| money(t.value, &t.currency)).collect::<Vec<_>>().join(" + ");
    if dated {
        // Every charge in the dates, from any of them (running or not).
        let mut totals: Vec<AskTotal> = Vec::new();
        let mut charged: Vec<(&Recurring, &Row, f64)> = Vec::new();
        for s in &subs {
            for (msg, date, v) in &s.charges {
                if in_range(*date) {
                    if let Some(r) = rows.get(msg) {
                        charged.push((s, r, *v));
                    }
                }
            }
        }
        charged.sort_by_key(|x| std::cmp::Reverse(x.1.date));
        for s in &subs {
            let mine: Vec<&(&Recurring, &Row, f64)> = charged.iter().filter(|x| std::ptr::eq(x.0, s)).collect();
            if mine.is_empty() {
                continue;
            }
            let mut g = Vec::new();
            for x in &mine {
                add(&mut g, x.2, &s.amount.currency);
                add(&mut totals, x.2, &s.amount.currency);
            }
            a.groups.push(AskGroup {
                label: s.merchant.clone(),
                count: mine.len(),
                totals: g,
                nights: None,
                cites: mine.iter().map(|x| x.1.cite()).collect(),
                start: None,
                best: false,
            });
        }
        a.headline = if charged.is_empty() {
            format!("No subscription charges{}", cx.range_phrase())
        } else {
            format!(
                "{} on {}{}",
                fmt_totals(&totals),
                plural(a.groups.len(), "subscription", "subscriptions"),
                cx.range_phrase()
            )
        };
        a.detail = Some(format!("Across {}", plural(charged.len(), "charge", "charges")));
        a.sum = Some(AskSum {
            totals: totals.clone(),
            basis: "Each charge from a recurring merchant".into(),
            duplicates: 0,
            skipped: 0,
            unpaid: 0,
        });
        a.items = charged
            .iter()
            .map(|(s, r, v)| {
                let mut it = r.item(Some(format!("{} charge", s.cadence)));
                it.amount = Some(AskAmount { value: *v, currency: s.amount.currency.clone(), source: format!("{} charge", s.cadence) });
                it
            })
            .collect();
        result.count = Some(charged.len() as u64);
        result.totals = totals;
    } else {
        // Running ones, and ones a charge late (listed, and flagged: the
        // charge may just not have come yet); older ones have stopped.
        let listed: Vec<&Recurring> = subs.iter().filter(|s| s.active || s.late).collect();
        let stopped: Vec<&Recurring> = subs.iter().filter(|s| !s.active && !s.late).collect();
        let last_day = |s: &Recurring| s.charges.last().map(|x| fmt_day(x.1, cx.off)).unwrap_or_default();
        let mut monthly: Vec<AskTotal> = Vec::new();
        for s in &listed {
            add(&mut monthly, s.per_month, &s.amount.currency);
            let last = s.charges.last().and_then(|x| rows.get(&x.0));
            a.groups.push(AskGroup {
                label: s.merchant.clone(),
                count: s.charges.len(),
                totals: vec![AskTotal { value: s.amount.value, currency: s.amount.currency.clone(), count: 1 }],
                nights: None,
                cites: last.map(|r| vec![r.cite()]).unwrap_or_default(),
                start: None,
                best: false,
            });
            let mut f = cx.fact(
                &s.merchant,
                format!(
                    "{} {} \u{b7} last {} \u{b7} {} {}",
                    money(s.amount.value, &s.amount.currency),
                    s.cadence.to_lowercase(),
                    last_day(s),
                    if s.active { "next about" } else { "was due about" },
                    fmt_day(s.next, cx.off)
                ),
            );
            if let Some(r) = last {
                f.cite = Some(r.cite());
                f.date = Some(r.date);
                let mut it = r.item(Some(format!("{} \u{b7} {} charges", s.cadence, s.charges.len())));
                it.amount = Some(AskAmount { value: s.amount.value, currency: s.amount.currency.clone(), source: format!("Latest {} charge", s.cadence.to_lowercase()) });
                a.items.push(it);
            }
            a.facts.push(f);
        }
        let all_monthly = listed.iter().all(|s| s.cadence == "Monthly");
        a.headline = if listed.is_empty() {
            "No subscriptions running in your receipts".into()
        } else {
            format!(
                "{}, {}{} a month: {}",
                plural(listed.len(), "subscription", "subscriptions"),
                if all_monthly { "" } else { "about " },
                fmt_totals(&monthly),
                listed.iter().map(|s| s.merchant.as_str()).collect::<Vec<_>>().join(", ")
            )
        };
        let late: Vec<String> = listed.iter().filter(|s| !s.active).map(|s| format!("{} (last {})", s.merchant, last_day(s))).collect();
        let mut detail = vec![if all_monthly {
            "Their latest charges.".to_string()
        } else {
            "Their latest charges, a yearly or weekly one as a month's worth.".to_string()
        }];
        if !late.is_empty() {
            detail.push(format!(
                "Not charged on schedule: {}; {} may have stopped.",
                late.join(", "),
                if late.len() == 1 { "it" } else { "they" }
            ));
        }
        if !stopped.is_empty() {
            detail.push(format!(
                "Stopped: {}.",
                stopped.iter().map(|s| format!("{} (last {})", s.merchant, last_day(s))).collect::<Vec<_>>().join(", ")
            ));
        }
        a.detail = Some(detail.join(" "));
        result.count = Some(listed.len() as u64);
        result.totals = monthly;
    }
    a.confidence = if subs.is_empty() { AskConfidence::None } else { AskConfidence::Medium };
    a.coverage = cx.coverage("receipts")?;
    a.result = Some(result);
    Ok(cx.finish(a))
}

/// Contact facts from the target's own emails (newest first), extracting
/// unscanned recent ones on the fly.
pub(super) fn contact_facts(cx: &mut Cx, t: &Target) -> Result<Vec<Hit>> {
    let emails = t.emails.clone();
    let out: Vec<Hit> = cx.store.read(|c| {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {FACT_COLS}, {ROW_COLS} FROM messages m JOIN extracted x ON x.msg = m.rowid
             JOIN threads t ON t.rowid = m.thread_rowid
             WHERE m.from_email = ?1 COLLATE NOCASE AND x.kind = 'contact' AND m.flags & ?2 = 0
             ORDER BY m.date DESC LIMIT 50"
        ))?;
        let mut unscanned = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
             WHERE m.from_email = ?1 COLLATE NOCASE AND m.extracted IS NULL AND m.flags & ?2 = 0
             ORDER BY m.date DESC LIMIT 20"
        ))?;
        let mut v = Vec::new();
        for e in &emails {
            for x in stmt.query_map(params![e, HIDDEN], |r| {
                Ok((read_fact(r)?, read_row_at(r, 11)?))
            })? {
                if let (Some(f), row) = x? {
                    v.push(Hit { f, row });
                }
            }
            let rows: Vec<Row> = unscanned
                .query_map(params![e, HIDDEN], read_row)?
                .collect::<rusqlite::Result<_>>()?;
            for row in rows {
                for f in extract_now(c, row.rowid)? {
                    if f.kind == "contact" {
                        v.push(Hit {
                            f,
                            row: row.clone(),
                        });
                    }
                }
            }
        }
        v.sort_by_key(|h| std::cmp::Reverse(h.row.date));
        Ok(v)
    })?;
    cx.steps.push(format!(
        "Read the phone numbers and addresses in {}'s signatures: {}",
        t.label,
        plural(out.len(), "email", "emails")
    ));
    Ok(out
        .into_iter()
        .filter(|h| cx.in_scope(&h.row.account_id))
        .collect())
}

// -------------------------------------------------------------- counts

pub(super) fn count_facts(cx: &mut Cx, kind: FactKind, who: Option<&str>) -> Result<AskAnswer> {
    let (table, one, many) = match kind {
        FactKind::Flight => ("flight", "flight", "flights"),
        FactKind::Stay => ("lodging", "stay", "stays"),
        FactKind::Order => ("order", "order", "orders"),
        FactKind::Shipment => ("shipment", "package", "packages"),
        FactKind::Bill => ("bill", "bill", "bills"),
        FactKind::Booking => ("reservation", "reservation", "reservations"),
    };
    let hits = cx.facts(table, who)?;
    let owned: Vec<Hit> = hits
        .into_iter()
        .filter(|h| who.is_none_or(|w| matches_thing(h, w)))
        .filter(|h| {
            // Travel counts by when it happened; the rest by the email.
            let ms = match (kind, h.f.at.as_deref().and_then(parse_at)) {
                (FactKind::Flight | FactKind::Stay | FactKind::Booking, Some((d, _))) => day_ms(d, cx.off),
                _ => h.row.date,
            };
            cx.lo.is_none_or(|lo| ms >= lo) && cx.hi.is_none_or(|hi| ms < hi)
        })
        .filter(|h| !matches!(&h.f.fact, Extracted::Flight(f) if f.status.as_deref() == Some("cancelled")))
        .collect();
    let groups = group(&owned, |h| match &h.f.fact {
        // A return leg without its own number is keyed by its booking, so
        // two trips home on the same day stay two flights.
        Extracted::Flight(f) => Some(format!(
            "{}|{}",
            f.flight_number
                .clone()
                .or_else(|| f.confirmation.clone().map(|c| format!("conf:{c}")))
                .unwrap_or_default(),
            f.depart_time
                .as_deref()
                .and_then(|t| t.get(..10))
                .unwrap_or("")
        )),
        _ => h.f.reference.clone().map(|r| format!("{}|{r}", h.f.key)),
    });
    let n = groups.len();
    let dups = owned.len() - n;
    let mut a = cx.answer(AskIntent::Count);
    a.coverage = cx.coverage(many)?;
    let from = who
        .map(|w| format!(" with {}", capitalize(w)))
        .unwrap_or_default();
    a.headline = format!("{}{from}{}", plural(n, one, many), cx.range_phrase());
    if dups > 0 {
        cx.steps.push(format!(
            "Counted each {one} once: {} about the same booking, order or parcel were merged",
            plural(dups, "email", "emails")
        ));
    }
    // With amounts, show the total too.
    let mut totals: Vec<AskTotal> = Vec::new();
    for g in &groups {
        if let (Some(v), Some(c)) = (g[0].f.amount, &g[0].f.currency) {
            match totals.iter_mut().find(|t| &t.currency == c) {
                Some(t) => {
                    t.value += v;
                    t.count += 1;
                }
                None => totals.push(AskTotal {
                    value: v,
                    currency: c.clone(),
                    count: 1,
                }),
            }
        }
    }
    if !totals.is_empty() {
        a.detail = Some(format!(
            "{} across the ones with an amount",
            totals
                .iter()
                .map(|t| money(t.value, &t.currency))
                .collect::<Vec<_>>()
                .join(" + ")
        ));
        a.sum = Some(AskSum {
            totals,
            basis: format!("Totals printed on each {one}"),
            duplicates: dups,
            skipped: 0,
            unpaid: 0,
        });
    }
    for g in groups.iter().take(MAX_ITEMS) {
        let mut it = g[0].row.item(None);
        if let (Some(v), Some(c)) = (g[0].f.amount, &g[0].f.currency) {
            it.amount = Some(AskAmount {
                value: v,
                currency: c.clone(),
                source: String::new(),
            });
        }
        a.items.push(it);
        a.cards.push(cx.card(g[0], related(g), None));
    }
    a.confidence = if n == 0 {
        AskConfidence::None
    } else if a.coverage.is_some() {
        AskConfidence::Medium
    } else {
        AskConfidence::High
    };
    Ok(cx.finish(a))
}

// ------------------------------------------------ amounts for spend sums

/// What one email adds to a spend sum, from its extracted facts: an
/// order/receipt total, a booking's total, or a bill marked paid.
/// Unscanned emails are extracted now. None = nothing to add.
pub(super) struct Spent {
    pub value: f64,
    pub currency: String,
    pub source: String,
    /// Order number etc., to count one order once.
    pub reference: Option<String>,
    /// An invoice still due (not spent yet).
    pub unpaid: bool,
}

fn spent_of(f: &StoredFact) -> Option<Spent> {
    let (Some(value), Some(currency)) = (f.amount, f.currency.clone()) else {
        return None;
    };
    let (source, unpaid) = match &f.fact {
        Extracted::Order(o) => (
            o.total_source
                .clone()
                .unwrap_or_else(|| "Order total".into()),
            false,
        ),
        Extracted::Bill(b) => (
            b.amount_source
                .clone()
                .unwrap_or_else(|| "Amount due".into()),
            b.status.as_deref() != Some("paid"),
        ),
        Extracted::Flight(_) => ("Booking total".into(), false),
        Extracted::Lodging(_) => ("Stay total".into(), false),
        Extracted::Reservation(_) => ("Booking total".into(), false),
        _ => return None,
    };
    Some(Spent {
        value,
        currency,
        source: clip(&source, 160),
        reference: f.reference.clone(),
        unpaid,
    })
}

/// What each of `rowids` adds to a spend sum, from its extracted facts
/// (read in batches; unscanned messages are extracted now). A message
/// missing from the map has no amount fact.
pub(super) fn spent_many(c: &Connection, rowids: &[i64]) -> Result<HashMap<i64, Spent>> {
    let mut out: HashMap<i64, Spent> = HashMap::new();
    for chunk in rowids.chunks(500) {
        // Integers only: safe to inline.
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = c.prepare(&format!(
            "SELECT {FACT_COLS} FROM extracted x WHERE x.msg IN ({list}) AND x.amount IS NOT NULL ORDER BY x.msg, x.ord"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, read_fact(r)?)))?;
        for row in rows {
            if let (msg, Some(f)) = row? {
                if let std::collections::hash_map::Entry::Vacant(e) = out.entry(msg) {
                    if let Some(s) = spent_of(&f) {
                        e.insert(s);
                    }
                }
            }
        }
        let unscanned: Vec<i64> = c
            .prepare(&format!(
                "SELECT rowid FROM messages WHERE rowid IN ({list}) AND extracted IS NULL"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for rowid in unscanned {
            if let Some(s) = extract_now(c, rowid)?.iter().find_map(spent_of) {
                out.insert(rowid, s);
            }
        }
    }
    Ok(out)
}

/// Travel spending by kind ("how much did I spend on flights"): the
/// totals printed on each booking, one per booking.
pub(super) fn spend_category(cx: &mut Cx, kind: FactKind, label: &str) -> Result<AskAnswer> {
    let table = match kind {
        FactKind::Stay => "lodging",
        FactKind::Booking => "reservation",
        _ => "flight",
    };
    let hits = cx.facts(table, None)?;
    let owned: Vec<Hit> = hits
        .into_iter()
        .filter(|h| h.f.amount.is_some())
        .filter(|h| {
            cx.lo.is_none_or(|lo| h.row.date >= lo) && cx.hi.is_none_or(|hi| h.row.date < hi)
        })
        .collect();
    let groups = group(&owned, |h| {
        h.f.reference.clone().map(|r| format!("{}|{r}", h.f.key))
    });
    let mut a = cx.answer(AskIntent::Spend);
    a.coverage = cx.coverage("bookings")?;
    let mut totals: Vec<AskTotal> = Vec::new();
    for g in &groups {
        let (Some(v), Some(c)) = (g[0].f.amount, &g[0].f.currency) else {
            continue;
        };
        match totals.iter_mut().find(|t| &t.currency == c) {
            Some(t) => {
                t.value += v;
                t.count += 1;
            }
            None => totals.push(AskTotal {
                value: v,
                currency: c.clone(),
                count: 1,
            }),
        }
        let mut it = g[0].row.item(None);
        it.amount = Some(AskAmount {
            value: v,
            currency: c.clone(),
            source: "Booking total".into(),
        });
        a.items.push(it);
    }
    totals.sort_by_key(|t| std::cmp::Reverse(t.count));
    if totals.is_empty() {
        a.headline = format!(
            "I couldn't find {label} bookings with a price{} in your mail",
            cx.range_phrase()
        );
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    }
    let text = totals
        .iter()
        .map(|t| money(t.value, &t.currency))
        .collect::<Vec<_>>()
        .join(" + ");
    a.headline = format!(
        "{text} on {label}{} across {}",
        cx.range_phrase(),
        plural(groups.len(), "booking", "bookings")
    );
    a.detail = Some("The total printed on each booking email; a booking sent twice counts once. Airline receipts without a total aren't included.".into());
    a.sum = Some(AskSum {
        totals,
        basis: format!("Total price on each {label} booking"),
        duplicates: owned.len() - groups.len(),
        skipped: 0,
        unpaid: 0,
    });
    a.confidence = if a.coverage.is_some() {
        AskConfidence::Medium
    } else {
        AskConfidence::High
    };
    Ok(cx.finish(a))
}
