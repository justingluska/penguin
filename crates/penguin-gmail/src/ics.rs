//! Calendar invitations in mail: a minimal iCalendar (RFC 5545) reader for
//! the `text/calendar` part of an invite (Google, Outlook, Apple, Zoom…).
//! OWNER: calendar agent.
//!
//! Only the first VEVENT is read: method, UID, title, time, place,
//! organizer and attendees. Times: UTC (`…Z`), floating (read as local),
//! all-day (`VALUE=DATE`), and `TZID=` resolved through the file's own
//! VTIMEZONE (STANDARD/DAYLIGHT observances with yearly BYMONTH/BYDAY
//! rules), else as UTC for `UTC`/`GMT`-style ids, else as local time.

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Weekday};
use penguin_core::{Address, CalendarEvent, EventAttendee, InviteSnapshot};

use crate::calendar::{description_text, find_conference_link, local_midnight_ms};

/// Largest calendar part we parse.
pub const MAX_ICS_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    /// REQUEST (an invitation or update), CANCEL, REPLY, PUBLISH…
    pub method: Option<String>,
    pub uid: Option<String>,
    /// VEVENT STATUS: CONFIRMED | TENTATIVE | CANCELLED.
    pub status: Option<String>,
    pub sequence: i64,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub organizer: Option<Address>,
    pub attendees: Vec<EventAttendee>,
    /// Has an RRULE (a recurring series).
    pub recurring: bool,
    /// RECURRENCE-ID: this invite is about one instance.
    pub recurrence_id: Option<i64>,
    /// Content lines echoed verbatim in a REPLY / COUNTER (see `imip`).
    pub raw: RawInvite,
}

/// The invite's own content lines that an iTIP answer must echo exactly
/// (re-serialized from the parsed properties, values untouched).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawInvite {
    pub uid: Option<String>,
    pub dtstart: Option<String>,
    pub dtend: Option<String>,
    pub duration: Option<String>,
    pub recurrence_id: Option<String>,
    pub organizer: Option<String>,
    /// The account's ATTENDEE address as the invite wrote it (case kept).
    pub self_address: Option<String>,
    pub self_name: Option<String>,
    /// DTSTART's TZID.
    pub tzid: Option<String>,
    /// Every VTIMEZONE block, BEGIN to END.
    pub timezones: Vec<String>,
}

impl Invite {
    pub fn is_cancel(&self) -> bool {
        self.method.as_deref() == Some("CANCEL") || self.status.as_deref() == Some("CANCELLED")
    }

    /// request | cancel | reply | publish | counter | other. No METHOD
    /// (a bare .ics file) reads as a request.
    pub fn method_name(&self) -> &'static str {
        if self.is_cancel() {
            return "cancel";
        }
        match self.method.as_deref() {
            Some("REQUEST") | None => "request",
            Some("REPLY") => "reply",
            Some("PUBLISH") => "publish",
            Some("COUNTER") => "counter",
            _ => "other",
        }
    }

    /// Your own REPLY/COUNTER (e.g. the copy of one Penguin sent): not an
    /// invitation to show.
    pub fn is_own_answer(&self) -> bool {
        matches!(self.method_name(), "reply" | "counter")
            && !self.attendees.is_empty()
            && self.attendees.iter().all(|a| a.is_self)
    }

    /// What the store keeps for the row chip, the card and diffs.
    pub fn to_snapshot(&self) -> InviteSnapshot {
        InviteSnapshot {
            method: self.method_name().to_string(),
            uid: self.uid.clone(),
            sequence: self.sequence,
            recurrence_id: self.recurrence_id,
            recurring: self.recurring,
            summary: self.summary.clone(),
            location: self.location.clone(),
            start: self.start,
            end: self.end,
            all_day: self.all_day,
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
            time_zone: self.raw.tzid.clone().filter(|z| !is_utc_id(z)),
            organizer: self.organizer.clone(),
            attendees: self.attendees.clone(),
        }
    }

    /// As a stored-event shape for the invite card (no calendar, no id).
    pub fn to_event(&self, account_id: &str) -> CalendarEvent {
        let conference = find_conference_link(&self.location)
            .or_else(|| find_conference_link(&self.description));
        CalendarEvent {
            account_id: account_id.to_string(),
            calendar_id: String::new(),
            id: String::new(),
            ical_uid: self.uid.clone(),
            status: match self.status.as_deref() {
                Some("TENTATIVE") => "tentative".into(),
                _ => "confirmed".into(),
            },
            summary: self.summary.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            start: self.start,
            end: self.end,
            all_day: self.all_day,
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
            organizer: self.organizer.clone(),
            attendees: self.attendees.clone(),
            my_response: self
                .attendees
                .iter()
                .find(|a| a.is_self)
                .map(|a| a.response.clone()),
            html_link: None,
            conference_url: conference.as_ref().map(|c| c.0.clone()),
            conference_kind: conference.map(|c| c.1.to_string()),
            recurring_event_id: None,
            free: false,
            updated: 0,
        }
    }
}

/// One content line: NAME;PARAM=V;…:VALUE
#[derive(Debug)]
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

impl Prop {
    /// Back to one (unfolded) content line; parameter values that need it
    /// are quoted again.
    fn to_line(&self) -> String {
        let mut out = self.name.clone();
        for (k, v) in &self.params {
            out.push(';');
            out.push_str(k);
            out.push('=');
            if v.contains([':', ';', ',']) {
                out.push('"');
                out.push_str(&v.replace('"', ""));
                out.push('"');
            } else {
                out.push_str(v);
            }
        }
        out.push(':');
        out.push_str(&self.value);
        out
    }
}

fn unfold(ics: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

fn parse_line(line: &str) -> Option<Prop> {
    // The value starts at the first ':' outside a quoted parameter value.
    let mut in_quote = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = split_unquoted(head, ';').into_iter();
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((
                k.trim().to_ascii_uppercase(),
                v.trim().trim_matches('"').to_string(),
            ))
        })
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

fn split_unquoted(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_quote = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
        } else if c == sep && !in_quote {
            out.push(&s[start..i]);
            start = i + 1;
        }
    }
    out.push(&s[start..]);
    out
}

/// TEXT value escapes: \n \N \, \; \\
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn mailto(v: &str) -> String {
    let v = v.trim();
    let v = if v.len() >= 7 && v[..7].eq_ignore_ascii_case("mailto:") {
        &v[7..]
    } else {
        v
    };
    v.trim().to_lowercase()
}

fn partstat(p: Option<&str>) -> String {
    match p.map(|s| s.to_ascii_uppercase()).as_deref() {
        Some("ACCEPTED") => "accepted",
        Some("DECLINED") => "declined",
        Some("TENTATIVE") => "tentative",
        _ => "needsAction",
    }
    .into()
}

// ---------- time zones ----------

/// One STANDARD or DAYLIGHT block of a VTIMEZONE.
#[derive(Debug, Clone, Default)]
struct Observance {
    /// Local wall time the observance first starts.
    start: Option<NaiveDateTime>,
    offset_to_secs: i32,
    /// Yearly rule: month and (nth, weekday), nth < 0 counts from the end.
    by_month: Option<u32>,
    by_day: Option<(i32, Weekday)>,
}

#[derive(Debug, Clone, Default)]
struct VTimezone {
    id: String,
    observances: Vec<Observance>,
}

fn parse_offset(v: &str) -> Option<i32> {
    let v = v.trim();
    let (sign, rest) = match v.as_bytes().first()? {
        b'+' => (1, &v[1..]),
        b'-' => (-1, &v[1..]),
        _ => (1, v),
    };
    if rest.len() < 4 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i32 = rest[0..2].parse().ok()?;
    let m: i32 = rest[2..4].parse().ok()?;
    let s: i32 = rest.get(4..6).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(sign * (h * 3600 + m * 60 + s))
}

fn weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

fn parse_rrule(v: &str, o: &mut Observance) {
    for part in v.split(';') {
        let Some((k, val)) = part.split_once('=') else {
            continue;
        };
        match k.to_ascii_uppercase().as_str() {
            "BYMONTH" => o.by_month = val.split(',').next().and_then(|m| m.parse().ok()),
            "BYDAY" => {
                let d = val.split(',').next().unwrap_or("").to_ascii_uppercase();
                let split = d.len().saturating_sub(2);
                let (n, wd) = d.split_at(split);
                let nth = if n.is_empty() {
                    1
                } else {
                    n.parse().unwrap_or(1)
                };
                o.by_day = weekday(wd).map(|w| (nth, w));
            }
            _ => {}
        }
    }
}

/// The nth weekday of a month (nth < 0: from the end).
fn nth_weekday(year: i32, month: u32, nth: i32, wd: Weekday) -> Option<NaiveDate> {
    if nth > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let shift = (7 + wd.num_days_from_monday() as i64
            - first.weekday().num_days_from_monday() as i64)
            % 7;
        first
            .checked_add_signed(chrono::Duration::days(shift + 7 * (nth as i64 - 1)))
            .filter(|d| d.month() == month)
    } else {
        let next = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)?
        };
        let last = next.pred_opt()?;
        let shift = (7 + last.weekday().num_days_from_monday() as i64
            - wd.num_days_from_monday() as i64)
            % 7;
        last.checked_sub_signed(chrono::Duration::days(shift + 7 * (-nth as i64 - 1)))
            .filter(|d| d.month() == month)
    }
}

impl Observance {
    /// When this observance last began at or before local time `t`.
    fn onset_before(&self, t: NaiveDateTime) -> Option<NaiveDateTime> {
        let start = self.start?;
        if start > t {
            return None;
        }
        let (Some(month), Some((nth, wd))) = (self.by_month, self.by_day) else {
            return Some(start);
        };
        for year in [t.year(), t.year() - 1] {
            let onset = nth_weekday(year, month, nth, wd)?.and_time(start.time());
            if onset <= t && onset >= start {
                return Some(onset);
            }
        }
        Some(start)
    }
}

impl VTimezone {
    fn offset_at(&self, local: NaiveDateTime) -> Option<i32> {
        self.observances
            .iter()
            .filter_map(|o| o.onset_before(local).map(|t| (t, o.offset_to_secs)))
            .max_by_key(|(t, _)| *t)
            .map(|(_, off)| off)
            .or_else(|| self.observances.first().map(|o| o.offset_to_secs))
    }
}

fn parse_naive(v: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(v.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok()
}

fn is_utc_id(id: &str) -> bool {
    matches!(
        id.to_ascii_uppercase().as_str(),
        "UTC"
            | "GMT"
            | "Z"
            | "ETC/UTC"
            | "ETC/GMT"
            | "UTC+00:00"
            | "(UTC) COORDINATED UNIVERSAL TIME"
    )
}

/// DTSTART/DTEND → (unix ms, all-day date).
fn resolve_time(p: &Prop, zones: &[VTimezone]) -> Option<(i64, Option<String>)> {
    let v = p.value.trim();
    let is_date = p
        .param("VALUE")
        .is_some_and(|x| x.eq_ignore_ascii_case("DATE"))
        || (v.len() == 8 && v.bytes().all(|b| b.is_ascii_digit()));
    if is_date {
        let d = NaiveDate::parse_from_str(&v[..8.min(v.len())], "%Y%m%d").ok()?;
        return Some((local_midnight_ms(d), Some(d.format("%Y-%m-%d").to_string())));
    }
    let naive = parse_naive(v)?;
    if v.ends_with('Z') {
        return Some((naive.and_utc().timestamp_millis(), None));
    }
    if let Some(tzid) = p.param("TZID") {
        if let Some(off) = zones
            .iter()
            .find(|z| z.id == tzid)
            .and_then(|z| z.offset_at(naive))
        {
            return Some((naive.and_utc().timestamp_millis() - off as i64 * 1000, None));
        }
        if is_utc_id(tzid) {
            return Some((naive.and_utc().timestamp_millis(), None));
        }
    }
    // Floating time (or an unknown zone): the reader's local time.
    let local = Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| naive.and_utc().timestamp_millis());
    Some((local, None))
}

/// `-PT1H30M`, `P1D`, `PT45M` → ms (for DURATION in place of DTEND).
fn parse_duration(v: &str) -> Option<i64> {
    let v = v.trim();
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for c in v.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (unit, in_time) {
                    ('W', _) => 7 * 86_400_000,
                    ('D', _) => 86_400_000,
                    ('H', true) => 3_600_000,
                    ('M', true) => 60_000,
                    ('S', true) => 1000,
                    _ => return None,
                };
            }
        }
    }
    Some(if neg { -total } else { total })
}

/// Parse an invitation. `account_email` marks the account's own attendee
/// row. None when there is no usable VEVENT.
pub fn parse_invite(ics: &str, account_email: &str) -> Option<Invite> {
    let me = account_email.trim().to_lowercase();
    let props: Vec<Prop> = unfold(ics).iter().filter_map(|l| parse_line(l)).collect();

    // Pass 1: time zones.
    let mut zones = Vec::new();
    let mut tz: Option<VTimezone> = None;
    let mut obs: Option<Observance> = None;
    let mut tz_blocks: Vec<String> = Vec::new();
    let mut tz_lines: Option<Vec<String>> = None;
    for p in &props {
        let upper_v = p.value.trim().to_ascii_uppercase();
        if p.name == "BEGIN" && upper_v == "VTIMEZONE" {
            tz_lines = Some(Vec::new());
        }
        if let Some(lines) = tz_lines.as_mut() {
            lines.push(p.to_line());
            if p.name == "END" && upper_v == "VTIMEZONE" {
                tz_blocks.push(lines.join("\r\n"));
                tz_lines = None;
            }
        }
        match (
            p.name.as_str(),
            p.value.trim().to_ascii_uppercase().as_str(),
        ) {
            ("BEGIN", "VTIMEZONE") => tz = Some(VTimezone::default()),
            ("END", "VTIMEZONE") => zones.extend(tz.take()),
            ("BEGIN", "STANDARD" | "DAYLIGHT") if tz.is_some() => obs = Some(Observance::default()),
            ("END", "STANDARD" | "DAYLIGHT") => {
                if let (Some(z), Some(o)) = (tz.as_mut(), obs.take()) {
                    z.observances.push(o);
                }
            }
            _ => match (tz.as_mut(), obs.as_mut(), p.name.as_str()) {
                (_, Some(o), "DTSTART") => o.start = parse_naive(&p.value),
                (_, Some(o), "TZOFFSETTO") => {
                    o.offset_to_secs = parse_offset(&p.value).unwrap_or(0)
                }
                (_, Some(o), "RRULE") => parse_rrule(&p.value, o),
                (Some(z), None, "TZID") => z.id = p.value.trim().to_string(),
                _ => {}
            },
        }
    }

    // Pass 2: the first VEVENT.
    let mut method = None;
    let mut depth_event = false;
    let mut nested = 0;
    let mut ev: Vec<&Prop> = Vec::new();
    for p in &props {
        let upper = p.value.trim().to_ascii_uppercase();
        match p.name.as_str() {
            "METHOD" if !depth_event => method = Some(upper),
            "BEGIN" if upper == "VEVENT" => {
                if !ev.is_empty() {
                    break;
                }
                depth_event = true;
            }
            "END" if upper == "VEVENT" => {
                if depth_event {
                    break;
                }
            }
            // VALARM and friends inside the event.
            "BEGIN" if depth_event => nested += 1,
            "END" if depth_event && nested > 0 => nested -= 1,
            _ if depth_event && nested == 0 => ev.push(p),
            _ => {}
        }
    }
    if !depth_event {
        return None;
    }
    let get = |name: &str| ev.iter().find(|p| p.name == name).copied();
    let (start, start_date) = resolve_time(get("DTSTART")?, &zones)?;
    let (end, end_date) = match get("DTEND").and_then(|p| resolve_time(p, &zones)) {
        Some(e) => e,
        None => match get("DURATION").and_then(|p| parse_duration(&p.value)) {
            Some(d) => (start + d, None),
            // All-day with no end lasts the day; timed with no end is instant.
            None if start_date.is_some() => (start + 86_400_000, None),
            None => (start, None),
        },
    };
    let organizer = get("ORGANIZER").map(|p| Address {
        email: mailto(&p.value),
        name: p.param("CN").map(str::to_string).filter(|n| !n.is_empty()),
    });
    let organizer_email = organizer.as_ref().map(|o| o.email.clone());
    let self_prop = ev
        .iter()
        .find(|p| p.name == "ATTENDEE" && mailto(&p.value) == me);
    let line = |name: &str| get(name).map(|p| p.to_line());
    let raw = RawInvite {
        uid: get("UID").map(|p| p.value.trim().to_string()),
        dtstart: line("DTSTART"),
        dtend: line("DTEND"),
        duration: line("DURATION"),
        recurrence_id: line("RECURRENCE-ID"),
        organizer: line("ORGANIZER"),
        self_address: self_prop.map(|p| {
            let v = p.value.trim();
            if v.len() >= 7 && v[..7].eq_ignore_ascii_case("mailto:") {
                v[7..].trim().to_string()
            } else {
                v.to_string()
            }
        }),
        self_name: self_prop
            .and_then(|p| p.param("CN"))
            .map(str::to_string)
            .filter(|n| !n.is_empty()),
        tzid: get("DTSTART")
            .and_then(|p| p.param("TZID"))
            .map(str::to_string),
        timezones: tz_blocks,
    };
    let attendees = ev
        .iter()
        .filter(|p| p.name == "ATTENDEE")
        .map(|p| {
            let email = mailto(&p.value);
            let cutype = p.param("CUTYPE").unwrap_or("").to_ascii_uppercase();
            EventAttendee {
                is_self: email == me,
                organizer: organizer_email.as_deref() == Some(email.as_str()),
                name: p
                    .param("CN")
                    .map(str::to_string)
                    .filter(|n| n != &email && !n.is_empty()),
                response: partstat(p.param("PARTSTAT")),
                optional: p
                    .param("ROLE")
                    .is_some_and(|r| r.eq_ignore_ascii_case("OPT-PARTICIPANT")),
                resource: matches!(cutype.as_str(), "ROOM" | "RESOURCE"),
                email,
            }
        })
        .filter(|a| a.email.contains('@'))
        .collect();
    let text = |name: &str| get(name).map(|p| unescape(&p.value)).unwrap_or_default();
    Some(Invite {
        method,
        uid: get("UID")
            .map(|p| p.value.trim().to_string())
            .filter(|u| !u.is_empty()),
        status: get("STATUS").map(|p| p.value.trim().to_ascii_uppercase()),
        sequence: get("SEQUENCE")
            .and_then(|p| p.value.trim().parse().ok())
            .unwrap_or(0),
        summary: text("SUMMARY").trim().to_string(),
        description: description_text(&text("DESCRIPTION")),
        location: text("LOCATION").trim().to_string(),
        start,
        end: end.max(start),
        all_day: start_date.is_some(),
        start_date,
        end_date,
        organizer,
        attendees,
        recurring: get("RRULE").is_some(),
        recurrence_id: get("RECURRENCE-ID")
            .and_then(|p| resolve_time(p, &zones))
            .map(|t| t.0),
        raw,
    })
}

#[cfg(test)]
#[path = "ics_tests.rs"]
mod tests;
