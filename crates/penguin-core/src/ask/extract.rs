//! Deterministic extraction from message text: currency amounts, the total
//! of a receipt or invoice, and calendar dates mentioned in prose. No
//! guessing beyond fixed rules; every result carries the text it came from
//! so the answer can quote it.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::dates::ymd;

/// Month names in English (`dates::month_num`) and Spanish.
fn month_num(w: &str) -> Option<u32> {
    crate::dates::month_num(w).or(match w {
        "enero" | "ene" => Some(1),
        "febrero" => Some(2),
        "marzo" => Some(3),
        "abril" | "abr" => Some(4),
        "mayo" => Some(5),
        "junio" => Some(6),
        "julio" => Some(7),
        "agosto" | "ago" => Some(8),
        "septiembre" | "setiembre" => Some(9),
        "octubre" => Some(10),
        "noviembre" => Some(11),
        "diciembre" | "dic" => Some(12),
        _ => None,
    })
}

// ---------------------------------------------------------------- amounts

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Amount {
    /// Major units (dollars, euros); negative for "-$5" or "($5.00)".
    pub value: f64,
    /// ISO code: USD, EUR, GBP, CAD, AUD, PHP, MXN, INR, JPY.
    pub currency: &'static str,
    /// Byte span in the source text.
    pub start: usize,
    pub end: usize,
}

/// Currency markers written before the number ("$12", "US$ 12", "EUR 12").
const PREFIXES: &[(&str, &str)] = &[
    ("us$", "USD"),
    ("usd", "USD"),
    ("ca$", "CAD"),
    ("c$", "CAD"),
    ("cad", "CAD"),
    ("au$", "AUD"),
    ("a$", "AUD"),
    ("aud", "AUD"),
    ("mx$", "MXN"),
    ("mxn", "MXN"),
    ("$", "USD"),
    ("€", "EUR"),
    ("eur", "EUR"),
    ("£", "GBP"),
    ("gbp", "GBP"),
    ("₱", "PHP"),
    ("php", "PHP"),
    ("₹", "INR"),
    ("inr", "INR"),
    ("¥", "JPY"),
    ("jpy", "JPY"),
];

/// Markers written after the number ("12.50 USD", "12,50 €").
const SUFFIXES: &[(&str, &str)] = &[
    ("usd", "USD"),
    ("dollars", "USD"),
    ("eur", "EUR"),
    ("euros", "EUR"),
    ("€", "EUR"),
    ("gbp", "GBP"),
    ("£", "GBP"),
    ("cad", "CAD"),
    ("aud", "AUD"),
    ("php", "PHP"),
    ("mxn", "MXN"),
    ("inr", "INR"),
    ("jpy", "JPY"),
];

/// Every currency amount in `text`, in order.
pub(crate) fn amounts(text: &str) -> Vec<Amount> {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; offsets are only
    // reported when they line up, which is always the case for ASCII/€/£/$.
    let same = lower.len() == text.len();
    let b = lower.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !lower.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &lower[i..];
        // Prefix marker, which must not be glued to a preceding letter ("feur").
        let prev_alpha = lower[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphabetic());
        let mut matched = None;
        for (p, code) in PREFIXES {
            let alpha = p.chars().all(|c| c.is_ascii_alphabetic());
            if rest.starts_with(p) && !(alpha && prev_alpha) {
                matched = Some((p.len(), *code));
                break;
            }
        }
        if let Some((plen, code)) = matched {
            let mut j = i + plen;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if let Some((value, end)) = number(&lower, j) {
                let neg = negative_before(&lower, i);
                let paren = lower[..i].trim_end().ends_with('(') && lower[end..].starts_with(')');
                let v = if neg || paren { -value } else { value };
                let start = if neg {
                    lower[..i].rfind('-').unwrap_or(i)
                } else {
                    i
                };
                if same {
                    out.push(Amount {
                        value: v,
                        currency: code,
                        start,
                        end,
                    });
                }
                i = end;
                continue;
            }
        }
        // Number followed by a suffix marker.
        if b[i].is_ascii_digit() && (i == 0 || !is_word_byte(b[i - 1])) {
            if let Some((value, end)) = number(&lower, i) {
                let mut k = end;
                while k < b.len() && b[k] == b' ' {
                    k += 1;
                }
                let tail = &lower[k..];
                if let Some((s, code)) = SUFFIXES.iter().find(|(s, _)| {
                    tail.starts_with(s)
                        && !tail[s.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_alphanumeric())
                }) {
                    let neg = negative_before(&lower, i);
                    if same {
                        out.push(Amount {
                            value: if neg { -value } else { value },
                            currency: code,
                            start: i,
                            end: k + s.len(),
                        });
                    }
                    i = k + s.len();
                    continue;
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'.' || c == b','
}

fn negative_before(s: &str, i: usize) -> bool {
    let before = s[..i].trim_end_matches(' ');
    before.ends_with('-') || before.ends_with('\u{2212}')
}

/// Parse a money number at `i`: "1,234.56", "1.234,56", "12", "12.5".
/// Returns (value, end byte).
fn number(s: &str, i: usize) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    if i >= b.len() || !b[i].is_ascii_digit() {
        return None;
    }
    let mut j = i;
    while j < b.len()
        && (b[j].is_ascii_digit()
            || ((b[j] == b',' || b[j] == b'.') && j + 1 < b.len() && b[j + 1].is_ascii_digit()))
    {
        j += 1;
    }
    let raw = &s[i..j];
    // A number that runs into letters ("3pm", "2fa") is not money.
    if b.get(j).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    // The last separator is a decimal point when 1–2 digits follow it
    // ("12.50", "12,50"); with 3 it groups thousands ("1,234", "1.234").
    let value = match raw.rfind([',', '.']) {
        None => raw.parse::<f64>().ok()?,
        Some(p) => {
            let frac = &raw[p + 1..];
            let int: String = raw[..p].chars().filter(|c| c.is_ascii_digit()).collect();
            match frac.len() {
                1 | 2 => format!("{int}.{frac}").parse::<f64>().ok()?,
                3 => format!("{int}{frac}").parse::<f64>().ok()?,
                _ => return None,
            }
        }
    };
    Some((value, j))
}

/// Receipt/invoice total: the amount on the most specific "total" line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Total {
    pub amount: Amount,
    /// The line it came from, trimmed (≤160 chars).
    pub line: String,
}

/// Total-line labels, most specific first. A line containing an exclusion
/// (subtotal, savings) never counts.
const TOTAL_KEYS: &[(&str, u8)] = &[
    ("grand total", 5),
    ("order total", 5),
    ("trip total", 5),
    ("total charged", 5),
    ("amount charged", 5),
    ("total paid", 5),
    ("amount paid", 5),
    ("you paid", 5),
    ("refund total", 5),
    ("refund of", 5),
    ("refunded", 4),
    ("total pagado", 5),
    ("importe total", 4),
    ("monto total", 4),
    ("total a pagar", 4),
    ("total amount", 4),
    ("payment amount", 4),
    ("charged to", 4),
    ("total", 3),
    ("amount due", 2),
    ("balance due", 2),
    ("charged", 2),
];
const TOTAL_EXCLUDE: &[&str] = &[
    "subtotal",
    "sub-total",
    "sub total",
    "total savings",
    "total discount",
    "you saved",
    "total points",
    "total miles",
    "total distance",
    "total time",
];

/// Every TOTAL_KEYS label contains one of these; lines without any are
/// skipped before the full checks (most of a long email).
const TOTAL_STEMS: &[&str] = &[
    "total", "charged", "paid", "amount", "due", "importe", "monto", "refund",
];

pub(crate) fn receipt_total(text: &str) -> Option<Total> {
    let lower_all = text.to_lowercase();
    if !TOTAL_STEMS.iter().any(|k| lower_all.contains(k)) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut best: Option<(u8, usize, Amount, String)> = None;
    // Lowercasing never adds or removes line breaks, so the lines pair up.
    for ((n, line), l) in lines.iter().enumerate().zip(lower_all.lines()) {
        if !TOTAL_STEMS.iter().any(|k| l.contains(k)) {
            continue;
        }
        if TOTAL_EXCLUDE.iter().any(|x| l.contains(x)) {
            continue;
        }
        let Some(rank) = TOTAL_KEYS
            .iter()
            .find(|(k, _)| contains_word(l, k))
            .map(|(_, r)| *r)
        else {
            continue;
        };
        // The amount is on the line, or alone on the next non-empty line
        // (two-column receipts rendered as text).
        let mut found = amounts(line)
            .into_iter()
            .last()
            .map(|a| (a, line.trim().to_string()));
        if found.is_none() {
            if let Some(next) = lines[n + 1..].iter().find(|x| !x.trim().is_empty()) {
                if let Some(a) = amounts(next).into_iter().last() {
                    found = Some((a, format!("{} {}", line.trim(), next.trim())));
                }
            }
        }
        let Some((a, text_line)) = found else {
            continue;
        };
        // Later lines win ties: totals come after line items.
        if best.as_ref().is_none_or(|b| rank >= b.0) {
            best = Some((rank, n, a, text_line));
        }
    }
    best.map(|(_, _, amount, line)| Total {
        amount,
        line: clip(&line, 160),
    })
}

fn contains_word(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(p) = hay[from..].find(needle) {
        let s = from + p;
        let e = s + needle.len();
        let before = hay[..s]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after = hay[e..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        from = e;
    }
    false
}

pub(crate) fn clip(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

// ------------------------------------------------------------------ dates

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DateMention {
    pub date: NaiveDate,
    /// "7pm", "7:30 pm", "19:00" right after the date, if any.
    pub time: Option<String>,
    /// Byte span of the date words in the text.
    pub start: usize,
    pub end: usize,
    /// The sentence containing the mention, whitespace-collapsed (≤220 chars).
    pub sentence: String,
    /// Written with an explicit day (not just "Thursday" or "tomorrow").
    pub explicit: bool,
}

struct Tok {
    s: usize,
    e: usize,
    w: String,
}

fn toks(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let push = |out: &mut Vec<Tok>, s: usize, e: usize| {
        let raw = &text[s..e];
        let raw = raw.trim_end_matches(['-', '/', '.']);
        if !raw.is_empty() {
            out.push(Tok {
                s,
                e: s + raw.len(),
                w: raw.to_lowercase(),
            });
        }
    };
    for (i, c) in text.char_indices() {
        let part = c.is_alphanumeric()
            || ((c == '/' || c == '-' || c == ':' || c == '.') && start.is_some());
        if part {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            push(&mut out, s, i);
        }
    }
    if let Some(s) = start {
        push(&mut out, s, text.len());
    }
    out
}

fn weekday_word(w: &str) -> Option<Weekday> {
    Some(match w {
        "monday" | "mon" => Weekday::Mon,
        "tuesday" | "tue" | "tues" => Weekday::Tue,
        "wednesday" | "wed" | "weds" => Weekday::Wed,
        "thursday" | "thu" | "thur" | "thurs" => Weekday::Thu,
        "friday" | "fri" => Weekday::Fri,
        "saturday" | "sat" => Weekday::Sat,
        "sunday" | "sun" => Weekday::Sun,
        "lunes" => Weekday::Mon,
        "martes" => Weekday::Tue,
        "miércoles" | "miercoles" => Weekday::Wed,
        "jueves" => Weekday::Thu,
        "viernes" => Weekday::Fri,
        "sábado" | "sabado" => Weekday::Sat,
        "domingo" => Weekday::Sun,
        _ => return None,
    })
}

fn day_num(w: &str) -> Option<u32> {
    let digits = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = &w[digits.len()..];
    if digits.is_empty() || digits.len() > 2 || !matches!(suffix, "" | "st" | "nd" | "rd" | "th") {
        return None;
    }
    digits.parse().ok().filter(|d| (1..=31).contains(d))
}

fn year_tok(w: &str) -> Option<i32> {
    if w.len() == 4 {
        return w.parse().ok().filter(|y| (1990..=2100).contains(y));
    }
    None
}

/// Resolve a month/day with no year to the occurrence closest to `anchor`,
/// leaning forward: mail usually talks about upcoming dates.
fn nearest_year(m: u32, d: u32, anchor: NaiveDate) -> Option<NaiveDate> {
    [anchor.year() - 1, anchor.year(), anchor.year() + 1]
        .into_iter()
        .filter_map(|y| ymd(y, m, d))
        .min_by_key(|x| {
            let diff = (*x - anchor).num_days();
            if diff >= 0 {
                diff
            } else {
                -diff * 3
            }
        })
}

/// Dates written in `text`, resolved against `anchor` (the message's date).
pub(crate) fn date_mentions(text: &str, anchor: NaiveDate) -> Vec<DateMention> {
    let t = toks(text);
    let mut out: Vec<DateMention> = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let w = t[i].w.as_str();
        let w1 = t.get(i + 1).map(|x| x.w.as_str());
        let w2 = t.get(i + 2).map(|x| x.w.as_str());
        let mut found: Option<(NaiveDate, usize, bool)> = None; // (date, tokens used, explicit)

        // 2026-09-24
        if found.is_none() {
            let parts: Vec<&str> = w.split(['-', '/', '.']).collect();
            let num = |s: &str| s.parse::<u32>().ok();
            match parts.as_slice() {
                [y, m, d] if y.len() == 4 => {
                    if let (Some(y), Some(m), Some(d)) = (year_tok(y), num(m), num(d)) {
                        found = ymd(y, m, d).map(|x| (x, 1, true));
                    }
                }
                // US order: 9/24/2026, 9/24/26, 9/24; day first when the
                // first number can't be a month (24/09/2026).
                [m, d, y] if w.contains('/') => {
                    let y = if y.len() == 2 {
                        y.parse::<i32>().ok().map(|v| 2000 + v)
                    } else {
                        year_tok(y)
                    };
                    if let (Some(y), Some(m), Some(d)) = (y, num(m), num(d)) {
                        let (m, d) = if m > 12 && d <= 12 { (d, m) } else { (m, d) };
                        found = ymd(y, m, d).map(|x| (x, 1, true));
                    }
                }
                [m, d] if w.contains('/') && m.len() <= 2 && d.len() <= 2 => {
                    if let (Some(m), Some(d)) = (num(m), num(d)) {
                        if (1..=12).contains(&m) {
                            found = nearest_year(m, d, anchor).map(|x| (x, 1, true));
                        }
                    }
                }
                _ => {}
            }
        }

        // Sep 24 [2026] / September 24th, 2026
        if found.is_none() {
            if let (Some(m), Some(d)) = (month_num(w.trim_end_matches('.')), w1.and_then(day_num)) {
                // "may 5" is the month; "may" alone is not.
                if let Some(y) = w2.and_then(year_tok) {
                    found = ymd(y, m, d).map(|x| (x, 3, true));
                } else {
                    found = nearest_year(m, d, anchor).map(|x| (x, 2, true));
                }
            }
        }
        // 24 September [2026] / 24th of Sep
        if found.is_none() {
            if let Some(d) = day_num(w) {
                let (mi, skip) = if matches!(w1, Some("of" | "de")) {
                    (i + 2, 3)
                } else {
                    (i + 1, 2)
                };
                if let Some(m) = t.get(mi).and_then(|x| month_num(x.w.trim_end_matches('.'))) {
                    // "24 September 2026", "15 de octubre de 2026".
                    let de = t.get(mi + 1).is_some_and(|x| x.w == "de") as usize;
                    if let Some(y) = t.get(mi + 1 + de).and_then(|x| year_tok(&x.w)) {
                        found = ymd(y, m, d).map(|x| (x, skip + 1 + de, true));
                    } else {
                        found = nearest_year(m, d, anchor).map(|x| (x, skip, true));
                    }
                }
            }
        }
        // today / tonight / tomorrow
        if found.is_none() {
            found = match w {
                "today" | "tonight" | "hoy" => Some((anchor, 1, false)),
                "tomorrow" => Some((anchor + Duration::days(1), 1, false)),
                _ => None,
            };
        }
        // [this|next|on] Thursday — the next one after the message date.
        if found.is_none() {
            if let Some(wd) = weekday_word(w) {
                let prev = if i > 0 { t[i - 1].w.as_str() } else { "" };
                // Short forms ("sat", "sun", "wed") only right after a cue word.
                let short = w.len() <= 4 && !matches!(w, "tues" | "thur");
                if !short
                    || matches!(
                        prev,
                        "on" | "this" | "next" | "by" | "until" | "el" | "este" | "próximo"
                    )
                {
                    // "next Monday" = the Monday of the following week;
                    // otherwise the first one after the message date.
                    let ahead = if prev == "next" || prev == "próximo" {
                        7 - anchor.weekday().num_days_from_monday() as i64
                            + wd.num_days_from_monday() as i64
                    } else {
                        let base = (7 + wd.num_days_from_monday() as i64
                            - anchor.weekday().num_days_from_monday() as i64)
                            % 7;
                        if base == 0 {
                            7
                        } else {
                            base
                        }
                    };
                    // "Thursday, Sep 24": let the explicit date that follows win.
                    // "viernes 9 de octubre" / "Friday 9 October" too.
                    let explicit_next = (w1.is_some_and(|x| month_num(x).is_some())
                        && w2.and_then(day_num).is_some())
                        || (w1.and_then(day_num).is_some()
                            && w2
                                .is_some_and(|x| x == "de" || x == "of" || month_num(x).is_some()));
                    if !explicit_next {
                        found = Some((anchor + Duration::days(ahead), 1, false));
                    }
                }
            }
        }

        let Some((date, used, explicit)) = found else {
            i += 1;
            continue;
        };
        let start = if i > 0 && weekday_word(&t[i - 1].w).is_some() && explicit {
            t[i - 1].s
        } else {
            t[i].s
        };
        let end = t[i + used - 1].e;
        let time = time_after(&t, i + used);
        out.push(DateMention {
            date,
            time,
            start,
            end,
            sentence: sentence_around(text, start, end),
            explicit,
        });
        i += used;
    }
    out
}

/// "at 7pm", "7:30 pm", "@ 19:00" right after a date.
fn time_after(t: &[Tok], mut i: usize) -> Option<String> {
    if t.get(i)
        .is_some_and(|x| matches!(x.w.as_str(), "at" | "around" | "by" | "from"))
    {
        i += 1;
    } else if t
        .get(i)
        .is_some_and(|x| matches!(x.w.as_str(), "a" | "sobre" | "hacia"))
        && t.get(i + 1)
            .is_some_and(|x| matches!(x.w.as_str(), "las" | "la"))
    {
        // "a las 21:00", "sobre las 8 pm"
        i += 2;
    }
    let w = t.get(i)?.w.as_str();
    let next = t.get(i + 1).map(|x| x.w.as_str());
    let is_clock = |s: &str| {
        let (h, m) = s.split_once(':').unwrap_or((s, "00"));
        h.parse::<u32>().is_ok_and(|h| h <= 23)
            && m.len() == 2
            && m.parse::<u32>().is_ok_and(|m| m < 60)
    };
    for suf in ["am", "pm"] {
        if let Some(base) = w.strip_suffix(suf) {
            if is_clock(base) {
                return Some(w.to_string());
            }
        }
    }
    if is_clock(w) && (w.contains(':') || matches!(next, Some("am" | "pm" | "a.m" | "p.m"))) {
        return Some(match next {
            Some(ap @ ("am" | "pm")) => format!("{w} {ap}"),
            _ => w.to_string(),
        });
    }
    None
}

/// The sentence around the first case-insensitive, punctuation-insensitive
/// occurrence of `phrase` (lowercase words) in `text`.
pub(crate) fn quote_around(text: &str, phrase: &str) -> Option<String> {
    let t = crate::text::tokens(text);
    let want: Vec<&str> = phrase.split_whitespace().collect();
    let i = (0..t.len()).find(|&i| {
        want.iter()
            .enumerate()
            .all(|(k, w)| t.get(i + k).is_some_and(|x| x.2 == *w))
    })?;
    Some(sentence_around(text, t[i].0, t[i + want.len() - 1].1))
}

fn sentence_around(text: &str, start: usize, end: usize) -> String {
    let is_stop = |c: char| matches!(c, '.' | '!' | '?' | '\n');
    let s = text[..start]
        .char_indices()
        .rev()
        .find(|(i, c)| is_stop(*c) && !(*c == '.' && abbrev_dot(text, *i)))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let e = text[end..]
        .char_indices()
        .find(|(i, c)| is_stop(*c) && !(*c == '.' && abbrev_dot(text, end + *i)))
        .map_or(text.len(), |(i, c)| end + i + c.len_utf8());
    clip(text[s..e].trim(), 220)
}

/// "Sep. 24", "Mr. Smith", "7 p.m.": a dot that doesn't end the sentence.
pub(crate) fn abbrev_dot(text: &str, dot: usize) -> bool {
    let word: String = text[..dot]
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let w = word.to_lowercase();
    month_num(&w).is_some()
        || matches!(
            w.as_str(),
            "mr" | "mrs"
                | "ms"
                | "dr"
                | "st"
                | "a"
                | "p"
                | "m"
                | "vs"
                | "etc"
                | "inc"
                | "e"
                | "i"
                | "g"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(s: &str) -> Vec<(f64, &'static str)> {
        amounts(s)
            .into_iter()
            .map(|a| (a.value, a.currency))
            .collect()
    }

    #[test]
    fn currency_amounts() {
        assert_eq!(vals("Total $23.40"), vec![(23.40, "USD")]);
        assert_eq!(vals("You paid $1,234.56 today"), vec![(1234.56, "USD")]);
        assert_eq!(vals("$ 12"), vec![(12.0, "USD")]);
        assert_eq!(
            vals("€12,50 and 1.234,56 €"),
            vec![(12.50, "EUR"), (1234.56, "EUR")]
        );
        assert_eq!(vals("£7.20"), vec![(7.20, "GBP")]);
        assert_eq!(vals("US$ 99.00"), vec![(99.0, "USD")]);
        assert_eq!(vals("CA$15"), vec![(15.0, "CAD")]);
        assert_eq!(vals("₱1,500.00"), vec![(1500.0, "PHP")]);
        assert_eq!(vals("12.50 USD"), vec![(12.50, "USD")]);
        assert_eq!(vals("Refund -$4.00"), vec![(-4.0, "USD")]);
        assert_eq!(vals("Credit ($5.00)"), vec![(-5.0, "USD")]);
        assert_eq!(vals("$1,000"), vec![(1000.0, "USD")]);
        assert_eq!(vals("EUR 30"), vec![(30.0, "EUR")]);
        // Not money.
        assert!(vals("meet at 3pm on 9/24").is_empty());
        assert!(vals("order #12345").is_empty());
        assert!(vals("amazing 50% off").is_empty());
        assert!(vals("the feur 12").is_empty());
    }

    #[test]
    fn receipt_totals() {
        let uber = "Thanks for riding, Sam\nTrip fare $18.20\nBooking fee $2.10\nSubtotal $20.30\nTip $3.10\nTotal $23.40\nCharged to Visa ••1234";
        let t = receipt_total(uber).unwrap();
        assert_eq!((t.amount.value, t.amount.currency), (23.40, "USD"));
        assert_eq!(t.line, "Total $23.40");

        let two_col = "Order summary\nItems $40.00\nOrder Total\n$42.99\n";
        assert_eq!(receipt_total(two_col).unwrap().amount.value, 42.99);

        let inv = "Invoice #88\nAmount due: €1.250,00\nDue by Oct 1";
        let t = receipt_total(inv).unwrap();
        assert_eq!((t.amount.value, t.amount.currency), (1250.0, "EUR"));

        // "Grand total" beats a later plain "total" mention of savings.
        let g = "Grand total: $99.00\nTotal savings: $10.00";
        assert_eq!(receipt_total(g).unwrap().amount.value, 99.0);

        assert!(receipt_total("Get $5 off your next ride!").is_none());
        assert!(receipt_total("Subtotal $4.00").is_none());
    }

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    fn first(s: &str, anchor: NaiveDate) -> Option<NaiveDate> {
        date_mentions(s, anchor).first().map(|m| m.date)
    }

    #[test]
    fn date_mentions_resolve() {
        let a = d(2026, 9, 20); // a Sunday
        assert_eq!(
            first("Want to grab dinner on Thursday, Sep 24 at 7pm?", a),
            Some(d(2026, 9, 24))
        );
        assert_eq!(first("Dinner on Thursday?", a), Some(d(2026, 9, 24)));
        assert_eq!(
            first("Your lease renews on 2027-01-31.", a),
            Some(d(2027, 1, 31))
        );
        assert_eq!(first("Renewal due 1/31/2027", a), Some(d(2027, 1, 31)));
        assert_eq!(first("due 1/31/27", a), Some(d(2027, 1, 31)));
        assert_eq!(
            first("See you on the 3rd of October", a),
            Some(d(2026, 10, 3))
        );
        assert_eq!(first("Kickoff was August 3rd", a), Some(d(2026, 8, 3)));
        assert_eq!(first("starts January 5", a), Some(d(2027, 1, 5)));
        assert_eq!(
            first("It closed on 24 September 2025.", a),
            Some(d(2025, 9, 24))
        );
        assert_eq!(first("let's do tomorrow", a), Some(d(2026, 9, 21)));
        assert_eq!(first("Sept. 30, 2026 works", a), Some(d(2026, 9, 30)));
        assert_eq!(first("next Monday", a), Some(d(2026, 9, 21)));
        assert_eq!(first("nothing to see", a), None);
        assert_eq!(first("I may be late", a), None);
        assert_eq!(first("ticket 12345 and 3.5 stars", a), None);

        let m = &date_mentions(
            "Hi Alex. Want to grab dinner on Thursday, Sep 24 at 7pm? Let me know.",
            a,
        )[0];
        assert_eq!(m.time.as_deref(), Some("7pm"));
        assert_eq!(
            m.sentence,
            "Want to grab dinner on Thursday, Sep 24 at 7pm?"
        );
        assert!(m.explicit);
        let m = &date_mentions("Call at 10:30 am on Oct 2.", a)[0];
        assert_eq!(m.date, d(2026, 10, 2));
        let m = &date_mentions("Oct 2 at 10:30 am", a)[0];
        assert_eq!(m.time.as_deref(), Some("10:30 am"));
    }
}
