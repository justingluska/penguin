//! Structured facts pulled out of mail at index time: flights, hotel stays,
//! orders and receipts, shipments, bills and invoices, reservations and
//! tickets, and a person's phone numbers and postal addresses.
//!
//! Deterministic and on device. Sources, most trusted first:
//!
//! 1. **schema.org JSON-LD** (`<script type="application/ld+json">`), the
//!    format Google's Email Markup documents for FlightReservation,
//!    LodgingReservation, FoodEstablishmentReservation, EventReservation,
//!    RentalCarReservation, Order, ParcelDelivery and Invoice (`schema_org.rs`).
//! 2. **schema.org microdata** (`itemscope`/`itemtype`/`itemprop`), read
//!    with the WHATWG property-value rules into the same tree and mapped by
//!    the same code (`microdata.rs`).
//! 3. **Patterns** on the authored text, for the (much larger) share of mail
//!    without markup: labeled order/confirmation numbers, receipt totals,
//!    due dates, check-in/check-out, flight numbers checked against IATA
//!    airline designators, airports, and tracking numbers checked with each
//!    carrier's check digit (`patterns.rs`, `tracking.rs`).
//!
//! Markup wins: patterns only add kinds the markup didn't cover. Every
//! extracted value comes from the message itself; nothing is looked up.
//! Stored in the `extracted` table (`store_extracted.rs`) and read by Ask.
//! Mirrored in `apps/desktop/src/lib/types.ts` (Extracted and friends).

mod microdata;
mod patterns;
mod places;
mod schema_org;
#[cfg(test)]
mod tests;
pub(crate) mod tracking;

use serde::{Deserialize, Serialize};

pub(crate) use patterns::addresses as patterns_addresses;
pub(crate) use places::{airline_by_code, airport_by_code, airport_names, airports_for_place, city_in};

/// Bumped when the extractors change enough that stored rows should be
/// recomputed (the scanner re-reads messages scanned by an older version).
pub const EXTRACTOR_VERSION: i64 = 3;

/// An amount of money.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    pub value: f64,
    /// ISO 4217 ("USD").
    pub currency: String,
}

/// Where a fact came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// schema.org JSON-LD in the HTML part.
    JsonLd,
    /// schema.org microdata in the HTML part.
    Microdata,
    /// Fixed text patterns.
    Pattern,
}

impl Source {
    pub(crate) fn code(self) -> i64 {
        match self {
            Source::JsonLd => 1,
            Source::Microdata => 2,
            Source::Pattern => 3,
        }
    }
}

/// One flight segment.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Flight {
    /// "TAP Air Portugal".
    pub airline: Option<String>,
    /// IATA designator ("TP").
    pub airline_code: Option<String>,
    /// "TP 238".
    pub flight_number: Option<String>,
    /// Booking reference / record locator.
    pub confirmation: Option<String>,
    pub passenger: Option<String>,
    /// IATA airport codes and names.
    pub depart_airport: Option<String>,
    pub depart_name: Option<String>,
    pub arrive_airport: Option<String>,
    pub arrive_name: Option<String>,
    /// Local wall time at the airport, "2026-10-02T19:05" (or a date alone).
    pub depart_time: Option<String>,
    pub arrive_time: Option<String>,
    /// "confirmed", "cancelled", "changed".
    pub status: Option<String>,
    pub total: Option<Money>,
}

/// A hotel or rental stay.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lodging {
    pub name: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// Local "2026-10-02T15:00" or "2026-10-02".
    pub checkin: Option<String>,
    pub checkout: Option<String>,
    pub confirmation: Option<String>,
    pub guest: Option<String>,
    pub status: Option<String>,
    pub total: Option<Money>,
}

/// A purchase: an order confirmation or a receipt.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    /// "Uber", "Acme Outfitters".
    pub merchant: Option<String>,
    pub order_number: Option<String>,
    pub total: Option<Money>,
    /// The line or property the total came from ("Total $23.40").
    pub total_source: Option<String>,
    /// Item names (at most five).
    pub items: Vec<String>,
    /// "processing", "shipped", "delivered", "cancelled", "returned", "refunded".
    pub status: Option<String>,
}

/// A parcel on its way.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Shipment {
    /// "UPS", "USPS", "FedEx", "DHL", "Amazon", or the markup's name.
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    /// The check digit was verified (or the number came from markup).
    pub verified: bool,
    pub tracking_url: Option<String>,
    /// "shipped", "inTransit", "outForDelivery", "delivered", "exception".
    pub status: Option<String>,
    /// Expected delivery, local date "2026-10-02".
    pub expected: Option<String>,
    pub merchant: Option<String>,
    pub order_number: Option<String>,
    pub items: Vec<String>,
}

/// An invoice or bill to pay.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bill {
    pub biller: Option<String>,
    pub invoice_number: Option<String>,
    pub amount_due: Option<Money>,
    pub amount_source: Option<String>,
    /// Local date "2026-10-31".
    pub due_date: Option<String>,
    /// "due", "paid", "overdue".
    pub status: Option<String>,
}

/// An event ticket, restaurant table, rental car, train or bus.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reservation {
    /// "event", "restaurant", "rentalCar", "train", "bus".
    pub category: String,
    pub name: Option<String>,
    /// Local "2026-10-02T19:30" or a date.
    pub start: Option<String>,
    pub end: Option<String>,
    pub venue: Option<String>,
    pub address: Option<String>,
    pub confirmation: Option<String>,
    pub party_size: Option<u32>,
    pub status: Option<String>,
    pub total: Option<Money>,
}

/// Phone numbers and postal addresses a person wrote in their own message
/// (a signature, usually).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub phones: Vec<String>,
    pub addresses: Vec<String>,
}

/// One extracted fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Extracted {
    Flight(Flight),
    Lodging(Lodging),
    Order(Order),
    Shipment(Shipment),
    Bill(Bill),
    Reservation(Reservation),
    Contact(Contact),
}

impl Extracted {
    /// The `kind` column.
    pub fn kind(&self) -> &'static str {
        match self {
            Extracted::Flight(_) => "flight",
            Extracted::Lodging(_) => "lodging",
            Extracted::Order(_) => "order",
            Extracted::Shipment(_) => "shipment",
            Extracted::Bill(_) => "bill",
            Extracted::Reservation(_) => "reservation",
            Extracted::Contact(_) => "contact",
        }
    }

    /// The reference number that identifies it (`ref` column): booking
    /// reference, order number, tracking number, invoice number.
    pub fn reference(&self) -> Option<&str> {
        match self {
            Extracted::Flight(f) => f.confirmation.as_deref(),
            Extracted::Lodging(l) => l.confirmation.as_deref(),
            Extracted::Order(o) => o.order_number.as_deref(),
            Extracted::Shipment(s) => s.tracking_number.as_deref(),
            Extracted::Bill(b) => b.invoice_number.as_deref(),
            Extracted::Reservation(r) => r.confirmation.as_deref(),
            Extracted::Contact(_) => None,
        }
    }

    /// The money it's about (`amount`, `currency` columns).
    pub fn money(&self) -> Option<&Money> {
        match self {
            Extracted::Flight(f) => f.total.as_ref(),
            Extracted::Lodging(l) => l.total.as_ref(),
            Extracted::Order(o) => o.total.as_ref(),
            Extracted::Shipment(_) | Extracted::Contact(_) => None,
            Extracted::Bill(b) => b.amount_due.as_ref(),
            Extracted::Reservation(r) => r.total.as_ref(),
        }
    }

    /// The local date-time the fact is about (`at` column): departure,
    /// check-in, due date, event start, expected delivery.
    pub fn when(&self) -> Option<&str> {
        match self {
            Extracted::Flight(f) => f.depart_time.as_deref(),
            Extracted::Lodging(l) => l.checkin.as_deref(),
            Extracted::Order(_) | Extracted::Contact(_) => None,
            Extracted::Shipment(s) => s.expected.as_deref(),
            Extracted::Bill(b) => b.due_date.as_deref(),
            Extracted::Reservation(r) => r.start.as_deref(),
        }
    }
}

/// An extracted fact with its provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub fact: Extracted,
    pub source: Source,
}

/// What the extractors read from one message.
pub struct MailInput<'a> {
    pub subject: &'a str,
    pub from_email: &'a str,
    pub from_name: Option<&'a str>,
    /// Unix ms (anchors relative dates like "Thursday").
    pub date: i64,
    /// Authored plain text (quoted history removed).
    pub text: &'a str,
    /// The raw HTML part, when there is one.
    pub html: Option<&'a str>,
    /// Bulk mail (List-Unsubscribe or a promotions/updates category).
    pub bulk: bool,
    /// You sent it.
    pub sent: bool,
}

/// Extract every fact from one message. Sent mail yields nothing.
pub fn extract(m: &MailInput) -> Vec<Found> {
    if m.sent {
        return Vec::new();
    }
    let mut out: Vec<Found> = Vec::new();
    if let Some(html) = m.html {
        for fact in schema_org::from_json_ld(html) {
            out.push(Found {
                fact,
                source: Source::JsonLd,
            });
        }
        if out.is_empty() {
            for fact in microdata::from_microdata(html) {
                out.push(Found {
                    fact,
                    source: Source::Microdata,
                });
            }
        }
    }
    let markup = out.len();
    for fact in patterns::extract(m) {
        let described: Vec<usize> = (0..markup)
            .filter(|&i| out[i].fact.kind() == fact.kind())
            .collect();
        if described.is_empty() {
            out.push(Found {
                fact,
                source: Source::Pattern,
            });
            continue;
        }
        // Markup already described this thing: trust it, and take from the
        // text only what the markup left out (a FlightReservation without
        // departure times, an Order without its items).
        if let Some(&i) = described
            .iter()
            .find(|&&i| same_thing(&out[i].fact, &fact, described.len()))
        {
            fill_missing(&mut out[i].fact, &fact);
            continue;
        }
        // Another flight of a booking the markup described (the return leg
        // of a round trip whose markup lists only the outbound one).
        if let Extracted::Flight(p) = &fact {
            let same_booking = described.iter().any(|&i| {
                matches!(&out[i].fact, Extracted::Flight(f) if same_ref(f.confirmation.as_deref(), p.confirmation.as_deref()))
            });
            if same_booking && p.depart_time.is_some() {
                out.push(Found {
                    fact,
                    source: Source::Pattern,
                });
            }
        }
    }
    out
}

/// Reference numbers compare without case, spaces or dashes.
fn norm_ref(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn same_ref(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if !norm_ref(a).is_empty() && norm_ref(a) == norm_ref(b))
}

/// "TP 238" = "TP238" = "tp 0238".
fn norm_flight(s: &str) -> String {
    let n = norm_ref(s);
    let (code, num) = n.split_at(n.len().min(2));
    format!("{code}{}", num.trim_start_matches('0'))
}

/// Do a markup fact and a text fact of the same kind describe one thing?
/// `described`: how many markup facts of this kind the message has.
fn same_thing(markup: &Extracted, text: &Extracted, described: usize) -> bool {
    if let (Extracted::Flight(a), Extracted::Flight(b)) = (markup, text) {
        return match (&a.flight_number, &b.flight_number) {
            (Some(x), Some(y)) => norm_flight(x) == norm_flight(y),
            // A leg the text reads without a number is another flight.
            (_, None) => false,
            (None, Some(_)) => {
                same_ref(a.confirmation.as_deref(), b.confirmation.as_deref())
                    || (described == 1 && a.confirmation.is_none())
            }
        };
    }
    match (markup.reference(), text.reference()) {
        (Some(_), Some(_)) => same_ref(markup.reference(), text.reference()),
        _ => described == 1,
    }
}

/// Fill the fields `into` leaves empty from `from` (same kind).
fn fill_missing(into: &mut Extracted, from: &Extracted) {
    use serde_json::Value;
    let (Ok(Value::Object(mut a)), Ok(Value::Object(b))) =
        (serde_json::to_value(&*into), serde_json::to_value(from))
    else {
        return;
    };
    for (k, v) in b {
        let empty = match a.get(&k) {
            None | Some(Value::Null) => true,
            Some(Value::Array(x)) => x.is_empty(),
            Some(Value::String(s)) => s.is_empty(),
            _ => false,
        };
        if empty && !v.is_null() {
            a.insert(k, v);
        }
    }
    if let Ok(merged) = serde_json::from_value(Value::Object(a)) {
        *into = merged;
    }
}

/// The merchant/airline/carrier/biller as a lowercase key for lookups
/// ("Uber Receipts" → "uber"), falling back to the sender's domain label.
pub fn merchant_key(name: Option<&str>, from_email: &str) -> String {
    if let Some(n) = name {
        let k = clean_org(n).to_lowercase();
        if !k.is_empty() {
            return k;
        }
    }
    org_label(from_email)
}

/// "noreply@email.uber.example" → "uber". Skips mail-service subdomains.
pub fn org_label(email: &str) -> String {
    let domain = email.rsplit('@').next().unwrap_or("").to_lowercase();
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return domain;
    }
    // Second-level label, skipping two-part public suffixes ("co.uk").
    let n = labels.len();
    let two_part = n >= 3 && matches!(labels[n - 2], "co" | "com" | "org" | "net" | "ac" | "gov");
    let idx = if two_part { n - 3 } else { n - 2 };
    labels[idx].to_string()
}

/// A sender name without mail-department words ("Uber Receipts" → "Uber").
pub fn clean_org(name: &str) -> String {
    const NOISE: &[&str] = &[
        "receipts",
        "receipt",
        "orders",
        "order",
        "billing",
        "notifications",
        "notification",
        "no-reply",
        "noreply",
        "team",
        "support",
        "updates",
        "shipping",
        "shipment",
        "delivery",
        "store",
        "customer service",
        "reservations",
        "confirmation",
        "confirmations",
        "via",
        "info",
    ];
    let mut words: Vec<&str> = name
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| {
                matches!(c, '"' | '\'' | '(' | ')' | ',' | '.' | ':' | '-' | '|')
            })
        })
        .filter(|w| !w.is_empty())
        .collect();
    while words.len() > 1
        && words
            .last()
            .is_some_and(|w| NOISE.contains(&w.to_lowercase().as_str()))
    {
        words.pop();
    }
    words.join(" ")
}
