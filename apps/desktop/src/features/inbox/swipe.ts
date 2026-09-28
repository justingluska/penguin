// Trackpad swipe gesture on a list row, as a pure state machine (no DOM, no
// React) so it can be unit-tested (tests/swipe.test.ts). The DOM side lives in
// useRowSwipe.ts.
//
// Wheel events are the only signal a trackpad two-finger swipe gives a web
// view: there is no "begin" or "end", and macOS keeps sending momentum deltas
// after the fingers lift. So:
//   - a gesture starts with the first event after a quiet period, and locks to
//     an axis once it has moved a few pixels; mostly-vertical gestures are
//     ignored until they end (scrolling never swipes);
//   - it ends after IDLE_MS without events (momentum included);
//   - after it ends, a short cooldown swallows stray momentum tails, so a
//     committed row can't be swiped (and committed) again by leftover deltas.
//
// The row follows the fingers 1:1 (`raw`), and rubber-bands past its limits:
// the far end of the row's travel, or at once on a side with no action, so a
// dead side resists instead of sliding freely. A fast flick toward a side
// commits its first action even short of the threshold (`peak`, the fastest
// outward speed since the last change of direction).
//
// Keep this file free of imports and non-erasable TS syntax: the tests run it
// directly under Node's type stripping.

/** Where the row sits relative to the thresholds; null = below all of them. */
export type SwipeZone = "right" | "left" | "leftLong" | null;

export type SwipePhase = "idle" | "deciding" | "tracking" | "ignoring" | "cooldown";

export interface SwipeState {
  phase: SwipePhase;
  /** Row offset in px as shown (rubber-banded); positive = row moved right (a right swipe). */
  offset: number;
  /** Finger travel in px, before rubber-banding. */
  raw: number;
  /** Movement accumulated while deciding the axis. */
  ax: number;
  ay: number;
  /** Time of the last event (ms). */
  last: number;
  /** In cooldown, events before this time are swallowed. */
  until: number;
  /** Smoothed horizontal speed of the row, px/ms (positive = moving right). */
  v: number;
  /** Fastest speed since the last change of direction (signed like `v`). */
  peak: number;
}

/** Which zones have an action configured ("none" disables a zone). */
export interface SwipeAvail {
  right: boolean;
  left: boolean;
  leftLong: boolean;
}

export const SWIPE = {
  /** Quiet time that ends a gesture. */
  IDLE_MS: 120,
  /** Movement needed before the axis is decided. */
  DECIDE_PX: 8,
  /** Horizontal must beat vertical by this factor to count as a swipe. */
  AXIS_RATIO: 1.2,
  /** Short action threshold: this fraction of the row width, within SHORT_MIN_PX..SHORT_MAX_PX
   *  (a wide one-line row shouldn't need a much longer swipe than a narrow one). */
  SHORT_FRAC: 0.22,
  SHORT_MIN_PX: 64,
  SHORT_MAX_PX: 112,
  /** Long (left) action threshold, as a fraction of the row width. */
  LONG_FRAC: 0.6,
  /** The row travels freely up to this fraction of its width, then rubber-bands. */
  MAX_FRAC: 0.92,
  /** Past its limit the row gives at most this much more, however far the fingers go. */
  RUBBER_PX: 24,
  /** A side with no action gives at most this much (it resists from the start). */
  DEAD_PX: 40,
  /** A release this fast (px/ms) toward a side commits its first action… */
  FLICK_V: 0.9,
  /** …once the row has moved at least this fraction of the short threshold. */
  FLICK_MIN_FRAC: 0.4,
  /** After a gesture ends, events are swallowed for this long (extended by each). */
  COOLDOWN_MS: 300,
} as const;

const ALL: SwipeAvail = { right: true, left: true, leftLong: true };

export function initialSwipe(): SwipeState {
  return { phase: "idle", offset: 0, raw: 0, ax: 0, ay: 0, last: 0, until: 0, v: 0, peak: 0 };
}

/** The zone an offset falls in for a row of `width`, given the configured actions. */
export function zoneOf(offset: number, width: number, avail: SwipeAvail): SwipeZone {
  const short = shortThreshold(width);
  const long = width * SWIPE.LONG_FRAC;
  if (offset >= short) return avail.right ? "right" : null;
  if (offset <= -long && avail.leftLong) return "leftLong";
  if (offset <= -short && avail.left) return "left";
  return null;
}

/** How far (0..1) the row is toward arming its side's first action. */
export function swipeProgress(offset: number, width: number): number {
  return Math.min(1, Math.abs(offset) / shortThreshold(width));
}

/**
 * Feed one wheel event (deltas in px, as the browser reports them: a swipe
 * to the right produces negative deltaX). Returns the next state.
 */
export function feedSwipe(s: SwipeState, dx: number, dy: number, now: number, width: number, avail: SwipeAvail = ALL): SwipeState {
  // A gap longer than IDLE_MS means the previous gesture ended unobserved.
  let st = s;
  if (st.phase !== "idle" && st.phase !== "cooldown" && now - st.last > SWIPE.IDLE_MS) st = initialSwipe();
  if (st.phase === "cooldown") {
    if (now < st.until) return { ...st, last: now, until: now + SWIPE.IDLE_MS };
    st = initialSwipe();
  }

  switch (st.phase) {
    case "idle":
    case "deciding": {
      const ax = st.ax + dx;
      const ay = st.ay + dy;
      if (Math.abs(ax) + Math.abs(ay) < SWIPE.DECIDE_PX) return { ...st, phase: "deciding", ax, ay, last: now };
      if (Math.abs(ax) > Math.abs(ay) * SWIPE.AXIS_RATIO) {
        const raw = -ax;
        return { ...st, phase: "tracking", raw, offset: rubberSwipe(raw, width, avail), ax, ay, last: now, v: 0, peak: 0 };
      }
      return { ...st, phase: "ignoring", ax, ay, last: now };
    }
    case "tracking": {
      const raw = st.raw - dx;
      // Wheel events come every ~8–16 ms; a longer gap says little about speed.
      const dt = Math.min(Math.max(now - st.last, 4), 50);
      const v = st.v * 0.5 + (-dx / dt) * 0.5;
      const reversed = Math.sign(v) !== Math.sign(st.peak) && v !== 0;
      const peak = reversed || Math.abs(v) > Math.abs(st.peak) ? v : st.peak;
      return { ...st, raw, offset: rubberSwipe(raw, width, avail), last: now, v, peak };
    }
    case "ignoring":
      return { ...st, last: now };
  }
  return st;
}

/**
 * The gesture ended (IDLE_MS passed with no events). Returns the zone to
 * commit (null = spring back, nothing happens) and the cooldown state.
 */
export function releaseSwipe(
  s: SwipeState,
  now: number,
  width: number,
  avail: SwipeAvail,
): { zone: SwipeZone; next: SwipeState } {
  // Only a horizontal gesture leaves a cooldown; a scroll can be followed by a swipe right away.
  if (s.phase !== "tracking") return { zone: null, next: initialSwipe() };
  let zone = zoneOf(s.offset, width, avail);
  if (zone === null) zone = flickZone(s, width, avail);
  return { zone, next: { ...initialSwipe(), phase: "cooldown", last: now, until: now + SWIPE.COOLDOWN_MS } };
}

/** A fast flick outward, past a minimum distance, commits the side's first action. */
function flickZone(s: SwipeState, width: number, avail: SwipeAvail): SwipeZone {
  const outward = Math.sign(s.peak) === Math.sign(s.offset) && s.offset !== 0;
  if (!outward || Math.abs(s.peak) < SWIPE.FLICK_V) return null;
  if (Math.abs(s.offset) < shortThreshold(width) * SWIPE.FLICK_MIN_FRAC) return null;
  if (s.offset > 0) return avail.right ? "right" : null;
  return avail.left ? "left" : avail.leftLong ? "leftLong" : null;
}

export function shortThreshold(width: number): number {
  return Math.min(SWIPE.SHORT_MAX_PX, Math.max(SWIPE.SHORT_MIN_PX, width * SWIPE.SHORT_FRAC));
}

/**
 * Where the row shows for a finger travel of `raw`: 1:1 up to its limit, then
 * a rubber band that gives less the further you pull (never more than
 * RUBBER_PX past it). A side with no action has no free travel at all.
 */
export function rubberSwipe(raw: number, width: number, avail: SwipeAvail = ALL): number {
  if (raw === 0) return 0;
  const has = raw > 0 ? avail.right : avail.left || avail.leftLong;
  const limit = has ? width * SWIPE.MAX_FRAC : 0;
  const give = has ? SWIPE.RUBBER_PX : SWIPE.DEAD_PX;
  const x = Math.abs(raw);
  const shown = x <= limit ? x : limit + band(x - limit, give);
  return Math.sign(raw) * shown;
}

/** Slope 1 at 0, approaching `give` as `over` grows. */
function band(over: number, give: number): number {
  return (over * give) / (over + give);
}
