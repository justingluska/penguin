//! "Send later" and "remind me" times: the preset choices (In 1 hour, This
//! afternoon, Tomorrow morning…) and times typed in words ("tomorrow 9am",
//! "fri 3pm", "in 2 hours", "oct 3 10:30", "10/3 4pm", "tonight"). English,
//! in the caller's time zone. These look ahead; `dates.rs` reads the
//! past-facing date ranges of search.
//!
//! The Mac's composer runs the same rules in TypeScript as you type
//! (apps/desktop/src/features/compose/when.ts); the iPhone asks this
//! module through penguin-ffi. The tests are the same cases.

use chrono::{
    DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike,
};

const MINUTE_MS: i64 = 60_000;

/// One preset choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLaterPreset {
    Hour,
    ThisAfternoon,
    TomorrowMorning,
    TomorrowAfternoon,
    MondayMorning,
}

impl SendLaterPreset {
    pub fn label(self) -> &'static str {
        match self {
            SendLaterPreset::Hour => "In 1 hour",
            SendLaterPreset::ThisAfternoon => "This afternoon",
            SendLaterPreset::TomorrowMorning => "Tomorrow morning",
            SendLaterPreset::TomorrowAfternoon => "Tomorrow afternoon",
            SendLaterPreset::MondayMorning => "Monday morning",
        }
    }
}

/// A local wall-clock time in `tz`; in a daylight-saving gap, the hour after.
fn local<Tz: TimeZone>(tz: &Tz, at: NaiveDateTime) -> Option<DateTime<Tz>> {
    tz.from_local_datetime(&at).earliest().or_else(|| {
        tz.from_local_datetime(&(at + Duration::hours(1)))
            .earliest()
    })
}

fn at_hour<Tz: TimeZone>(now: &DateTime<Tz>, days: i64, hour: u32) -> Option<DateTime<Tz>> {
    let day = now.date_naive() + Duration::days(days);
    local(
        &now.timezone(),
        day.and_time(NaiveTime::from_hms_opt(hour, 0, 0)?),
    )
}

/// Round up to the next multiple of `step_ms`.
fn ceil_ms<Tz: TimeZone>(t: DateTime<Tz>, step_ms: i64) -> DateTime<Tz> {
    let ms = t.timestamp_millis();
    let up = ms.div_euclid(step_ms) * step_ms
        + if ms.rem_euclid(step_ms) > 0 {
            step_ms
        } else {
            0
        };
    t.timezone().timestamp_millis_opt(up).single().unwrap_or(t)
}

/// In 1 hour · This afternoon (5 PM, until 4 PM) · Tomorrow morning ·
/// Tomorrow afternoon (1 PM) · Monday morning (not on a Sunday, when it's
/// tomorrow): Gmail's and Apple Mail's choices. Mornings are at
/// `morning_hour` (Settings → Compose, default 8). In time order.
pub fn send_later_presets<Tz: TimeZone>(
    now: &DateTime<Tz>,
    morning_hour: u32,
) -> Vec<(SendLaterPreset, DateTime<Tz>)> {
    let mut out = vec![(
        SendLaterPreset::Hour,
        ceil_ms(now.clone() + Duration::hours(1), 5 * MINUTE_MS),
    )];
    if now.hour() < 16 {
        out.extend(at_hour(now, 0, 17).map(|t| (SendLaterPreset::ThisAfternoon, t)));
    }
    out.extend(at_hour(now, 1, morning_hour).map(|t| (SendLaterPreset::TomorrowMorning, t)));
    out.extend(at_hour(now, 1, 13).map(|t| (SendLaterPreset::TomorrowAfternoon, t)));
    let dow = now.weekday().num_days_from_sunday() as i64;
    let to_monday = match (8 - dow) % 7 {
        0 => 7,
        n => n,
    };
    if to_monday > 1 {
        out.extend(
            at_hour(now, to_monday, morning_hour).map(|t| (SendLaterPreset::MondayMorning, t)),
        );
    }
    out
}

const WEEKDAYS: [&str; 7] = [
    "sunday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
];
const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// Sunday = 0; None when not a weekday ("thurs", "tues", "weds" too).
fn weekday(word: &str) -> Option<u32> {
    let w = word.strip_suffix('.').unwrap_or(word);
    if w.len() < 2 {
        return None;
    }
    match w {
        "tues" => return Some(2),
        "weds" => return Some(3),
        "thur" | "thurs" => return Some(4),
        _ => {}
    }
    if w.len() < 3 {
        return None;
    }
    WEEKDAYS
        .iter()
        .position(|d| d.starts_with(w))
        .map(|i| i as u32)
}

/// January = 0.
fn month(word: &str) -> Option<u32> {
    let w = word.strip_suffix('.').unwrap_or(word);
    if w.len() < 3 {
        return None;
    }
    if w == "sept" {
        return Some(8);
    }
    MONTHS
        .iter()
        .position(|m| m.starts_with(w))
        .map(|i| i as u32)
}

/// "3", "3rd", "21st": a day of the month.
fn day_number(w: &str) -> Option<u32> {
    let digits = w
        .strip_suffix("st")
        .or_else(|| w.strip_suffix("nd"))
        .or_else(|| w.strip_suffix("rd"))
        .or_else(|| w.strip_suffix("th"))
        .unwrap_or(w);
    ((1..=2).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// "9", "9am", "9:30", "9:30pm", "21:00", "noon", "midnight" → (hour, minute).
fn parse_time(s: &str) -> Option<(u32, u32)> {
    let t: String = s
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '.')
        .collect();
    match t.as_str() {
        "noon" => return Some((12, 0)),
        "midnight" => return Some((0, 0)),
        _ => {}
    }
    let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    if !(1..=2).contains(&digits_end) {
        return None;
    }
    let mut h: u32 = t[..digits_end].parse().ok()?;
    let mut rest = &t[digits_end..];
    let mut minute = None;
    if let Some(m) = rest.strip_prefix(':') {
        if m.len() < 2 || !m.as_bytes()[..2].iter().all(u8::is_ascii_digit) {
            return None;
        }
        minute = Some(m[..2].parse::<u32>().ok()?);
        rest = &m[2..];
    }
    let ap = match rest {
        "" => None,
        "am" | "a" => Some('a'),
        "pm" | "p" => Some('p'),
        _ => return None,
    };
    let min = minute.unwrap_or(0);
    if min > 59 {
        return None;
    }
    match ap {
        Some(ap) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            if ap == 'p' && h < 12 {
                h += 12;
            }
            if ap == 'a' && h == 12 {
                h = 0;
            }
        }
        None if h > 23 => return None,
        // "send at 3" means the afternoon.
        None if minute.is_none() && (1..=7).contains(&h) => h += 12,
        None => {}
    }
    Some((h, min))
}

/// The next `dow` after today (a week on when `extra` is 7).
fn next_weekday(today: NaiveDate, dow: u32, extra: i64) -> NaiveDate {
    let now = today.weekday().num_days_from_sunday() as i64;
    let ahead = match (dow as i64 - now + 7) % 7 {
        0 => 7,
        n => n,
    };
    today + Duration::days(ahead + extra)
}

/// That date this year, or next year once it's past; None if impossible.
fn dated_day(today: NaiveDate, month0: u32, day: u32) -> Option<NaiveDate> {
    let d = NaiveDate::from_ymd_opt(today.year(), month0 + 1, day)?;
    if d < today {
        NaiveDate::from_ymd_opt(today.year() + 1, month0 + 1, day)
    } else {
        Some(d)
    }
}

/// The time `text` means, or None when it can't be read. A day without a
/// time is `morning_hour`; a time without a day is today if still ahead,
/// else tomorrow; a weekday is the next one after today.
pub fn parse_when<Tz: TimeZone>(
    text: &str,
    now: &DateTime<Tz>,
    morning_hour: u32,
) -> Option<DateTime<Tz>> {
    let lowered = text.trim().to_lowercase().replace(',', " ");
    let mut s = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        return None;
    }
    for lead in ["at ", "on "] {
        if let Some(r) = s.strip_prefix(lead) {
            s = r.to_string();
        }
    }

    if let Some(rel) = relative(&s) {
        if rel <= 0 {
            return None;
        }
        return Some(ceil_ms(
            now.clone() + Duration::milliseconds(rel),
            MINUTE_MS,
        ));
    }

    let today = now.date_naive();
    let words: Vec<&str> = s.split(' ').collect();
    let mut day: Option<NaiveDate> = None;
    let mut part: Option<i32> = None;
    let mut rest: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        match w {
            "today" => day = Some(today),
            "tonight" => {
                day = Some(today);
                part = Some(21);
            }
            "tomorrow" | "tmrw" | "tmr" | "tom" => day = Some(today + Duration::days(1)),
            "next" | "this" if next.and_then(weekday).is_some() => {
                let wd = next.and_then(weekday)?;
                let extra = if w == "next" && wd > today.weekday().num_days_from_sunday() {
                    7
                } else {
                    0
                };
                day = Some(next_weekday(today, wd, extra));
                i += 1;
            }
            "next" if next == Some("week") => {
                day = Some(next_weekday(today, 1, 0));
                i += 1;
            }
            _ if weekday(w).is_some() => day = Some(next_weekday(today, weekday(w)?, 0)),
            _ if month(w).is_some() && next.and_then(day_number).is_some() => {
                day = Some(dated_day(today, month(w)?, next.and_then(day_number)?)?);
                i += 1;
            }
            _ if day_number(w).is_some() && next.and_then(month).is_some() => {
                day = Some(dated_day(today, next.and_then(month)?, day_number(w)?)?);
                i += 1;
            }
            _ if is_month_slash_day(w) => {
                let (mo, d) = w.split_once('/')?;
                let (mo, d): (u32, u32) = (mo.parse().ok()?, d.parse().ok()?);
                if mo == 0 {
                    return None;
                }
                day = Some(dated_day(today, mo - 1, d)?);
            }
            "morning" => part = Some(-1),
            "afternoon" => part = Some(13),
            "evening" => part = Some(18),
            "night" => part = Some(21),
            "at" => {}
            _ => rest.push(w),
        }
        i += 1;
    }

    let time = if rest.is_empty() {
        None
    } else {
        // What's left must be one time ("3pm", "3 pm", "9:30").
        Some(parse_time(&rest.concat())?)
    };
    if day.is_none() && time.is_none() && part.is_none() {
        return None;
    }
    let (hour, minute) = match (time, part) {
        (Some(t), _) => t,
        (None, Some(-1)) | (None, None) => (morning_hour, 0),
        (None, Some(p)) => (p as u32, 0),
    };
    let base = day.unwrap_or(today);
    let tz = now.timezone();
    let mut at = local(
        &tz,
        base.and_time(NaiveTime::from_hms_opt(hour, minute, 0)?),
    )?;
    if day.is_none() && at <= *now {
        at = local(
            &tz,
            (base + Duration::days(1)).and_time(at.naive_local().time()),
        )?;
    }
    Some(at)
}

fn is_month_slash_day(w: &str) -> bool {
    let Some((a, b)) = w.split_once('/') else {
        return false;
    };
    let ok = |s: &str| (1..=2).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_digit());
    ok(a) && ok(b)
}

/// "in 2 hours", "in 30 min", "in a day", "in half an hour" → ms.
fn relative(s: &str) -> Option<i64> {
    let rest = s.strip_prefix("in ")?;
    let (count, unit_text) = if let Some(u) = rest.strip_prefix("half an") {
        (0.5, u)
    } else if let Some(u) = rest.strip_prefix("half a") {
        (0.5, u)
    } else {
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        let word = &rest[..end];
        let digits = word
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(word.len());
        if digits > 0 {
            (rest[..digits].parse::<f64>().ok()?, &rest[digits..])
        } else {
            let n = match word {
                "a" | "an" | "one" => 1.0,
                _ => return None,
            };
            (n, &rest[end..])
        }
    };
    let unit = unit_text.strip_prefix(' ').unwrap_or(unit_text);
    let ms = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => MINUTE_MS,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * MINUTE_MS,
        "d" | "day" | "days" => 24 * 60 * MINUTE_MS,
        "w" | "wk" | "wks" | "week" | "weeks" => 7 * 24 * 60 * MINUTE_MS,
        _ => return None,
    };
    Some((count * ms as f64) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn wed() -> DateTime<FixedOffset> {
        // Wed Sep 23 2026, 10:07, UTC-4.
        FixedOffset::west_opt(4 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 9, 23, 10, 7, 0)
            .unwrap()
    }

    fn parts(t: Option<DateTime<FixedOffset>>) -> Option<(i32, u32, u32, u32, u32)> {
        t.map(|t| (t.year(), t.month(), t.day(), t.hour(), t.minute()))
    }

    #[test]
    fn presets_follow_the_clock_and_the_morning_hour() {
        let p = send_later_presets(&wed(), 9);
        let ids: Vec<SendLaterPreset> = p.iter().map(|x| x.0).collect();
        assert_eq!(
            ids,
            [
                SendLaterPreset::Hour,
                SendLaterPreset::ThisAfternoon,
                SendLaterPreset::TomorrowMorning,
                SendLaterPreset::TomorrowAfternoon,
                SendLaterPreset::MondayMorning
            ]
        );
        assert_eq!((p[0].1.hour(), p[0].1.minute()), (11, 10));
        assert_eq!(p[2].1.hour(), 9);
        assert_eq!(p[4].1.weekday().num_days_from_sunday(), 1);
        let tz = wed().timezone();
        let late = tz.with_ymd_and_hms(2026, 9, 23, 17, 0, 0).unwrap();
        assert!(!send_later_presets(&late, 8)
            .iter()
            .any(|x| x.0 == SendLaterPreset::ThisAfternoon));
        let sunday = tz.with_ymd_and_hms(2026, 9, 27, 9, 0, 0).unwrap();
        assert!(!send_later_presets(&sunday, 8)
            .iter()
            .any(|x| x.0 == SendLaterPreset::MondayMorning));
    }

    #[test]
    fn send_times_typed_in_words() {
        let now = wed();
        let at = |s: &str| parts(parse_when(s, &now, 8));
        assert_eq!(at("tomorrow 9am"), Some((2026, 9, 24, 9, 0)));
        assert_eq!(at("tomorrow"), Some((2026, 9, 24, 8, 0)));
        assert_eq!(at("tomorrow afternoon"), Some((2026, 9, 24, 13, 0)));
        assert_eq!(at("fri 3pm"), Some((2026, 9, 25, 15, 0)));
        assert_eq!(at("Friday at 3:30 pm"), Some((2026, 9, 25, 15, 30)));
        assert_eq!(at("wed 9"), Some((2026, 9, 30, 9, 0)), "the next Wednesday");
        assert_eq!(at("monday"), Some((2026, 9, 28, 8, 0)));
        assert_eq!(at("next week"), Some((2026, 9, 28, 8, 0)));
        assert_eq!(at("3pm"), Some((2026, 9, 23, 15, 0)));
        assert_eq!(at("9am"), Some((2026, 9, 24, 9, 0)), "already past today");
        assert_eq!(at("at 3"), Some((2026, 9, 23, 15, 0)));
        assert_eq!(at("tonight"), Some((2026, 9, 23, 21, 0)));
        assert_eq!(at("oct 3 10:30"), Some((2026, 10, 3, 10, 30)));
        assert_eq!(at("3 october"), Some((2026, 10, 3, 8, 0)));
        assert_eq!(
            at("9/1"),
            Some((2027, 9, 1, 8, 0)),
            "past this year: next year"
        );
        assert_eq!(at("in 2 hours"), Some((2026, 9, 23, 12, 7)));
        assert_eq!(at("in 30 min"), Some((2026, 9, 23, 10, 37)));
        assert_eq!(at("in a day"), Some((2026, 9, 24, 10, 7)));
        assert_eq!(at("in 2h"), Some((2026, 9, 23, 12, 7)));
        assert_eq!(at("in half an hour"), Some((2026, 9, 23, 10, 37)));
        assert_eq!(at("next fri 9:15am"), Some((2026, 10, 2, 9, 15)));
        assert_eq!(at("noon"), Some((2026, 9, 23, 12, 0)));
        assert_eq!(at(""), None);
        assert_eq!(at("whenever"), None);
        assert_eq!(at("feb 30"), None);
        assert_eq!(at("25pm"), None);
        assert_eq!(at("in 0 hours"), None);
    }
}
