// "List style" (Settings → General): how the conversation list's rows look.
// One attribute, <html data-list-style>, carries it to the CSS
// (styles/list-styles.css); the row heights that go with each style live here,
// because the list is virtualized and its rows are sized in JS
// (features/inbox/ThreadList.tsx), not by their content.
//
// Like the sidebar theme, the last choice is cached in localStorage and put on
// before first paint, so a Classic user never sees Quiet rows (or their
// heights) while settings load.
import { useSyncExternalStore } from "react";
import type { Density, ListStyle } from "./types";

export const DEFAULT_LIST_STYLE: ListStyle = "quiet";

export const LIST_STYLES: { id: ListStyle; name: string; blurb: string }[] = [
  { id: "quiet", name: "Quiet", blurb: "One accent, the unread dot. Read mail steps back." },
  { id: "classic", name: "Classic", blurb: "Colored monograms and an account bar on the edge." },
  { id: "contrast", name: "High contrast", blurb: "New mail on a tint in bold; read mail muted." },
  { id: "cards", name: "Cards", blurb: "Each conversation a card, with air between." },
  { id: "mail", name: "Mail", blurb: "Bigger faces and two lines of preview." },
];

const isListStyle = (v: unknown): v is ListStyle => LIST_STYLES.some((s) => s.id === v);

/**
 * Stacked (three-line) row height per style and density, in px. Classic is
 * the original 72/80. Quiet gets 4px more, split evenly above and below the
 * text (the rows center their lines), so the list breathes a little without
 * losing a row on screen. High contrast leaves a 2px gap between its tinted
 * rows. Cards inset each card 3px top and bottom inside its slot and still
 * keep Quiet's air inside the card. Mail fits 40px faces and a two-line preview.
 */
export const STACKED_ROW_H: Record<ListStyle, Record<Density, number>> = {
  classic: { compact: 72, comfortable: 80 },
  quiet: { compact: 76, comfortable: 84 },
  contrast: { compact: 74, comfortable: 82 },
  cards: { compact: 82, comfortable: 90 },
  mail: { compact: 86, comfortable: 94 },
};

/**
 * One-line row height (wide list) per density; the same for every style, and
 * matches --row-h (styles/penguin.css, features/settings/settings.css).
 */
export const LINE_ROW_H: Record<Density, number> = { compact: 40, comfortable: 52 };

/** The virtualizer's row height for this style, density and layout. */
export function listRowHeight(style: ListStyle, density: Density, stacked: boolean): number {
  return stacked ? STACKED_ROW_H[style][density] : LINE_ROW_H[density];
}

const CACHE_KEY = "penguin.listStyle";
let current: ListStyle = DEFAULT_LIST_STYLE;
const subs = new Set<() => void>();

/** Put the style on <html> and remember it for the next launch. */
export function applyListStyle(next: ListStyle) {
  const style = isListStyle(next) ? next : DEFAULT_LIST_STYLE;
  if (typeof document !== "undefined") document.documentElement.dataset.listStyle = style;
  try {
    localStorage.setItem(CACHE_KEY, style);
  } catch {
    // No storage (private window): the setting still applies once loaded.
  }
  if (style === current) return;
  current = style;
  subs.forEach((f) => f());
}

/** Before first paint: last run's style, until settings load. */
export function applyCachedListStyle() {
  let cached: string | null = null;
  try {
    cached = localStorage.getItem(CACHE_KEY);
  } catch {
    // Unreadable storage: the default until settings load.
  }
  applyListStyle(isListStyle(cached) ? cached : DEFAULT_LIST_STYLE);
}

/** The style on screen now (the cached one until settings load). */
export function useListStyle(): ListStyle {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => current,
  );
}
