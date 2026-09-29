// Reply Later and Follow up actions (backend: src-tauri reply_later.rs).
//
// Reply Later (Y): a conversation you've read and owe a reply leaves the
// Inbox at once, stays read, and waits in Reply Later under its synced label.
// Y on a conversation that's already there puts it back in the Inbox. E in
// the Reply Later view is "done here" (the label comes off; it stays
// archived). Follow up: E / Dismiss hides a conversation until something
// newer is sent in it. Every one of these has an Undo toast wired to Z.
import { inboxLikeView } from "../smart/catalog";
import { api } from "../../lib/api";
import type { ThreadRef, ThreadSummary } from "../../lib/types";
import { REPLY_LATER_LABEL } from "../../lib/types";
import { getUi } from "../../lib/ui";
import { num } from "../../lib/format";
import { dismissToast, toast } from "../../components/Toast";
import { fail, isPendingUndo, isUnread, runUndoSteps, setUndo, targets, undo } from "../../app/actions";
import { single, type UndoOp } from "../../app/remoteUndo";
import { cachedThread, list, meta, patchThreads, removeLocal, restoreLocal, revertChange, settleRemoval, type Removal } from "../../app/store";
import { reloadTriageSoon, shiftTriageCount } from "./state";

const key = (r: ThreadRef) => r.accountId + "\u0000" + r.threadId;

function summary(ref: ThreadRef): ThreadSummary | undefined {
  return list.get().items.find((t) => t.threadId === ref.threadId && t.accountId === ref.accountId);
}

function labelsOf(ref: ThreadRef): string[] {
  return summary(ref)?.labelIds ?? cachedThread(ref)?.labelIds ?? [];
}

/** Each account's Reply Later label id, from the loaded labels (null: not created yet). */
export function replyLaterLabel(accountId: string): string | null {
  const want = REPLY_LATER_LABEL.toLowerCase();
  return meta.get().labels.find((l) => l.accountId === accountId && l.kind === "user" && l.name.toLowerCase() === want)?.id ?? null;
}

/** Whether the conversation carries its account's Reply Later label. */
export function inReplyLater(ref: ThreadRef): boolean {
  if (getUi().view.kind === "replyLater") return true;
  const id = replyLaterLabel(ref.accountId);
  return !!id && labelsOf(ref).includes(id);
}

const undoToast = (message: string, undoKey: string, run: () => void) =>
  toast({
    kind: "action",
    key: `undo:${undoKey}`,
    message,
    action: { label: "Undo", keys: "z", run: () => isPendingUndo(run) && undo() },
  });

/** Y: into Reply Later, or, when every target is already there, back to the Inbox. */
export function toggleReplyLater(refs = targets()) {
  if (refs.length === 0) return;
  if (refs.every(inReplyLater)) return backToInbox(refs);
  return markReplyLater(refs.filter((r) => !inReplyLater(r)));
}

/** Out of the Inbox now, read, into Reply Later (the label is created on first use). */
export function markReplyLater(refs: ThreadRef[]) {
  if (refs.length === 0) return;
  const prev = refs.map((ref) => ({ ref, unread: isUnread(ref), inbox: getUi().view.kind === "inbox" || labelsOf(ref).includes("INBOX") }));
  // Leaves the views that show inbox mail only; elsewhere the row stays, read and archived.
  const leaving = getUi().view.kind === "inbox" || inboxLikeView(getUi().view) || getUi().unreadOnly;
  const call = api.replyLater(refs, true);
  const marked = (t: ThreadSummary): ThreadSummary => {
    const label = replyLaterLabel(t.accountId);
    const labelIds = t.labelIds.filter((l) => l !== "INBOX" && l !== "UNREAD");
    return { ...t, unread: false, labelIds: label && !labelIds.includes(label) ? [...labelIds, label] : labelIds };
  };
  const removal: Removal | null = leaving ? removeLocal(refs, marked) : null;
  const was = new Map(refs.map((r) => [key(r), summary(r)]));
  const change = removal ? null : patchThreads(refs, marked, call);
  shiftTriageCount(refs, "replyLater", 1);
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    const back = putBack(prev);
    if (removal) restoreLocal(removal, back);
    else patchThreads(refs, (t) => was.get(key(t)) ?? t, back);
    shiftTriageCount(refs, "replyLater", -1);
    back.catch((e) => fail("undo", e, refs)).finally(reloadTriageSoon);
  };
  const message = refs.length > 1 ? `Moved ${num(refs.length)} to Reply Later` : "Moved to Reply Later";
  setUndo("Reply later", undoRun, { message, steps: putBackSteps(prev) });
  toastId = undoToast(message, "ReplyLater", undoRun);
  call
    .catch((e) => {
      if (removal) restoreLocal(removal);
      if (change) revertChange(change);
      reloadTriageSoon();
      fail("move to Reply Later", e, refs);
    })
    .finally(() => removal && settleRemoval(refs, removal));
}

/** Undo a mark: the label off, then the inbox and unread state as they were. */
function putBack(prev: { ref: ThreadRef; unread: boolean; inbox: boolean }[]) {
  return runUndoSteps(putBackSteps(prev));
}

/** putBack as data (app/remoteUndo.ts), for a conversation window that closes. */
function putBackSteps(prev: { ref: ThreadRef; unread: boolean; inbox: boolean }[]): UndoOp[][] {
  const inbox = prev.filter((p) => p.inbox).map((p) => p.ref);
  const unread = prev.filter((p) => p.unread).map((p) => p.ref);
  return [
    [{ cmd: "replyLater", refs: prev.map((p) => p.ref), on: false }],
    [
      ...(inbox.length ? [{ cmd: "modify" as const, refs: inbox, action: { kind: "moveToInbox" as const } }] : []),
      ...(unread.length ? [{ cmd: "modify" as const, refs: unread, action: { kind: "markUnread" as const } }] : []),
    ],
  ];
}

/** Y on a Reply Later conversation: the label comes off and it's back in the Inbox. */
function backToInbox(refs: ThreadRef[]) {
  const here = getUi().view.kind === "replyLater";
  const unmarked = (t: ThreadSummary): ThreadSummary => {
    const label = replyLaterLabel(t.accountId);
    const labelIds = t.labelIds.filter((l) => l !== label);
    return { ...t, labelIds: labelIds.includes("INBOX") ? labelIds : [...labelIds, "INBOX"] };
  };
  const call = api.replyLater(refs, false).then(() => api.modifyThreads(refs, { kind: "moveToInbox" }));
  const removal: Removal | null = here ? removeLocal(refs, unmarked) : null;
  const change = removal ? null : patchThreads(refs, unmarked, call);
  shiftTriageCount(refs, "replyLater", -1);
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    const back = api.replyLater(refs, true);
    if (removal) restoreLocal(removal, back);
    shiftTriageCount(refs, "replyLater", 1);
    back.catch((e) => fail("undo", e, refs)).finally(reloadTriageSoon);
  };
  const message = refs.length > 1 ? `Moved ${num(refs.length)} back to the inbox` : "Moved back to the inbox";
  setUndo("Back to inbox", undoRun, { message, steps: single({ cmd: "replyLater", refs, on: true }) });
  toastId = undoToast(message, "ReplyLaterOff", undoRun);
  call
    .catch((e) => {
      if (removal) restoreLocal(removal);
      if (change) revertChange(change);
      reloadTriageSoon();
      fail("move back to the inbox", e, refs);
    })
    .finally(() => removal && settleRemoval(refs, removal));
}

/** E in Reply Later: done there without replying (the label comes off; it stays archived). */
export function leaveReplyLater(refs: ThreadRef[]) {
  if (refs.length === 0) return;
  const call = api.replyLater(refs, false);
  const removal = removeLocal(refs, (t) => ({ ...t, labelIds: t.labelIds.filter((l) => l !== replyLaterLabel(t.accountId)) }));
  shiftTriageCount(refs, "replyLater", -1);
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    const back = api.replyLater(refs, true);
    restoreLocal(removal, back);
    shiftTriageCount(refs, "replyLater", 1);
    back.catch((e) => fail("undo", e, refs)).finally(reloadTriageSoon);
  };
  const message = refs.length > 1 ? `Took ${num(refs.length)} out of Reply Later` : "Took it out of Reply Later";
  setUndo("Done", undoRun, { message, steps: single({ cmd: "replyLater", refs, on: true }) });
  toastId = undoToast(message, "ReplyLaterDone", undoRun);
  call
    .catch((e) => {
      restoreLocal(removal);
      reloadTriageSoon();
      fail("update Reply Later", e, refs);
    })
    .finally(() => settleRemoval(refs, removal));
}

/** Follow up's Dismiss (E): hidden until something newer is sent in the conversation. */
export function dismissFollowUps(refs: ThreadRef[]) {
  if (refs.length === 0) return;
  const here = getUi().view.kind === "followUp";
  const call = api.dismissFollowUps(refs, true);
  const removal: Removal | null = here ? removeLocal(refs) : null;
  shiftTriageCount(refs, "followUp", -1);
  let toastId = 0;
  const undoRun = () => {
    dismissToast(toastId);
    const back = api.dismissFollowUps(refs, false);
    if (removal) restoreLocal(removal, back);
    shiftTriageCount(refs, "followUp", 1);
    back.catch((e) => fail("undo", e, refs)).finally(reloadTriageSoon);
  };
  const message = refs.length > 1 ? `Dismissed ${num(refs.length)} follow-ups` : "Dismissed";
  setUndo("Dismissed", undoRun, { message, steps: single({ cmd: "dismissFollowUps", refs, dismissed: false }) });
  toastId = undoToast(message, "Dismissed", undoRun);
  call
    .catch((e) => {
      if (removal) restoreLocal(removal);
      reloadTriageSoon();
      fail("dismiss", e, refs);
    })
    .finally(() => removal && settleRemoval(refs, removal));
}
