//! The simulated model: the question's gold query written as the on-device
//! model's `QueryDraft` (strings, as `@Generable struct MailQuery` produces
//! them), then validated and answered by `Store::ask_draft` exactly as the
//! app does with a real model's output. A perfect parse, so this bounds
//! what the model path can add; it measures the plumbing, not the model.

use penguin_core::ask::{AskAnswer, AskScope, QueryDraft};
use penguin_core::Store;
use penguin_eval::corpus::world::{CITIES, CLIENTS, COLLEAGUES, FRIENDS, MERCHANTS, PARTNERS};

use crate::oracle::{Gold, Group, Measure, Op, Side, Subject};

/// How a model would name an entity: the display name, not our tag key.
fn display(key: &str) -> String {
    if let Some(c) = CITIES.iter().find(|c| c.0 == key) {
        return c.1.to_string();
    }
    if let Some(m) = MERCHANTS.iter().find(|m| m.key == key) {
        return m.name.to_string();
    }
    if let Some(p) = COLLEAGUES.iter().chain(PARTNERS).chain(FRIENDS).chain(CLIENTS).find(|p| p.key == key) {
        return p.name.to_string();
    }
    let mut c = key.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn to_draft(g: &Gold) -> Option<QueryDraft> {
    if g.op == Op::Passage {
        return None;
    }
    Some(QueryDraft {
        subject: match g.subject {
            Subject::Flights => "flights",
            Subject::Stays => "stays",
            Subject::Orders => "orders",
            Subject::Parcels => "parcels",
            Subject::Bills => "bills",
            Subject::Bookings => "bookings",
            Subject::Spending => "spending",
            Subject::Messages => "messages",
        }
        .into(),
        op: format!("{:?}", g.op).to_lowercase(),
        measure: match g.measure {
            Measure::Items => "items",
            Measure::Money => "money",
            Measure::Nights => "nights",
            Measure::Trips => "trips",
        }
        .into(),
        group_by: match g.group {
            None => "none",
            Some(Group::Month) => "month",
            Some(Group::Year) => "year",
            Some(Group::Merchant) => "merchant",
            Some(Group::Place) => "place",
            Some(Group::Person) => "person",
        }
        .into(),
        timeframe: g.timeframe.clone().unwrap_or_default(),
        compare: g
            .vs
            .iter()
            .map(|s| match s {
                Side::Range(_, _, t) => t.clone(),
                Side::Name(n) => display(n),
            })
            .collect(),
        place: g.place.as_deref().map(display).unwrap_or_default(),
        merchant: g.merchant.as_deref().map(display).unwrap_or_default(),
        person: g.person.as_deref().map(display).unwrap_or_default(),
        direction: g.dir.unwrap_or("any").into(),
        field: match g.field.as_deref() {
            Some("confirmation") => "confirmation",
            Some("tracking") => "tracking",
            Some("ordernumber") => "orderNumber",
            Some("flightnumber") => "flightNumber",
            Some("amount") => "amount",
            Some("due") | Some("date") => "date",
            _ => "none",
        }
        .into(),
        tense: match g.future {
            Some(true) => "future",
            Some(false) => "past",
            None => "any",
        }
        .into(),
    })
}

pub fn simulated(store: &Store, question: &str, g: &Gold, scope: &AskScope, now: i64, off: i32) -> Option<AskAnswer> {
    let d = to_draft(g)?;
    store.ask_draft(question, d, scope, now, off).ok().flatten()
}
