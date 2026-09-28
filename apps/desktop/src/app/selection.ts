// Multi-select in the thread list (the list and Floe). The state machine is in
// selectionModel.ts; this is the store, the hooks, and the glue to the list:
// ranges follow the list's order, rows that leave the list drop out, and
// switching views, tabs, accounts or profiles clears the selection.
//
// Rows subscribe to their own "am I checked" (a Set lookup), so selecting
// 1,000+ threads re-renders only the ~40 rows the virtualizer has mounted.
import { useSyncExternalStore } from "react";
import type { ThreadRef } from "../lib/types";
import { getUi, subscribeUi } from "../lib/ui";
import { list, listAllInView } from "./store";
import {
  EMPTY_SELECTION,
  isEmpty,
  prune,
  selectAll,
  selectAllMatching as selectAllMatchingModel,
  selectRange as selectRangeModel,
  toggle,
  type SelectionState,
} from "./selectionModel";

let state: SelectionState = EMPTY_SELECTION;
const subs = new Set<() => void>();
function set(next: SelectionState) {
  if (next === state) return;
  state = next;
  subs.forEach((f) => f());
}
function subscribe(cb: () => void) {
  subs.add(cb);
  return () => subs.delete(cb);
}

export const keyOf = (r: ThreadRef) => r.accountId + "\u0000" + r.threadId;
const refOf = (k: string): ThreadRef => {
  const i = k.indexOf("\u0000");
  return { accountId: k.slice(0, i), threadId: k.slice(i + 1) };
};
const order = () => list.get().items.map(keyOf);

export function getSelection(): SelectionState {
  return state;
}
export function useSelection<T>(select: (s: SelectionState) => T): T {
  return useSyncExternalStore(subscribe, () => select(state));
}
export function hasSelection(): boolean {
  return !isEmpty(state);
}
export function useHasSelection(): boolean {
  return useSelection((s) => !isEmpty(s));
}
export function selectionCount(): number {
  return state.keys.size;
}
export function useSelectionCount(): number {
  return useSelection((s) => s.keys.size);
}
export function isSelected(ref: ThreadRef): boolean {
  return state.keys.has(keyOf(ref));
}
export function useIsSelected(ref: ThreadRef): boolean {
  const k = keyOf(ref);
  return useSelection((s) => s.keys.has(k));
}
/** The loaded selected threads, in list order. */
export function selectionRefs(): ThreadRef[] {
  if (state.keys.size === 0) return [];
  return list
    .get()
    .items.filter((t) => state.keys.has(keyOf(t)))
    .map((t) => ({ accountId: t.accountId, threadId: t.threadId }));
}

export function toggleSelect(ref: ThreadRef) {
  set(toggle(state, keyOf(ref)));
}
export function selectRange(to: ThreadRef) {
  set(selectRangeModel(state, keyOf(to), order()));
}
export function selectAllLoaded() {
  set(selectAll(state, order()));
}
export function selectAllMatching() {
  set(selectAllMatchingModel(state, order()));
}
export function clearSelection() {
  set(EMPTY_SELECTION);
}

/**
 * The threads an action on the selection applies to: with "all matching" on,
 * every thread in the view (paged from the backend), else the loaded selection.
 */
export async function selectionTargets(): Promise<ThreadRef[]> {
  if (!state.allMatching) return selectionRefs();
  const all = await listAllInView();
  // Anything unchecked since (toggle turns "all matching" off) is honored by
  // the model; here the whole view is meant.
  return all.length ? all : [...state.keys].map(refOf);
}

// Rows that leave the list drop out of the selection.
let lastItems = list.get().items;
list.subscribe(() => {
  const items = list.get().items;
  if (items === lastItems) return;
  lastItems = items;
  set(prune(state, items.map(keyOf)));
});

// A different view, tab, account or profile starts with nothing selected.
const scopeOf = () => {
  const ui = getUi();
  return JSON.stringify([ui.view, ui.split, ui.accountFilter, ui.profileId]);
};
let lastScope = scopeOf();
subscribeUi(() => {
  const scope = scopeOf();
  if (scope === lastScope) return;
  lastScope = scope;
  if (!isEmpty(state)) clearSelection();
});
