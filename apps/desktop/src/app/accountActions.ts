// Account edits shared by Settings → Accounts and the sidebar's account
// context menu: nickname/color (update_account), order (Settings.accountOrder)
// and removal. The store's account list is patched in place so every surface
// updates at once.
import type { Account, AccountPatch } from "../lib/types";
import { api, asCommandError } from "../lib/api";
import { getUi, setUi } from "../lib/ui";
import { toast } from "../components/Toast";
import { meta, setAccounts } from "./store";
import { currentSettings, updateSettings, useSetting } from "../lib/settings";
import { isHiddenFromAll, withShownInAll } from "./allInboxes";
import { moveBy, moveWithin, sameOrder } from "./accountOrder";

/** MAX_NICKNAME_CHARS in src-tauri/src/ops.rs. */
export const MAX_NICKNAME = 32;

export async function patchAccount(a: Account, patch: AccountPatch): Promise<void> {
  try {
    const next = await api.updateAccount(a.id, patch);
    setAccounts(meta.get().accounts.map((x) => (x.id === next.id ? next : x)));
  } catch (e) {
    toast({ tone: "error", message: `Couldn't update ${a.email}: ${asCommandError(e).message}` });
  }
}

/** Whether the account's mail is part of "All accounts" (Settings.hiddenFromAll). */
export function useShownInAll(accountId: string): boolean {
  return !isHiddenFromAll(useSetting("hiddenFromAll"), accountId);
}

/**
 * Show the account's mail in "All accounts", or leave it out. It stays
 * signed in and syncing either way. Resolves false (after a toast) when the
 * setting couldn't be saved.
 */
export async function setShownInAll(a: Account, shown: boolean): Promise<boolean> {
  const ids = meta.get().accounts.map((x) => x.id);
  try {
    await updateSettings({ hiddenFromAll: withShownInAll(currentSettings().hiddenFromAll, a.id, shown, ids) });
    return true;
  } catch (e) {
    toast({ tone: "error", message: `Couldn't update ${a.email}: ${asCommandError(e).message}` });
    return false;
  }
}

/**
 * Move an account to `toIndex` of the list the user is looking at (`visible`:
 * every account, or a profile's accounts), counted without the account
 * itself, and save the new order (Settings.accountOrder). Accounts outside
 * `visible` keep their places. Resolves false (after a toast) on failure.
 */
export async function moveAccountTo(a: Account, visible: string[], toIndex: number): Promise<boolean> {
  const all = meta.get().accounts.map((x) => x.id);
  return saveOrder(a, moveWithin(all, visible, a.id, toIndex));
}

/** "Move up" / "Move down" (menus, ⌥↑/⌥↓ in Settings → Accounts). */
export async function moveAccountBy(a: Account, visible: string[], delta: -1 | 1): Promise<boolean> {
  const all = meta.get().accounts.map((x) => x.id);
  return saveOrder(a, moveBy(all, visible, a.id, delta));
}

async function saveOrder(a: Account, order: string[]): Promise<boolean> {
  if (sameOrder(order, meta.get().accounts.map((x) => x.id))) return true;
  try {
    await updateSettings({ accountOrder: order });
    return true;
  } catch (e) {
    toast({ tone: "error", message: `Couldn't move ${a.email}: ${asCommandError(e).message}` });
    return false;
  }
}

/**
 * Sign out and delete the account's local mail (the caller confirms first).
 * Resolves true once removed. Removing the last account reloads the app,
 * which boots back into onboarding.
 */
export async function removeAccount(a: Account): Promise<boolean> {
  try {
    await api.removeAccount(a.id);
    const left = await api.listAccounts();
    if (left.length === 0) {
      location.reload();
      return true;
    }
    setAccounts(left);
    if (getUi().accountFilter === a.id) setUi({ accountFilter: null, selected: null });
    toast({ message: `Removed ${a.email} and its local mail` });
    return true;
  } catch (e) {
    toast({ tone: "error", message: `Couldn't remove ${a.email}: ${asCommandError(e).message}` });
    return false;
  }
}

/** Gmail in the browser, signed in as this account. */
export function gmailInboxUrl(email: string): string {
  return `https://mail.google.com/mail/u/${encodeURIComponent(email)}/`;
}

/** A conversation in Gmail (thread ids are the same hex ids Gmail's web UI uses). */
export function gmailThreadUrl(email: string, threadId: string): string {
  return `https://mail.google.com/mail/u/${encodeURIComponent(email)}/#all/${encodeURIComponent(threadId)}`;
}
