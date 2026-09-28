//! Calendar windows ("last spring", "last month") resolved in local time,
//! with the same meaning as the search parser (`penguin-core/src/dates.rs`):
//! seasons are meteorological, "last <season>" is the most recent one that
//! has fully ended, "last month" is the previous calendar month.
//!
//! The corpus places time-scoped targets inside a window and the judgments
//! test membership in the same window, so time-scoped queries stay correct
//! whatever day the evaluation runs.

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};

use crate::rng::Rng;

pub const DAY: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// `a..b` whole days before today (a < b), e.g. DaysAgo(0, 7).
    DaysAgo(i64, i64),
    LastMonth,
    LastSpring,
    LastSummer,
    LastWinter,
    ThisYear,
    LastYear,
    /// The most recent completed month with this number (1-12), never the
    /// current month.
    Month(u32),
    /// Absolute `[start, end)` in unix ms, for queries written against the
    /// generation day (`after:2026/08/14`).
    Abs(i64, i64),
    /// The previous calendar quarter.
    LastQuarter,
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

/// The first of the month `n` months after `d`'s month.
pub(crate) fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + n;
    ymd(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1)
}

pub fn local_ms(d: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis())
}

pub fn today(now: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(now)
        .single()
        .map(|t| t.date_naive())
        .unwrap_or_else(|| {
            chrono::DateTime::from_timestamp_millis(now)
                .unwrap()
                .date_naive()
        })
}

fn last_season(t: NaiveDate, start_month: u32) -> (NaiveDate, NaiveDate) {
    let mut y = t.year() + 1;
    loop {
        let s = ymd(y, start_month, 1);
        let e = add_months(s, 3);
        if e <= t {
            return (s, e);
        }
        y -= 1;
    }
}

impl Window {
    /// Half-open `[start, end)` in unix ms.
    pub fn range(self, now: i64) -> (i64, i64) {
        let t = today(now);
        let (a, b) = match self {
            Window::DaysAgo(a, b) => {
                return (
                    local_ms(t - Duration::days(b)),
                    local_ms(t - Duration::days(a)) + DAY,
                )
            }
            Window::LastMonth => {
                let first = ymd(t.year(), t.month(), 1);
                (add_months(first, -1), first)
            }
            Window::LastSpring => last_season(t, 3),
            Window::LastSummer => last_season(t, 6),
            Window::LastWinter => last_season(t, 12),
            Window::ThisYear => (ymd(t.year(), 1, 1), ymd(t.year() + 1, 1, 1)),
            Window::LastYear => (ymd(t.year() - 1, 1, 1), ymd(t.year(), 1, 1)),
            Window::Abs(a, b) => return (a, b),
            Window::LastQuarter => {
                let q0 = ymd(t.year(), (t.month0() / 3) * 3 + 1, 1);
                (add_months(q0, -3), q0)
            }
            Window::Month(m) => {
                let y = if m < t.month() {
                    t.year()
                } else {
                    t.year() - 1
                };
                let s = ymd(y, m, 1);
                (s, add_months(s, 1))
            }
        };
        (local_ms(a), local_ms(b))
    }

    pub fn contains(self, now: i64, ms: i64) -> bool {
        let (a, b) = self.range(now);
        ms >= a && ms < b
    }

    /// A time well inside the window (not in its first or last day), during
    /// office hours, and never in the future.
    pub fn pick(self, now: i64, rng: &mut Rng) -> i64 {
        let (a, b) = self.range(now);
        let b = b.min(now - DAY);
        let (lo, hi) = if b - a > 3 * DAY {
            (a + DAY, b - DAY)
        } else {
            (a, b.max(a + 1))
        };
        let day = lo + rng.range(0, ((hi - lo) / DAY).max(0)) * DAY;
        let day_start = local_ms(today(day));
        day_start + rng.range(8 * 3_600_000, 18 * 3_600_000)
    }
}

/// Local midnight of `now`'s day (the corpus anchor: runs on the same day
/// generate the same mailbox).
pub fn day_anchor(now: i64) -> i64 {
    local_ms(today(now)) + 12 * 3_600_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_spring_and_month_are_calendar_windows() {
        // 2026-09-26 12:00 local.
        let now = local_ms(ymd(2026, 9, 26)) + 12 * 3_600_000;
        let (a, b) = Window::LastSpring.range(now);
        assert_eq!(
            (a, b),
            (local_ms(ymd(2026, 3, 1)), local_ms(ymd(2026, 6, 1)))
        );
        let (a, b) = Window::LastMonth.range(now);
        assert_eq!(
            (a, b),
            (local_ms(ymd(2026, 8, 1)), local_ms(ymd(2026, 9, 1)))
        );
        let (a, _) = Window::LastWinter.range(now);
        assert_eq!(a, local_ms(ymd(2025, 12, 1)));
        let (a, _) = Window::Month(10).range(now);
        assert_eq!(a, local_ms(ymd(2025, 10, 1)));
        let (a, b) = Window::LastQuarter.range(now);
        assert_eq!((a, b), (local_ms(ymd(2026, 4, 1)), local_ms(ymd(2026, 7, 1))));
        let mut r = Rng::new(1);
        for _ in 0..200 {
            let t = Window::LastSpring.pick(now, &mut r);
            assert!(Window::LastSpring.contains(now, t));
        }
    }
}
