// Shortcut coach (Settings → General): after a mouse action that has a key,
// a small hint by the pointer shows the key, e.g. "E  Mark done". The
// policy (rate limits, keys you already know) is lib/coach.ts.
//
// What counts as a mouse action with a key:
// - a click on an element marked `data-shortcut="<registry id>"` (toolbar
//   and sidebar buttons), when that shortcut is registered;
// - a context-menu choice made with the pointer on an entry that shows a
//   registered shortcut's key (components/ContextMenu.tsx → notifyMenuChoice;
//   menu-only keys like ⌘C in a text menu aren't the app's to teach).
// A click made with the keyboard (Enter or Space on a focused button,
// `detail === 0`) isn't one. Keys pressed for real are counted through
// `onShortcutKey`, so a key you use stops being hinted.
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { getShortcuts, onShortcutKey } from "../lib/keyboard";
import { useSetting } from "../lib/settings";
import { COACH, emptyCoach, learnedKeys, onMenuChoice, parseCoach, placeHint, recordHint, recordKeyUse, shouldHint, type CoachState } from "../lib/coach";
import { Keys } from "../components/Kbd";

const STORE_KEY = "penguin.shortcutCoach";

let state: CoachState | null = null;
function load(): CoachState {
  if (state) return state;
  try {
    state = parseCoach(localStorage.getItem(STORE_KEY));
  } catch {
    // No storage (private window): the coach still works for this session.
    state = emptyCoach();
  }
  return state;
}
function save(next: CoachState) {
  state = next;
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(next));
  } catch {
    // Not persisted; the in-memory state still rate-limits this session.
  }
}

/** Forget which keys you know and which hints you've seen (Settings). */
export function resetCoach() {
  save(emptyCoach());
}

/** How many keys you've learned (pressed often enough that they're no longer hinted). */
export function coachLearned(): number {
  return learnedKeys(load()).length;
}

interface Hint {
  id: number;
  keys: string;
  label: string;
  /** What to place it by: the clicked element, or the pointer. */
  rect: { left: number; top: number; width: number; height: number };
}

let hint: Hint | null = null;
let seq = 0;
let enabled = false;
const subs = new Set<() => void>();
const setHint = (h: Hint | null) => {
  hint = h;
  subs.forEach((f) => f());
};

/** Offer a hint for `keys` (a registry key spec); shown only if the policy allows. */
function offer(keys: string, label: string, rect: Hint["rect"]) {
  if (!enabled) return;
  const now = Date.now();
  const s = load();
  if (!shouldHint(s, keys, now)) return;
  save(recordHint(s, keys, now));
  setHint({ id: ++seq, keys, label, rect });
}

let pointer = { x: 0, y: 0 };

/** A context-menu entry with a registered shortcut's key, chosen with the pointer. */
function onMenu(keys: string, label: string) {
  if (!getShortcuts().some((s) => s.keys === keys)) return;
  offer(keys, label, { left: pointer.x, top: pointer.y, width: 0, height: 0 });
}

function onClick(e: MouseEvent) {
  // detail 0: Enter/Space on a focused button, not the mouse.
  if (!enabled || e.detail === 0 || !(e.target instanceof Element)) return;
  const el = e.target.closest<HTMLElement>("[data-shortcut]");
  if (!el || (el as HTMLButtonElement).disabled) return;
  const id = el.dataset.shortcut;
  const s = getShortcuts().find((x) => x.id === id);
  if (!s) return;
  const r = el.getBoundingClientRect();
  offer(s.keys, el.dataset.shortcutLabel ?? s.label, { left: r.left, top: r.top, width: r.width, height: r.height });
}

function onPointer(e: PointerEvent) {
  pointer = { x: e.clientX, y: e.clientY };
}

/** Mounted once by App. */
export function ShortcutCoach() {
  const on = useSetting("shortcutCoach");
  useEffect(() => {
    enabled = on;
    if (!on) {
      setHint(null);
      return;
    }
    // Capture: count the click even if a handler stops it bubbling.
    window.addEventListener("click", onClick, true);
    window.addEventListener("pointerdown", onPointer, true);
    const offMenu = onMenuChoice(onMenu);
    const offKeys = onShortcutKey((s) => {
      save(recordKeyUse(load(), s.keys, Date.now()));
      // Pressing the key a hint is showing for answers it.
      if (hint?.keys === s.keys) setHint(null);
    });
    return () => {
      enabled = false;
      window.removeEventListener("click", onClick, true);
      window.removeEventListener("pointerdown", onPointer, true);
      offKeys();
      offMenu();
    };
  }, [on]);

  const current = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => hint,
  );
  return current ? <Bubble key={current.id} hint={current} /> : null;
}

function Bubble({ hint: h }: { hint: Hint }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [leaving, setLeaving] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setPos(placeHint(h.rect, { width: el.offsetWidth, height: el.offsetHeight }, { width: window.innerWidth, height: window.innerHeight }));
  }, [h]);
  useEffect(() => {
    const fade = window.setTimeout(() => setLeaving(true), COACH.SHOW_MS);
    const gone = window.setTimeout(() => setHint(null), COACH.SHOW_MS + 200);
    return () => {
      window.clearTimeout(fade);
      window.clearTimeout(gone);
    };
  }, [h]);
  return (
    <div
      ref={ref}
      className={"coach-hint keys-always" + (leaving ? " is-leaving" : "")}
      role="status"
      aria-live="polite"
      style={pos ? { left: pos.left, top: pos.top } : { visibility: "hidden" }}
      onClick={() => setHint(null)}
    >
      <Keys keys={h.keys} />
      <span className="coach-label">{h.label}</span>
    </div>
  );
}
