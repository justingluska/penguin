// Triage actions. Each one updates the UI immediately (optimistic), then calls
// the backend; failures toast and re-sync the list. Removals (archive/trash)
// get an Undo toast wired to Z.
import { api, asCommandError } from "../lib/api";
import type { Account, ThreadAction, ThreadRef, ThreadSummary } from "../lib/types";
import {
  INBOX,
  applyDelta,
  deltaCalls,
  moveDelta,
  shownIn,
  threadAbilities,
  undoDelta,
  type LabelDelta,
  type ThreadAbilities,
} from "../lib/capabilities";
import { getUi, leaveMarkedUnread, setUi } from "../lib/ui";
import { dismissToast, toast } from "../components/Toast";
import { openReconnect } from "../features/settings/ReconnectModal";
import { num } from "../lib/format";
import { fmtUntil } from "../features/snooze/presets";
import { pickMessage } from "../features/compose/quote";
import { getSelection, hasSelection, selectionRefs, selectionTargets } from "./selection";
import { dismissFollowUps, leaveReplyLater } from "../features/triage/actions";
import { snoozeLocal, snoozesChanged, unsnoozeLocal } from "../features/snooze/state";
import { hintActionExit } from "../features/inbox/rowExit";
import { inboxLikeView } from "../features/smart/catalog";
import { noteCleared } from "../features/zero/zero";
import { single, type RemoteUndo, type UndoOp } from "./remoteUndo";
import { reportSpamUndo, targetsAreSpam } from "./spam";
import {
  accountById,
  accountInScope,
  cachedThread,
  list,
  meta,
  patchThreads,
  refreshList,
  removeLocal,
  restoreLocal,
  revertChange,
  settleRemoval,
  type Removal,
} from "./store";

/**
 * What Z undoes. `remote` is the same undo as data, for an action that
 * closes a conversation window (app/remoteUndo.ts): the main window offers it
 * once that window is gone.
 */
let lastUndo: { label: string; run: () => void; remote?: RemoteUndo } | null = null;
/** Counts undos as they're set (a closing window checks whether its last action left one). */
let undoCount = 0;

export function canUndo(): boolean {
  return lastUndo !== null;
}

/** Make `run` what Z undoes next (e.g. compose's pending send). Every undo is set here. */
export function setUndo(label: string, run: () => void, remote?: RemoteUndo) {
  lastUndo = { label, run, remote };
  undoCount++;
}

/** How many undos have been set so far (compare before and after an action). */
export function undoSerial(): number {
  return undoCount;
}

/** The current undo as data, when it has one (see RemoteUndo). */
export function currentRemoteUndo(): RemoteUndo | null {
  return lastUndo?.remote ?? null;
}

/** Forget `run` if it's still the pending undo (e.g. the send went out). */
export function clearUndo(run: () => void) {
  if (lastUndo?.run === run) lastUndo = null;
}

/** Whether `run` is still what Z undoes (a toast's Undo only undoes its own action). */
export function isPendingUndo(run: () => void): boolean {
  return lastUndo?.run === run;
}

export function undo() {
  const u = lastUndo;
  lastUndo = null;
  if (!u) {
    toast({ message: "Nothing to undo" });
    return;
  }
  u.run();
}

export function fail(what: string, err: unknown, refs: ThreadRef[] = []) {
  const e = asCommandError(err);
  const reauth = e.code === "needsReauth";
  const why = reauth ? "sign in to this account again" : e.message;
  const accountId = refs[0]?.accountId;
  toast({
    kind: "error",
    message: `Couldn't ${what}`,
    detail: why,
    title: e.message,
    action: reauth && accountId ? { label: "Reconnect", run: () => openReconnect(accountId) } : undefined,
  });
  void refreshList();
}

/**
 * The thread(s) an action applies to: the multi-selection when there is one
 * (its loaded rows), else the cursor row.
 */
export function targets(): ThreadRef[] {
  if (hasSelection()) return selectionRefs();
  const s = getUi().selected;
  return s ? [s] : [];
}

/** Like targets(), but a "select all N matching" selection expands to the whole view. */
export function resolveTargets(): Promise<ThreadRef[]> {
  return hasSelection() ? selectionTargets() : Promise.resolve(targets());
}

/** Run an action on resolveTargets() (keys and the selection bar). */
export function onTargets(action: (refs: ThreadRef[]) => unknown) {
  void resolveTargets().then((refs) => refs.length && action(refs));
}

function summary(ref: ThreadRef): ThreadSummary | undefined {
  return list.get().items.find((t) => t.threadId === ref.threadId && t.accountId === ref.accountId);
}

/**
 * What an action does to a row's labels, as the backend applies it locally
 * (views.rs ThreadAction::local_delta), so rows and unread counts can change
 * before it answers.
 */
export function actionPatch(action: ThreadAction): (t: ThreadSummary) => ThreadSummary {
  const [add, remove] = labelDelta(action);
  return (t) => {
    let labelIds = t.labelIds.filter((l) => !remove.includes(l));
    for (const l of add) if (!labelIds.includes(l)) labelIds = [...labelIds, l];
    const unread = add.includes("UNREAD") ? true : remove.includes("UNREAD") ? false : t.unread;
    const starred = add.includes("STARRED") ? true : remove.includes("STARRED") ? false : t.starred;
    const sameLabels = labelIds.length === t.labelIds.length && labelIds.every((l, i) => l === t.labelIds[i]);
    return sameLabels && unread === t.unread && starred === t.starred ? t : { ...t, labelIds, unread, starred };
  };
}

function labelDelta(action: ThreadAction): [string[], string[]] {
  switch (action.kind) {
    case "archive": return [[], ["INBOX"]];
    case "moveToInbox": return [["INBOX"], []];
    case "trash": return [["TRASH"], ["INBOX"]];
    case "untrash": return [["INBOX"], ["TRASH"]];
    case "reportSpam": return [["SPAM"], ["INBOX"]];
    case "notSpam": return [["INBOX"], ["SPAM"]];
    case "markRead": return [[], ["UNREAD"]];
    case "markUnread": return [["UNREAD"], []];
    case "star": return [["STARRED"], []];
    case "unstar": return [[], ["STARRED"]];
    case "addLabel": return [[action.labelId], []];
    case "removeLabel": return [[], [action.labelId]];
    case "replyLater": return [[action.labelId], ["INBOX", "UNREAD"]];
  }
}

async function removeWith(
  refs: ThreadRef[],
  action: ThreadAction,
  /** What Undo sends: one action for every ref, or steps of calls (Report spam's per-conversation undo). */
  inverse: ThreadAction | UndoOp[][],
  doneLabel: string,
  verb: string,
  /** The toast for a batch: "Archived 12". */
  many: (n: string) => string,
) {
  if (refs.length === 0) return;
  const steps = Array.isArray(inverse) ? inverse : single({ cmd: "modify", refs, action: inverse });
  hintActionExit(refs, action.kind); // how the rows animate out (list motion only)
  const removal: Removal = removeLocal(refs, actionPatch(action));
  // The zero screen's "cleared today": conversations done or trashed from the inbox.
  const cleared = getUi().view.kind === "inbox" && (action.kind === "archive" || action.kind === "trash") ? refs.length : 0;
  noteCleared(cleared);
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    noteCleared(-cleared);
    const back = runUndoSteps(steps);
    restoreLocal(removal, back);
    back.catch((e) => fail("undo", e, refs));
  };
  const message = refs.length > 1 ? many(num(refs.length)) : doneLabel;
  setUndo(doneLabel, undoRun, { message, steps });
  toastId = toast({
    kind: "action",
    key: `undo:${doneLabel}`,
    message,
    action: { label: "Undo", keys: "z", run: () => lastUndo?.run === undoRun && undo() },
  });
  try {
    await api.modifyThreads(refs, action);
  } catch (e) {
    restoreLocal(removal);
    noteCleared(-cleared);
    fail(verb, e, refs);
  } finally {
    settleRemoval(refs, removal);
  }
}

/**
 * E — archive ("Done"). Only removes from the list where the list is the
 * inbox. In Reply Later it's done there (the label comes off); in Follow up
 * it dismisses.
 */
export function archive(refs = targets()) {
  if (refs.length === 0) return;
  const view = getUi().view.kind;
  if (view === "replyLater") return leaveReplyLater(refs);
  if (view === "followUp") return dismissFollowUps(refs);
  // Newsletters and People you know list inbox mail: done takes a row out, as in the inbox.
  if (view === "inbox" || inboxLikeView(getUi().view))
    return removeWith(refs, { kind: "archive" }, { kind: "moveToInbox" }, "Archived", "archive", (n) => `Archived ${n}`);
  if (view === "done" || view === "trash" || view === "snoozed" || view === "spam") {
    // Already out of the inbox: E moves it back, like Superhuman's toggle
    // (out of Spam, that's Not spam).
    return moveToInbox(refs);
  }
  // Elsewhere the rows stay; they just lose the inbox label.
  setUndo("Archived", () => modify(refs, { kind: "moveToInbox" }, "undo"));
  modify(refs, { kind: "archive" }, "archive");
  toast({ message: refs.length > 1 ? `Archived ${num(refs.length)}` : "Archived", action: { label: "Undo", keys: "z", run: undo } });
}

/** Back to the inbox: leaves views that don't show inbox mail (Done, Trash, Spam). */
export function moveToInbox(refs = targets()) {
  if (refs.length === 0) return;
  const view = getUi().view.kind;
  if (view === "trash") {
    return removeWith(refs, { kind: "untrash" }, { kind: "trash" }, "Restored from Trash", "restore", (n) => `Restored ${n} from Trash`);
  }
  // Snoozed (or a snoozed row anywhere): moving it to the inbox ends the snooze.
  if (view === "snoozed" || refs.every((r) => snoozedUntil(r) !== null)) return unsnooze(refs);
  // Out of Spam means Not spam: the SPAM label comes off too.
  if (view === "spam") return notSpam(refs);
  if (view === "done") {
    return removeWith(refs, { kind: "moveToInbox" }, { kind: "archive" }, "Moved to inbox", "move to inbox", (n) => `Moved ${n} to inbox`);
  }
  // Everywhere else the rows stay; they just gain the inbox label.
  setUndo("Moved to inbox", () => modify(refs, { kind: "archive" }, "undo"));
  modify(refs, { kind: "moveToInbox" }, "move to inbox");
  toast({ message: refs.length > 1 ? `Moved ${num(refs.length)} to inbox` : "Moved to inbox", action: { label: "Undo", keys: "z", run: undo } });
}

/** # — trash. */
export function trash(refs = targets()) {
  if (refs.length === 0) return;
  if (getUi().view.kind === "trash") {
    return removeWith(refs, { kind: "untrash" }, { kind: "trash" }, "Restored from Trash", "restore", (n) => `Restored ${n} from Trash`);
  }
  return removeWith(refs, { kind: "trash" }, { kind: "untrash" }, "Moved to Trash", "trash", (n) => `Moved ${n} to Trash`);
}

/**
 * ! — Report spam: into Spam and out of the inbox (Gmail +SPAM −INBOX; IMAP
 * and Outlook move it to Junk). The rows leave every view but Spam. Undo puts
 * each conversation back where it was: the inbox, or Done.
 */
export function reportSpam(refs = targets()) {
  if (refs.length === 0) return;
  const undoSteps = reportSpamUndo(refs, (r) => labelsOf(r).includes(INBOX));
  return removeWith(refs, { kind: "reportSpam" }, undoSteps, "Reported as spam", "report spam", (n) => `Reported ${n} as spam`);
}

/** Not spam (! or E in Spam): out of Spam and into the inbox. */
export function notSpam(refs = targets()) {
  if (refs.length === 0) return;
  return removeWith(refs, { kind: "notSpam" }, { kind: "reportSpam" }, "Not spam: moved to inbox", "mark as not spam", (n) => `Not spam: moved ${n} to inbox`);
}

/** Whether ! means Not spam for these conversations: the Spam view, or every one of them is in Spam. */
export function targetsInSpam(refs: ThreadRef[] = targets()): boolean {
  return targetsAreSpam(getUi().view.kind, refs.map((r) => summary(r)?.labelIds ?? cachedThread(r)?.labelIds ?? null));
}

/**
 * A label change on rows that stay in the list: rows, cached threads and
 * unread counts change now; if the backend refuses, all of it goes back.
 */
function modify(refs: ThreadRef[], action: ThreadAction, verb: string) {
  if (refs.length === 0) return;
  const call = api.modifyThreads(refs, action);
  const change = patchThreads(refs, actionPatch(action), call);
  call.catch((e) => {
    revertChange(change);
    fail(verb, e, refs);
  });
}

const withLabel = (t: ThreadSummary, id: string, on: boolean): ThreadSummary => {
  const has = t.labelIds.includes(id);
  if (has === on) return t;
  return { ...t, labelIds: on ? [...t.labelIds, id] : t.labelIds.filter((l) => l !== id) };
};

export function markRead(refs = targets()) {
  modify(refs, { kind: "markRead" }, "mark read");
}

export function markUnread(refs = targets()) {
  modify(refs, { kind: "markUnread" }, "mark unread");
}

export function isUnread(ref: ThreadRef): boolean {
  const s = summary(ref);
  if (s) return s.unread;
  return cachedThread(ref)?.messages.some((m) => m.unread) ?? false;
}

export function isStarred(ref: ThreadRef): boolean {
  const s = summary(ref);
  if (s) return s.starred;
  return cachedThread(ref)?.messages.some((m) => m.starred) ?? false;
}

const countWord = (refs: ThreadRef[]) => (refs.length > 1 ? ` ${num(refs.length)}` : "");

/**
 * ⇧U, Mark unread in the thread toolbar and menus: mark unread and say so.
 * From inside the open thread it stays unread (lib/ui leaveMarkedUnread).
 */
export function markUnreadAndSay(refs = targets()) {
  if (refs.length === 0) return;
  markUnread(refs);
  toast({ message: `Marked${countWord(refs)} as unread` });
  leaveMarkedUnread(refs);
}

/** U — toggle read/unread. A batch with anything unread is marked read. */
export function toggleRead(refs = targets()) {
  if (refs.length === 0) return;
  if (refs.some(isUnread)) {
    markRead(refs);
    toast({ message: `Marked${countWord(refs)} as read` });
  } else {
    markUnreadAndSay(refs);
  }
}

/** S — toggle star. A batch with anything unstarred gets starred. */
export function toggleStar(refs = targets()) {
  if (refs.length === 0) return;
  const on = !refs.every(isStarred);
  modify(refs, { kind: on ? "star" : "unstar" }, on ? "star" : "unstar");
}

/** The thread's current snooze time, if it's snoozed (per its list row). */
export function snoozedUntil(ref: ThreadRef): number | null {
  return summary(ref)?.snoozedUntil ?? null;
}

function inInbox(ref: ThreadRef): boolean {
  if (getUi().view.kind === "inbox") return true;
  const labels = summary(ref)?.labelIds ?? cachedThread(ref)?.labelIds ?? [];
  return labels.includes("INBOX");
}

type SnoozeState = { ref: ThreadRef; until: number | null; inbox: boolean };

/**
 * Put threads back the way they were before a snooze or unsnooze: snoozed
 * again at the old time, else unsnoozed (back to the inbox only if it was there).
 */
function putBack(prev: SnoozeState[]): Promise<unknown> {
  return runUndoSteps(putBackSteps(prev));
}

/** putBack as data (one step: every call in parallel). */
function putBackSteps(prev: SnoozeState[]): UndoOp[][] {
  const ops: UndoOp[] = [];
  const byUntil = new Map<number, ThreadRef[]>();
  for (const p of prev) if (p.until !== null) byUntil.set(p.until, [...(byUntil.get(p.until) ?? []), p.ref]);
  for (const [until, refs] of byUntil) ops.push({ cmd: "snooze", refs, until });
  const toInbox = prev.filter((p) => p.until === null && p.inbox).map((p) => p.ref);
  const elsewhere = prev.filter((p) => p.until === null && !p.inbox).map((p) => p.ref);
  if (toInbox.length) ops.push({ cmd: "unsnooze", refs: toInbox, toInbox: true });
  if (elsewhere.length) ops.push({ cmd: "unsnooze", refs: elsewhere, toInbox: false });
  return [ops];
}

/** Run an undo described as data (RemoteUndo): each step's calls together, the steps in order. */
export async function runUndoSteps(steps: UndoOp[][]): Promise<void> {
  for (const step of steps) {
    await Promise.all(
      step.map((op) => {
        switch (op.cmd) {
          case "modify":
            return api.modifyThreads(op.refs, op.action);
          case "snooze":
            return api.snoozeThreads(op.refs, op.until);
          case "unsnooze":
            return api.unsnoozeThreads(op.refs, op.toInbox);
          case "replyLater":
            return api.replyLater(op.refs, op.on);
          case "dismissFollowUps":
            return api.dismissFollowUps(op.refs, op.dismissed);
        }
      }),
    );
  }
}

/**
 * H — snooze until `until`: out of the inbox now (archived on Gmail), back
 * on top, unread, when it wakes. Rows leave the list where the list is the
 * inbox; elsewhere they stay with their new time (the Snoozed view re-sorts).
 */
export function snooze(refs: ThreadRef[], until: number) {
  if (refs.length === 0) return;
  const prev: SnoozeState[] = refs.map((ref) => ({ ref, until: snoozedUntil(ref), inbox: inInbox(ref) }));
  const call = api.snoozeThreads(refs, until);
  const snoozed = (t: ThreadSummary): ThreadSummary => ({ ...withLabel(t, "INBOX", false), snoozedUntil: until });
  const removal: Removal | null = getUi().view.kind === "inbox" || inboxLikeView(getUi().view) ? removeLocal(refs, snoozed) : null;
  const change = removal ? null : patchThreads(refs, snoozed, call);
  snoozeLocal(refs, until);
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    const back = putBack(prev);
    if (removal) restoreLocal(removal, back);
    else
      patchThreads(
        refs,
        (t) => {
          const p = prev.find((x) => x.ref.accountId === t.accountId && x.ref.threadId === t.threadId);
          return p ? { ...withLabel(t, "INBOX", p.inbox), snoozedUntil: p.until } : t;
        },
        back,
      );
    snoozesChanged();
    back.catch((e) => fail("undo", e, refs));
  };
  const when = fmtUntil(until);
  const message = refs.length > 1 ? `Snoozed ${num(refs.length)} ${when}` : `Snoozed ${when}`;
  setUndo("Snoozed", undoRun, { message, steps: putBackSteps(prev) });
  toastId = toast({
    kind: "action",
    key: "undo:Snoozed",
    message,
    action: { label: "Undo", keys: "z", run: () => lastUndo?.run === undoRun && undo() },
  });
  call
    .catch((e) => {
      if (removal) restoreLocal(removal);
      if (change) revertChange(change);
      snoozesChanged();
      fail("snooze", e, refs);
    })
    .finally(() => removal && settleRemoval(refs, removal));
}

/** End the snooze now and move the threads to the inbox (the Snoozed view's Unsnooze). */
export function unsnooze(refs = targets()) {
  const inSnoozed = getUi().view.kind === "snoozed";
  refs = refs.filter((r) => inSnoozed || snoozedUntil(r) !== null);
  if (refs.length === 0) return;
  const prev: SnoozeState[] = refs.map((ref) => ({ ref, until: snoozedUntil(ref), inbox: false }));
  const call = api.unsnoozeThreads(refs, true);
  const woken = (t: ThreadSummary): ThreadSummary => ({ ...withLabel(t, "INBOX", true), snoozedUntil: null });
  const removal: Removal | null = inSnoozed ? removeLocal(refs, woken) : null;
  const change = removal ? null : patchThreads(refs, woken, call);
  unsnoozeLocal(refs);
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    const back = putBack(prev);
    if (removal) restoreLocal(removal, back);
    else
      patchThreads(
        refs,
        (t) => {
          const p = prev.find((x) => x.ref.accountId === t.accountId && x.ref.threadId === t.threadId);
          return p ? { ...withLabel(t, "INBOX", false), snoozedUntil: p.until } : t;
        },
        back,
      );
    snoozesChanged();
    back.catch((e) => fail("undo", e, refs));
  };
  const message = refs.length > 1 ? `Unsnoozed ${num(refs.length)}, moved to inbox` : "Unsnoozed, moved to inbox";
  setUndo("Unsnoozed", undoRun, { message, steps: putBackSteps(prev) });
  toastId = toast({
    kind: "action",
    key: "undo:Unsnoozed",
    message,
    action: { label: "Undo", keys: "z", run: () => lastUndo?.run === undoRun && undo() },
  });
  call
    .catch((e) => {
      if (removal) restoreLocal(removal);
      if (change) revertChange(change);
      snoozesChanged();
      fail("unsnooze", e, refs);
    })
    .finally(() => removal && settleRemoval(refs, removal));
}

// ---------------------------------------------------------------------------
// Label as… / Move to…, gated on what the targets' accounts can do
// ---------------------------------------------------------------------------

/** The accounts an action on `refs` touches ("select all N" reaches every account in view). */
export function targetAccounts(refs: ThreadRef[] = targets()): (Account | undefined)[] {
  const ids = new Set(refs.map((r) => r.accountId));
  if (getSelection().allMatching) for (const a of meta.get().accounts) if (accountInScope(a.id)) ids.add(a.id);
  return [...ids].map(accountById);
}

/** Whether Label as… and Move to… apply to the targets (every account must support it). */
export function targetAbilities(refs: ThreadRef[] = targets()): ThreadAbilities {
  return threadAbilities(targetAccounts(refs));
}

/** L: Label as… where every target's account has labels, else Move to… on folder accounts. */
export function openLabelOrMove() {
  const can = targetAbilities();
  if (can.label) setUi({ overlay: "label" });
  else if (can.move) setUi({ overlay: "move" });
}

function labelsOf(ref: ThreadRef): string[] {
  return summary(ref)?.labelIds ?? cachedThread(ref)?.labelIds ?? [];
}

/** Send label deltas as modify_threads calls: every add first, then the removals. */
async function sendDeltas(items: { ref: ThreadRef; delta: LabelDelta }[]) {
  await runUndoSteps(deltaSteps(items));
}

/** sendDeltas as data: the adds, then the removals. */
function deltaSteps(items: { ref: ThreadRef; delta: LabelDelta }[]): UndoOp[][] {
  const { add, remove } = deltaCalls(items);
  return [
    [...add].map(([id, refs]): UndoOp => ({ cmd: "modify", refs, action: id === INBOX ? { kind: "moveToInbox" } : { kind: "addLabel", labelId: id } })),
    [...remove].map(([id, refs]): UndoOp => ({ cmd: "modify", refs, action: id === INBOX ? { kind: "archive" } : { kind: "removeLabel", labelId: id } })),
  ];
}

/**
 * V — move conversations to one folder, or back to the inbox (folder
 * accounts: IMAP, Microsoft). `targetFor` is the folder's id in each
 * account (INBOX for the inbox); accounts without it are skipped. Rows the
 * view no longer shows leave the list; Z undoes.
 */
export function moveTo(refs: ThreadRef[], targetFor: (accountId: string) => string | null, name: string) {
  const items = refs.flatMap((ref) => {
    const target = targetFor(ref.accountId);
    if (!target) return [];
    const before = labelsOf(ref);
    return [{ ref, before, delta: moveDelta(before, target) }];
  });
  if (items.length === 0) return;
  const view = getUi().view;
  const byKey = new Map(items.map((i) => [i.ref.accountId + "\u0000" + i.ref.threadId, i]));
  const patch = (fwd: boolean) => (t: ThreadSummary) => {
    const i = byKey.get(t.accountId + "\u0000" + t.threadId);
    return i ? { ...t, labelIds: fwd ? applyDelta(t.labelIds, i.delta) : applyDelta(t.labelIds, undoDelta(i.before, i.delta)) } : t;
  };
  const all = items.map((i) => i.ref);
  const call = sendDeltas(items.map((i) => ({ ref: i.ref, delta: i.delta })));
  const leaving = items.filter((i) => !shownIn(view, applyDelta(i.before, i.delta)));
  const removal: Removal | null = leaving.length ? removeLocal(leaving.map((i) => i.ref), patch(true)) : null;
  // Rows that stay change in place; rows that leave took their count change with them.
  const staying = items.filter((i) => !leaving.includes(i)).map((i) => i.ref);
  const change = staying.length ? patchThreads(staying, patch(true), call) : null;
  const done = `Moved to ${name}`;
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    const back = sendDeltas(items.map((i) => ({ ref: i.ref, delta: undoDelta(i.before, i.delta) })));
    if (removal) restoreLocal(removal, back);
    if (staying.length) patchThreads(staying, patch(false), back);
    back.catch((e) => fail("undo", e, all));
  };
  const message = all.length > 1 ? `Moved ${num(all.length)} to ${name}` : done;
  setUndo(done, undoRun, { message, steps: deltaSteps(items.map((i) => ({ ref: i.ref, delta: undoDelta(i.before, i.delta) }))) });
  toastId = toast({
    kind: "action",
    key: "undo:Moved",
    message,
    action: { label: "Undo", keys: "z", run: () => lastUndo?.run === undoRun && undo() },
  });
  call
    .catch((e) => {
      if (removal) restoreLocal(removal);
      if (change) revertChange(change);
      fail("move", e, all);
    })
    .finally(() => removal && settleRemoval(leaving.map((i) => i.ref), removal));
}

export function setLabel(refs: ThreadRef[], labelId: string, on: boolean) {
  modify(refs, on ? { kind: "addLabel", labelId } : { kind: "removeLabel", labelId }, on ? "add label" : "remove label");
}

export function openCompose(mode: "new" | "reply" | "replyAll" | "forward") {
  const ui = getUi();
  if (mode === "new") {
    setUi({ overlay: "compose", composeContext: { mode: "new" } });
    return;
  }
  const thread = ui.selected;
  if (!thread) return;
  const cached = cachedThread(thread);
  // The latest message that isn't my own unsent draft.
  const messageId = cached ? pickMessage(cached.messages, undefined)?.id : undefined;
  setUi({ overlay: "compose", composeContext: { mode, thread, messageId, whole: true } });
}
