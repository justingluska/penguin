// Account order (src/app/accountOrder.ts): Settings.accountOrder applied to
// the account list, drag and Move up / Move down, profile-scoped moves.
import { test } from "node:test";
import assert from "node:assert/strict";
import { canMove, dropIndex, moveBy, moveWithin, orderAccounts, sameOrder } from "../src/app/accountOrder.ts";

const A = "a@north.example";
const B = "b@harbor.example";
const C = "c@home.example";
const D = "d@linden.example";
const acc = (id: string) => ({ id });
const ids = (list: { id: string }[]) => list.map((a) => a.id);

test("no saved order keeps the backend's order (same array)", () => {
  const list = [A, B, C].map(acc);
  assert.equal(orderAccounts(list, []), list);
});

test("listed accounts come first in list order, the rest keep theirs after", () => {
  const list = [A, B, C, D].map(acc);
  assert.deepEqual(ids(orderAccounts(list, [C, A])), [C, A, B, D]);
});

test("a new account (not in the order yet) goes last", () => {
  const list = [A, B, C, D].map(acc); // D just signed in
  assert.deepEqual(ids(orderAccounts(list, [C, B, A])), [C, B, A, D]);
});

test("ids of removed accounts and duplicates in the order are ignored", () => {
  const list = [A, B].map(acc);
  assert.deepEqual(ids(orderAccounts(list, ["gone@x.example", B, B, A])), [B, A]);
});

test("an order that changes nothing returns the same array", () => {
  const list = [A, B, C].map(acc);
  assert.equal(orderAccounts(list, [A, B]), list);
});

test("moveWithin: drag to the top, the bottom and the middle", () => {
  const all = [A, B, C, D];
  assert.deepEqual(moveWithin(all, all, C, 0), [C, A, B, D]);
  assert.deepEqual(moveWithin(all, all, A, 3), [B, C, D, A]);
  assert.deepEqual(moveWithin(all, all, A, 1), [B, A, C, D]);
  // The index counts the other rows; out of range clamps.
  assert.deepEqual(moveWithin(all, all, B, 99), [A, C, D, B]);
  assert.deepEqual(moveWithin(all, all, D, -5), [D, A, B, C]);
});

test("moveWithin in a profile only reorders its accounts; the others keep their places", () => {
  const all = [A, B, C, D];
  // The profile shows B and D (in the global order). Drag D above B.
  assert.deepEqual(moveWithin(all, [B, D], D, 0), [A, D, C, B]);
  // Profile order given out of global order still reads as the global order.
  assert.deepEqual(moveWithin(all, [D, B], D, 0), [A, D, C, B]);
  // A profile of one: nothing to move.
  assert.deepEqual(moveWithin(all, [C], C, 0), all);
});

test("moveWithin ignores accounts it doesn't know", () => {
  const all = [A, B, C];
  assert.equal(moveWithin(all, all, "gone@x.example", 0), all);
  assert.equal(moveWithin(all, [A, B], C, 0), all);
  assert.deepEqual(moveWithin(all, [A, "gone@x.example", C], C, 0), [C, B, A]);
});

test("moveBy: Move up / Move down, a no-op at either end", () => {
  const all = [A, B, C];
  assert.deepEqual(moveBy(all, all, B, -1), [B, A, C]);
  assert.deepEqual(moveBy(all, all, B, 1), [A, C, B]);
  assert.equal(moveBy(all, all, A, -1), all);
  assert.equal(moveBy(all, all, C, 1), all);
  assert.equal(canMove(all, all, A, -1), false);
  assert.equal(canMove(all, all, A, 1), true);
  // Within a profile: neighbours are the profile's accounts.
  assert.deepEqual(moveBy([A, B, C, D], [A, D], D, -1), [D, B, C, A]);
  assert.equal(canMove([A, B, C, D], [A, D], A, -1), false);
});

test("dropIndex: past a row's midpoint counts as after it", () => {
  const rows = [
    { top: 0, height: 32 },
    { top: 33, height: 32 },
    { top: 66, height: 32 },
  ];
  assert.equal(dropIndex(-10, rows), 0);
  assert.equal(dropIndex(15, rows), 0);
  assert.equal(dropIndex(17, rows), 1);
  assert.equal(dropIndex(50, rows), 2);
  assert.equal(dropIndex(90, rows), 3);
  assert.equal(dropIndex(10, []), 0);
});

test("sameOrder", () => {
  assert.equal(sameOrder([A, B], [A, B]), true);
  assert.equal(sameOrder([A, B], [B, A]), false);
  assert.equal(sameOrder([A], [A, B]), false);
});
