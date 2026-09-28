// Whether the Settings screen is showing, and the keys that open/close it.
// OWNER: settings agent. Kept out of lib/ui.ts so the screen can be added
// without reshaping the shared UI state.
import { useSyncExternalStore } from "react";
import { registerShortcuts } from "../../lib/keyboard";
import { getUi, setUi } from "../../lib/ui";

export type SettingsSection = "you" | "accounts" | "profiles" | "general" | "inbox" | "sync" | "calendar" | "compose" | "signatures" | "privacy" | "search" | "ai" | "views" | "rules" | "keyboard" | "diagnostics" | "whatsnew" | "about";

let open = false;
let focusSection: SettingsSection | null = null;
/** Bumped by every openSettings(), so an open Settings can switch to the requested page. */
let requests = 0;
const subs = new Set<() => void>();
const notify = () => subs.forEach((f) => f());

/** Open Settings at `section`'s page (null: the first page), or switch to it when Settings is already open. */
export function openSettings(section: SettingsSection | null = null) {
  focusSection = section;
  open = true;
  requests++;
  // Settings is its own modal; close any overlay (e.g. ⌘K) that opened it.
  if (getUi().overlay) setUi({ overlay: null });
  notify();
}

export function closeSettings() {
  if (!open) return;
  open = false;
  focusSection = null;
  notify();
}

export function isSettingsOpen(): boolean {
  return open;
}

/** Page to show for the latest openSettings() call (then cleared). */
export function takeFocusSection(): SettingsSection | null {
  const s = focusSection;
  focusSection = null;
  return s;
}

/** Changes on every openSettings() call; Settings takes the requested page when it does. */
export function useSettingsRequest(): number {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => requests,
  );
}

/** Notified when Settings opens or closes (for non-React consumers such as the menu bar). */
export function subscribeSettingsOpen(cb: () => void): () => void {
  subs.add(cb);
  return () => subs.delete(cb);
}

export function useSettingsOpen(): boolean {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => open,
  );
}

/** ⌘, opens Settings (also listed in the ⌘K palette). Registered once by App. */
export function registerSettingsShortcuts(): () => void {
  return registerShortcuts([
    {
      id: "app.settings",
      keys: "mod+,",
      label: "Settings",
      group: "App",
      allowInInput: true,
      when: () => !open && (getUi().overlay === null || getUi().overlay === "command"),
      run: () => openSettings(),
    },
  ]);
}
