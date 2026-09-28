// Ask card formatting (features/search/askFormat.ts): local wall times from
// extracted facts, day counts, highlighted passages, parcel progress and
// status tones.
import { test } from "node:test";
import assert from "node:assert/strict";
import { daysUntil, fmtTime, markSegments, parseLocal, shipStep, statusTone } from "../src/features/search/askFormat.ts";

test("local wall times parse without a timezone shift", () => {
  const p = parseLocal("2026-10-02T19:05");
  assert.ok(p);
  assert.equal(p.hasTime, true);
  assert.equal(p.date.getHours(), 19);
  assert.equal(p.date.getMinutes(), 5);
  assert.equal(p.date.getDate(), 2);
  const d = parseLocal("2026-10-02");
  assert.equal(d?.hasTime, false);
  assert.equal(parseLocal("soon"), null);
  assert.equal(parseLocal(null), null);
  assert.equal(fmtTime("2026-10-02"), "");
  assert.notEqual(fmtTime("2026-10-02T19:05"), "");
});

test("days until a local date", () => {
  const now = new Date(2026, 8, 26, 23, 30).getTime();
  assert.equal(daysUntil("2026-09-27", now), 1);
  assert.equal(daysUntil("2026-09-26T08:00", now), 0);
  assert.equal(daysUntil("2026-09-20", now), -6);
  assert.equal(daysUntil(null, now), null);
});

test("passage highlights split at the marks", () => {
  const text = "The wifi password is harbor2026.";
  assert.deepEqual(markSegments(text, [[4, 8], [9, 17]]), [
    { text: "The ", mark: false },
    { text: "wifi", mark: true },
    { text: " ", mark: false },
    { text: "password", mark: true },
    { text: " is harbor2026.", mark: false },
  ]);
  // Out-of-range and overlapping marks are ignored, unsorted ones sorted.
  assert.deepEqual(markSegments("abc", [[2, 3], [0, 1], [0, 9]]), [
    { text: "a", mark: true },
    { text: "b", mark: false },
    { text: "c", mark: true },
  ]);
  assert.deepEqual(markSegments("", []), []);
});

test("parcel progress and status tones", () => {
  assert.equal(shipStep("delivered"), 3);
  assert.equal(shipStep("outForDelivery"), 2);
  assert.equal(shipStep("inTransit"), 1);
  assert.equal(shipStep("shipped"), 0);
  assert.equal(shipStep(null), 0);
  assert.equal(shipStep("exception"), -1);
  assert.equal(statusTone("Cancelled"), "red");
  assert.equal(statusTone("Overdue"), "red");
  assert.equal(statusTone("Delivered"), "green");
  assert.equal(statusTone("Due in 3 days"), "amber");
  assert.equal(statusTone("In 6 days"), "blue");
  assert.equal(statusTone("3 weeks ago"), "gray");
  assert.equal(statusTone(null), "gray");
});
