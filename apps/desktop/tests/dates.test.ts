// date: phrases and the "Use 'february' as a date?" hint (src/features/search/dates.ts),
// mirroring penguin-core's date_like_hints / date_operator cases.
import { test } from "node:test";
import assert from "node:assert/strict";
import { dateChipLabel, dateHint, parseDatePhrase } from "../src/features/search/dates.ts";

const NOW = new Date(2026, 8, 24, 10, 0); // Thu Sep 24, 2026
const day = (y: number, m: number, d: number) => new Date(y, m, d).getTime();

/** Plain word tokens the way SearchPanel builds them (no ':' or '"', no '-'). */
function words(q: string) {
  const out: Array<{ start: number; end: number; text: string }> = [];
  for (const m of q.matchAll(/\S+/g)) {
    const t = m[0];
    if (/[:"]/.test(t) || t.startsWith("-") || t === "OR") continue;
    out.push({ start: m.index!, end: m.index! + t.length, text: t });
  }
  return out;
}
const hint = (q: string) => dateHint(words(q), q, NOW);

test("hint: finds the first longest date-like run and rewrites it", () => {
  assert.deepEqual(
    { raw: hint("invoice february")?.raw, rewrite: hint("invoice february")?.rewrite },
    { raw: "february", rewrite: "date:february" },
  );
  assert.equal(hint("lease last spring")?.rewrite, 'date:"last spring"');
  const since = hint("receipts since  March");
  assert.equal(since?.raw, "since  March");
  assert.equal(since?.rewrite, 'date:"since march"');
  assert.equal(hint("the week of feb 10 receipts")?.rewrite, 'date:"the week of feb 10"');
  assert.equal(hint("meeting may 10")?.rewrite, 'date:"may 10"');
});

test("hint: nothing for verbs, numbers, operators, quoted or negated words", () => {
  for (const q of ["we may need it", "invoice 2025", "INV-20417", "date:february", '"february"', "-february", "lease renewal"]) {
    assert.equal(hint(q), null, q);
  }
});

test("date: phrases resolve to ranges", () => {
  assert.deepEqual(parseDatePhrase("february", NOW), { from: day(2026, 1, 1), to: day(2026, 2, 1) });
  // "last february" is the one before the most recent.
  assert.deepEqual(parseDatePhrase("last february", NOW), { from: day(2025, 1, 1), to: day(2025, 2, 1) });
  // A month still ahead this year means last year's.
  assert.deepEqual(parseDatePhrase("november", NOW), { from: day(2025, 10, 1), to: day(2025, 11, 1) });
  assert.deepEqual(parseDatePhrase("may", NOW), { from: day(2026, 4, 1), to: day(2026, 5, 1) });
  assert.deepEqual(parseDatePhrase("feb 10th, 2025", NOW), { from: day(2025, 1, 10), to: day(2025, 1, 11) });
  assert.deepEqual(parseDatePhrase("late february", NOW), { from: day(2026, 1, 21), to: day(2026, 2, 1) });
  assert.deepEqual(parseDatePhrase("since march", NOW), { from: day(2026, 2, 1), to: null });
  assert.deepEqual(parseDatePhrase("jan 5 to jan 20", NOW), { from: day(2026, 0, 5), to: day(2026, 0, 21) });
  assert.deepEqual(parseDatePhrase("between nov and feb", NOW), { from: day(2025, 10, 1), to: day(2026, 2, 1) });
  // The week of Feb 10, 2026 (a Tuesday) runs Mon Feb 9 – Sun Feb 15.
  assert.deepEqual(parseDatePhrase("the week of feb 10", NOW), { from: day(2026, 1, 9), to: day(2026, 1, 16) });
  assert.deepEqual(parseDatePhrase("tuesday", NOW), { from: day(2026, 8, 22), to: day(2026, 8, 23) });
  assert.deepEqual(parseDatePhrase("2026-02", NOW), { from: day(2026, 1, 1), to: day(2026, 2, 1) });
  assert.equal(parseDatePhrase("banana", NOW), null);
  assert.equal(parseDatePhrase("february banana", NOW), null);
});

test("chip labels read like penguin-core's", () => {
  assert.equal(dateChipLabel("february", parseDatePhrase("february", NOW)!), "February · Feb 1 – 28, 2026");
  assert.equal(dateChipLabel("since march", parseDatePhrase("since march", NOW)!), "Since march · since Mar 1, 2026");
  assert.equal(dateChipLabel("last spring", parseDatePhrase("last spring", NOW)!), "Last spring · Mar 1 – May 31, 2026");
});
