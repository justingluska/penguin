use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use penguin_core::{Account, Store};

use super::*;

const ME: &str = "ada@northwind.example";
const DAY: i64 = 86_400_000;
/// 2026-09-24T12:00:00Z
const NOW: i64 = 1_790_251_200_000;

fn window() -> CalendarWindow {
    CalendarWindow {
        start: NOW - 730 * DAY,
        end: NOW + 365 * DAY,
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&Account {
        id: ME.into(),
        email: ME.into(),
        display_name: Some("Ada".into()),
        nickname: None,
        color: "#336699".into(),
        added_at: 0,
        ..Account::default()
    })
    .unwrap();
    s
}

fn cal(id: &str, primary: bool, selected: bool) -> WireCalendar {
    WireCalendar {
        id: id.into(),
        summary: id.into(),
        background_color: Some("#0b8043".into()),
        selected,
        primary,
        access_role: "owner".into(),
        ..Default::default()
    }
}

fn ev(id: &str, summary: &str, start_ms: i64, mins: i64) -> WireEvent {
    WireEvent {
        id: id.into(),
        status: "confirmed".into(),
        summary: Some(summary.into()),
        start: Some(WireTime {
            date_time: Some(rfc3339(start_ms)),
            date: None,
        }),
        end: Some(WireTime {
            date_time: Some(rfc3339(start_ms + mins * 60_000)),
            date: None,
        }),
        ical_uid: Some(format!("{id}@google.com")),
        attendees: vec![
            WirePerson {
                email: ME.into(),
                is_self: true,
                response_status: Some("accepted".into()),
                ..Default::default()
            },
            WirePerson {
                email: "Mike@Linden.example".into(),
                display_name: Some("Mike Delgado".into()),
                response_status: Some("accepted".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn cancelled(id: &str) -> WireEvent {
    WireEvent {
        id: id.into(),
        status: "cancelled".into(),
        ..Default::default()
    }
}

fn page(items: Vec<WireEvent>, sync: Option<&str>, next: Option<&str>) -> Result<EventsPage> {
    Ok(EventsPage {
        items,
        next_page_token: next.map(str::to_string),
        next_sync_token: sync.map(str::to_string),
    })
}

#[derive(Default)]
struct Fake {
    calendars: Vec<WireCalendar>,
    pages: Mutex<HashMap<String, VecDeque<Result<EventsPage>>>>,
    calls: Mutex<Vec<(String, EventsQuery)>>,
    event_json: serde_json::Value,
    patched: Mutex<Option<serde_json::Value>>,
    patched_ids: Mutex<Vec<String>>,
}

impl Fake {
    fn with(calendars: Vec<WireCalendar>) -> Fake {
        Fake {
            calendars,
            ..Default::default()
        }
    }
    fn script(&self, cal: &str, pages: Vec<Result<EventsPage>>) {
        self.pages
            .lock()
            .unwrap()
            .entry(cal.into())
            .or_default()
            .extend(pages);
    }
    fn calls(&self) -> Vec<(String, EventsQuery)> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl CalendarApi for Fake {
    async fn calendar_list(&self, _: Option<&str>) -> Result<CalendarListPage> {
        Ok(CalendarListPage {
            items: self.calendars.clone(),
            next_page_token: None,
        })
    }
    async fn list_events(&self, calendar_id: &str, query: &EventsQuery) -> Result<EventsPage> {
        self.calls
            .lock()
            .unwrap()
            .push((calendar_id.into(), query.clone()));
        self.pages
            .lock()
            .unwrap()
            .get_mut(calendar_id)
            .and_then(|q| q.pop_front())
            .unwrap_or_else(|| panic!("unscripted events.list for {calendar_id}: {query:?}"))
    }
    async fn get_event_json(&self, _: &str, _: &str) -> Result<serde_json::Value> {
        Ok(self.event_json.clone())
    }
    async fn patch_event(&self, _: &str, id: &str, body: &serde_json::Value) -> Result<WireEvent> {
        *self.patched.lock().unwrap() = Some(body.clone());
        self.patched_ids.lock().unwrap().push(id.to_string());
        let mut e = ev(id, "Weekly sync", NOW + DAY, 30);
        e.attendees[0].response_status = Some("declined".into());
        Ok(e)
    }
}

fn all_events(s: &Store) -> Vec<CalendarEvent> {
    s.list_events(NOW - 1000 * DAY, NOW + 1000 * DAY, None, 1000)
        .unwrap()
}

#[tokio::test]
async fn full_then_incremental_with_cancellations() {
    let s = store();
    let fake = Fake::with(vec![
        cal(ME, true, true),
        cal("team@group.example", false, false),
    ]);
    fake.script(
        ME,
        vec![
            page(
                vec![ev("a", "Design review", NOW + DAY, 60)],
                None,
                Some("p2"),
            ),
            page(
                vec![ev("b", "Weekly sync", NOW - DAY, 30)],
                Some("tok1"),
                None,
            ),
        ],
    );
    let r = sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert_eq!((r.calendars, r.full, r.changed), (1, 1, 2));
    let calls = fake.calls();
    assert_eq!(calls.len(), 2, "unselected calendars are never listed");
    assert_eq!(calls[0].1.time_min, Some(window().start));
    assert_eq!(calls[0].1.time_max, Some(window().end));
    assert_eq!(calls[1].1.page_token.as_deref(), Some("p2"));
    assert_eq!(all_events(&s).len(), 2);
    assert_eq!(
        s.calendar_cursor(ME, ME).unwrap().sync_token.as_deref(),
        Some("tok1")
    );
    let cals = s.list_calendars(None).unwrap();
    assert_eq!(cals.len(), 2);
    assert!(cals[0].primary && cals[0].selected && !cals[1].selected);

    // Incremental: one edit, one cancellation, one new event.
    let mut moved = ev("a", "Design review (moved)", NOW + 2 * DAY, 60);
    moved.location = Some("https://acme.zoom.us/j/123456".into());
    fake.script(
        ME,
        vec![page(
            vec![moved, cancelled("b"), ev("c", "Lunch", NOW + 3 * DAY, 60)],
            Some("tok2"),
            None,
        )],
    );
    let r = sync_account(&fake, &s, ME, ME, window(), NOW + 60_000)
        .await
        .unwrap();
    assert_eq!((r.full, r.changed), (0, 3));
    let calls = fake.calls();
    assert_eq!(calls[0].1.sync_token.as_deref(), Some("tok1"));
    assert_eq!(
        calls[0].1.time_min, None,
        "timeMin is incompatible with syncToken"
    );
    let evs = all_events(&s);
    let titles: Vec<&str> = evs.iter().map(|e| e.summary.as_str()).collect();
    assert_eq!(titles, vec!["Design review (moved)", "Lunch"]);
    assert_eq!(evs[0].conference_kind.as_deref(), Some("zoom"));
    assert_eq!(
        s.calendar_cursor(ME, ME).unwrap().sync_token.as_deref(),
        Some("tok2")
    );
}

#[tokio::test]
async fn gone_token_triggers_a_full_resync() {
    let s = store();
    let fake = Fake::with(vec![cal(ME, true, true)]);
    fake.script(
        ME,
        vec![page(
            vec![
                ev("a", "Old", NOW, 30),
                ev("stale", "Deleted while away", NOW, 30),
            ],
            Some("tok1"),
            None,
        )],
    );
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    fake.calls();
    fake.script(
        ME,
        vec![
            Err(Error::Http {
                status: 410,
                body: r#"{"error":{"errors":[{"reason":"fullSyncRequired"}],"code":410}}"#.into(),
            }),
            page(vec![ev("a", "Fresh", NOW, 30)], Some("tok9"), None),
        ],
    );
    let r = sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert_eq!(r.full, 1);
    let calls = fake.calls();
    assert!(calls[0].1.sync_token.is_some() && calls[1].1.sync_token.is_none());
    let evs = all_events(&s);
    assert_eq!(evs.len(), 1, "the full listing replaces everything");
    assert_eq!(evs[0].summary, "Fresh");
    assert_eq!(
        s.calendar_cursor(ME, ME).unwrap().sync_token.as_deref(),
        Some("tok9")
    );
}

#[tokio::test]
async fn generated_calendars_are_relisted_daily_without_sync_tokens() {
    const HOLIDAYS: &str = "en.usa#holiday@group.v.calendar.google.com";
    let s = store();
    let fake = Fake::with(vec![cal(HOLIDAYS, false, true)]);
    fake.script(
        HOLIDAYS,
        vec![page(vec![ev("h1", "Holiday", NOW, 30)], Some("tok1"), None)],
    );
    let r = sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert_eq!((r.full, r.changed), (1, 1));
    fake.calls();

    // Later the same day: no events.list at all (a sync token would 410).
    let r = sync_account(&fake, &s, ME, ME, window(), NOW + 3_600_000)
        .await
        .unwrap();
    assert_eq!((r.full, r.changed), (0, 0));
    assert!(fake.calls().is_empty());

    // A day on: one full listing, never a sync token.
    fake.script(
        HOLIDAYS,
        vec![page(vec![ev("h2", "Holiday 2", NOW, 30)], Some("tok2"), None)],
    );
    let r = sync_account(&fake, &s, ME, ME, window(), NOW + DAY)
        .await
        .unwrap();
    assert_eq!(r.full, 1);
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.sync_token.is_none());
    assert_eq!(all_events(&s)[0].summary, "Holiday 2");
}

#[tokio::test]
async fn other_errors_keep_the_token_and_surface() {
    let s = store();
    let fake = Fake::with(vec![cal(ME, true, true)]);
    fake.script(
        ME,
        vec![page(vec![], Some("tok1"), None), Err(Error::RateLimited)],
    );
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    let err = sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::RateLimited));
    assert_eq!(
        s.calendar_cursor(ME, ME).unwrap().sync_token.as_deref(),
        Some("tok1")
    );
}

#[tokio::test]
async fn recurrence_window_is_enforced_and_slides() {
    let s = store();
    let fake = Fake::with(vec![cal(ME, true, true)]);
    // Instances of a weekly series, as singleEvents=true returns them.
    let series: Vec<WireEvent> = (0..4)
        .map(|i| {
            let mut e = ev(&format!("wk_{i}"), "Weekly sync", NOW + i * 7 * DAY, 30);
            e.recurring_event_id = Some("wk".into());
            e.ical_uid = Some("wk@google.com".into());
            e
        })
        .collect();
    fake.script(ME, vec![page(series, Some("tok1"), None)]);
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert_eq!(all_events(&s).len(), 4);

    // An incremental change that moves an instance past the window's end
    // drops it; an instance outside the window is never stored.
    let mut far = ev("wk_1", "Weekly sync", window().end + DAY, 30);
    far.recurring_event_id = Some("wk".into());
    fake.script(
        ME,
        vec![page(
            vec![far, ev("ancient", "Ancient", window().start - 10 * DAY, 30)],
            Some("tok2"),
            None,
        )],
    );
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    let ids: Vec<String> = all_events(&s).into_iter().map(|e| e.id).collect();
    assert_eq!(ids, vec!["wk_0", "wk_2", "wk_3"]);

    // Cancelling the series parent removes its instances.
    fake.script(ME, vec![page(vec![cancelled("wk")], Some("tok3"), None)]);
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert!(all_events(&s).is_empty());

    // A week later the window's end has slid: full listing again, and
    // events that ended before the new start are pruned.
    fake.calls();
    let later = CalendarWindow {
        start: window().start + 8 * DAY,
        end: window().end + 8 * DAY,
    };
    assert!(needs_full(&s.calendar_cursor(ME, ME).unwrap(), later));
    assert!(!needs_full(&s.calendar_cursor(ME, ME).unwrap(), window()));
    fake.script(ME, vec![page(vec![], Some("tok4"), None)]);
    sync_account(&fake, &s, ME, ME, later, NOW + 8 * DAY)
        .await
        .unwrap();
    assert_eq!(fake.calls()[0].1.time_max, Some(later.end));
}

#[tokio::test]
async fn deselecting_drops_events_and_selecting_resyncs() {
    let s = store();
    let fake = Fake::with(vec![cal(ME, true, true)]);
    fake.script(
        ME,
        vec![page(vec![ev("a", "A", NOW, 30)], Some("tok1"), None)],
    );
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert!(s.set_calendar_selected(ME, ME, false).unwrap());
    assert!(all_events(&s).is_empty());
    assert_eq!(s.calendar_cursor(ME, ME).unwrap().sync_token, None);
    let r = sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert_eq!(r.calendars, 0);
    assert!(s.set_calendar_selected(ME, ME, true).unwrap());
    fake.script(
        ME,
        vec![page(vec![ev("a", "A", NOW, 30)], Some("tok2"), None)],
    );
    let r = sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert_eq!(r.full, 1);
    // A calendar removed from the list goes with its events.
    let gone = Fake::with(vec![]);
    sync_account(&gone, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    assert!(s.list_calendars(None).unwrap().is_empty());
    assert!(all_events(&s).is_empty());
}

#[test]
fn converts_all_day_conference_and_html_description() {
    let w = WireEvent {
        id: "x".into(),
        status: "confirmed".into(),
        summary: Some("  Offsite ".into()),
        description: Some(
            "Agenda:<br><b>Plan</b> <a href=\"https://docs.example/a\">doc</a>".into(),
        ),
        start: Some(WireTime {
            date: Some("2026-10-01".into()),
            date_time: None,
        }),
        end: Some(WireTime {
            date: Some("2026-10-03".into()),
            date_time: None,
        }),
        hangout_link: Some("https://meet.google.com/abc-defg-hij".into()),
        html_link: Some("javascript:alert(1)".into()),
        transparency: Some("transparent".into()),
        organizer: Some(WirePerson {
            email: "Boss@Northwind.example".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let e = to_event(ME, ME, ME, &w).unwrap();
    assert!(e.all_day && e.free);
    assert_eq!(e.summary, "Offsite");
    assert_eq!(e.start_date.as_deref(), Some("2026-10-01"));
    assert_eq!(e.end_date.as_deref(), Some("2026-10-03"));
    assert_eq!(
        e.end - e.start,
        2 * DAY,
        "two local days (no DST change then)"
    );
    assert_eq!(e.conference_kind.as_deref(), Some("meet"));
    assert_eq!(e.html_link, None, "only https links are kept");
    assert!(!e.description.contains('<') && e.description.contains("Plan"));
    assert_eq!(e.organizer.unwrap().email, "boss@northwind.example");
    assert_eq!(e.my_response, None);

    let mut w = ev("y", "Standup", NOW, 15);
    w.description = Some("Join: https://teams.microsoft.com/l/meetup-join/19%3a).".into());
    let e = to_event(ME, ME, ME, &w).unwrap();
    assert_eq!(e.conference_kind.as_deref(), Some("teams"));
    assert_eq!(e.my_response.as_deref(), Some("accepted"));
    assert!(e.attendees[1].email == "mike@linden.example" && !e.attendees[1].is_self);

    assert!(to_event(ME, ME, ME, &cancelled("z")).is_none());
    let mut wl = ev("w", "Home", NOW, 60);
    wl.event_type = Some("workingLocation".into());
    assert!(to_event(ME, ME, ME, &wl).is_none());
}

#[test]
fn conference_links_by_host() {
    assert_eq!(conference_kind("https://us02web.zoom.us/j/1"), Some("zoom"));
    assert_eq!(conference_kind("https://evilzoom.us/j/1"), None);
    assert_eq!(conference_kind("http://zoom.us/j/1"), None);
    assert_eq!(
        find_conference_link(
            "Room 4 / https://docs.example/x then https://acme.webex.com/meet/ada."
        ),
        Some(("https://acme.webex.com/meet/ada".into(), "webex"))
    );
    assert_eq!(find_conference_link("no links"), None);
}

#[test]
fn errors_are_classified() {
    let disabled = Error::Http {
        status: 403,
        body:
            r#"{"error":{"status":"PERMISSION_DENIED","details":[{"reason":"SERVICE_DISABLED"}]}}"#
                .into(),
    };
    assert!(is_api_disabled(&disabled) && !is_not_granted(&disabled));
    let scope = Error::Http {
        status: 403,
        body: "Request had insufficient authentication scopes.".into(),
    };
    assert!(is_not_granted(&scope) && !is_api_disabled(&scope));
    assert!(is_gone(&Error::Http {
        status: 410,
        body: String::new()
    }));
}

#[tokio::test]
async fn rsvp_patches_every_attendee_and_stores_the_result() {
    let s = store();
    let mut fake = Fake::with(vec![cal(ME, true, true)]);
    fake.event_json = serde_json::json!({
        "id": "a",
        "attendees": [
            {"email": "mike@linden.example", "responseStatus": "accepted", "organizer": true, "comment": "keep me"},
            {"email": "ADA@northwind.example", "responseStatus": "needsAction"}
        ]
    });
    fake.script(
        ME,
        vec![page(
            vec![ev("a", "Weekly sync", NOW + DAY, 30)],
            Some("t"),
            None,
        )],
    );
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    let event = s.get_event(ME, ME, "a").unwrap().unwrap();
    assert!(respond(&fake, &s, &event, ME, "maybe").await.is_err());
    let updated = respond(&fake, &s, &event, ME, "declined").await.unwrap();
    let body = fake.patched.lock().unwrap().clone().unwrap();
    let atts = body["attendees"].as_array().unwrap();
    assert_eq!(atts.len(), 2);
    assert_eq!(atts[0]["comment"], "keep me");
    assert_eq!(atts[1]["responseStatus"], "declined");
    assert_eq!(updated.my_response.as_deref(), Some("declined"));
    assert_eq!(
        s.get_event(ME, ME, "a")
            .unwrap()
            .unwrap()
            .my_response
            .as_deref(),
        Some("declined")
    );

    fake.event_json = serde_json::json!({"attendees": [{"email": "x@y.example"}]});
    assert!(respond(&fake, &s, &event, ME, "accepted").await.is_err());
}

#[tokio::test]
async fn rsvp_with_a_note_and_for_a_whole_series() {
    let s = store();
    let mut fake = Fake::with(vec![cal(ME, true, true)]);
    fake.event_json = serde_json::json!({
        "attendees": [
            {"email": "mike@linden.example", "responseStatus": "accepted", "organizer": true},
            {"email": "ada@northwind.example", "self": true, "responseStatus": "needsAction", "comment": "old note"}
        ]
    });
    let mut instance = ev("a_20261001T150000Z", "Weekly sync", NOW + DAY, 30);
    instance.recurring_event_id = Some("a".into());
    fake.script(ME, vec![page(vec![instance], Some("t"), None)]);
    sync_account(&fake, &s, ME, ME, window(), NOW)
        .await
        .unwrap();
    let event = s.get_event(ME, ME, "a_20261001T150000Z").unwrap().unwrap();

    // One instance, with a note: the comment rides on your attendee row.
    let one = respond_with(
        &fake,
        &s,
        &event,
        ME,
        "tentative",
        Some("  Might be late "),
        false,
    )
    .await
    .unwrap();
    assert!(one.is_some());
    let body = fake.patched.lock().unwrap().clone().unwrap();
    assert_eq!(body["attendees"][1]["comment"], "Might be late");
    assert_eq!(body["attendees"][1]["responseStatus"], "tentative");
    assert!(body["attendees"][0].get("comment").is_none());

    // The series: the master event is patched; an empty note clears it.
    let series = respond_with(&fake, &s, &event, ME, "accepted", Some(""), true)
        .await
        .unwrap();
    assert!(series.is_none());
    assert_eq!(
        fake.patched_ids.lock().unwrap().clone(),
        vec!["a_20261001T150000Z".to_string(), "a".to_string()]
    );
    let body = fake.patched.lock().unwrap().clone().unwrap();
    assert!(body["attendees"][1].get("comment").is_none());
}

#[test]
fn window_snaps_to_local_midnight() {
    let w = CalendarWindow::around(NOW, 24, 12);
    let w2 = CalendarWindow::around(NOW + 3_600_000, 24, 12);
    assert!(w.start < NOW - 700 * DAY && w.end > NOW + 360 * DAY);
    assert!(w == w2 || w2.start - w.start == DAY);
}
