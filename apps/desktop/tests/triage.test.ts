// Reply Later / Follow up: Follow up row text and groups
// (src/features/triage/format.ts) and the mock's automated-address rule,
// which mirrors penguin-core `is_automated_address` (store_triage.rs).
import { test } from "node:test";
import assert from "node:assert/strict";
import { daysSince, waitGroup, waitingText } from "../src/features/triage/format.ts";
import { isAutomatedAddress } from "../src/lib/mock/automated.ts";

const DAY = 86_400_000;
const NOW = new Date(2026, 8, 25, 15, 0).getTime();

test("waiting text counts whole days since it was sent", () => {
  assert.equal(waitingText(NOW - 5 * DAY, NOW), "Sent 5 days ago · no reply");
  assert.equal(waitingText(NOW - DAY - 1000, NOW), "Sent yesterday · no reply");
  assert.equal(waitingText(NOW - 3 * 3_600_000, NOW), "Sent today · no reply");
  assert.equal(waitingText(NOW + DAY, NOW), "Sent today · no reply");
  assert.equal(daysSince(NOW - 13.9 * DAY, NOW), 13);
});

test("follow up groups by how long it has waited", () => {
  assert.equal(waitGroup(NOW - 3 * DAY, NOW), "Waiting under a week");
  assert.equal(waitGroup(NOW - 7 * DAY, NOW), "Waiting over a week");
  assert.equal(waitGroup(NOW - 13 * DAY, NOW), "Waiting over a week");
  assert.equal(waitGroup(NOW - 14 * DAY, NOW), "Waiting 2 weeks or more");
});

test("machines are left out of Follow up, people and support desks aren't", () => {
  for (const e of [
    "noreply@shop.example",
    "no-reply@shop.example",
    "do_not_reply@bank.example",
    "DoNotReply@bank.example",
    "notifications@github.example",
    "notifications+abc@tracker.example",
    "mailer-daemon@mx.example",
    "bounces+123@list.example",
    "newsletter@letters.example",
    "calendar-notification@google.com",
    "abc123@group.calendar.google.com",
  ])
    assert.ok(isAutomatedAddress(e), e);
  for (const e of ["ana@harbor.example", "support@vendor.example", "newsroom@paper.example", "noah@reply.example", "billing@vendor.example"])
    assert.ok(!isAutomatedAddress(e), e);
});
