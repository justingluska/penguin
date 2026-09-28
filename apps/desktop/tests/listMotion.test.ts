// List row motion planning (features/inbox/listMotion.ts). Run: npm test.
import { test } from "node:test";
import assert from "node:assert/strict";
import { MOTION, initialMotion, nextExpiry, planMotion, pruneMotion, type ListMotion, type MotionContext } from "../src/features/inbox/listMotion.ts";

type Row = { key: string };

function input(keys: string[], listKey = "inbox") {
  const rows = keys.map((key) => ({ key }));
  return { listKey, rows, keys, index: new Map(keys.map((k, i) => [k, i])) };
}

function ctx(now: number, rendered: string[], over: Partial<MotionContext> = {}): MotionContext {
  return { now, rendered: new Set(rendered), motion: true, exitOf: () => "left", ...over };
}

const letters = (s: string) => s.split("");

test("a removed row on screen becomes a ghost anchored to the row that took its place", () => {
  const m0 = initialMotion<Row>(input(letters("abcdef")));
  const { state, changed } = planMotion(m0, input(letters("abdef")), ctx(100, letters("abcdef")));
  assert.equal(changed, true);
  assert.equal(state.ghosts.length, 1);
  const g = state.ghosts[0];
  assert.deepEqual([g.key, g.anchor, g.after, g.exit], ["c", "d", "b", "left"]);
  assert.deepEqual(g.item, { key: "c" });
  assert.ok(state.flowUntil > 100);
});

test("rows removed off screen leave no ghost, but the list still glides", () => {
  const m0 = initialMotion<Row>(input(letters("abcdef")));
  const { state, changed } = planMotion(m0, input(letters("abcde")), ctx(100, letters("abc")));
  assert.equal(changed, true);
  assert.equal(state.ghosts.length, 0);
});

test("pressing E five times fast stacks ghosts in the same gap and re-anchors them", () => {
  let m: ListMotion<Row> = initialMotion(input(letters("abcdefgh")));
  let keys = letters("abcdefgh");
  for (let i = 0; i < 5; i++) {
    const gone = keys[1];
    const next = keys.filter((k) => k !== gone);
    m = planMotion(m, input(next), ctx(100 + i * 30, keys)).state;
    keys = next;
  }
  assert.deepEqual(keys, letters("agh"));
  // Ghosts younger than LIFE_MS: b..f removed at 100..220; all alive at 220.
  assert.deepEqual(m.ghosts.map((g) => g.key), letters("bcdef"));
  assert.ok(m.ghosts.every((g) => g.anchor === "g" && g.after === "a"), JSON.stringify(m.ghosts.map((g) => [g.key, g.anchor])));
});

test("a ghost expires after LIFE_MS; pruning drops it and the glide ends", () => {
  const m0 = initialMotion<Row>(input(letters("abc")));
  const m1 = planMotion(m0, input(letters("ac")), ctx(100, letters("abc"))).state;
  assert.equal(nextExpiry(m1), 100 + Math.min(MOTION.LIFE_MS, MOTION.FLOW_MS));
  assert.equal(pruneMotion(m1, 150), m1, "nothing due yet: same object");
  const m2 = pruneMotion(m1, 100 + Math.max(MOTION.LIFE_MS, MOTION.FLOW_MS));
  assert.equal(m2.ghosts.length, 0);
  assert.equal(m2.flowUntil, 0);
  assert.equal(nextExpiry(m2), null);
});

test("undo brings the row back: the ghost is dropped and the row opens in place", () => {
  const m0 = initialMotion<Row>(input(letters("abcd")));
  const m1 = planMotion(m0, input(letters("abd")), ctx(100, letters("abcd"))).state;
  const p = planMotion(m1, input(letters("abcd")), ctx(150, letters("abd")));
  assert.equal(p.state.ghosts.length, 0);
  assert.ok(p.state.entering.has("c"));
  assert.deepEqual(p.added, [], "a row coming back is not new mail");
});

test("undo of the last row animates too (it was just removed), later appends don't", () => {
  const m0 = initialMotion<Row>(input(letters("abc")));
  const m1 = planMotion(m0, input(letters("ab")), ctx(100, letters("abc"))).state;
  const m2 = pruneMotion(m1, 1000);
  const back = planMotion(m2, input(letters("abc")), ctx(2000, letters("ab"))).state;
  assert.ok(back.entering.has("c"));
  const paged = planMotion(back, input(letters("abcxyz")), ctx(2100, letters("abc")));
  assert.equal(paged.state.entering.has("x"), false);
  assert.deepEqual(paged.added, []);
});

test("new mail at the top opens in place and is reported as added", () => {
  const m0 = initialMotion<Row>(input(letters("cde")));
  const p = planMotion(m0, input(letters("abcde")), ctx(100, letters("cde")));
  assert.deepEqual(p.added, ["a", "b"]);
  assert.ok(p.state.entering.has("a") && p.state.entering.has("b"));
});

test("a row that jumps to the top doesn't glide across the list", () => {
  const m0 = initialMotion<Row>(input(letters("abcde")));
  const p = planMotion(m0, input(letters("dabce")), ctx(100, letters("abcde")));
  assert.deepEqual([...p.state.noGlide], ["d"]);
  assert.ok(p.state.entering.has("d"));
});

test("content-only changes (same keys) don't move anything", () => {
  const m0 = initialMotion<Row>(input(letters("abc")));
  const p = planMotion(m0, input(letters("abc")), ctx(100, letters("abc")));
  assert.equal(p.changed, false);
  assert.equal(p.state.flowUntil, 0);
});

test("a new mailbox, the first load and reduced motion never animate", () => {
  const m0 = initialMotion<Row>(input(letters("abc")));
  assert.equal(planMotion(m0, input(letters("ab"), "sent"), ctx(100, letters("abc"))).state.ghosts.length, 0);
  assert.equal(planMotion(initialMotion<Row>(input([])), input(letters("abc")), ctx(100, [])).state.entering.size, 0);
  const reduced = planMotion(m0, input(letters("ab")), ctx(100, letters("abc"), { motion: false }));
  assert.equal(reduced.state.ghosts.length, 0);
  assert.equal(reduced.state.flowUntil, 0);
});

test("a big insertion (a reload) just appears", () => {
  const m0 = initialMotion<Row>(input(["z"]));
  const many = Array.from({ length: MOTION.MAX_ENTERING + 5 }, (_, i) => "n" + i);
  const p = planMotion(m0, input([...many, "z"]), ctx(100, ["z"]));
  assert.equal(p.state.entering.size, 0);
  assert.equal(p.added.length, many.length);
});
