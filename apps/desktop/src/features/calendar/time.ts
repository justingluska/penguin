// Calendar helpers: day math, agenda grouping, week-grid overlap layout,
// month-grid lanes, relative times and a text-only linkifier. Pure functions, no DOM or
// state, so tests/calendar.test.ts runs them under node.
import type { CalendarEvent } from "../../lib/types";

export const MIN = 60_000;
export const HOUR = 60 * MIN;
export const DAY = 24 * HOUR;

/** Local midnight of the day containing `ms`. */
export function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** Local midnight `n` calendar days after `dayStart` (DST-safe). */
export function addDays(dayStart: number, n: number): number {
  const d = new Date(dayStart);
  d.setDate(d.getDate() + n);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** Monday 00:00 of the week containing `ms`. */
export function startOfWeek(ms: number): number {
  const day = startOfDay(ms);
  const dow = (new Date(day).getDay() + 6) % 7; // Mon = 0
  return addDays(day, -dow);
}

/** Local midnight on the 1st of the month containing `ms`. */
export function startOfMonth(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  d.setDate(1);
  return d.getTime();
}

/** The same day `n` months later, clamped to that month (Jan 31 + 1 → Feb 28). */
export function addMonths(day: number, n: number): number {
  const d = new Date(startOfDay(day));
  const want = d.getDate();
  d.setDate(1);
  d.setMonth(d.getMonth() + n);
  const last = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
  d.setDate(Math.min(want, last));
  return d.getTime();
}

/**
 * Month grid rows: the week starts (Monday, as the week view) of every week
 * that touches the month of `ms`. 4 to 6 rows; days outside the month fill
 * the first and last rows.
 */
export function monthWeeks(ms: number): number[] {
  const first = startOfMonth(ms);
  const next = addMonths(first, 1);
  const out: number[] = [];
  for (let w = startOfWeek(first); w < next; w = addDays(w, 7)) out.push(w);
  return out;
}

/** Days an event touches, as local-midnight starts (all-day end is exclusive). */
export function eventDays(e: CalendarEvent): number[] {
  const first = startOfDay(e.start);
  // An event ending exactly at midnight doesn't touch the next day.
  const lastMoment = Math.max(e.start, e.end - 1);
  const last = startOfDay(lastMoment);
  const out: number[] = [];
  for (let d = first; d <= last && out.length < 62; d = addDays(d, 1)) out.push(d);
  return out;
}

/** Busy for "next up" and conflicts: not declined, not free. */
export function isBusy(e: CalendarEvent): boolean {
  return e.myResponse !== "declined" && !e.free;
}

export interface DayGroup {
  day: number;
  events: CalendarEvent[];
}

/**
 * Agenda days from `from` (a local midnight) for `days` days: each day
 * lists the events touching it, all-day first, then by start. Empty days
 * are left out.
 */
export function groupByDay(events: CalendarEvent[], from: number, days: number): DayGroup[] {
  const buckets = new Map<number, CalendarEvent[]>();
  const end = addDays(from, days);
  for (const e of events) {
    for (const d of eventDays(e)) {
      if (d < from || d >= end) continue;
      let b = buckets.get(d);
      if (!b) buckets.set(d, (b = []));
      b.push(e);
    }
  }
  return [...buckets.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([day, evs]) => ({ day, events: evs.sort(byAllDayThenStart) }));
}

export function byAllDayThenStart(a: CalendarEvent, b: CalendarEvent): number {
  if (a.allDay !== b.allDay) return a.allDay ? -1 : 1;
  return a.start - b.start || a.end - b.end || a.summary.localeCompare(b.summary);
}

/** The same meeting on two of your calendars shows once. */
export function dedupe(events: CalendarEvent[]): CalendarEvent[] {
  const seen = new Set<string>();
  return events.filter((e) => {
    const k = `${e.icalUid ?? e.accountId + "/" + e.id}@${e.start}`;
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

export interface Placed {
  event: CalendarEvent;
  /** Column within its overlap cluster, and how many columns it has. */
  col: number;
  cols: number;
  /** Minutes from the day's midnight, clipped to the day. */
  top: number;
  height: number;
}

/**
 * Side-by-side layout for one day's timed events: events that overlap
 * share a cluster and each takes the first free column.
 */
export function layoutDay(events: CalendarEvent[], dayStart: number): Placed[] {
  const dayEnd = addDays(dayStart, 1);
  const timed = events
    .filter((e) => !e.allDay && e.end > dayStart && e.start < dayEnd)
    .sort((a, b) => a.start - b.start || b.end - a.end);
  const out: Placed[] = [];
  let cluster: Placed[] = [];
  let colEnds: number[] = [];
  let clusterEnd = -Infinity;
  const flush = () => {
    for (const p of cluster) p.cols = colEnds.length;
    cluster = [];
    colEnds = [];
  };
  for (const e of timed) {
    const s = Math.max(e.start, dayStart);
    // Short events still get a readable block (15 minutes).
    const en = Math.max(Math.min(e.end, dayEnd), s + 15 * MIN);
    if (s >= clusterEnd) {
      flush();
      clusterEnd = -Infinity;
    }
    let col = colEnds.findIndex((end) => end <= s);
    if (col < 0) {
      col = colEnds.length;
      colEnds.push(en);
    } else colEnds[col] = en;
    clusterEnd = Math.max(clusterEnd, en);
    const p: Placed = { event: e, col, cols: 1, top: (s - dayStart) / MIN, height: (en - s) / MIN };
    cluster.push(p);
    out.push(p);
  }
  flush();
  return out;
}

export interface MonthSlot {
  event: CalendarEvent;
  /** Weekday column (0 = Monday) and how many columns it covers this week. */
  col: number;
  span: number;
  /** Row inside the week, shared by every day it covers. */
  lane: number;
  /** All-day or multi-day: a filled bar; else a dot + time line. */
  bar: boolean;
  /** Continues from the previous week / into the next one. */
  before: boolean;
  after: boolean;
}

/**
 * One month-grid week: bars (all-day and multi-day events) take the lowest
 * lanes, longest first, then each day's timed events in start order. A lane
 * is free only if it is free on every day the event covers.
 */
export function layoutMonthWeek(events: CalendarEvent[], weekStart: number): MonthSlot[] {
  const weekEnd = addDays(weekStart, 7);
  const items = events
    .map((e) => {
      // First and last day it touches (as eventDays, but without its cap, so
      // a months-long event still reaches every week).
      const first = startOfDay(e.start);
      const last = startOfDay(Math.max(e.start, e.end - 1));
      if (last < weekStart || first >= weekEnd) return null;
      // Rounded: a DST week has a 23- or 25-hour day.
      const col = Math.max(0, Math.round((first - weekStart) / DAY));
      const end = Math.min(6, Math.round((last - weekStart) / DAY));
      return { event: e, col, span: end - col + 1, lane: 0, bar: e.allDay || last > first, before: first < weekStart, after: last >= weekEnd };
    })
    .filter((x): x is MonthSlot => x !== null)
    .sort((a, b) => Number(b.bar) - Number(a.bar) || (a.bar ? a.col - b.col || b.span - a.span : 0) || byAllDayThenStart(a.event, b.event));
  const used: boolean[][] = Array.from({ length: 7 }, () => []);
  for (const s of items) {
    let lane = 0;
    while (used.slice(s.col, s.col + s.span).some((c) => c[lane])) lane++;
    s.lane = lane;
    for (let c = s.col; c < s.col + s.span; c++) used[c][lane] = true;
  }
  return items;
}

/**
 * Fit a week into `lanes` rows. A day that overflows gives its last row to
 * "+N more"; a bar shows only if its lane fits on every day it covers.
 * `more[col]` counts the hidden events touching each day.
 */
export function fitMonthWeek(slots: MonthSlot[], lanes: number): { shown: MonthSlot[]; more: number[] } {
  const depth = Array<number>(7).fill(0);
  for (const s of slots) for (let c = s.col; c < s.col + s.span; c++) depth[c] = Math.max(depth[c], s.lane + 1);
  const limit = depth.map((d) => (d <= lanes ? lanes : Math.max(0, lanes - 1)));
  const shown: MonthSlot[] = [];
  const more = Array<number>(7).fill(0);
  for (const s of slots) {
    const cols = Array.from({ length: s.span }, (_, i) => s.col + i);
    if (cols.every((c) => s.lane < limit[c])) shown.push(s);
    else for (const c of cols) more[c]++;
  }
  return { shown, more };
}

/** "in 25 min", "in 2 h 5 min", "now", "10 min ago", "3 days ago", "in 2 days". */
export function relative(ms: number, now: number): string {
  const diff = ms - now;
  const abs = Math.abs(diff);
  const future = diff > 0;
  if (abs < MIN) return "now";
  let s: string;
  if (abs < HOUR) s = `${Math.round(abs / MIN)} min`;
  else if (abs < DAY) {
    const h = Math.floor(abs / HOUR);
    const m = Math.round((abs - h * HOUR) / MIN);
    s = m ? `${h} h ${m} min` : `${h} h`;
  } else {
    const days = Math.round((startOfDay(ms) - startOfDay(now)) / DAY);
    const n = Math.abs(days) || 1;
    if (n === 1) return future ? "tomorrow" : "yesterday";
    s = n < 14 ? `${n} days` : n < 60 ? `${Math.round(n / 7)} weeks` : `${Math.round(n / 30)} months`;
  }
  return future ? `in ${s}` : `${s} ago`;
}

/** "30 min", "1 h", "1 h 30 min", "2 days". */
export function duration(e: CalendarEvent): string {
  if (e.allDay) {
    const days = Math.max(1, Math.round((e.end - e.start) / DAY));
    return days === 1 ? "All day" : `${days} days`;
  }
  const mins = Math.round((e.end - e.start) / MIN);
  if (mins < 60) return `${mins} min`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "short", day: "numeric" });
const shortDayFmt = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" });

export function timeOf(ms: number): string {
  return timeFmt.format(ms);
}

/** "Today", "Tomorrow", "Yesterday", else "Thursday, Oct 1". */
export function dayLabel(day: number, now: number): string {
  const today = startOfDay(now);
  if (day === today) return "Today";
  if (day === addDays(today, 1)) return "Tomorrow";
  if (day === addDays(today, -1)) return "Yesterday";
  return dayFmt.format(day);
}

/** "10:00 – 10:30 AM", "All day", "Thu, Oct 1 – Sat, Oct 3". */
export function timeRange(e: CalendarEvent): string {
  if (e.allDay) {
    const days = eventDays(e);
    return days.length > 1 ? `${shortDayFmt.format(days[0])} – ${shortDayFmt.format(days[days.length - 1])}` : "All day";
  }
  if (startOfDay(e.start) !== startOfDay(Math.max(e.start, e.end - 1)))
    return `${shortDayFmt.format(e.start)}, ${timeOf(e.start)} – ${shortDayFmt.format(e.end)}, ${timeOf(e.end)}`;
  return `${timeOf(e.start)} – ${timeOf(e.end)}`;
}

/** "Thu, Oct 1 · 10:00 – 10:30 AM" (with the day unless it is today). */
export function whenLabel(e: CalendarEvent, now: number): string {
  const day = startOfDay(e.start);
  const d = day === startOfDay(now) ? "Today" : day === addDays(startOfDay(now), 1) ? "Tomorrow" : shortDayFmt.format(e.start);
  if (e.allDay && eventDays(e).length > 1) return timeRange(e);
  return `${d} · ${e.allDay ? "All day" : timeRange(e)}`;
}

export type Segment = { text: string; href?: string };

/**
 * Split plain text into text and https links. Only https URLs become links;
 * everything is rendered as text nodes (never HTML).
 */
export function linkify(text: string): Segment[] {
  const out: Segment[] = [];
  const re = /https:\/\/[^\s<>"'()[\]]+/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const url = m[0].replace(/[.,;:!?]+$/, "");
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    out.push({ text: url, href: url });
    last = m.index + url.length;
    re.lastIndex = last;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** Location that is itself a link (no map button then). */
export function isUrl(s: string): boolean {
  return /^\s*https?:\/\//i.test(s);
}

export function mapUrl(location: string): string {
  return `https://maps.google.com/?q=${encodeURIComponent(location.trim())}`;
}

export function joinLabel(kind: CalendarEvent["conferenceKind"]): string {
  switch (kind) {
    case "meet":
      return "Join Google Meet";
    case "zoom":
      return "Join Zoom";
    case "teams":
      return "Join Teams";
    case "webex":
      return "Join Webex";
    default:
      return "Join call";
  }
}

/**
 * The status bar's events (busy, timed): what's running now, most recently
 * started first, and the next one starting within `horizon`. While something
 * runs, `next` is kept only when it starts before a running event ends or
 * within the hour, so a long block (a flight, a focus day) never hides the
 * meeting that starts in the middle of it.
 */
export function nextUp(
  events: CalendarEvent[],
  now: number,
  horizon = 12 * HOUR,
): { running: CalendarEvent[]; next: CalendarEvent | null } | null {
  const timed = events.filter((e) => !e.allDay && isBusy(e)).sort((a, b) => a.start - b.start);
  const running = timed.filter((e) => e.start <= now && e.end > now).reverse();
  let next = timed.find((e) => e.start > now && e.start - now <= horizon) ?? null;
  if (next && running.length) {
    const lastEnd = Math.max(...running.map((e) => e.end));
    if (next.start >= lastEnd && next.start - now > HOUR) next = null;
  }
  return running.length || next ? { running, next } : null;
}
