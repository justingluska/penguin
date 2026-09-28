// Snooze times: the picker's presets and how a wake time reads ("Tue 8:00 AM").
// Pure (no DOM, no app state) so tests/snooze.test.ts can pin them to a clock.
// Local time throughout; Gmail's defaults: mornings at 8, evenings at 6.

export interface SnoozePreset {
  id: "later" | "tomorrow" | "weekend" | "nextWeek";
  label: string;
  /** Unix ms. */
  at: number;
}

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
export const MORNING_HOUR = 8;
export const EVENING_HOUR = 18;
/** Snoozes further out than this are refused (the backend's limit). */
export const MAX_AHEAD_MS = 366 * DAY;

function at(base: Date, dayOffset: number, hour: number): number {
  const d = new Date(base);
  d.setDate(d.getDate() + dayOffset);
  d.setHours(hour, 0, 0, 0);
  return d.getTime();
}

/** "Later today": 6 PM while that's over an hour away, else ~3 hours on (on the hour), none after 9 PM. */
function laterToday(now: Date): number | null {
  const evening = at(now, 0, EVENING_HOUR);
  if (evening - now.getTime() > HOUR) return evening;
  if (now.getHours() >= 21) return null;
  const d = new Date(now.getTime() + 3 * HOUR);
  if (d.getMinutes() > 0 || d.getSeconds() > 0 || d.getMilliseconds() > 0) d.setHours(d.getHours() + 1, 0, 0, 0);
  return d.getDate() === now.getDate() ? d.getTime() : null;
}

/**
 * Later today · Tomorrow morning · This weekend (Saturday) · Next week (Monday),
 * in time order, all in the future. A preset landing on the same time as an
 * earlier one is dropped (on a Friday, "This weekend" is tomorrow morning).
 */
export function snoozePresets(now = new Date()): SnoozePreset[] {
  const out: SnoozePreset[] = [];
  const later = laterToday(now);
  if (later !== null) out.push({ id: "later", label: "Later today", at: later });
  out.push({ id: "tomorrow", label: "Tomorrow morning", at: at(now, 1, MORNING_HOUR) });
  const dow = now.getDay(); // 0 Sun … 6 Sat
  if (dow >= 1 && dow <= 5) out.push({ id: "weekend", label: "This weekend", at: at(now, 6 - dow, MORNING_HOUR) });
  const toMonday = ((8 - dow) % 7) || 7;
  out.push({ id: "nextWeek", label: "Next week", at: at(now, toMonday, MORNING_HOUR) });
  const seen = new Set<number>();
  return out.filter((p) => p.at > now.getTime() && !seen.has(p.at) && (seen.add(p.at), true));
}

/** The default when the picker opens without a choice: tomorrow at 8 AM. */
export function defaultCustom(now = new Date()): number {
  return at(now, 1, MORNING_HOUR);
}

/** Why a custom time can't be used, or null when it can. */
export function customError(ms: number, now = Date.now()): string | null {
  if (Number.isNaN(ms)) return "Pick a date and time.";
  if (ms <= now) return "Pick a time in the future.";
  if (ms > now + MAX_AHEAD_MS) return "Pick a time within a year.";
  return null;
}

/** `<input type=datetime-local>` value for a unix ms time (local clock). */
export function toLocalInput(ms: number): string {
  const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60_000);
  return d.toISOString().slice(0, 16);
}

/** The inverse of toLocalInput ("2026-09-28T08:00" → local unix ms); NaN if unreadable. */
export function fromLocalInput(v: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(v.trim());
  if (!m) return NaN;
  return new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5]).getTime();
}

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** Whole local days from `now`'s day to `ms`'s day (DST-safe). */
function dayDiff(ms: number, now: number): number {
  return Math.round((startOfDay(ms) - startOfDay(now)) / DAY);
}

/**
 * A wake time, short: "6:00 PM" (today), "Tomorrow 8:00 AM", "Sat 8:00 AM"
 * (within the week), "Oct 12, 8:00 AM", "Jan 3, 2027, 8:00 AM" (another year).
 */
export function fmtWake(ms: number, now = Date.now(), locale?: string): string {
  const time = new Date(ms).toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" });
  const days = dayDiff(ms, now);
  if (days === 0) return time;
  if (days === 1) return `Tomorrow ${time}`;
  if (days > 1 && days < 7) return `${new Date(ms).toLocaleDateString(locale, { weekday: "short" })} ${time}`;
  const sameYear = new Date(ms).getFullYear() === new Date(now).getFullYear();
  const date = new Date(ms).toLocaleDateString(locale, sameYear ? { month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" });
  return `${date}, ${time}`;
}

/** The Snoozed view's group headers, by wake day: Today · Tomorrow · This week · Later. */
export function wakeGroup(ms: number, now = Date.now()): string {
  const days = dayDiff(ms, now);
  if (days <= 0) return "Today";
  if (days === 1) return "Tomorrow";
  if (days < 7) return "This week";
  return "Later";
}

/** "until Tue 8:00 AM" / "until 6:00 PM" (the Snoozed view and toasts). */
export function fmtUntil(ms: number, now = Date.now(), locale?: string): string {
  const s = fmtWake(ms, now, locale);
  return `until ${s.startsWith("Tomorrow") ? "t" + s.slice(1) : s}`;
}
