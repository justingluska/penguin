// Snooze picker presets and wake-time labels (src/features/snooze/presets.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { customError, defaultCustom, fmtUntil, fmtWake, fromLocalInput, snoozePresets, toLocalInput, wakeGroup } from "../src/features/snooze/presets.ts";

// Local-time instants, so the tests hold in any time zone.
const t = (m: number, d: number, h: number, min = 0, y = 2026) => new Date(y, m, d, h, min).getTime();
const presets = (now: number) => snoozePresets(new Date(now)).map((p) => [p.id, p.at] as const);

test("a weekday morning: later today at 6 PM, tomorrow 8 AM, Saturday, Monday", () => {
  // Thu Sep 24, 2026, 10:00
  assert.deepEqual(presets(t(8, 24, 10)), [
    ["later", t(8, 24, 18)],
    ["tomorrow", t(8, 25, 8)],
    ["weekend", t(8, 26, 8)],
    ["nextWeek", t(8, 28, 8)],
  ]);
});

test("late afternoon: later today is ~3 hours on, on the hour; none late at night", () => {
  assert.equal(snoozePresets(new Date(t(8, 24, 17, 20)))[0].at, t(8, 24, 21)); // 17:20 → 21:00
  assert.equal(snoozePresets(new Date(t(8, 24, 18)))[0].at, t(8, 24, 21)); // exactly 18:00 → 21:00
  assert.equal(snoozePresets(new Date(t(8, 24, 16, 59)))[0].at, t(8, 24, 18)); // 18:00 is over an hour away
  assert.ok(!snoozePresets(new Date(t(8, 24, 21, 30))).some((p) => p.id === "later"));
});

test("Friday: this weekend is tomorrow morning, so it's shown once", () => {
  // Fri Sep 25, 2026
  assert.deepEqual(presets(t(8, 25, 9)).map(([id]) => id), ["later", "tomorrow", "nextWeek"]);
  assert.equal(presets(t(8, 25, 9))[2][1], t(8, 28, 8));
});

test("weekends: no 'this weekend'; on Sunday next week is tomorrow", () => {
  // Sat Sep 26
  assert.deepEqual(presets(t(8, 26, 9)), [
    ["later", t(8, 26, 18)],
    ["tomorrow", t(8, 27, 8)],
    ["nextWeek", t(8, 28, 8)],
  ]);
  // Sun Sep 27: Monday 8 AM is tomorrow morning.
  assert.deepEqual(presets(t(8, 27, 9)).map(([id]) => id), ["later", "tomorrow"]);
});

test("Monday: next week is the following Monday; month and year roll over", () => {
  assert.equal(presets(t(8, 28, 9)).find(([id]) => id === "nextWeek")?.[1], t(9, 5, 8));
  // Wed Dec 30, 2026 → Sat Jan 2, Mon Jan 4, 2027
  const nye = presets(t(11, 30, 12));
  assert.equal(nye.find(([id]) => id === "weekend")?.[1], t(0, 2, 8, 0, 2027));
  assert.equal(nye.find(([id]) => id === "nextWeek")?.[1], t(0, 4, 8, 0, 2027));
});

test("every preset is in the future and in time order", () => {
  for (let h = 0; h < 24; h++) {
    for (const day of [21, 22, 23, 24, 25, 26, 27]) {
      const now = t(8, day, h, 30);
      const ps = snoozePresets(new Date(now));
      assert.ok(ps.some((p) => p.id === "tomorrow"));
      for (let i = 0; i < ps.length; i++) {
        assert.ok(ps[i].at > now);
        if (i > 0) assert.ok(ps[i].at > ps[i - 1].at);
      }
    }
  }
});

test("custom times: default is tomorrow 8 AM; past and >1 year are refused", () => {
  const now = t(8, 24, 10);
  assert.equal(defaultCustom(new Date(now)), t(8, 25, 8));
  assert.equal(customError(now + 60_000, now), null);
  assert.match(customError(now, now)!, /future/);
  assert.match(customError(now + 367 * 86_400_000, now)!, /within a year/);
  assert.match(customError(NaN, now)!, /Pick/);
  assert.equal(fromLocalInput(toLocalInput(t(8, 28, 8, 15))), t(8, 28, 8, 15));
  assert.ok(Number.isNaN(fromLocalInput("next tuesday")));
});

test("the Snoozed view groups by wake day", () => {
  const now = t(8, 24, 10);
  assert.deepEqual(
    [t(8, 24, 18), t(8, 25, 8), t(8, 29, 8), t(9, 12, 8)].map((ms) => wakeGroup(ms, now)),
    ["Today", "Tomorrow", "This week", "Later"],
  );
});

test("wake labels: today, tomorrow, weekday, date, other year", () => {
  const now = t(8, 24, 10); // Thu
  assert.equal(fmtWake(t(8, 24, 18), now, "en-US"), "6:00 PM");
  assert.equal(fmtWake(t(8, 25, 8), now, "en-US"), "Tomorrow 8:00 AM");
  assert.equal(fmtWake(t(8, 29, 8), now, "en-US"), "Tue 8:00 AM");
  assert.equal(fmtWake(t(9, 12, 8), now, "en-US"), "Oct 12, 8:00 AM");
  assert.equal(fmtWake(t(0, 3, 8, 0, 2027), now, "en-US"), "Jan 3, 2027, 8:00 AM");
  assert.equal(fmtUntil(t(8, 29, 8), now, "en-US"), "until Tue 8:00 AM");
  assert.equal(fmtUntil(t(8, 25, 8), now, "en-US"), "until tomorrow 8:00 AM");
});
