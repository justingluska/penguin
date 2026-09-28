// Sidebar counts for Reply Later and Follow up (api.triageCounts, per
// account). Loaded by the first component that asks; refreshed, coalesced,
// on mail-changed (marking, sending, dismissing and sync all emit it) and when
// the Follow up wait changes in Settings. Follow up also moves with the clock,
// so it's re-read every 15 minutes.
import { useEffect, useSyncExternalStore } from "react";
import { api, onMailChanged } from "../../lib/api";
import type { TriageCount } from "../../lib/types";
import { currentSettings, subscribeSettings } from "../../lib/settings";
import { getUi } from "../../lib/ui";
import { refreshList } from "../../app/store";

let counts: TriageCount[] = [];
const subs = new Set<() => void>();
let started = false;
let timer: ReturnType<typeof setTimeout> | null = null;
let seq = 0;

async function load() {
  const mine = ++seq;
  try {
    const next = await api.triageCounts(null);
    if (mine !== seq) return;
    counts = next;
    subs.forEach((f) => f());
  } catch {
    // Keep the last counts; the next change retries.
  }
}

/**
 * Move the Reply Later / Follow up counts now, by `n` per target (a mark,
 * an undo). A read already under way may predate the change, so it's
 * ignored; the change's own mail-changed reads the counts back.
 */
export function shiftTriageCount(refs: { accountId: string }[], field: "replyLater" | "followUp", n: 1 | -1) {
  if (refs.length === 0) return;
  const by = new Map<string, number>();
  for (const r of refs) by.set(r.accountId, (by.get(r.accountId) ?? 0) + n);
  seq++;
  const next = counts.map((c) => (by.has(c.accountId) ? { ...c, [field]: Math.max(0, c[field] + by.get(c.accountId)!) } : c));
  for (const [accountId, d] of by)
    if (!counts.some((c) => c.accountId === accountId) && d > 0) next.push({ accountId, replyLater: 0, followUp: 0, [field]: d });
  counts = next;
  subs.forEach((f) => f());
}

/** Coalesce bursts (a sync batch emits many mail-changed events). */
export function reloadTriageSoon() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    void load();
  }, 300);
}

function start() {
  if (started) return;
  started = true;
  void onMailChanged(reloadTriageSoon);
  // A new wait changes both the count and the open Follow up list.
  let days = currentSettings().followUpDays;
  subscribeSettings(() => {
    if (currentSettings().followUpDays === days) return;
    days = currentSettings().followUpDays;
    reloadTriageSoon();
    if (getUi().view.kind === "followUp") void refreshList();
  });
  setInterval(reloadTriageSoon, 15 * 60_000);
  void load();
}

/** Reply Later and Follow up sizes in scope: one account, a set (a profile), or all. */
export function useTriageCounts(accountFilter: string | null, accountIds: string[] | null): { replyLater: number; followUp: number } {
  useEffect(start, []);
  const all = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => counts,
  );
  let replyLater = 0;
  let followUp = 0;
  for (const c of all) {
    if (accountFilter ? c.accountId !== accountFilter : accountIds && !accountIds.includes(c.accountId)) continue;
    replyLater += c.replyLater;
    followUp += c.followUp;
  }
  return { replyLater, followUp };
}
