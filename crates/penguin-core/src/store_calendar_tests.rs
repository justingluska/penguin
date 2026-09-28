//! Calendar storage, meetings-with-a-person, conflicts and event search.

use crate::store::CalendarCursor;
use crate::types::*;
use crate::Store;

const ME: &str = "ada@northwind.example";
const WORK: &str = "ada@work.example";
const DAY: i64 = 86_400_000;
const HOUR: i64 = 3_600_000;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn account(id: &str) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        display_name: None,
        nickname: None,
        color: "#123456".into(),
        added_at: 1,
        ..Account::default()
    }
}

fn person(email: &str, name: &str, response: &str) -> EventAttendee {
    EventAttendee {
        email: email.into(),
        name: Some(name.into()),
        response: response.into(),
        organizer: false,
        is_self: false,
        optional: false,
        resource: false,
    }
}

fn me(account: &str, response: &str) -> EventAttendee {
    EventAttendee {
        is_self: true,
        name: None,
        ..person(account, "", response)
    }
}

fn event(account: &str, id: &str, summary: &str, start: i64, hours: i64) -> CalendarEvent {
    CalendarEvent {
        account_id: account.into(),
        calendar_id: account.into(),
        id: id.into(),
        ical_uid: Some(format!("{id}@google.com")),
        status: "confirmed".into(),
        summary: summary.into(),
        description: String::new(),
        location: String::new(),
        start,
        end: start + hours * HOUR,
        all_day: false,
        start_date: None,
        end_date: None,
        organizer: None,
        attendees: vec![me(account, "accepted")],
        my_response: Some("accepted".into()),
        html_link: None,
        conference_url: None,
        conference_kind: None,
        recurring_event_id: None,
        free: false,
        updated: 0,
    }
}

fn calendar(account: &str) -> CalendarInfo {
    CalendarInfo {
        account_id: account.into(),
        id: account.into(),
        summary: account.into(),
        color: None,
        selected: true,
        primary: true,
        access_role: "owner".into(),
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    for a in [ME, WORK] {
        s.upsert_account(&account(a)).unwrap();
        s.replace_calendars(a, &[calendar(a)], &[None]).unwrap();
    }
    s
}

fn put(s: &Store, account: &str, events: &[CalendarEvent]) {
    s.replace_calendar_events(account, account, events, &CalendarCursor::default())
        .unwrap();
}

fn search(s: &Store, q: &str) -> SearchResponse {
    s.search(&SearchRequest {
        query: q.into(),
        account_id: None,
        account_ids: None,
        limit: 50,
    })
    .unwrap()
}

fn titles(events: &[CalendarEvent]) -> Vec<&str> {
    events.iter().map(|e| e.summary.as_str()).collect()
}

#[test]
fn range_queries_and_account_scope() {
    let s = store();
    let t = now();
    let mut allday = event(ME, "d", "Offsite", t - 2 * HOUR, 24);
    allday.all_day = true;
    put(
        &s,
        ME,
        &[
            event(ME, "a", "Standup", t + HOUR, 1),
            event(ME, "b", "Yesterday thing", t - DAY, 1),
            allday,
        ],
    );
    put(
        &s,
        WORK,
        &[event(WORK, "w", "Work review", t + 2 * HOUR, 1)],
    );
    let today = s.list_events(t - HOUR, t + DAY, None, 100).unwrap();
    // All-day first, then by start; an event that started earlier and is
    // still running overlaps the range.
    assert_eq!(titles(&today), vec!["Offsite", "Standup", "Work review"]);
    let only_work = s
        .list_events(t - HOUR, t + DAY, Some(&[WORK.to_string()]), 100)
        .unwrap();
    assert_eq!(titles(&only_work), vec!["Work review"]);
    // Deselected calendars show nothing.
    s.set_calendar_selected(WORK, WORK, false).unwrap();
    assert_eq!(
        s.list_events(t - HOUR, t + DAY, None, 100).unwrap().len(),
        2
    );
    // Removing an account removes its calendar data.
    s.remove_account(ME).unwrap();
    assert!(s
        .list_events(t - 10 * DAY, t + DAY, None, 100)
        .unwrap()
        .is_empty());
    assert!(s
        .list_calendars(Some(&[ME.to_string()]))
        .unwrap()
        .is_empty());
}

#[test]
fn last_and_next_meeting_with_a_person() {
    let s = store();
    let t = now();
    let with_mike = |id: &str, title: &str, start: i64, response: &str| {
        let mut e = event(ME, id, title, start, 1);
        e.attendees
            .push(person("mike@linden.example", "Mike Delgado", response));
        e
    };
    let mut declined_by_me = with_mike("x", "Skipped", t - DAY, "accepted");
    declined_by_me.my_response = Some("declined".into());
    put(
        &s,
        ME,
        &[
            with_mike("old", "Kickoff", t - 30 * DAY, "accepted"),
            with_mike("recent", "Budget review", t - 3 * DAY, "accepted"),
            with_mike("no", "He declined", t - 2 * DAY, "declined"),
            declined_by_me,
            with_mike("soon", "Q4 planning", t + 2 * DAY, "needsAction"),
            with_mike("later", "Retro", t + 9 * DAY, "accepted"),
            event(ME, "solo", "Focus time", t - HOUR, 1),
        ],
    );
    let m = s.person_meetings("Mike@Linden.example", t).unwrap();
    assert_eq!(
        m.last.as_ref().map(|e| e.summary.as_str()),
        Some("Budget review")
    );
    assert_eq!(
        m.next.as_ref().map(|e| e.summary.as_str()),
        Some("Q4 planning")
    );
    assert_eq!(m.count, 5);
    let none = s.person_meetings("nobody@x.example", t).unwrap();
    assert!(none.last.is_none() && none.next.is_none() && none.count == 0);
    // The account's own address is never "a person you met".
    assert_eq!(s.person_meetings(ME, t).unwrap().count, 0);
}

#[test]
fn conflicts_across_accounts() {
    let s = store();
    let t = now() + 5 * DAY;
    let mut free = event(ME, "f", "Lunch (free)", t, 1);
    free.free = true;
    let mut declined = event(WORK, "d", "Declined sync", t, 1);
    declined.my_response = Some("declined".into());
    put(
        &s,
        ME,
        &[event(ME, "a", "Design review", t + HOUR / 2, 1), free],
    );
    put(
        &s,
        WORK,
        &[
            declined,
            event(WORK, "b", "Vendor call", t - HOUR / 2, 1),
            event(WORK, "c", "Later", t + 3 * HOUR, 1),
        ],
    );
    let c = s.event_conflicts(t, t + HOUR, None, None).unwrap();
    assert_eq!(titles(&c), vec!["Vendor call", "Design review"]);
    // The invite's own event is not a conflict with itself.
    let c = s
        .event_conflicts(t, t + HOUR, Some("a@google.com"), None)
        .unwrap();
    assert_eq!(titles(&c), vec!["Vendor call"]);
}

#[test]
fn incremental_changes_keep_the_index_consistent() {
    let s = store();
    let t = now();
    put(&s, ME, &[event(ME, "a", "Alpha planning", t, 1)]);
    let mut moved = event(ME, "a", "Beta planning", t + DAY, 1);
    moved.location = "Harbor room".into();
    s.apply_calendar_changes(ME, ME, &[moved], &[], &CalendarCursor::default())
        .unwrap();
    assert!(search(&s, "alpha").events.is_empty());
    assert_eq!(titles(&search(&s, "harbor").events), vec!["Beta planning"]);
    s.apply_calendar_changes(ME, ME, &[], &["a".into()], &CalendarCursor::default())
        .unwrap();
    assert!(search(&s, "beta").events.is_empty());
}

#[test]
fn search_finds_events_by_text_people_and_dates() {
    let s = store();
    let t = now();
    let mut board = event(ME, "b", "Board meeting", t + 3 * DAY, 2);
    board.description = "Agenda: runway and hiring plan".into();
    board.location = "Harbor room".into();
    board.organizer = Some(Address {
        email: "priya@contoso.example".into(),
        name: Some("Priya Shah".into()),
    });
    board
        .attendees
        .push(person("mike@linden.example", "Mike Delgado", "accepted"));
    let mut dup = board.clone();
    dup.account_id = WORK.into();
    dup.calendar_id = WORK.into();
    put(
        &s,
        ME,
        &[board, event(ME, "o", "Old board review", t - 400 * DAY, 1)],
    );
    put(&s, WORK, &[dup]);

    let r = search(&s, "board");
    // Same meeting on two calendars → one result; nearer in time first.
    assert_eq!(titles(&r.events), vec!["Board meeting", "Old board review"]);
    assert_eq!(titles(&search(&s, "runway").events), vec!["Board meeting"]);
    assert_eq!(titles(&search(&s, "harbor").events), vec!["Board meeting"]);
    assert_eq!(
        titles(&search(&s, "mike delgado").events),
        vec!["Board meeting"]
    );
    assert_eq!(
        titles(&search(&s, "from:priya").events),
        vec!["Board meeting"]
    );
    assert!(
        search(&s, "from:mike").events.is_empty(),
        "from: is the organizer"
    );
    assert_eq!(titles(&search(&s, "to:mike").events), vec!["Board meeting"]);
    assert_eq!(
        titles(&search(&s, "board -old").events),
        vec!["Board meeting"]
    );
    // Mail-only filters leave the calendar group empty.
    assert!(search(&s, "board is:unread").events.is_empty());
    assert!(search(&s, "board has:pdf").events.is_empty());
    // Dates apply to the event's start.
    let old_year = chrono::DateTime::from_timestamp_millis(t - 400 * DAY)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    let r = search(&s, &format!("board before:{old_year}"));
    assert!(r.events.is_empty() || titles(&r.events) == vec!["Old board review"]);
    let r = search(&s, "board older_than:6m");
    assert_eq!(titles(&r.events), vec!["Old board review"]);
    // account: narrows.
    let r = search(&s, "board account:ada@work");
    assert_eq!(titles(&r.events), vec!["Board meeting"]);
    assert_eq!(r.events[0].account_id, WORK);
}

#[test]
fn type_event_lists_only_events() {
    let s = store();
    let t = now();
    put(
        &s,
        ME,
        &[
            event(ME, "p", "Past thing", t - 2 * DAY, 1),
            event(ME, "n", "Next thing", t + DAY, 1),
            event(ME, "m", "Much later", t + 20 * DAY, 1),
        ],
    );
    let r = search(&s, "type:event");
    assert_eq!(titles(&r.events), vec!["Next thing", "Much later"]);
    assert!(r.hits.is_empty());
    assert_eq!(r.chips[0].kind, "type");
    assert_eq!(
        titles(&search(&s, "is:event thing").events),
        vec!["Next thing", "Past thing"]
    );
    assert_eq!(
        titles(&search(&s, "in:calendar past").events),
        vec!["Past thing"]
    );
    // "type:" with another value is plain text, not a filter.
    assert!(search(&s, "type:banana")
        .chips
        .iter()
        .all(|c| c.kind != "type"));
}

#[test]
fn has_invite_matches_calendar_parts() {
    let s = store();
    let t = now();
    let msg = |id: &str, subject: &str, att: Option<(&str, &str)>| Message {
        account_id: ME.into(),
        id: id.into(),
        thread_id: id.into(),
        date: t - HOUR,
        from: Address {
            name: None,
            email: "mike@linden.example".into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: subject.into(),
        snippet: String::new(),
        body_text: "hello".into(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: att
            .map(|(f, m)| AttachmentMeta {
                id: "a".into(),
                filename: f.into(),
                mime_type: m.into(),
                size: 900,
                content_id: None,
                inline: false,
            })
            .into_iter()
            .collect(),
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    };
    s.upsert_messages(&[
        msg(
            "g",
            "Invitation: Board meeting @ Tue",
            Some(("invite.ics", "application/ics")),
        ),
        msg("o", "Vendor call", Some(("untitled.ics", "text/calendar"))),
        msg("p", "Report", Some(("q3.pdf", "application/pdf"))),
        msg("n", "Plain", None),
        {
            // An invite that also carries an agenda PDF.
            let mut m = msg(
                "b",
                "Offsite agenda",
                Some(("agenda.pdf", "application/pdf")),
            );
            m.attachments.push(AttachmentMeta {
                id: "b2".into(),
                filename: "invite.ics".into(),
                mime_type: "text/calendar".into(),
                size: 800,
                content_id: None,
                inline: false,
            });
            m
        },
    ])
    .unwrap();
    let r = search(&s, "has:invite");
    let mut ids: Vec<&str> = r.hits.iter().map(|h| h.thread_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["b", "g", "o"]);
    assert_eq!(r.chips[0].label, "Has invitation");
    // The attachment panel lists only the calendar files, not the PDF.
    let names: Vec<&str> = r
        .attachments
        .iter()
        .map(|a| a.attachment.filename.as_str())
        .collect();
    assert!(!names.is_empty());
    assert!(names.iter().all(|n| n.ends_with(".ics")), "{names:?}");
    let r = search(&s, "-has:invite report");
    assert_eq!(r.hits.len(), 1);

    // The invite's thread is found for the stored event.
    let board = event(ME, "b", "Board meeting", t + DAY, 1);
    put(&s, ME, std::slice::from_ref(&board));
    let (thread, subject) = s.event_thread(&board).unwrap().unwrap();
    assert_eq!(thread, "g");
    assert!(subject.starts_with("Invitation:"));
    assert!(s
        .event_thread(&event(ME, "z", "Unrelated", t, 1))
        .unwrap()
        .is_none());
}

#[test]
fn events_by_uid_prefers_own_account_and_nearest_instance() {
    let s = store();
    let t = now();
    let mut a = event(ME, "wk_1", "Weekly", t + 7 * DAY, 1);
    let mut b = event(ME, "wk_2", "Weekly", t + 14 * DAY, 1);
    let mut c = event(WORK, "wk_1", "Weekly", t + 7 * DAY, 1);
    for e in [&mut a, &mut b, &mut c] {
        e.ical_uid = Some("wk@google.com".into());
    }
    put(&s, ME, &[a, b]);
    put(&s, WORK, &[c]);
    let r = s
        .events_by_uid("wk@google.com", Some(WORK), Some(t + 13 * DAY))
        .unwrap();
    assert_eq!(r[0].account_id, WORK);
    let r = s
        .events_by_uid("wk@google.com", Some(ME), Some(t + 13 * DAY))
        .unwrap();
    assert_eq!(r[0].id, "wk_2");
    assert!(s.events_by_uid("nope", None, None).unwrap().is_empty());
}

#[test]
fn a_common_word_cannot_crowd_out_the_nearest_event() {
    let s = store();
    let t = now();
    // Far more old matches than the candidate cap, inserted first so they
    // hold the lowest FTS rowids; all equally relevant.
    let old: Vec<CalendarEvent> = (0..600)
        .map(|i| event(ME, &format!("old{i}"), "Team sync", t - (400 + i) * DAY, 1))
        .collect();
    put(&s, ME, &old);
    s.apply_calendar_changes(
        ME,
        ME,
        &[event(ME, "soon", "Team sync", t + DAY, 1)],
        &[],
        &CalendarCursor::default(),
    )
    .unwrap();
    let r = search(&s, "sync");
    assert_eq!(r.events[0].id, "soon", "the next meeting ranks first");

    // And a strong title match far away still beats weak nearby ones.
    let mut near_weak: Vec<CalendarEvent> = (0..300)
        .map(|i| {
            let mut e = event(ME, &format!("w{i}"), "Weekly catch-up", t + i * HOUR, 1);
            e.description = "notes from the quarterly planning session and more".into();
            e
        })
        .collect();
    near_weak.push(event(ME, "strong", "Quarterly planning", t - 300 * DAY, 2));
    put(&s, WORK, &[]);
    put(&s, ME, &near_weak);
    let r = search(&s, "quarterly planning");
    assert!(
        r.events.iter().any(|e| e.id == "strong"),
        "the best title match survives the candidate cap"
    );
}
