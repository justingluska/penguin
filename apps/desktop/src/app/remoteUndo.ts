// An Undo that can run in another window (pure; tests/windows.test.ts).
//
// Archiving, trashing, snoozing or moving the conversation in a conversation
// window closes that window, so its "Archived · Undo" can't stay there. Each
// such action also describes its undo as data: steps of backend calls, the
// calls in a step in parallel, the steps in order (a label first, then the
// inbox). The closing window hands that to the main window
// (app/windowShell.tsx → app/handoffs.ts), which offers the same toast and Z.
import type { ThreadAction, ThreadRef } from "../lib/types";

export type UndoOp =
  | { cmd: "modify"; refs: ThreadRef[]; action: ThreadAction }
  | { cmd: "snooze"; refs: ThreadRef[]; until: number }
  | { cmd: "unsnooze"; refs: ThreadRef[]; toInbox: boolean }
  | { cmd: "replyLater"; refs: ThreadRef[]; on: boolean }
  | { cmd: "dismissFollowUps"; refs: ThreadRef[]; dismissed: boolean };

export interface RemoteUndo {
  /** The toast's text ("Archived", "Snoozed until tomorrow"). */
  message: string;
  steps: UndoOp[][];
}

const ACTIONS = new Set(["archive", "moveToInbox", "trash", "untrash", "markRead", "markUnread", "star", "unstar", "addLabel", "removeLabel", "replyLater"]);

const isObj = (x: unknown): x is Record<string, unknown> => !!x && typeof x === "object" && !Array.isArray(x);
const isRefs = (x: unknown): x is ThreadRef[] =>
  Array.isArray(x) && x.length > 0 && x.length <= 10_000 && x.every((r) => isObj(r) && typeof r.accountId === "string" && typeof r.threadId === "string");

function isAction(x: unknown): x is ThreadAction {
  if (!isObj(x) || !ACTIONS.has(x.kind as string)) return false;
  if (x.kind === "addLabel" || x.kind === "removeLabel" || x.kind === "replyLater") return typeof x.labelId === "string";
  return true;
}

function isOp(x: unknown): x is UndoOp {
  if (!isObj(x) || !isRefs(x.refs)) return false;
  switch (x.cmd) {
    case "modify":
      return isAction(x.action);
    case "snooze":
      return typeof x.until === "number" && Number.isFinite(x.until);
    case "unsnooze":
      return typeof x.toInbox === "boolean";
    case "replyLater":
      return typeof x.on === "boolean";
    case "dismissFollowUps":
      return typeof x.dismissed === "boolean";
    default:
      return false;
  }
}

/** An undo from another window, or null when it isn't one. Empty steps are dropped. */
export function asRemoteUndo(x: unknown): RemoteUndo | null {
  if (!isObj(x) || typeof x.message !== "string" || !x.message || !Array.isArray(x.steps)) return null;
  const steps: UndoOp[][] = [];
  for (const step of x.steps) {
    if (!Array.isArray(step) || !step.every(isOp)) return null;
    if (step.length) steps.push(step);
  }
  return steps.length ? { message: x.message.slice(0, 200), steps } : null;
}

/** One step with one call. */
export const single = (op: UndoOp): UndoOp[][] => [[op]];
