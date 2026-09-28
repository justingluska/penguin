// "Show keyboard shortcut hints" (Settings → General), off by default. Off
// hides every key hint in the UI through one attribute,
// <html data-shortcut-hints="off">, which the CSS in styles/app.css keys on:
// .kbd caps, rows that are only hints, and footers of hints. The "?" cheat
// sheet opts out with .keys-always.
// Tooltips drop their key part through keyTip()/useKeyTip(). The shortcuts
// themselves keep working either way.
import { useSyncExternalStore } from "react";

const CACHE_KEY = "penguin.shortcutHints";
let on = false;
const subs = new Set<() => void>();

export function shortcutHintsOn(): boolean {
  return on;
}

export function applyShortcutHints(next: boolean) {
  if (typeof document !== "undefined") {
    if (next) delete document.documentElement.dataset.shortcutHints;
    else document.documentElement.dataset.shortcutHints = "off";
  }
  try {
    localStorage.setItem(CACHE_KEY, next ? "on" : "off");
  } catch {
    // No storage (private window): the setting still applies once loaded.
  }
  if (next === on) return;
  on = next;
  subs.forEach((f) => f());
}

/** Before first paint: last run's choice, so hints don't flash on while settings load. */
export function applyCachedShortcutHints() {
  let cached: string | null = null;
  try {
    cached = localStorage.getItem(CACHE_KEY);
  } catch {
    // Unreadable storage: hints stay off (the default) until settings load.
  }
  applyShortcutHints(cached === "on");
}

/** "Previous (K)" with hints on, "Previous" with them off. */
export function keyTip(label: string, keys: string): string {
  return on ? `${label} (${keys})` : label;
}

/** keyTip for components: re-renders when the setting changes. */
export function useKeyTip(): (label: string, keys: string) => string {
  useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => on,
  );
  return keyTip;
}
