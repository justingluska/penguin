// "Hide for 6 hours" on a sync alert, saved per device in localStorage and
// shared by every window through the storage event. The rules (which kind a
// hide covers, what breaks through) are in lib/syncHealth.ts; Settings → Sync
// lists what's hidden and shows it again.
import { useSyncExternalStore } from "react";
import type { SyncErrorKind, SyncStatus } from "./types";
import { liveHides, parseHides, withHide, type SyncHide } from "./syncHealth";

const KEY = "penguin.syncHides.v1";

function read(): SyncHide[] {
  try {
    return parseHides(localStorage.getItem(KEY));
  } catch {
    // Blocked storage (a locked-down web view): nothing is hidden.
    return [];
  }
}

let hides: SyncHide[] = read();
const subs = new Set<() => void>();
let timer: ReturnType<typeof setTimeout> | null = null;

function publish(next: SyncHide[]) {
  hides = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch (e) {
    // The hide still holds for this session; it just won't outlive it.
    console.warn("couldn't save hidden sync alerts", e);
  }
  changed();
}

/** Notify, and wake again when the next hide runs out so its alert comes back on time. */
function changed() {
  subs.forEach((f) => f());
  if (timer) clearTimeout(timer);
  timer = null;
  const now = Date.now();
  const next = Math.min(...hides.map((h) => h.until));
  if (Number.isFinite(next)) {
    timer = setTimeout(() => {
      hides = liveHides(hides);
      changed();
    }, Math.max(0, Math.min(next - now + 50, 2 ** 31 - 1)));
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== KEY) return;
    hides = read();
    changed();
  });
  changed();
}

export const getSyncHides = (): SyncHide[] => hides;

export function subscribeSyncHides(cb: () => void): () => void {
  subs.add(cb);
  return () => subs.delete(cb);
}

export function useSyncHides(): SyncHide[] {
  return useSyncExternalStore(subscribeSyncHides, getSyncHides);
}

/** Hide this status's alert (its account and error kind) for 6 hours. */
export function hideSyncAlert(s: SyncStatus): SyncHide {
  const next = withHide(hides, s);
  publish(next);
  return next[next.length - 1];
}

/** Show an account's alerts again (one kind, or all of them). */
export function unhideSync(accountId: string, kind?: SyncErrorKind) {
  publish(hides.filter((h) => h.accountId !== accountId || (kind !== undefined && h.kind !== kind)));
}
