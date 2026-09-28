use penguin_core::store::CalendarCursor;
use penguin_core::{
    Account, AccountProvider, Address, AttachmentMeta, CalendarEvent, CalendarInfo, EventAttendee,
    InviteResponse, Message, Store,
};
use penguin_gmail::ics::parse_invite;

use super::*;

const ME: &str = "ada@northwind.example";
const WORK: &str = "ada@work.example";
const HOUR: i64 = 3_600_000;
/// 2026-09-24T12:00:00Z
const NOW: i64 = 1_790_251_200_000;
/// 2026-10-01T15:00:00Z, the invite below.
const START: i64 = 1_790_866_800_000;

fn invite_ics(method: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nDTSTART:20261001T150000Z\r\n\
         DTEND:20261001T160000Z\r\nUID:q4@google.com\r\nSUMMARY:Q4 planning\r\n\
         ORGANIZER;CN=Mike:mailto:mike@linden.example\r\n\
         ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:{ME}\r\n\
         ATTENDEE;PARTSTAT=ACCEPTED:mailto:mike@linden.example\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn att(filename: &str, mime: &str, size: u64) -> AttachmentMeta {
    AttachmentMeta {
        id: filename.into(),
        filename: filename.into(),
        mime_type: mime.into(),
        size,
        content_id: None,
        inline: false,
    }
}

fn msg(id: &str, attachments: Vec<AttachmentMeta>) -> Message {
    Message {
        account_id: ME.into(),
        id: id.into(),
        thread_id: "t".into(),
        date: NOW,
        from: Address {
            name: None,
            email: "mike@linden.example".into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Invitation: Q4 planning".into(),
        snippet: String::new(),
        body_text: String::new(),
        body_html: None,
        label_ids: vec![],
        attachments,
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

fn event(account: &str, id: &str, summary: &str, start: i64, uid: &str) -> CalendarEvent {
    CalendarEvent {
        account_id: account.into(),
        calendar_id: account.into(),
        id: id.into(),
        ical_uid: Some(uid.into()),
        status: "confirmed".into(),
        summary: summary.into(),
        description: String::new(),
        location: String::new(),
        start,
        end: start + HOUR,
        all_day: false,
        start_date: None,
        end_date: None,
        organizer: None,
        attendees: vec![EventAttendee {
            email: account.into(),
            name: None,
            response: "needsAction".into(),
            organizer: false,
            is_self: true,
            optional: false,
            resource: false,
        }],
        my_response: Some("needsAction".into()),
        html_link: None,
        conference_url: None,
        conference_kind: None,
        recurring_event_id: None,
        free: false,
        updated: 0,
    }
}

/// A Google account's card context (calendar read / RSVP granted).
fn ctx(account: &str, read: bool, rsvp: bool) -> CardContext<'_> {
    CardContext {
        account_id: account,
        thread_id: "t",
        message_id: "m1",
        message_date: NOW,
        provider: AccountProvider::Gmail,
        google_calendar: true,
        calendar_connected: read,
        rsvp_granted: rsvp,
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    for a in [ME, WORK] {
        s.upsert_account(&Account {
            id: a.into(),
            email: a.into(),
            display_name: None,
            nickname: None,
            color: "#123456".into(),
            added_at: 0,
            ..Account::default()
        })
        .unwrap();
        s.replace_calendars(
            a,
            &[CalendarInfo {
                account_id: a.into(),
                id: a.into(),
                summary: a.into(),
                color: None,
                selected: true,
                primary: true,
                access_role: "owner".into(),
            }],
            &[None],
        )
        .unwrap();
    }
    s
}

#[test]
fn picks_the_newest_calendar_part_preferring_text_calendar() {
    let messages = vec![
        msg("old", vec![att("invite.ics", "application/ics", 900)]),
        msg(
            "new",
            vec![
                att("q3.pdf", "application/pdf", 900),
                att("invite.ics", "application/ics", 900),
                att("untitled.ics", "text/calendar", 900),
            ],
        ),
        msg("reply", vec![]),
        msg(
            "huge",
            vec![att("big.ics", "text/calendar", 50 * 1024 * 1024)],
        ),
    ];
    let (m, a) = pick_invite_part(&messages).unwrap();
    assert_eq!(
        (m.id.as_str(), a.mime_type.as_str()),
        ("new", "text/calendar")
    );
    assert!(pick_invite_part(&[msg("x", vec![att("a.pdf", "application/pdf", 1)])]).is_none());
    assert!(is_calendar_part(&att(
        "Meeting.ICS",
        "application/octet-stream",
        10
    )));
}

#[test]
fn card_from_the_invite_alone_with_conflicts_across_accounts() {
    let s = store();
    s.replace_calendar_events(
        WORK,
        WORK,
        &[event(WORK, "v", "Vendor call", START + HOUR / 2, "v@x")],
        &CalendarCursor::default(),
    )
    .unwrap();
    let inv = parse_invite(&invite_ics("REQUEST"), ME).unwrap();
    let card = invite_card(&s, &ctx(ME, true, true), &inv, NOW).unwrap();
    assert_eq!(card.method, "request");
    // Not on a calendar yet: answered by email (and RSVP wouldn't help).
    assert!(!card.in_calendar && card.can_respond);
    assert_eq!(card.route, "email");
    assert!(!card.rsvp_available);
    assert!(card.can_propose);
    assert_eq!(card.response.as_deref(), Some("needsAction"));
    assert_eq!(card.event.summary, "Q4 planning");
    assert_eq!(card.event.my_response.as_deref(), Some("needsAction"));
    assert_eq!(card.conflicts.len(), 1);
    assert_eq!(card.conflicts[0].summary, "Vendor call");
}

#[test]
fn card_uses_the_synced_event_and_rsvp_rules() {
    let s = store();
    let mut stored = event(ME, "q4", "Q4 planning (moved)", START, "q4@google.com");
    stored.my_response = Some("accepted".into());
    s.replace_calendar_events(ME, ME, &[stored], &CalendarCursor::default())
        .unwrap();
    let inv = parse_invite(&invite_ics("REQUEST"), ME).unwrap();
    let card = invite_card(&s, &ctx(ME, true, true), &inv, NOW).unwrap();
    assert!(card.in_calendar && card.can_respond);
    assert_eq!(card.route, "calendar");
    assert_eq!(card.response.as_deref(), Some("accepted"));
    assert_eq!(card.event.summary, "Q4 planning (moved)");
    assert_eq!(card.event.my_response.as_deref(), Some("accepted"));
    assert!(
        card.conflicts.is_empty(),
        "an event never conflicts with itself"
    );

    // No RSVP scope: by email, and Settings can turn RSVP on.
    let card = invite_card(&s, &ctx(ME, true, false), &inv, NOW).unwrap();
    assert!(card.can_respond && card.rsvp_available);
    assert_eq!(card.route, "email");
    // A past event, or a cancellation: read-only.
    assert!(
        !invite_card(&s, &ctx(ME, true, true), &inv, START + 2 * HOUR)
            .unwrap()
            .can_respond
    );
    let cancel = parse_invite(&invite_ics("CANCEL"), ME).unwrap();
    let card = invite_card(&s, &ctx(ME, true, true), &cancel, NOW).unwrap();
    assert_eq!(card.method, "cancel");
    assert!(!card.can_respond && card.conflicts.is_empty());

    // Another account's copy of the same meeting isn't this account's.
    let card = invite_card(&s, &ctx(WORK, true, true), &inv, NOW).unwrap();
    assert!(!card.in_calendar);
}

#[test]
fn a_series_invite_shows_the_next_instance() {
    let s = store();
    let week = 7 * 24 * HOUR;
    let instances: Vec<CalendarEvent> = (-2..3)
        .map(|i| {
            event(
                ME,
                &format!("wk_{i}"),
                "Weekly",
                NOW + i * week,
                "wk@google.com",
            )
        })
        .collect();
    s.replace_calendar_events(ME, ME, &instances, &CalendarCursor::default())
        .unwrap();
    let ics = "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VEVENT\nUID:wk@google.com\nSUMMARY:Weekly\n\
               DTSTART:20260801T120000Z\nDTEND:20260801T130000Z\nRRULE:FREQ=WEEKLY\n\
               ATTENDEE:mailto:ada@northwind.example\nEND:VEVENT\nEND:VCALENDAR";
    let inv = parse_invite(ics, ME).unwrap();
    let card = invite_card(&s, &ctx(ME, true, false), &inv, NOW + HOUR / 2).unwrap();
    assert!(card.recurring && card.in_calendar);
    assert_eq!(
        card.event.id, "wk_0",
        "the instance in progress, not an old one"
    );
    let card = invite_card(&s, &ctx(ME, true, false), &inv, NOW + 2 * HOUR).unwrap();
    assert_eq!(card.event.id, "wk_1");
}

#[test]
fn each_provider_answers_its_own_way() {
    let s = store();
    let inv = parse_invite(&invite_ics("REQUEST"), ME).unwrap();
    let mut c = ctx(ME, false, false);
    c.provider = AccountProvider::Imap;
    c.google_calendar = false;
    let card = invite_card(&s, &c, &inv, NOW).unwrap();
    assert_eq!(card.route, "email");
    assert!(!card.rsvp_available, "IMAP has no RSVP to turn on");
    c.provider = AccountProvider::Microsoft;
    assert_eq!(invite_card(&s, &c, &inv, NOW).unwrap().route, "graph");
    // A reply to your event, or an invite you organize: nothing to answer.
    let reply = parse_invite(&invite_ics("REPLY"), ME).unwrap();
    assert_eq!(invite_card(&s, &c, &reply, NOW).unwrap().route, "none");
    let mine = parse_invite(&invite_ics("REQUEST"), "mike@linden.example").unwrap();
    let card = invite_card(&s, &ctx("mike@linden.example", true, true), &mine, NOW).unwrap();
    assert!(!card.can_respond);
}

#[test]
fn an_update_says_what_changed_and_keeps_what_you_sent() {
    let s = store();
    let first = parse_invite(&invite_ics("REQUEST"), ME).unwrap();
    s.put_invite(ME, "m0", "t", NOW - HOUR, Some(&first.to_snapshot()))
        .unwrap();
    s.record_invite_response(
        ME,
        "q4@google.com",
        None,
        &InviteResponse {
            response: "accepted".into(),
            via: "email".into(),
            sequence: 0,
            comment: Some("See you there".into()),
            proposed_start: None,
            proposed_end: None,
            at: NOW - HOUR / 2,
        },
    )
    .unwrap();
    // Same sequence: your answer stands, nothing changed.
    let card = invite_card(&s, &ctx(ME, true, false), &first, NOW).unwrap();
    assert_eq!(card.response.as_deref(), Some("accepted"));
    assert_eq!(
        card.sent.as_ref().unwrap().comment.as_deref(),
        Some("See you there")
    );
    // Moved an hour later and to another room, SEQUENCE 1.
    let moved = invite_ics("REQUEST")
        .replace("DTSTART:20261001T150000Z", "DTSTART:20261001T160000Z")
        .replace("DTEND:20261001T160000Z", "DTEND:20261001T170000Z")
        .replace(
            "SUMMARY:Q4 planning",
            "SEQUENCE:1\r\nLOCATION:Room 9\r\nSUMMARY:Q4 planning",
        );
    let moved = parse_invite(&moved, ME).unwrap();
    let card = invite_card(&s, &ctx(ME, true, false), &moved, NOW).unwrap();
    assert!(card.updated);
    assert_eq!(card.changes, vec!["time", "location"]);
    assert_eq!(card.previous.as_ref().unwrap().start, START);
    // The organizer changed it after you answered: asked again.
    assert_eq!(card.response.as_deref(), Some("needsAction"));
}

// ---- a real Google Calendar thread, end to end ----------------------------

const GUEST: &str = "alex.morgan@gmail.example";
const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../crates/penguin-gmail/tests/fixtures"
);

fn gmail_fixture(name: &str) -> penguin_gmail::convert::GmailMessage {
    let json = std::fs::read_to_string(format!("{FIXTURES}/{name}.json")).unwrap();
    serde_json::from_str(&json).unwrap()
}

/// "Invitation" then "Updated invitation" (Changed: time) in one Gmail
/// thread, as messages.get(format=full) returns them: converted, stored,
/// read back, picked, downloaded (the `part:` path of get_attachment),
/// parsed, carded and chipped.
#[test]
fn a_google_invitation_thread_goes_all_the_way_to_the_card_and_chip() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&Account {
        id: GUEST.into(),
        email: GUEST.into(),
        color: "#123456".into(),
        ..Account::default()
    })
    .unwrap();
    let raw: Vec<_> = ["google_invite", "google_invite_updated"]
        .into_iter()
        .map(gmail_fixture)
        .collect();
    let messages: Vec<Message> = raw
        .iter()
        .map(|gm| penguin_gmail::convert::to_message(GUEST, gm, &Default::default()))
        .collect();
    s.upsert_messages(&messages).unwrap();

    let thread = s.get_thread(GUEST, "19a7c0thread").unwrap().unwrap();
    assert_eq!(thread.messages.len(), 2);
    for m in &thread.messages {
        let kinds: Vec<_> = m
            .attachments
            .iter()
            .map(|a| (a.mime_type.as_str(), a.filename.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("text/calendar", "untitled.ics"),
                ("application/ics", "invite.ics")
            ]
        );
    }
    let (m, part) = pick_invite_part(&thread.messages).unwrap();
    assert_eq!(
        (m.id.as_str(), part.id.as_str()),
        ("19a7c0update", "part:0.2")
    );
    // What get_attachment does for a `part:` id: re-read the message.
    let bytes =
        penguin_gmail::convert::inline_part_bytes(&raw[1], part.id.strip_prefix("part:").unwrap())
            .unwrap();
    let inv = parse_invite(&String::from_utf8_lossy(&bytes), GUEST).unwrap();
    assert_eq!((inv.method_name(), inv.sequence), ("request", 1));

    // The scanner (45 days back) finds both messages and stores what they say.
    let now = 1_790_400_000_000; // 2026-09-26, before the lunch
    let found = s.invite_scan_candidates(now - 45 * 24 * HOUR, 25).unwrap();
    assert_eq!(found.len(), 2);
    for c in &found {
        let gm = raw.iter().find(|g| g.id == c.message_id).unwrap();
        let part = c
            .parts
            .iter()
            .find(|p| p.mime_type == "text/calendar")
            .unwrap();
        let bytes =
            penguin_gmail::convert::inline_part_bytes(gm, part.id.strip_prefix("part:").unwrap())
                .unwrap();
        let snap = parse_invite(&String::from_utf8_lossy(&bytes), GUEST)
            .unwrap()
            .to_snapshot();
        s.put_invite(GUEST, &c.message_id, &c.thread_id, c.date, Some(&snap))
            .unwrap();
    }

    let c = CardContext {
        account_id: GUEST,
        thread_id: &m.thread_id,
        message_id: &m.id,
        message_date: m.date,
        provider: AccountProvider::Gmail,
        google_calendar: true,
        calendar_connected: false,
        rsvp_granted: false,
    };
    let card = invite_card(&s, &c, &inv, now).unwrap();
    assert!(card.can_respond && card.updated);
    assert_eq!(card.route, "email");
    assert_eq!(card.changes, vec!["time"]);
    assert_eq!(card.event.summary, "Sam <> Alex / Lunch");

    let query: penguin_core::ListQuery =
        serde_json::from_value(serde_json::json!({ "view": { "kind": "inbox" }, "limit": 50 }))
            .unwrap();
    let mut rows = s.list_threads(&query).unwrap();
    s.attach_invites(&mut rows, now).unwrap();
    let chip = rows[0].invite.as_ref().expect("the row's event chip");
    assert!(rows[0].has_attachments && chip.updated && chip.can_respond);
    assert_eq!(chip.start, inv.start);
}

// ---- calendar in the add-account / reconnect consent ------------------------

const GMAIL: &str = "https://www.googleapis.com/auth/gmail.modify";

fn scopes(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn sign_in_asks_for_read_only_calendar_when_the_setting_is_on() {
    // A new account: read-only only, never the RSVP scope.
    assert_eq!(sign_in_scopes(true, &[]), vec![CALENDAR_READONLY_SCOPE]);
    // Setting off: mail only.
    assert!(sign_in_scopes(false, &[]).is_empty());
    assert!(sign_in_scopes(false, &scopes(&["openid", GMAIL])).is_empty());
}

#[test]
fn reconnect_keeps_the_calendar_access_the_account_had() {
    let with_rsvp = scopes(&[GMAIL, CALENDAR_READONLY_SCOPE, CALENDAR_EVENTS_SCOPE]);
    // Even with the setting off: it was connected before, so keep it.
    for on in [true, false] {
        assert_eq!(
            sign_in_scopes(on, &with_rsvp),
            vec![CALENDAR_READONLY_SCOPE, CALENDAR_EVENTS_SCOPE]
        );
    }
    let read_only = scopes(&[GMAIL, CALENDAR_READONLY_SCOPE]);
    assert_eq!(
        sign_in_scopes(false, &read_only),
        vec![CALENDAR_READONLY_SCOPE]
    );
}

#[test]
fn calendar_is_connected_only_when_asked_for_and_granted() {
    let asked = [CALENDAR_READONLY_SCOPE];
    // Granted: the first sync starts now.
    assert!(calendar_granted(
        &asked,
        &scopes(&["openid", GMAIL, CALENDAR_READONLY_SCOPE])
    ));
    // The calendar box was unticked on Google's screen: mail only, no error.
    assert!(!calendar_granted(&asked, &scopes(&["openid", GMAIL])));
    // Only RSVP granted (read-only unticked): not connected.
    assert!(!calendar_granted(
        &[CALENDAR_READONLY_SCOPE, CALENDAR_EVENTS_SCOPE],
        &scopes(&[GMAIL, CALENDAR_EVENTS_SCOPE])
    ));
    // Not asked for (setting off): nothing to start, whatever the token has.
    assert!(!calendar_granted(
        &[],
        &scopes(&[GMAIL, CALENDAR_READONLY_SCOPE])
    ));
}
