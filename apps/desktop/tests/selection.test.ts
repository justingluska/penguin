// Multi-select state machine (src/app/selectionModel.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { EMPTY_SELECTION, isEmpty, prune, selectAll, selectAllMatching, selectRange, toggle } from "../src/app/selectionModel.ts";

const order = ["a", "b", "c", "d", "e", "f"];
const keys = (s: { keys: ReadonlySet<string> }) => [...s.keys].sort();

test("toggle adds and removes, and moves the anchor", () => {
  let s = toggle(EMPTY_SELECTION, "b");
  assert.deepEqual(keys(s), ["b"]);
  assert.equal(s.anchor, "b");
  s = toggle(s, "d");
  assert.deepEqual(keys(s), ["b", "d"]);
  s = toggle(s, "b");
  assert.deepEqual(keys(s), ["d"]);
  assert.equal(s.anchor, "b");
  assert.ok(isEmpty(toggle(s, "d")));
});

test("range selects from the anchor to the row, either direction, adding to the selection", () => {
  let s = toggle(EMPTY_SELECTION, "b");
  s = selectRange(s, "d", order);
  assert.deepEqual(keys(s), ["b", "c", "d"]);
  s = toggle(s, "f");
  s = selectRange(s, "e", order); // anchor f → e
  assert.deepEqual(keys(s), ["b", "c", "d", "e", "f"]);
  const up = selectRange(toggle(EMPTY_SELECTION, "e"), "b", order);
  assert.deepEqual(keys(up), ["b", "c", "d", "e"]);
});

test("range without an anchor (or a vanished one) selects just the row", () => {
  assert.deepEqual(keys(selectRange(EMPTY_SELECTION, "c", order)), ["c"]);
  const s = { ...toggle(EMPTY_SELECTION, "zz"), keys: new Set<string>() };
  assert.deepEqual(keys(selectRange(s, "c", order)), ["c"]);
  assert.equal(selectRange(EMPTY_SELECTION, "nope", order), EMPTY_SELECTION);
});

test("select all loaded, then all matching; toggling narrows back", () => {
  const all = selectAll(EMPTY_SELECTION, order);
  assert.equal(all.keys.size, order.length);
  assert.equal(all.allMatching, false);
  const matching = selectAllMatching(all, order);
  assert.equal(matching.allMatching, true);
  const narrowed = toggle(matching, "a");
  assert.equal(narrowed.allMatching, false);
  assert.equal(narrowed.keys.size, order.length - 1);
});

test("prune drops rows that left the list and keeps identity when nothing did", () => {
  const s = selectRange(toggle(EMPTY_SELECTION, "a"), "c", order);
  assert.equal(prune(s, order), s, "unchanged → same object (no re-render)");
  const p = prune(s, ["a", "d", "e"]);
  assert.deepEqual(keys(p), ["a"]);
  assert.equal(p.anchor, null, "the anchor (c) is gone");
  assert.ok(isEmpty(prune(selectAllMatching(EMPTY_SELECTION, ["x"]), [])), "all-matching with nothing left is empty");
});

test("large selections stay cheap", () => {
  const big = Array.from({ length: 20000 }, (_, i) => `t${i}`);
  const t0 = performance.now();
  let s = selectAll(EMPTY_SELECTION, big);
  s = toggle(s, "t5");
  s = prune(s, big.slice(0, 15000));
  assert.equal(s.keys.size, 14999);
  assert.ok(performance.now() - t0 < 200, "20k keys in well under 200ms");
});
