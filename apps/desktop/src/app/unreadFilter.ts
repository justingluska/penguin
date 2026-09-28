// The list's Unread filter, refresh side: pure (no store, no React) so it can
// be unit-tested (tests/unreadFilter.test.ts). Keep this file free of imports
// and non-erasable TS syntax: the tests run it under Node's type stripping.

export interface Row {
  accountId: string;
  threadId: string;
  unread: boolean;
  lastDate: number;
}

const key = (r: Row) => r.accountId + "\u0000" + r.threadId;

/**
 * Merge a refreshed unread-only page with the rows on screen. A thread you
 * read here stays in the list (shown as read) until the filter or the view
 * changes, so reading doesn't pull the row out from under the cursor. A row
 * the backend dropped while it was still unread on screen went away for
 * another reason (archived, or read on another device) and is not kept, and
 * neither is one being removed optimistically (`removed`). Only rows inside
 * the refreshed window (lastDate ≥ floor) are considered; the caller keeps
 * the paged tail below it.
 */
export function keepReadHere<T extends Row>(fresh: T[], onScreen: T[], floor: number, removed: (k: string) => boolean): T[] {
  const have = new Set(fresh.map(key));
  const kept = onScreen.filter((t) => !t.unread && t.lastDate >= floor && !have.has(key(t)) && !removed(key(t)));
  if (kept.length === 0) return fresh;
  return [...fresh, ...kept].sort((a, b) => b.lastDate - a.lastDate);
}
