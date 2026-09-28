// "Show in All Inboxes" (Settings → Accounts, the sidebar account menu).
// An account left out of All stays signed in and syncing; only the implicit
// "All accounts" scope (no account picked, no profile) skips it. Picking the
// account, or a profile that lists it, shows its mail as usual: profiles are
// explicit, so they win.
//
// Pure scoping rules here (tests/allInboxes.test.ts); store.ts and the
// sidebar apply them to the live UI state and settings.

export interface ScopeInput {
  /** The single account picked in the switcher or sidebar, if any. */
  accountFilter: string | null;
  /** The active profile's signed-in accounts; null when no profile is active. */
  profileScope: string[] | null;
  /** Every signed-in account id, in sidebar order. */
  accountIds: string[];
  /** Settings.hiddenFromAll. */
  hidden: string[];
}

/**
 * The account set mailbox queries are limited to (ListQuery.accountIds):
 * null = every account. A picked account or a profile keeps its own scope;
 * only "All accounts" leaves the hidden ones out. When nothing is hidden it
 * stays null, so the backend keeps its unscoped fast path.
 */
export function mailScope({ accountFilter, profileScope, accountIds, hidden }: ScopeInput): string[] | null {
  if (accountFilter || profileScope) return profileScope;
  if (hidden.length === 0) return null;
  const out = new Set(hidden);
  const shown = accountIds.filter((id) => !out.has(id));
  return shown.length === accountIds.length ? null : shown;
}

/** Whether "All accounts" leaves this account out. */
export function isHiddenFromAll(hidden: string[], accountId: string): boolean {
  return hidden.includes(accountId);
}

/**
 * The hidden list after turning one account's "Show in All Inboxes" on or
 * off. Ids that aren't signed-in accounts are dropped, so a removed account
 * doesn't linger in settings.json. Order follows the accounts.
 */
export function withShownInAll(hidden: string[], accountId: string, shown: boolean, accountIds: string[]): string[] {
  const next = new Set(hidden);
  if (shown) next.delete(accountId);
  else next.add(accountId);
  return accountIds.filter((id) => next.has(id));
}

/**
 * Whether opening a conversation marks it read. A hidden account's mail is
 * only marked read when you opened it from the list of a view that shows that
 * account (you picked the account, or a profile that lists it): you went there
 * to read it. Found through search, the person card, a notification or a
 * link, it stays unread: you looked something up, you didn't triage that
 * inbox. Every other account's mail is marked read as usual, and U still
 * marks any of them read or unread by hand.
 */
export function marksReadOnOpen(
  hidden: string[],
  accountId: string,
  opened: { outsideList: boolean; accountInView: boolean },
): boolean {
  return !isHiddenFromAll(hidden, accountId) || (!opened.outsideList && opened.accountInView);
}
