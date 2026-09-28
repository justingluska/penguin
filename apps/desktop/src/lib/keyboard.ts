// Global keyboard shortcut registry. Every action in the app registers here so
// the ⌘K palette and the "?" cheat sheet can list it with its key.
// OWNER: inbox/thread agent (implementation); others only call the API.
//
// Key syntax: "e", "shift+u", "mod+k" (⌘ on mac, Ctrl elsewhere), "alt+1",
// "enter", "escape", "arrowdown", and sequences "g i" (press g, then i).
// Symbols are written as the character they produce ("?", "#", "/"), not as
// shift+key. Letters are case-insensitive; use "shift+x" for the capital.
//
// Shortcuts are ignored while typing in inputs/textareas/contenteditable
// unless `allowInInput` is true.
//
// Conflicts: when several shortcuts match the same key, only those whose
// `when()` passes are considered, and a shortcut with a `when` beats one
// without (context-specific wins over global). A `fallback` shortcut loses to
// every other live one, so it runs only when nothing else claims the key
// (Esc leaving Floe mode after popovers, selections and threads have had
// their turn). A key that starts a sequence ("g") is never also a
// single-key shortcut.

export interface Shortcut {
  id: string;
  keys: string;
  label: string;
  group?: "Navigate" | "Triage" | "Compose" | "Search" | "App" | string;
  /** Return false to make the shortcut inactive in the current context. */
  when?: () => boolean;
  allowInInput?: boolean;
  /** Alias of another entry (e.g. "enter" for "o"): works, but not listed. */
  hidden?: boolean;
  /** Runs only when no other live shortcut has the same key (see Conflicts above). */
  fallback?: boolean;
  run: () => void;
}

const registry = new Map<string, Shortcut>();
const subs = new Set<() => void>();

export function registerShortcuts(list: Shortcut[]): () => void {
  list.forEach((s) => registry.set(s.id, s));
  subs.forEach((f) => f());
  return () => {
    list.forEach((s) => {
      if (registry.get(s.id) === s) registry.delete(s.id);
    });
    subs.forEach((f) => f());
  };
}

export function getShortcuts(): Shortcut[] {
  return [...registry.values()];
}

export function subscribeShortcuts(cb: () => void): () => void {
  subs.add(cb);
  return () => subs.delete(cb);
}

const keyRuns = new Set<(s: Shortcut) => void>();

/** Called after a shortcut runs from the keyboard (not from ⌘K, a menu or a click): the shortcut coach counts it. */
export function onShortcutKey(cb: (s: Shortcut) => void): () => void {
  keyRuns.add(cb);
  return () => keyRuns.delete(cb);
}

function runFromKey(s: Shortcut) {
  s.run();
  keyRuns.forEach((f) => f(s));
}

export const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform || navigator.userAgent);

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------
const GLYPH: Record<string, string> = {
  mod: isMac ? "⌘" : "Ctrl",
  shift: "⇧",
  alt: isMac ? "⌥" : "Alt",
  ctrl: isMac ? "⌃" : "Ctrl",
  enter: "↵",
  escape: "Esc",
  tab: "⇥",
  space: "Space",
  backspace: "⌫",
  delete: "⌦",
  arrowup: "↑",
  arrowdown: "↓",
  arrowleft: "←",
  arrowright: "→",
};

/**
 * Split a key spec into display chords: "g i" → [["G"], ["I"]],
 * "mod+k" → [["⌘K"]], "shift+u" → [["⇧", "U"]].
 * Each inner array renders as adjacent <kbd>s; chords are joined by "then".
 */
export function formatKeys(keys: string): string[][] {
  return keys.split(" ").map((chord) => {
    const parts = chord.split("+").filter(Boolean);
    if (chord.endsWith("++")) parts.push("+");
    const out = parts.map((p) => GLYPH[p] ?? (p.length === 1 ? p.toUpperCase() : p[0].toUpperCase() + p.slice(1)));
    // "⌘K" reads better as one key cap on mac; keep ⇧ separate like the mockups.
    if (parts[0] === "mod" && isMac && out.length === 2) return [out[0] + out[1]];
    return out;
  });
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------
const SEQUENCE_TIMEOUT = 1200;
let pending: string | null = null;
let pendingTimer: ReturnType<typeof setTimeout> | null = null;
const pendingSubs = new Set<(p: string | null) => void>();

function setPending(p: string | null) {
  pending = p;
  if (pendingTimer) clearTimeout(pendingTimer);
  pendingTimer = p ? setTimeout(() => setPending(null), SEQUENCE_TIMEOUT) : null;
  pendingSubs.forEach((f) => f(p));
}

/** Observe the half-typed sequence (e.g. "g") to show a hint in the UI. */
export function subscribePendingSequence(cb: (p: string | null) => void): () => void {
  pendingSubs.add(cb);
  return () => pendingSubs.delete(cb);
}

const NAMED: Record<string, string> = {
  " ": "space",
  Enter: "enter",
  Escape: "escape",
  Esc: "escape",
  Tab: "tab",
  Backspace: "backspace",
  Delete: "delete",
  ArrowUp: "arrowup",
  ArrowDown: "arrowdown",
  ArrowLeft: "arrowleft",
  ArrowRight: "arrowright",
};

/** Normalize a keydown into a chord string in the registry's syntax. */
export function chordOf(e: KeyboardEvent): string | null {
  if (e.key === "Shift" || e.key === "Meta" || e.key === "Control" || e.key === "Alt" || e.key === "Dead") return null;
  const mod = isMac ? e.metaKey : e.ctrlKey;
  const ctrl = isMac && e.ctrlKey;
  let base: string;
  let usesShift = false;
  if (NAMED[e.key]) {
    base = NAMED[e.key];
    usesShift = e.shiftKey;
  } else if (/^Key[A-Z]$/.test(e.code) && (e.altKey || mod || ctrl || /^[a-z]$/i.test(e.key))) {
    // Letters: use the physical key so ⌥/⌘ combos and caps lock behave.
    base = e.code.slice(3).toLowerCase();
    usesShift = e.shiftKey;
  } else if (/^Digit\d$/.test(e.code) && (e.altKey || mod || ctrl)) {
    base = e.code.slice(5);
    usesShift = e.shiftKey;
  } else if (e.key.length === 1) {
    // Symbols: the produced character already encodes shift ("?", "#").
    base = e.key.toLowerCase();
  } else {
    base = e.key.toLowerCase();
  }
  let chord = "";
  if (mod) chord += "mod+";
  if (ctrl) chord += "ctrl+";
  if (e.altKey) chord += "alt+";
  if (usesShift) chord += "shift+";
  return chord + base;
}

function isTypingTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  if (t.isContentEditable) return true;
  const tag = t.tagName;
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag === "INPUT") {
    const type = (t as HTMLInputElement).type;
    return !["checkbox", "radio", "button", "submit", "reset", "range", "color"].includes(type);
  }
  return false;
}

function active(s: Shortcut, typing: boolean): boolean {
  if (typing && !s.allowInInput) return false;
  return s.when ? s.when() : true;
}

/**
 * Which of the live shortcuts for one key runs: a fallback only when it is
 * alone; otherwise context-specific (has `when`) beats global, and later
 * registration wins ties. Exported for tests.
 */
export function pickShortcut(candidates: Shortcut[]): Shortcut | undefined {
  const primary = candidates.filter((s) => !s.fallback);
  const pool = primary.length > 0 ? primary : candidates;
  let pick: Shortcut | undefined;
  for (const s of pool) {
    if (!pick || (s.when && !pick.when) || !!s.when === !!pick.when) pick = s;
  }
  return pick;
}

/** Install the global keydown listener once at app start. */
export function installKeyboard(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (e.isComposing || e.defaultPrevented) return;
    const chord = chordOf(e);
    if (!chord) return;
    const typing = isTypingTarget(e.target);
    const all = [...registry.values()];

    if (pending) {
      const seq = `${pending} ${chord}`;
      setPending(null);
      const hit = pickShortcut(all.filter((s) => s.keys === seq && active(s, typing)));
      if (hit) {
        e.preventDefault();
        runFromKey(hit);
        return;
      }
      // Fall through: treat this key on its own.
    }

    // Does this key start a sequence that is live right now?
    if (!typing || e.altKey || e.metaKey || e.ctrlKey) {
      const starts = all.some((s) => s.keys.startsWith(chord + " ") && active(s, typing));
      if (starts) {
        e.preventDefault();
        setPending(chord);
        return;
      }
    }

    const hit = pickShortcut(all.filter((s) => s.keys === chord && active(s, typing)));
    if (hit) {
      e.preventDefault();
      runFromKey(hit);
    }
  };
  // Bubble phase on window: a component that owns a key (an overlay's input,
  // a menu) handles it first and calls preventDefault() to claim it.
  window.addEventListener("keydown", onKey);
  return () => {
    window.removeEventListener("keydown", onKey);
    setPending(null);
  };
}
