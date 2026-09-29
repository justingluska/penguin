// What moves between windows when a message does (pure; tests/windows.test.ts).
//
// A composer's whole editor state travels as a seed: Pop out hands it to a
// new compose window (through open_window, which keeps it for that window
// only), and a compose window that sends hands it to the main window, where
// the Undo countdown runs (handoff.ts). The state is plain JSON (the editor's
// document is ProseMirror JSON; files are base64 or refs to the saved draft),
// but it comes from another window, so it is checked before use.
import type { EditorState } from "./draft";

export interface ComposeSeed {
  v: 1;
  state: EditorState;
  /** The last save didn't reach the server (or never ran): save again before relying on the draft. */
  unsaved: boolean;
}

const isObj = (x: unknown): x is Record<string, unknown> => !!x && typeof x === "object" && !Array.isArray(x);
const isStr = (x: unknown): x is string => typeof x === "string";
const isStrOrNull = (x: unknown) => x === null || isStr(x);
const isAddr = (x: unknown) => isObj(x) && isStr(x.email) && (x.name === null || x.name === undefined || isStr(x.name));
const isAddrs = (x: unknown) => Array.isArray(x) && x.every(isAddr);
const MODES = new Set(["new", "reply", "replyAll", "forward"]);

function isAttachment(x: unknown): boolean {
  if (!isObj(x) || !isStr(x.filename) || !isStr(x.mimeType)) return false;
  if (x.kind === "file") return isStr(x.dataBase64);
  if (x.kind === "gmail") return isStr(x.messageId) && isStr(x.attachmentId) && typeof x.size === "number";
  return false;
}

function isThreadRef(x: unknown): boolean {
  return isObj(x) && isStr(x.accountId) && isStr(x.threadId);
}

/** A seed from another window, or null when it isn't one (then the window opens blank or from the saved draft). */
export function asComposeSeed(x: unknown): ComposeSeed | null {
  if (!isObj(x) || x.v !== 1 || typeof x.unsaved !== "boolean" || !isObj(x.state)) return null;
  const s = x.state;
  const ctx = s.ctx;
  if (!isObj(ctx) || !MODES.has(ctx.mode as string) || (ctx.thread !== undefined && !isThreadRef(ctx.thread))) return null;
  if (!isStr(s.key) || !isStr(s.accountId) || !isStrOrNull(s.draftId) || !isStrOrNull(s.draftAccountId)) return null;
  if (!isAddrs(s.to) || !isAddrs(s.cc) || !isAddrs(s.bcc)) return null;
  if (!isStr(s.subject) || !isStr(s.body)) return null;
  if (!Array.isArray(s.attachments) || !s.attachments.every(isAttachment)) return null;
  if (!Array.isArray(s.earlier) || !s.earlier.every(isAttachment)) return null;
  if (s.bodyDoc !== null && !isObj(s.bodyDoc)) return null;
  if (!isStrOrNull(s.bodyHtml) || !isStrOrNull(s.replyToThreadId) || !isStrOrNull(s.replyToMessageId)) return null;
  if (s.quote !== null && !(isObj(s.quote) && typeof s.quote.forward === "boolean" && isStr(s.quote.header) && isStr(s.quote.text))) return null;
  if (s.remindAfterMs !== null && typeof s.remindAfterMs !== "number") return null;
  if (s.replyingTo !== null && !isAddr(s.replyingTo)) return null;
  if (s.original !== null && !(isObj(s.original) && isStr(s.original.accountId) && isStr(s.original.messageId))) return null;
  return {
    v: 1,
    unsaved: x.unsaved,
    state: {
      ...(s as unknown as EditorState),
      showCc: s.showCc === true,
      showBcc: s.showBcc === true,
      touched: s.touched === true,
    },
  };
}

/** The window's title for a message: its subject, else what it is. */
export function composeTitle(state: Pick<EditorState, "subject" | "ctx">): string {
  const s = state.subject.trim();
  if (s) return s;
  return state.ctx.mode === "new" ? "New Message" : state.ctx.mode === "forward" ? "Forward" : "Reply";
}

/**
 * How many seconds of Undo a send handed to the main window still has: what
 * the setting gives, less what already ran where it started (never below one
 * second, so Undo is always offered when the setting asks for it).
 */
export function remainingUndo(settingSeconds: number, elapsedMs: number): number {
  if (settingSeconds <= 0) return 0;
  return Math.max(1, Math.round(settingSeconds - elapsedMs / 1000));
}
