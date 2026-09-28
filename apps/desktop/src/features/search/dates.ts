// Natural-language dates for the `date:` operator, mirroring penguin-core's
// dates.rs closely enough for the UI: the "Use 'february' as a date?" hint,
// `date:` autocomplete, and the mock backend's ranges. The backend is the
// authority for real searches.
//
// Free text is never a date: only `date:value` is (plus before:/after:).

const MONTHS = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
const MONTH_ABBR: Record<string, number> = { jan: 0, feb: 1, mar: 2, apr: 3, jun: 5, jul: 6, aug: 7, sep: 8, sept: 8, oct: 9, nov: 10, dec: 11 };
const WEEKDAYS: Record<string, number> = {
  sunday: 0, monday: 1, tuesday: 2, tues: 2, wednesday: 3, weds: 3, thursday: 4, thurs: 4, friday: 5, saturday: 6,
};
const DAY_MS = 86_400_000;

function month(w: string): number | null {
  const i = MONTHS.indexOf(w);
  if (i !== -1) return i;
  return MONTH_ABBR[w] ?? null;
}

function year(w: string | undefined): number | null {
  if (!w) return null;
  if (/^\d{4}$/.test(w)) return +w;
  const m = /^['’](\d{2})$/.exec(w);
  return m ? 2000 + +m[1] : null;
}

/** [from, to) in local unix ms; `to` null = open-ended ("since"). */
export interface DateRange {
  from: number | null;
  to: number | null;
}

const d = (y: number, m: number, day = 1) => new Date(y, m, day).getTime();

/** Most recent occurrence of month m (this year if it has started). */
function recentMonthYear(m: number, now: Date): number {
  return m <= now.getMonth() ? now.getFullYear() : now.getFullYear() - 1;
}

function monthRange(y: number, m: number): DateRange {
  return { from: d(y, m), to: d(y, m + 1) };
}

function dayOf(w: string): number | null {
  const m = /^(\d{1,2})(?:st|nd|rd|th)?,?$/.exec(w);
  return m && +m[1] >= 1 && +m[1] <= 31 ? +m[1] : null;
}

/** One date form (no since/between/to). `words` is lowercased and split. */
function single(words: string[], now: Date, iso: boolean): DateRange | null {
  const s = words.join(" ");
  const y = now.getFullYear();
  const today = d(y, now.getMonth(), now.getDate());
  const tomorrow = today + DAY_MS;
  const n = words.length;
  if (s === "today") return { from: today, to: tomorrow };
  if (s === "yesterday") return { from: today - DAY_MS, to: today };
  if (s === "past 12 months") return { from: today - 365 * DAY_MS, to: tomorrow };
  let m: RegExpExecArray | null;
  if ((m = /^(this|last|past) (week|month|year)$/.exec(s))) {
    const [, which, unit] = m;
    if (unit === "week") {
      const monday = today - ((now.getDay() + 6) % 7) * DAY_MS;
      if (which === "this") return { from: monday, to: tomorrow };
      if (which === "last") return { from: monday - 7 * DAY_MS, to: monday };
      return { from: today - 7 * DAY_MS, to: tomorrow };
    }
    if (unit === "month") {
      if (which === "this") return { from: d(y, now.getMonth()), to: tomorrow };
      if (which === "last") return monthRange(y, now.getMonth() - 1);
      return { from: today - 30 * DAY_MS, to: tomorrow };
    }
    if (which === "this") return { from: d(y, 0), to: tomorrow };
    if (which === "last") return { from: d(y - 1, 0), to: d(y, 0) };
    return { from: today - 365 * DAY_MS, to: tomorrow };
  }
  if ((m = /^(this|last) (spring|summer|fall|autumn|winter)$/.exec(s))) {
    const start: Record<string, number> = { spring: 2, summer: 5, fall: 8, autumn: 8, winter: 11 };
    const sm = start[m[2]];
    // The most recent season that has started; "last" = the one before it.
    let sy = sm <= now.getMonth() ? y : y - 1;
    if (m[2] === "winter" && now.getMonth() < 2) sy = y - 1;
    if (m[1] === "last" && d(sy, sm + 3) > now.getTime()) sy -= 1;
    return { from: d(sy, sm), to: d(sy, sm + 3) };
  }
  if ((m = /^(?:last|past) (\d+) (day|week|month|year)s?$/.exec(s))) {
    const days = { day: 1, week: 7, month: 30, year: 365 }[m[2] as "day"];
    return { from: today - +m[1] * days * DAY_MS, to: tomorrow };
  }
  if ((m = /^(\d+) (day|week|month|year)s? ago$/.exec(s))) {
    const days = { day: 1, week: 7, month: 30, year: 365 }[m[2] as "day"];
    const at = today - +m[1] * days * DAY_MS;
    return { from: at, to: at + DAY_MS };
  }
  // Weekends of Monday-first weeks; quarters.
  if ((m = /^(this|last|past) weekend$/.exec(s))) {
    const monday = today - ((now.getDay() + 6) % 7) * DAY_MS;
    const sat = m[1] === "this" ? monday + 5 * DAY_MS : monday - 2 * DAY_MS;
    return { from: sat, to: sat + 2 * DAY_MS };
  }
  if ((m = /^(this|last|past) quarter$/.exec(s))) {
    const q0 = Math.floor(now.getMonth() / 3) * 3;
    if (m[1] === "this") return { from: d(y, q0), to: d(y, q0 + 3) };
    if (m[1] === "last") return { from: d(y, q0 - 3), to: d(y, q0) };
    return { from: d(y, now.getMonth() - 3, now.getDate()), to: tomorrow };
  }
  if ((m = /^(?:in )?q ?([1-4])(?: (\d{4}|['’]\d{2}))?$/.exec(s))) {
    const qm = (+m[1] - 1) * 3;
    const qy = m[2] ? year(m[2])! : d(y, qm) <= now.getTime() ? y : y - 1;
    return { from: d(qy, qm), to: d(qy, qm + 3) };
  }
  // Day first: "18 august 2026", "18th of august".
  if ((m = /^(\d{1,2})(?:st|nd|rd|th)?,? (?:of )?([a-z]+)(?: (\d{4}|['’]\d{2}))?$/.exec(s)) && month(m[2]) !== null) {
    const mo = month(m[2])!;
    const day = +m[1];
    let yy = m[3] ? year(m[3])! : y;
    if (!m[3] && d(yy, mo, day) > now.getTime()) yy -= 1;
    return { from: d(yy, mo, day), to: d(yy, mo, day + 1) };
  }
  // Weekdays: the most recent one (today counts); "last" = the one before today.
  if (n === 1 && words[0] in WEEKDAYS) {
    const back = (now.getDay() - WEEKDAYS[words[0]] + 7) % 7;
    return { from: today - back * DAY_MS, to: today - back * DAY_MS + DAY_MS };
  }
  if (n === 2 && words[0] === "last" && words[1] in WEEKDAYS) {
    const back = (now.getDay() - WEEKDAYS[words[1]] + 7) % 7 || 7;
    return { from: today - back * DAY_MS, to: today - back * DAY_MS + DAY_MS };
  }
  // Months.
  if (n === 1 && month(words[0]) !== null) {
    const mo = month(words[0])!;
    return monthRange(recentMonthYear(mo, now), mo);
  }
  if (n === 2 && (words[0] === "last" || words[0] === "in") && month(words[1]) !== null) {
    const mo = month(words[1])!;
    const ry = recentMonthYear(mo, now);
    return monthRange(words[0] === "last" ? ry - 1 : ry, mo);
  }
  if (n === 2 && words[0] === "in" && /^\d{4}$/.test(words[1])) return { from: d(+words[1], 0), to: d(+words[1] + 1, 0) };
  if (n === 2 && month(words[0]) !== null && year(words[1]) !== null) return monthRange(year(words[1])!, month(words[0])!);
  if ((n === 2 || n === 3) && month(words[0]) !== null && dayOf(words[1]) !== null && (n === 2 || year(words[2]) !== null)) {
    const mo = month(words[0])!;
    const day = dayOf(words[1])!;
    let yy = n === 3 ? year(words[2])! : y;
    if (n === 2 && d(yy, mo, day) > now.getTime()) yy -= 1;
    return { from: d(yy, mo, day), to: d(yy, mo, day + 1) };
  }
  const part = (mo: number, yy: number | null, which: "early" | "mid" | "late"): DateRange => {
    const yr = yy ?? recentMonthYear(mo, now);
    if (which === "early") return { from: d(yr, mo, 1), to: d(yr, mo, 11) };
    if (which === "mid") return { from: d(yr, mo, 11), to: d(yr, mo, 21) };
    return { from: d(yr, mo, 21), to: d(yr, mo + 1) };
  };
  if ((n === 2 || n === 3) && /^(early|mid|late)$/.test(words[0]) && month(words[1]) !== null && (n === 2 || year(words[2]) !== null)) {
    return part(month(words[1])!, n === 3 ? year(words[2]) : null, words[0] as "early");
  }
  if ((n === 3 || n === 4) && /^(beginning|middle|end)$/.test(words[0]) && words[1] === "of" && month(words[2]) !== null && (n === 3 || year(words[3]) !== null)) {
    const which = { beginning: "early", middle: "mid", end: "late" }[words[0]] as "early";
    return part(month(words[2])!, n === 4 ? year(words[3]) : null, which);
  }
  const wk = words[0] === "the" ? words.slice(1) : words;
  if (wk[0] === "week" && wk[1] === "of" && wk.length >= 4) {
    const day = single(wk.slice(2), now, false);
    if (day && day.from !== null && day.to !== null && day.to - day.from <= DAY_MS) {
      const dt = new Date(day.from);
      const monday = day.from - ((dt.getDay() + 6) % 7) * DAY_MS;
      return { from: monday, to: monday + 7 * DAY_MS };
    }
  }
  if (iso && n === 1) {
    if ((m = /^(\d{4})$/.exec(s))) return { from: d(+m[1], 0), to: d(+m[1] + 1, 0) };
    if ((m = /^(\d{4})-(\d{2})$/.exec(s))) return monthRange(+m[1], +m[2] - 1);
    if ((m = /^(\d{4})[-/](\d{2})[-/](\d{2})$/.exec(s))) return { from: d(+m[1], +m[2] - 1, +m[3]), to: d(+m[1], +m[2] - 1, +m[3] + 1) };
    // a/b/y (a.b.y dotted): month first like Gmail unless only day first is a
    // valid date; dotted dates are day first (18.08.2026). Two-digit years are 20yy.
    if ((m = /^(\d{1,2})([/.-])(\d{1,2})\2(\d{4}|\d{2})$/.exec(s))) {
      const [a, b] = [+m[1], +m[3]];
      const yy = m[4].length === 2 ? 2000 + +m[4] : +m[4];
      const valid = (mo: number, day: number) => mo >= 1 && mo <= 12 && day >= 1 && day <= new Date(yy, mo, 0).getDate();
      const dayFirst = m[2] === "." ? valid(b, a) || !valid(a, b) : !valid(a, b) && valid(b, a);
      const [mo, day] = dayFirst ? [b, a] : [a, b];
      if (!valid(mo, day)) return null;
      return { from: d(yy, mo - 1, day), to: d(yy, mo - 1, day + 1) };
    }
  }
  return null;
}

const NUMBER_WORDS = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];
const UNIT = /^(days?|weeks?|months?|years?)$/;

/** As penguin-core's dates::normalize: numbers in words, "aug." → "aug". */
function normalizeWords(words: string[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < words.length; i++) {
    const [w, next, after] = [words[i], words[i + 1], words[i + 2]];
    if ((w === "a" || w === "an") && next === "couple" && after === "of") {
      out.push("2");
      i += 2;
    } else if (((w === "a" || w === "an") && next === "couple") || (w === "couple" && next === "of")) {
      out.push("2");
      i += 1;
    } else if (w === "a" && next === "few") {
      out.push("3");
      i += 1;
    } else if ((w === "a" || w === "an") && next !== undefined && UNIT.test(next)) {
      out.push("1");
    } else if (NUMBER_WORDS.includes(w) && next !== undefined && UNIT.test(next)) {
      out.push(String(NUMBER_WORDS.indexOf(w) + 1));
    } else {
      out.push(/^[a-z]+\.$/.test(w) ? w.slice(0, -1) : w);
    }
  }
  return out;
}

/** Any date phrase: single forms, since/before/…, between…and, A to B. */
export function parseDatePhrase(phrase: string, now = new Date(), iso = true): DateRange | null {
  // As penguin-core's date_words: "aug1..aug15" = "aug 1 to aug 15".
  const words = normalizeWords(
    phrase
      .trim()
      .toLowerCase()
      .replace(/\.\./g, " to ")
      .replace(/([a-z])(\d)/g, "$1 $2")
      .split(/\s+/)
      .filter(Boolean),
  );
  if (!words.length || words.length > 8) return null;
  const [head, ...rest] = words;
  if (/^(since|after)$/.test(head)) {
    const r = single(rest, now, iso);
    return r ? { from: r.from, to: null } : null;
  }
  if (head === "before") {
    const r = single(rest, now, iso);
    return r ? { from: null, to: r.from } : null;
  }
  if (/^(until|till|through|thru)$/.test(head)) {
    const r = single(rest, now, iso);
    return r ? { from: null, to: r.to } : null;
  }
  if (head === "between") {
    const i = rest.indexOf("and");
    const a = i > 0 ? single(rest.slice(0, i), now, iso) : null;
    const b = i > 0 ? single(rest.slice(i + 1), now, iso) : null;
    return a && b ? { from: a.from, to: b.to } : null;
  }
  const body = head === "from" ? rest : words;
  const sep = body.findIndex((w) => /^(to|through|until|-|–)$/.test(w));
  if (sep > 0) {
    const a = single(body.slice(0, sep), now, iso);
    const b = single(body.slice(sep + 1), now, iso);
    return a && b ? { from: a.from, to: b.to } : null;
  }
  if (head === "from") {
    const r = single(rest, now, iso);
    return r ? { from: r.from, to: null } : null;
  }
  return single(words, now, iso);
}

const fmtDay = (ms: number, withYear: boolean) =>
  new Date(ms).toLocaleDateString("en-US", withYear ? { month: "short", day: "numeric", year: "numeric" } : { month: "short", day: "numeric" });

/** "February · Feb 1 – 28, 2026", "Since march · since Mar 1, 2026". */
export function dateChipLabel(phrase: string, r: DateRange): string {
  const p = phrase.trim().replace(/^"|"$/g, "");
  const lead = p.charAt(0).toUpperCase() + p.slice(1).toLowerCase();
  let range: string;
  if (r.from !== null && r.to === null) range = `since ${fmtDay(r.from, true)}`;
  else if (r.from === null && r.to !== null) range = `before ${fmtDay(r.to, true)}`;
  else {
    const a = new Date(r.from!);
    const b = new Date(r.to! - 1);
    if (r.to! - r.from! <= DAY_MS) range = fmtDay(r.from!, true);
    else if (a.getFullYear() !== b.getFullYear()) range = `${fmtDay(r.from!, true)} – ${fmtDay(b.getTime(), true)}`;
    else if (a.getMonth() === b.getMonth()) range = `${fmtDay(r.from!, false)} – ${b.getDate()}, ${b.getFullYear()}`;
    else range = `${fmtDay(r.from!, false)} – ${fmtDay(b.getTime(), true)}`;
  }
  return `${lead} · ${range}`;
}

/** `date:` value autocomplete, most useful first. Months count back from now. */
export function dateSuggestions(now = new Date()): string[] {
  const out = ["today", "yesterday", "this week", "last week", "this month", "last month"];
  for (let i = 0; i < 12; i++) out.push(MONTHS[(now.getMonth() - i + 12) % 12]);
  out.push("this year", "last year", "last spring", "last summer", "last fall", "last winter");
  return out;
}

export interface DateHint {
  /** Exact substring of the query (spacing preserved). */
  raw: string;
  start: number;
  end: number;
  /** date:february or date:"last spring". */
  rewrite: string;
}

/**
 * The first, longest run of up to 6 plain words that reads as a date — for
 * "Use 'february' as a date?". Mirrors penguin_core::query::date_hint: "may"
 * alone (a verb) and single tokens with digits ("2025", "INV-20417") never are.
 */
export function dateHint(words: Array<{ start: number; end: number; text: string }>, q: string, now = new Date()): DateHint | null {
  for (let i = 0; i < words.length; i++) {
    for (let j = Math.min(words.length, i + 6); j > i; j--) {
      // A run is contiguous plain words (only spaces between them).
      const run = words.slice(i, j);
      if (run.some((w, k) => k > 0 && q.slice(run[k - 1].end, w.start).trim() !== "")) continue;
      const raw = q.slice(run[0].start, run[run.length - 1].end);
      const lower = raw.toLowerCase().replace(/\s+/g, " ");
      if (run.length === 1 && (lower === "may" || /\d/.test(lower))) continue;
      if (!parseDatePhrase(lower, now, false)) continue;
      return { raw, start: run[0].start, end: run[run.length - 1].end, rewrite: run.length === 1 ? `date:${lower}` : `date:"${lower}"` };
    }
  }
  return null;
}
