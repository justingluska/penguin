//! schema.org markup → facts. JSON-LD blocks are parsed here; microdata is
//! turned into the same JSON shape by `microdata.rs` and mapped by
//! [`map_items`].
//!
//! Types and properties follow schema.org and Google's Email Markup
//! reference: FlightReservation (reservationFor: Flight with flightNumber,
//! airline, departureAirport/arrivalAirport, departureTime/arrivalTime),
//! LodgingReservation (checkinTime/checkoutTime, reservationFor:
//! LodgingBusiness), FoodEstablishmentReservation, EventReservation,
//! RentalCarReservation, TrainReservation, BusReservation, Order
//! (merchant, orderNumber, price/priceCurrency, acceptedOffer, orderStatus),
//! ParcelDelivery (trackingNumber, carrier, expectedArrivalUntil,
//! partOfOrder, deliveryStatus) and Invoice (provider, totalPaymentDue,
//! paymentDueDate, paymentStatus). Values may be strings, numbers, nested
//! objects or arrays of either; every reader here accepts all of them.

use serde_json::Value;

use super::{Bill, Extracted, Flight, Lodging, Money, Order, Reservation, Shipment};

/// Most bytes of one JSON-LD block parsed (real ones are a few KB).
const MAX_BLOCK: usize = 256 * 1024;

/// Facts from every `<script type="application/ld+json">` in `html`.
pub(crate) fn from_json_ld(html: &str) -> Vec<Extracted> {
    if memchr::memmem::find(html.as_bytes(), b"ld+json").is_none() {
        return Vec::new();
    }
    let mut roots = Vec::new();
    for block in json_ld_blocks(html) {
        let parsed = serde_json::from_str::<Value>(block.trim()).or_else(|_| {
            // Some senders HTML-escape the script body.
            serde_json::from_str::<Value>(&unescape(block.trim()))
        });
        if let Ok(v) = parsed {
            roots.push(v);
        }
    }
    map_items(&roots)
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The bodies of JSON-LD script elements (ASCII case-insensitive tags).
fn json_ld_blocks(html: &str) -> Vec<&str> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = lower[from..].find("<script") {
        let tag_start = from + p;
        let Some(tag_len) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + tag_len + 1;
        let tag = &lower[tag_start..tag_end];
        let Some(close) = lower[tag_end..].find("</script") else {
            break;
        };
        let body_end = tag_end + close;
        if tag.contains("ld+json") && body_end - tag_end <= MAX_BLOCK {
            out.push(&html[tag_end..body_end]);
        }
        from = body_end;
    }
    out
}

// ------------------------------------------------------------ readers

/// The schema.org type names of a node ("FlightReservation"), without
/// the "http://schema.org/" prefix.
fn types(v: &Value) -> Vec<String> {
    let strip = |s: &str| s.rsplit(['/', '#', ':']).next().unwrap_or(s).to_string();
    match v.get("@type").or_else(|| v.get("type")) {
        Some(Value::String(s)) => vec![strip(s)],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(strip).collect(),
        _ => vec![],
    }
}

fn is(v: &Value, t: &str) -> bool {
    types(v).iter().any(|x| x.eq_ignore_ascii_case(t))
}

/// Property `k`: the first value when it's an array.
fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    match v.get(k)? {
        Value::Array(a) => a.first(),
        x => Some(x),
    }
}

/// Every value of property `k`.
fn all<'a>(v: &'a Value, k: &str) -> Vec<&'a Value> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(x) => vec![x],
        None => vec![],
    }
}

/// A property as text: a string, a number, or a nested thing's name.
fn text(v: &Value, k: &str) -> Option<String> {
    as_text(get(v, k)?)
}

fn as_text(x: &Value) -> Option<String> {
    let s = match x {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Object(_) => {
            return text(x, "name").or_else(|| text(x, "@id").filter(|s| !s.starts_with("http")))
        }
        _ => return None,
    };
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

fn number(x: &Value) -> Option<f64> {
    match x {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t: String = s
                .trim()
                .chars()
                .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'))
                .collect();
            if t.contains(',') && t.contains('.') {
                // "1,234.56" or "1.234,56": the last separator is decimal.
                if t.rfind(',') > t.rfind('.') {
                    t.replace('.', "").replace(',', ".").parse().ok()
                } else {
                    t.replace(',', "").parse().ok()
                }
            } else if t.contains(',') && t.rsplit(',').next().is_some_and(|f| f.len() <= 2) {
                t.replace(',', ".").parse().ok()
            } else {
                t.replace(',', "").parse().ok()
            }
        }
        _ => None,
    }
}

/// Money from `price_key`/`currency_key` on `v`, or a nested
/// PriceSpecification.
fn money(v: &Value, price_keys: &[&str], currency_key: &str) -> Option<Money> {
    for k in price_keys {
        let Some(p) = get(v, k) else { continue };
        if p.is_object() {
            let value = get(p, "price")
                .or_else(|| get(p, "value"))
                .and_then(number)?;
            let cur = text(p, "priceCurrency")
                .or_else(|| text(p, "currency"))
                .or_else(|| text(v, currency_key))?;
            return Some(Money {
                value,
                currency: cur.to_uppercase(),
            });
        }
        if let (Some(value), Some(cur)) = (number(p), text(v, currency_key)) {
            return Some(Money {
                value,
                currency: cur.to_uppercase(),
            });
        }
    }
    None
}

/// "2026-10-02T19:05:00-07:00" → "2026-10-02T19:05" (the local wall time
/// as written; offsets are dropped because the time is local to the place).
pub(crate) fn local_time(s: &str) -> Option<String> {
    let s = s.trim();
    let date = s.get(..10)?;
    let ok_date = date.len() == 10
        && date.as_bytes()[4] == b'-'
        && date.as_bytes()[7] == b'-'
        && date
            .bytes()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !ok_date {
        return None;
    }
    match s.get(10..11) {
        Some("T") | Some(" ") => {
            let hm = s.get(11..16)?;
            if hm.as_bytes()[2] == b':'
                && hm
                    .bytes()
                    .enumerate()
                    .all(|(i, c)| i == 2 || c.is_ascii_digit())
            {
                Some(format!("{date}T{hm}"))
            } else {
                Some(date.to_string())
            }
        }
        _ => Some(date.to_string()),
    }
}

fn time(v: &Value, k: &str) -> Option<String> {
    local_time(&text(v, k)?)
}

fn status(v: &Value, k: &str) -> Option<String> {
    let s = text(v, k)?;
    let s = s.rsplit('/').next().unwrap_or(&s).to_lowercase();
    Some(
        match s.as_str() {
            "reservationconfirmed" | "confirmed" => "confirmed",
            "reservationcancelled" | "cancelled" | "canceled" => "cancelled",
            "reservationpending" | "pending" => "pending",
            "reservationhold" => "hold",
            "orderprocessing" | "processing" => "processing",
            "orderintransit" | "intransit" => "inTransit",
            "orderdelivered" | "delivered" => "delivered",
            "orderpickupavailable" => "readyForPickup",
            "orderreturned" | "returned" => "returned",
            "ordercancelled" => "cancelled",
            "orderpaymentdue" | "paymentdue" => "due",
            "orderproblem" | "problem" => "exception",
            "outfordelivery" => "outForDelivery",
            "paymentcomplete" | "paymentautomaticallyapplied" | "paid" => "paid",
            "paymentpastdue" | "pastdue" => "overdue",
            "paymentdeclined" => "declined",
            other => return Some(other.to_string()),
        }
        .to_string(),
    )
}

fn address(v: &Value) -> Option<String> {
    let a = get(v, "address")?;
    if let Some(s) = a.as_str() {
        return Some(s.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    let parts: Vec<String> = [
        "streetAddress",
        "addressLocality",
        "addressRegion",
        "postalCode",
        "addressCountry",
    ]
    .iter()
    .filter_map(|k| text(a, k))
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// Gmail's `reservationNumber`, or schema.org's `reservationId`.
fn reservation_id(v: &Value) -> Option<String> {
    text(v, "reservationNumber").or_else(|| text(v, "reservationId"))
}

fn person_name(v: &Value, k: &str) -> Option<String> {
    let p = get(v, k)?;
    as_text(p).or_else(|| {
        let g = text(p, "givenName")?;
        Some(match text(p, "familyName") {
            Some(f) => format!("{g} {f}"),
            None => g,
        })
    })
}

fn item_names(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for offer in all(v, "acceptedOffer")
        .into_iter()
        .chain(all(v, "orderedItem"))
    {
        let name = get(offer, "itemOffered")
            .and_then(as_text)
            .or_else(|| get(offer, "orderedItem").and_then(as_text))
            .or_else(|| text(offer, "name"));
        if let Some(n) = name {
            if !out.contains(&n) && out.len() < 5 {
                out.push(n);
            }
        }
    }
    out
}

// ------------------------------------------------------------ mapping

/// Map schema.org nodes (roots, `@graph`s and arrays) to facts.
pub(crate) fn map_items(roots: &[Value]) -> Vec<Extracted> {
    let mut nodes: Vec<&Value> = Vec::new();
    fn collect<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        match v {
            Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
            Value::Object(_) => {
                if let Some(g) = v.get("@graph") {
                    collect(g, out);
                }
                if !types(v).is_empty() {
                    out.push(v);
                }
            }
            _ => {}
        }
    }
    roots.iter().for_each(|r| collect(r, &mut nodes));
    let mut out = Vec::new();
    for v in nodes {
        if let Some(f) = map_one(v) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

fn map_one(v: &Value) -> Option<Extracted> {
    if is(v, "FlightReservation") {
        return Some(Extracted::Flight(flight(v)));
    }
    if is(v, "LodgingReservation") {
        let place = get(v, "reservationFor");
        return Some(Extracted::Lodging(Lodging {
            name: place.and_then(as_text),
            address: place.and_then(address),
            phone: place.and_then(|p| text(p, "telephone")),
            // Gmail's reference spells these checkinDate/checkoutDate;
            // schema.org has checkinTime/checkoutTime. Accept all.
            checkin: time(v, "checkinTime")
                .or_else(|| time(v, "checkinDate"))
                .or_else(|| place.and_then(|p| time(p, "checkinTime"))),
            checkout: time(v, "checkoutTime")
                .or_else(|| time(v, "checkoutDate"))
                .or_else(|| place.and_then(|p| time(p, "checkoutTime"))),
            confirmation: reservation_id(v),
            guest: person_name(v, "underName"),
            status: status(v, "reservationStatus"),
            total: money(v, &["totalPrice", "price"], "priceCurrency"),
        }));
    }
    for (t, category) in [
        ("FoodEstablishmentReservation", "restaurant"),
        ("EventReservation", "event"),
        ("RentalCarReservation", "rentalCar"),
        ("TrainReservation", "train"),
        ("BusReservation", "bus"),
    ] {
        if is(v, t) {
            return Some(Extracted::Reservation(reservation(v, category)));
        }
    }
    if is(v, "ParcelDelivery") {
        let order = get(v, "partOfOrder");
        let carrier = get(v, "carrier")
            .or_else(|| get(v, "provider"))
            .and_then(as_text);
        let number = text(v, "trackingNumber").map(|s| s.replace(' ', ""));
        let url = text(v, "trackingUrl").filter(|u| u.starts_with("https://"));
        let status = get(v, "deliveryStatus")
            .and_then(|d| {
                if d.is_object() {
                    status(d, "hasDeliveryMethod").or_else(|| as_text(d).map(|s| s.to_lowercase()))
                } else {
                    status(v, "deliveryStatus")
                }
            })
            .or_else(|| order.and_then(|o| status(o, "orderStatus")));
        return Some(Extracted::Shipment(Shipment {
            tracking_url: url.or_else(|| {
                let c = carrier.as_deref()?;
                super::tracking::tracking_url(c, number.as_deref()?)
            }),
            carrier,
            verified: number.is_some(),
            tracking_number: number,
            status,
            expected: time(v, "expectedArrivalUntil")
                .or_else(|| time(v, "expectedArrivalFrom"))
                .map(|t| t[..10].to_string()),
            merchant: order.and_then(|o| {
                get(o, "merchant")
                    .or_else(|| get(o, "seller"))
                    .and_then(as_text)
            }),
            order_number: order.and_then(|o| text(o, "orderNumber")),
            items: order.map(item_names).unwrap_or_default(),
        }));
    }
    if is(v, "Order") {
        return Some(Extracted::Order(Order {
            merchant: get(v, "merchant")
                .or_else(|| get(v, "seller"))
                .and_then(as_text),
            order_number: text(v, "orderNumber"),
            total: money(
                v,
                &["price", "totalPrice", "totalPaymentDue"],
                "priceCurrency",
            )
            .or_else(|| {
                // Price only on the offers: sum them when they share a currency.
                let offers = all(v, "acceptedOffer");
                let ms: Vec<Money> = offers
                    .iter()
                    .filter_map(|o| money(o, &["price"], "priceCurrency"))
                    .collect();
                (!ms.is_empty() && ms.iter().all(|m| m.currency == ms[0].currency)).then(|| Money {
                    value: ms.iter().map(|m| m.value).sum(),
                    currency: ms[0].currency.clone(),
                })
            }),
            total_source: Some("schema.org Order price".into()),
            items: item_names(v),
            status: status(v, "orderStatus"),
        }));
    }
    if is(v, "Invoice") {
        return Some(Extracted::Bill(Bill {
            biller: get(v, "provider")
                .or_else(|| get(v, "broker"))
                .and_then(as_text),
            invoice_number: text(v, "confirmationNumber")
                .or_else(|| text(v, "identifier"))
                .or_else(|| text(v, "accountId")),
            amount_due: money(
                v,
                &["totalPaymentDue", "minimumPaymentDue"],
                "priceCurrency",
            ),
            amount_source: Some("schema.org Invoice totalPaymentDue".into()),
            due_date: time(v, "paymentDueDate")
                .or_else(|| time(v, "paymentDue"))
                .map(|t| t[..10].to_string()),
            status: status(v, "paymentStatus"),
        }));
    }
    None
}

fn flight(v: &Value) -> Flight {
    let f = get(v, "reservationFor").unwrap_or(v);
    let airline = get(f, "airline");
    let code = airline
        .and_then(|a| text(a, "iataCode"))
        .map(|c| c.to_uppercase());
    let num = text(f, "flightNumber").map(|n| {
        let n = n.replace(' ', "").to_uppercase();
        // "238" with airline "TP" → "TP 238"; "TP238" → "TP 238".
        match &code {
            Some(c) if !n.starts_with(c.as_str()) => format!("{c} {n}"),
            Some(c) => format!("{c} {}", &n[c.len()..]),
            None => n,
        }
    });
    let airport = |k: &str| -> (Option<String>, Option<String>) {
        match get(f, k) {
            None => (None, None),
            Some(a) => {
                let code = text(a, "iataCode").map(|c| c.to_uppercase());
                let name = text(a, "name").or_else(|| {
                    code.as_deref()
                        .and_then(super::airport_by_code)
                        .map(|(_, city)| city.to_string())
                });
                let code = code.or_else(|| {
                    a.as_str()
                        .filter(|s| s.len() == 3)
                        .map(|s| s.to_uppercase())
                });
                (code, name)
            }
        }
    };
    let (depart_airport, depart_name) = airport("departureAirport");
    let (arrive_airport, arrive_name) = airport("arrivalAirport");
    Flight {
        airline: airline.and_then(as_text).or_else(|| {
            code.as_deref()
                .and_then(super::airline_by_code)
                .map(|(_, n)| n.to_string())
        }),
        airline_code: code,
        flight_number: num,
        confirmation: reservation_id(v),
        passenger: person_name(v, "underName"),
        depart_airport,
        depart_name,
        arrive_airport,
        arrive_name,
        depart_time: time(f, "departureTime"),
        arrive_time: time(f, "arrivalTime"),
        status: status(v, "reservationStatus"),
        total: money(v, &["totalPrice", "price"], "priceCurrency"),
    }
}

fn reservation(v: &Value, category: &str) -> Reservation {
    let what = get(v, "reservationFor");
    // The place: an event's location, a restaurant itself, a car's pickup.
    let venue = what
        .and_then(|w| get(w, "location"))
        .or_else(|| (category == "restaurant").then_some(what).flatten())
        .or_else(|| get(v, "pickupLocation"))
        .or_else(|| what.and_then(|w| get(w, "departureStation")))
        .or_else(|| what.and_then(|w| get(w, "departureBusStop")));
    Reservation {
        category: category.to_string(),
        name: what.and_then(as_text),
        start: what
            .and_then(|w| time(w, "startDate"))
            .or_else(|| time(v, "startTime"))
            .or_else(|| time(v, "pickupTime"))
            .or_else(|| what.and_then(|w| time(w, "departureTime"))),
        end: what
            .and_then(|w| time(w, "endDate"))
            .or_else(|| time(v, "endTime"))
            .or_else(|| time(v, "dropoffTime"))
            .or_else(|| what.and_then(|w| time(w, "arrivalTime"))),
        venue: venue.and_then(as_text),
        address: venue.and_then(address),
        confirmation: reservation_id(v),
        party_size: get(v, "partySize").and_then(number).map(|n| n as u32),
        status: status(v, "reservationStatus"),
        total: money(v, &["totalPrice", "price"], "priceCurrency"),
    }
}
