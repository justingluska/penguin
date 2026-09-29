// Every key the app answers to, for the "?" sheet (pure; tests/keyCatalog.test.ts).
//
// The shortcut registry (lib/keyboard.ts) holds the main keys. Three kinds
// of keys live elsewhere, and the sheet must show them too:
// - aliases registered `hidden` so the ⌘K palette lists an action once
//   (↓ for J, ↵ for O, ⌘Z for Z, \ for Floe mode): folded into their
//   action's row as alternates;
// - the Mac menu bar's accelerators (src-tauri/src/app_menu.rs: ⌘R Reply,
//   ⌘1 Inbox, ⌘W Close…): MENU_KEYS mirrors them, and the test reads
//   app_menu.rs so the two can't drift;
// - keys an overlay handles itself while it's open (the composer's ⌘↵,
//   ⌘⇧A…): OVERLAY_KEYS. The composer's live in features/compose/index.tsx
//   `onKey`; keep this list in step with it.
import type { Shortcut } from "../lib/keyboard";

/**
 * Menu bar accelerators in registry key syntax, by menu item id. Only on
 * the Mac (the menu bar is macOS's); the sheet leaves them out elsewhere.
 * Must equal what `menuKeysFromSource(app_menu.rs)` finds.
 */
export const MENU_KEYS: Record<string, string> = {
  "compose.reply": "mod+r",
  "compose.replyAll": "mod+shift+r",
  "compose.forward": "mod+alt+f",
  "triage.done": "ctrl+mod+a",
  "triage.trash": "mod+backspace",
  "triage.read": "mod+shift+u",
  "triage.star": "mod+shift+l",
  "go.inbox": "mod+1",
  "go.starred": "mod+2",
  "go.sent": "mod+3",
  "go.drafts": "mod+4",
  "go.done": "mod+5",
  "go.all": "mod+6",
  "go.trash": "mod+7",
  "go.snoozed": "mod+8",
  "app.settings": "mod+,",
  "compose.new": "mod+n",
  "window.close": "mod+w",
  "search.open": "mod+f",
  "app.sidebar": "mod+\\",
  "floe.toggle": "mod+shift+f",
  "view.zoom.reset": "mod+0",
  "view.zoom.in": "mod+=",
  "view.zoom.out": "mod+-",
  "app.sync": "mod+shift+n",
  "compose.newWindow": "mod+alt+n",
};

/** Menu items that aren't registry shortcuts, as sheet rows. */
const MENU_ONLY_ROWS: { id: string; label: string; group: string }[] = [
  { id: "window.close", label: "Close window or the frontmost panel", group: "Window" },
  { id: "view.zoom.in", label: "Zoom in", group: "Window" },
  { id: "view.zoom.out", label: "Zoom out", group: "Window" },
  { id: "view.zoom.reset", label: "Actual size", group: "Window" },
];

/** Keys an overlay handles itself while it's open. */
export const OVERLAY_KEYS: { group: string; rows: [string, string][] }[] = [
  {
    group: "Compose window",
    rows: [
      ["Send", "mod+enter"],
      ["Send later…", "mod+shift+enter"],
      ["Discard draft (press twice)", "mod+shift+backspace"],
      ["Attach files", "mod+shift+a"],
      ["Choose the From account", "mod+shift+o"],
      ["Send from account 1–9", "alt+1"],
      ["Add Cc", "mod+shift+c"],
      ["Add Bcc", "mod+shift+b"],
      ["Go to the subject", "mod+shift+s"],
      ["Go to the message", "mod+j"],
      ["Save draft now", "mod+s"],
      ["Close (the draft is kept)", "escape"],
    ],
  },
  {
    // features/search/index.tsx `onKey`.
    group: "In search",
    rows: [
      ["Open the highlighted result", "enter"],
      ["Next / previous result", "arrowdown"],
      ["Next group of results", "tab"],
      ["Accept the suggestion", "tab"],
      ["Also search Gmail", "mod+shift+enter"],
      ["Last search again", "mod+arrowup"],
      ["Save this search", "mod+s"],
      ["Pin this search to the sidebar", "mod+shift+s"],
      ["Search tips", "mod+/"],
      ["Clear, then close", "escape"],
    ],
  },
];

export interface SheetRow {
  /** Stable React key. */
  id: string;
  label: string;
  /** The main key first, then alternates. */
  keys: string[];
}

export interface SheetGroup {
  name: string;
  rows: SheetRow[];
}

const ORDER = ["Navigate", "Select", "Go to", "Triage", "Compose", "Search", "Image viewer", "App", "Profiles", "Compose window", "In search", "Window"];

/**
 * The sheet's groups: every registry shortcut (one row per label within a
 * group, its hidden aliases and Mac menu accelerator as alternates), the
 * menu-only keys, and the overlays' own keys. `q` filters by label or key.
 */
export function sheetGroups(shortcuts: Shortcut[], opts: { mac: boolean; q?: string }): SheetGroup[] {
  const q = (opts.q ?? "").trim().toLowerCase();
  const byGroup = new Map<string, Map<string, SheetRow>>();
  const row = (group: string, label: string, id: string): SheetRow => {
    let rows = byGroup.get(group);
    if (!rows) byGroup.set(group, (rows = new Map()));
    let r = rows.get(label);
    if (!r) rows.set(label, (r = { id, label, keys: [] }));
    return r;
  };
  const addKey = (r: SheetRow, keys: string | undefined) => {
    if (keys && !r.keys.includes(keys)) r.keys.push(keys);
  };
  // Visible entries first, so a row's main key is the one the app teaches.
  const ordered = [...shortcuts.filter((s) => !s.hidden), ...shortcuts.filter((s) => s.hidden)];
  const labels = new Map<string, string>(); // label → group of its visible row
  for (const s of ordered) {
    const group = s.group ?? "Other";
    if (s.hidden) {
      // An alias of a listed action joins that row. A hidden Esc on its own
      // is a panel's close key, already said in the panel.
      const home = labels.get(s.label);
      if (home) addKey(row(home, s.label, s.id), s.keys);
      else if (s.keys !== "escape") addKey(row(group, s.label, s.id), s.keys);
      continue;
    }
    labels.set(s.label, group);
    const r = row(group, s.label, s.id);
    addKey(r, s.keys);
    if (opts.mac) addKey(r, MENU_KEYS[s.id]);
  }
  if (opts.mac) for (const m of MENU_ONLY_ROWS) addKey(row(m.group, m.label, m.id), MENU_KEYS[m.id]);
  for (const o of OVERLAY_KEYS) for (const [label, keys] of o.rows) addKey(row(o.group, label, `${o.group}:${label}`), keys);

  const matches = (r: SheetRow) => !q || r.label.toLowerCase().includes(q) || r.keys.some((k) => k.includes(q));
  return [...byGroup.entries()]
    .map(([name, rows]) => ({ name, rows: [...rows.values()].filter((r) => r.keys.length > 0 && matches(r)) }))
    .filter((g) => g.rows.length > 0)
    .sort((a, b) => rank(a.name) - rank(b.name));
}

function rank(g: string): number {
  const i = ORDER.indexOf(g);
  return i < 0 ? ORDER.length : i;
}

/** "CmdOrCtrl+Shift+R" → "mod+shift+r" (the registry's syntax). */
export function menuAccelToKeys(accel: string): string {
  return accel
    .split("+")
    .map((p) => {
      const l = p.toLowerCase();
      if (l === "cmdorctrl" || l === "cmd" || l === "commandorcontrol") return "mod";
      if (l === "option") return "alt";
      return l;
    })
    .join("+");
}

/** Every `("id", "Title", Some("Accel"))` in app_menu.rs, as id → registry keys. */
export function menuKeysFromSource(rust: string): Record<string, string> {
  const out: Record<string, string> = {};
  // rustfmt splits a long spec over lines, with a trailing comma.
  const re = /\(\s*"([a-zA-Z0-9_.]+)",\s*"(?:[^"\\]|\\.)*",\s*Some\("([^"]+)"\),?\s*\)/g;
  // Rust string escapes ("CmdOrCtrl+\\" is ⌘\).
  for (const m of rust.matchAll(re)) out[m[1]] = menuAccelToKeys(m[2].replace(/\\(.)/g, "$1"));
  return out;
}
