// The native menu bar's UI side (src-tauri/src/app_menu.rs has the menus and
// the full story). A chosen item arrives as `penguin://menu {id}`: ids from
// the shortcut registry run that shortcut, behind the same when() gate as
// its key, and the few menu-only ids are handled below. In the other
// direction, the UI state that decides which items are enabled is pushed
// with set_menu_context whenever it changes.
//
// A ⌘-key reaches the page first; the page's handler claims the keys it has
// (preventDefault), so a menu accelerator that the page also binds (⌘\, ⇧⌘F,
// ⌘,) fires here only when the page didn't act on it — never twice.
import { api, asCommandError, onMenu } from "../lib/api";
import { getShortcuts } from "../lib/keyboard";
import { getUi, subscribeUi } from "../lib/ui";
import { getLayout, subscribeLayout } from "../lib/layout";
import { currentSettings, subscribeSettings, updateSettings } from "../lib/settings";
import type { MenuContext, SettingsPatch } from "../lib/types";
import { toast } from "../components/Toast";
import { isSettingsOpen, openSettings, subscribeSettingsOpen } from "../features/settings/state";
import { isStarred, isUnread } from "./actions";
import { list, meta } from "./store";
import { debugInfo } from "../lib/debugInfo";
import { isMainWindow } from "../lib/windowBus";

/**
 * Penguin → Copy Debug Info. A native menu choice isn't a click in the page,
 * so WebKit won't let the page write the clipboard; the backend does it.
 */
async function copyDebugInfo() {
  try {
    await api.copyText(await debugInfo());
    toast({ message: "Debug info copied" });
  } catch (e) {
    toast({ tone: "error", message: "Couldn't copy debug info", detail: asCommandError(e).message });
  }
}

function save(patch: SettingsPatch) {
  updateSettings(patch).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }));
}

const noModal = () => {
  const o = getUi().overlay;
  return !isSettingsOpen() && (o === null || o === "command");
};

/**
 * Something ⌘W should close before the window: an overlay (a compose
 * window's composer too: closing it closes the window), Settings, a dialog,
 * an open thread (in the main window; a conversation window is its thread).
 */
function somethingOpen(): boolean {
  return (
    getUi().overlay !== null ||
    isSettingsOpen() ||
    (isMainWindow && getUi().threadOpen) ||
    document.querySelector('[aria-modal="true"], [role="menu"]') !== null
  );
}

/** Whether a layer was open when ⌘W was pressed (set in the capture phase, before any handler closes it). */
let openAtCmdW: { at: number; open: boolean } | null = null;

/**
 * ⌘W: close the frontmost layer the way Esc does (every overlay, dialog and
 * popover already handles Esc), else hide the window (macOS; the app keeps
 * syncing and the Dock icon brings it back).
 */
function closeFrontmost() {
  // A ⌘W keypress reaches the page before the menu, and a context menu closes
  // itself on any ⌘-key, so by now it may be gone: that keypress was spent
  // closing it, not the window.
  const layerAtKey = openAtCmdW !== null && performance.now() - openAtCmdW.at < 1000 && openAtCmdW.open;
  openAtCmdW = null;
  if (layerAtKey && !somethingOpen()) return;
  if (somethingOpen()) {
    const target = document.activeElement ?? document.body;
    target.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true, cancelable: true }));
    return;
  }
  // A conversation or compose window closes for real (windowShell.tsx
  // finishes its work first); the main window only hides.
  if (!isMainWindow) {
    closeThisWindow();
    return;
  }
  if (!("__TAURI_INTERNALS__" in window)) return;
  void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow()
      .hide()
      .catch((e) => console.warn("penguin: could not hide the window", e)),
  );
}

/**
 * Close this (conversation or compose) window the way its red button does:
 * the close is requested, and windowShell.tsx's close handler saves and hands
 * over what's left before the window goes.
 */
export function closeThisWindow() {
  if (!("__TAURI_INTERNALS__" in window)) {
    window.dispatchEvent(new Event(CLOSE_REQUEST));
    return;
  }
  void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow()
      .close()
      .catch((e) => console.warn("penguin: could not close the window", e)),
  );
}

/** In a browser there is no close request to intercept: windowShell.tsx listens for this instead. */
export const CLOSE_REQUEST = "penguin:close-request";

const MENU_ONLY: Record<string, () => void> = {
  "window.close": closeFrontmost,
  "app.copyDebug": () => void copyDebugInfo(),
  "app.accounts": () => noModal() && openSettings("accounts"),
  "view.theme.system": () => save({ theme: "system" }),
  "view.theme.light": () => save({ theme: "light" }),
  "view.theme.dark": () => save({ theme: "dark" }),
  "view.density.compact": () => save({ density: "compact" }),
  "view.density.comfortable": () => save({ density: "comfortable" }),
  "view.hints": () => save({ showShortcutHints: !currentSettings().showShortcutHints }),
};

/** Run a menu item by id (exported for tests). */
export function runMenuItem(id: string) {
  const direct = MENU_ONLY[id];
  if (direct) return direct();
  const s = getShortcuts().find((x) => x.id === id);
  if (!s) {
    console.warn(`penguin: no action for menu item ${id}`);
    return;
  }
  if (s.when && !s.when()) return;
  s.run();
}

export function menuContext(): MenuContext {
  const ui = getUi();
  const sel = ui.selected;
  return {
    mail: meta.get().accounts.length > 0,
    blocked: !noModal() || ui.overlay === "command",
    selection: sel !== null,
    selectionUnread: sel !== null && isUnread(sel),
    selectionStarred: sel !== null && isStarred(sel),
    sidebarVisible: !getLayout().sidebarCollapsed,
    floe: currentSettings().floeMode,
    unreadOnly: ui.unreadOnly,
    detached: !isMainWindow,
  };
}

/** Listen for menu items and keep their enabled state current. Registered once by App. */
export function installMenu(): () => void {
  const off = onMenu((e) => runMenuItem(e.id));
  const onKey = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.code === "KeyW") openAtCmdW = { at: performance.now(), open: somethingOpen() };
  };
  window.addEventListener("keydown", onKey, true);
  let last = "";
  let queued = false;
  const push = () => {
    if (queued) return;
    queued = true;
    // Coalesce a burst of store updates (a list load, a keypress) into one IPC.
    queueMicrotask(() => {
      queued = false;
      const ctx = menuContext();
      const sig = JSON.stringify(ctx);
      if (sig === last) return;
      last = sig;
      api.setMenuContext(ctx).catch((e) => console.warn("penguin: menu context", e));
    });
  };
  const unsubs = [subscribeUi(push), list.subscribe(push), meta.subscribe(push), subscribeSettings(push), subscribeLayout(push), subscribeSettingsOpen(push)];
  push();
  return () => {
    void off.then((f) => f());
    window.removeEventListener("keydown", onKey, true);
    unsubs.forEach((u) => u());
  };
}
