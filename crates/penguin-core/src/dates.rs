//! Date expressions for the `date:` search operator (`date:february`,
//! `date:"late march"`, `date:"since feb 10"`, `date:"between nov and feb"`,
//! `date:"the week of mar 2"`, `date:tuesday`, `date:2026-02`). Free text is
//! never parsed as a date: "february" searches the word.
//!
//! Operates on lowercased, whitespace-separated words and resolves against a
//! local calendar date. The result is a half-open range of local dates,
//! either end of which may be open. Converting to unix ms (with the user's
//! timezone) is the caller's job.
//!
//! Month names, weekdays and "N days ago" style forms always refer to the
//! most recent past occurrence: in September 2026, "february" is Feb 2026 and
//! "october" is Oct 2025. "last <month>" is the occurrence before that.

use chrono::{Datelike, Duration, Months, NaiveDate, Weekday};

/// A half-open local-date range `[from, to)`; `None` = unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DateRange {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// Parse a date expression at the start of `words` (lowercased,
/// whitespace-split). Returns how many words it consumed and the range; a
/// caller parsing a whole value should require all words to be consumed.
pub(crate) fn parse(words: &[&str], today: NaiveDate) -> Option<(usize, DateRange)> {
    // Numbers in words and abbreviations with a period ("two weeks ago",
    // "a couple of weeks ago", "aug. 18"); the count maps back to the words
    // as typed.
    let (norm, ends) = normalize(words);
    let refs: Vec<&str> = norm.iter().map(String::as_str).collect();
    let (n, r) = parse_normalized(&refs, today)?;
    Some((ends[n - 1], r))
}

const NUMBER_WORDS: [&str; 12] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
    "twelve",
];

/// Words as `parse_normalized` reads them, with, for each, how many of the
/// original words it (and everything before it) used.
fn normalize(words: &[&str]) -> (Vec<String>, Vec<usize>) {
    let mut out = Vec::with_capacity(words.len());
    let mut ends = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        let after = words.get(i + 2).copied();
        let (value, used) = match (w, next, after) {
            ("a" | "an", Some("couple"), Some("of")) => ("2".to_string(), 3),
            ("a" | "an", Some("couple"), _) | ("couple", Some("of"), _) => ("2".to_string(), 2),
            ("a", Some("few"), _) => ("3".to_string(), 2),
            // "a week ago", "an hour ago"
            ("a" | "an", Some(n), _) if unit_word(n).is_some() => ("1".to_string(), 1),
            _ => match NUMBER_WORDS.iter().position(|x| *x == w) {
                Some(k) if next.is_some_and(|n| unit_word(n).is_some()) => ((k + 1).to_string(), 1),
                _ => {
                    // "aug." / "sept." → "aug"; digits keep their dots (2026.08.18).
                    let t = w.strip_suffix('.').filter(|s| s.chars().all(char::is_alphabetic));
                    (t.unwrap_or(w).to_string(), 1)
                }
            },
        };
        i += used;
        out.push(value);
        ends.push(i);
    }
    (out, ends)
}

fn parse_normalized(words: &[&str], today: NaiveDate) -> Option<(usize, DateRange)> {
    let first = *words.first()?;
    match first {
        "since" | "from" => {
            // "from jan 5 to jan 20" is a range; "since february" is open.
            if let Some((n, range)) = span(&words[1..], today) {
                return Some((n + 1, range));
            }
            let (n, a) = atom(&words[1..], today)?;
            return Some((
                n + 1,
                DateRange {
                    from: Some(a.0),
                    to: None,
                },
            ));
        }
        "after" => {
            let (n, a) = atom(&words[1..], today)?;
            return Some((
                n + 1,
                DateRange {
                    from: Some(a.1),
                    to: None,
                },
            ));
        }
        "before" => {
            let (n, a) = atom(&words[1..], today)?;
            return Some((
                n + 1,
                DateRange {
                    from: None,
                    to: Some(a.0),
                },
            ));
        }
        "until" | "till" | "thru" | "through" => {
            let (n, a) = atom(&words[1..], today)?;
            return Some((
                n + 1,
                DateRange {
                    from: None,
                    to: Some(a.1),
                },
            ));
        }
        "between" => {
            let (na, _) = atom(&words[1..], today)?;
            if words.get(1 + na) != Some(&"and") {
                return None;
            }
            let rest = &words[2 + na..];
            let (nb, b) = atom(rest, today)?;
            // Resolve the start relative to the end: "between nov and feb" in
            // September 2026 is Nov 2025 – Feb 2026.
            let (_, a) = atom(&words[1..], b.1 - Duration::days(1))?;
            return Some((
                2 + na + nb,
                DateRange {
                    from: Some(a.0.min(b.0)),
                    to: Some(b.1.max(a.1)),
                },
            ));
        }
        _ => {}
    }
    // "jan 5 to jan 20": a range when both sides are dates.
    if let Some(found) = span(words, today) {
        return Some(found);
    }
    let (n, a) = atom(words, today)?;
    Some((
        n,
        DateRange {
            from: Some(a.0),
            to: if a.2 { None } else { Some(a.1) },
        },
    ))
}

/// `<atom> (to|through|until|-) <atom>`, the start resolved relative to the end.
fn span(words: &[&str], today: NaiveDate) -> Option<(usize, DateRange)> {
    let (n, _) = atom(words, today)?;
    let sep = *words.get(n)?;
    if !matches!(
        sep,
        "to" | "through" | "thru" | "until" | "till" | "-" | "\u{2013}"
    ) {
        return None;
    }
    let (nb, b) = atom(&words[n + 1..], today)?;
    let (_, a) = atom(words, b.1 - Duration::days(1))?;
    Some((
        n + 1 + nb,
        DateRange {
            from: Some(a.0.min(b.0)),
            to: Some(b.1.max(a.1)),
        },
    ))
}

/// (start, end-exclusive, open_ended) of one date atom, plus words consumed.
/// `open_ended` marks "past week"-style ranges that run up to now.
type Atom = (NaiveDate, NaiveDate, bool);

fn atom(words: &[&str], r: NaiveDate) -> Option<(usize, Atom)> {
    let w0 = *words.first()?;
    let w1 = words.get(1).copied();
    let w2 = words.get(2).copied();
    let day = |d: NaiveDate| (d, d + Duration::days(1), false);
    let tomorrow = r + Duration::days(1);

    // today / yesterday
    match w0 {
        "today" => return Some((1, day(r))),
        "yesterday" => return Some((1, day(r - Duration::days(1)))),
        _ => {}
    }

    // Three-word relative forms.
    if let (Some(b), Some(c)) = (w1, w2) {
        if let (Ok(n), Some(unit)) = (b.parse::<u32>(), unit_word(c)) {
            if (w0 == "last" || w0 == "past") && (1..=1000).contains(&n) {
                let start = sub_units(r, n, unit)?;
                return Some((3, (start, tomorrow, true)));
            }
        }
        if let (Ok(n), Some(unit), "ago") = (w0.parse::<u32>(), unit_word(b), c) {
            if (1..=1000).contains(&n) {
                let span = match unit {
                    'd' => day(r - Duration::days(n as i64)),
                    'w' => {
                        let d = r - Duration::weeks(n as i64);
                        (d - Duration::days(3), d + Duration::days(4), false)
                    }
                    'm' => {
                        let d = sub_units(first_of_month(r), n, 'm')?;
                        (d, add_months(d, 1)?, false)
                    }
                    _ => {
                        let y = r.year() - n as i32;
                        (ymd(y, 1, 1)?, ymd(y + 1, 1, 1)?, false)
                    }
                };
                return Some((3, span));
            }
        }
    }

    // "the week of feb 10" / "week of feb 10"
    let week_of = match (w0, w1, w2) {
        ("the", Some("week"), Some("of")) => Some(3),
        ("week", Some("of"), _) => Some(2),
        _ => None,
    };
    if let Some(skip) = week_of {
        let (n, d) = month_day(&words[skip..], r)?;
        let m = monday(d);
        return Some((skip + n, (m, m + Duration::days(7), false)));
    }

    // this|last|past week/month/year, seasons, "last <month>", "last <weekday>"
    if let Some(b) = w1 {
        let found: Option<Atom> = match (w0, b) {
            ("this", "week") => {
                let m = monday(r);
                Some((m, m + Duration::days(7), false))
            }
            ("last", "week") => {
                let m = monday(r);
                Some((m - Duration::days(7), m, false))
            }
            ("past", "week") => Some((r - Duration::days(7), tomorrow, true)),
            ("this", "month") => {
                let f = first_of_month(r);
                Some((f, add_months(f, 1)?, false))
            }
            ("last", "month") => {
                let f = first_of_month(r);
                Some((sub_units(f, 1, 'm')?, f, false))
            }
            ("past", "month") => Some((sub_units(r, 1, 'm')?, tomorrow, true)),
            // Saturday and Sunday of this (Monday-first) week, or the week before.
            ("this", "weekend") => {
                let m = monday(r);
                Some((m + Duration::days(5), m + Duration::days(7), false))
            }
            ("last" | "past", "weekend") => {
                let m = monday(r);
                Some((m - Duration::days(2), m, false))
            }
            ("this", "quarter") => {
                let q = quarter_start(r);
                Some((q, add_months(q, 3)?, false))
            }
            ("last", "quarter") => {
                let q = quarter_start(r);
                Some((q.checked_sub_months(Months::new(3))?, q, false))
            }
            ("past", "quarter") => Some((sub_units(r, 3, 'm')?, tomorrow, true)),
            ("this", "year") => Some((ymd(r.year(), 1, 1)?, ymd(r.year() + 1, 1, 1)?, false)),
            ("last", "year") => Some((ymd(r.year() - 1, 1, 1)?, ymd(r.year(), 1, 1)?, false)),
            ("past", "year") => Some((sub_units(r, 12, 'm')?, tomorrow, true)),
            ("this" | "last", s) if season_start(s).is_some() => {
                let (a, b) = season_range(r, season_start(s)?, w0 == "last")?;
                Some((a, b, false))
            }
            ("last", m) if month_num(m).is_some() => {
                let recent = recent_month(month_num(m)?, r)?;
                let prev = recent.checked_sub_months(Months::new(12))?;
                Some((prev, add_months(prev, 1)?, false))
            }
            ("last", wd) if weekday(wd).is_some() => {
                let d = recent_weekday(weekday(wd)?, r - Duration::days(1));
                Some(day(d))
            }
            _ => None,
        };
        if let Some(a) = found {
            return Some((2, a));
        }
        if w0 == "past" && b == "12" && w2 == Some("months") {
            return Some((3, (sub_units(r, 12, 'm')?, tomorrow, true)));
        }
    }

    if let Some(found) = quarter(words, r) {
        return Some(found);
    }

    // Weekday: the most recent one (today counts).
    if let Some(wd) = weekday(w0) {
        return Some((1, day(recent_weekday(wd, r))));
    }

    // "in <month> [year]" / "in <year>"
    if w0 == "in" {
        let b = w1?;
        if let Some(y) = year_num(b) {
            return Some((2, (ymd(y, 1, 1)?, ymd(y + 1, 1, 1)?, false)));
        }
        if let Some((n, a)) = quarter(&words[1..], r) {
            return Some((1 + n, a));
        }
        let (n, a) = month_expr(&words[1..], r)?;
        return Some((1 + n, a));
    }

    // ISO dates and bare years.
    if let Some(a) = iso(w0) {
        return Some((1, a));
    }
    if let Some(y) = year_num(w0) {
        return Some((1, (ymd(y, 1, 1)?, ymd(y + 1, 1, 1)?, false)));
    }

    month_expr(words, r)
}

/// `[early|mid|late] <month> [<day>] [<year>]`, or `<month> <year>`.
fn month_expr(words: &[&str], r: NaiveDate) -> Option<(usize, Atom)> {
    let w0 = *words.first()?;
    let part = match w0 {
        "early" | "beginning" => Some((1, 10)),
        "mid" | "middle" => Some((11, 20)),
        "late" | "end" => Some((21, 31)),
        _ => None,
    };
    if let Some((d0, d1)) = part {
        let rest = &words[1..];
        // "end of march" / "beginning of march"
        let rest = if rest.first() == Some(&"of") {
            &rest[1..]
        } else {
            rest
        };
        let skipped = words.len() - rest.len();
        let (n, (start, end, _)) = month_only(rest, r)?;
        let last_day = (end - Duration::days(1)).day();
        let a = start.with_day(d0)?;
        let b = start.with_day(d1.min(last_day))? + Duration::days(1);
        return Some((skipped + n, (a, b, false)));
    }
    // "<month> <day> [year]" before "<month> [year]".
    if let Some((n, d)) = month_day(words, r) {
        return Some((n, (d, d + Duration::days(1), false)));
    }
    // Day first, as most of the world writes it: "18 august 2026",
    // "18th of august".
    if let Some(d) = day_word(w0) {
        let rest = if words.get(1) == Some(&"of") { &words[2..] } else { &words[1..] };
        let skipped = words.len() - rest.len();
        let m = month_num(rest.first()?)?;
        let (n, date) = match rest.get(1).and_then(|w| year_word(w)) {
            Some(y) => (2, ymd(y, m, d)?),
            None => (1, recent_day(m, d, r)?),
        };
        return Some((skipped + n, (date, date + Duration::days(1), false)));
    }
    month_only(words, r)
}

/// The most recent past `m`/`d` (this year's, else last year's).
fn recent_day(m: u32, d: u32, r: NaiveDate) -> Option<NaiveDate> {
    match ymd(r.year(), m, d) {
        Some(x) if x <= r => Some(x),
        _ => ymd(r.year() - 1, m, d).or_else(|| ymd(r.year() - 2, m, d)),
    }
}

/// "q2" (the most recent one that has started), "q2 2026". `date:q2`
/// arrives as "q 2" (date_words splits a letter from a digit).
fn quarter(words: &[&str], r: NaiveDate) -> Option<(usize, Atom)> {
    let w0 = *words.first()?;
    let (n, used) = quarter_word(w0).map(|n| (n, 1)).or_else(|| {
        (w0 == "q")
            .then(|| words.get(1).and_then(|x| x.parse::<u32>().ok()).filter(|n| (1..=4).contains(n)))
            .flatten()
            .map(|n| (n, 2))
    })?;
    let month = (n - 1) * 3 + 1;
    if let Some(y) = words.get(used).and_then(|w| year_word(w)) {
        let s = ymd(y, month, 1)?;
        return Some((used + 1, (s, add_months(s, 3)?, false)));
    }
    let this = ymd(r.year(), month, 1)?;
    let s = if this <= r { this } else { ymd(r.year() - 1, month, 1)? };
    Some((used, (s, add_months(s, 3)?, false)))
}

/// "q1"…"q4".
fn quarter_word(w: &str) -> Option<u32> {
    let n: u32 = w.strip_prefix('q')?.parse().ok()?;
    (1..=4).contains(&n).then_some(n)
}

/// First day of the quarter containing `d`.
fn quarter_start(d: NaiveDate) -> NaiveDate {
    ymd(d.year(), d.month0() / 3 * 3 + 1, 1).expect("valid quarter start")
}

/// `<month> [year]`.
fn month_only(words: &[&str], r: NaiveDate) -> Option<(usize, Atom)> {
    let m = month_num(words.first()?)?;
    if let Some(y) = words.get(1).and_then(|w| year_word(w)) {
        let s = ymd(y, m, 1)?;
        return Some((2, (s, add_months(s, 1)?, false)));
    }
    let s = recent_month(m, r)?;
    Some((1, (s, add_months(s, 1)?, false)))
}

/// `<month> <day> [year]` (day may carry st/nd/rd/th and a trailing comma).
/// Without a year, the most recent past occurrence.
fn month_day(words: &[&str], r: NaiveDate) -> Option<(usize, NaiveDate)> {
    let m = month_num(words.first()?)?;
    let d = day_word(words.get(1)?)?;
    if let Some(y) = words.get(2).and_then(|w| year_word(w)) {
        return Some((3, ymd(y, m, d)?));
    }
    let this_year = ymd(r.year(), m, d);
    let date = match this_year {
        Some(x) if x <= r => x,
        _ => ymd(r.year() - 1, m, d).or_else(|| ymd(r.year() - 2, m, d))?,
    };
    Some((2, date))
}

fn iso(w: &str) -> Option<Atom> {
    let parts: Vec<&str> = w.split(['-', '/', '.']).collect();
    let num = |s: &str| s.parse::<u32>().ok();
    match parts.as_slice() {
        [y, m] if y.len() == 4 => {
            let s = ymd(year_num(y)?, num(m)?, 1)?;
            Some((s, add_months(s, 1)?, false))
        }
        [y, m, d] if y.len() == 4 => {
            let s = ymd(year_num(y)?, num(m)?, num(d)?)?;
            Some((s, s + Duration::days(1), false))
        }
        [a, b, y] if y.len() == 4 || (y.len() == 2 && a.len() <= 2) => {
            let s = numeric_day(w.contains('.'), num(a)?, num(b)?, y)?;
            Some((s, s + Duration::days(1), false))
        }
        _ => None,
    }
}

/// A numeric day written `a b y`. Month first (Gmail, US) unless that is
/// impossible and day first isn't (18/08/2026), or the date is dotted, which
/// is day first (18.08.2026, as in Germany) unless only month first works.
/// Two-digit years are 20yy.
pub(crate) fn numeric_day(dotted: bool, a: u32, b: u32, y: &str) -> Option<NaiveDate> {
    let year = match y.len() {
        4 => year_num(y)?,
        2 => 2000 + y.parse::<i32>().ok()?,
        _ => return None,
    };
    let month_first = ymd(year, a, b);
    let day_first = ymd(year, b, a);
    if dotted {
        day_first.or(month_first)
    } else {
        month_first.or(day_first)
    }
}

// ---------- vocabulary ----------

pub(crate) fn month_num(w: &str) -> Option<u32> {
    Some(match w {
        "january" | "jan" => 1,
        "february" | "feb" => 2,
        "march" | "mar" => 3,
        "april" | "apr" => 4,
        "may" => 5,
        "june" | "jun" => 6,
        "july" | "jul" => 7,
        "august" | "aug" => 8,
        "september" | "sep" | "sept" => 9,
        "october" | "oct" => 10,
        "november" | "nov" => 11,
        "december" | "dec" => 12,
        _ => return None,
    })
}

fn weekday(w: &str) -> Option<Weekday> {
    Some(match w {
        "monday" => Weekday::Mon,
        "tuesday" | "tues" => Weekday::Tue,
        "wednesday" | "weds" => Weekday::Wed,
        "thursday" | "thurs" => Weekday::Thu,
        "friday" => Weekday::Fri,
        "saturday" => Weekday::Sat,
        "sunday" => Weekday::Sun,
        _ => return None,
    })
}

fn unit_word(w: &str) -> Option<char> {
    Some(match w {
        "day" | "days" => 'd',
        "week" | "weeks" => 'w',
        "month" | "months" => 'm',
        "year" | "years" => 'y',
        _ => return None,
    })
}

pub(crate) fn year_num(w: &str) -> Option<i32> {
    if w.len() != 4 {
        return None;
    }
    w.parse::<i32>().ok().filter(|y| (1970..=2100).contains(y))
}

/// "2025", "'25", "’25".
fn year_word(w: &str) -> Option<i32> {
    if let Some(y) = year_num(w) {
        return Some(y);
    }
    let short = w
        .strip_prefix('\'')
        .or_else(|| w.strip_prefix('\u{2019}'))?;
    if short.len() != 2 {
        return None;
    }
    short.parse::<i32>().ok().map(|y| 2000 + y)
}

/// "10", "10th", "1st", "10," → 10.
fn day_word(w: &str) -> Option<u32> {
    let w = w.trim_end_matches(',');
    let digits = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = &w[digits.len()..];
    if !matches!(suffix, "" | "st" | "nd" | "rd" | "th") || digits.is_empty() || digits.len() > 2 {
        return None;
    }
    digits.parse::<u32>().ok().filter(|d| (1..=31).contains(d))
}

// ---------- calendar helpers ----------

pub(crate) fn ymd(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(y, m, d)
}

pub(crate) fn first_of_month(d: NaiveDate) -> NaiveDate {
    d.with_day(1).expect("day 1 exists")
}

pub(crate) fn add_months(d: NaiveDate, n: u32) -> Option<NaiveDate> {
    d.checked_add_months(Months::new(n))
}

pub(crate) fn sub_units(d: NaiveDate, n: u32, unit: char) -> Option<NaiveDate> {
    match unit {
        'd' => d.checked_sub_signed(Duration::days(n as i64)),
        'w' => d.checked_sub_signed(Duration::weeks(n as i64)),
        'm' => d.checked_sub_months(Months::new(n)),
        'y' => d.checked_sub_months(Months::new(n.checked_mul(12)?)),
        _ => None,
    }
}

pub(crate) fn monday(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// First day of the most recent month `m` that has started by `r`.
fn recent_month(m: u32, r: NaiveDate) -> Option<NaiveDate> {
    let y = if m <= r.month() {
        r.year()
    } else {
        r.year() - 1
    };
    ymd(y, m, 1)
}

/// The most recent `wd` on or before `r`.
fn recent_weekday(wd: Weekday, r: NaiveDate) -> NaiveDate {
    let back =
        (7 + r.weekday().num_days_from_monday() as i64 - wd.num_days_from_monday() as i64) % 7;
    r - Duration::days(back)
}

/// Meteorological seasons (northern hemisphere): start month.
fn season_start(w: &str) -> Option<u32> {
    match w {
        "spring" => Some(3),
        "summer" => Some(6),
        "fall" | "autumn" => Some(9),
        "winter" => Some(12),
        _ => None,
    }
}

/// `last`: the most recent instance of the season that has fully ended.
/// `this`: the instance in progress, or the one in the current season-year.
fn season_range(today: NaiveDate, start_month: u32, last: bool) -> Option<(NaiveDate, NaiveDate)> {
    let inst = |y: i32| -> Option<(NaiveDate, NaiveDate)> {
        let s = ymd(y, start_month, 1)?;
        Some((s, add_months(s, 3)?))
    };
    let mut y = today.year() + 1;
    if last {
        loop {
            let (s, e) = inst(y)?;
            if e <= today {
                return Some((s, e));
            }
            y -= 1;
        }
    }
    loop {
        let (s, e) = inst(y)?;
        if s <= today {
            if e > today || s.year() == today.year() {
                return Some((s, e));
            }
            return inst(y + 1);
        }
        y -= 1;
    }
}
