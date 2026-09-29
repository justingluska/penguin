// App settings, persisted Rust-side in <config dir>/settings.json.
// OWNER: settings agent.
//
// One cached copy for the whole UI: loaded once at startup, kept current by
// `penguin://settings-changed` (fired by update_settings, from any window),
// and read synchronously by components through useSetting(). Until the first
// load resolves, reads return the defaults below — the same defaults serde
// fills in on the Rust side (src-tauri/src/settings.rs).
//
// Side effects that belong to settings live here too: the theme is pushed into
// ui.ts, the density onto <html data-density> and the list style onto
// <html data-list-style> (lib/listStyle.ts), so every screen follows the
// stored choice without importing the Settings page.
import { useSyncExternalStore } from "react";
import { api, onSettingsChanged } from "./api";
import type { Settings, SettingsPatch } from "./types";
import { getUi, setUi } from "./ui";
import { refetchCachedThreads } from "../app/store";
import {
  applyAccentAndCorners,
  applyCachedAccentAndCorners,
  applyCachedDarkShade,
  applyCachedSidebarTheme,
  applyDarkShade,
  applySidebarTheme,
} from "./themes";
import { applyCachedShortcutHints, applyShortcutHints } from "./shortcutHints";
import { applyCachedListStyle, applyListStyle } from "./listStyle";

export const DEFAULT_SETTINGS: Settings = {
  theme: "system",
  density: "compact",
  listStyle: "quiet",
  inboxTabs: false,
  inboxSplits: [
    { id: "important", name: "Important", query: "is:important -is:newsletter -has:invite", hideWhenEmpty: false },
    { id: "calendar", name: "Calendar", query: "has:invite OR from:calendar-notification@google.com OR from:@calendly.com", hideWhenEmpty: false },
    { id: "news", name: "News", query: "is:newsletter", hideWhenEmpty: false },
  ],
  getToZero: true,
  zeroCelebration: true,
  remoteImages: "ask",
  trustedImageSenders: [],
  blockTrackingPixels: true,
  stripLinkTracking: false,
  requestReadReceipts: false,
  profiles: [],
  hiddenFromAll: [],
  accountOrder: [],
  swipeRight: "toggleRead",
  swipeLeft: "archive",
  swipeLeftLong: "trash",
  sidebarTheme: "graphite",
  matchAccent: false,
  darkShade: "black",
  accentColor: "blue",
  corners: "rounded",
  darkEmailBodies: false,
  floeMode: false,
  composeFont: "inter",
  composeFontSize: 15,
  sidebarTextSize: 0,
  followUpDays: 3,
  // Filled from settings.json on load; the backend supplies the starters.
  snippets: [],
  undoSendSeconds: 10,
  sendLaterHour: 8,
  instantReplies: { enabled: true, replies: ["Sounds good, thanks!", "Thanks, got it.", "Let me check and get back to you."], aiSuggestions: false },
  writeWithAi: true,
  checkSpelling: true,
  checkGrammar: false,
  signatures: [],
  signatureDefaults: {},
  signatureInsert: { newMessages: true, replies: true, forwards: true },
  signatureSeparator: false,
  lockReplyAccount: true,
  gmailUnitsPerMin: 6000,
  syncWindowMonths: 6,
  olderMail: "headers",
  mcp: { access: "off", enabled: false, sendDelaySeconds: 60, sendKnownOnly: true },
  showShortcutHints: false,
  shortcutCoach: true,
  unsubscribeButton: true,
  semanticSearch: true,
  summaries: true,
  askWithAi: true,
  senderPhotos: { contacts: false, bimi: true, favicons: true, gravatar: false },
  avatarPlacement: "both",
  me: { name: "", title: "", company: "", signOff: "", photo: null },
  calendar: { pastMonths: 24, futureMonths: 12, nextUp: true, connectOnSignIn: true },
  notifications: { enabled: false, accounts: {}, knownSendersOnly: false },
  pendingSetups: [],
  smartViews: { shown: [], counts: [], custom: [] },
  welcomeCompleted: false,
};

let current: Settings = DEFAULT_SETTINGS;
let loaded: Promise<Settings> | null = null;
const subs = new Set<() => void>();

function set(next: Settings) {
  const prev = current;
  current = { ...DEFAULT_SETTINGS, ...next };
  applySideEffects(current);
  // get_thread renders remote images, trackers and links per these settings: cached renders are stale.
  if (
    prev.remoteImages !== current.remoteImages ||
    prev.blockTrackingPixels !== current.blockTrackingPixels ||
    prev.stripLinkTracking !== current.stripLinkTracking ||
    prev.trustedImageSenders.join("\n") !== current.trustedImageSenders.join("\n")
  )
    refetchCachedThreads();
  subs.forEach((f) => f());
}

function subscribe(cb: () => void): () => void {
  ensureLoaded();
  subs.add(cb);
  return () => subs.delete(cb);
}

function ensureLoaded(): Promise<Settings> {
  if (!loaded) {
    void onSettingsChanged(set);
    loaded = api.getSettings().then(
      (s) => {
        set(s);
        return current;
      },
      (e) => {
        // Unreadable settings fall back to defaults (the backend does the
        // same); the next update_settings writes a fresh file.
        console.warn("penguin: could not load settings", e);
        set(DEFAULT_SETTINGS);
        return current;
      },
    );
  }
  return loaded;
}

/** The settings as last loaded (defaults before the first load resolves). */
export function currentSettings(): Settings {
  return current;
}

/** Resolves once settings have been read from disk. */
export function getSettings(): Promise<Settings> {
  return ensureLoaded();
}

/** Notified after every change (for non-React consumers such as the mail store). */
export function subscribeSettings(cb: () => void): () => void {
  return subscribe(cb);
}

/** Subscribe a component to one setting. */
export function useSetting<K extends keyof Settings>(key: K): Settings[K] {
  return useSyncExternalStore(subscribe, () => current[key]);
}

/** Subscribe a component to all settings. */
export function useSettings(): Settings {
  return useSyncExternalStore(subscribe, () => current);
}

/**
 * Apply `patch` immediately (optimistic), then persist. On failure the
 * previous settings come back and the promise rejects.
 */
export async function updateSettings(patch: SettingsPatch): Promise<Settings> {
  await ensureLoaded();
  const before = current;
  // `me` and `mcp` are merged field by field, like the backend does.
  const mcp = { ...current.mcp, ...patch.mcp };
  set({ ...current, ...patch, me: { ...current.me, ...patch.me }, mcp: { ...mcp, enabled: mcp.access !== "off" } });
  try {
    const saved = await api.updateSettings(patch);
    set(saved);
    return saved;
  } catch (e) {
    set(before);
    throw e;
  }
}

/** Remote images for one sender load without asking (remoteImages = "ask"). */
export function isTrustedImageSender(email: string): boolean {
  return current.trustedImageSenders.includes(email.trim().toLowerCase());
}

export function setTrustedImageSender(email: string, trusted: boolean): Promise<Settings> {
  const addr = email.trim().toLowerCase();
  const rest = current.trustedImageSenders.filter((e) => e !== addr);
  return updateSettings({ trustedImageSenders: trusted ? [...rest, addr].sort() : rest });
}

// ---------------------------------------------------------------------------
// Side effects
// ---------------------------------------------------------------------------
const systemDark = () =>
  typeof window === "undefined" || !window.matchMedia ? true : window.matchMedia("(prefers-color-scheme: dark)").matches;

let appliedTheme: Settings["theme"] | null = null;

const SIDEBAR_TEXT_KEY = "penguin.sidebarText";

/** `<html data-sidebar-text>`: unset at the default, else −2…2 (styles/app.css). */
export function applySidebarText(step: number) {
  const n = Math.max(-2, Math.min(2, Math.round(step || 0)));
  if (typeof document !== "undefined") {
    if (n === 0) delete document.documentElement.dataset.sidebarText;
    else document.documentElement.dataset.sidebarText = String(n);
  }
  try {
    localStorage.setItem(SIDEBAR_TEXT_KEY, String(n));
  } catch {
    // No storage: applied once settings load.
  }
}

// First paint uses the last run's size, so the sidebar doesn't jump when settings arrive.
try {
  const cached = typeof localStorage === "undefined" ? null : localStorage.getItem(SIDEBAR_TEXT_KEY);
  if (cached) applySidebarText(Number(cached));
} catch {
  // No storage.
}

function applySideEffects(s: Settings) {
  if (typeof document !== "undefined") document.documentElement.dataset.density = s.density;
  applyListStyle(s.listStyle);
  applySidebarTheme(s.sidebarTheme, s.matchAccent);
  applyDarkShade(s.darkShade);
  applyAccentAndCorners(s.accentColor, s.corners);
  applySidebarText(s.sidebarTextSize);
  applyShortcutHints(s.showShortcutHints);
  // Only push the theme when the stored choice changes, so a transient T
  // toggle isn't undone by an unrelated settings update.
  if (s.theme === appliedTheme) return;
  appliedTheme = s.theme;
  if (s.theme === "system") {
    const theme = systemDark() ? "dark" : "light";
    if (getUi().theme !== theme || getUi().themeSource !== "system") setUi({ theme, themeSource: "system" });
  } else if (getUi().theme !== s.theme || getUi().themeSource !== "user") {
    setUi({ theme: s.theme, themeSource: "user" });
  }
}

// Load at startup so the stored theme and density apply before the user
// opens anything. The sidebar theme from last run goes on first, so there's
// no flash of the default while settings load.
if (typeof window !== "undefined") {
  applyCachedSidebarTheme();
  applyCachedDarkShade();
  applyCachedAccentAndCorners();
  applyCachedShortcutHints();
  applyCachedListStyle();
  void ensureLoaded();
}
