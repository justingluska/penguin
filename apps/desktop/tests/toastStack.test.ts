// Toast deck geometry (src/components/toastStack.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { STACK, stackHeight, stackLayout } from "../src/components/toastStack.ts";

test("collapsed: front in place, cards behind peek above it, smaller and dimmer", () => {
  const p = stackLayout([50, 50, 50], false);
  assert.deepEqual(p.map((x) => x.y), [0, -STACK.PEEK, -2 * STACK.PEEK]);
  assert.deepEqual(p.map((x) => x.scale), [1, 1 - STACK.SCALE_STEP, 1 - 2 * STACK.SCALE_STEP]);
  assert.ok(p.every((x) => x.opacity === 1), "opaque: a translucent card would show the one behind");
  assert.ok(p[0].dim < p[1].dim && p[1].dim < p[2].dim);
  assert.ok(p[0].z > p[1].z && p[1].z > p[2].z);
});

test("collapsed: peek offsets follow the front card's height, not each card's own", () => {
  // A taller card behind a short front one is pushed down so its top lines up, and clipped.
  const [, tall] = stackLayout([40, 90], false);
  assert.equal(tall.y, 90 - 40 - STACK.PEEK);
  assert.equal(tall.clipBottom, 50);
  // A shorter card behind a tall front one moves up to line up its top; nothing to clip.
  const [, short] = stackLayout([90, 40], false);
  assert.equal(short.y, 40 - 90 - STACK.PEEK);
  assert.equal(short.clipBottom, 0);
  // Its top edge (bottom-anchored card: top = y - height) sits PEEK above the front's top.
  assert.equal(short.y - 40, -90 - STACK.PEEK);
});

test("expanded: a list upward from the front card, GAP apart, unclipped", () => {
  const p = stackLayout([40, 60, 50], true);
  assert.deepEqual(p.map((x) => x.y), [0, -(40 + STACK.GAP), -(40 + 60 + 2 * STACK.GAP)]);
  assert.ok(p.every((x) => x.scale === 1 && x.clipBottom === 0 && x.opacity === 1 && x.dim === 0));
  assert.equal(stackHeight([40, 60, 50], true), 40 + 60 + 50 + 2 * STACK.GAP);
});

test("past the cap: hidden behind the last visible card, in both modes", () => {
  const c = stackLayout([50, 50, 50, 50, 50], false);
  assert.deepEqual(c.map((x) => x.hidden), [false, false, false, true, true]);
  assert.equal(c[4].y, c[2].y);
  assert.equal(c[4].opacity, 0);
  const e = stackLayout([50, 50, 50, 50], true);
  assert.ok(e[3].hidden && e[3].opacity === 0);
  assert.equal(stackHeight([50, 50, 50, 50], true), 150 + 2 * STACK.GAP);
  assert.equal(stackHeight([50, 50, 50, 50], false), 50 + 2 * STACK.PEEK);
});

test("unmeasured cards use the default height", () => {
  const [a, b] = stackLayout([undefined, undefined], false);
  assert.equal(a.y, 0);
  assert.equal(b.y, -STACK.PEEK);
});
