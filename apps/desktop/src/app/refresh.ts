// "Refresh" (the list header button, ⇧R): poke every account's sync and
// follow it through to a result, so the click gets visible feedback: a
// progress toast while it runs ("Checking 3 accounts…"), then "3 new
// conversations" / "Everything's up to date", and any account that needs a
// fix as its own error toast (app/syncToasts.ts). Driven by what the backend
// already emits: an account is done when its sync-status comes back with a newer lastSyncedAt
// (the engine stamps it at the end of each incremental poll), or in
// error/needsReauth. New mail is counted from mail-changed thread ids that
// weren't in the list before and are in it now.
import { useSyncExternalStore } from "react";
import { api, asCommandError, onMailChanged } from "../lib/api";
import type { SyncStatus } from "../lib/types";
import { dismissToastKey, toast } from "../components/Toast";
import { accountName } from "../components/Identity";
import { list, meta } from "./store";
import { showSyncIssues } from "./syncToasts";

export type RefreshAccountState =
  /** Poked, waiting for its poll to finish. */
  | "checking"
  /** Its poll finished. */
  | "done"
  /** Still backfilling: the poke is folded into the backfill, nothing to wait for. */
  | "backfilling"
  /** error / needsReauth: needs a fix, not a wait. */
  | "attention"
  /** No answer within SLOW_MS; it keeps syncing in the background. */
  | "slow";

export interface RefreshState {
  /** A refresh is in flight (the button spins). */
  active: boolean;
  startedAt: number;
  finishedAt: number | null;
  order: string[];
  accounts: Record<string, RefreshAccountState>;
  /** Thread ids reported changed during the refresh, per account. */
  changed: Record<string, string[]>;
}

const SLOW_MS = 20_000;
const SETTLE_MS = 350;
/** How long the result toast stays. */
const RESULT_MS = 2_000;
const TOAST_KEY = "refresh";

let state: RefreshState = { active: false, startedAt: 0, finishedAt: null, order: [], accounts: {}, changed: {} };
const subs = new Set<() => void>();
function set(patch: Partial<RefreshState>) {
  state = { ...state, ...patch };
  subs.forEach((f) => f());
}

export function getRefresh(): RefreshState {
  return state;
}
export function useRefresh<T>(select: (s: RefreshState) => T): T {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => select(state),
  );
}

/** Threads in the list when the refresh started ("new" = changed and not among these). */
let before = new Set<string>();
const refKey = (accountId: string, threadId: string) => accountId + "\u0000" + threadId;

let cleanup: (() => void) | null = null;

const needsFix = (s: SyncStatus | undefined) => !!s && (s.phase === "error" || s.phase === "needsReauth");

export function startRefresh() {
  if (state.active) return; // the progress toast is already up
  cleanup?.();
  const startedAt = Date.now();
  const { accounts, sync } = meta.get();
  const baseline: Record<string, number> = {};
  const acc: Record<string, RefreshAccountState> = {};
  for (const a of accounts) {
    const s = sync[a.id];
    baseline[a.id] = s?.lastSyncedAt ?? 0;
    acc[a.id] = needsFix(s) ? "attention" : s?.phase === "backfilling" ? "backfilling" : "checking";
  }
  before = new Set(list.get().items.map((t) => refKey(t.accountId, t.threadId)));
  set({ active: true, startedAt, finishedAt: null, order: accounts.map((a) => a.id), accounts: acc, changed: {} });
  showProgress();

  // Follow sync statuses (App feeds every sync-status event into meta.sync).
  const settling = new Set<string>();
  const mark = (id: string, to: RefreshAccountState) => {
    settling.delete(id);
    if (!state.active || (state.accounts[id] !== "checking" && state.accounts[id] !== "slow")) return;
    set({ accounts: { ...state.accounts, [id]: to } });
    showProgress();
    maybeFinish();
  };
  const unsubMeta = meta.subscribe(() => {
    if (!state.active) return;
    const sync = meta.get().sync;
    for (const id of state.order) {
      const cur = state.accounts[id];
      if ((cur !== "checking" && cur !== "slow") || settling.has(id)) continue;
      const s = sync[id];
      if (needsFix(s)) mark(id, "attention");
      else if (s && (s.lastSyncedAt ?? 0) > baseline[id]) {
        // With new mail, give the list refresh (debounced in App) a moment to
        // land so the row goes straight to "2 new" rather than via "2 updated".
        if (state.changed[id]?.length) {
          settling.add(id);
          setTimeout(() => mark(id, "done"), SETTLE_MS);
        } else mark(id, "done");
      }
    }
  });
  let offMail: (() => void) | null = null;
  let disposed = false;
  void onMailChanged((e) => {
    if (!state.active && state.finishedAt === null) return;
    const prev = state.changed[e.accountId] ?? [];
    set({ changed: { ...state.changed, [e.accountId]: [...new Set([...prev, ...e.threadIds])] } });
  }).then((off) => (disposed ? off() : (offMail = off)));
  const slow = setTimeout(() => {
    const next = { ...state.accounts };
    for (const id of state.order) if (next[id] === "checking") next[id] = "slow";
    set({ accounts: next });
    maybeFinish(true);
  }, SLOW_MS);

  cleanup = () => {
    unsubMeta();
    clearTimeout(slow);
    disposed = true;
    offMail?.();
    cleanup = null;
  };

  api.syncNow().catch((e) => {
    cleanup?.();
    set({ active: false });
    dismissToastKey(TOAST_KEY);
    toast({ kind: "error", message: `Couldn't check for mail: ${asCommandError(e).message}` });
  });
  maybeFinish();
}

function maybeFinish(force = false) {
  if (!state.active) return;
  const waiting = state.order.some((id) => state.accounts[id] === "checking");
  if (waiting && !force) return;
  set({ active: false, finishedAt: Date.now() });
  // Keep listening briefly: late mail-changed events still count.
  setTimeout(() => cleanup?.(), 1_000);
  showResult();
}

const nameOf = (id: string) => {
  const { accounts } = meta.get();
  const a = accounts.find((x) => x.id === id);
  return a ? accountName(a, accounts) : id;
};

function showProgress() {
  const n = state.order.length;
  const done = state.order.filter((id) => state.accounts[id] !== "checking").length;
  toast({
    kind: "progress",
    key: TOAST_KEY,
    message: `Checking ${n} ${n === 1 ? "account" : "accounts"}…`,
    detail: n > 1 ? `${done} of ${n} done` : undefined,
    progress: n > 1 ? done / n : null,
  });
}

function showResult() {
  const items = list.get().items;
  const parts: string[] = [];
  let fresh = 0;
  for (const id of state.order) {
    const c = refreshCounts(id, items).fresh;
    if (c > 0) parts.push(`${nameOf(id)} ${c}`);
    fresh += c;
  }
  const slow = state.order.filter((id) => state.accounts[id] === "slow").map(nameOf);
  const broken = state.order.filter((id) => state.accounts[id] === "attention").length;
  const checked = state.order.length - broken;
  if (checked === 0) {
    // Nothing could be checked: the sync-issue toasts say why.
    dismissToastKey(TOAST_KEY);
  } else {
    const detail = [parts.join(" · "), slow.length ? `${slow.join(", ")} still syncing` : ""].filter(Boolean).join(" · ");
    toast({
      kind: "success",
      key: TOAST_KEY,
      message: fresh > 0 ? `${fresh} new ${fresh === 1 ? "conversation" : "conversations"}` : "Everything's up to date",
      detail: detail || undefined,
      progress: undefined,
      duration: RESULT_MS + (detail ? 1_000 : 0),
    });
  }
  if (broken > 0) showSyncIssues();
}

/** New threads (changed during the refresh, absent before, in the list now) and other changes. */
function refreshCounts(accountId: string, items: { accountId: string; threadId: string }[]): { fresh: number; updated: number } {
  const ids = state.changed[accountId] ?? [];
  if (ids.length === 0) return { fresh: 0, updated: 0 };
  const now = new Set(items.map((t) => refKey(t.accountId, t.threadId)));
  let fresh = 0;
  for (const id of ids) {
    const k = refKey(accountId, id);
    if (!before.has(k) && now.has(k)) fresh++;
  }
  return { fresh, updated: ids.length - fresh };
}
