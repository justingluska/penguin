// Demo mode at boot. main.tsx imports this first, before any module can touch
// localStorage. With the flag on (Settings → Developer → Demo mode, or ⌘K
// "Enter demo mode"), api.ts routes every command and event to the mock
// backend, and localStorage is swapped for an in-memory stand-in, so the page
// shows only the mock's fictional data and nothing real is read or changed.
// See docs/DEMO.md.
import { readDemoFlag, shadowLocalStorage, writeDemoFlag } from "./demoFlag";

function storageOf(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

/** The real localStorage, captured before demo mode shadows it. Only the flag is written through it. */
export const realStorage: Storage | null = storageOf();

function start(): boolean {
  if (!readDemoFlag(realStorage)) return false;
  if (shadowLocalStorage(window)) return true;
  // Without the stand-in, real recent searches and snippets would show among
  // the demo data. Stay out of demo mode instead of mixing the two.
  console.warn("penguin: demo mode needs to replace localStorage and couldn't; staying in normal mode");
  try {
    writeDemoFlag(realStorage, false);
  } catch {
    // Storage refused: the flag stays, and the next boot tries again.
  }
  return false;
}

/** This page runs on demo data. Fixed for the page's lifetime: switching reloads. */
export const isDemo: boolean = start();

if (isDemo) {
  // A hidden hint only (no badge, so screenshots stay clean): the page title,
  // which the overlay title bar doesn't show, and an attribute for devtools.
  document.documentElement.dataset.demo = "";
  document.title = "Penguin (demo)";
}
