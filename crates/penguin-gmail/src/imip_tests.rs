//! iTIP answers: REPLY / COUNTER generation, escaping and folding, the
//! email around them, and parsing updates and cancellations into snapshots.
//! Fictional people and .example domains.

use super::*;
use crate::ics::parse_invite;

const ME: &str = "sam@northwind.example";

/// Google Calendar's "updated invitation" shape, organizer in New York.
const UPDATED: &str = "BEGIN:VCALENDAR\r
PRODID:-//Google Inc//Google Calendar 70.9054//EN\r
VERSION:2.0\r
CALSCALE:GREGORIAN\r
METHOD:REQUEST\r
BEGIN:VTIMEZONE\r
TZID:America/New_York\r
X-LIC-LOCATION:America/New_York\r
BEGIN:DAYLIGHT\r
TZOFFSETFROM:-0500\r
TZOFFSETTO:-0400\r
TZNAME:EDT\r
DTSTART:19700308T020000\r
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\r
END:DAYLIGHT\r
BEGIN:STANDARD\r
TZOFFSETFROM:-0400\r
TZOFFSETTO:-0500\r
TZNAME:EST\r
DTSTART:19701101T020000\r
RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\r
END:STANDARD\r
END:VTIMEZONE\r
BEGIN:VEVENT\r
DTSTART;TZID=America/New_York:20260929T100000\r
DTEND;TZID=America/New_York:20260929T110000\r
DTSTAMP:20260925T120000Z\r
ORGANIZER;CN=Priya Natarajan:mailto:priya@linden.example\r
UID:0a1b2c3d4e@google.com\r
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Priya\r
  Natarajan;X-NUM-GUESTS=0:mailto:priya@linden.example\r
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=\r
 TRUE;CN=Sam Okafor;X-NUM-GUESTS=0:mailto:Sam@Northwind.example\r
LOCATION:Linden HQ\\, Room 4\r
SEQUENCE:3\r
STATUS:CONFIRMED\r
SUMMARY:Roadmap review\\; Q4\r
END:VEVENT\r
END:VCALENDAR\r
";

fn answer(response: &str) -> Answer {
    Answer {
        response: response.into(),
        comment: None,
        proposal: None,
        attendee: Address {
            name: Some("Sam Okafor".into()),
            email: ME.into(),
        },
        dtstamp_ms: 1_790_000_000_000,
    }
}

/// Unfold and split an iCalendar object into lines.
fn lines(ics: &str) -> Vec<String> {
    ics.replace("\r\n ", "")
        .split("\r\n")
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn reply_echoes_the_event_and_carries_your_answer() {
    let inv = parse_invite(UPDATED, ME).unwrap();
    let mut a = answer("accepted");
    a.comment = Some("Running 5 min late; sorry, all".into());
    let ics = answer_ics(&inv, &a).unwrap();
    // Every line ends in CRLF and none is longer than 75 octets.
    assert!(ics.ends_with("END:VCALENDAR\r\n"));
    assert!(ics.split("\r\n").all(|l| l.len() <= 75));
    let l = lines(&ics);
    assert!(l.contains(&"METHOD:REPLY".to_string()));
    assert!(l.contains(&"UID:0a1b2c3d4e@google.com".to_string()));
    assert!(l.contains(&"SEQUENCE:3".to_string()));
    assert!(l.contains(&"DTSTAMP:20260921T141320Z".to_string()), "{l:?}");
    // Echoed exactly, with the zone it references.
    assert!(l.contains(&"DTSTART;TZID=America/New_York:20260929T100000".to_string()));
    assert!(l.contains(&"DTEND;TZID=America/New_York:20260929T110000".to_string()));
    assert!(l.contains(&"TZID:America/New_York".to_string()));
    assert_eq!(l.iter().filter(|x| *x == "BEGIN:VTIMEZONE").count(), 1);
    assert!(l.contains(&"ORGANIZER;CN=Priya Natarajan:mailto:priya@linden.example".to_string()));
    // Only you, as the invite addressed you (case kept), with your answer.
    let attendees: Vec<&String> = l.iter().filter(|x| x.starts_with("ATTENDEE")).collect();
    assert_eq!(
        attendees,
        vec!["ATTENDEE;PARTSTAT=ACCEPTED;CN=Sam Okafor:mailto:Sam@Northwind.example"]
    );
    // RFC 5545 escaping of the text Penguin writes.
    assert!(l.contains(&"SUMMARY:Roadmap review\\; Q4".to_string()));
    assert!(l.contains(&"COMMENT:Running 5 min late\\; sorry\\, all".to_string()));
    assert!(!l.iter().any(|x| x.starts_with("RECURRENCE-ID")));
}

#[test]
fn counter_proposes_a_new_time_in_utc() {
    let inv = parse_invite(UPDATED, ME).unwrap();
    let mut a = answer("tentative");
    let start = inv.start + 2 * 3_600_000;
    a.proposal = Some((start, start + 3_600_000));
    let l = lines(&answer_ics(&inv, &a).unwrap());
    assert!(l.contains(&"METHOD:COUNTER".to_string()));
    // 10:00 EDT is 14:00Z; two hours later is 16:00Z.
    assert!(l.contains(&"DTSTART:20260929T160000Z".to_string()), "{l:?}");
    assert!(l.contains(&"DTEND:20260929T170000Z".to_string()));
    assert!(!l.iter().any(|x| x.starts_with("DTSTART;TZID")));
    assert!(l
        .iter()
        .any(|x| x.starts_with("ATTENDEE;PARTSTAT=TENTATIVE")));
    // An empty or backwards range is refused.
    a.proposal = Some((start, start));
    assert!(answer_ics(&inv, &a).is_err());
}

#[test]
fn an_instance_answer_keeps_its_recurrence_id() {
    let one = UPDATED.replace(
        "SEQUENCE:3\r\n",
        "SEQUENCE:3\r\nRECURRENCE-ID;TZID=America/New_York:20260929T100000\r\n",
    );
    let inv = parse_invite(&one, ME).unwrap();
    assert!(inv.recurrence_id.is_some());
    let l = lines(&answer_ics(&inv, &answer("declined")).unwrap());
    assert!(l.contains(&"RECURRENCE-ID;TZID=America/New_York:20260929T100000".to_string()));
    assert!(l
        .iter()
        .any(|x| x.starts_with("ATTENDEE;PARTSTAT=DECLINED")));
}

#[test]
fn not_on_the_guest_list_answers_as_the_account() {
    let other = UPDATED.replace("Sam@Northwind.example", "team@northwind.example");
    let inv = parse_invite(&other, ME).unwrap();
    let l = lines(&answer_ics(&inv, &answer("accepted")).unwrap());
    assert!(l.contains(
        &"ATTENDEE;PARTSTAT=ACCEPTED;CN=Sam Okafor:mailto:sam@northwind.example".to_string()
    ));
}

#[test]
fn refuses_what_cannot_be_answered() {
    let no_uid = UPDATED.replace("UID:0a1b2c3d4e@google.com\r\n", "");
    assert!(answer_ics(&parse_invite(&no_uid, ME).unwrap(), &answer("accepted")).is_err());
    let no_org = UPDATED.replace(
        "ORGANIZER;CN=Priya Natarajan:mailto:priya@linden.example\r\n",
        "",
    );
    assert!(answer_ics(&parse_invite(&no_org, ME).unwrap(), &answer("accepted")).is_err());
    let inv = parse_invite(UPDATED, ME).unwrap();
    assert!(answer_ics(&inv, &answer("maybe")).is_err());
}

#[test]
fn escaping_and_folding() {
    assert_eq!(escape_text("a,b;c\\d\r\ne"), "a\\,b\\;c\\\\d\\ne");
    assert_eq!(param_value("Okafor, Sam"), "\"Okafor, Sam\"");
    assert_eq!(param_value("Sam \"the\" Okafor"), "Sam the Okafor");
    // Multi-byte characters are never split by a fold.
    let long = format!("SUMMARY:{}", "é".repeat(60));
    let folded = fold(&long);
    for part in folded.trim_end_matches("\r\n").split("\r\n") {
        assert!(part.len() <= 75);
        assert!(std::str::from_utf8(part.as_bytes()).is_ok());
    }
    assert_eq!(folded.replace("\r\n ", "").trim_end(), long);
}

#[test]
fn the_email_goes_to_the_organizer_as_text_plus_calendar() {
    use mail_parser::{MessageParser, MimeHeaders};
    let inv = parse_invite(UPDATED, ME).unwrap();
    let mut a = answer("declined");
    a.comment = Some("Out that week".into());
    let raw = answer_message(
        &a.attendee,
        inv.organizer.as_ref().unwrap(),
        &inv,
        &a,
        Some("<invite-1@calendar.example>"),
        &[],
    )
    .unwrap();
    let m = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(m.subject(), Some("Declined: Roadmap review; Q4"));
    assert_eq!(
        m.to().unwrap().first().unwrap().address(),
        Some("priya@linden.example")
    );
    assert_eq!(m.in_reply_to().as_text(), Some("invite-1@calendar.example"));
    let text = m.body_text(0).unwrap();
    assert!(text.contains("Sam Okafor has declined this invitation."));
    assert!(text.contains("Note: Out that week"));
    let cal = m
        .parts
        .iter()
        .find(|p| {
            p.content_type()
                .is_some_and(|c| c.subtype() == Some("calendar"))
        })
        .expect("a text/calendar part");
    assert_eq!(
        cal.content_type().unwrap().attribute("method"),
        Some("REPLY")
    );
    let body = std::str::from_utf8(cal.contents()).unwrap();
    assert!(body.contains("METHOD:REPLY"));
    assert!(body.contains("PARTSTAT=DECLINED"));
}

#[test]
fn updates_and_cancellations_as_snapshots() {
    let inv = parse_invite(UPDATED, ME).unwrap();
    let s = inv.to_snapshot();
    assert_eq!(s.method, "request");
    assert_eq!(s.sequence, 3);
    assert_eq!(s.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(s.me().unwrap().response, "needsAction");
    assert_eq!(s.summary, "Roadmap review; Q4");
    assert_eq!(s.location, "Linden HQ, Room 4");

    // The earlier version: an hour earlier, another room.
    let before = UPDATED
        .replace("SEQUENCE:3", "SEQUENCE:1")
        .replace("20260929T100000", "20260929T090000")
        .replace("20260929T110000", "20260929T100000")
        .replace("Room 4", "Room 2");
    let prev = parse_invite(&before, ME).unwrap().to_snapshot();
    assert_eq!(s.changes_since(&prev), vec!["time", "location"]);
    assert_eq!(s.start - prev.start, 3_600_000);

    // METHOD:CANCEL, and STATUS:CANCELLED on its own.
    let cancel = UPDATED.replace("METHOD:REQUEST", "METHOD:CANCEL");
    assert_eq!(
        parse_invite(&cancel, ME).unwrap().to_snapshot().method,
        "cancel"
    );
    let cancelled = UPDATED.replace("STATUS:CONFIRMED", "STATUS:CANCELLED");
    assert_eq!(
        parse_invite(&cancelled, ME).unwrap().method_name(),
        "cancel"
    );

    // A REPLY you sent yourself is not an invitation to show.
    let own = UPDATED
        .replace("METHOD:REQUEST", "METHOD:REPLY")
        .replace(
            "ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Priya\r\n  Natarajan;X-NUM-GUESTS=0:mailto:priya@linden.example\r\n",
            "",
        );
    let own = parse_invite(&own, ME).unwrap();
    assert!(own.is_own_answer());
    // Someone else's reply to your event is.
    let theirs = parse_invite(
        &UPDATED.replace("METHOD:REQUEST", "METHOD:REPLY"),
        "priya@linden.example",
    )
    .unwrap();
    assert!(!theirs.is_own_answer());
}
