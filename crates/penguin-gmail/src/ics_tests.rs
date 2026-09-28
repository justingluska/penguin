use super::*;

const ME: &str = "ada@northwind.example";

/// Shape of Google Calendar's invite.ics (fictional people and domains).
const GOOGLE: &str = "BEGIN:VCALENDAR\r
PRODID:-//Google Inc//Google Calendar 70.9054//EN\r
VERSION:2.0\r
CALSCALE:GREGORIAN\r
METHOD:REQUEST\r
BEGIN:VEVENT\r
DTSTART:20261001T150000Z\r
DTEND:20261001T153000Z\r
DTSTAMP:20260924T120000Z\r
ORGANIZER;CN=Mike Delgado:mailto:mike@linden.example\r
UID:7kq2c9v0m1@google.com\r
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Mike Del\r
 gado;X-NUM-GUESTS=0:mailto:mike@linden.example\r
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=\r
 TRUE;CN=ada@northwind.example;X-NUM-GUESTS=0:mailto:ada@northwind.example\r
ATTENDEE;CUTYPE=ROOM;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=\"Room 4, HQ\":\r
 mailto:c_room4@resource.calendar.example\r
X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r
DESCRIPTION:Quarterly plan\\, part 2.\\nJoin with Google Meet: https://meet.g\r
 oogle.com/abc-defg-hij\r
LOCATION:HQ\\; Room 4\r
SEQUENCE:0\r
STATUS:CONFIRMED\r
SUMMARY:Q4 planning\r
TRANSP:OPAQUE\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:This is an event reminder\r
TRIGGER:-P0DT0H10M0S\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";

/// Outlook style: Windows zone name with its own VTIMEZONE.
const OUTLOOK: &str = "BEGIN:VCALENDAR
METHOD:REQUEST
PRODID:Microsoft Exchange Server 2010
VERSION:2.0
BEGIN:VTIMEZONE
TZID:Eastern Standard Time
BEGIN:STANDARD
DTSTART:16010101T020000
TZOFFSETFROM:-0400
TZOFFSETTO:-0500
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=1SU;BYMONTH=11
END:STANDARD
BEGIN:DAYLIGHT
DTSTART:16010101T020000
TZOFFSETFROM:-0500
TZOFFSETTO:-0400
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=2SU;BYMONTH=3
END:DAYLIGHT
END:VTIMEZONE
BEGIN:VEVENT
ORGANIZER;CN=Priya Shah:mailto:priya@contoso.example
ATTENDEE;ROLE=OPT-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Ada:mailto:ADA@northwind.example
SUMMARY;LANGUAGE=en-US:Vendor call
DTSTART;TZID=Eastern Standard Time:20261015T100000
DTEND;TZID=Eastern Standard Time:20261015T110000
UID:040000008200E00074C5B7101A82E00800000000
LOCATION;LANGUAGE=en-US:https://contoso.zoom.us/j/99999
SEQUENCE:2
END:VEVENT
END:VCALENDAR
";

#[test]
fn google_invite() {
    let inv = parse_invite(GOOGLE, ME).unwrap();
    assert_eq!(inv.method.as_deref(), Some("REQUEST"));
    assert_eq!(inv.uid.as_deref(), Some("7kq2c9v0m1@google.com"));
    assert_eq!(inv.summary, "Q4 planning");
    assert_eq!(inv.location, "HQ; Room 4");
    assert!(inv.description.starts_with("Quarterly plan, part 2.\nJoin"));
    // 2026-10-01T15:00:00Z
    assert_eq!(inv.start, 1_790_866_800_000);
    assert_eq!(inv.end - inv.start, 30 * 60_000);
    assert!(!inv.all_day && !inv.is_cancel() && !inv.recurring);
    let org = inv.organizer.clone().unwrap();
    assert_eq!(
        (org.email.as_str(), org.name.as_deref()),
        ("mike@linden.example", Some("Mike Delgado"))
    );
    assert_eq!(inv.attendees.len(), 3);
    let me = inv.attendees.iter().find(|a| a.is_self).unwrap();
    assert_eq!(me.response, "needsAction");
    assert_eq!(me.name, None, "a CN that is just the address is no name");
    assert!(inv.attendees[0].organizer && inv.attendees[0].name.as_deref() == Some("Mike Delgado"));
    assert!(inv.attendees[2].resource && inv.attendees[2].name.as_deref() == Some("Room 4, HQ"));
    // The VALARM's DESCRIPTION didn't leak into the event.
    assert!(!inv.description.contains("reminder"));
    let e = inv.to_event(ME);
    assert_eq!(e.conference_kind.as_deref(), Some("meet"));
    assert_eq!(e.my_response.as_deref(), Some("needsAction"));
}

#[test]
fn outlook_invite_with_vtimezone() {
    let inv = parse_invite(OUTLOOK, ME).unwrap();
    assert_eq!(inv.summary, "Vendor call");
    // Oct 15 is daylight time (-04:00): 10:00 EDT = 14:00Z.
    assert_eq!(inv.start, 1_792_072_800_000);
    assert_eq!(inv.end - inv.start, 3_600_000);
    assert_eq!(inv.sequence, 2);
    let me = &inv.attendees[0];
    assert!(me.is_self && me.optional && me.email == ME);
    assert_eq!(inv.to_event(ME).conference_kind.as_deref(), Some("zoom"));

    // The same zone in winter is standard time (-05:00).
    let winter = OUTLOOK
        .replace("20261015T100000", "20261210T100000")
        .replace("20261015T110000", "20261210T110000");
    let inv = parse_invite(&winter, ME).unwrap();
    // 2026-12-10T15:00:00Z
    assert_eq!(inv.start, 1_796_914_800_000);
}

#[test]
fn nth_weekday_rules() {
    assert_eq!(
        nth_weekday(2026, 3, 2, Weekday::Sun),
        NaiveDate::from_ymd_opt(2026, 3, 8)
    );
    assert_eq!(
        nth_weekday(2026, 11, 1, Weekday::Sun),
        NaiveDate::from_ymd_opt(2026, 11, 1)
    );
    assert_eq!(
        nth_weekday(2026, 10, -1, Weekday::Sun),
        NaiveDate::from_ymd_opt(2026, 10, 25)
    );
    assert_eq!(nth_weekday(2026, 2, 5, Weekday::Mon), None);
}

#[test]
fn all_day_cancel_and_duration() {
    let ics = "BEGIN:VCALENDAR\nMETHOD:CANCEL\nBEGIN:VEVENT\nUID:x-1\nSUMMARY:Offsite\nDTSTART;VALUE=DATE:20261102\nDTEND;VALUE=DATE:20261104\nSTATUS:CANCELLED\nEND:VEVENT\nEND:VCALENDAR\n";
    let inv = parse_invite(ics, ME).unwrap();
    assert!(inv.all_day && inv.is_cancel());
    assert_eq!(inv.start_date.as_deref(), Some("2026-11-02"));
    assert_eq!(inv.end_date.as_deref(), Some("2026-11-04"));
    assert!(inv.end > inv.start);

    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:d\nDTSTART;TZID=UTC:20261102T090000\nDURATION:PT1H30M\nRRULE:FREQ=WEEKLY\nEND:VEVENT\nEND:VCALENDAR";
    let inv = parse_invite(ics, ME).unwrap();
    assert_eq!(inv.end - inv.start, 90 * 60_000);
    assert!(inv.recurring && inv.method.is_none());
    assert_eq!(parse_duration("-P1W"), Some(-7 * 86_400_000));
    assert_eq!(parse_duration("P1D"), Some(86_400_000));
    assert_eq!(parse_duration("1H"), None);
}

#[test]
fn junk_is_rejected() {
    assert!(parse_invite("", ME).is_none());
    assert!(parse_invite("BEGIN:VCALENDAR\nEND:VCALENDAR", ME).is_none());
    assert!(parse_invite(
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:no start\nEND:VEVENT",
        ME
    )
    .is_none());
    assert!(parse_invite("<html>not a calendar</html>", ME).is_none());
}
