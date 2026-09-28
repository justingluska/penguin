// Mock snooze records, shared by ./mail.ts (list_threads, modify_threads)
// and ./snooze.ts (the commands and the wake timer). No imports, so neither
// side depends on the other's load order.
import type { Snooze } from "../types";

const k = (accountId: string, threadId: string) => `${accountId}:${threadId}`;
export const snoozes = new Map<string, Snooze>();

export function snoozeOf(accountId: string, threadId: string): Snooze | undefined {
  return snoozes.get(k(accountId, threadId));
}

export function setSnooze(z: Snooze) {
  snoozes.set(k(z.accountId, z.threadId), z);
}

export function dropSnooze(accountId: string, threadId: string): boolean {
  return snoozes.delete(k(accountId, threadId));
}

/** Like penguin-core's refresh_thread: back in the inbox (or trashed) before waking ends the snooze. */
export function reconcileSnooze(accountId: string, threadId: string, labelIds: string[]) {
  if (labelIds.includes("INBOX") || labelIds.includes("TRASH") || labelIds.includes("SPAM")) dropSnooze(accountId, threadId);
}
