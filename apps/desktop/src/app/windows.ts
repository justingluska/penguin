// Opening conversations and new messages in windows of their own
// (src-tauri/src/windows.rs; the shells are app/windowShell.tsx). ⇧O, the
// row and thread menus, Message → Open in New Window and ⌘K open a
// conversation; ⌥⌘N, a right-click on New and ⌘K a new message.
import { api, asCommandError } from "../lib/api";
import type { ThreadRef } from "../lib/types";
import { toast } from "../components/Toast";
import type { MenuEntries } from "../components/ContextMenu";
import { cachedThread, listSummary } from "./store";
import { openCompose } from "./actions";
import { preferredFromAccount } from "./profiles";

/** Open a conversation in its own window (or bring its window forward). */
export async function openThreadWindow(ref: ThreadRef): Promise<void> {
  // The subject titles the window until it has loaded the thread itself.
  const title = listSummary(ref)?.subject || cachedThread(ref)?.subject || null;
  try {
    await api.openWindow({ kind: "thread", accountId: ref.accountId, threadId: ref.threadId, title });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't open a window: ${asCommandError(e).message}` });
  }
}

/** Right-click on New (the sidebar, Floe): here, or in a window of its own. */
export function newMessageMenu(): MenuEntries {
  return [
    { label: "New message", icon: "compose", keys: "c", onSelect: () => openCompose("new") },
    { label: "New message in new window", icon: "window", keys: "mod+alt+n", onSelect: () => void openComposeWindow() },
  ];
}

/** A new message in its own window, from the account the main composer would pick. */
export async function openComposeWindow(): Promise<void> {
  try {
    await api.openWindow({ kind: "compose", accountId: preferredFromAccount() });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't open a window: ${asCommandError(e).message}` });
  }
}
