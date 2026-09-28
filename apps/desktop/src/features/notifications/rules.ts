// New-mail notifications, the UI half (the backend decides what to show:
// src-tauri/src/notify.rs). Pure: tests/notifications.test.ts runs it.
import type { MailboxView, NotificationSettings, NotifyContext, ThreadRef } from "../../lib/types";

/** Whether an account's switch is on: its own, else on unless it's hidden from All Inboxes (settings.rs notifies_for). */
export function accountNotifies(n: NotificationSettings, hiddenFromAll: string[], accountId: string): boolean {
  return n.accounts[accountId] ?? !hiddenFromAll.includes(accountId);
}

/** The section after flipping one account's switch; an account back at its default is dropped from the map. */
export function withAccount(n: NotificationSettings, hiddenFromAll: string[], accountId: string, on: boolean): NotificationSettings {
  const accounts = { ...n.accounts };
  if (on === !hiddenFromAll.includes(accountId)) delete accounts[accountId];
  else accounts[accountId] = on;
  return { ...n, accounts };
}

/** What the UI shows, as far as notifications care (a slice of UiState). */
export interface ScreenInput {
  surface: "mail" | "calendar";
  view: MailboxView;
  overlay: string | null;
  threadOpen: boolean;
  selected: ThreadRef | null;
  accountFilter: string | null;
  /** app/store listScope: the account set the list shows; null = every account. */
  scope: string[] | null;
  /** Every signed-in account. */
  accountIds: string[];
}

/**
 * What's on screen: the accounts whose inbox list is showing, and the thread
 * open or previewed. An overlay (compose, search, a dialog) or the calendar
 * covers the mail, so nothing counts as shown then.
 */
export function notifyContextOf(s: ScreenInput): NotifyContext {
  const mail = s.surface === "mail" && s.overlay === null;
  if (!mail) return { inboxAccounts: [], thread: null };
  let inboxAccounts: string[] = [];
  if (s.view.kind === "inbox" && !s.threadOpen) {
    const inScope = s.scope ?? s.accountIds;
    inboxAccounts = s.accountFilter ? (inScope.includes(s.accountFilter) || s.scope === null ? [s.accountFilter] : []) : inScope;
  }
  return { inboxAccounts, thread: s.selected ? { accountId: s.selected.accountId, threadId: s.selected.threadId } : null };
}

/** Where to turn notifications back on (macOS can't tell us they're off, only that they don't appear). */
export const SYSTEM_SETTINGS_HELP = "System Settings → Notifications → Penguin, and turn on Allow notifications";
