//! A relationship story and a year of receipts planted on top of the
//! synthetic corpus, so Ask's relationship, spend and date questions do real
//! work. Fictional people at `.example` domains only.
#![allow(dead_code)]

use penguin_core::{Address, Message};

use super::corpus;

const DAY: i64 = corpus::DAY;

#[allow(clippy::too_many_arguments)]
fn msg(
    id: String,
    thread: String,
    date: i64,
    from: Address,
    to: Address,
    subject: &str,
    body: &str,
    sent: bool,
) -> Message {
    Message {
        account_id: corpus::ACCOUNTS[0].into(),
        id,
        thread_id: thread,
        date,
        from,
        to: vec![to],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: subject.into(),
        snippet: body.chars().take(100).collect(),
        body_text: body.into(),
        body_html: None,
        label_ids: vec![if sent { "SENT" } else { "INBOX" }.into()],
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        sender_authenticated: true,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
    }
}

/// A relationship story and a year of receipts on top of the corpus, so
/// the relationship, spend and date questions do real work.
pub fn story(now: i64) -> Vec<Message> {
    let me = Address {
        name: Some("Alex".into()),
        email: corpus::ACCOUNTS[0].into(),
    };
    let mike = Address {
        name: Some("Mike Kestrel".into()),
        email: "mike@kettleontheknoll.example".into(),
    };
    let fernwood = Address {
        name: Some("Mike Kestrel".into()),
        email: "mike@fernwood.example".into(),
    };
    let uber = Address {
        name: Some("Uber Receipts".into()),
        email: "noreply@uber.example".into(),
    };
    let mut out = Vec::new();
    for w in 0..70 {
        let d = now - 480 * DAY + w * 5 * DAY;
        out.push(msg(
            format!("s{w}a"),
            format!("st{w}"),
            d,
            mike.clone(),
            me.clone(),
            "Design sync",
            "Priorities for this week.",
            false,
        ));
        out.push(msg(
            format!("s{w}b"),
            format!("st{w}"),
            d + DAY,
            me.clone(),
            mike.clone(),
            "Re: Design sync",
            "On it.",
            true,
        ));
    }
    out.push(msg(
        "sh".into(),
        "sh".into(),
        now - 90 * DAY,
        mike.clone(),
        me.clone(),
        "Where we're leaving off",
        "We're bringing design in-house, so this is where we're leaving off.",
        false,
    ));
    out.push(msg(
        "sd".into(),
        "sd".into(),
        now - 4 * DAY,
        fernwood,
        me.clone(),
        "Dinner Thursday?",
        "Want to grab dinner on Thursday at 7pm?",
        false,
    ));
    for i in 0..300 {
        let body = format!(
            "Thanks for riding\nTrip fare ${}.00\nTotal ${}.40",
            10 + i % 30,
            12 + i % 30
        );
        out.push(msg(
            format!("u{i}"),
            format!("u{i}"),
            now - (i as i64) * DAY,
            uber.clone(),
            me.clone(),
            "Your trip with Uber",
            &body,
            false,
        ));
    }
    out
}
