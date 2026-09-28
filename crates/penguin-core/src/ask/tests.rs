//! End-to-end Ask tests on a small fictional mailbox. "Now" is
//! Thu 2026-09-24 12:00 UTC; the user's offset is UTC.
//!
//! The Kettle on the Knoll story (the questions that motivated Ask): intro
//! Jan 2025, regular work May 2025 – Jun 2026, a "where we're leaving off"
//! handoff on Jun 26 2026, a final invoice in July, then Mike (now at
//! Fernwood) sets up dinner for Thu Sep 24.

use chrono::{NaiveDate, TimeZone, Utc};

use super::*;
use crate::types::{Account, AttachmentMeta, Message};
use crate::Store;

const ME: &str = "alex@studio.example";

fn now() -> i64 {
    Utc.with_ymd_and_hms(2026, 9, 24, 12, 0, 0)
        .unwrap()
        .timestamp_millis()
}

fn at(y: i32, m: u32, d: u32, h: u32) -> i64 {
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0)
        .unwrap()
        .timestamp_millis()
}

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: Some(name.into()),
        email: email.into(),
    }
}

struct Fx {
    msgs: Vec<Message>,
    n: usize,
}

impl Fx {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        thread: &str,
        date: i64,
        from: Address,
        to: Vec<Address>,
        subject: &str,
        body: &str,
        labels: &[&str],
    ) -> &mut Message {
        self.n += 1;
        let sent = from.email == ME;
        let mut label_ids: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
        if sent {
            label_ids.push("SENT".into());
        }
        self.msgs.push(Message {
            account_id: ME.into(),
            id: format!("m{}", self.n),
            thread_id: thread.into(),
            date,
            from,
            to,
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: subject.into(),
            snippet: body.chars().take(120).collect(),
            body_text: body.into(),
            body_html: None,
            label_ids,
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: true,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        });
        self.msgs.last_mut().unwrap()
    }
}

fn pdf(name: &str) -> AttachmentMeta {
    AttachmentMeta {
        id: format!("att-{name}"),
        filename: name.into(),
        mime_type: "application/pdf".into(),
        size: 1000,
        content_id: None,
        inline: false,
    }
}

fn mailbox() -> Store {
    let store = Store::open_in_memory().unwrap();
    fill(&store);
    store
}

/// The fixture mailbox, written into `store`.
fn fill(store: &Store) {
    store
        .upsert_account(&Account {
            id: ME.into(),
            email: ME.into(),
            display_name: Some("Alex".into()),
            nickname: None,
            color: "#336699".into(),
            added_at: 0,
            ..Account::default()
        })
        .unwrap();
    let me = addr("Alex", ME);
    let mike = addr("Mike Kestrel", "mike@kettleontheknoll.example");
    let mike_fernwood = addr("Mike Kestrel", "mike@fernwood.example");
    let delgado = addr("Mike Delgado", "mike@delgado.example");
    let priya = addr("Priya Natarajan", "priya@linden.example");
    let omar = addr("Omar Haddad", "omar@northwind.example");
    let dana = addr("Dana Whitfield", "dana@oakrealty.example");
    let uber_r = addr("Uber Receipts", "noreply@uber.example");
    let uber = addr("Uber", "uber@uber.example");
    let inbox: &[&str] = &["INBOX"];
    let mut fx = Fx { msgs: vec![], n: 0 };

    // --- Kettle on the Knoll
    fx.add(
        "kotk-intro",
        at(2025, 1, 15, 15),
        mike.clone(),
        vec![me.clone()],
        "Intro: Kettle on the Knoll design",
        "Hi Alex, a friend recommended you for design work. Could we talk sometime?",
        &[],
    );
    let mut month = NaiveDate::from_ymd_opt(2025, 5, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    while month <= end {
        let (y, m) = (
            chrono::Datelike::year(&month),
            chrono::Datelike::month(&month),
        );
        let t = format!("kotk-{y}-{m}");
        let subj = format!("Design sync — {}", month.format("%b %Y"));
        fx.add(
            &t,
            at(y, m, 5, 14),
            mike.clone(),
            vec![me.clone()],
            &subj,
            "Here are this week's priorities for the site.",
            &[],
        );
        fx.add(
            &t,
            at(y, m, 6, 16),
            me.clone(),
            vec![mike.clone()],
            &format!("Re: {subj}"),
            "Thanks Mike, on it. Report attached.",
            &[],
        );
        fx.add(
            &t,
            at(y, m, 12, 14),
            mike.clone(),
            vec![me.clone()],
            &format!("Re: {subj}"),
            "Looks great, thank you.",
            &[],
        );
        month = crate::dates::add_months(month, 1).unwrap();
    }
    fx.add(
        "kotk-handoff",
        at(2026, 6, 26, 17),
        mike.clone(),
        vec![me.clone()],
        "Where we're leaving off",
        "Hi Alex,\nThanks for everything this past year. We've decided to bring design in-house, so this is where we're leaving off. Please send over a final invoice.\nMike",
        &[],
    );
    fx.add(
        "kotk-final",
        at(2026, 7, 10, 15),
        me.clone(),
        vec![mike.clone()],
        "Final invoice",
        "Hi Mike, attached is the final invoice. Total $4,500.00. It was a pleasure.",
        &[],
    )
    .attachments = vec![pdf("studio-invoice-final.pdf")];
    fx.add(
        "dinner",
        at(2026, 9, 20, 18),
        mike_fernwood.clone(),
        vec![me.clone()],
        "Dinner Thursday?",
        "Hey Alex,\nNow that I've landed at Fernwood, want to grab dinner on Thursday, Sep 24 at 7pm? My treat.\nMike",
        inbox,
    );
    fx.add(
        "dinner",
        at(2026, 9, 24, 9),
        me.clone(),
        vec![mike_fernwood.clone()],
        "Re: Dinner Thursday?",
        "See you tonight!",
        &[],
    );

    // --- Others
    fx.add(
        "delgado",
        at(2026, 3, 3, 12),
        delgado.clone(),
        vec![me.clone()],
        "Quick hello",
        "Hi Alex, great meeting you at the conference.",
        &[],
    );
    for (i, (y, m, d)) in [(2026, 4, 2), (2026, 6, 9), (2026, 8, 4)]
        .into_iter()
        .enumerate()
    {
        let t = format!("priya-{i}");
        fx.add(
            &t,
            at(y, m, d, 13),
            priya.clone(),
            vec![me.clone()],
            "Linden roadmap",
            "Sharing the latest roadmap draft.",
            &[],
        );
        fx.add(
            &t,
            at(y, m, d + 1, 13),
            me.clone(),
            vec![priya.clone()],
            "Re: Linden roadmap",
            "Thanks, looks good.",
            &[],
        );
    }
    // You wrote Priya on the 15th and haven't heard back.
    fx.add(
        "priya-waiting",
        at(2026, 9, 15, 10),
        me.clone(),
        vec![priya.clone()],
        "Contract draft for Q4",
        "Hi Priya, here's the Q4 contract draft. Let me know.",
        &[],
    );
    // Priya asked you something on the 22nd; you haven't replied.
    fx.add(
        "priya-owed",
        at(2026, 9, 22, 11),
        priya.clone(),
        vec![me.clone()],
        "Quick question on pricing",
        "Can you confirm the pricing for the Q4 retainer?",
        &["INBOX", "UNREAD"],
    );
    // A newsletter in the inbox never counts as owed.
    let n = fx.add(
        "news",
        at(2026, 9, 23, 7),
        addr("Weekly Digest", "digest@news.example"),
        vec![me.clone()],
        "This week in design",
        "Top stories this week.",
        &["INBOX", "CATEGORY_PROMOTIONS"],
    );
    n.list_unsubscribe = Some("<mailto:unsub@news.example>".into());

    fx.add(
        "omar-1041",
        at(2026, 8, 1, 9),
        omar.clone(),
        vec![me.clone()],
        "Invoice #1041",
        "Please find invoice #1041 attached. Amount due: $1,200.00",
        &[],
    )
    .attachments = vec![pdf("invoice-1041.pdf")];
    fx.add(
        "omar-1042",
        at(2026, 9, 1, 9),
        omar.clone(),
        vec![me.clone()],
        "Invoice #1042",
        "Please find invoice #1042 attached. Amount due: $1,350.00",
        &[],
    )
    .attachments = vec![pdf("invoice-1042.pdf")];
    fx.add(
        "omar-chat",
        at(2026, 9, 10, 9),
        omar.clone(),
        vec![me.clone()],
        "Lunch next week?",
        "Free for lunch next week?",
        &[],
    );

    fx.add(
        "lease",
        at(2026, 8, 15, 16),
        dana.clone(),
        vec![me.clone()],
        "Lease renewal for 12 Birch St",
        "Hi Alex,\nYour lease renewal is due by October 31, 2026. Please sign the attached renewal form.\nDana",
        inbox,
    )
    .attachments = vec![pdf("renewal-form.pdf")];

    let receipts = [
        (at(2025, 12, 10, 20), "Your Wednesday evening trip with Uber", "Thanks for riding\nTrip fare $26.00\nBooking fee $4.00\nTotal $30.00"),
        (at(2026, 2, 3, 20), "Your Tuesday evening trip with Uber", "Thanks for riding\nTrip fare $19.10\nSubtotal $21.20\nTip $2.20\nTotal $23.40\nCharged to Visa ••1234"),
        (at(2026, 5, 14, 9), "Your Thursday morning trip with Uber", "Trip fare $16.00\nTotal $18.75"),
        (at(2026, 6, 2, 9), "Your refund from Uber", "We've refunded your cancellation fee.\nRefund total: $5.00"),
        (at(2026, 8, 30, 22), "Your Sunday night trip with Uber", "Trip fare $36.10\nTotal $41.10"),
    ];
    for (i, (d, subj, body)) in receipts.into_iter().enumerate() {
        fx.add(
            &format!("uber-{i}"),
            d,
            uber_r.clone(),
            vec![me.clone()],
            subj,
            body,
            &["CATEGORY_UPDATES"],
        );
    }
    fx.add(
        "uber-promo",
        at(2026, 7, 1, 9),
        uber.clone(),
        vec![me.clone()],
        "50% off your next 3 rides",
        "Save up to $5 per ride this week only.",
        &["CATEGORY_PROMOTIONS"],
    );

    store.upsert_messages(&fx.msgs).unwrap();
}

fn ask(store: &Store, q: &str) -> AskAnswer {
    store.ask(q, &AskScope::default(), now(), 0).unwrap()
}

fn fact<'a>(a: &'a AskAnswer, label: &str) -> &'a AskFact {
    a.facts
        .iter()
        .find(|f| f.label == label)
        .unwrap_or_else(|| panic!("no fact {label:?} in {:#?}", a.facts))
}

// ----------------------------------------------- the three real questions

#[test]
fn last_spoke_to_mike_from_kotk_or_fernwood() {
    let s = mailbox();
    let a = ask(
        &s,
        "when did I last speak to Mike from Kettle on the Knoll / Fernwood?",
    );
    assert_eq!(a.intent, AskIntent::LastContact);
    assert_eq!(
        a.headline,
        "You last emailed Mike Kestrel on Sep 24, 2026 (today)"
    );
    let p = a.person.as_ref().unwrap();
    assert_eq!(
        p.emails,
        vec!["mike@kettleontheknoll.example", "mike@fernwood.example"]
    );
    assert_eq!(fact(&a, "Last from them").date, Some(at(2026, 9, 20, 18)));
    assert_eq!(fact(&a, "First contact").date, Some(at(2025, 1, 15, 15)));
    assert_eq!(a.items[0].subject, "Re: Dinner Thursday?");
    assert_eq!(a.confidence, AskConfidence::High);
    assert!(
        a.steps.iter().any(|s| s.contains("kettleontheknoll")),
        "{:?}",
        a.steps
    );
}

#[test]
fn when_did_we_work_for_mike_kotk() {
    let s = mailbox();
    let a = ask(&s, "when did we work for Mike KOTK, from when to now?");
    assert_eq!(a.intent, AskIntent::Relationship);
    assert_eq!(
        a.headline,
        "You worked with Mike Kestrel from May 2025 to Jun 2026"
    );
    // Resolved through the domain acronym, and only the KOTK address.
    assert_eq!(
        a.person.as_ref().unwrap().emails,
        vec!["mike@kettleontheknoll.example"]
    );
    assert_eq!(fact(&a, "First contact").date, Some(at(2025, 1, 15, 15)));
    assert_eq!(fact(&a, "Regular from").date, Some(at(2025, 5, 5, 14)));
    assert_eq!(fact(&a, "Handoff").date, Some(at(2026, 6, 26, 17)));
    assert_eq!(fact(&a, "Last contact").date, Some(at(2026, 7, 10, 15)));
    assert!(
        a.detail
            .as_deref()
            .unwrap()
            .contains("this is where we're leaving off"),
        "{:?}",
        a.detail
    );
    let tl = a.timeline.as_ref().unwrap();
    assert_eq!(tl.unit, "month");
    assert_eq!(tl.buckets.first().unwrap().label, "Jan 2025");
    let may = tl.buckets.iter().find(|b| b.label == "May 2025").unwrap();
    assert_eq!((may.from_them, may.from_me), (2, 1));
    assert!(tl.markers.iter().any(|m| m.label == "Handoff"));
    assert_eq!(a.items[0].note.as_deref(), Some("Handoff"));
    assert_eq!(a.items[0].subject, "Where we're leaving off");
}

#[test]
fn when_did_he_let_us_go_uses_the_previous_person() {
    let s = mailbox();
    let first = ask(&s, "when did we work for Mike KOTK?");
    let scope = AskScope {
        person: Some(first.person.unwrap().emails),
        ..AskScope::default()
    };
    let a = s.ask("when did he let us go?", &scope, now(), 0).unwrap();
    assert_eq!(a.intent, AskIntent::Relationship);
    assert_eq!(
        a.headline,
        "It looks like it ended on Jun 26, 2026: \u{201c}Where we're leaving off\u{201d}"
    );
    assert_eq!(a.items[0].subject, "Where we're leaving off");

    // Without context, it asks rather than guessing.
    let a = ask(&s, "when did he let us go?");
    assert_eq!(a.confidence, AskConfidence::None);
    assert!(a.headline.starts_with("Who do you mean"), "{}", a.headline);
}

#[test]
fn when_is_the_dinner() {
    let s = mailbox();
    let a = ask(&s, "when is the dinner?");
    assert_eq!(a.intent, AskIntent::When);
    assert_eq!(a.headline, "Dinner: Thu, Sep 24, 2026 at 7pm (today)");
    assert!(
        a.detail
            .as_deref()
            .unwrap()
            .contains("grab dinner on Thursday, Sep 24 at 7pm"),
        "{:?}",
        a.detail
    );
    // Cites the message the date is written in, not the reply.
    let cite = a.facts[0].cite.as_ref().unwrap();
    let m = s
        .get_message(&cite.account_id, &cite.message_id)
        .unwrap()
        .unwrap();
    assert_eq!(m.subject, "Dinner Thursday?");
}

// ------------------------------------------------------------ the others

#[test]
fn first_contact_and_directions() {
    let s = mailbox();
    let a = ask(&s, "first email from Priya");
    assert_eq!(a.intent, AskIntent::FirstContact);
    assert_eq!(
        a.headline,
        "Priya Natarajan first emailed you on Apr 2, 2026 (5 months ago)"
    );
    let a = ask(&s, "when did I last hear from Priya?");
    assert_eq!(
        a.headline,
        "Priya Natarajan last emailed you on Sep 22, 2026 (2 days ago)"
    );
    let a = ask(&s, "when did I last email Mike D");
    assert_eq!(
        a.person.as_ref().unwrap().emails,
        vec!["mike@delgado.example"]
    );
    assert!(
        a.detail
            .as_deref()
            .unwrap()
            .contains("You haven't written to Mike Delgado"),
        "{:?}",
        a.detail
    );
}

#[test]
fn latest_invoice_and_count() {
    let s = mailbox();
    let a = ask(&s, "latest invoice from Omar");
    assert_eq!(a.intent, AskIntent::LatestItem);
    assert_eq!(
        a.headline,
        "The latest invoice from Omar Haddad is \u{201c}Invoice #1042\u{201d} (Sep 1, 2026)"
    );
    assert_eq!(a.detail.as_deref(), Some("Attached: invoice-1042.pdf"));
    assert_eq!(fact(&a, "Invoices in total").value, "2");

    let a = ask(&s, "how many invoices from Omar this year");
    assert_eq!(a.headline, "Omar Haddad sent you 2 invoices this year");
    // The date reading is shown and can be dropped.
    assert_eq!(
        fact(&a, "Dates").value,
        "\u{201c}this year\u{201d} = Jan 1, 2026 – Dec 31, 2026"
    );
    assert_eq!(a.followups[0].question, "How many invoices from omar");
    let a = ask(&s, "how many emails from omar@northwind.example");
    assert_eq!(a.headline, "Omar Haddad sent you 3 emails");
    let a = ask(
        &s,
        "how many emails have I sent to Mike from kettle on the knoll in 2025",
    );
    assert_eq!(a.headline, "You sent Mike Kestrel 8 emails in 2025");
    assert_eq!(a.confidence, AskConfidence::High);
}

#[test]
fn spend_sums_every_receipt() {
    let s = mailbox();
    let a = ask(&s, "how much did I spend on Uber this year?");
    assert_eq!(a.intent, AskIntent::Spend);
    // 23.40 + 18.75 − 5.00 (refund) + 41.10; the promo has no total.
    assert_eq!(a.headline, "$78.25 across 4 Uber receipts this year");
    let sum = a.sum.as_ref().unwrap();
    assert_eq!(sum.totals.len(), 1);
    assert_eq!((sum.totals[0].count, sum.skipped), (4, 1));
    assert_eq!(a.items.len(), 4);
    let amounts: Vec<f64> = a
        .items
        .iter()
        .map(|i| i.amount.as_ref().unwrap().value)
        .collect();
    assert_eq!(amounts, vec![41.10, -5.0, 18.75, 23.40]);
    assert_eq!(a.items[0].amount.as_ref().unwrap().source, "Total $41.10");
    assert_eq!(a.sum.as_ref().unwrap().skipped, 1);
    assert_eq!(a.confidence, AskConfidence::Medium);

    let a = ask(&s, "how much did I spend on uber in 2025");
    assert_eq!(a.headline, "$30.00 across 1 Uber receipt in 2025");
}

#[test]
fn who_and_who_is() {
    let s = mailbox();
    let a = ask(&s, "who emailed me about the lease renewal?");
    assert_eq!(a.intent, AskIntent::WhoAbout);
    assert_eq!(
        a.headline,
        "Dana Whitfield emailed you about \u{201c}lease renewal\u{201d}"
    );
    let a = ask(&s, "who is Priya?");
    assert_eq!(a.headline, "Priya Natarajan <priya@linden.example>");
    assert_eq!(fact(&a, "Company domain").value, "linden.example");
    let a = ask(&s, "who is fernwood");
    assert!(
        a.headline.starts_with("Fernwood (fernwood.example)"),
        "{}",
        a.headline
    );
}

#[test]
fn open_loops() {
    let s = mailbox();
    let a = ask(&s, "what am I waiting on?");
    assert_eq!(a.intent, AskIntent::WaitingOn);
    assert_eq!(a.headline, "You're waiting on replies in 1 thread");
    assert_eq!(a.items[0].subject, "Contract draft for Q4");
    assert!(
        a.items[0]
            .note
            .as_deref()
            .unwrap()
            .starts_with("Waiting 9 days · to Priya Natarajan"),
        "{:?}",
        a.items[0].note
    );
    let a = ask(&s, "what am I waiting on from Omar");
    assert!(
        a.headline.starts_with("Nothing pending: Omar Haddad"),
        "{}",
        a.headline
    );

    let a = ask(&s, "what do I owe replies to?");
    assert_eq!(a.intent, AskIntent::OweReplies);
    // Priya's question; not the newsletter, not Dana (older than 30 days),
    // not Mike (you replied).
    assert_eq!(a.headline, "You owe replies in 1 thread");
    assert_eq!(a.items[0].subject, "Quick question on pricing");
    assert!(a.items[0].note.as_deref().unwrap().starts_with("Unread"));
}

#[test]
fn when_is_my_lease_renewal() {
    let s = mailbox();
    let a = ask(&s, "when is my lease renewal?");
    assert_eq!(a.headline, "Lease renewal: Sat, Oct 31, 2026 (in 5 weeks)");
    assert_eq!(a.confidence, AskConfidence::Medium);
}

#[test]
fn unknown_people_and_questions_say_so() {
    let s = mailbox();
    let a = ask(&s, "when did I last email Zelda");
    assert_eq!(a.confidence, AskConfidence::None);
    assert_eq!(
        a.headline,
        "I don't know anyone matching \u{201c}zelda\u{201d}"
    );
    assert_eq!(a.search_query.as_deref(), Some("zelda"));
    let a = ask(&s, "why is the roadmap late");
    assert_eq!(a.intent, AskIntent::Passage);
    assert_eq!(a.confidence, AskConfidence::None);
    assert_eq!(a.search_query.as_deref(), Some("roadmap late"));
}

#[test]
fn account_scope_limits_answers() {
    let s = mailbox();
    let scope = AskScope {
        account_ids: Some(vec!["other@x.example".into()]),
        ..AskScope::default()
    };
    let a = s
        .ask("when did I last email Priya", &scope, now(), 0)
        .unwrap();
    assert_eq!(a.confidence, AskConfidence::None);
}

#[test]
fn account_nickname_names_a_company() {
    let s = mailbox();
    s.upsert_account(&Account {
        id: "alex@kettleontheknoll.example".into(),
        email: "alex@kettleontheknoll.example".into(),
        display_name: None,
        nickname: Some("Spot".into()),
        color: "#aa3300".into(),
        added_at: 0,
        ..Account::default()
    })
    .unwrap();
    let a = ask(&s, "when did I last email Mike from Knoll");
    assert_eq!(
        a.person.as_ref().unwrap().emails,
        vec!["mike@kettleontheknoll.example"]
    );
}

#[test]
fn whole_word_names_beat_busier_prefix_matches() {
    let s = mailbox();
    let me = addr("Alex", ME);
    let mut fx = Fx {
        msgs: vec![],
        n: 100,
    };
    for d in 1..=9 {
        fx.add(
            &format!("ann-{d}"),
            at(2026, 9, d, 9),
            addr("Annabel Ruiz", "annabel@ruiz.example"),
            vec![me.clone()],
            "Hi",
            "Hello",
            &[],
        );
    }
    fx.add(
        "ann",
        at(2026, 9, 10, 9),
        addr("Ann Lee", "ann@lee.example"),
        vec![me.clone()],
        "Hi",
        "Hello",
        &[],
    );
    s.upsert_messages(&fx.msgs).unwrap();
    let a = ask(&s, "when did I last hear from Ann");
    assert_eq!(a.person.as_ref().unwrap().emails, vec!["ann@lee.example"]);
    assert!(
        a.candidates
            .iter()
            .any(|c| c.label.contains("annabel@ruiz.example")),
        "{:?}",
        a.candidates
    );
}

/// The MCP server answers from `Store::open_read_only` (query_only, no
/// writes): Ask must work there unchanged. Latency is measured by
/// `cargo run -p penguin-core --release --example bench_ask`, not here.
#[test]
fn works_on_a_read_only_store() {
    let dir = std::env::temp_dir().join(format!("penguin-ask-ro-{}", std::process::id()));
    let path = dir.join("penguin.db");
    let _ = std::fs::remove_dir_all(&dir);
    {
        let store = Store::open(&path).unwrap();
        fill(&store);
    }
    let ro = Store::open_read_only(&path).unwrap();
    let a = ro
        .ask(
            "when did I last speak to Mike from Kettle on the Knoll / Fernwood?",
            &AskScope::default(),
            now(),
            0,
        )
        .unwrap();
    assert_eq!(
        a.headline,
        "You last emailed Mike Kestrel on Sep 24, 2026 (today)"
    );
    let a = ro
        .ask(
            "how much did I spend on Uber this year?",
            &AskScope::default(),
            now(),
            0,
        )
        .unwrap();
    assert!(a.headline.starts_with("$78.25 across 4"), "{}", a.headline);
    let a = ro
        .ask("when is the dinner", &AskScope::default(), now(), 0)
        .unwrap();
    assert_eq!(a.intent, AskIntent::When);
    drop(ro);
    let _ = std::fs::remove_dir_all(&dir);
}
