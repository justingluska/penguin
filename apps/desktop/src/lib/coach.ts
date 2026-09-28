// Shortcut coach: when you do something with the mouse that has a key, a
// small hint shows that key (Settings → General "Shortcut coach"). This is
// the policy, pure so it runs in node tests (tests/coach.test.ts); the hint
// itself is app/ShortcutCoach.tsx.
//
// Rate limits, so it teaches without nagging:
// - one hint at a time, and none within GLOBAL_GAP_MS of the last one;
// - the same key at most once per KEY_GAP_MS, and MAX_HINTS times ever;
// - never again for a key you've pressed LEARNED_AFTER times yourself
//   (keys are counted when a shortcut runs from the keyboard);
// - never for a key you pressed within the last minute (you know it, you
//   just used the mouse this time).
// State is per device (localStorage): which keys you know is a habit of
// the hands at this keyboard, not a setting to sync.

export const COACH = {
  GLOBAL_GAP_MS: 20_000,
  KEY_GAP_MS: 10 * 60_000,
  MAX_HINTS: 4,
  LEARNED_AFTER: 2,
  RECENT_USE_MS: 60_000,
  /** How long a hint stays up. */
  SHOW_MS: 2600,
} as const;

export interface CoachState {
  /** Times each key spec ran a shortcut from the keyboard. */
  used: Record<string, number>;
  /** Last keyboard use of each key (unix ms). */
  usedAt: Record<string, number>;
  /** Hints shown per key spec, and when the last one was. */
  shown: Record<string, { n: number; last: number }>;
  /** When any hint was last shown. */
  lastHint: number;
}

export function emptyCoach(): CoachState {
  return { used: {}, usedAt: {}, shown: {}, lastHint: 0 };
}

/** Whether a mouse action with key `keys` should get a hint now. */
export function shouldHint(s: CoachState, keys: string, now: number): boolean {
  if (!keys) return false;
  if ((s.used[keys] ?? 0) >= COACH.LEARNED_AFTER) return false;
  if (now - (s.usedAt[keys] ?? -Infinity) < COACH.RECENT_USE_MS) return false;
  if (now - s.lastHint < COACH.GLOBAL_GAP_MS) return false;
  const shown = s.shown[keys];
  if (shown && (shown.n >= COACH.MAX_HINTS || now - shown.last < COACH.KEY_GAP_MS)) return false;
  return true;
}

export function recordHint(s: CoachState, keys: string, now: number): CoachState {
  const prev = s.shown[keys];
  return { ...s, lastHint: now, shown: { ...s.shown, [keys]: { n: (prev?.n ?? 0) + 1, last: now } } };
}

export function recordKeyUse(s: CoachState, keys: string, now: number): CoachState {
  return { ...s, used: { ...s.used, [keys]: (s.used[keys] ?? 0) + 1 }, usedAt: { ...s.usedAt, [keys]: now } };
}

/** Keys the coach will never hint again (you've learned them). */
export function learnedKeys(s: CoachState): string[] {
  return Object.entries(s.used)
    .filter(([, n]) => n >= COACH.LEARNED_AFTER)
    .map(([k]) => k)
    .sort();
}

/** A stored state, tolerant of anything malformed (starts over). */
export function parseCoach(raw: string | null): CoachState {
  if (!raw) return emptyCoach();
  try {
    const v = JSON.parse(raw) as Partial<CoachState>;
    const obj = (x: unknown) => (x && typeof x === "object" && !Array.isArray(x) ? (x as Record<string, never>) : {});
    return {
      used: obj(v.used),
      usedAt: obj(v.usedAt),
      shown: obj(v.shown),
      lastHint: typeof v.lastHint === "number" ? v.lastHint : 0,
    };
  } catch {
    return emptyCoach();
  }
}

const menuChoices = new Set<(keys: string, label: string) => void>();

/** A context-menu entry showing `keys` was chosen with the pointer (components/ContextMenu.tsx). */
export function notifyMenuChoice(keys: string | undefined, label: string) {
  if (keys) menuChoices.forEach((f) => f(keys, label));
}

/** The coach listens here while it's on. */
export function onMenuChoice(cb: (keys: string, label: string) => void): () => void {
  menuChoices.add(cb);
  return () => menuChoices.delete(cb);
}

/**
 * Where the hint goes: centered above `rect` (below it when there's no room),
 * kept inside the viewport. Returns the hint's top-left corner.
 */
export function placeHint(
  rect: { left: number; top: number; width: number; height: number },
  size: { width: number; height: number },
  viewport: { width: number; height: number },
): { left: number; top: number } {
  const gap = 8;
  const margin = 8;
  let top = rect.top - size.height - gap;
  if (top < margin) top = rect.top + rect.height + gap;
  top = Math.min(Math.max(margin, top), viewport.height - size.height - margin);
  let left = rect.left + rect.width / 2 - size.width / 2;
  left = Math.min(Math.max(margin, left), viewport.width - size.width - margin);
  return { left: Math.round(left), top: Math.round(top) };
}
