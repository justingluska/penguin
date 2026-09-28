//! Facts from the text of mail without schema.org markup.
//!
//! Each extractor needs a cue before it looks at all (a subject word like
//! "order", "invoice", "itinerary", or a labeled field), then pulls labeled
//! values ("Order #", "Confirmation code", "Amount due", "Check-in") and
//! checks shapes (flight numbers against airline designators, tracking
//! numbers against check digits). English plus common Spanish labels.
//! Only the authored text is read, never quoted history.

use chrono::{DateTime, NaiveDate};

use super::tracking;
use super::{
    clean_org, org_label, Bill, Contact, Extracted, Flight, Lodging, MailInput, Money, Order,
    Reservation, Shipment,
};
use crate::ask::extract::{amounts, clip, date_mentions, receipt_total, DateMention};

/// Bytes of authored text read per message.
const TEXT_WINDOW: usize = 40_000;

pub(crate) fn extract(m: &MailInput) -> Vec<Extracted> {
    let text = prefix(m.text, TEXT_WINDOW);
    let lower = text.to_lowercase();
    let subj = m.subject.to_lowercase();
    let anchor = DateTime::from_timestamp_millis(m.date)
        .unwrap_or_default()
        .date_naive();
    let cx = Cx {
        m,
        text,
        lower: &lower,
        subj: &subj,
        anchor,
    };
    let mut out = Vec::new();
    let mut flights = cx.flights();
    let travel = !flights.is_empty();
    out.append(&mut flights);
    if let Some(l) = cx.lodging() {
        out.push(Extracted::Lodging(l));
    }
    let mut ships = cx.shipments();
    let shipped = !ships.is_empty();
    out.append(&mut ships);
    let bill = cx.bill();
    let billed = bill.is_some();
    if let Some(b) = bill {
        out.push(Extracted::Bill(b));
    }
    // Tickets and tables are the thing bought: an event with its total,
    // not a separate order.
    let mut reserved = false;
    if !billed && !travel && !shipped && out.is_empty() {
        if let Some(r) = cx.reservation() {
            out.push(Extracted::Reservation(r));
            reserved = true;
        }
    }
    if !billed && !travel && !reserved {
        if let Some(o) = cx.order(shipped) {
            out.push(Extracted::Order(o));
        }
    }
    // Transactional mail isn't a person's signature.
    if out.is_empty() {
        if let Some(c) = cx.contact() {
            out.push(Extracted::Contact(c));
        }
    }
    out
}

struct Cx<'a> {
    m: &'a MailInput<'a>,
    text: &'a str,
    lower: &'a str,
    subj: &'a str,
    anchor: NaiveDate,
}

/// The longest prefix of `s` of at most `n` bytes, on a char boundary.
fn prefix(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut e = n;
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    &s[..e]
}

/// `s[start..start+n]` on char boundaries.
fn window(s: &str, start: usize, n: usize) -> &str {
    let mut a = start.min(s.len());
    while !s.is_char_boundary(a) {
        a -= 1;
    }
    let mut e = (a + n).min(s.len());
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    &s[a..e]
}

fn has_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Every whole-word occurrence of `needle` in `hay` (byte offsets). The
/// boundary checks apply only at alphanumeric ends ("order #" may touch
/// the number after it).
fn find_words(hay: &str, needle: &str) -> Vec<(usize, usize)> {
    let alnum_start = needle.chars().next().is_some_and(char::is_alphanumeric);
    let alnum_end = needle
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric);
    hay.match_indices(needle)
        .map(|(i, _)| (i, i + needle.len()))
        .filter(|&(s, e)| {
            let before = !alnum_start
                || hay[..s]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !c.is_alphanumeric());
            let after = !alnum_end || hay[e..].chars().next().is_none_or(|c| !c.is_alphanumeric());
            before && after
        })
        .collect()
}

/// "7pm" → "19:00", "7:30 pm" → "19:30", "19:05" → "19:05".
fn clock(t: &str) -> Option<String> {
    let t = t.replace(['.', ' '], "");
    let (body, pm) = if let Some(b) = t.strip_suffix("pm") {
        (b.to_string(), Some(true))
    } else if let Some(b) = t.strip_suffix("am") {
        (b.to_string(), Some(false))
    } else {
        (t.clone(), None)
    };
    let (h, m) = body.split_once(':').unwrap_or((&body, "00"));
    let mut h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if m > 59 || h > 23 {
        return None;
    }
    match pm {
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    Some(format!("{h:02}:{m:02}"))
}

fn local(d: NaiveDate, time: Option<&str>) -> String {
    match time.and_then(clock) {
        Some(t) => format!("{}T{t}", d.format("%Y-%m-%d")),
        None => d.format("%Y-%m-%d").to_string(),
    }
}

/// The first value printed after one of `labels` (lowercase): the text up
/// to the end of the line, after ":", "#", "No." and spaces.
fn labeled<'t>(text: &'t str, lower: &str, labels: &[&str]) -> Option<&'t str> {
    for label in labels {
        for (_, e) in find_words(lower, label) {
            let rest = &text[e..];
            let rest = rest.trim_start_matches(|c: char| {
                matches!(
                    c,
                    ':' | '#' | ' ' | '\t' | '.' | '\u{a0}' | '-' | '\u{2013}'
                )
            });
            let rest = rest
                .strip_prefix("No.")
                .or_else(|| rest.strip_prefix("no."))
                .or_else(|| rest.strip_prefix("Nº"))
                .or_else(|| rest.strip_prefix("n.º"))
                .map(|r| r.trim_start_matches([' ', ':', '#']))
                .unwrap_or(rest);
            let line = rest.lines().next().unwrap_or("").trim();
            if !line.is_empty() {
                return Some(line);
            }
            // Two-column layouts: the value is on the next non-empty line.
            if let Some(next) = rest.lines().skip(1).map(str::trim).find(|l| !l.is_empty()) {
                return Some(next);
            }
        }
    }
    None
}

/// A reference code: the first token of `v` that looks like one (letters,
/// digits and dashes, with at least one digit unless `letters_ok`).
fn code_token(v: &str, min: usize, letters_ok: bool) -> Option<String> {
    let tok: String = v
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string();
    let ok = tok.len() >= min
        && tok.len() <= 40
        && tok
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && (letters_ok || tok.chars().any(|c| c.is_ascii_digit()))
        && tok.chars().any(|c| c.is_ascii_alphanumeric());
    ok.then_some(tok)
}

fn money_of(value: f64, currency: &str) -> Money {
    Money {
        value,
        currency: currency.to_string(),
    }
}

impl Cx<'_> {
    fn sender(&self) -> String {
        let name = self.m.from_name.map(clean_org).unwrap_or_default();
        if name.is_empty() || name.contains('@') {
            let l = org_label(self.m.from_email);
            let mut c = l.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => l,
            }
        } else {
            name
        }
    }

    /// The first date written within `span` bytes after one of `keys`.
    fn date_after(&self, keys: &[&str], span: usize, explicit_only: bool) -> Option<DateMention> {
        let mut best: Option<(usize, DateMention)> = None;
        for k in keys {
            for (s, e) in find_words(self.lower, k) {
                let w = window(self.text, e, span);
                let found = date_mentions(w, self.anchor)
                    .into_iter()
                    .find(|d| d.explicit || !explicit_only);
                if let Some(d) = found {
                    if best.as_ref().is_none_or(|b| s < b.0) {
                        best = Some((s, d));
                    }
                    break;
                }
            }
            if best.is_some() {
                break;
            }
        }
        best.map(|(_, d)| d)
    }

    fn confirmation(&self, letters_ok: bool) -> Option<String> {
        let v = labeled(
            self.text,
            self.lower,
            &[
                "confirmation code",
                "confirmation number",
                "confirmation #",
                "confirmation no",
                "record locator",
                "booking reference",
                "booking code",
                "booking number",
                "booking id",
                "reservation code",
                "reservation number",
                "reservation #",
                "itinerary number",
                "pnr",
                "código de reserva",
                "codigo de reserva",
                "localizador",
                "número de reserva",
                "numero de reserva",
                "número de confirmación",
                "confirmation",
            ],
        )?;
        code_token(v, 5, letters_ok)
    }

    // ------------------------------------------------------------ flights

    fn flights(&self) -> Vec<Extracted> {
        const CUES: &[&str] = &[
            "flight",
            "itinerary",
            "boarding pass",
            "e-ticket",
            "eticket",
            "record locator",
            "vuelo",
            "itinerario",
            "tarjeta de embarque",
            "pase de abordar",
        ];
        if !has_any(self.subj, CUES) && !has_any(self.lower, CUES) {
            return vec![];
        }
        let sender_airline = super::places::airline_in(&format!(
            "{} {}",
            self.m.from_name.unwrap_or("").to_lowercase(),
            self.m.from_email.to_lowercase()
        ))
        .or_else(|| super::places::airline_in(self.lower));
        let mut found: Vec<(usize, String, String)> = Vec::new(); // (pos, code, number)
        let b = self.text.as_bytes();
        let mut i = 0;
        while i + 3 <= b.len() {
            // Token start.
            if i > 0 && b[i - 1].is_ascii_alphanumeric() {
                i += 1;
                continue;
            }
            let c0 = b[i];
            let c1 = b.get(i + 1).copied().unwrap_or(0);
            let two = (c0.is_ascii_uppercase() || c0.is_ascii_digit())
                && (c1.is_ascii_uppercase() || c1.is_ascii_digit())
                && (c0.is_ascii_uppercase() || c1.is_ascii_uppercase());
            if two {
                let mut j = i + 2;
                if b.get(j) == Some(&b' ') {
                    j += 1;
                }
                let ds = j;
                while j < b.len() && b[j].is_ascii_digit() && j - ds < 5 {
                    j += 1;
                }
                let nd = j - ds;
                let end_ok = j >= b.len() || !b[j].is_ascii_alphanumeric();
                if (1..=4).contains(&nd) && end_ok {
                    let code = &self.text[i..i + 2];
                    let num = &self.text[ds..j];
                    let before = self.lower[..i].trim_end();
                    let cue = before.ends_with("flight")
                        || before.ends_with("flight:")
                        || before.ends_with("flight no.")
                        || before.ends_with("flight number")
                        || before.ends_with("flight number:")
                        || before.ends_with("vuelo")
                        || before.ends_with("vuelo:")
                        || before.ends_with("flt");
                    let known = super::airline_by_code(code).is_some();
                    let same = sender_airline.is_some_and(|(c, _)| c == code);
                    if (known && (cue || same)) || cue {
                        found.push((i, code.to_string(), num.trim_start_matches('0').to_string()));
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        // "Flight 238" with the airline known from the sender, only when the
        // mail names no flight with its airline code: then a bare number is
        // more likely a counter ("Flight 1 of 2") than a flight.
        if let Some((code, _)) = sender_airline.filter(|_| found.is_empty()) {
            for key in ["flight ", "flight number ", "flight no. ", "vuelo "] {
                for (s, e) in find_words(self.lower, key.trim_end()) {
                    let rest = self.text[e..].trim_start_matches([' ', ':', '#']);
                    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    let after = rest[digits.len()..].chars().next();
                    // "Flight 1 of 2" / "Vuelo 1 de 2" numbers segments, not flights.
                    let tail = rest[digits.len()..].trim_start().to_lowercase();
                    let counter = ["of ", "de ", "/"].iter().any(|w| {
                        tail.strip_prefix(w)
                            .is_some_and(|t| t.trim_start().starts_with(|c: char| c.is_ascii_digit()))
                    });
                    if (1..=4).contains(&digits.len())
                        && after.is_none_or(|c| !c.is_alphanumeric())
                        && !counter
                    {
                        found.push((
                            s,
                            code.to_string(),
                            digits.trim_start_matches('0').to_string(),
                        ));
                    }
                }
            }
        }
        found.sort_by_key(|f| f.0);
        let mut seen: Vec<String> = Vec::new();
        let confirmation = self
            .confirmation(true)
            .filter(|c| c.len() == 6 || c.chars().any(|x| x.is_ascii_digit()));
        let status = if has_any(self.subj, &["cancel", "cancelad"]) {
            Some("cancelled".to_string())
        } else if has_any(self.subj, &["change", "cambio"]) {
            Some("changed".to_string())
        } else {
            None
        };
        let mut out = Vec::new();
        for (pos, code, num) in found {
            if num.is_empty() {
                continue;
            }
            let number = format!("{code} {num}");
            if seen.contains(&number) {
                continue;
            }
            seen.push(number.clone());
            let w = window(self.text, pos, 500);
            // Cut the window at the next flight number so segments don't mix.
            let (dep, arr) = airports(w);
            let dates = date_mentions(w, self.anchor);
            let date = dates.iter().find(|d| d.explicit).or(dates.first());
            let times = clocks(w);
            let depart_time = date.map(|d| {
                local(
                    d.date,
                    d.time.as_deref().or(times.first().map(String::as_str)),
                )
            });
            let arrive_time = date.and_then(|d| {
                let t = times.get(1)?;
                let t0 = times.first().and_then(|x| clock(x))?;
                let t1 = clock(t)?;
                // Arrival earlier than departure: the next day.
                let day = if t1 < t0 { d.date.succ_opt()? } else { d.date };
                Some(local(day, Some(t)))
            });
            // The sender's name when it's the airline and the table
            // disagrees (a code can be shared: IATA "controlled duplicates").
            let sender = self.m.from_name.map(clean_org).filter(|n| {
                let l = n.to_lowercase();
                [
                    "air",
                    "airline",
                    "airlines",
                    "airways",
                    "aero",
                    "aerolineas",
                    "aerolíneas",
                    "vuelos",
                ]
                .iter()
                .any(|w| l.split_whitespace().any(|x| x == *w) || l.starts_with("aero"))
            });
            let table = super::airline_by_code(&code).map(|(_, n)| n.to_string());
            let airline = match (&sender_airline, sender) {
                (Some((c, _)), _) if *c == code => table,
                (_, Some(s)) => Some(s),
                _ => table,
            };
            let name = |c: &Option<String>| {
                c.as_deref()
                    .and_then(super::airport_by_code)
                    .map(|(_, city)| city.to_string())
            };
            // A flight number alone (a friend's "we're on AV118") isn't a booking.
            if date.is_none() && dep.is_none() {
                continue;
            }
            out.push(Extracted::Flight(Flight {
                airline,
                airline_code: Some(code),
                flight_number: Some(number),
                confirmation: confirmation.clone(),
                passenger: None,
                depart_name: name(&dep),
                arrive_name: name(&arr),
                depart_airport: dep,
                arrive_airport: arr,
                depart_time,
                arrive_time,
                status: status.clone(),
                total: None,
            }));
        }
        // A round trip written with one flight number and a "Return:" date
        // ("Depart: Fri, Oct 2 / Return: Sat, Oct 10") is two flights.
        if out.len() == 1 {
            if let Some(ret) = self.return_leg(&out[0]) {
                out.push(ret);
            }
        }
        // A booking total goes on the first segment.
        if let Some(Extracted::Flight(f)) = out.first_mut() {
            if let Some(t) = receipt_total(self.text) {
                f.total = Some(money_of(t.amount.value, t.amount.currency));
            }
        }
        out
    }

    /// The return leg of a round trip booked as one segment: the same route
    /// backwards on the date after "Return" / "Vuelta" (its flight number
    /// when one follows the word, else none). Only after the outbound date,
    /// and never for a one-way ticket.
    fn return_leg(&self, outbound: &Extracted) -> Option<Extracted> {
        let Extracted::Flight(f) = outbound else {
            return None;
        };
        let (dep, arr) = (f.depart_airport.clone()?, f.arrive_airport.clone()?);
        let out_day = NaiveDate::parse_from_str(f.depart_time.as_deref()?.get(..10)?, "%Y-%m-%d").ok()?;
        if has_any(self.lower, &["one-way", "one way", "solo ida", "sólo ida"]) {
            return None;
        }
        const KEYS: &[&str] = &["return flight", "return", "returning", "vuelta", "regreso"];
        let d = self.date_after(KEYS, 60, true)?;
        if d.date <= out_day {
            return None;
        }
        // "Return SK 2211: Wednesday, October 23" names the flight.
        let number = KEYS.iter().find_map(|k| {
            let (_, e) = *find_words(self.lower, k).first()?;
            let rest = self.text[e..].trim_start_matches([' ', ':']);
            let code: String = rest.chars().take(2).collect();
            let ok_code = code.len() == 2
                && code.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && code.chars().any(|c| c.is_ascii_uppercase());
            if !ok_code {
                return None;
            }
            let digits: String = rest[2..]
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (1..=4)
                .contains(&digits.len())
                .then(|| format!("{code} {}", digits.trim_start_matches('0')))
        });
        Some(Extracted::Flight(Flight {
            airline: f.airline.clone(),
            airline_code: f.airline_code.clone(),
            flight_number: number,
            confirmation: f.confirmation.clone(),
            passenger: f.passenger.clone(),
            depart_name: f.arrive_name.clone(),
            arrive_name: f.depart_name.clone(),
            depart_airport: Some(arr),
            arrive_airport: Some(dep),
            depart_time: Some(local(d.date, d.time.as_deref())),
            arrive_time: None,
            status: f.status.clone(),
            total: None,
        }))
    }

    // ------------------------------------------------------------ lodging

    fn lodging(&self) -> Option<Lodging> {
        let nights = self.nights();
        let has_in = has_any(
            self.lower,
            &["check-in", "check in", "checkin", "llegada", "entrada:"],
        ) || (nights.is_some()
            && has_any(self.lower, &["arrive", "arrival", "arriving"]));
        let has_out = has_any(
            self.lower,
            &["check-out", "check out", "checkout", "salida"],
        ) || nights.is_some();
        if !has_in || !has_out {
            return None;
        }
        if !has_any(
            &format!("{} {}", self.subj, prefix(self.lower, 3000)),
            &[
                "reservation",
                "booking",
                "your stay",
                "hotel",
                "reserva",
                "estancia",
                "confirmed",
            ],
        ) {
            return None;
        }
        let cin = self.date_after(
            &[
                "check-in", "check in", "checkin", "llegada", "entrada", "arrive", "arrival",
                "arriving",
            ],
            80,
            true,
        )?;
        // Check-out as written, else check-in plus the nights ("3 nights").
        let cout =
            match self.date_after(&["check-out", "check out", "checkout", "salida"], 80, true) {
                Some(c) => c,
                None => {
                    let n = nights?;
                    DateMention {
                        date: cin.date + chrono::Duration::days(n as i64),
                        time: None,
                        start: 0,
                        end: 0,
                        sentence: String::new(),
                        explicit: true,
                    }
                }
            };
        if cout.date < cin.date {
            return None;
        }
        let name = subject_place(self.m.subject).unwrap_or_else(|| self.sender());
        let total = receipt_total(self.text).map(|t| money_of(t.amount.value, t.amount.currency));
        Some(Lodging {
            name: Some(name),
            address: addresses(self.text).into_iter().next(),
            phone: None,
            checkin: Some(local(cin.date, cin.time.as_deref())),
            checkout: Some(local(cout.date, cout.time.as_deref())),
            confirmation: self.confirmation(true),
            guest: None,
            status: has_any(self.subj, &["cancel", "cancelad"]).then(|| "cancelled".into()),
            total,
        })
    }

    /// "3 nights", "7 noches".
    fn nights(&self) -> Option<u32> {
        for key in ["nights", "night", "noches", "noche"] {
            for (s, _) in find_words(self.lower, key) {
                let before = self.lower[..s].trim_end();
                let n: String = before
                    .chars()
                    .rev()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if let Ok(n) = n.parse::<u32>() {
                    if (1..=60).contains(&n) {
                        return Some(n);
                    }
                }
            }
        }
        None
    }

    // ---------------------------------------------------------- shipments

    fn shipments(&self) -> Vec<Extracted> {
        const CUES: &[&str] = &[
            "ship",
            "track",
            "deliver",
            "package",
            "parcel",
            "on its way",
            "on the way",
            "envío",
            "envio",
            "enviado",
            "paquete",
            "entrega",
            "rastreo",
            "seguimiento",
            "en camino",
        ];
        let head = format!("{} {}", self.subj, prefix(self.lower, 4000));
        if !has_any(&head, CUES) {
            return vec![];
        }
        // Numbers and carrier names also appear only in the subject or
        // the sender ("Your DHL shipment 3318810025 …", From: FedEx).
        let with_subject = format!("{}\n{}", self.m.subject, self.text);
        let cues = format!(
            "{} {} {}\n{}",
            self.m.from_name.unwrap_or("").to_lowercase(),
            self.m.from_email.to_lowercase(),
            self.subj,
            self.lower
        );
        let found = tracking::find_with_cues(&with_subject, &with_subject.to_lowercase(), &cues);
        if found.is_empty() {
            return vec![];
        }
        let status = ship_status(&format!("{} {}", self.subj, prefix(self.lower, 600)));
        let expected = self
            .date_after(
                &[
                    "arriving",
                    "arrives",
                    "expected delivery",
                    "estimated delivery",
                    "delivery date",
                    "deliver by",
                    "delivery by",
                    "expected by",
                    "llega",
                    "llegará",
                    "entrega estimada",
                    "fecha estimada",
                    "fecha de entrega",
                ],
                60,
                false,
            )
            .map(|d| d.date.format("%Y-%m-%d").to_string());
        let carrier_sender = has_any(
            &self.m.from_email.to_lowercase(),
            &[
                "ups.", "usps.", "fedex.", "dhl.", "@ups", "@usps", "@fedex", "@dhl",
            ],
        );
        let order_number = self.order_number();
        found
            .iter()
            .map(|t| {
                let mut s = Shipment::from_tracking(t);
                s.status = status.clone();
                s.expected = expected.clone();
                s.merchant = (!carrier_sender).then(|| self.sender());
                s.order_number = order_number.clone();
                Extracted::Shipment(s)
            })
            .collect()
    }

    fn order_number(&self) -> Option<String> {
        // The subject first ("Your refund for order #A-2200 …").
        let subj = labeled(
            self.m.subject,
            self.subj,
            &["order #", "order number", "order no", "pedido nº", "pedido"],
        )
        .and_then(|v| code_token(v, 4, false));
        if subj.is_some() {
            return subj;
        }
        let v = labeled(
            self.text,
            self.lower,
            &[
                "order number",
                "order no",
                "order #",
                "order id",
                "order:",
                "número de pedido",
                "numero de pedido",
                "n.º de pedido",
                "pedido nº",
                "pedido",
                "order",
            ],
        )?;
        code_token(v, 4, false)
    }

    // -------------------------------------------------------------- bills

    fn bill(&self) -> Option<Bill> {
        const CUES: &[&str] = &[
            "invoice",
            "premium",
            "your bill",
            "bill is",
            "bill for",
            "statement",
            "payment due",
            "amount due",
            "balance due",
            "factura",
            "vencimiento",
            "importe a pagar",
        ];
        let head = format!("{} {}", self.subj, prefix(self.lower, 2500));
        if !has_any(&head, CUES) {
            return None;
        }
        let due = self.date_after(
            &[
                "due date",
                "payment due",
                "due on",
                "due by",
                "pay by",
                "is due",
                "due",
                "fecha de vencimiento",
                "vence el",
                "vence",
                "fecha límite de pago",
                "fecha limite de pago",
            ],
            60,
            true,
        );
        let amount = bill_amount(self.text).or_else(|| {
            receipt_total(self.text).map(|t| (money_of(t.amount.value, t.amount.currency), t.line))
        });
        // "invoice INV-20417 for $18,400.00"
        let amount = amount.or_else(|| {
            let (_, e) = find_words(self.lower, "invoice").into_iter().next()?;
            let w = window(self.text, e, 60);
            let p = w.find(" for ")?;
            let a = amounts(&w[p..(p + 20).min(w.len())]).into_iter().next()?;
            Some((
                money_of(a.value, a.currency),
                clip(window(self.text, e.saturating_sub(8), 70), 160),
            ))
        });
        // "Payment terms net 30", "due in 30 days": from the email's date.
        let due = due.or_else(|| {
            let k = ["net ", "due in ", "due within ", "payable within "]
                .iter()
                .find_map(|k| self.lower.find(k).map(|p| p + k.len()))?;
            let n: String = self.lower[k..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let days: i64 = n.parse().ok().filter(|d| (1..=120).contains(d))?;
            let rest = self.lower[k + n.len()..].trim_start();
            if !(k >= 4 && self.lower[..k].ends_with("net ") || rest.starts_with("day")) {
                return None;
            }
            Some(DateMention {
                date: self.anchor + chrono::Duration::days(days),
                time: None,
                start: 0,
                end: 0,
                sentence: String::new(),
                explicit: true,
            })
        });
        let number = labeled(
            self.text,
            self.lower,
            &[
                "invoice number",
                "invoice no",
                "invoice #",
                "invoice id",
                "número de factura",
                "numero de factura",
                "factura nº",
                "factura no",
                "invoice",
            ],
        )
        .and_then(|v| code_token(v, 3, false));
        let paid = has_any(
            self.subj,
            &[
                "paid",
                "payment received",
                "receipt",
                "thank you for your payment",
                "pagada",
                "pagado",
                "pago recibido",
            ],
        );
        // "We received your payment of $89.00 for invoice INV-3310."
        let amount = amount.or_else(|| {
            if !paid {
                return None;
            }
            let p = ["payment of", "pago de"]
                .iter()
                .find_map(|k| self.lower.find(k).map(|p| p + k.len()))?;
            let a = amounts(window(self.text, p, 40)).into_iter().next()?;
            Some((
                money_of(a.value, a.currency),
                crate::ask::extract::clip(window(self.text, p.saturating_sub(20), 80), 160),
            ))
        });
        if due.is_none() && amount.is_none() && !(paid && number.is_some()) {
            return None;
        }
        let overdue = has_any(&head, &["overdue", "past due", "vencida", "vencido"]);
        Some(Bill {
            biller: Some(self.sender()),
            invoice_number: number,
            amount_due: amount.as_ref().map(|a| a.0.clone()),
            amount_source: amount.map(|a| a.1),
            due_date: due.map(|d| d.date.format("%Y-%m-%d").to_string()),
            status: Some(
                if paid {
                    "paid"
                } else if overdue {
                    "overdue"
                } else {
                    "due"
                }
                .into(),
            ),
        })
    }

    // ------------------------------------------------------------- orders

    fn order(&self, shipped: bool) -> Option<Order> {
        const SUBJECT_CUES: &[&str] = &[
            "order",
            "receipt",
            "your purchase",
            "purchase confirmation",
            "payment received",
            "payment confirmation",
            "thanks for your payment",
            "thank you for your payment",
            "your trip",
            "trip with",
            "your ride",
            "ride with",
            "refund",
            "pedido",
            "recibo",
            "tu compra",
            "su compra",
            "confirmación de compra",
            "pago recibido",
            "tu viaje",
            "reembolso",
        ];
        let cue = has_any(self.subj, SUBJECT_CUES);
        let number = self.order_number();
        if !cue && number.is_none() {
            return None;
        }
        // Promotions talk about orders too ("20% off your next order").
        let promo = has_any(
            self.subj,
            &[
                "% off",
                "sale",
                "deal",
                "coupon",
                "save ",
                "descuento",
                "oferta",
            ],
        );
        if promo && number.is_none() {
            return None;
        }
        // An order is a total someone paid: without one ("your order
        // MS-123 is on its way"), there is nothing to add up.
        let total = receipt_total(self.text)?;
        let status = if has_any(self.subj, &["refund", "reembolso"]) {
            Some("refunded")
        } else if has_any(self.subj, &["cancel"]) {
            Some("cancelled")
        } else if has_any(self.subj, &["delivered", "entregado"]) {
            Some("delivered")
        } else if shipped
            || has_any(
                self.subj,
                &["shipped", "on its way", "enviado", "en camino"],
            )
        {
            Some("shipped")
        } else if has_any(self.subj, &["return"]) {
            Some("returned")
        } else {
            None
        };
        let _ = shipped;
        let mut total_money = Some(money_of(total.amount.value, total.amount.currency));
        if status == Some("refunded") {
            if let Some(m) = total_money.as_mut() {
                m.value = -m.value.abs();
            }
        }
        Some(Order {
            merchant: Some(self.sender()),
            order_number: number,
            total: total_money,
            total_source: Some(total.line),
            items: vec![],
            status: status.map(String::from),
        })
    }

    // ------------------------------------------------------- reservations

    fn reservation(&self) -> Option<Reservation> {
        let head = format!("{} {}", self.subj, prefix(self.lower, 3000));
        let restaurant = has_any(&head, &["table for", "party of", "mesa para", "your table"])
            && has_any(&head, &["reservation", "reserva", "booked", "confirmed"]);
        let event = has_any(
            self.subj,
            &[
                "your tickets",
                "ticket confirmation",
                "tickets for",
                "your ticket",
                "entradas",
                "boletos",
                "tu entrada",
            ],
        );
        if !restaurant && !event {
            return None;
        }
        // A booking has a reference or a price ("tickets for Thursday?"
        // from a friend has neither).
        let reference = self.confirmation(true).or_else(|| self.order_number());
        let total = receipt_total(self.text).map(|t| money_of(t.amount.value, t.amount.currency));
        if event && !restaurant && reference.is_none() && total.is_none() {
            return None;
        }
        let dates = date_mentions(self.text, self.anchor);
        let d = dates
            .iter()
            .find(|d| d.explicit && d.date >= self.anchor)
            .or_else(|| dates.iter().find(|d| d.date >= self.anchor))?;
        let party = if restaurant {
            ["table for", "party of", "mesa para"].iter().find_map(|k| {
                let p = self.lower.find(k)?;
                let rest = self.lower[p + k.len()..].trim_start();
                let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                n.parse::<u32>().ok().or_else(|| small_number_word(rest))
            })
        } else {
            None
        };
        let name = if event {
            subject_after(
                self.m.subject,
                &[
                    "tickets for",
                    "ticket for",
                    "tickets:",
                    "entradas para",
                    "boletos para",
                    "order for",
                ],
            )
            .or_else(|| subject_place(self.m.subject))
        } else {
            subject_place(self.m.subject)
        }
        .or_else(|| Some(self.sender()));
        Some(Reservation {
            category: if restaurant { "restaurant" } else { "event" }.into(),
            name,
            start: Some(local(d.date, d.time.as_deref())),
            end: None,
            venue: None,
            address: addresses(self.text).into_iter().next(),
            confirmation: reference,
            party_size: party,
            status: has_any(self.subj, &["cancel", "cancelad"]).then(|| "cancelled".into()),
            total,
        })
    }

    // ------------------------------------------------------------ contact

    fn contact(&self) -> Option<Contact> {
        if self.m.bulk || crate::ask::automated(self.m.from_email) {
            return None;
        }
        // The signature: the last lines of what they wrote.
        let lines: Vec<&str> = self.text.lines().collect();
        let tail = lines[lines.len().saturating_sub(14)..].join("\n");
        let phones = phones(&tail);
        let addrs = addresses(&tail);
        if phones.is_empty() && addrs.is_empty() {
            return None;
        }
        // A table of numbers isn't a signature.
        if phones.len() > 3 {
            return None;
        }
        Some(Contact {
            phones,
            addresses: addrs,
        })
    }
}

fn small_number_word(s: &str) -> Option<u32> {
    let w = s.split_whitespace().next()?;
    Some(match w {
        "one" | "uno" | "una" => 1,
        "two" | "dos" => 2,
        "three" | "tres" => 3,
        "four" | "cuatro" => 4,
        "five" | "cinco" => 5,
        "six" | "seis" => 6,
        "seven" | "siete" => 7,
        "eight" | "ocho" => 8,
        _ => return None,
    })
}

/// "Your reservation at The Alder Hotel is confirmed" → "The Alder Hotel".
fn subject_place(subject: &str) -> Option<String> {
    subject_after(subject, &[" at ", " en ", " @ "])
}

fn subject_after(subject: &str, keys: &[&str]) -> Option<String> {
    let lower = subject.to_lowercase();
    // Offsets are shared between the two only when lowercasing kept lengths.
    if lower.len() != subject.len() {
        return None;
    }
    for k in keys {
        let Some(p) = lower.find(k).map(|p| p + k.len()) else {
            continue;
        };
        let rest = &subject[p..];
        let cut = [
            " is ", " has ", " - ", " – ", " | ", " on ", " for ", ": ", "!", ",", " (", " el ",
            " para ",
        ]
        .iter()
        .filter_map(|c| rest.find(c))
        .min()
        .unwrap_or(rest.len());
        let name = rest[..cut].trim().trim_end_matches('.');
        if !name.is_empty() && name.len() <= 80 {
            return Some(name.to_string());
        }
    }
    None
}

fn ship_status(head: &str) -> Option<String> {
    let s = if has_any(head, &["out for delivery", "en reparto", "sale a reparto"]) {
        "outForDelivery"
    } else if has_any(
        head,
        &[
            "delivered",
            "entregado",
            "was delivered",
            "has been delivered",
        ],
    ) {
        "delivered"
    } else if has_any(
        head,
        &[
            "exception",
            "delayed",
            "delay",
            "unable to deliver",
            "retrasado",
        ],
    ) {
        "exception"
    } else if has_any(head, &["in transit", "en tránsito", "en transito"]) {
        "inTransit"
    } else if has_any(
        head,
        &[
            "shipped",
            "on its way",
            "on the way",
            "has shipped",
            "enviado",
            "en camino",
            "dispatched",
        ],
    ) {
        "shipped"
    } else {
        return None;
    };
    Some(s.into())
}

/// Labeled amount-due lines, most specific first; (money, line).
fn bill_amount(text: &str) -> Option<(Money, String)> {
    const KEYS: &[&str] = &[
        "total amount due",
        "amount due",
        "balance due",
        "total due",
        "new balance",
        "statement balance",
        "amount to pay",
        "importe a pagar",
        "total a pagar",
        "importe total",
        "annual premium",
        "premium",
        "your balance",
        "balance",
        "saldo",
        "amount",
    ];
    let lines: Vec<&str> = text.lines().collect();
    for key in KEYS {
        for (n, line) in lines.iter().enumerate() {
            let l = line.to_lowercase();
            if !l.contains(key) || l.contains("minimum") || l.contains("mínimo") {
                continue;
            }
            let found = amounts(line)
                .into_iter()
                .last()
                .map(|a| (a, line.trim().to_string()));
            let found = found.or_else(|| {
                let next = lines[n + 1..].iter().find(|x| !x.trim().is_empty())?;
                amounts(next)
                    .into_iter()
                    .last()
                    .map(|a| (a, format!("{} {}", line.trim(), next.trim())))
            });
            if let Some((a, src)) = found {
                return Some((
                    money_of(a.value, a.currency),
                    crate::ask::extract::clip(&src, 160),
                ));
            }
        }
    }
    None
}

/// Clock times in `s` ("7:05 PM", "19:05", "7pm"), in order.
fn clocks(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = s.to_lowercase();
    let b = lower.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit()
            || (i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b':'))
        {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b':') {
            j += 1;
        }
        let num = &lower[i..j];
        let mut k = j;
        while k < b.len() && b[k] == b' ' {
            k += 1;
        }
        let rest = &lower[k..];
        let ampm = ["a.m.", "p.m.", "am", "pm"].into_iter().find(|x| {
            rest.starts_with(x)
                && rest[x.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
        });
        let colon = num.contains(':');
        let valid_colon = colon && {
            let (h, m) = num.split_once(':').unwrap_or(("", ""));
            h.len() <= 2 && m.len() == 2 && !m.contains(':')
        };
        if (valid_colon || (ampm.is_some() && !colon && num.len() <= 2))
            && (ampm.is_some() || valid_colon)
        {
            let t = match ampm {
                Some(a) => format!("{num}{}", a.replace('.', "")),
                None => num.to_string(),
            };
            if clock(&t).is_some() {
                out.push(t);
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Words that look like IATA codes but aren't airports in mail.
const NOT_AIRPORTS: &[&str] = &[
    "THE", "AND", "FOR", "YOU", "USD", "EUR", "GBP", "MXN", "CAD", "PDF", "FAQ", "VIP", "TSA",
    "ETA", "ETD", "NEW", "ALL", "OUT", "ARR", "DEP", "SEAT", "PNR", "FLT", "GMT", "UTC", "EST",
    "PST", "PDT", "EDT", "CST", "CDT", "MST", "MDT", "CET", "API", "APP", "TAX", "FEE", "NOT",
    "NON", "ONE", "TWO", "BAG", "MRS", "DOB", "REF", "TOP", "WWW", "COM", "AIR", "NOW", "FLY",
    "HRS", "MIN",
];

/// The first departure/arrival airport pair written in `w`: "SFO → LIS",
/// "SFO - LIS", "SFO to LIS", "(SFO) … (LIS)", or two known codes.
fn airports(w: &str) -> (Option<String>, Option<String>) {
    let b = w.as_bytes();
    let mut codes: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        let is3 = b[i..i + 3].iter().all(u8::is_ascii_uppercase)
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && b.get(i + 3).is_none_or(|c| !c.is_ascii_alphanumeric());
        if is3 {
            let c = &w[i..i + 3];
            if !NOT_AIRPORTS.contains(&c) {
                codes.push((i, c.to_string()));
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    // An explicit pair: "SFO → LIS", "SFO to LIS", or with the city
    // between, "(SFO) → Lisbon (LIS)".
    const ARROWS: &[&str] = &["→", "->", "-", "–", "—", ">", "to", "a", "✈", "/"];
    for pair in codes.windows(2) {
        let between = w[pair[0].0 + 3..pair[1].0].trim();
        let between = between.trim_matches(|c: char| c == ')' || c == '(' || c.is_whitespace());
        let arrow_then_city = ARROWS.iter().any(|a| {
            between.strip_prefix(a).is_some_and(|rest| {
                let rest = rest.trim();
                (a.chars().all(|c| !c.is_alphabetic())
                    || rest.starts_with(|c: char| c.is_uppercase()))
                    && rest.split_whitespace().count() <= 4
                    && rest
                        .chars()
                        .all(|c| c.is_alphabetic() || c == ' ' || c == '.' || c == '\'' || c == '-')
            })
        });
        if (ARROWS.contains(&between) || between.is_empty() || arrow_then_city)
            && pair[0].1 != pair[1].1
        {
            let explicit = !between.is_empty();
            let known = super::airport_by_code(&pair[0].1).is_some()
                && super::airport_by_code(&pair[1].1).is_some();
            if explicit || known {
                return (Some(pair[0].1.clone()), Some(pair[1].1.clone()));
            }
        }
    }
    // Two known airports anywhere, in order.
    let known: Vec<&String> = codes
        .iter()
        .map(|c| &c.1)
        .filter(|c| super::airport_by_code(c).is_some())
        .collect();
    match known.as_slice() {
        [a, b, ..] if a != b => (Some((*a).clone()), Some((*b).clone())),
        [a] => (Some((*a).clone()), None),
        _ => (None, None),
    }
}

/// Phone numbers: "+1 415 555 0199", "(415) 555-0199", "415.555.0199",
/// "+34 612 34 56 78". Needs separators or a leading "+" or "(" so order
/// numbers and dates don't pass; 8–15 digits (E.164's maximum).
pub(crate) fn phones(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // Not the tail of a longer number ("9400 1118 9922 …").
        let after_digits = i >= 2 && b[i - 1] == b' ' && b[i - 2].is_ascii_digit();
        let start_ok = (b[i] == b'+' || b[i] == b'(' || b[i].is_ascii_digit())
            && !after_digits
            && (i == 0
                || !(b[i - 1].is_ascii_alphanumeric()
                    || b[i - 1] == b'/'
                    || b[i - 1] == b'-'
                    || b[i - 1] == b'.'));
        if !start_ok {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut digits = 0;
        let mut seps = 0;
        let mut last_digit = i;
        while j < b.len() {
            let c = b[j];
            if c.is_ascii_digit() {
                digits += 1;
                last_digit = j;
            } else if matches!(c, b' ' | b'-' | b'.' | b'(' | b')') || (c == b'+' && j == i) {
                if c != b'+' {
                    seps += 1;
                }
                // Two separators in a row (other than ") ") end it.
                if j + 1 < b.len()
                    && !b[j + 1].is_ascii_digit()
                    && !(c == b')' && b[j + 1] == b' ')
                    && b[j + 1] != b'('
                {
                    break;
                }
            } else {
                break;
            }
            j += 1;
        }
        let cand = &s[i..=last_digit.max(i)];
        let plus = b[i] == b'+';
        let paren = cand.contains('(');
        let ok = (8..=15).contains(&digits)
            && (plus || paren || seps >= 2)
            && b.get(last_digit + 1)
                .is_none_or(|c| !c.is_ascii_alphanumeric())
            && !looks_like_date(cand);
        if ok {
            let norm = cand.trim().to_string();
            if !out.contains(&norm) {
                out.push(norm);
            }
            i = last_digit + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn looks_like_date(s: &str) -> bool {
    let parts: Vec<&str> = s
        .split(['-', '.', '/', ' '])
        .filter(|p| !p.is_empty())
        .collect();
    parts.len() == 3
        && (parts[0].len() == 4 || parts[2].len() == 4)
        && parts.iter().all(|p| p.len() <= 4)
}

/// Street suffixes (US) and street types (Spanish) that make a line an
/// address.
const STREET: &[&str] = &[
    "street",
    "st",
    "avenue",
    "ave",
    "road",
    "rd",
    "boulevard",
    "blvd",
    "drive",
    "dr",
    "lane",
    "ln",
    "way",
    "court",
    "ct",
    "place",
    "pl",
    "parkway",
    "pkwy",
    "terrace",
    "ter",
    "circle",
    "cir",
    "highway",
    "hwy",
    "square",
    "sq",
    "alley",
    "trail",
    "plaza",
];
const CALLE: &[&str] = &[
    "calle", "avenida", "av", "avda", "paseo", "plaza", "carrera", "rua",
];

/// Postal addresses: "418 Alder St, Unit 3B" (+ "Portland, OR 97205" on
/// the same or next line) or "Calle Mayor 12, 28013 Madrid".
pub(crate) fn addresses(s: &str) -> Vec<String> {
    let lines: Vec<&str> = s.lines().map(str::trim).collect();
    let mut out: Vec<String> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        if line.len() > 140 {
            continue;
        }
        let words: Vec<String> = line
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|w| !w.is_empty())
            .map(|w| w.trim_end_matches('.').to_lowercase())
            .collect();
        let Some(first) = words.first() else { continue };
        let us = first.chars().all(|c| c.is_ascii_digit())
            && first.len() <= 6
            && words
                .iter()
                .skip(2)
                .take(5)
                .any(|w| STREET.contains(&w.as_str()));
        let es = CALLE.contains(&first.as_str())
            && words
                .iter()
                .skip(2)
                .any(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()));
        if !us && !es {
            continue;
        }
        // Where the address starts inside the line ("Office: 418 Alder St").
        let mut addr = line.to_string();
        // City/state/ZIP on the next line.
        if let Some(next) = lines.get(n + 1) {
            if city_line(next) && !city_line(line) {
                addr = format!("{addr}, {next}");
            }
        }
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    out
}

/// "Portland, OR 97205", "28013 Madrid", "Portland OR 97205-1234".
fn city_line(l: &str) -> bool {
    let t: Vec<&str> = l
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    if t.len() < 2 || t.len() > 6 || l.len() > 60 {
        return false;
    }
    let zip = |w: &str| {
        let d = w.split('-').next().unwrap_or(w);
        d.len() == 5 && d.chars().all(|c| c.is_ascii_digit())
    };
    let state = |w: &str| w.len() == 2 && w.chars().all(|c| c.is_ascii_uppercase());
    (t.len() >= 3 && state(t[t.len() - 2]) && zip(t[t.len() - 1])) || zip(t[0])
}
