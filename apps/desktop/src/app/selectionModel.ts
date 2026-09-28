// Multi-select in the thread list, as a pure state machine (no React, no
// store) so it can be unit-tested (tests/selection.test.ts). The store and
// hooks live in app/selection.ts. Keep this file free of imports and
// non-erasable TS syntax: the tests run it under Node's type stripping.
//
// Threads are identified by key ("<accountId>\u0000<threadId>"); ranges use
// the list's current order, which the caller passes in.

export interface SelectionState {
  /** Selected thread keys. Replaced (never mutated) on every change. */
  keys: ReadonlySet<string>;
  /** Where a shift-range starts: the last row toggled or clicked. */
  anchor: string | null;
  /**
   * "Select all N matching": the whole view, including rows not loaded yet.
   * Actions expand it to every thread in the view when they run.
   */
  allMatching: boolean;
}

export const EMPTY_SELECTION: SelectionState = { keys: new Set(), anchor: null, allMatching: false };

/** x / ⌘-click / checkbox: flip one row, and make it the range anchor. */
export function toggle(s: SelectionState, key: string): SelectionState {
  const keys = new Set(s.keys);
  if (keys.has(key)) keys.delete(key);
  else keys.add(key);
  // Unselecting a row while "all matching" is on narrows back to the loaded rows.
  return { keys, anchor: key, allMatching: false };
}

/**
 * ⇧x / shift-click: select every row from the anchor to `key` (inclusive) in
 * list order, adding to what's already selected. Without an anchor (or when
 * the anchor is no longer listed) it selects just `key`.
 */
export function selectRange(s: SelectionState, key: string, order: readonly string[]): SelectionState {
  const to = order.indexOf(key);
  if (to < 0) return s;
  const from = s.anchor === null ? -1 : order.indexOf(s.anchor);
  const keys = new Set(s.keys);
  if (from < 0) {
    keys.add(key);
  } else {
    const [a, b] = from <= to ? [from, to] : [to, from];
    for (let i = a; i <= b; i++) keys.add(order[i]);
  }
  return { keys, anchor: key, allMatching: false };
}

/** ⌘A: every loaded row. */
export function selectAll(s: SelectionState, order: readonly string[]): SelectionState {
  return { keys: new Set(order), anchor: s.anchor, allMatching: false };
}

/** The banner's "Select all N matching": the whole view. */
export function selectAllMatching(s: SelectionState, order: readonly string[]): SelectionState {
  return { keys: new Set(order), anchor: s.anchor, allMatching: true };
}

/** Drop keys that left the list (archived elsewhere, filtered out). */
export function prune(s: SelectionState, order: readonly string[]): SelectionState {
  if (s.keys.size === 0) return s;
  const present = new Set(order);
  let gone = false;
  for (const k of s.keys) {
    if (!present.has(k)) {
      gone = true;
      break;
    }
  }
  if (!gone) return s;
  const keys = new Set([...s.keys].filter((k) => present.has(k)));
  return { keys, anchor: s.anchor !== null && present.has(s.anchor) ? s.anchor : null, allMatching: s.allMatching && keys.size > 0 };
}

export function isEmpty(s: SelectionState): boolean {
  return s.keys.size === 0 && !s.allMatching;
}
