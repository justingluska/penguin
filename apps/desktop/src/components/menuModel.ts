// Menu entries and the pure logic behind menus (ContextMenu.tsx): which
// entries show, which can be chosen, and keyboard movement. No DOM, so the
// tests can run it under node.
import type { ReactNode } from "react";
import type { IconName } from "./Icon";

export interface MenuItem {
  type?: "item";
  label: ReactNode;
  /** Plain text for type-ahead and accessibility when `label` isn't a string. */
  text?: string;
  icon?: IconName;
  /** A custom leading visual (color swatch, account dot); wins over `icon`. */
  lead?: ReactNode;
  /** Registry key spec ("e", "shift+u", "mod+c"); hidden with shortcut hints off. */
  keys?: string;
  /** Muted text on the right when there are no keys (e.g. a count). */
  end?: ReactNode;
  /** true, or the reason it can't be chosen (shown under the label). */
  disabled?: boolean | string;
  /** Destructive: red text. */
  danger?: boolean;
  /** Show a check mark (toggles, the current choice in a Select). */
  checked?: boolean;
  /** Leave the menu open after choosing (e.g. toggling several labels). */
  keepOpen?: boolean;
  onSelect?: () => void;
  submenu?: MenuEntries | (() => MenuEntries);
  /**
   * Lay the submenu out as a grid this many columns wide (color pickers):
   * each item shows only its `lead` as a cell, its label as the tooltip and
   * accessible name, and the arrows move in two dimensions (gridStep).
   */
  submenuGrid?: number;
}
export interface MenuSeparator {
  type: "separator";
}
export interface MenuHeader {
  type: "header";
  label: string;
}
export type MenuEntry = MenuItem | MenuSeparator | MenuHeader;
export type MenuEntries = (MenuEntry | false | null | undefined | 0 | "")[];

/** Drop falsy entries and collapse leading/trailing/double separators (and orphaned headers). */
export function normalizeEntries(list: MenuEntries): MenuEntry[] {
  const present = list.filter((e): e is MenuEntry => !!e);
  // A header with no items under it (its section emptied out) goes first…
  const kept = present.filter((e, i) => e.type !== "header" || isItem(present[i + 1]));
  // …then separators collapse and trim.
  const out: MenuEntry[] = [];
  for (const e of kept) {
    if (e.type === "separator" && (out.length === 0 || out[out.length - 1].type === "separator")) continue;
    out.push(e);
  }
  while (out.length && out[out.length - 1].type === "separator") out.pop();
  return out;
}

export function isItem(e: MenuEntry | undefined): e is MenuItem {
  return !!e && (e.type === undefined || e.type === "item");
}
export function enabled(e: MenuEntry | undefined): e is MenuItem {
  return isItem(e) && !e.disabled;
}
export function itemText(e: MenuItem): string {
  return (e.text ?? (typeof e.label === "string" ? e.label : "")).trim();
}
export function subEntries(e: MenuItem): MenuEntry[] {
  return normalizeEntries(typeof e.submenu === "function" ? e.submenu() : e.submenu ?? []);
}

/** Next enabled index from `from` in direction `dir` (wrapping), or -1. */
export function firstEnabled(entries: MenuEntry[], from = 0, dir = 1): number {
  const n = entries.length;
  for (let k = 0; k < n; k++) {
    const i = (((from + dir * k) % n) + n) % n;
    if (enabled(entries[i])) return i;
  }
  return -1;
}


/**
 * Arrow-key movement in a grid of `count` cells, `cols` wide (color pickers):
 * ←→ within the row, ↑↓ between rows, Home/End to the row's ends. Returns
 * the new index (the same one at an edge), "left-edge" for ← in the first
 * column (a submenu closes on it), or null for keys that aren't movement.
 * From no active cell (-1) any movement starts at the first cell.
 */
export function gridStep(index: number, key: string, count: number, cols: number): number | "left-edge" | null {
  if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(key)) return null;
  if (count <= 0) return -1;
  if (index < 0 || index >= count) return 0;
  const col = index % cols;
  const rowStart = index - col;
  switch (key) {
    case "ArrowLeft":
      return col === 0 ? "left-edge" : index - 1;
    case "ArrowRight":
      return col < cols - 1 && index + 1 < count ? index + 1 : index;
    case "ArrowUp":
      return index - cols >= 0 ? index - cols : index;
    case "ArrowDown":
      return index + cols < count ? index + cols : index;
    case "Home":
      return rowStart;
    default:
      return Math.min(rowStart + cols, count) - 1;
  }
}

/**
 * Type-ahead: the enabled item whose text starts with `typed`, searching from
 * after `active` for a single letter (so repeating it cycles) and from
 * `active` itself for a longer prefix (so the match holds while typing).
 */
export function typeaheadMatch(entries: MenuEntry[], active: number, typed: string): number {
  const n = entries.length;
  const t = typed.toLowerCase();
  const start = t.length === 1 ? active + 1 : Math.max(active, 0);
  for (let k = 0; k < n; k++) {
    const i = (start + k) % n;
    const it = entries[i];
    if (enabled(it) && itemText(it).toLowerCase().startsWith(t)) return i;
  }
  return -1;
}
