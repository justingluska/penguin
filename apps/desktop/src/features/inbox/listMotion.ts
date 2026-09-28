// Row motion for the virtualized thread list, as pure bookkeeping (no DOM, no
// React) so it can be unit-tested (tests/listMotion.test.ts). ThreadList.tsx
// renders what this plans; the CSS is in styles/app.css ("List motion").
//
// The list's data changes at once (an archive removes the row from the store
// immediately); motion is purely visual and never holds a state change back:
//   - a removed row that was on screen stays rendered as a "ghost" for LIFE_MS:
//     same React key, so its DOM node (and a swipe's fly-off already under way)
//     carries on; it collapses in place while the rows below glide up;
//   - a row inserted among existing ones (undo, new mail) opens in place while
//     the rows below glide down; rows appended by paging just appear;
//   - a row that moved (a thread jumping to the top on a new message) appears
//     at its new place without gliding across the list.
// A ghost is positioned by its "anchor", the first surviving row after it, so
// several quick removals (E E E E E) stack their ghosts in the one gap and the
// rows below retarget smoothly instead of jumping.
//
// Keep this file free of imports and non-erasable TS syntax: the tests run it
// directly under Node's type stripping.

/** How a removed row leaves: a short slide (keyboard archive/trash, matching
 *  the swipe side that action lives on), a fade, or nothing extra (a swipe
 *  already flew it off). */
export type ExitStyle = "left" | "right" | "fade" | "swipe";

export interface Ghost<T> {
  key: string;
  item: T;
  /** First surviving row after it (it sits where that row now starts), or null at the end. */
  anchor: string | null;
  /** Last surviving row before it (used when there is no anchor). */
  after: string | null;
  born: number;
  exit: ExitStyle;
}

export interface ListMotion<T> {
  listKey: string;
  rows: readonly T[];
  keys: readonly string[];
  index: ReadonlyMap<string, number>;
  ghosts: readonly Ghost<T>[];
  /** Rows opening in place (key → start time). */
  entering: ReadonlyMap<string, number>;
  /** Rows that moved: shown at the new place without gliding there. */
  noGlide: ReadonlySet<string>;
  /** Recently removed keys (→ time): coming back (undo) animates even at the end. */
  recent: ReadonlyMap<string, number>;
  /** Rows glide to new positions until this time. */
  flowUntil: number;
}

export interface MotionInput<T> {
  listKey: string;
  rows: readonly T[];
  keys: readonly string[];
  index: ReadonlyMap<string, number>;
}

export interface MotionContext {
  now: number;
  /** Keys rendered at the last commit (only those can leave visibly). */
  rendered: ReadonlySet<string>;
  /** False under prefers-reduced-motion: everything is instant. */
  motion: boolean;
  exitOf: (key: string) => ExitStyle;
}

export interface MotionPlan<T> {
  state: ListMotion<T>;
  /** Keys inserted among existing rows that weren't just removed (new mail), in list order. */
  added: string[];
  /** Whether any row changed place (keys added, removed or reordered). */
  changed: boolean;
}

export const MOTION = {
  /** A ghost's life: the fold (--dur-row), after a swipe's fly-off head start
   *  (SWIPE_DELAY in styles/app.css), plus a frame or two of slack. */
  LIFE_MS: 330,
  /** An entering row keeps its class this long (its animation is --dur-row). */
  ENTER_MS: 240,
  /** Rows keep their glide transition this long after the last change. */
  FLOW_MS: 330,
  /** An undo within this long of the removal animates the row back in. */
  RECENT_MS: 12_000,
  /** More rows than this entering at once (a reload) just appear. */
  MAX_ENTERING: 60,
} as const;

const EMPTY_MAP: ReadonlyMap<string, number> = new Map();
const EMPTY_SET: ReadonlySet<string> = new Set();

export function initialMotion<T>(input: MotionInput<T>): ListMotion<T> {
  return { ...input, ghosts: [], entering: EMPTY_MAP, noGlide: EMPTY_SET, recent: EMPTY_MAP, flowUntil: 0 };
}

/** The first key at or after prev index `i` that survives in `next`. */
function survivorFrom(prevKeys: readonly string[], i: number, next: ReadonlyMap<string, number>, step: 1 | -1): string | null {
  for (let j = i; j >= 0 && j < prevKeys.length; j += step) {
    if (next.has(prevKeys[j])) return prevKeys[j];
  }
  return null;
}

/** Where a ghost's anchor went: itself if it survived, else the next survivor after it. */
function reanchor(key: string | null, prev: ListMotion<unknown>, next: ReadonlyMap<string, number>, step: 1 | -1): string | null {
  if (key === null || next.has(key)) return key;
  const i = prev.index.get(key);
  return i === undefined ? null : survivorFrom(prev.keys, i + step, next, step);
}

/** Plan the motion for a new list (called when the list's rows change). */
export function planMotion<T>(prev: ListMotion<T>, input: MotionInput<T>, ctx: MotionContext): MotionPlan<T> {
  const { now } = ctx;
  // A different mailbox, the first load or reduced motion: no motion at all.
  if (!ctx.motion || prev.listKey !== input.listKey || prev.keys.length === 0) {
    return { state: initialMotion(input), added: [], changed: false };
  }
  const next = input.index;

  // Removed rows that were on screen become ghosts, in their old order.
  const removed: { key: string; i: number }[] = [];
  let removedAny = prev.keys.length > input.keys.length;
  for (const key of ctx.rendered) {
    const i = prev.index.get(key);
    if (i !== undefined && !next.has(key)) removed.push({ key, i });
  }
  removed.sort((a, b) => a.i - b.i);
  if (removed.length) removedAny = true;

  // Live ghosts: drop expired ones and ones whose row came back; follow their anchors.
  const ghosts: Ghost<T>[] = [];
  for (const g of prev.ghosts) {
    if (now - g.born >= MOTION.LIFE_MS || next.has(g.key)) continue;
    const anchor = reanchor(g.anchor, prev, next, 1);
    const after = reanchor(g.after, prev, next, -1);
    ghosts.push(anchor === g.anchor && after === g.after ? g : { ...g, anchor, after });
  }
  for (const { key, i } of removed) {
    const item = prev.rows[i];
    ghosts.push({
      key,
      item,
      anchor: survivorFrom(prev.keys, i + 1, next, 1),
      after: survivorFrom(prev.keys, i - 1, next, -1),
      born: now,
      exit: ctx.exitOf(key),
    });
  }

  const recent = new Map<string, number>();
  for (const [k, t] of prev.recent) if (now - t < MOTION.RECENT_MS && !next.has(k)) recent.set(k, t);
  for (const { key } of removed) recent.set(key, now);

  // Insertions. Rows past the last surviving row were appended (paging): no motion.
  const lastSurvivor = survivorFrom(prev.keys, prev.keys.length - 1, next, -1);
  const tail = lastSurvivor === null ? -1 : next.get(lastSurvivor)!;
  const entering = new Map<string, number>();
  for (const [k, t] of prev.entering) if (now - t < MOTION.ENTER_MS && next.has(k)) entering.set(k, t);
  const added: string[] = [];
  const opening: string[] = [];
  let addedAny = false;
  for (let i = 0; i < input.keys.length; i++) {
    const k = input.keys[i];
    if (prev.index.has(k)) continue;
    addedAny = true;
    const back = prev.recent.has(k) || prev.ghosts.some((g) => g.key === k);
    if (i > tail && !back) continue;
    opening.push(k);
    if (!back) added.push(k);
  }

  // Rows that moved against the others (among those on screen): no glide, they open in place.
  const noGlide = new Set<string>();
  const onScreen = [...ctx.rendered].filter((k) => prev.index.has(k) && next.has(k)).sort((a, b) => prev.index.get(a)! - prev.index.get(b)!);
  let high = -1;
  for (const k of onScreen) {
    const j = next.get(k)!;
    if (j < high) noGlide.add(k);
    else high = j;
  }

  const changed = removedAny || addedAny || noGlide.size > 0;
  if (!changed) {
    return { state: { ...prev, ...input, ghosts, recent, entering }, added: [], changed: false };
  }
  if (opening.length + noGlide.size <= MOTION.MAX_ENTERING) {
    for (const k of opening) entering.set(k, now);
    for (const k of noGlide) entering.set(k, now);
  }
  return {
    state: { ...input, ghosts, entering, noGlide, recent, flowUntil: now + MOTION.FLOW_MS },
    added,
    changed: true,
  };
}

/** Drop what has finished; returns the same object when nothing did. */
export function pruneMotion<T>(m: ListMotion<T>, now: number): ListMotion<T> {
  const ghosts = m.ghosts.filter((g) => now - g.born < MOTION.LIFE_MS);
  let entering = m.entering;
  if ([...m.entering.values()].some((t) => now - t >= MOTION.ENTER_MS)) {
    entering = new Map([...m.entering].filter(([, t]) => now - t < MOTION.ENTER_MS));
  }
  const flowDone = m.flowUntil !== 0 && now >= m.flowUntil;
  if (ghosts.length === m.ghosts.length && entering === m.entering && !flowDone) return m;
  return {
    ...m,
    ghosts,
    entering,
    noGlide: entering.size === 0 ? EMPTY_SET : m.noGlide,
    flowUntil: flowDone ? 0 : m.flowUntil,
  };
}

/** When something in `m` next needs to be dropped (null = nothing pending). */
export function nextExpiry(m: ListMotion<unknown>): number | null {
  let t = Infinity;
  for (const g of m.ghosts) t = Math.min(t, g.born + MOTION.LIFE_MS);
  for (const s of m.entering.values()) t = Math.min(t, s + MOTION.ENTER_MS);
  if (m.flowUntil) t = Math.min(t, m.flowUntil);
  return t === Infinity ? null : t;
}
