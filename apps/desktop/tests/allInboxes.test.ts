// "Show in All Inboxes" scoping (src/app/allInboxes.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { isHiddenFromAll, mailScope, marksReadOnOpen, withShownInAll } from "../src/app/allInboxes.ts";

const A = "a@north.example";
const B = "b@harbor.example";
const C = "c@home.example";
const accountIds = [A, B, C];

test("nothing hidden: All accounts stays unscoped (null)", () => {
  assert.equal(mailScope({ accountFilter: null, profileScope: null, accountIds, hidden: [] }), null);
});

test("All accounts leaves hidden accounts out, in account order", () => {
  assert.deepEqual(mailScope({ accountFilter: null, profileScope: null, accountIds, hidden: [C, A] }), [B]);
});

test("every account hidden: All accounts shows nothing (empty scope, not null)", () => {
  assert.deepEqual(mailScope({ accountFilter: null, profileScope: null, accountIds, hidden: [A, B, C] }), []);
});

test("hidden ids that aren't signed-in accounts change nothing", () => {
  assert.equal(mailScope({ accountFilter: null, profileScope: null, accountIds, hidden: ["gone@x.example"] }), null);
});

test("picking the hidden account shows its mail (no extra scope on top of the filter)", () => {
  assert.equal(mailScope({ accountFilter: C, profileScope: null, accountIds, hidden: [C] }), null);
});

test("a profile that lists a hidden account still shows it", () => {
  assert.deepEqual(mailScope({ accountFilter: null, profileScope: [B, C], accountIds, hidden: [C] }), [B, C]);
  // …and a picked account inside the profile keeps the profile scope, as before.
  assert.deepEqual(mailScope({ accountFilter: C, profileScope: [B, C], accountIds, hidden: [C] }), [B, C]);
});

test("toggling updates the list, keeps account order and drops unknown ids", () => {
  assert.deepEqual(withShownInAll([], C, false, accountIds), [C]);
  assert.deepEqual(withShownInAll([C], A, false, accountIds), [A, C]);
  assert.deepEqual(withShownInAll([A, C], A, true, accountIds), [C]);
  assert.deepEqual(withShownInAll(["gone@x.example", C], B, false, accountIds), [B, C]);
  assert.deepEqual(withShownInAll([], A, true, accountIds), []);
  assert.ok(isHiddenFromAll([C], C));
  assert.ok(!isHiddenFromAll([C], A));
});

test("opening a shown account's mail marks it read from anywhere", () => {
  for (const outsideList of [false, true])
    for (const accountInView of [false, true])
      assert.ok(marksReadOnOpen([C], A, { outsideList, accountInView }));
});

test("a hidden account's mail found through search, the person card or a notification stays unread", () => {
  // From outside the list, even while that account is the one picked.
  assert.ok(!marksReadOnOpen([C], C, { outsideList: true, accountInView: false }));
  assert.ok(!marksReadOnOpen([C], C, { outsideList: true, accountInView: true }));
});

test("a hidden account's mail is marked read from its own list (the account or a profile picked)", () => {
  assert.ok(marksReadOnOpen([C], C, { outsideList: false, accountInView: true }));
  // Not in view (All accounts leaves it out): a leftover cursor on it isn't a choice to read.
  assert.ok(!marksReadOnOpen([C], C, { outsideList: false, accountInView: false }));
});
