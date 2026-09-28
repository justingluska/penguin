//! Search operators backed by derived data or thread structure: first
//! contacts, replies, me, size, thread length, weekday, grouping.

use super::*;

fn ids(s: &Store, q: &str) -> Vec<String> {
    let mut v = hit_ids(&search(s, q));
    v.sort();
    v
}

fn sorted(v: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = v.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

fn received(id: &str, thread: &str, date: i64, from: &str) -> Message {
    M::new(A, id, thread, date).from("", from).done()
}

fn sent(id: &str, thread: &str, date: i64, to: &[&str]) -> Message {
    let to: Vec<(&str, &str)> = to.iter().map(|e| ("", *e)).collect();
    M::new(A, id, thread, date)
        .from("Me", A)
        .to(&to)
        .labels(&["SENT"])
        .done()
}

#[test]
fn new_sender_is_the_first_message_from_an_address() {
    let s = store_with_accounts(&[A]);
    let t = now();
    // Inserted newest first: the stored minimum must still win.
    s.upsert_messages(&[
        received("ana2", "t2", t - 5 * DAY, "ana@ruiz.example"),
        received("bob1", "t3", t - 3 * DAY, "bob@acme.example"),
    ])
    .unwrap();
    s.upsert_messages(&[received("ana1", "t1", t - 10 * DAY, "Ana@Ruiz.example")])
        .unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["ana1", "bob1"]));
    assert_eq!(ids(&s, "from:new"), sorted(&["ana1", "bob1"]));
    // "First contact happened in the range."
    assert_eq!(ids(&s, "is:new-sender newer_than:4d"), sorted(&["bob1"]));
    assert_eq!(ids(&s, "-is:new-sender"), sorted(&["ana2"]));

    // Deleting the first message promotes the next one.
    s.delete_messages(A, &["ana1".into()]).unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["ana2", "bob1"]));

    // Spam doesn't count as a first contact; leaving Spam counts again.
    s.upsert_messages(&[received("ana0", "t0", t - 20 * DAY, "ana@ruiz.example")])
        .unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["ana0", "bob1"]));
    s.modify_message_labels(A, &["ana0".into()], &["SPAM".into()], &["INBOX".into()])
        .unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["ana2", "bob1"]));
    s.modify_message_labels(A, &["ana0".into()], &["INBOX".into()], &["SPAM".into()])
        .unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["ana0", "bob1"]));

    // Re-storing a message (body fetched later) keeps it the first contact.
    let again = s.get_message(A, "ana0").unwrap().unwrap();
    s.upsert_messages(&[again]).unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["ana0", "bob1"]));
}

#[test]
fn first_outbound_is_the_first_message_sent_to_an_address() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        sent("s2", "u2", t - 2 * DAY, &["x@one.example"]),
        sent("s1", "u1", t - 9 * DAY, &["x@one.example", "y@two.example"]),
        sent("s3", "u3", t - DAY, &["z@three.example", "x@one.example"]),
        received("r1", "u1", t - 8 * DAY, "x@one.example"),
    ])
    .unwrap();
    assert_eq!(ids(&s, "is:first-outbound"), sorted(&["s1", "s3"]));
    assert_eq!(ids(&s, "to:new"), sorted(&["s1", "s3"]));
    assert_eq!(ids(&s, "to:new newer_than:3d"), sorted(&["s3"]));
    // The reply from x is a new sender (the first mail *from* x).
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["r1"]));

    s.delete_messages(A, &["s1".into()]).unwrap();
    // x's first is now s2; y has no sent mail left.
    assert_eq!(ids(&s, "is:first-outbound"), sorted(&["s2", "s3"]));

    // Drafts aren't sent mail.
    let draft = M::new(A, "d1", "u9", t - 30 * DAY)
        .from("Me", A)
        .to(&[("", "w@four.example")])
        .labels(&["DRAFT"])
        .done();
    s.upsert_messages(&[draft]).unwrap();
    assert_eq!(ids(&s, "is:first-outbound"), sorted(&["s2", "s3"]));
}

#[test]
fn known_sender_is_mail_from_someone_you_have_written_to() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        received("r1", "T1", t - 10 * DAY, "Ana@Ruiz.example"),
        sent("s1", "T1", t - 9 * DAY, &["ana@ruiz.example"]),
        received("r2", "T2", t - 8 * DAY, "bob@acme.example"),
        // Written to later than they wrote: still someone you know.
        received("r3", "T3", t - 7 * DAY, "cy@acme.example"),
        sent("s3", "T4", t - 2 * DAY, &["cy@acme.example"]),
    ])
    .unwrap();
    assert_eq!(ids(&s, "is:known-sender"), sorted(&["r1", "r3"]));
    assert_eq!(ids(&s, "from:known"), sorted(&["r1", "r3"]));
    // Your own mail is never "from someone you know".
    assert_eq!(ids(&s, "-is:known-sender"), sorted(&["r2", "s1", "s3"]));
    assert_eq!(ids(&s, "is:known from:acme"), sorted(&["r3"]));
    // Drafts don't count as writing to someone.
    let draft = M::new(A, "d1", "T2", t - DAY)
        .from("Me", A)
        .to(&[("", "bob@acme.example")])
        .labels(&["DRAFT"])
        .done();
    s.upsert_messages(&[draft]).unwrap();
    assert_eq!(ids(&s, "is:known-sender"), sorted(&["r1", "r3"]));
}

#[test]
fn reply_states() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        // Answered.
        received("r1", "T1", t - 10 * DAY, "ana@ruiz.example"),
        sent("s1", "T1", t - 9 * DAY, &["ana@ruiz.example"]),
        // Never answered.
        received("r2", "T2", t - 8 * DAY, "bob@acme.example"),
        // You wrote last; no answer yet.
        sent("s3", "T3", t - 7 * DAY, &["cy@acme.example"]),
        // You wrote, they answered, you didn't.
        sent("s4", "T4", t - 6 * DAY, &["di@acme.example"]),
        received("r4", "T4", t - 5 * DAY, "di@acme.example"),
        // Bulk mail is never "unanswered".
        M::new(A, "n5", "T5", t - 4 * DAY)
            .from("", "news@list.example")
            .unsubscribe()
            .done(),
    ])
    .unwrap();
    assert_eq!(ids(&s, "is:unanswered"), sorted(&["r2", "r4"]));
    assert_eq!(ids(&s, "is:replied"), sorted(&["r1"]));
    // s1 answered r1 and nothing came back; s4 got an answer.
    assert_eq!(ids(&s, "is:awaiting"), sorted(&["s1", "s3"]));
    assert_eq!(ids(&s, "is:reply"), sorted(&["s1", "r4"]));
    assert_eq!(ids(&s, "is:newsletter"), sorted(&["n5"]));
    assert_eq!(ids(&s, "has:unsubscribe"), sorted(&["n5"]));
    assert_eq!(ids(&s, "is:unanswered from:bob"), sorted(&["r2"]));
    // A draft reply doesn't answer anything, nor end the wait.
    let draft = M::new(A, "d2", "T2", t - DAY)
        .from("Me", A)
        .labels(&["DRAFT"])
        .done();
    s.upsert_messages(&[draft]).unwrap();
    assert_eq!(ids(&s, "is:unanswered"), sorted(&["r2", "r4"]));
}

#[test]
fn me_domain_and_participants() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        received("r1", "T1", t - 3 * DAY, "ana@ruiz.example"),
        sent("s1", "T2", t - 2 * DAY, &["bob@acme.example"]),
        M::new(A, "r2", "T3", t - DAY)
            .from("", "cy@acme.example")
            .to(&[("", "team@acme.example")])
            .done(),
    ])
    .unwrap();
    assert_eq!(ids(&s, "from:me"), sorted(&["s1"]));
    // r1 is addressed to the account (M::new's default To).
    assert_eq!(ids(&s, "to:me"), sorted(&["r1"]));
    assert_eq!(ids(&s, "-to:me"), sorted(&["r2", "s1"]));
    assert_eq!(ids(&s, "domain:acme.example"), sorted(&["r2", "s1"]));
    assert_eq!(ids(&s, "domain:@acme.example from:me"), sorted(&["s1"]));
    assert_eq!(ids(&s, "with:bob"), sorted(&["s1"]));
    assert_eq!(ids(&s, "participant:ana"), sorted(&["r1"]));
    // Inside an OR next to a SQL-only term.
    assert_eq!(ids(&s, "to:me OR is:first-outbound"), sorted(&["r1", "s1"]));
}

#[test]
fn to_me_follows_accounts_being_added_and_removed() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "x", "T1", t - DAY)
            .to(&[("", "Me@Home.example")])
            .done(),
        M::new(A, "y", "T2", t - DAY)
            .to(&[("", "other@x.example")])
            .done(),
    ])
    .unwrap();
    assert!(ids(&s, "to:me").is_empty());
    s.upsert_account(&account(B)).unwrap();
    assert_eq!(ids(&s, "to:me"), sorted(&["x"]));
    // Re-saving an account doesn't disturb it; removing it does.
    s.upsert_account(&account(B)).unwrap();
    assert_eq!(ids(&s, "to:me"), sorted(&["x"]));
    s.remove_account(B).unwrap();
    assert!(ids(&s, "to:me").is_empty());
    // Relabelling keeps the bit.
    s.upsert_account(&account(B)).unwrap();
    s.modify_message_labels(A, &["x".into()], &["STARRED".into()], &[])
        .unwrap();
    assert_eq!(ids(&s, "to:me"), sorted(&["x"]));
}

#[test]
fn to_me_includes_plus_addressed_copies() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "tagged", "T1", t - DAY)
            .to(&[("", "Me+Shop@Work.example")])
            .done(),
        M::new(A, "other", "T2", t - DAY)
            .to(&[("", "me+x@elsewhere.example")])
            .done(),
        M::new(A, "home", "T3", t - DAY)
            .to(&[("", "me+news@home.example")])
            .done(),
    ])
    .unwrap();
    assert_eq!(ids(&s, "to:me"), sorted(&["tagged"]));
    // Adding and removing the account the tag belongs to.
    s.upsert_account(&account(B)).unwrap();
    assert_eq!(ids(&s, "to:me"), sorted(&["home", "tagged"]));
    s.remove_account(B).unwrap();
    assert_eq!(ids(&s, "to:me"), sorted(&["tagged"]));
}

#[test]
fn size_thread_length_and_weekday() {
    let s = store_with_accounts(&[A]);
    // Wed 2026-09-23 12:00 UTC and Sat 2026-09-19 12:00 UTC (noon: the same
    // weekday in every timezone from UTC-11 to UTC+11).
    let wed = 1_790_164_800_000;
    let sat = wed - 4 * DAY;
    s.upsert_messages(&[
        M::new(A, "one", "T1", wed)
            .attach("a.pdf", "application/pdf")
            .done(),
        M::new(A, "two", "T2", sat)
            .attach("a.pdf", "application/pdf")
            .attach("b.pdf", "application/pdf")
            .done(),
        M::new(A, "none", "T2", sat + 1000).done(),
    ])
    .unwrap();
    assert_eq!(ids(&s, "larger:1500"), sorted(&["two"]));
    assert_eq!(ids(&s, "size:>1K"), sorted(&["two"]));
    assert_eq!(ids(&s, "smaller:1500"), sorted(&["none", "one"]));
    assert_eq!(ids(&s, "larger:1M"), Vec::<String>::new());
    // Thread length: T2 has 2 messages (grouped: best one per thread).
    assert_eq!(search(&s, "messages:>1").hits.len(), 1);
    assert_eq!(ids(&s, "messages:1"), sorted(&["one"]));
    assert_eq!(search(&s, "messages:1..2").hits.len(), 2);
    assert_eq!(ids(&s, "day:wednesday"), sorted(&["one"]));
    assert_eq!(search(&s, "day:weekend").hits.len(), 1);
    assert_eq!(ids(&s, "day:mon,wed"), sorted(&["one"]));
    assert_eq!(ids(&s, "-day:weekend"), sorted(&["one"]));
}

#[test]
fn grouping_and_snooze() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "a", "T1", t - 3 * DAY)
            .from("", "ana@ruiz.example")
            .subject("lease")
            .done(),
        M::new(A, "b", "T2", t - 2 * DAY)
            .from("", "bob@acme.example")
            .subject("lease")
            .attach("x.pdf", "application/pdf")
            .done(),
        M::new(A, "c", "T3", t - DAY)
            .from("", "cy@acme.example")
            .subject("invoice")
            .done(),
    ])
    .unwrap();
    assert_eq!(ids(&s, "(from:ana OR from:bob) lease"), sorted(&["a", "b"]));
    assert_eq!(ids(&s, "(from:ana lease) OR has:pdf"), sorted(&["a", "b"]));
    assert_eq!(ids(&s, "-(from:ana OR from:bob)"), sorted(&["c"]));
    assert_eq!(ids(&s, "-(lease has:pdf)"), sorted(&["a", "c"]));
    assert_eq!(
        ids(&s, "(subject:lease OR subject:invoice) -(from:bob)"),
        sorted(&["a", "c"])
    );

    s.snooze_threads(&[(A.to_string(), "T3".to_string())], t + DAY, t)
        .unwrap();
    assert_eq!(ids(&s, "is:snoozed"), sorted(&["c"]));
    assert_eq!(ids(&s, "in:snoozed"), sorted(&["c"]));
}

#[test]
fn braces_not_near_wildcards_and_operator_groups() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "a", "T1", t - 3 * DAY)
            .from("", "ana@ruiz.example")
            .subject("lease renewal")
            .body("Monthly rent goes to 2450 starting June 1.")
            .done(),
        M::new(A, "b", "T2", t - 2 * DAY)
            .from("", "bob@acme.example")
            .subject("invoice 7")
            .body("Rent reminder. Payment details follow below, as agreed with the landlord last year, and the rest arrives in June.")
            .done(),
        M::new(A, "c", "T3", t - DAY)
            .from("", "cy@acme.example")
            .subject("invoices")
            .done(),
    ])
    .unwrap();
    assert_eq!(ids(&s, "{from:ana from:bob} "), sorted(&["a", "b"]));
    assert_eq!(ids(&s, "-{from:ana from:bob} "), sorted(&["c"]));
    assert_eq!(ids(&s, "invoic* NOT from:bob "), sorted(&["c"]));
    assert_eq!(ids(&s, "subject:(lease renewal) "), sorted(&["a"]));
    assert_eq!(ids(&s, "from:(ana OR cy) "), sorted(&["a", "c"]));
    // Proximity: "rent" and "june" within 5 words only in a.
    assert_eq!(ids(&s, "rent AROUND 5 june "), sorted(&["a"]));
    assert_eq!(ids(&s, "rent AROUND 20 june "), sorted(&["a", "b"]));
    assert_eq!(ids(&s, "\u{201c}lease renewal\u{201d} "), sorted(&["a"]));
    assert_eq!(ids(&s, "\u{201c}renewal lease\u{201d} "), Vec::<String>::new());
}

#[test]
fn words_match_the_forms_the_mail_uses() {
    let s = store_with_accounts(&[A]);
    let t = now();
    let m = |id: &str, subject: &str, body: &str| {
        M::new(A, id, id, t - DAY).subject(subject).body(body).done()
    };
    s.upsert_messages(&[
        m("inv", "Crestline invoice CR-4410", "Monthly platform support."),
        m("uk", "Colour proofs", "The grey is a bit dark; shall we organise a call? Your order was cancelled."),
        m("hyph", "Handover", "Please hand off on-call before you leave. Wi-fi is down."),
        m("rent", "Lease", "Monthly rent goes to $2,450 starting June 1."),
        m("eur", "Reserva", "Importe total: 1.234,56 € por dos noches."),
        m("nordic", "Kickoff", "Best regards, Søren Ødegård"),
        m("de", "Adresse", "Hauptstraße 12, München"),
        m("ups", "Shipped", "Tracking number: 1Z 4F8 A62 03 1187 2290"),
        m("usps", "On its way", "Tracking: 9400111899562537866361"),
        m("phone", "New number", "My new cell is (415) 555-0138."),
        m("uk-phone", "Booking", "Call us on +44 20 7946 0958."),
        m("code", "Past due", "Invoice INV-20417 is 30 days past due."),
        m("ver", "Deploy", "Release v2.14.3 goes out tonight."),
        m("bike", "Bike", "Your bike is ready for pickup."),
        M::new(A, "will", "will", t - DAY).from("Robert Kowalski", "rkowalski@law.example").subject("Will draft").body("Draft attached. Bob").done(),
    ])
    .unwrap();
    for (q, want) in [
        ("crestline invoices", "inv"),
        ("color proofs", "uk"),
        ("gray organize", "uk"),
        ("order canceled", "uk"),
        ("oncall handoff", "hyph"),
        ("wifi down", "hyph"),
        ("2450", "rent"),
        ("$2,450.00", "rent"),
        ("1234.56", "eur"),
        ("odegard", "nordic"),
        ("hauptstrasse", "de"),
        ("1Z4F8A620311872290", "ups"),
        ("9400 1118 9956 2537 8663 61", "usps"),
        ("4155550138", "phone"),
        ("+1 415 555 0138", "phone"),
        ("020 7946 0958", "uk-phone"),
        ("INV20417", "code"),
        ("2.14.3", "ver"),
        ("bike pick up", "bike"),
        ("bike pick-up", "bike"),
        ("crestlnie invoice", "inv"),
        ("rob kowalski", "will"),
    ] {
        assert_eq!(hit_ids(&search(&s, &format!("{q} "))), vec![want.to_string()], "{q}");
    }
    // The typed form still counts, and quotes stay exact.
    assert_eq!(ids(&s, "invoice crestline "), sorted(&["inv"]));
    assert!(search(&s, "\"crestline invoices\" ").hits.is_empty());
    // A form the mail doesn't use adds nothing.
    assert!(search(&s, "zebras ").hits.is_empty());
}

#[test]
fn account_removal_rebuilds_first_contacts() {
    let s = store_with_accounts(&[A, B]);
    let t = now();
    s.upsert_messages(&[
        M::new(B, "b1", "T1", t - 9 * DAY)
            .from("", "ana@ruiz.example")
            .done(),
        M::new(A, "a1", "T2", t - 5 * DAY)
            .from("", "ana@ruiz.example")
            .done(),
    ])
    .unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["b1"]));
    s.remove_account(B).unwrap();
    assert_eq!(ids(&s, "is:new-sender"), sorted(&["a1"]));
}
