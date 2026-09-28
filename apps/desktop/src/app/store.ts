// Mail data held by the UI: accounts, labels, sync status, the current thread
// list (paged, virtualized by the list component) and a small thread cache.
// Everything reads local state; the backend is only asked in the background.
import { useSyncExternalStore } from "react";
import { api } from "../lib/api";
import type {
  Account,
  Label,
  ListQuery,
  SplitCount,
  SplitFilter,
  SyncStatus,
  ThreadRef,
  ThreadSummary,
  ThreadView,
} from "../lib/types";
import { getUi, sameThread, setUi, subscribeUi, type UiState } from "../lib/ui";
import { currentSettings, getSettings, subscribeSettings } from "../lib/settings";
import { keepReadHere } from "./unreadFilter";
import { mailScope, marksReadOnOpen } from "./allInboxes";
import { OTHER, effectiveSplitId, splitFilterFor } from "./splits";
import { orderAccounts } from "./accountOrder";
import { addDeltas, applyCounts, countDeltas, createOpLog, sameLabelLook, sameValue, shareRows, type Op } from "./optimistic";

// ---------------------------------------------------------------------------
// Tiny store helper
// ---------------------------------------------------------------------------
function createStore<T extends object>(initial: T) {
  let state = initial;
  const subs = new Set<() => void>();
  const subscribe = (cb: () => void) => {
    subs.add(cb);
    return () => subs.delete(cb);
  };
  return {
    get: () => state,
    set(patch: Partial<T> | ((s: T) => Partial<T>)) {
      const p = typeof patch === "function" ? patch(state) : patch;
      state = { ...state, ...p };
      subs.forEach((f) => f());
    },
    subscribe,
    use<U>(select: (s: T) => U): U {
      return useSyncExternalStore(subscribe, () => select(state));
    },
  };
}

// ---------------------------------------------------------------------------
// Accounts, labels, sync
// ---------------------------------------------------------------------------
export interface MetaState {
  accounts: Account[];
  labels: Label[];
  sync: Record<string, SyncStatus>;
  /** Split Inbox tab counts by split id (and OTHER); null until counted. */
  splitCounts: Record<string, SplitCount> | null;
  /** The counts stopped at the backend's cap: they are a floor. */
  splitMore: boolean;
  /**
   * Bumped when what a label looks like changes (added, removed, renamed,
   * recolored, hidden), not when only its unread count moves. Rows and the
   * reading pane draw label chips: they subscribe to this, so marking a
   * conversation read doesn't re-render every row on screen.
   */
  labelLook: number;
}

export const meta = createStore<MetaState>({ accounts: [], labels: [], sync: {}, splitCounts: null, splitMore: false, labelLook: 0 });

/** Re-render when label chips would draw differently (names, colors, the set of labels); not on count changes. */
export function useLabelLook(): number {
  return meta.use((m) => m.labelLook);
}

let labelIndex = new Map<string, Label>();
let accountIndex = new Map<string, Account>();
let myEmails = new Set<string>();

export function accountById(id: string): Account | undefined {
  return accountIndex.get(id);
}
export function labelById(accountId: string, labelId: string): Label | undefined {
  return labelIndex.get(accountId + "\u0000" + labelId);
}
export function isMe(email: string): boolean {
  return myEmails.has(email.toLowerCase());
}

/**
 * Accounts of the active profile that are signed in, in the profile's order;
 * null when no profile is active (or it no longer exists). Account-set
 * filtering itself happens in the backend (ListQuery/SearchRequest.accountIds).
 */
export function profileScope(ui: UiState = getUi()): string[] | null {
  if (!ui.profileId) return null;
  const p = currentSettings().profiles.find((x) => x.id === ui.profileId);
  return p ? p.accountIds.filter((id) => accountIndex.has(id)) : null;
}

/**
 * The account set the mailbox views are limited to: the active profile's
 * accounts, or under "All accounts" every account not hidden from it
 * (Settings.hiddenFromAll); null = every account. See app/allInboxes.ts.
 */
export function listScope(ui: UiState = getUi()): string[] | null {
  return mailScope({
    accountFilter: ui.accountFilter,
    profileScope: profileScope(ui),
    accountIds: [...accountIndex.keys()],
    hidden: currentSettings().hiddenFromAll,
  });
}

/** Whether an account's mail is shown under the current account filter, profile and All Inboxes. */
export function accountInScope(accountId: string, ui: UiState = getUi()): boolean {
  if (ui.accountFilter) return ui.accountFilter === accountId;
  const scope = listScope(ui);
  return scope === null || scope.includes(accountId);
}

/**
 * Whether opening this conversation marks it read (app/allInboxes.ts
 * marksReadOnOpen): not for a hidden account's mail opened from outside that
 * account's own views.
 */
export function opensAsRead(ref: ThreadRef, ui: UiState = getUi()): boolean {
  return marksReadOnOpen(currentSettings().hiddenFromAll, ref.accountId, {
    outsideList: ui.selectedOutside,
    accountInView: accountInScope(ref.accountId, ui),
  });
}

// meta.accounts is kept in the user's order (Settings.accountOrder), so every
// reader lists accounts the same way. `incoming` is the list as the backend
// gave it; accounts the order doesn't name keep that order, after the rest.
let incoming: Account[] = [];
let appliedOrder: string[] | null = null;
let orderSub: (() => void) | null = null;

/** Whether the list shows one split: the Split Inbox is on (Settings) and the view is the inbox. */
export function splitsHere(ui: UiState = getUi()): boolean {
  return currentSettings().inboxTabs && ui.view.kind === "inbox";
}

/** The split tab the list shows (a split id or OTHER), or null for the whole list. */
export function currentSplit(ui: UiState = getUi()): string | null {
  return splitsHere(ui) ? effectiveSplitId(currentSettings().inboxSplits, ui.split) : null;
}

/** The list query's split filter (app/splits.ts), or null. */
function splitFilter(ui: UiState): SplitFilter | null {
  const id = currentSplit(ui);
  return id === null ? null : splitFilterFor(currentSettings().inboxSplits, id);
}

/** The accounts the Split Inbox counts cover: the picked account, else the list's scope. */
function countScope(ui: UiState): string[] | null {
  return ui.accountFilter ? [ui.accountFilter] : listScope(ui);
}

export function setAccounts(accounts: Account[]) {
  incoming = accounts;
  // Subscribed here, not at load: lib/settings imports this module (see startMail).
  orderSub ??= subscribeSettings(() => {
    if (currentSettings().accountOrder !== appliedOrder) publishAccounts();
  });
  publishAccounts();
}

function publishAccounts() {
  appliedOrder = currentSettings().accountOrder;
  const accounts = orderAccounts(incoming, appliedOrder);
  if (accounts.length === meta.get().accounts.length && accounts.every((a, i) => a === meta.get().accounts[i])) return;
  accountIndex = new Map(accounts.map((a) => [a.id, a]));
  myEmails = new Set(accounts.map((a) => a.email.toLowerCase()));
  meta.set({ accounts });
}

/** api.listAccounts() in the user's order, as meta.accounts is (for views that ask the backend afresh). */
export async function listOrderedAccounts(): Promise<Account[]> {
  const [accounts, settings] = await Promise.all([api.listAccounts(), getSettings()]);
  return orderAccounts(accounts, settings.accountOrder);
}

let labelsSeq = 0;
export async function loadLabels() {
  const seq = ++labelsSeq;
  const read = changes.readBegin();
  try {
    const fresh = await api.listLabels(null);
    if (seq !== labelsSeq) return;
    // Changes the backend didn't have yet when it counted stay applied.
    let d = new Map<string, number>();
    for (const op of changes.pending(read)) d = addDeltas(d, op.counts);
    setLabels(applyCounts(fresh, d));
  } finally {
    changes.readEnd(read);
  }
}

function setLabels(labels: Label[]) {
  const cur = meta.get().labels;
  if (labels === cur) return;
  // Unchanged labels keep their objects; an identical list keeps the array (no re-render).
  const old = new Map(cur.map((l) => [l.accountId + "\u0000" + l.id, l]));
  let same = labels.length === cur.length;
  const next = labels.map((l, i) => {
    const o = old.get(l.accountId + "\u0000" + l.id);
    const keep = o && sameValue(o, l) ? o : l;
    if (keep !== cur[i]) same = false;
    return keep;
  });
  if (same) return;
  labelIndex = new Map(next.map((l) => [l.accountId + "\u0000" + l.id, l]));
  meta.set((m) => ({ labels: next, labelLook: sameLabelLook(cur, next) ? m.labelLook : m.labelLook + 1 }));
}

export function applySyncStatus(s: SyncStatus) {
  meta.set((m) => ({ sync: { ...m.sync, [s.accountId]: s } }));
}

export async function loadSyncStatus() {
  const list = await api.syncStatus();
  meta.set({ sync: Object.fromEntries(list.map((s) => [s.accountId, s])) });
}

let countsSeq = 0;
/**
 * Split Inbox tab counts: conversations and unread in each split, then Other,
 * from one walk over the inbox in the backend (split_counts, bounded).
 */
export async function loadSplitCounts() {
  const seq = ++countsSeq;
  const settings = currentSettings();
  // Off (the default): nothing shows these counts, so skip the query.
  if (!settings.inboxTabs) return;
  const ui = getUi();
  const ids = [...settings.inboxSplits.map((x) => x.id), OTHER];
  const read = changes.readBegin();
  try {
    const r = await api.splitCounts(
      settings.inboxSplits.map((x) => x.query),
      countScope(ui),
    );
    if (seq !== countsSeq) return;
    const byId: Record<string, SplitCount> = {};
    ids.forEach((id, i) => (byId[id] = { ...(r.splits[i] ?? { total: 0, unread: 0 }) }));
    // Changes the backend may not have counted yet stay applied.
    for (const op of changes.pending(read)) {
      const d = op.patch?.split;
      const c = d && byId[d.id];
      if (c) byId[d.id] = { total: Math.max(0, c.total + d.total), unread: Math.max(0, c.unread + d.unread) };
    }
    meta.set({ splitCounts: byId, splitMore: r.more });
  } catch {
    // Counts are a nicety: a failed read keeps the last ones; the next change retries.
  } finally {
    changes.readEnd(read);
  }
}

// ---------------------------------------------------------------------------
// Thread list
// ---------------------------------------------------------------------------
export interface ListState {
  key: string;
  items: ThreadSummary[];
  loading: boolean;
  loaded: boolean;
  hasMore: boolean;
  error: string | null;
}

export const list = createStore<ListState>({ key: "", items: [], loading: false, loaded: false, hasMore: false, error: null });

const PAGE = 100;
const REFRESH_CAP = 1000;
let listSeq = 0;
let refreshSeq = 0;

const refKey = (r: ThreadRef) => r.accountId + "\u0000" + r.threadId;
/** Threads removed optimistically whose backend call hasn't settled yet. */
const suppressed = new Map<string, number>();

// ---------------------------------------------------------------------------
// Optimistic changes
// ---------------------------------------------------------------------------
// Every action changes what's on screen at once: the rows, the cached
// threads, and the unread counts (sidebar, accounts, profiles, tabs). The
// backend call runs behind it. Until it has settled, anything read back
// from the backend (a list page, a thread, the label counts) gets the change
// re-applied, so a read that raced the call can't flash the old state back
// (app/optimistic.ts). On failure the change is reverted and the truth is
// read back; mail-changed events keep everything in line with the backend.
type Patch = (t: ThreadSummary) => ThreadSummary;
interface ChangeData {
  patch: Patch | null;
  /** The rows as they were, and as this change left them (for revert). */
  before: Map<string, ThreadSummary>;
  after: Map<string, ThreadSummary>;
  /** The open split's counts moved by this much (rows left it, or were read). */
  split: SplitDelta | null;
}
export type Change = Op<ChangeData>;
const changes = createOpLog<ChangeData>();

/** Rows with the patches of changes a read begun as `read` may not have seen. */
function overlay(read: number, items: ThreadSummary[]): ThreadSummary[] {
  const ops = changes.pending(read).filter((o) => o.patch?.patch);
  if (ops.length === 0) return items;
  return items.map((t) => {
    const k = refKey(t);
    let x = t;
    for (const o of ops) if (o.keys.has(k)) x = o.patch!.patch!(x);
    return x;
  });
}

/** What we know of a thread: its row, else its cached copy. */
function knownSummary(k: string, rows: Map<string, ThreadSummary>): ThreadSummary | undefined {
  return rows.get(k) ?? (threadCache.has(k) ? summaryOf(threadCache.get(k)!) : undefined);
}

interface SplitDelta {
  id: string;
  total: number;
  unread: number;
}

/** Move the sidebar/account/profile counts and the open split's counts. */
function shiftCounts(d: Map<string, number>, split: SplitDelta | null, sign: 1 | -1) {
  if (d.size) setLabels(applyCounts(meta.get().labels, sign === 1 ? d : addDeltas(new Map(), d, -1)));
  const counts = meta.get().splitCounts;
  const c = split && counts?.[split.id];
  if (split && c) {
    const next = { total: Math.max(0, c.total + sign * split.total), unread: Math.max(0, c.unread + sign * split.unread) };
    meta.set({ splitCounts: { ...counts, [split.id]: next } });
  }
}

const negate = (d: SplitDelta | null): SplitDelta | null => d && { id: d.id, total: -d.total, unread: -d.unread };

/**
 * Record a change to `refs` whose rows become `after(row)`; counts move now.
 * It settles when `settle` (the backend call) does, at once ("now"), or
 * when the caller says so ("manual").
 */
function record(refs: ThreadRef[], patch: Patch | null, after: Patch, settle: Promise<unknown> | "now" | "manual"): Change {
  const before = new Map<string, ThreadSummary>();
  const afterRows = new Map<string, ThreadSummary>();
  const pairs: { before: ThreadSummary; after: ThreadSummary }[] = [];
  const splitNow = currentSplit(getUi());
  const rows = new Map(list.get().items.map((t) => [refKey(t), t]));
  let dTotal = 0;
  let dUnread = 0;
  for (const r of refs) {
    const k = refKey(r);
    if (before.has(k)) continue;
    const s = knownSummary(k, rows);
    if (!s) continue;
    const a = after(s);
    before.set(k, s);
    afterRows.set(k, a);
    pairs.push({ before: s, after: a });
    // Rows in the list are in the open split; leaving the inbox leaves it.
    if (splitNow !== null && rows.has(k)) {
      const stays = a.labelIds.includes("INBOX");
      dTotal += Number(stays) - 1;
      dUnread += Number(a.unread && stays) - Number(s.unread);
    }
  }
  const counts = countDeltas(pairs);
  const split = splitNow !== null && (dTotal || dUnread) ? { id: splitNow, total: dTotal, unread: dUnread } : null;
  const op = changes.add(
    refs.map(refKey),
    { patch, before, after: afterRows, split },
    counts,
  );
  shiftCounts(counts, split, 1);
  if (settle === "now") changes.settle(op);
  else if (settle !== "manual") void settle.then(() => changes.settle(op), () => changes.settle(op));
  return op;
}

/**
 * Patch summaries in place (optimistic read/star/label changes): the rows,
 * the cached threads, the remembered pages of other views, and the unread
 * counts, all now. With `call` (the backend request), reads that may not see
 * it yet keep the patch until it settles. Returns the change, for revertChange.
 */
export function patchThreads(refs: ThreadRef[], patch: Patch, call?: Promise<unknown>): Change {
  const op = record(refs, patch, patch, call ?? "now");
  const keys = new Set(refs.map(refKey));
  const patched = new Map<string, ThreadSummary>();
  const items = list.get().items.map((t) => {
    if (!keys.has(refKey(t))) return t;
    const p = patch(t);
    if (p !== t) patched.set(refKey(t), p);
    return p;
  });
  if (patched.size) list.set({ items });
  // The row objects on screen now, so a revert can tell they're untouched since.
  for (const [k, p] of patched) op.patch!.after.set(k, p);
  for (const [k, page] of pages) pages.set(k, page.map((t) => (keys.has(refKey(t)) ? patch(t) : t)));
  for (const r of refs) {
    const k = refKey(r);
    const cached = threadCache.get(k);
    if (cached) setCached(k, patchView(cached, patch));
  }
  return op;
}

/**
 * Undo a change whose backend call failed: rows it left untouched go back,
 * counts move back, and cached threads are read again from the backend.
 */
export function revertChange(op: Change) {
  changes.drop(op);
  const d = op.patch!;
  shiftCounts(op.counts, d.split, -1);
  if (d.after.size) {
    list.set((l) => ({ items: l.items.map((t) => (d.after.get(refKey(t)) === t ? d.before.get(refKey(t))! : t)) }));
  }
  for (const k of op.keys) {
    const cached = threadCache.get(k);
    if (cached) void fetchThread({ accountId: cached.accountId, threadId: cached.threadId }, true);
  }
}

/** A cached thread with a summary patch applied to its messages. */
function patchView(cached: ThreadView, patch: Patch): ThreadView {
  const s = patch(summaryOf(cached));
  const last = cached.messages.length - 1;
  const messages = cached.messages.map((m, i) => {
    // Marking read clears every message; marking unread flags the latest.
    const unread = s.unread ? m.unread || i === last : false;
    return unread === m.unread && s.starred === m.starred ? m : { ...m, starred: s.starred, unread };
  });
  const same = messages.every((m, i) => m === cached.messages[i]) && sameValue(s.labelIds, cached.labelIds);
  return same ? cached : { ...cached, labelIds: s.labelIds, messages };
}

function summaryOf(v: ThreadView): ThreadSummary {
  const last = v.messages[v.messages.length - 1];
  return {
    accountId: v.accountId,
    threadId: v.threadId,
    subject: v.subject,
    snippet: last?.snippet ?? "",
    participants: [],
    messageCount: v.messages.length,
    unread: v.messages.some((m) => m.unread),
    starred: v.messages.some((m) => m.starred),
    hasAttachments: v.messages.some((m) => m.attachments.length > 0),
    labelIds: v.labelIds,
    lastDate: last?.date ?? 0,
  };
}

// ---------------------------------------------------------------------------
// Remembered first pages: switching back to a view shows its rows at once
// while the backend confirms them (then they're reconciled in place).
// ---------------------------------------------------------------------------
const PAGES_MAX = 8;
const pages = new Map<string, ThreadSummary[]>();

function rememberPage(key: string, items: ThreadSummary[]) {
  if (!key) return;
  pages.delete(key);
  pages.set(key, items.slice(0, PAGE));
  while (pages.size > PAGES_MAX) pages.delete(pages.keys().next().value!);
}

/** The mailbox the list shows: view, split (with its queries) and account scope. */
function viewKey(ui: UiState): string {
  return JSON.stringify([ui.view, currentSplit(ui), splitFilter(ui), ui.accountFilter, listScope(ui)]);
}

/** The whole list query: the mailbox plus the Unread filter. */
function queryKey(ui: UiState): string {
  return viewKey(ui) + (ui.unreadOnly ? "|unread" : "");
}

function baseQuery(ui: UiState): Omit<ListQuery, "limit" | "before"> {
  return {
    view: ui.view,
    tab: null,
    split: splitFilter(ui),
    accountId: ui.accountFilter,
    accountIds: listScope(ui),
    unreadOnly: ui.unreadOnly,
  };
}

/**
 * The inbox's list query in the current account scope, without paging: the
 * open split's (when the Split Inbox shows one) or the whole inbox. For
 * actions over more than the loaded rows (Get to zero).
 */
export function inboxQuery(wholeInbox: boolean, ui: UiState = getUi()): Omit<ListQuery, "limit" | "before"> {
  return {
    view: { kind: "inbox" },
    tab: null,
    split: wholeInbox ? null : splitFilter(ui),
    accountId: ui.accountFilter,
    accountIds: listScope(ui),
    unreadOnly: false,
  };
}

/** The Unread filter (the list header's toggle, G then U): only unread threads in the current view. */
export function toggleUnreadOnly() {
  setUi((s) => ({ unreadOnly: !s.unreadOnly }));
}

const visible = (items: ThreadSummary[]) =>
  suppressed.size === 0 ? items : items.filter((t) => !suppressed.has(refKey(t)));

export async function reloadList() {
  const ui = getUi();
  const key = queryKey(ui);
  const seq = ++listSeq;
  const cur = list.get();
  const keyChanged = cur.key !== key;
  // A view seen before shows its remembered rows now; the answer reconciles them.
  const remembered = keyChanged ? pages.get(key) : undefined;
  if (keyChanged) {
    if (cur.loaded) rememberPage(cur.key, cur.items);
    list.set(
      remembered
        ? { key, items: visible(remembered), loading: true, loaded: true, hasMore: remembered.length >= PAGE, error: null }
        : { key, items: [], loading: true, loaded: false, hasMore: false, error: null },
    );
    if (remembered) ensureSelection();
  } else list.set({ loading: true });
  const shownFirst = remembered ? getUi().selected : null;
  const read = changes.readBegin();
  try {
    const fresh = await api.listThreads({ ...baseQuery(ui), limit: PAGE, before: null });
    if (seq !== listSeq) return;
    const items = shareRows(list.get().items, visible(overlay(read, fresh)));
    list.set({ items, loading: false, loaded: true, hasMore: fresh.length >= PAGE, error: null });
    rememberPage(key, items);
    // The app picked the remembered first row; if the real first row differs, move to it.
    const now = getUi();
    if (shownFirst && now.selectedByApp && !now.threadOpen && sameThread(now.selected, shownFirst) && items[0] && !sameThread(items[0], shownFirst))
      setUi({ selected: { accountId: items[0].accountId, threadId: items[0].threadId }, selectedByApp: true });
    ensureSelection();
  } catch (e) {
    if (seq !== listSeq) return;
    list.set({ loading: false, loaded: true, error: String(e) });
  } finally {
    changes.readEnd(read);
  }
}

/**
 * Every thread in the current view, paged through the backend ("select all N
 * matching" in the thread list). Threads being removed optimistically are left out.
 */
export async function listAllInView(): Promise<ThreadRef[]> {
  const q = baseQuery(getUi());
  const ALL_PAGE = 1000;
  const out: ThreadRef[] = [];
  const seen = new Set<string>();
  let before: number | null = null;
  for (;;) {
    const page = await api.listThreads({ ...q, limit: ALL_PAGE, before });
    for (const t of page) {
      const k = refKey(t);
      if (seen.has(k) || suppressed.has(k)) continue;
      seen.add(k);
      out.push({ accountId: t.accountId, threadId: t.threadId });
    }
    if (page.length < ALL_PAGE) break;
    const next = page[page.length - 1].lastDate;
    if (next === before) break; // a page of identical timestamps: no progress possible
    before = next;
  }
  return out;
}

export async function loadMore() {
  const s = list.get();
  if (s.loading || !s.hasMore || s.items.length === 0) return;
  const ui = getUi();
  const seq = listSeq; // only a full reload invalidates a page fetch
  list.set({ loading: true });
  const read = changes.readBegin();
  try {
    const before = s.items[s.items.length - 1].lastDate;
    const page = await api.listThreads({ ...baseQuery(ui), limit: PAGE, before });
    if (seq !== listSeq || list.get().key !== s.key) return;
    const have = new Set(list.get().items.map(refKey));
    const add = visible(overlay(read, page)).filter((t) => !have.has(refKey(t)));
    list.set((l) => ({ items: add.length ? [...l.items, ...add] : l.items, loading: false, hasMore: page.length >= PAGE }));
  } catch (e) {
    if (seq !== listSeq) return;
    list.set({ loading: false, error: String(e) });
  } finally {
    changes.readEnd(read);
  }
}

/**
 * Re-read the loaded window after a mail-changed event, keeping scroll depth.
 * Rows that didn't change keep their objects (memoized rows don't re-render),
 * and an unchanged answer leaves the list untouched.
 */
export async function refreshList() {
  const s = list.get();
  if (!s.loaded) return reloadList();
  const ui = getUi();
  const seq = listSeq;
  const rseq = ++refreshSeq;
  const limit = Math.min(Math.max(s.items.length, PAGE), REFRESH_CAP);
  const read = changes.readBegin();
  try {
    const fresh = await api.listThreads({ ...baseQuery(ui), limit, before: null });
    if (seq !== listSeq || rseq !== refreshSeq || list.get().key !== s.key) return;
    const cur = list.get().items;
    let items = visible(overlay(read, fresh));
    const floor = fresh.length >= limit ? fresh[fresh.length - 1].lastDate : -Infinity;
    if (ui.unreadOnly) items = keepReadHere(items, cur, floor, (k) => suppressed.has(k));
    if (fresh.length >= limit && cur.length > limit) {
      // Keep the already-paged tail below the refreshed window.
      items = items.concat(cur.filter((t) => t.lastDate < floor));
    }
    items = shareRows(cur, items);
    const hasMore = fresh.length >= limit ? s.hasMore || fresh.length >= PAGE : false;
    const prevIndex = indexOfSelected();
    if (items !== cur || hasMore !== list.get().hasMore) list.set({ items, hasMore });
    rememberPage(s.key, items);
    ensureSelection(prevIndex);
  } catch {
    // A failed background refresh keeps the current list; the next event retries.
  } finally {
    changes.readEnd(read);
  }
}

export function indexOfSelected(): number {
  const sel = getUi().selected;
  if (!sel) return -1;
  return list.get().items.findIndex((t) => t.threadId === sel.threadId && t.accountId === sel.accountId);
}

/**
 * Toggling the Unread filter starts from a clean list: nothing selected, and
 * no row is picked for you (ensureSelection) until you pick one or change
 * mailbox.
 */
let skipAutoSelect = false;

/** Keep a row selected: first row on load, the same index if it vanished. */
function ensureSelection(prevIndex = -1) {
  const ui = getUi();
  const items = list.get().items;
  if (skipAutoSelect && !ui.selected) return;
  if (ui.threadOpen && ui.selected) return;
  if (ui.selected && indexOfSelected() >= 0) return;
  if (items.length === 0) {
    if (ui.selected) setUi({ selected: null });
    return;
  }
  if (prevIndex >= 0) {
    const t = items[Math.min(prevIndex, items.length - 1)];
    setUi({ selected: { accountId: t.accountId, threadId: t.threadId }, selectedByApp: true });
    return;
  }
  const t = items[0];
  setUi({ selected: { accountId: t.accountId, threadId: t.threadId }, focusMessageId: null, selectedByApp: true });
}

/** Direction the cursor last moved (1 down, -1 up): prefetch leans that way. */
let lastMove = 1;

export function selectIndex(i: number) {
  skipAutoSelect = false;
  const items = list.get().items;
  if (items.length === 0) return;
  const t = items[Math.max(0, Math.min(i, items.length - 1))];
  if (!sameThread(getUi().selected, t) || getUi().selectedByApp) {
    // Ask for the thread before React renders the selection, not after.
    void fetchThread(t);
    setUi({ selected: { accountId: t.accountId, threadId: t.threadId }, focusMessageId: null, selectedByApp: false });
  }
  if (i >= items.length - 20) void loadMore();
}

export function selectThread(ref: ThreadRef) {
  skipAutoSelect = false;
  void fetchThread(ref);
  setUi({ selected: ref, focusMessageId: null, selectedByApp: false });
}

export function moveSelection(delta: number) {
  const i = indexOfSelected();
  if (delta) lastMove = delta > 0 ? 1 : -1;
  selectIndex(i < 0 ? 0 : i + delta);
}

export interface Removal {
  items: { t: ThreadSummary; index: number }[];
  prevSelected: ThreadRef | null;
  /** The count change this removal made (settleRemoval / restoreLocal finish it). */
  change: Change;
}

/**
 * Remove threads from the list now (archive/trash), moving the cursor to the
 * row that took the removed row's place — like Superhuman. `after` is what
 * the action does to a row (archive drops INBOX…), so unread counts move at
 * once too. Returns what's needed to put them back for undo.
 */
export function removeLocal(refs: ThreadRef[], after: Patch = (t) => t): Removal {
  const keys = new Set(refs.map(refKey));
  const items = list.get().items;
  const removed: Removal["items"] = [];
  const kept: ThreadSummary[] = [];
  items.forEach((t, index) => (keys.has(refKey(t)) ? removed.push({ t, index }) : kept.push(t)));
  const prevSelected = getUi().selected;
  const selIndex = indexOfSelected();
  const change = record(refs, null, after, "manual");
  refs.forEach((r) => suppressed.set(refKey(r), (suppressed.get(refKey(r)) ?? 0) + 1));
  list.set({ items: kept });
  if (prevSelected && keys.has(refKey(prevSelected))) {
    // First surviving row at or after the old index, else the one before.
    const removedBefore = removed.filter((r) => r.index < selIndex).length;
    const nextIndex = selIndex - removedBefore;
    const next = kept[nextIndex] ?? kept[kept.length - 1];
    setUi(
      next
        ? { selected: { accountId: next.accountId, threadId: next.threadId }, focusMessageId: null, selectedByApp: false }
        : { selected: null, threadOpen: false },
    );
    if (nextIndex >= kept.length - 20) void loadMore();
  }
  return { items: removed, prevSelected, change };
}

/** Backend call for an optimistic removal settled (either way). */
export function settleRemoval(refs: ThreadRef[], removal?: Removal | null) {
  for (const r of refs) {
    const k = refKey(r);
    const n = (suppressed.get(k) ?? 1) - 1;
    if (n <= 0) suppressed.delete(k);
    else suppressed.set(k, n);
  }
  if (removal) changes.settle(removal.change);
}

/**
 * Put removed rows back. With `call` (undo: the inverse request) the counts
 * move back as a change of their own; without it (the removal failed) the
 * removal is forgotten and its count change undone.
 */
export function restoreLocal(r: Removal, call?: Promise<unknown>) {
  const items = [...list.get().items];
  for (const { t, index } of [...r.items].sort((a, b) => a.index - b.index)) {
    suppressed.delete(refKey(t));
    if (!items.some((x) => refKey(x) === refKey(t))) items.splice(Math.min(index, items.length), 0, t);
  }
  list.set({ items });
  if (call) {
    const back = changes.add(r.change.keys, { patch: null, before: new Map(), after: new Map(), split: negate(r.change.patch!.split) }, addDeltas(new Map(), r.change.counts, -1));
    shiftCounts(back.counts, back.patch!.split, 1);
    void call.then(() => changes.settle(back), () => changes.settle(back));
  } else {
    changes.drop(r.change);
    shiftCounts(r.change.counts, r.change.patch!.split, -1);
  }
  // The cursor goes back where it was; that isn't opening the thread (it isn't marked read).
  if (r.prevSelected) setUi({ selected: r.prevSelected, selectedByApp: true });
}

// Reload whenever the query changes: view, tab, account, the active
// profile's accounts (switching profiles, or editing the active one), the
// accounts hidden from All Inboxes, or the Unread filter. A new mailbox resets
// the cursor and closes the thread; toggling the Unread filter clears the
// selection too and doesn't auto-select a row.
let lastKey = "";
let lastViewKey = "";
let lastUnreadOnly = false;
function reloadIfQueryChanged() {
  const ui = getUi();
  const k = queryKey(ui);
  if (k === lastKey) return;
  lastKey = k;
  const v = viewKey(ui);
  const newView = v !== lastViewKey;
  lastViewKey = v;
  const unreadToggled = !newView && ui.unreadOnly !== lastUnreadOnly;
  lastUnreadOnly = ui.unreadOnly;
  if (meta.get().accounts.length > 0) {
    if (newView) {
      skipAutoSelect = false;
      setUi({ selected: null, threadOpen: false });
    } else if (unreadToggled) {
      skipAutoSelect = true;
      setUi({ selected: null, threadOpen: false });
    }
    void reloadList();
    if (newView && getUi().view.kind === "inbox") void loadSplitCounts();
  }
}
subscribeUi(reloadIfQueryChanged);
// Settings are subscribed in startMail, not here: lib/settings imports this
// module, so it may not be initialized yet while this one evaluates.
let settingsSub: (() => void) | null = null;

/**
 * A row the pointer rests on is likely clicked next: read its thread ahead
 * (delegated on the document, so the rows themselves stay plain).
 */
let hoverPrefetch = false;
function installHoverPrefetch() {
  if (hoverPrefetch || typeof document === "undefined") return;
  hoverPrefetch = true;
  let timer: ReturnType<typeof setTimeout> | null = null;
  document.addEventListener(
    "pointerover",
    (e) => {
      const row = (e.target as Element | null)?.closest?.<HTMLElement>('[role="option"][data-thread][data-account]');
      if (timer) clearTimeout(timer);
      timer = null;
      if (!row) return;
      const ref = { accountId: row.dataset.account!, threadId: row.dataset.thread! };
      // Passing over rows on the way somewhere else doesn't count.
      timer = setTimeout(() => prefetchThread(ref), 50);
    },
    { passive: true },
  );
}

/** Called once accounts are known. */
export function startMail() {
  installHoverPrefetch();
  settingsSub ??= subscribeSettings(reloadIfQueryChanged);
  lastKey = queryKey(getUi());
  lastViewKey = viewKey(getUi());
  void reloadList();
  void loadLabels();
  void loadSyncStatus();
  void loadSplitCounts();
}

// ---------------------------------------------------------------------------
// Thread cache (recent getThread results + prefetch)
// ---------------------------------------------------------------------------
const CACHE_MAX = 60;
const threadCache = new Map<string, ThreadView>();
const inflight = new Map<string, Promise<ThreadView | null>>();
const missing = new Set<string>();
const again = new Set<string>();
const cacheSubs = new Set<() => void>();
let cacheVersion = 0;

/**
 * Store a thread (most recent last: the least recently used go first). A
 * refetch that changed nothing keeps the cached object, and unchanged
 * messages keep theirs, so the open thread doesn't re-render for it.
 */
function setCached(k: string, v: ThreadView) {
  const prev = threadCache.get(k);
  let next = v;
  if (prev && prev !== v) {
    if (sameValue(prev, v)) next = prev;
    else {
      const old = new Map(prev.messages.map((m) => [m.id, m]));
      const messages = v.messages.map((m) => {
        const o = old.get(m.id);
        return o && sameValue(o, m) ? o : m;
      });
      next = { ...v, messages };
    }
  }
  threadCache.delete(k);
  threadCache.set(k, next);
  while (threadCache.size > CACHE_MAX) threadCache.delete(threadCache.keys().next().value!);
  if (next === prev) return;
  cacheVersion++;
  cacheSubs.forEach((f) => f());
}

export function fetchThread(ref: ThreadRef, force = false): Promise<ThreadView | null> {
  const k = refKey(ref);
  const hit = threadCache.get(k);
  if (!force && hit) {
    // Recently used: last to be evicted.
    threadCache.delete(k);
    threadCache.set(k, hit);
    return Promise.resolve(hit);
  }
  const running = inflight.get(k);
  if (running) {
    // Asked again while a read runs (mail-changed): it may predate the change, so read once more after it.
    if (force && !again.has(k)) {
      again.add(k);
      const retry = () => {
        again.delete(k);
        void fetchThread(ref, true);
      };
      void running.then(retry, retry);
    }
    return running;
  }
  const read = changes.readBegin();
  const p = api
    .getThread(ref.accountId, ref.threadId)
    .then((fresh) => {
      if (fresh) {
        // Changes the backend may not have had yet stay applied.
        let v = fresh;
        for (const op of changes.pending(read)) if (op.keys.has(k) && op.patch?.patch) v = patchView(v, op.patch.patch);
        missing.delete(k);
        setCached(k, v);
      } else {
        missing.add(k);
        cacheVersion++;
        cacheSubs.forEach((f) => f());
      }
      return fresh && threadCache.get(k)!;
    })
    .finally(() => {
      inflight.delete(k);
      changes.readEnd(read);
    });
  inflight.set(k, p);
  return p;
}

/**
 * Fetch the selected thread's neighbours so j/k shows them in the same frame:
 * the next three in the direction the cursor is moving and one behind.
 */
export function prefetchAround() {
  const i = indexOfSelected();
  if (i < 0) return;
  const items = list.get().items;
  const d = lastMove;
  for (const j of [i + d, i + 2 * d, i - d, i + 3 * d]) {
    const t = items[j];
    if (t && !threadCache.has(refKey(t))) void fetchThread(t);
  }
}

/** Warm the cache for a thread the pointer rests on (a click is likely next). */
export function prefetchThread(ref: ThreadRef) {
  if (!threadCache.has(refKey(ref))) void fetchThread(ref);
}

export function cachedThread(ref: ThreadRef): ThreadView | undefined {
  return threadCache.get(refKey(ref));
}

/** The list row for a thread, if it's in the list. */
export function listSummary(ref: ThreadRef): ThreadSummary | undefined {
  const k = refKey(ref);
  return list.get().items.find((t) => refKey(t) === k);
}

/** Replace one message in a cached thread (after "Load images"). */
export function replaceCachedMessage(ref: ThreadRef, message: ThreadView["messages"][number]) {
  const k = refKey(ref);
  const v = threadCache.get(k);
  if (!v) return;
  setCached(k, { ...v, messages: v.messages.map((m) => (m.id === message.id ? message : m)) });
}

export function useThread(ref: ThreadRef | null): { thread: ThreadView | null; loading: boolean; missing: boolean } {
  useSyncExternalStore(
    (cb) => {
      cacheSubs.add(cb);
      return () => cacheSubs.delete(cb);
    },
    () => cacheVersion,
  );
  if (!ref) return { thread: null, loading: false, missing: false };
  const k = refKey(ref);
  const v = threadCache.get(k);
  if (!v && !inflight.has(k) && !missing.has(k)) queueMicrotask(() => void fetchThread(ref));
  return { thread: v ?? null, loading: !v && !missing.has(k), missing: missing.has(k) };
}

/** mail-changed: refetch any cached copies in the background (no flicker). */
export function invalidateThreads(accountId: string, threadIds: string[]) {
  for (const id of threadIds) {
    const k = refKey({ accountId, threadId: id });
    missing.delete(k);
    if (threadCache.has(k)) void fetchThread({ accountId, threadId: id }, true);
  }
}

/**
 * Re-render every cached thread in the background (e.g. the remote-images
 * setting changed, so the sanitized HTML differs). Added by the settings agent.
 */
export function refetchCachedThreads() {
  for (const v of [...threadCache.values()]) void fetchThread({ accountId: v.accountId, threadId: v.threadId }, true);
}
