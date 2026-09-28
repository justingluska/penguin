// Global UI state shared across features. Small on purpose.
// OWNER: inbox/thread agent (may extend; keep existing fields stable).
import { useSyncExternalStore } from "react";
import type { MailboxView, ThreadRef } from "./types";

export type Overlay = null | "search" | "command" | "compose" | "shortcuts" | "label" | "move" | "snooze" | "attachment" | "image" | "details";

export interface UiState {
  view: MailboxView;
  /** Mail (list + reading pane) or the Calendar (features/calendar). Threads open over either. */
  surface: "mail" | "calendar";
  /**
   * Split Inbox: the tab picked (a split id, or app/splits.ts OTHER). null =
   * the first. Only read while the Split Inbox is on and the inbox shows.
   */
  split: string | null;
  /** null = unified inbox across accounts (or across the active profile's accounts) */
  accountFilter: string | null;
  /**
   * Active account profile (Settings → Profiles): narrows the inbox, search,
   * labels and sync to its accounts. null = all accounts. Remembered on this
   * machine (localStorage), not a synced setting. See app/profiles.ts.
   */
  profileId: string | null;
  /**
   * The list's Unread filter: only threads with unread mail in the current
   * view (and account scope). Stays on across views until toggled off.
   */
  unreadOnly: boolean;
  selected: ThreadRef | null;
  /** When opened from search: jump to this message inside the thread. */
  focusMessageId: string | null;
  overlay: Overlay;
  /** Prefill for compose (reply/forward) — set by whoever opens compose. */
  composeContext: {
    mode: "new" | "reply" | "replyAll" | "forward";
    thread?: ThreadRef;
    messageId?: string;
    /**
     * The conversation as a whole (F, the thread and list menus, the reply
     * dock) rather than one message's own button: a forward also offers the
     * files on the conversation's other messages.
     */
    whole?: boolean;
    /** Gmail draft id when reopening a saved draft. */
    draftId?: string;
  } | null;
  theme: "dark" | "light";
  /** "system" follows prefers-color-scheme; T pins the opposite of the current theme. */
  themeSource: "system" | "user";
  /**
   * Full-width thread view (sidebar | thread | context panel) instead of the
   * list + reading pane. Set by o/Enter and by opening a search result; Esc
   * returns to the list.
   */
  threadOpen: boolean;
  /** Context panel (sender info) in the full thread view. Toggled with i. */
  contextPanel: boolean;
  /**
   * True when `selected` was placed by the app (first row after a list load)
   * rather than by the user. Such a thread is previewed but not marked read.
   */
  selectedByApp: boolean;
  /**
   * True when `selected` was opened from outside the mailbox list: a search
   * result, the person card, a notification, a link in the context panel
   * (openThread). Any other change of `selected` clears it (setUi). A hidden
   * account's mail opened this way isn't marked read (app/allInboxes.ts).
   */
  selectedOutside: boolean;
  /** Optional query to prefill when the search overlay opens ("All mail with …"). */
  searchPrefill: string | null;
  /**
   * Where each open thread came from, innermost last: Back (Esc, the thread
   * view's back button) returns to the top entry. Empty = back to the list.
   * Cleared whenever the thread view closes any other way.
   */
  navStack: NavEntry[];
  /**
   * Handed to an overlay that Back reopens (e.g. the search snapshot); the
   * overlay consumes it on mount and clears it.
   */
  overlayRestore: unknown;
}

/** Where a thread was opened from. `restore` is opaque, owned by that surface. */
export type NavOrigin = { kind: "search"; restore: unknown } | { kind: "person"; restore: unknown };

export interface NavEntry {
  origin: NavOrigin;
  /** The list selection before the thread opened, put back on return. */
  prevSelected: ThreadRef | null;
  prevSelectedByApp: boolean;
}

/** localStorage key for the active profile (written by app/profiles.ts). */
export const PROFILE_STORAGE_KEY = "penguin.activeProfile";

function storedProfile(): string | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage.getItem(PROFILE_STORAGE_KEY);
  } catch {
    // Storage can be unavailable (private mode, blocked site data): start in All.
    return null;
  }
}

const systemDark = () =>
  typeof window === "undefined" || !window.matchMedia ? true : window.matchMedia("(prefers-color-scheme: dark)").matches;

let state: UiState = {
  view: { kind: "inbox" },
  surface: "mail",
  split: null,
  accountFilter: null,
  profileId: storedProfile(),
  unreadOnly: false,
  selected: null,
  focusMessageId: null,
  overlay: null,
  composeContext: null,
  theme: systemDark() ? "dark" : "light",
  themeSource: "system",
  threadOpen: false,
  contextPanel: true,
  selectedByApp: false,
  selectedOutside: false,
  searchPrefill: null,
  navStack: [],
  overlayRestore: null,
};
const subs = new Set<() => void>();

export function getUi(): UiState {
  return state;
}
export function setUi(patch: Partial<UiState> | ((s: UiState) => Partial<UiState>)) {
  const p = typeof patch === "function" ? patch(state) : patch;
  state = { ...state, ...p };
  // Leaving the thread view by any route but goBack (switching views,
  // accounts, a removed thread…) forgets where it came from.
  if (p.threadOpen === false && !("navStack" in p) && state.navStack.length) state = { ...state, navStack: [] };
  // Selecting anything but through openThread (the list, j/k, Back, Undo…) is inside the list.
  if ("selected" in p && !("selectedOutside" in p) && state.selectedOutside) state = { ...state, selectedOutside: false };
  subs.forEach((f) => f());
}
export function subscribeUi(cb: () => void): () => void {
  subs.add(cb);
  return () => subs.delete(cb);
}
export function useUi<T>(select: (s: UiState) => T): T {
  return useSyncExternalStore(subscribeUi, () => select(state));
}

export function sameThread(a: ThreadRef | null | undefined, b: ThreadRef | null | undefined): boolean {
  return !!a && !!b && a.threadId === b.threadId && a.accountId === b.accountId;
}

/**
 * Open a thread in the full view (e.g. from search), optionally at a message.
 * With an `origin`, Back returns there; `keepOverlay` leaves the current
 * overlay up (search's ⌘Enter "open, keep search").
 */
export function openThread(
  ref: ThreadRef,
  focusMessageId: string | null = null,
  origin?: NavOrigin,
  keepOverlay = false,
) {
  setUi((s) => {
    let navStack = s.navStack;
    if (origin) {
      const entry: NavEntry = {
        origin,
        // Opening another result from the same surface replaces its entry
        // (and keeps the list selection from before the first one).
        prevSelected: s.threadOpen && navStack.length ? navStack[navStack.length - 1].prevSelected : s.selected,
        prevSelectedByApp: s.threadOpen && navStack.length ? navStack[navStack.length - 1].prevSelectedByApp : s.selectedByApp,
      };
      const top = navStack[navStack.length - 1];
      navStack = s.threadOpen && top?.origin.kind === origin.kind ? [...navStack.slice(0, -1), entry] : [...navStack, entry];
    }
    return {
      selected: ref,
      focusMessageId,
      threadOpen: true,
      overlay: keepOverlay ? s.overlay : null,
      selectedByApp: false,
      selectedOutside: true,
      navStack,
    };
  });
}

/**
 * The open conversation was just marked unread from inside it (U, ⇧U, the
 * toolbar's Mark unread, the message menu): keep it unread. The full thread
 * view goes Back to where it was opened from, as Gmail and Superhuman do,
 * leaving the cursor on it as if the app had placed it there (selectedByApp),
 * so the reading pane shows it without marking it read again. The reading
 * pane itself stays open; its mark-read already ran and won't run again
 * (features/thread/ThreadView useMarkRead).
 */
export function leaveMarkedUnread(refs: ThreadRef[]) {
  const ref = state.selected;
  if (!state.threadOpen || !ref || !refs.some((r) => sameThread(r, ref))) return;
  goBack();
  if (sameThread(state.selected, ref)) setUi({ selectedByApp: true });
}

/**
 * Back from the thread view: to where it was opened from (reopening that
 * surface from its snapshot, in the same update so the list never flashes),
 * else to the list.
 */
export function goBack() {
  setUi((s) => {
    const entry = s.navStack[s.navStack.length - 1];
    const navStack = s.navStack.slice(0, -1);
    if (!entry) return { threadOpen: false, focusMessageId: null, navStack };
    const back = { threadOpen: false, focusMessageId: null, navStack, selected: entry.prevSelected, selectedByApp: entry.prevSelectedByApp };
    switch (entry.origin.kind) {
      case "search":
        return { ...back, overlay: "search", overlayRestore: entry.origin.restore };
      case "person":
        // The person card isn't a back target yet; return to the list.
        return back;
    }
  });
}
