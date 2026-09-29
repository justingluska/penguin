// The app's keyboard vocabulary (Gmail/Superhuman). Registered once by App.
// Overlays (search, ⌘K, compose) register their own keys; everything here is
// inactive while an overlay is open unless noted.
import { isMac, registerShortcuts, type Shortcut } from "../lib/keyboard";
import { subscribeSettings } from "../lib/settings";
import { getUi, goBack, setUi, type UiState } from "../lib/ui";
import { toggleSidebar } from "../lib/layout";
import { startRefresh } from "./refresh";
import type { MailboxView } from "../lib/types";
import { openDraftForThread } from "../features/compose";
import { accountName } from "../components/Identity";
import { archive, markUnreadAndSay, onTargets, openCompose, openLabelOrMove, targetAbilities, toggleRead, toggleStar, trash, undo } from "./actions";
import {
  clearSelection,
  hasSelection as hasMultiSelection,
  selectAllLoaded,
  selectRange,
  toggleSelect,
} from "./selection";
import { meta, moveSelection, selectIndex, list, profileScope, toggleUnreadOnly } from "./store";
import { currentUnsubscribeTarget, startUnsubscribe } from "../features/unsubscribe/actions";
import { getProfiles, switchProfile } from "./profiles";
import { openSnooze } from "../features/snooze/SnoozePicker";
import { toggleReplyLater } from "../features/triage/actions";
import { COPY_CONVERSATION_KEYS, copyConversation } from "../features/thread/copy";
import { openComposeWindow, openThreadWindow } from "./windows";

// The ⌘K palette evaluates when() to list context-valid commands while it's
// open, so "command" counts as no overlay. Keys can't fire meanwhile: the
// palette's input has focus and the input guard blocks them.
const noOverlay = () => {
  const o = getUi().overlay;
  return o === null || o === "command";
};
/** A cursor row or a multi-selection to act on. */
const hasSelection = () =>
  noOverlay() && (getUi().surface === "mail" || getUi().threadOpen) && (getUi().selected !== null || hasMultiSelection());
// j/k move the list cursor (or step threads in the open thread): not while the
// calendar is showing, where the list and its selection are hidden.
const onMail = () => noOverlay() && (getUi().threadOpen || getUi().surface === "mail");
const inList = () => noOverlay() && !getUi().threadOpen && getUi().surface === "mail";
const inThread = () => noOverlay() && getUi().threadOpen && getUi().selected !== null;

export function goTo(view: MailboxView) {
  setUi({ view, surface: "mail", threadOpen: false, overlay: null });
}

/** The Calendar surface (features/calendar); any goTo(view) returns to mail. */
export function goToCalendar() {
  setUi({ surface: "calendar", threadOpen: false, overlay: null });
}

export function toggleTheme() {
  setUi((s) => ({ theme: s.theme === "dark" ? "light" : "dark", themeSource: "user" }));
}

/**
 * Narrow to one account (null = every account in scope). Inside a profile,
 * picking an account outside it leaves the profile.
 */
export function switchAccount(accountId: string | null) {
  const scope = profileScope();
  const leave = accountId !== null && scope !== null && !scope.includes(accountId);
  setUi({ accountFilter: accountId, threadOpen: false, ...(leave ? { profileId: null } : {}) });
}

/** ⌃ on the Mac (⌘ is taken by the app menu's ⌘1…); Ctrl elsewhere. */
export const PROFILE_MOD = isMac ? "ctrl" : "mod";

/** Key for the n-th profile (0-based), or undefined past the ninth. */
export function profileKeys(index: number): string | undefined {
  return index < 9 ? `${PROFILE_MOD}+${index + 1}` : undefined;
}

/** o / Enter: open the thread, or in Drafts, reopen the draft in compose. */
export function openSelected() {
  const ui = getUi();
  if (!ui.selected) return;
  if (ui.view.kind === "drafts") void openDraftForThread(ui.selected);
  else setUi({ threadOpen: true, selectedByApp: false });
}

/** Message-level navigation inside the open thread (n / p / o), handled by ThreadView. */
export const threadNav = {
  next: () => {},
  prev: () => {},
  expandAll: () => {},
};

/**
 * The keys a conversation window answers to (app/windowShell.tsx): its own
 * conversation's triage, replies, message navigation, Undo, ⌘K and the sheet.
 * Nothing that switches mailbox, account or list: that's the main window's.
 */
export const THREAD_WINDOW_KEYS = new Set([
  "nav.back",
  "msg.next",
  "msg.prev",
  "msg.expand",
  "ctx.toggle",
  "triage.done",
  "triage.trash",
  "triage.star",
  "triage.read",
  "triage.unread",
  "triage.label",
  "triage.move",
  "triage.move.l",
  "triage.unsubscribe",
  "triage.snooze",
  "triage.replyLater",
  "thread.copy",
  "triage.undo",
  "triage.undo.mod",
  "compose.reply",
  "compose.replyAll",
  "compose.forward",
  "compose.newWindow",
  "command.open",
  "app.shortcuts",
  "app.theme",
]);

/** ⇧O: the cursor's conversation in a window of its own (one row, not a multi-selection). */
const canOpenWindow = () =>
  noOverlay() && (getUi().surface === "mail" || getUi().threadOpen) && getUi().selected !== null && !hasMultiSelection();

/**
 * Register the app's keys: all of them in the main window, THREAD_WINDOW_KEYS
 * in a conversation window.
 */
export function registerAppShortcuts(scope: "main" | "thread" = "main"): () => void {
  const all: Shortcut[] = [
    // ---- Navigate
    { id: "nav.down", keys: "j", label: "Next conversation", group: "Navigate", when: onMail, run: () => moveSelection(1) },
    { id: "nav.up", keys: "k", label: "Previous conversation", group: "Navigate", when: onMail, run: () => moveSelection(-1) },
    { id: "nav.down.arrow", keys: "arrowdown", label: "Next conversation", group: "Navigate", hidden: true, when: inList, run: () => moveSelection(1) },
    { id: "nav.up.arrow", keys: "arrowup", label: "Previous conversation", group: "Navigate", hidden: true, when: inList, run: () => moveSelection(-1) },
    { id: "nav.open", keys: "o", label: "Open conversation", group: "Navigate", when: () => inList() && getUi().selected !== null, run: openSelected },
    { id: "nav.open.enter", keys: "enter", label: "Open conversation", group: "Navigate", hidden: true, when: () => inList() && getUi().selected !== null, run: openSelected },
    { id: "thread.openWindow", keys: "shift+o", label: "Open in new window", group: "Navigate", when: canOpenWindow, run: () => void openThreadWindow(getUi().selected!) },
    { id: "nav.back", keys: "escape", label: "Back", group: "Navigate", when: () => noOverlay() && getUi().threadOpen, run: goBack },
    // ---- Multi-select (list and Floe)
    { id: "select.toggle", keys: "x", label: "Select conversation", group: "Select", when: () => inList() && getUi().selected !== null, run: () => toggleSelect(getUi().selected!) },
    { id: "select.range", keys: "shift+x", label: "Select range to here", group: "Select", when: () => inList() && getUi().selected !== null, run: () => selectRange(getUi().selected!) },
    { id: "select.all", keys: "mod+a", label: "Select all loaded", group: "Select", when: () => inList() && list.get().items.length > 0, run: selectAllLoaded },
    { id: "select.clear", keys: "escape", label: "Clear selection", group: "Select", when: () => inList() && hasMultiSelection(), run: clearSelection },
    { id: "nav.first", keys: "shift+k", label: "First conversation", group: "Navigate", hidden: true, when: inList, run: () => selectIndex(0) },
    { id: "nav.last", keys: "shift+j", label: "Last loaded conversation", group: "Navigate", hidden: true, when: inList, run: () => selectIndex(list.get().items.length - 1) },
    { id: "msg.next", keys: "n", label: "Next message", group: "Navigate", when: inThread, run: () => threadNav.next() },
    { id: "msg.prev", keys: "p", label: "Previous message", group: "Navigate", when: inThread, run: () => threadNav.prev() },
    { id: "msg.expand", keys: "o", label: "Expand all messages", group: "Navigate", when: inThread, run: () => threadNav.expandAll() },
    { id: "ctx.toggle", keys: "i", label: "Toggle context panel", group: "Navigate", when: () => noOverlay() && getUi().threadOpen, run: () => setUi((s) => ({ contextPanel: !s.contextPanel })) },
    { id: "go.inbox", keys: "g i", label: "Go to Inbox", group: "Go to", when: noOverlay, run: () => goTo({ kind: "inbox" }) },
    { id: "go.replyLater", keys: "g y", label: "Go to Reply Later", group: "Go to", when: noOverlay, run: () => goTo({ kind: "replyLater" }) },
    { id: "go.followUp", keys: "g f", label: "Go to Follow up", group: "Go to", when: noOverlay, run: () => goTo({ kind: "followUp" }) },
    { id: "go.starred", keys: "g s", label: "Go to Starred", group: "Go to", when: noOverlay, run: () => goTo({ kind: "starred" }) },
    { id: "go.snoozed", keys: "g h", label: "Go to Snoozed", group: "Go to", when: noOverlay, run: () => goTo({ kind: "snoozed" }) },
    { id: "go.sent", keys: "g t", label: "Go to Sent", group: "Go to", when: noOverlay, run: () => goTo({ kind: "sent" }) },
    { id: "go.drafts", keys: "g d", label: "Go to Drafts", group: "Go to", when: noOverlay, run: () => goTo({ kind: "drafts" }) },
    { id: "go.done", keys: "g e", label: "Go to Done", group: "Go to", when: noOverlay, run: () => goTo({ kind: "done" }) },
    { id: "go.all", keys: "g a", label: "Go to All mail", group: "Go to", when: noOverlay, run: () => goTo({ kind: "all" }) },
    { id: "go.trash", keys: "g #", label: "Go to Trash", group: "Go to", when: noOverlay, run: () => goTo({ kind: "trash" }) },
    { id: "go.calendar", keys: "g c", label: "Go to Calendar", group: "Go to", when: noOverlay, run: goToCalendar },
    // A filter on the current view, not a view: G then U flips it back.
    { id: "list.unread", keys: "g u", label: "Only unread (on / off)", group: "Go to", when: onMail, run: toggleUnreadOnly },

    // ---- Triage
    // Triage keys act on the multi-selection when there is one (see targets()).
    { id: "triage.done", keys: "e", label: "Mark done (archive)", group: "Triage", when: hasSelection, run: () => onTargets(archive) },
    { id: "triage.trash", keys: "#", label: "Move to Trash", group: "Triage", when: hasSelection, run: () => onTargets(trash) },
    { id: "triage.star", keys: "s", label: "Star / unstar", group: "Triage", when: hasSelection, run: () => onTargets(toggleStar) },
    { id: "triage.read", keys: "u", label: "Toggle read / unread", group: "Triage", when: hasSelection, run: () => onTargets(toggleRead) },
    { id: "triage.unread", keys: "shift+u", label: "Mark unread", group: "Triage", when: hasSelection, run: () => onTargets(markUnreadAndSay) },
    // Label as… where the accounts have labels (Gmail, Microsoft); Move to… where
    // labels are folders (IMAP, Microsoft). On a folder-only account L moves too.
    { id: "triage.label", keys: "l", label: "Label…", group: "Triage", when: () => hasSelection() && targetAbilities().label, run: openLabelOrMove },
    { id: "triage.move", keys: "v", label: "Move to…", group: "Triage", when: () => hasSelection() && targetAbilities().move, run: () => setUi({ overlay: "move" }) },
    {
      id: "triage.move.l",
      keys: "l",
      label: "Move to…",
      group: "Triage",
      hidden: true,
      when: () => {
        if (!hasSelection()) return false;
        const can = targetAbilities();
        return !can.label && can.move;
      },
      run: openLabelOrMove,
    },
    // The open conversation's newest message with an unsubscribe option (Settings → Privacy can hide it).
    { id: "triage.unsubscribe", keys: "mod+u", label: "Unsubscribe", group: "Triage", when: () => noOverlay() && currentUnsubscribeTarget() !== null, run: () => void startUnsubscribe() },
    { id: "triage.snooze", keys: "h", label: "Snooze…", group: "Triage", when: hasSelection, run: openSnooze },
    // Out of the inbox, read, into Reply Later; again = back to the inbox.
    { id: "triage.replyLater", keys: "y", label: "Reply later", group: "Triage", when: hasSelection, run: () => onTargets(toggleReplyLater) },
    // The cursor's conversation as plain text (for an AI chat, a note); features/thread/copy.ts.
    {
      id: "thread.copy",
      keys: COPY_CONVERSATION_KEYS,
      label: "Copy conversation as text",
      group: "Triage",
      when: () => noOverlay() && (getUi().surface === "mail" || getUi().threadOpen) && getUi().selected !== null,
      run: () => copyConversation(),
    },
    { id: "triage.undo", keys: "z", label: "Undo", group: "Triage", when: noOverlay, run: undo },
    { id: "triage.undo.mod", keys: "mod+z", label: "Undo", group: "Triage", hidden: true, when: noOverlay, run: undo },

    // ---- Compose
    { id: "compose.new", keys: "c", label: "New message", group: "Compose", when: noOverlay, run: () => openCompose("new") },
    // ⌥⌘N like File → New Message in New Window (⇧⌘N is Check for New Mail).
    { id: "compose.newWindow", keys: "mod+alt+n", label: "New message in new window", group: "Compose", when: noOverlay, run: () => void openComposeWindow() },
    { id: "compose.reply", keys: "r", label: "Reply", group: "Compose", when: hasSelection, run: () => openCompose("reply") },
    { id: "compose.replyAll", keys: "a", label: "Reply all", group: "Compose", when: hasSelection, run: () => openCompose("replyAll") },
    { id: "compose.forward", keys: "f", label: "Forward", group: "Compose", when: hasSelection, run: () => openCompose("forward") },

    // ---- Search
    { id: "search.open", keys: "/", label: "Search", group: "Search", when: noOverlay, run: () => setUi({ overlay: "search", searchPrefill: null }) },
    // ⌘F too (Edit ▸ Find carries the same accelerator; the page claims the
    // key first, so it never fires twice). Not in text fields.
    { id: "search.open.find", keys: "mod+f", label: "Search", group: "Search", when: noOverlay, run: () => setUi({ overlay: "search", searchPrefill: null }) },
    {
      id: "command.open",
      keys: "mod+k",
      label: "Command palette",
      group: "Search",
      allowInInput: true,
      when: () => getUi().overlay !== "compose",
      run: () => setUi((s: UiState) => ({ overlay: s.overlay === "command" ? null : "command" })),
    },

    // ---- App
    { id: "app.shortcuts", keys: "?", label: "Keyboard shortcuts", group: "App", when: () => getUi().overlay === null || getUi().overlay === "shortcuts", run: () => setUi((s) => ({ overlay: s.overlay === "shortcuts" ? null : "shortcuts" })) },
    { id: "app.sidebar", keys: "mod+\\", label: "Show / hide sidebar", group: "App", when: noOverlay, run: toggleSidebar },
    { id: "app.sidebar.bracket", keys: "[", label: "Show / hide sidebar", group: "App", hidden: true, when: noOverlay, run: toggleSidebar },
    { id: "app.theme", keys: "t", label: "Switch theme", group: "App", when: noOverlay, run: toggleTheme },
    { id: "app.sync", keys: "shift+r", label: "Sync now", group: "App", when: noOverlay, run: startRefresh },
    { id: "acct.all", keys: "alt+0", label: "All accounts", group: "App", when: noOverlay, run: () => switchProfile(null) },
  ];
  if (scope === "thread") return registerShortcuts(all.filter((s) => THREAD_WINDOW_KEYS.has(s.id)));
  const unregStatic = registerShortcuts(all);

  // ⌥1–⌥9 switch account; re-registered when accounts change.
  let unregAccounts = () => {};
  const syncAccountKeys = () => {
    unregAccounts();
    unregAccounts = registerShortcuts(
      meta.get().accounts.slice(0, 9).map((a, i) => ({
        id: `acct.${i + 1}`,
        keys: `alt+${i + 1}`,
        label: `Switch to ${accountName(a, meta.get().accounts)}`,
        group: "App",
        when: noOverlay,
        run: () => switchAccount(a.id),
      })),
    );
  };
  // ⌃1–⌃9 switch profile, in Settings order, and ⌃0 leaves it (listed only
  // once there are profiles); re-registered when profiles change.
  let unregProfiles = () => {};
  let profileSig = "";
  const syncProfileKeys = () => {
    const profiles = getProfiles().slice(0, 9);
    const sig = profiles.map((p) => `${p.id}\u0000${p.name}`).join("\u0001");
    if (sig === profileSig) return;
    profileSig = sig;
    unregProfiles();
    const keys: Shortcut[] = profiles.map((p, i) => ({
      id: `profile.${i + 1}`,
      keys: profileKeys(i)!,
      label: `Switch to profile: ${p.name}`,
      group: "Profiles",
      when: noOverlay,
      run: () => switchProfile(p.id),
    }));
    if (profiles.length > 0)
      keys.push({ id: "profile.all", keys: `${PROFILE_MOD}+0`, label: "All accounts", group: "Profiles", when: noOverlay, run: () => switchProfile(null) });
    unregProfiles = registerShortcuts(keys);
  };

  let lastAccounts = meta.get().accounts;
  syncAccountKeys();
  syncProfileKeys();
  const unsubMeta = meta.subscribe(() => {
    if (meta.get().accounts !== lastAccounts) {
      lastAccounts = meta.get().accounts;
      syncAccountKeys();
    }
  });
  const unsubSettings = subscribeSettings(syncProfileKeys);

  return () => {
    unregStatic();
    unregAccounts();
    unregProfiles();
    unsubMeta();
    unsubSettings();
  };
}
