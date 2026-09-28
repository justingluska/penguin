// Pane layout: sidebar and list widths, sidebar collapse, and collapsed or
// hidden sidebar sections. Persisted per device in localStorage (best effort: a
// blocked or empty storage just means the defaults).
import { useSyncExternalStore } from "react";

export const SIDEBAR_W = { def: 200, min: 190, max: 320 };
export const LIST_W = { def: 340, min: 280, max: 760 };
/** The reading pane never gets narrower than this while dragging the list. */
export const READING_MIN_W = 380;

export type SidebarSection = "labels" | "accounts";

export interface LayoutState {
  sidebarW: number;
  listW: number;
  sidebarCollapsed: boolean;
  collapsedSections: SidebarSection[];
  /** Sections taken out of the sidebar entirely (Settings → General). */
  hiddenSections: SidebarSection[];
}

const KEY = "penguin.layout.v1";
const DEFAULTS: LayoutState = { sidebarW: SIDEBAR_W.def, listW: LIST_W.def, sidebarCollapsed: false, collapsedSections: [], hiddenSections: [] };

const sections = (v: unknown): SidebarSection[] =>
  Array.isArray(v) ? v.filter((s): s is SidebarSection => s === "labels" || s === "accounts") : [];

export const clamp = (n: number, r: { min: number; max: number }) => Math.round(Math.min(r.max, Math.max(r.min, n)));

function load(): LayoutState {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULTS;
    const p = JSON.parse(raw) as Partial<LayoutState>;
    return {
      sidebarW: typeof p.sidebarW === "number" ? clamp(p.sidebarW, SIDEBAR_W) : DEFAULTS.sidebarW,
      listW: typeof p.listW === "number" ? clamp(p.listW, LIST_W) : DEFAULTS.listW,
      sidebarCollapsed: p.sidebarCollapsed === true,
      collapsedSections: sections(p.collapsedSections),
      hiddenSections: sections(p.hiddenSections),
    };
  } catch {
    return DEFAULTS;
  }
}

let state: LayoutState = load();
const subs = new Set<() => void>();

export function getLayout(): LayoutState {
  return state;
}

export function setLayout(patch: Partial<LayoutState> | ((s: LayoutState) => Partial<LayoutState>)) {
  const p = typeof patch === "function" ? patch(state) : patch;
  state = { ...state, ...p };
  try {
    localStorage.setItem(KEY, JSON.stringify(state));
  } catch {
    // Storage unavailable (private mode, quota): the layout still applies for this session.
  }
  subs.forEach((f) => f());
}

/** Notified after every change (for non-React consumers such as the menu bar). */
export function subscribeLayout(cb: () => void): () => void {
  subs.add(cb);
  return () => subs.delete(cb);
}

export function useLayout<T>(select: (s: LayoutState) => T): T {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => select(state),
  );
}

export function toggleSidebar() {
  setLayout((s) => ({ sidebarCollapsed: !s.sidebarCollapsed }));
}

export function toggleSection(section: SidebarSection) {
  setLayout((s) => ({
    collapsedSections: s.collapsedSections.includes(section)
      ? s.collapsedSections.filter((x) => x !== section)
      : [...s.collapsedSections, section],
  }));
}

export function setSectionHidden(section: SidebarSection, hidden: boolean) {
  setLayout((s) => ({
    hiddenSections: hidden ? [...s.hiddenSections.filter((x) => x !== section), section] : s.hiddenSections.filter((x) => x !== section),
  }));
}
