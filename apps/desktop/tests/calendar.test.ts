// Calendar helpers (features/calendar/time.ts): agenda grouping, week-grid
// overlap layout, relative times, the text-only linkifier and "next up".
import { test } from "node:test";
import assert from "node:assert/strict";
import type { CalendarEvent } from "../src/lib/types.ts";
import {
  HOUR,
  MIN,
  addDays,
  addMonths,
  dedupe,
  eventDays,
  fitMonthWeek,
  groupByDay,
  layoutDay,
  layoutMonthWeek,
  linkify,
  monthWeeks,
  nextUp,
  relative,
  startOfDay,
  startOfMonth,
  startOfWeek,
} from "../src/features/calendar/time.ts";

const today = startOfDay(Date.now());

function ev(id: string, start: number, mins: number, extra: Partial<CalendarEvent> = {}): CalendarEvent {
  return {
    accountId: "a",
    calendarId: "c",
    id,
    icalUid: `${id}@x`,
    status: "confirmed",
    summary: id,
    description: "",
    location: "",
    start,
    end: start + mins * MIN,
    allDay: false,
    startDate: null,
    endDate: null,
    organizer: null,
    attendees: [],
    myResponse: null,
    htmlLink: null,
    conferenceUrl: null,
    conferenceKind: null,
    recurringEventId: null,
    free: false,
    updated: 0,
    ...extra,
  };
}

test("groups by local day, all-day first, multi-day events on each day", () => {
  const trip = ev("trip", today, 0, { allDay: true, end: addDays(today, 2) });
  const groups = groupByDay(
    [ev("late", today + 15 * HOUR, 30), ev("early", today + 9 * HOUR, 30), trip, ev("tomorrow", addDays(today, 1) + 10 * HOUR, 30), ev("gone", addDays(today, -1), 30)],
    today,
    7,
  );
  assert.deepEqual(
    groups.map((g) => [g.day, g.events.map((e) => e.id)]),
    [
      [today, ["trip", "early", "late"]],
      [addDays(today, 1), ["trip", "tomorrow"]],
    ],
  );
  // An event ending exactly at midnight stays on its day.
  assert.equal(eventDays(ev("x", today + 23 * HOUR, 60)).length, 1);
});

test("overlapping events share columns; separate clusters reset", () => {
  const placed = layoutDay(
    [ev("a", today + 9 * HOUR, 60), ev("b", today + 9 * HOUR + 30 * MIN, 60), ev("c", today + 10 * HOUR, 30), ev("d", today + 13 * HOUR, 30)],
    today,
  );
  const by = Object.fromEntries(placed.map((p) => [p.event.id, p]));
  assert.equal(by.a.col, 0);
  assert.equal(by.b.col, 1);
  assert.equal(by.c.col, 0, "a ended at 10:00, so c reuses its column");
  assert.equal(by.a.cols, 2);
  assert.equal(by.c.cols, 2);
  assert.equal(by.d.cols, 1);
  assert.equal(by.d.top, 13 * 60);
  // Short events get a readable minimum height.
  assert.equal(layoutDay([ev("s", today + 8 * HOUR, 5)], today)[0].height, 15);
});

test("relative times", () => {
  const now = today + 12 * HOUR;
  assert.equal(relative(now + 25 * MIN, now), "in 25 min");
  assert.equal(relative(now - 10 * MIN, now), "10 min ago");
  assert.equal(relative(now + 2 * HOUR + 5 * MIN, now), "in 2 h 5 min");
  assert.equal(relative(now + 20 * 1000, now), "now");
  assert.equal(relative(addDays(today, 3) + 12 * HOUR, now), "in 3 days");
  assert.equal(relative(addDays(today, -1) + 9 * HOUR, now), "yesterday");
});

test("linkify keeps text as text and only links https", () => {
  assert.deepEqual(linkify("Join https://meet.google.com/abc-defg. Or http://evil.example <b>x</b>"), [
    { text: "Join " },
    { text: "https://meet.google.com/abc-defg", href: "https://meet.google.com/abc-defg" },
    { text: ". Or http://evil.example <b>x</b>" },
  ]);
  assert.deepEqual(linkify("no links"), [{ text: "no links" }]);
  assert.deepEqual(linkify("javascript:alert(1)"), [{ text: "javascript:alert(1)" }]);
});

test("next up skips all-day, declined and free events; prefers the running one", () => {
  const now = today + 12 * HOUR;
  const events = [
    ev("allday", today, 0, { allDay: true, end: addDays(today, 1) }),
    ev("declined", now + 5 * MIN, 30, { myResponse: "declined" }),
    ev("free", now + 10 * MIN, 30, { free: true }),
    ev("sync", now + 25 * MIN, 30),
    ev("far", now + 13 * HOUR, 30),
  ];
  assert.deepEqual(nextUp(events, now), { running: [], next: events[3] });
  const running = ev("running", now - 10 * MIN, 30);
  const both = nextUp([...events, running], now);
  assert.deepEqual(both?.running.map((e) => e.id), ["running"]);
  assert.equal(both?.next?.id, "sync", "a meeting within the hour still shows beside the running one");
  assert.equal(nextUp([events[4]], now), null, "beyond 12 hours");
});

test("next up: a long running block keeps the meeting inside it; overlapping events all count", () => {
  const now = today + 10 * HOUR;
  const train = ev("train", now - 45 * MIN, 127);
  const meeting = ev("meeting", now + 70 * MIN, 30);
  const later = ev("later", now + 5 * HOUR, 30);
  assert.equal(nextUp([train, meeting], now)?.next?.id, "meeting", "starts before the train arrives");
  assert.equal(nextUp([train, later], now)?.next, null, "after the train and hours away");
  const call = ev("call", now - 5 * MIN, 30);
  assert.deepEqual(nextUp([train, call], now)?.running.map((e) => e.id), ["call", "train"], "latest started first");
});

test("the same meeting on two calendars shows once", () => {
  const a = ev("m", today, 30);
  assert.equal(dedupe([a, { ...a, accountId: "b", calendarId: "d" }]).length, 1);
});

test("month grid: Monday-first weeks covering the month, 4 to 6 rows", () => {
  const rows = (y: number, m: number) => monthWeeks(new Date(y, m, 15).getTime());
  // September 2026 starts on a Tuesday: Aug 31 – Oct 4, five rows.
  const sep = rows(2026, 8);
  assert.equal(sep.length, 5);
  assert.equal(sep[0], new Date(2026, 7, 31).getTime());
  assert.equal(addDays(sep[4], 6), new Date(2026, 9, 4).getTime());
  // August 2026 starts on a Saturday and has 31 days: six rows.
  assert.equal(rows(2026, 7).length, 6);
  // February 2027 starts on a Monday and has 28 days: exactly four.
  assert.equal(rows(2027, 1).length, 4);
  assert.equal(rows(2027, 1)[0], new Date(2027, 1, 1).getTime());
  for (const w of rows(2026, 2)) assert.equal(new Date(w).getDay(), 1, "every row starts on a Monday (DST month too)");
});

test("month math clamps to the month's length", () => {
  const d = (y: number, m: number, day: number) => new Date(y, m, day).getTime();
  assert.equal(startOfMonth(d(2026, 8, 25) + 13 * HOUR), d(2026, 8, 1));
  assert.equal(addMonths(d(2026, 0, 31), 1), d(2026, 1, 28));
  assert.equal(addMonths(d(2028, 0, 31), 1), d(2028, 1, 29), "leap year");
  assert.equal(addMonths(d(2026, 11, 15), 1), d(2027, 0, 15));
  assert.equal(addMonths(d(2026, 0, 15), -1), d(2025, 11, 15));
});

test("month week: bars share lanes across days, timed events fill in below", () => {
  const mon = startOfWeek(new Date(2026, 8, 21).getTime());
  const day = (n: number) => addDays(mon, n);
  const slots = layoutMonthWeek(
    [
      ev("standup", day(1) + 9 * HOUR, 15),
      ev("offsite", day(1), 0, { allDay: true, end: day(3) }),
      ev("launch", day(2), 0, { allDay: true, end: day(3) }),
      // Timed, Sat 22:00 → Mon 02:00: a bar into next week.
      ev("redeye", day(5) + 22 * HOUR, 28 * 60),
      // All-day from last week into this one.
      ev("trip", addDays(mon, -2), 0, { allDay: true, end: day(1) }),
      ev("elsewhere", addDays(mon, 7) + 9 * HOUR, 30),
    ],
    mon,
  );
  const by = Object.fromEntries(slots.map((s) => [s.event.id, s]));
  assert.equal(by.elsewhere, undefined);
  assert.deepEqual([by.trip.col, by.trip.span, by.trip.before, by.trip.after, by.trip.lane], [0, 1, true, false, 0]);
  assert.deepEqual([by.offsite.col, by.offsite.span, by.offsite.lane], [1, 2, 0]);
  assert.equal(by.launch.lane, 1, "Wednesday's lane 0 is the offsite");
  assert.deepEqual([by.redeye.col, by.redeye.span, by.redeye.bar, by.redeye.after], [5, 2, true, true]);
  assert.equal(by.standup.bar, false);
  assert.equal(by.standup.lane, 1, "under the offsite bar");
});

test("month week: an overflowing day gives its last line to +N more", () => {
  const mon = startOfWeek(new Date(2026, 8, 21).getTime());
  const tue = addDays(mon, 1);
  const busy = [9, 10, 11, 13, 15].map((h) => ev(`m${h}`, tue + h * HOUR, 30));
  const wide = ev("conf", mon, 0, { allDay: true, end: addDays(mon, 3) });
  const slots = layoutMonthWeek([...busy, wide], mon);
  // Plenty of room: everything shows.
  assert.deepEqual(fitMonthWeek(slots, 6).more, [0, 0, 0, 0, 0, 0, 0]);
  // Three lines: Tuesday has six items, so two show and "+4 more".
  const { shown, more } = fitMonthWeek(slots, 3);
  assert.deepEqual(more, [0, 4, 0, 0, 0, 0, 0]);
  assert.deepEqual(shown.map((s) => s.event.id), ["conf", "m9"]);
  // A bar whose lane is cut on one of its days is hidden on all of them.
  const { shown: tight, more: tightMore } = fitMonthWeek(slots, 1);
  assert.deepEqual(tight, []);
  assert.deepEqual(tightMore, [1, 6, 1, 0, 0, 0, 0]);
});
