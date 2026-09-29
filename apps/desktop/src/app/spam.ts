// Report spam / Not spam, the pure parts (tests/spam.test.ts): what Undo
// sends, and whether a set of conversations counts as spam. Keep this file
// free of runtime imports: the tests run it under Node's type stripping.
import type { ThreadRef } from "../lib/types";
import type { UndoOp } from "./remoteUndo";

export const SPAM = "SPAM";

/**
 * Undo of Report spam, as one step of calls: what was in the inbox goes back
 * there (Not spam), the rest only leaves Spam, so an archived conversation
 * goes back to Done rather than into the inbox.
 */
export function reportSpamUndo(refs: ThreadRef[], wasInInbox: (ref: ThreadRef) => boolean): UndoOp[][] {
  const inbox = refs.filter(wasInInbox);
  const elsewhere = refs.filter((r) => !wasInInbox(r));
  const ops: UndoOp[] = [];
  if (inbox.length) ops.push({ cmd: "modify", refs: inbox, action: { kind: "notSpam" } });
  if (elsewhere.length) ops.push({ cmd: "modify", refs: elsewhere, action: { kind: "removeLabel", labelId: SPAM } });
  return ops.length ? [ops] : [];
}

/** Whether `!` means Not spam: in the Spam view, or every target carries SPAM. */
export function targetsAreSpam(viewKind: string, labelsOf: readonly (readonly string[] | null)[]): boolean {
  if (viewKind === "spam") return true;
  return labelsOf.length > 0 && labelsOf.every((l) => !!l && l.includes(SPAM));
}
