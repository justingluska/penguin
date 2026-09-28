//! Answering an invitation by email: iTIP (RFC 5546) over iMIP (RFC 6047).
//! OWNER: calendar agent.
//!
//! A REPLY carries your PARTSTAT (and an optional COMMENT) for the event's
//! UID/SEQUENCE/RECURRENCE-ID, sent to the ORGANIZER as a `text/calendar;
//! method=REPLY` part. A COUNTER proposes a new DTSTART/DTEND the same way.
//! Google Calendar, Outlook/Exchange, iCloud and Fastmail all apply an
//! incoming REPLY to the organizer's copy of the event; a COUNTER shows as a
//! "proposed new time" in Outlook and Google Calendar and as a plain email
//! (the text part says what was proposed) elsewhere.
//!
//! The properties that identify the event (UID, RECURRENCE-ID, DTSTART,
//! DTEND/DURATION, ORGANIZER, and the VTIMEZONEs they reference) are
//! echoed from the invite exactly as parsed (`ics::RawInvite`); everything
//! Penguin writes is escaped (RFC 5545 §3.3.11) and folded at 75 octets.

use chrono::{DateTime, Local, TimeZone, Utc};
use mail_builder::headers::address::Address as MbAddress;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::date::Date;
use mail_builder::mime::{BodyPart, MimePart};
use mail_builder::MessageBuilder;
use penguin_core::Address;

use crate::ics::Invite;

pub const PRODID: &str = "-//Penguin//Penguin Mail//EN";

/// What you are sending.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// accepted | tentative | declined
    pub response: String,
    /// A note to the organizer (COMMENT).
    pub comment: Option<String>,
    /// A proposed new time (unix ms): makes it a COUNTER.
    pub proposal: Option<(i64, i64)>,
    /// You, as the attendee answering.
    pub attendee: Address,
    /// DTSTAMP (unix ms): when the answer was made.
    pub dtstamp_ms: i64,
}

impl Answer {
    pub fn method(&self) -> &'static str {
        if self.proposal.is_some() {
            "COUNTER"
        } else {
            "REPLY"
        }
    }
}

fn partstat(r: &str) -> Option<&'static str> {
    Some(match r {
        "accepted" => "ACCEPTED",
        "tentative" => "TENTATIVE",
        "declined" => "DECLINED",
        _ => return None,
    })
}

/// RFC 5545 TEXT escaping: backslash, semicolon, comma, newline.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c if c.is_control() && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// A parameter value: quoted when it holds `:` `;` `,`; DQUOTE and
/// control characters can't appear in one at all (RFC 5545 §3.1).
pub fn param_value(s: &str) -> String {
    let clean: String = s.chars().filter(|c| *c != '"' && !c.is_control()).collect();
    if clean.contains([':', ';', ',']) {
        format!("\"{clean}\"")
    } else {
        clean
    }
}

/// Fold one content line at 75 octets (never inside a UTF-8 sequence) and
/// end it with CRLF.
pub fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3 + 2);
    let mut width = 0;
    for c in line.chars() {
        let n = c.len_utf8();
        if width + n > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += n;
    }
    out.push_str("\r\n");
    out
}

fn utc_stamp(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

/// The iCalendar object for a REPLY (or, with a proposal, a COUNTER).
/// Errors when the invite lacks what an answer must echo.
pub fn answer_ics(invite: &Invite, answer: &Answer) -> Result<String, String> {
    let partstat = partstat(&answer.response)
        .ok_or_else(|| format!("unknown response {}", answer.response))?;
    let uid = invite
        .raw
        .uid
        .as_deref()
        .filter(|u| !u.is_empty())
        .ok_or("the invitation has no UID")?;
    let organizer = invite
        .raw
        .organizer
        .as_deref()
        .ok_or("the invitation has no organizer to answer")?;
    let mut lines: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        format!("PRODID:{PRODID}"),
        "VERSION:2.0".into(),
        "CALSCALE:GREGORIAN".into(),
        format!("METHOD:{}", answer.method()),
    ];
    for tz in &invite.raw.timezones {
        lines.extend(tz.split("\r\n").map(str::to_string));
    }
    lines.push("BEGIN:VEVENT".into());
    lines.push(format!("UID:{uid}"));
    lines.push(format!("DTSTAMP:{}", utc_stamp(answer.dtstamp_ms)));
    lines.push(format!("SEQUENCE:{}", invite.sequence.max(0)));
    if let Some(r) = &invite.raw.recurrence_id {
        lines.push(r.clone());
    }
    match answer.proposal {
        Some((start, end)) => {
            if end <= start {
                return Err("the proposed time must end after it starts".into());
            }
            lines.push(format!("DTSTART:{}", utc_stamp(start)));
            lines.push(format!("DTEND:{}", utc_stamp(end)));
        }
        None => {
            if let Some(s) = &invite.raw.dtstart {
                lines.push(s.clone());
            }
            if let Some(e) = invite.raw.dtend.as_ref().or(invite.raw.duration.as_ref()) {
                lines.push(e.clone());
            }
        }
    }
    if !invite.summary.is_empty() {
        lines.push(format!("SUMMARY:{}", escape_text(&invite.summary)));
    }
    lines.push(organizer.to_string());
    let address = invite
        .raw
        .self_address
        .clone()
        .unwrap_or_else(|| answer.attendee.email.clone());
    let name = invite
        .raw
        .self_name
        .clone()
        .or_else(|| answer.attendee.name.clone())
        .filter(|n| !n.trim().is_empty());
    let mut attendee = format!("ATTENDEE;PARTSTAT={partstat}");
    if let Some(n) = name {
        attendee.push_str(";CN=");
        attendee.push_str(&param_value(&n));
    }
    attendee.push_str(":mailto:");
    attendee.push_str(address.trim());
    lines.push(attendee);
    if let Some(c) = answer
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        lines.push(format!("COMMENT:{}", escape_text(c)));
    }
    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    Ok(lines.iter().map(|l| fold(l)).collect())
}

fn verb(response: &str) -> &'static str {
    match response {
        "accepted" => "Accepted",
        "tentative" => "Tentatively accepted",
        _ => "Declined",
    }
}

/// "Tue Sep 29, 2026, 10:00 (UTC-04:00)" in this machine's zone.
fn when(ms: i64) -> String {
    let t: DateTime<Local> = Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Local::now);
    t.format("%a %b %-d, %Y, %H:%M (UTC%:z)").to_string()
}

/// Subject and plain-text body of the answer email.
pub fn answer_text(invite: &Invite, answer: &Answer) -> (String, String) {
    let who = answer
        .attendee
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| answer.attendee.email.clone());
    let title = if invite.summary.is_empty() {
        "(no title)".to_string()
    } else {
        invite.summary.clone()
    };
    let (subject, mut body) = match answer.proposal {
        Some((start, end)) => (
            format!("New time proposed: {title}"),
            format!(
                "{who} proposed a new time for \"{title}\":\n{} to {}\n\nTheir answer: {}.",
                when(start),
                when(end),
                verb(&answer.response).to_lowercase()
            ),
        ),
        None => (
            format!("{}: {title}", verb(&answer.response)),
            format!(
                "{who} has {} this invitation.",
                match answer.response.as_str() {
                    "accepted" => "accepted",
                    "tentative" => "tentatively accepted",
                    _ => "declined",
                }
            ),
        ),
    };
    if let Some(c) = answer
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        body.push_str("\n\nNote: ");
        body.push_str(c);
    }
    body.push('\n');
    (subject, body)
}

fn bare_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// The whole email: text, then the calendar part, as
/// `multipart/alternative`, to the organizer, threaded under the invite.
pub fn answer_message(
    from: &Address,
    to: &Address,
    invite: &Invite,
    answer: &Answer,
    in_reply_to: Option<&str>,
    references: &[String],
) -> Result<Vec<u8>, String> {
    let ics = answer_ics(invite, answer)?;
    let (subject, text) = answer_text(invite, answer);
    let domain = from
        .email
        .rsplit_once('@')
        .map(|(_, d)| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "penguin.invalid".into());
    let mut builder = MessageBuilder::new()
        .from(MbAddress::new_address(
            from.name.as_deref(),
            from.email.trim(),
        ))
        .to(MbAddress::new_address(to.name.as_deref(), to.email.trim()))
        .subject(subject)
        .date(Date::new(answer.dtstamp_ms / 1000))
        .message_id(format!(
            "{:x}.{:016x}@{domain}",
            answer.dtstamp_ms,
            fastrand::u64(..)
        ));
    let parent = in_reply_to.map(bare_id).filter(|p| !p.is_empty());
    let mut refs: Vec<String> = references
        .iter()
        .map(|r| bare_id(r))
        .filter(|r| !r.is_empty())
        .collect();
    if let Some(p) = parent {
        if refs.last() != Some(&p) {
            refs.retain(|r| r != &p);
            refs.push(p.clone());
        }
        builder = builder.in_reply_to(p);
    }
    if !refs.is_empty() {
        builder = builder.references(refs);
    }
    let calendar = MimePart::new(
        ContentType::new("text/calendar")
            .attribute("method", answer.method())
            .attribute("charset", "utf-8"),
        BodyPart::Text(ics.into()),
    );
    let text = MimePart::new(
        ContentType::new("text/plain").attribute("charset", "utf-8"),
        BodyPart::Text(text.into()),
    );
    builder = builder.body(MimePart::new(
        "multipart/alternative",
        BodyPart::Multipart(vec![text, calendar]),
    ));
    builder.write_to_vec().map_err(|e| format!("compose: {e}"))
}

#[cfg(test)]
#[path = "imip_tests.rs"]
mod tests;
