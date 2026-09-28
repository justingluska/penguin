// Geometry of the toast deck (components/Toast.tsx), as a pure function so it
// can be unit-tested (tests/toastStack.test.ts). Keep it free of imports and
// non-erasable TS syntax: the tests run it under Node's type stripping.
//
// Every card is absolutely positioned with its bottom edge on the host's
// bottom edge; only transform, opacity and clip-path move it (no layout).
//
// Collapsed: the newest card sits in front. Older cards stand behind it, each
// PEEK px higher and a little smaller, sized to the FRONT card: a taller card
// is shifted down to line its top up and clipped at the bottom, so only its
// top edge peeks out whatever its own height.
// Expanded: the cards form a list upward from the front one, GAP apart.

export const STACK = {
  /** Cards shown (front + peeking); the rest are hidden behind and counted. */
  VISIBLE: 3,
  PEEK: 9,
  GAP: 8,
  /** Scale step per card behind the front. */
  SCALE_STEP: 0.05,
  /** Dimming step per card behind the front (an overlay, not opacity: a
   *  translucent card would show the one behind through it). */
  DIM_STEP: 0.12,
  /** Height used until a card has been measured. */
  DEFAULT_H: 52,
} as const;

export interface CardPlacement {
  /** translateY in px (negative = up). */
  y: number;
  scale: number;
  /** 0 = hidden (past the cap), else 1. */
  opacity: number;
  /** 0..1 dimming overlay (collapsed cards behind the front). */
  dim: number;
  /** clip-path inset from the bottom, px (in the card's own coordinates). */
  clipBottom: number;
  /** Stacking order: front highest. */
  z: number;
  /** Beyond the visible cap: not shown, not interactive. */
  hidden: boolean;
}

/**
 * Placements for cards ordered newest first. `heights[i]` is card i's
 * measured height (undefined until measured).
 */
export function stackLayout(heights: (number | undefined)[], expanded: boolean): CardPlacement[] {
  const h = heights.map((x) => x ?? STACK.DEFAULT_H);
  const front = h[0] ?? STACK.DEFAULT_H;
  const out: CardPlacement[] = [];
  let above = 0; // expanded: height of the cards below this one, plus gaps
  for (let i = 0; i < h.length; i++) {
    const hidden = i >= STACK.VISIBLE;
    const z = 1000 - i;
    if (expanded) {
      out.push({ y: above ? -above : 0, scale: 1, opacity: hidden ? 0 : 1, dim: 0, clipBottom: 0, z, hidden });
      if (!hidden) above += h[i] + STACK.GAP;
      continue;
    }
    // Behind cards past the cap sit exactly under the last visible one.
    const k = Math.min(i, STACK.VISIBLE - 1);
    const extra = h[i] - front; // > 0: taller than the front card
    out.push({
      y: extra - k * STACK.PEEK,
      scale: 1 - k * STACK.SCALE_STEP,
      opacity: hidden ? 0 : 1,
      dim: k * STACK.DIM_STEP,
      clipBottom: i === 0 ? 0 : Math.max(0, extra),
      z,
      hidden,
    });
  }
  return out;
}

/** Height the stack occupies (for the hover area / tests). */
export function stackHeight(heights: (number | undefined)[], expanded: boolean): number {
  const h = heights.map((x) => x ?? STACK.DEFAULT_H);
  if (h.length === 0) return 0;
  if (!expanded) return h[0] + Math.min(h.length - 1, STACK.VISIBLE - 1) * STACK.PEEK;
  const vis = h.slice(0, STACK.VISIBLE);
  return vis.reduce((a, b) => a + b, 0) + (vis.length - 1) * STACK.GAP;
}
