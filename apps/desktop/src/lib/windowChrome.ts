// This window's own chrome: its title (conversation and compose windows are
// titled with the subject: the tab bar, Window menu and Mission Control show
// it), and bringing the main window forward. The window API is loaded on
// first use, like app/menu.ts does, so the launch bundle doesn't carry it.

/** Bring this (main) window up: it may be hidden with ⌘W, minimized, or a background tab. */
export async function focusMainWindow(): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) {
    window.focus();
    return;
  }
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const w = getCurrentWindow();
  await Promise.all([w.show(), w.unminimize()]).catch((e) => console.warn("penguin: could not show the window", e));
  await w.setFocus().catch((e) => console.warn("penguin: could not focus the window", e));
}
const MAX_TITLE = 200;

let last = "";

export function setThisWindowTitle(title: string) {
  const t = title.replace(/\s+/g, " ").trim().slice(0, MAX_TITLE) || "Penguin";
  if (t === last) return;
  last = t;
  if (!("__TAURI_INTERNALS__" in window)) {
    document.title = t;
    return;
  }
  void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow()
      .setTitle(t)
      .catch((e) => console.warn("penguin: could not set the window title", e)),
  );
}
