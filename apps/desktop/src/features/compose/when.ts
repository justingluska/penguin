// "Send later" times typed in words: "tomorrow 9am", "fri 3pm", "in 2 hours",
// "monday", "oct 3 10:30", "10/3 4pm", "tonight". English, local time.
// Pure, so tests/composeSpeed.test.ts pins it to a clock.

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

export interface SendLaterPreset {
  id: "hour" | "today" | "tomorrow" | "tomorrowAfternoon" | "monday";
  label: string;
  at: number;
}

function atHour(base: Date, dayOffset: number, hour: number): number {
  const d = new Date(base);
  d.setDate(d.getDate() + dayOffset);
  d.setHours(hour, 0, 0, 0);
  return d.getTime();
}

/**
 * In 1 hour · This afternoon (5 PM, until 4 PM) · Tomorrow morning ·
 * Tomorrow afternoon (1 PM) · Monday morning (not on a Sunday, when it's
 * tomorrow): Gmail's and Apple Mail's choices. Mornings are at
 * `morningHour` (Settings → Compose, default 8). In time order; 1–5 pick
 * them in the menu.
 */
export function sendLaterPresets(now = new Date(), morningHour = 8): SendLaterPreset[] {
  const out: SendLaterPreset[] = [];
  out.push({ id: "hour", label: "In 1 hour", at: Math.ceil((now.getTime() + HOUR) / 300_000) * 300_000 });
  if (now.getHours() < 16) out.push({ id: "today", label: "This afternoon", at: atHour(now, 0, 17) });
  out.push({ id: "tomorrow", label: "Tomorrow morning", at: atHour(now, 1, morningHour) });
  out.push({ id: "tomorrowAfternoon", label: "Tomorrow afternoon", at: atHour(now, 1, 13) });
  const toMonday = ((8 - now.getDay()) % 7) || 7;
  if (toMonday > 1) out.push({ id: "monday", label: "Monday morning", at: atHour(now, toMonday, morningHour) });
  return out;
}

const WEEKDAYS = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];
const MONTHS = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

function weekday(word: string): number {
  const w = word.toLowerCase().replace(/\.$/, "");
  if (w.length < 2) return -1;
  // "thurs", "tues", "weds" as well as the usual three letters.
  const alias: Record<string, number> = { tues: 2, weds: 3, thur: 4, thurs: 4 };
  if (w in alias) return alias[w];
  return WEEKDAYS.findIndex((d) => d.startsWith(w) && w.length >= 3);
}

function month(word: string): number {
  const w = word.toLowerCase().replace(/\.$/, "");
  if (w.length < 3) return -1;
  if (w === "sept") return 8;
  return MONTHS.findIndex((m) => m.startsWith(w));
}

/** "9", "9am", "9:30", "9:30pm", "21:00", "noon", "midnight" → [hour, minute], or null. */
function parseTime(s: string): [number, number] | null {
  const t = s.trim().toLowerCase().replace(/\s+/g, "").replace(/\./g, "");
  if (t === "noon") return [12, 0];
  if (t === "midnight") return [0, 0];
  const m = /^(\d{1,2})(?::(\d{2}))?(am|pm|a|p)?$/.exec(t);
  if (!m) return null;
  let h = Number(m[1]);
  const min = m[2] ? Number(m[2]) : 0;
  if (min > 59) return null;
  const ap = m[3]?.[0];
  if (ap) {
    if (h < 1 || h > 12) return null;
    if (ap === "p" && h < 12) h += 12;
    if (ap === "a" && h === 12) h = 0;
  } else if (h > 23) return null;
  else if (!m[2] && h >= 1 && h <= 7) h += 12; // "send at 3" means the afternoon
  return [h, min];
}

const PARTS: Record<string, number> = { morning: -1, afternoon: 13, evening: 18, tonight: 21, night: 21 };

/**
 * The time `text` means, as unix ms, or null when it can't be read. A day
 * without a time is `morningHour`; a time without a day is today if still
 * ahead, else tomorrow; a weekday is the next one after today.
 */
export function parseWhen(text: string, now = new Date(), morningHour = 8): number | null {
  let s = text.trim().toLowerCase().replace(/[,]+/g, " ").replace(/\s+/g, " ");
  if (!s) return null;
  s = s.replace(/^(at|on)\s+/, "");

  const rel = /^in (\d+|a|an|one|half an?) ?(m|min|mins|minutes?|h|hr|hrs|hours?|d|days?|w|wks?|weeks?)$/.exec(s);
  if (rel) {
    const n = /^\d+$/.test(rel[1]) ? Number(rel[1]) : rel[1].startsWith("half") ? 0.5 : 1;
    const u = rel[2][0];
    const ms = u === "m" ? MIN : u === "h" ? HOUR : u === "d" ? DAY : 7 * DAY;
    if (n <= 0) return null;
    return Math.ceil((now.getTime() + n * ms) / MIN) * MIN;
  }

  const words = s.split(" ");
  let day: Date | null = null;
  let time: [number, number] | null = null;
  let part: number | null = null;
  const rest: string[] = [];

  for (let i = 0; i < words.length; i++) {
    const w = words[i];
    const next = words[i + 1];
    if (w === "today") day = new Date(now);
    else if (w === "tonight") {
      day = new Date(now);
      part = PARTS.tonight;
    } else if (w === "tomorrow" || w === "tmrw" || w === "tmr" || w === "tom") {
      day = new Date(now);
      day.setDate(day.getDate() + 1);
    } else if ((w === "next" || w === "this") && next && weekday(next) >= 0) {
      day = nextWeekday(now, weekday(next), w === "next" && weekday(next) > now.getDay() ? 7 : 0);
      i += 1;
    } else if (w === "next" && next === "week") {
      day = nextWeekday(now, 1, 0);
      i += 1;
    } else if (weekday(w) >= 0) day = nextWeekday(now, weekday(w), 0);
    else if (month(w) >= 0 && next && /^\d{1,2}(st|nd|rd|th)?$/.test(next)) {
      day = datedDay(now, month(w), parseInt(next, 10));
      i += 1;
    } else if (/^\d{1,2}(st|nd|rd|th)?$/.test(w) && next && month(next) >= 0) {
      day = datedDay(now, month(next), parseInt(w, 10));
      i += 1;
    } else if (/^\d{1,2}\/\d{1,2}$/.test(w)) {
      const [mo, d] = w.split("/").map(Number);
      day = datedDay(now, mo - 1, d);
    } else if (w in PARTS) part = PARTS[w];
    else if (w === "at") continue;
    else rest.push(w);
  }

  if (rest.length) {
    // What's left must be one time ("3pm", "3 pm", "9:30").
    time = parseTime(rest.join(""));
    if (!time) return null;
  }
  if (!day && !time && part === null) return null;
  if (day && day.getTime() === -1) return null;

  const hour = time ? time[0] : part !== null ? (part === -1 ? morningHour : part) : morningHour;
  const minute = time ? time[1] : 0;
  const base = day ?? new Date(now);
  const at = new Date(base.getFullYear(), base.getMonth(), base.getDate(), hour, minute, 0, 0);
  if (!day && at.getTime() <= now.getTime()) at.setDate(at.getDate() + 1);
  return at.getTime();
}

/** The next `dow` after today (a week on when `extra` is 7). */
function nextWeekday(now: Date, dow: number, extra: number): Date {
  const d = new Date(now);
  const ahead = ((dow - now.getDay() + 7) % 7) || 7;
  d.setDate(d.getDate() + ahead + extra);
  return d;
}

/** That date this year, or next year once it's past; an impossible date is marked with -1. */
function datedDay(now: Date, mo: number, d: number): Date {
  if (mo < 0 || mo > 11 || d < 1 || d > 31) return new Date(-1);
  let out = new Date(now.getFullYear(), mo, d);
  if (out.getMonth() !== mo) return new Date(-1);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  if (out.getTime() < today.getTime()) out = new Date(now.getFullYear() + 1, mo, d);
  return out;
}
