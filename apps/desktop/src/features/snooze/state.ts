// Active snoozes for the sidebar's Snoozed count, and the toasts when
// snoozed conversations come back (penguin://snooze-woke). Loaded lazily by
// the first component that asks; refreshed on mail-changed (snoozing,
// unsnoozing and early wakes all emit it) and on every wake.
import { useEffect, useSyncExternalStore } from "react";
import { api, onMailChanged, onSnoozeWoke } from "../../lib/api";
import type { Snooze, SnoozeWokeBatch } from "../../lib/types";
import { openThread } from "../../lib/ui";
import { toast } from "../../components/Toast";
import { num } from "../../lib/format";
import { isMainWindow } from "../../lib/thisWindow";

let snoozes: Snooze[] = [];
const subs = new Set<() => void>();
let started = false;
let timer: ReturnType<typeof setTimeout> | null = null;
let seq = 0;

async function load() {
  const mine = ++seq;
  try {
    const next = await api.listSnoozes(null);
    if (mine !== seq) return;
    snoozes = next;
    subs.forEach((f) => f());
  } catch {
    // Keep the last count; the next change retries.
  }
}

/**
 * Snooze/unsnooze shown at once (the sidebar's Snoozed count). A read already
 * under way may predate the change, so it's ignored; the change's own
 * mail-changed reads the backend's list back.
 */
export function snoozeLocal(refs: { accountId: string; threadId: string }[], wakeAt: number) {
  const keys = new Set(refs.map((r) => r.accountId + "\u0000" + r.threadId));
  const now = Date.now();
  seq++;
  snoozes = [
    ...snoozes.filter((z) => !keys.has(z.accountId + "\u0000" + z.threadId)),
    ...refs.map((r) => ({ accountId: r.accountId, threadId: r.threadId, wakeAt, snoozedAt: now })),
  ];
  subs.forEach((f) => f());
}

export function unsnoozeLocal(refs: { accountId: string; threadId: string }[]) {
  const keys = new Set(refs.map((r) => r.accountId + "\u0000" + r.threadId));
  seq++;
  snoozes = snoozes.filter((z) => !keys.has(z.accountId + "\u0000" + z.threadId));
  subs.forEach((f) => f());
}

/** Read the snoozes back (after an undo, or a failed call that emits no mail-changed). */
export function snoozesChanged() {
  reloadSoon();
}

/** Coalesce bursts (a sync batch emits many mail-changed events). */
function reloadSoon() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    void load();
  }, 200);
}

function start() {
  if (started) return;
  started = true;
  void onMailChanged(reloadSoon);
  void onSnoozeWoke((b) => {
    reloadSoon();
    // Once, in the main window (its Open shows the conversation there).
    if (isMainWindow) wokeToast(b);
  });
  void load();
}

/** Snoozed conversations in scope: one account, a profile's accounts, or all. */
export function useSnoozedCount(accountFilter: string | null, accountIds: string[] | null): number {
  useEffect(start, []);
  const all = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => snoozes,
  );
  return all.filter((z) => (accountFilter ? z.accountId === accountFilter : !accountIds || accountIds.includes(z.accountId))).length;
}

/** Mounted once (by the picker host), so wakes toast even before the sidebar asks. */
export function useSnoozeEvents() {
  useEffect(start, []);
}

export function wokeToast(b: SnoozeWokeBatch) {
  const n = b.items.length;
  if (n === 0) return;
  const late = b.missed > 0 ? `${b.missed === n ? "" : `${num(b.missed)} `}due while Penguin was closed` : undefined;
  if (n === 1) {
    const w = b.items[0];
    toast({
      kind: "action",
      key: `snooze-woke:${w.accountId}:${w.threadId}`,
      message: "Snoozed conversation is back",
      detail: [w.subject || "(no subject)", late].filter(Boolean).join(" · "),
      action: { label: "Open", run: () => openThread({ accountId: w.accountId, threadId: w.threadId }) },
      duration: 12_000,
    });
    return;
  }
  toast({
    kind: "action",
    key: "snooze-woke",
    message: `${num(n)} snoozed conversations are back`,
    detail: late ?? "They're at the top of your inbox",
    duration: 12_000,
  });
}
