// Swipe gesture state machine. Run: npm test (Node's built-in runner with type stripping).
import { test } from "node:test";
import assert from "node:assert/strict";
import { SWIPE, feedSwipe, initialSwipe, releaseSwipe, shortThreshold, zoneOf, type SwipeState } from "../src/features/inbox/swipe.ts";

const W = 340;
const ALL = { right: true, left: true, leftLong: true };

/** Feed a burst of events 16ms apart starting at t0. */
function burst(s: SwipeState, deltas: [number, number][], t0 = 1000): { s: SwipeState; t: number } {
  let t = t0;
  for (const [dx, dy] of deltas) {
    s = feedSwipe(s, dx, dy, t, W);
    t += 16;
  }
  return { s, t };
}
const repeat = (n: number, d: [number, number]) => Array.from({ length: n }, () => d);

test("a right swipe (negative deltaX) moves the row right and commits past the short threshold", () => {
  const { s, t } = burst(initialSwipe(), repeat(10, [-10, 0]));
  assert.equal(s.phase, "tracking");
  assert.equal(s.offset, 100);
  const r = releaseSwipe(s, t + SWIPE.IDLE_MS, W, ALL);
  assert.equal(r.zone, "right");
  assert.equal(r.next.phase, "cooldown");
});

test("a short left swipe archives, a long one trashes", () => {
  const short = burst(initialSwipe(), repeat(10, [10, 0]));
  assert.equal(releaseSwipe(short.s, short.t, W, ALL).zone, "left");
  const long = burst(initialSwipe(), repeat(25, [10, 0]));
  assert.ok(long.s.offset <= -W * SWIPE.LONG_FRAC);
  assert.equal(releaseSwipe(long.s, long.t, W, ALL).zone, "leftLong");
});

test("below the threshold nothing commits", () => {
  const { s, t } = burst(initialSwipe(), repeat(4, [10, 0]));
  assert.equal(s.phase, "tracking");
  assert.equal(releaseSwipe(s, t, W, ALL).zone, null);
});

test("swiping back before release follows the fingers", () => {
  const { s, t } = burst(initialSwipe(), [...repeat(10, [10, 0]), ...repeat(8, [-10, 0])]);
  assert.equal(s.offset, -20);
  assert.equal(releaseSwipe(s, t, W, ALL).zone, null);
});

test("mostly vertical gestures are ignored for their whole length", () => {
  const { s, t } = burst(initialSwipe(), [[2, 12], ...repeat(10, [-15, 0])]);
  assert.equal(s.phase, "ignoring");
  assert.equal(s.offset, 0);
  const r = releaseSwipe(s, t, W, ALL);
  assert.equal(r.zone, null);
  assert.equal(r.next.phase, "idle", "a scroll leaves no cooldown");
});

test("the axis is decided only after a few pixels", () => {
  const s = feedSwipe(initialSwipe(), -3, 1, 1000, W);
  assert.equal(s.phase, "deciding");
  assert.equal(s.offset, 0);
});

test("momentum after commit is swallowed; a new gesture after the cooldown works", () => {
  const { s, t } = burst(initialSwipe(), repeat(10, [10, 0]));
  let { next } = releaseSwipe(s, t + SWIPE.IDLE_MS, W, ALL);
  // Stray momentum tail right after release: ignored, and it keeps extending the cooldown.
  let now = t + SWIPE.IDLE_MS + 20;
  for (let i = 0; i < 30; i++, now += 16) next = feedSwipe(next, 4, 0, now, W);
  assert.equal(next.phase, "cooldown");
  assert.equal(next.offset, 0);
  // Quiet, then a fresh swipe.
  const fresh = burst(next, repeat(10, [-10, 0]), now + 500);
  assert.equal(fresh.s.phase, "tracking");
  assert.equal(fresh.s.offset, 100);
});

test("a gap longer than IDLE_MS starts a new gesture", () => {
  const a = burst(initialSwipe(), repeat(3, [10, 0]));
  // Without the gap reset this vertical event would just continue the tracking.
  const b = feedSwipe(a.s, 2, 12, a.t + SWIPE.IDLE_MS + 50, W);
  assert.equal(b.phase, "ignoring");
  assert.equal(b.offset, 0);
});

test("past its limit the row rubber-bands: it gives less and less, never more than RUBBER_PX", () => {
  const limit = W * SWIPE.MAX_FRAC;
  const a = burst(initialSwipe(), repeat(40, [-10, 0])).s; // 400 px of finger travel
  const b = burst(initialSwipe(), repeat(100, [-20, 0])).s; // 2000 px
  assert.ok(a.offset > limit && a.offset < limit + SWIPE.RUBBER_PX);
  assert.ok(b.offset > a.offset && b.offset < limit + SWIPE.RUBBER_PX);
  assert.equal(b.raw, 2000, "the finger travel itself is kept");
});

test("a side with no action resists from the start and never commits", () => {
  const onlyLeft = { right: false, left: true, leftLong: true };
  let s = initialSwipe();
  let t = 1000;
  for (let i = 0; i < 20; i++, t += 16) s = feedSwipe(s, -10, 0, t, W, onlyLeft);
  assert.ok(s.offset > 0 && s.offset < SWIPE.DEAD_PX, `offset ${s.offset}`);
  assert.equal(releaseSwipe(s, t, W, onlyLeft).zone, null);
});

test("tracking is 1:1 below the limit, and back again", () => {
  const { s } = burst(initialSwipe(), [...repeat(10, [10, 0]), ...repeat(5, [-10, 0])]);
  assert.equal(s.offset, -50);
  assert.equal(s.raw, -50);
});

test("a fast flick commits below the distance threshold; the same distance slowly does not", () => {
  const short = shortThreshold(W);
  // Fast: 3 events of 16 px, 8 ms apart (2 px/ms), ending about 60% of the way to the threshold.
  let s = initialSwipe();
  let t = 1000;
  for (let i = 0; i < 3; i++, t += 8) s = feedSwipe(s, 16, 0, t, W);
  assert.ok(Math.abs(s.offset) < short, "below the threshold");
  assert.equal(releaseSwipe(s, t + SWIPE.IDLE_MS, W, ALL).zone, "left");
  // Slow: the same distance in small steps.
  let q = initialSwipe();
  t = 1000;
  for (let i = 0; i < 24; i++, t += 16) q = feedSwipe(q, 2, 0, t, W);
  assert.equal(Math.round(q.offset), Math.round(s.offset));
  assert.equal(releaseSwipe(q, t + SWIPE.IDLE_MS, W, ALL).zone, null);
});

test("a flick that turned back doesn't commit", () => {
  let s = initialSwipe();
  let t = 1000;
  for (let i = 0; i < 3; i++, t += 8) s = feedSwipe(s, 16, 0, t, W);
  for (let i = 0; i < 4; i++, t += 16) s = feedSwipe(s, -4, 0, t, W);
  assert.equal(releaseSwipe(s, t + SWIPE.IDLE_MS, W, ALL).zone, null);
});

test("a tiny fast twitch doesn't commit", () => {
  let s = initialSwipe();
  let t = 1000;
  for (let i = 0; i < 2; i++, t += 8) s = feedSwipe(s, 8, 0, t, W);
  assert.ok(Math.abs(s.offset) < shortThreshold(W) * SWIPE.FLICK_MIN_FRAC);
  assert.equal(releaseSwipe(s, t + SWIPE.IDLE_MS, W, ALL).zone, null);
});

test("zones respect the configured actions", () => {
  const short = -shortThreshold(W) - 1;
  assert.equal(zoneOf(-W * 0.7, W, { right: true, left: true, leftLong: false }), "left", "no long action: stays short");
  assert.equal(zoneOf(short, W, { right: true, left: false, leftLong: true }), null, "no short action: nothing until long");
  assert.equal(zoneOf(-W * 0.7, W, { right: true, left: false, leftLong: true }), "leftLong");
  assert.equal(zoneOf(200, W, { right: false, left: true, leftLong: true }), null);
});

test("the short threshold scales with the row but stays within bounds", () => {
  assert.equal(shortThreshold(200), SWIPE.SHORT_MIN_PX);
  assert.equal(shortThreshold(400), 400 * SWIPE.SHORT_FRAC);
  assert.equal(shortThreshold(900), SWIPE.SHORT_MAX_PX);
});
