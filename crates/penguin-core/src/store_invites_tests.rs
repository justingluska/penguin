//! Invitations: scan candidates, snapshots, answers, "what changed" and the
//! row chip.

use super::*;
use crate::store::CalendarCursor;

const ME: &str = "sam@northwind.example";
const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn account() -> Account {
    Account {
        id: ME.into(),
        email: ME.into(),
        color: "#123456".into(),
        added_at: 1,
        ..Account::default()
    }
}

fn attendee(email: &str, response: &str, is_self: bool) -> EventAttendee {
    EventAttendee {
        email: email.into(),
        name: None,
        response: response.into(),
        organizer: false,
        is_self,
        optional: false,
        resource: false,
    }
}

fn snapshot(start: i64) -> InviteSnapshot {
    InviteSnapshot {
        method: "request".into(),
        uid: Some("plan-review@calendar.example".into()),
        sequence: 0,
        summary: "Plan review".into(),
        location: "Room 4".into(),
        start,
        end: start + HOUR,
        organizer: Some(Address {
            name: Some("Priya Natarajan".into()),
            email: "priya@linden.example".into(),
        }),
        attendees: vec![
            attendee("priya@linden.example", "accepted", false),
            attendee(ME, "needsAction", true),
        ],
        ..InviteSnapshot::default()
    }
}

fn message(id: &str, date: i64, with_ics: bool) -> Message {
    Message {
        account_id: ME.into(),
        id: id.into(),
        thread_id: "t1".into(),
        date,
        from: Address {
            name: None,
            email: "priya@linden.example".into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Invitation: Plan review".into(),
        snippet: String::new(),
        body_text: "Plan review".into(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: if with_ics {
            vec![AttachmentMeta {
                id: format!("att-{id}"),
                filename: "invite.ics".into(),
                mime_type: "text/calendar".into(),
                size: 900,
                content_id: None,
                inline: false,
            }]
        } else {
            vec![]
        },
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account()).unwrap();
    s
}

fn inbox(s: &Store) -> Vec<ThreadSummary> {
    let mut rows = s
        .list_threads(&ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 10,
            before: None,
            unread_only: false,
            split: None,
        })
        .unwrap();
    s.attach_invites(&mut rows, now()).unwrap();
    rows
}

#[test]
fn scanner_finds_calendar_parts_once() {
    let s = store();
    let t = now() - HOUR;
    s.upsert_messages(&[message("m1", t, true), message("m2", t + 1, false)])
        .unwrap();
    let c = s.invite_scan_candidates(t - DAY, 10).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].message_id, "m1");
    assert_eq!(c[0].thread_id, "t1");
    assert_eq!(c[0].parts[0].mime_type, "text/calendar");
    // Older than the window: not a candidate.
    assert!(s.invite_scan_candidates(t + 10, 10).unwrap().is_empty());
    // Once stored (even as "nothing usable") it isn't offered again.
    s.put_invite(ME, "m1", "t1", t, None).unwrap();
    assert!(s.invite_scan_candidates(t - DAY, 10).unwrap().is_empty());
    assert_eq!(s.thread_invite(ME, "t1").unwrap(), None);
}

#[test]
fn row_chip_merges_answers_and_conflicts() {
    let s = store();
    let t = now();
    let start = t + DAY;
    s.upsert_messages(&[message("m1", t - HOUR, true)]).unwrap();
    s.put_invite(ME, "m1", "t1", t - HOUR, Some(&snapshot(start)))
        .unwrap();
    // A busy event at the same time on the calendar.
    s.replace_calendars(
        ME,
        &[CalendarInfo {
            account_id: ME.into(),
            id: ME.into(),
            summary: ME.into(),
            color: None,
            selected: true,
            primary: true,
            access_role: "owner".into(),
        }],
        &[None],
    )
    .unwrap();
    let standup = CalendarEvent {
        account_id: ME.into(),
        calendar_id: ME.into(),
        id: "standup".into(),
        ical_uid: Some("standup@calendar.example".into()),
        status: "confirmed".into(),
        summary: "Standup".into(),
        description: String::new(),
        location: String::new(),
        start: start + 30 * 60_000,
        end: start + HOUR,
        all_day: false,
        start_date: None,
        end_date: None,
        organizer: None,
        attendees: vec![],
        my_response: None,
        html_link: None,
        conference_url: None,
        conference_kind: None,
        recurring_event_id: None,
        free: false,
        updated: 0,
    };
    s.replace_calendar_events(ME, ME, &[standup], &CalendarCursor::default())
        .unwrap();

    let rows = inbox(&s);
    let chip = rows[0].invite.clone().expect("chip");
    assert_eq!(chip.summary, "Plan review");
    assert_eq!(chip.method, "request");
    assert!(chip.can_respond);
    assert!(!chip.updated);
    assert_eq!(chip.response.as_deref(), Some("needsAction"));
    assert_eq!(chip.conflict.as_deref(), Some("Standup"));
    assert_eq!(chip.conflicts, 1);

    // Penguin sent "yes": the row says so at once.
    s.record_invite_response(
        ME,
        "plan-review@calendar.example",
        None,
        &InviteResponse {
            response: "accepted".into(),
            via: "email".into(),
            sequence: 0,
            comment: None,
            proposed_start: None,
            proposed_end: None,
            at: t,
        },
    )
    .unwrap();
    let chip = inbox(&s)[0].invite.clone().unwrap();
    assert_eq!(chip.response.as_deref(), Some("accepted"));

    // A newer version (higher SEQUENCE) asks again, and is "updated".
    let mut moved = snapshot(start + 2 * HOUR);
    moved.sequence = 2;
    s.upsert_messages(&[message("m2", t, true)]).unwrap();
    s.put_invite(ME, "m2", "t1", t, Some(&moved)).unwrap();
    let chip = inbox(&s)[0].invite.clone().unwrap();
    assert_eq!(chip.message_id, "m2");
    assert!(chip.updated);
    assert_eq!(chip.response.as_deref(), Some("needsAction"));
    // Moved away from the standup.
    assert_eq!(chip.conflicts, 0);

    // What changed against the version you had.
    let prev = s.previous_invite(ME, &moved, "m2", t).unwrap().unwrap();
    assert_eq!(prev.start, start);
    assert_eq!(moved.changes_since(&prev), vec!["time".to_string()]);
}

#[test]
fn effective_response_prefers_the_newest_source() {
    let t = now();
    let inv = snapshot(t + DAY);
    let mut ev = CalendarEvent {
        account_id: ME.into(),
        calendar_id: ME.into(),
        id: "e".into(),
        ical_uid: inv.uid.clone(),
        status: "confirmed".into(),
        summary: String::new(),
        description: String::new(),
        location: String::new(),
        start: inv.start,
        end: inv.end,
        all_day: false,
        start_date: None,
        end_date: None,
        organizer: None,
        attendees: vec![],
        my_response: Some("declined".into()),
        html_link: None,
        conference_url: None,
        conference_kind: None,
        recurring_event_id: None,
        free: false,
        updated: t - HOUR,
    };
    let sent = InviteResponse {
        response: "accepted".into(),
        via: "email".into(),
        sequence: 0,
        comment: None,
        proposed_start: None,
        proposed_end: None,
        at: t,
    };
    // Nothing but the invite: its PARTSTAT.
    assert_eq!(
        effective_response(&inv, None, None).as_deref(),
        Some("needsAction")
    );
    // Sent after the calendar last changed: what was sent.
    assert_eq!(
        effective_response(&inv, Some(&ev), Some(&sent)).as_deref(),
        Some("accepted")
    );
    // Changed in Google Calendar since: the calendar wins.
    ev.updated = t + 1;
    assert_eq!(
        effective_response(&inv, Some(&ev), Some(&sent)).as_deref(),
        Some("declined")
    );
    // Not a guest and never answered: nothing.
    let mut outsider = inv.clone();
    outsider.attendees.retain(|a| !a.is_self);
    assert_eq!(effective_response(&outsider, None, None), None);
}

#[test]
fn what_is_answerable() {
    let t = now();
    let inv = snapshot(t + DAY);
    assert!(answerable(&inv, t));
    // Over.
    assert!(!answerable(&snapshot(t - DAY), t));
    // A past start but a recurring series goes on.
    let mut series = snapshot(t - 30 * DAY);
    series.recurring = true;
    assert!(answerable(&series, t));
    // Cancelled, a reply, no UID, or you organize it.
    for f in [
        (|i: &mut InviteSnapshot| i.method = "cancel".into()) as fn(&mut InviteSnapshot),
        |i| i.method = "reply".into(),
        |i| i.uid = None,
        |i| {
            i.organizer = Some(Address {
                name: None,
                email: ME.into(),
            })
        },
    ] {
        let mut i = inv.clone();
        f(&mut i);
        assert!(!answerable(&i, t), "{i:?}");
    }
}

#[test]
fn changes_list_every_field_that_moved() {
    let a = snapshot(1_000_000);
    let mut b = a.clone();
    assert!(b.changes_since(&a).is_empty());
    b.location = "Room 9".into();
    b.summary = "Plan review (moved)".into();
    b.attendees
        .push(attendee("ana@harborlabs.example", "needsAction", false));
    assert_eq!(b.changes_since(&a), vec!["location", "title", "guests"]);
}

#[test]
fn series_answers_cover_instances_and_removal_clears() {
    let s = store();
    let r = |response: &str, at| InviteResponse {
        response: response.into(),
        via: "calendar".into(),
        sequence: 0,
        comment: Some("Running late".into()),
        proposed_start: None,
        proposed_end: None,
        at,
    };
    s.record_invite_response(ME, "u", None, &r("accepted", 10))
        .unwrap();
    assert_eq!(
        s.invite_response(ME, "u", Some(5))
            .unwrap()
            .unwrap()
            .response,
        "accepted"
    );
    s.record_invite_response(ME, "u", Some(5), &r("declined", 20))
        .unwrap();
    assert_eq!(
        s.invite_response(ME, "u", Some(5))
            .unwrap()
            .unwrap()
            .response,
        "declined"
    );
    assert_eq!(
        s.invite_response(ME, "u", None).unwrap().unwrap().response,
        "accepted"
    );
    s.remove_account(ME).unwrap();
    assert_eq!(s.invite_response(ME, "u", None).unwrap(), None);
}
