//! End-to-end Ask tests for extracted facts, quoted passages and meaning
//! search, on a fictional mailbox ("now" is Thu 2026-09-24 12:00 UTC).
//! Each fact question runs twice: before the background scanner has read
//! the mail (facts extracted on the fly from search hits) and after.

use chrono::{TimeZone, Utc};
use penguin_semantic::{ChunkRef, Embedder, FlatIndex, HashEmbedder, VectorIndex};

use super::*;
use crate::types::{Account, Message};
use crate::Store;

const ME: &str = "sam@northwind.example";

fn now() -> i64 {
    Utc.with_ymd_and_hms(2026, 9, 24, 12, 0, 0)
        .unwrap()
        .timestamp_millis()
}

fn days_ago(d: i64) -> i64 {
    now() - d * 86_400_000
}

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: Some(name.into()),
        email: email.into(),
    }
}

struct Mail {
    thread: &'static str,
    date: i64,
    from: Address,
    subject: &'static str,
    body: &'static str,
    html: Option<&'static str>,
    bulk: bool,
}

fn message(n: usize, m: &Mail) -> Message {
    let sent = m.from.email == ME;
    let mut labels = vec!["INBOX".to_string()];
    if sent {
        labels = vec!["SENT".into()];
    }
    if m.bulk {
        labels.push("CATEGORY_UPDATES".into());
    }
    Message {
        account_id: ME.into(),
        id: format!("m{n}"),
        thread_id: m.thread.into(),
        date: m.date,
        from: m.from.clone(),
        to: vec![if sent {
            addr("Priya Natarajan", "priya@linden.example")
        } else {
            addr("Sam Okafor", ME)
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: m.subject.into(),
        snippet: m.body.chars().take(120).collect(),
        body_text: m.body.into(),
        body_html: m.html.map(String::from),
        label_ids: labels,
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

const FLIGHT_HTML: &str = r#"<script type="application/ld+json">[
{"@context":"http://schema.org","@type":"FlightReservation","reservationNumber":"RXJ34P","reservationStatus":"http://schema.org/ReservationConfirmed",
 "underName":{"@type":"Person","name":"Sam Okafor"},
 "reservationFor":{"@type":"Flight","flightNumber":"238","airline":{"@type":"Airline","name":"Aurora Air","iataCode":"AU"},
  "departureAirport":{"@type":"Airport","name":"San Francisco","iataCode":"SFO"},"departureTime":"2026-10-02T19:05:00-07:00",
  "arrivalAirport":{"@type":"Airport","name":"Lisbon","iataCode":"LIS"},"arrivalTime":"2026-10-03T15:10:00+01:00"},
 "totalPrice":"1184.20","priceCurrency":"USD"},
{"@context":"http://schema.org","@type":"FlightReservation","reservationNumber":"RXJ34P",
 "reservationFor":{"@type":"Flight","flightNumber":"239","airline":{"@type":"Airline","name":"Aurora Air","iataCode":"AU"},
  "departureAirport":{"@type":"Airport","name":"Lisbon","iataCode":"LIS"},"departureTime":"2026-10-09T11:40:00+01:00",
  "arrivalAirport":{"@type":"Airport","name":"San Francisco","iataCode":"SFO"},"arrivalTime":"2026-10-09T15:05:00-07:00"}}]</script><p>Your trip</p>"#;

const HOTEL_HTML: &str = r#"<script type="application/ld+json">{"@context":"http://schema.org","@type":"LodgingReservation",
 "reservationNumber":"48291","reservationStatus":"http://schema.org/ReservationConfirmed",
 "reservationFor":{"@type":"LodgingBusiness","name":"The Alder Lisbon","address":{"@type":"PostalAddress","streetAddress":"Rua das Flores 12","addressLocality":"Lisboa","addressCountry":"PT"}},
 "checkinDate":"2026-10-03T15:00:00+01:00","checkoutDate":"2026-10-09T11:00:00+01:00",
 "totalPrice":{"@type":"PriceSpecification","price":"912.00","priceCurrency":"EUR"}}</script>"#;

fn mail() -> Vec<Mail> {
    let aurora = addr("Aurora Air", "bookings@auroraair.example");
    let paperleaf = addr("Paperleaf", "orders@paperleaf.example");
    let bright = addr("Brightwave Internet", "billing@brightwave.example");
    let ledgerly = addr("Ledgerly", "billing@ledgerly.example");
    let priya = addr("Priya Natarajan", "priya@linden.example");
    let grace = addr("Grace Kim", "grace@northwind.example");
    let me = addr("Sam Okafor", ME);
    vec![
        Mail { thread: "trip", date: days_ago(12), from: aurora.clone(), subject: "Your flight confirmation: Lisbon", body: "Thanks for booking your trip to Lisbon.", html: Some(FLIGHT_HTML), bulk: false },
        Mail { thread: "trip-old", date: days_ago(300), from: aurora, subject: "Your flight confirmation: Boston", body: "Confirmation code: QW8K2L\n\nFlight AU 417\nThu, Dec 4, 2025\nSFO → BOS\nDeparts 8:15 AM   Arrives 4:52 PM", html: None, bulk: false },
        Mail { thread: "hotel", date: days_ago(10), from: addr("Alder Hotels", "stay@alderhotels.example"), subject: "Reservation confirmed", body: "We look forward to your stay.", html: Some(HOTEL_HTML), bulk: false },
        Mail { thread: "order", date: days_ago(4), from: paperleaf.clone(), subject: "Order confirmation", body: "Thanks for your order!\nOrder #PL-112-7719\nDot grid notebook $24.00\nBrass pen $32.00\nOrder Total: $58.20", html: None, bulk: false },
        Mail { thread: "order", date: days_ago(2), from: paperleaf.clone(), subject: "Your Paperleaf order is on its way", body: "Good news! Your order PL-112-7719 has shipped.\nCarrier: UPS\nTracking number: 1Z879E930346834440\nArriving Saturday, Sep 26\nOrder Total: $58.20", html: None, bulk: false },
        Mail { thread: "order2", date: days_ago(60), from: paperleaf, subject: "Order confirmation", body: "Order #PL-100-2001\nOrder Total: $24.00", html: None, bulk: false },
        Mail { thread: "bill", date: days_ago(5), from: bright, subject: "Your Brightwave statement is ready", body: "New balance: $64.12\nPayment due by 10/08/2026", html: None, bulk: false },
        Mail { thread: "inv", date: days_ago(20), from: ledgerly.clone(), subject: "Invoice INV-2041 from Ledgerly", body: "Invoice number: INV-2041\nAmount due: $89.00\nDue date: October 1, 2026", html: None, bulk: false },
        Mail { thread: "inv", date: days_ago(3), from: ledgerly, subject: "Payment received — thank you!", body: "We received your payment of $89.00 for invoice INV-2041.", html: None, bulk: false },
        Mail { thread: "table", date: days_ago(1), from: addr("Fern & Fig", "hello@fernandfig.example"), subject: "Reservation confirmed at Fern & Fig", body: "Hi Sam, your table for 4 is booked for Saturday, October 3 at 7:30 PM.\nReservation number: FF-5521", html: None, bulk: false },
        Mail { thread: "code", date: now() - 5 * 60_000, from: addr("Rydeo", "noreply@rydeo.example"), subject: "0357 is your Rydeo code", body: "A one-time Rydeo code has been created for you.", html: None, bulk: false },
        Mail { thread: "pricing", date: days_ago(6), from: priya.clone(), subject: "Re: pricing page", body: "Thanks Sam. Let's keep the annual plan at $12 a seat and drop the setup fee for teams under ten.\n\nPriya Natarajan\nLinden Partners\nm: +1 (415) 555-0142\n418 Alder St, Suite 200\nPortland, OR 97205", html: None, bulk: false },
        Mail { thread: "contract", date: days_ago(8), from: me.clone(), subject: "Contract redlines", body: "Hi Priya, attached are our redlines on the services contract. Can you review by Friday?", html: None, bulk: false },
        Mail { thread: "offsite", date: days_ago(7), from: grace, subject: "Offsite logistics", body: "Hi all, a few logistics for next week. We start at 9:30 in the Harbor Room. The wifi password at the venue is harbor2026, all lowercase. Lunch is provided.", html: None, bulk: false },
    ]
}

fn mailbox() -> Store {
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_account(&Account {
            id: ME.into(),
            email: ME.into(),
            display_name: Some("Sam".into()),
            nickname: None,
            color: "#336699".into(),
            added_at: 0,
            ..Account::default()
        })
        .unwrap();
    let msgs: Vec<Message> = mail()
        .iter()
        .enumerate()
        .map(|(i, m)| message(i + 1, m))
        .collect();
    store.upsert_messages(&msgs).unwrap();
    store
}

fn scanned() -> Store {
    let s = mailbox();
    // The app's startup pass for verification codes in recent mail.
    s.backfill_otp(days_ago(30)).unwrap();
    let p = s.extract_pending(10_000).unwrap();
    assert_eq!(p.remaining, 0);
    s
}

fn ask(s: &Store, q: &str) -> AskAnswer {
    s.ask(q, &AskScope::default(), now(), 0).unwrap()
}

/// Same headline before and after the scanner ran.
fn both(q: &str) -> AskAnswer {
    let before = ask(&mailbox(), q);
    let after = ask(&scanned(), q);
    assert_eq!(before.headline, after.headline, "{q}");
    assert!(after.coverage.is_none());
    after
}

#[test]
fn flight_to_lisbon() {
    let a = both("When is my flight to Lisbon?");
    assert_eq!(a.intent, AskIntent::Flight);
    assert_eq!(
        a.headline,
        "AU 238 to Lisbon (LIS): Fri, Oct 2 at 7:05 PM from SFO (in 8 days)"
    );
    assert_eq!(a.confidence, AskConfidence::High);
    // The return leg of the same booking is shown too.
    assert_eq!(a.cards.len(), 2);
    assert!(
        matches!(&a.cards[1].fact, crate::structured::Extracted::Flight(f) if f.flight_number.as_deref() == Some("AU 239"))
    );
    assert_eq!(a.cards[0].source, crate::structured::Source::JsonLd);
    let a = both("what's my confirmation code for the flight to Lisbon");
    assert!(
        a.headline.starts_with("Confirmation code RXJ34P"),
        "{}",
        a.headline
    );
    let a = both("when did I fly to Boston");
    assert!(
        a.headline
            .starts_with("AU 417 to Boston (BOS): Thu, Dec 4, 2025"),
        "{}",
        a.headline
    );
    let a = ask(&scanned(), "¿Cuándo es mi vuelo a Lisboa?");
    assert!(a.headline.starts_with("AU 238 to Lisbon"), "{}", a.headline);
    let a = ask(&scanned(), "when is my flight to Tokyo");
    assert_eq!(a.confidence, AskConfidence::None);
}

#[test]
fn hotel_stay() {
    let a = both("where am I staying in Lisbon?");
    assert_eq!(
        a.headline,
        "The Alder Lisbon: check in Sat, Oct 3 at 3 PM, check out Fri, Oct 9 at 11 AM (in 9 days)"
    );
    assert_eq!(
        a.detail.as_deref(),
        Some("Rua das Flores 12, Lisboa, PT · confirmation 48291")
    );
}

#[test]
fn package_and_orders() {
    let a = both("where's my Paperleaf order?");
    assert_eq!(a.intent, AskIntent::Package);
    assert_eq!(
        a.headline,
        "Your Paperleaf order is on its way with UPS, arriving Sat, Sep 26"
    );
    assert_eq!(a.detail.as_deref(), Some("Tracking 1Z879E930346834440"));
    let crate::structured::Extracted::Shipment(s) = &a.cards[0].fact else {
        panic!()
    };
    assert_eq!(
        s.tracking_url.as_deref(),
        Some("https://www.ups.com/track?tracknum=1Z879E930346834440")
    );
    let a = both("what did I order from Paperleaf");
    assert!(
        a.headline
            .starts_with("Latest Paperleaf order PL-112-7719, $58.20"),
        "{}",
        a.headline
    );
    // Confirmation + shipping notice of one order: one card, the other related.
    assert_eq!(a.cards.len(), 2);
    assert_eq!(a.cards[0].related.len(), 1);
}

#[test]
fn spend_counts_an_order_once_and_shows_the_math() {
    let a = ask(&scanned(), "how much did I spend at Paperleaf this year");
    assert_eq!(a.headline, "$82.20 across 2 Paperleaf receipts this year");
    let sum = a.sum.unwrap();
    assert_eq!(sum.duplicates, 1);
    assert_eq!(sum.totals[0].count, 2);
    assert_eq!(a.items.len(), 2);
    // Before the scanner: the same math.
    let b = ask(&mailbox(), "how much did I spend at Paperleaf this year");
    assert_eq!(b.headline, a.headline);
    // Spanish.
    let c = ask(&scanned(), "¿Cuánto gasté en Paperleaf este año?");
    assert_eq!(c.headline, a.headline);
}

#[test]
fn bills_due_and_paid() {
    let a = both("what bills are due?");
    assert_eq!(a.intent, AskIntent::Bills);
    // Ledgerly's invoice was paid later: only Brightwave is due.
    assert_eq!(a.cards.len(), 1);
    assert!(
        a.headline
            .starts_with("Brightwave Internet: $64.12 due Thu, Oct 8"),
        "{}",
        a.headline
    );
    let a = both("when is my Ledgerly invoice due");
    assert!(a.headline.starts_with("Paid: Ledgerly"), "{}", a.headline);
}

#[test]
fn bookings_and_when() {
    let a = both("when is my dinner reservation?");
    assert_eq!(
        a.headline,
        "Fern & Fig: Sat, Oct 3 at 7:30 PM (in 9 days), 4 people"
    );
    // "When is …" questions check bookings first.
    let a = ask(&scanned(), "when is the Fern & Fig dinner");
    assert_eq!(a.intent, AskIntent::Booking);
}

#[test]
fn codes_only_when_allowed() {
    let s = scanned();
    let a = ask(&s, "latest verification code from Rydeo");
    assert_eq!(a.intent, AskIntent::Code);
    assert!(!a.headline.contains("0357"), "{}", a.headline);
    let scope = AskScope {
        reveal_codes: true,
        ..AskScope::default()
    };
    let a = s
        .ask("latest verification code from Rydeo", &scope, now(), 0)
        .unwrap();
    assert_eq!(a.headline, "0357 from Rydeo (5 min ago)");
}

#[test]
fn phone_numbers_from_signatures() {
    let a = both("what's Priya's phone number?");
    assert_eq!(
        a.headline,
        "Priya Natarajan's phone number: +1 (415) 555-0142"
    );
    let a = ask(&scanned(), "who is Priya");
    let phone = a.facts.iter().find(|f| f.label == "Phone").unwrap();
    assert!(phone.value.starts_with("+1 (415) 555-0142"));
    assert!(a
        .facts
        .iter()
        .any(|f| f.label == "Address" && f.value.contains("418 Alder St")));
}

#[test]
fn quoted_passages_answer_topic_questions() {
    let s = scanned();
    let a = ask(&s, "what is the wifi password for the offsite?");
    assert_eq!(a.intent, AskIntent::Passage);
    assert_eq!(
        a.headline,
        "\u{201c}The wifi password at the venue is harbor2026, all lowercase.\u{201d}"
    );
    assert_eq!(a.passages[0].from.email, "grace@northwind.example");
    // Marks point at the question's words, in JS offsets.
    let p = &a.passages[0];
    let marked: Vec<String> = p
        .marks
        .iter()
        .map(|[x, y]| {
            String::from_utf16(&p.text.encode_utf16().collect::<Vec<_>>()[*x as usize..*y as usize])
                .unwrap()
        })
        .collect();
    assert!(
        marked.contains(&"wifi".to_string()) && marked.contains(&"password".to_string()),
        "{marked:?}"
    );

    let a = ask(&s, "what did Priya say about the setup fee?");
    assert_eq!(a.intent, AskIntent::Said);
    assert!(a.headline.contains("drop the setup fee"), "{}", a.headline);
    assert!(a.person.is_some());

    // Nothing answers: say so, don't guess.
    let a = ask(&s, "what is the parking situation in Denver");
    assert_eq!(a.confidence, AskConfidence::None);
    assert!(a.passages.is_empty());
}

#[test]
fn did_they_reply_about_it() {
    let s = scanned();
    let a = ask(&s, "did Priya reply about the contract?");
    assert_eq!(a.intent, AskIntent::DidReply);
    assert!(
        a.headline
            .starts_with("Not yet: you wrote last on Sep 16, 2026"),
        "{}",
        a.headline
    );
}

#[test]
fn counts_of_orders_and_flights() {
    let s = scanned();
    // Counts go to the query layer, which lists every order it counted.
    let a = ask(&s, "how many orders did I place with Paperleaf this year");
    assert_eq!(a.headline, "Orders from Paperleaf in 2026: 2");
    assert_eq!(a.cards.len(), 2);
    let a = ask(&s, "how many flights did I take in 2025");
    assert_eq!(a.headline, "Flights in 2025: 1");
}

/// Meaning search plugs in through the Embedder/VectorIndex traits. The
/// stand-in embedder only knows words, so this checks the plumbing:
/// retrieval by vectors, rank fusion and sentence scoring by meaning.
#[test]
fn questions_read_as_queries() {
    let s = scanned();
    let count = |q: &str| {
        let a = ask(&s, q);
        assert_eq!(a.intent, AskIntent::Query, "{q}: {}", a.headline);
        a.result.as_ref().and_then(|r| r.count).unwrap_or(u64::MAX)
    };
    // "In December" asked in September is last December: the Boston flight.
    assert_eq!(count("how many times did I fly in december"), 1);
    assert_eq!(count("how many flights did I take in 2025"), 1);
    // A past question about October means October 2025; a future one the
    // coming October, with both legs of the Lisbon trip.
    assert_eq!(count("how many times did I fly in october"), 0);
    assert_eq!(count("how many flights do I have in october"), 2);
    let a = ask(&s, "how many flights do I have in october");
    assert!(a.headline.starts_with("Flights in October 2026: 2"), "{}", a.headline);
    assert_eq!(a.cards.len(), 2);
    let u = a.understood.as_ref().unwrap();
    assert_eq!(u.query.subject, QuerySubject::Flights);
    assert_eq!(u.source, QuerySource::Grammar);

    // Money in September: the Paperleaf order once (confirmation and
    // shipping notice), the flight and hotel booking totals, the paid
    // Ledgerly invoice; not the Brightwave bill that is only due.
    let a = ask(&s, "how much did I spend in september");
    assert_eq!(a.intent, AskIntent::Query, "{}", a.headline);
    let totals: Vec<(String, f64)> = a
        .result
        .as_ref()
        .unwrap()
        .totals
        .iter()
        .map(|t| (t.currency.clone(), (t.value * 100.0).round() / 100.0))
        .collect();
    assert!(totals.contains(&("USD".into(), 1331.40)), "{totals:?}");
    assert!(totals.contains(&("EUR".into(), 912.0)), "{totals:?}");

    let a = ask(&s, "did I spend more in july or september");
    assert_eq!(
        a.result.as_ref().unwrap().winner.as_deref(),
        Some("September 2026"),
        "{}",
        a.headline
    );
    assert_eq!(a.groups.len(), 2);
    let a = ask(&s, "which month did I spend the most");
    assert_eq!(
        a.result.as_ref().unwrap().winner.as_deref(),
        Some("September 2026"),
        "{}",
        a.headline
    );
    let a = ask(&s, "did I ever fly to Tokyo");
    assert_eq!(a.result.as_ref().unwrap().yes, Some(false));
    let a = ask(&s, "¿Cuántas veces volé en 2025?");
    assert_eq!(a.result.as_ref().and_then(|r| r.count), Some(1), "{}", a.headline);

    // A reading edited in the chips runs the same way.
    let mut q = a.understood.clone().unwrap().query;
    q.timeframe = Some("2026".into());
    q.tense = QueryTense::Any;
    let e = s
        .ask_query("edited", &q, QuerySource::Edited, &AskScope::default(), now(), 0)
        .unwrap();
    assert_eq!(e.result.as_ref().and_then(|r| r.count), Some(2), "{}", e.headline);
    assert_eq!(e.understood.as_ref().unwrap().source, QuerySource::Edited);

    // The model's reading is checked before it runs.
    let draft = QueryDraft {
        subject: "flights".into(),
        op: "count".into(),
        timeframe: "sometime soonish".into(),
        ..Default::default()
    };
    assert!(s
        .ask_draft("q", draft, &AskScope::default(), now(), 0)
        .unwrap()
        .is_none());
    let draft = QueryDraft {
        subject: "flights".into(),
        op: "count".into(),
        place: "Lisbon".into(),
        direction: "to".into(),
        timeframe: "october".into(),
        tense: "future".into(),
        ..Default::default()
    };
    let m = s
        .ask_draft(
            "how often am I heading to Lisbon in october",
            draft,
            &AskScope::default(),
            now(),
            0,
        )
        .unwrap()
        .expect("a valid reading");
    assert_eq!(m.result.as_ref().and_then(|r| r.count), Some(1), "{}", m.headline);
    assert_eq!(m.understood.as_ref().unwrap().source, QuerySource::Model);
}

#[test]
fn meaning_search_through_the_semantic_traits() {
    let s = scanned();
    let e = HashEmbedder::new(512);
    let idx = FlatIndex::default();
    for (i, m) in mail().iter().enumerate() {
        let v = e
            .embed_passages(&[&format!("{}. {}", m.subject, m.body)])
            .unwrap()
            .remove(0);
        idx.upsert(
            ChunkRef {
                account_id: ME.into(),
                thread_id: m.thread.into(),
                message_id: format!("m{}", i + 1),
                chunk: 0,
                date: m.date,
            },
            v,
        )
        .unwrap();
    }
    let sem = AskSemantic {
        embedder: &e,
        index: &idx,
    };
    let a = s
        .ask_with(
            "what is the wifi password for the offsite?",
            &AskScope::default(),
            now(),
            0,
            Some(sem),
        )
        .unwrap();
    assert!(a.headline.contains("harbor2026"), "{}", a.headline);
    assert!(
        a.steps
            .iter()
            .any(|x| x.starts_with("Searched by meaning (hash-bow")),
        "{:?}",
        a.steps
    );
    assert!(
        a.steps
            .iter()
            .any(|x| x.starts_with("Compared the meaning of")),
        "{:?}",
        a.steps
    );
    assert!(idx.len() == mail().len());
}

#[test]
fn subscriptions_are_recurring_charges() {
    let s = mailbox();
    let tune = addr("Tunewave", "billing@tunewave.example");
    let msgs: Vec<Message> = (0..6)
        .map(|k| {
            let m = Mail {
                thread: Box::leak(format!("tune-{k}").into_boxed_str()),
                date: days_ago(3 + 30 * k),
                from: tune.clone(),
                subject: "Your Tunewave receipt",
                body: "Thanks for being a member.\n\nPlan: Standard\nAmount charged: $10.99\nBilling period: monthly",
                html: None,
                bulk: true,
            };
            message(100 + k as usize, &m)
        })
        .collect();
    s.upsert_messages(&msgs).unwrap();
    s.extract_pending(10_000).unwrap();
    let a = ask(&s, "what subscriptions do I pay for?");
    assert_eq!(a.intent, AskIntent::Subscriptions);
    assert_eq!(a.headline, "1 subscription, $10.99 a month: Tunewave");
    assert_eq!(a.groups.len(), 1);
    assert_eq!(a.groups[0].count, 6);
    assert_eq!(a.result.as_ref().unwrap().totals[0].value, 10.99);
    // In a date range: every charge in it, added up.
    let a = ask(&s, "how much did I spend on subscriptions in the last 3 months");
    assert_eq!(a.intent, AskIntent::Subscriptions);
    assert!(a.headline.starts_with("$32.97 on 1 subscription"), "{}", a.headline);
    assert_eq!(a.items.len(), 3);
}
