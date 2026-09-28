//! Scheduled rules: daily or weekly at a local time. A slot missed while
//! the app was closed runs once on the next launch (not once per missed
//! slot).

use chrono::{Datelike, Duration, LocalResult, NaiveDate, TimeZone};

use super::model::{parse_hhmm, Every, Trigger};

/// The instant `minutes` after local midnight on `day`.
fn at<Tz: TimeZone>(tz: &Tz, day: NaiveDate, minutes: u32) -> Option<i64> {
    let naive = day.and_hms_opt(minutes / 60, minutes % 60, 0)?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => Some(t.timestamp_millis()),
        // In a DST gap: run an hour later, when the wall clock exists again.
        LocalResult::None => tz
            .from_local_datetime(&(naive + Duration::hours(1)))
            .earliest()
            .map(|t| t.timestamp_millis()),
    }
}

fn is_slot_day(every: Every, weekday: Option<u8>, day: NaiveDate) -> bool {
    match every {
        Every::Daily => true,
        Every::Weekly => weekday == Some(day.weekday().num_days_from_sunday() as u8),
    }
}

/// (most recent slot ≤ now, next slot > now), unix ms. None if the trigger
/// isn't a valid schedule.
pub fn slots<Tz: TimeZone>(trigger: &Trigger, now_ms: i64, tz: &Tz) -> Option<(Option<i64>, i64)> {
    let Trigger::Schedule {
        every,
        at: hhmm,
        weekday,
    } = trigger
    else {
        return None;
    };
    let minutes = parse_hhmm(hhmm)?;
    let today = tz.timestamp_millis_opt(now_ms).single()?.date_naive();
    let mut prev = None;
    let mut next = None;
    for offset in -8i64..=8 {
        let day = today + Duration::days(offset);
        if !is_slot_day(*every, *weekday, day) {
            continue;
        }
        let Some(t) = at(tz, day, minutes) else {
            continue;
        };
        if t <= now_ms {
            prev = Some(t);
        } else if next.is_none() {
            next = Some(t);
        }
    }
    Some((prev, next?))
}

/// Whether a scheduled rule should run now: a slot has passed since it last
/// ran (or since it was saved, so saving at 09:00 doesn't fire an 08:00 rule).
pub fn is_due<Tz: TimeZone>(
    trigger: &Trigger,
    last_run: Option<i64>,
    saved_at: i64,
    now_ms: i64,
    tz: &Tz,
) -> bool {
    let since = last_run.unwrap_or(saved_at).max(saved_at);
    matches!(slots(trigger, now_ms, tz), Some((Some(prev), _)) if prev > since)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    const HOUR: i64 = 3_600_000;

    fn ms(tz: &FixedOffset, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn daily_and_weekly_slots() {
        let tz = FixedOffset::west_opt(5 * 3600).unwrap();
        let daily = Trigger::Schedule {
            every: Every::Daily,
            at: "08:00".into(),
            weekday: None,
        };
        // Wed 2026-09-23 10:30 local.
        let now = ms(&tz, 2026, 9, 23, 10, 30);
        let (prev, next) = slots(&daily, now, &tz).unwrap();
        assert_eq!(prev, Some(ms(&tz, 2026, 9, 23, 8, 0)));
        assert_eq!(next, ms(&tz, 2026, 9, 24, 8, 0));

        // Mondays (weekday 1) at 07:15.
        let weekly = Trigger::Schedule {
            every: Every::Weekly,
            at: "07:15".into(),
            weekday: Some(1),
        };
        let (prev, next) = slots(&weekly, now, &tz).unwrap();
        assert_eq!(prev, Some(ms(&tz, 2026, 9, 21, 7, 15)));
        assert_eq!(next, ms(&tz, 2026, 9, 28, 7, 15));
    }

    #[test]
    fn due_once_per_slot_and_not_right_after_saving() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let t = Trigger::Schedule {
            every: Every::Daily,
            at: "08:00".into(),
            weekday: None,
        };
        let slot = ms(&tz, 2026, 9, 23, 8, 0);
        // Saved at 09:00: today's 08:00 already passed, so not due.
        assert!(!is_due(&t, None, slot + HOUR, slot + 2 * HOUR, &tz));
        // Saved yesterday, never ran: due.
        assert!(is_due(&t, None, slot - 20 * HOUR, slot + 1, &tz));
        // Ran at the slot: not due again until tomorrow's.
        assert!(!is_due(
            &t,
            Some(slot + 5),
            slot - 20 * HOUR,
            slot + 3 * HOUR,
            &tz
        ));
        assert!(is_due(
            &t,
            Some(slot + 5),
            slot - 20 * HOUR,
            slot + 24 * HOUR,
            &tz
        ));
        // App closed for three days: one run, not three.
        assert!(is_due(
            &t,
            Some(slot),
            slot - 20 * HOUR,
            slot + 72 * HOUR + 1,
            &tz
        ));
        assert!(!is_due(
            &t,
            Some(slot + 72 * HOUR + 1),
            slot - 20 * HOUR,
            slot + 72 * HOUR + 2,
            &tz
        ));
        assert!(!is_due(&Trigger::NewMessage, None, 0, slot, &tz));
    }
}
