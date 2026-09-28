// Sidebar themes and the account color palette. OWNER: themes.
//
// The tokens live in styles/themes.css under [data-sidebar-theme="…"]; this
// file only names the themes (in display order) and applies the choice to
// <html>. The Rust side validates the same ids (src-tauri/src/settings.rs).
import type { AccentColor, CornerStyle, DarkShade, SidebarTheme } from "./types";

export interface ThemeInfo {
  id: SidebarTheme;
  name: string;
}

// Neutrals first (Graphite, then the tinted grays and browns), then the
// colors around the wheel from red to orange, so neighbors in the grid are
// neighbors in hue. tests/themes.test.ts keeps every pair visibly apart.
export const SIDEBAR_THEMES: ThemeInfo[] = [
  { id: "graphite", name: "Graphite" },
  { id: "slate", name: "Slate" },
  { id: "mauve", name: "Mauve" },
  { id: "sand", name: "Sand" },
  { id: "mocha", name: "Mocha" },
  { id: "crimson", name: "Crimson" },
  { id: "rose", name: "Rose" },
  { id: "plum", name: "Plum" },
  { id: "lavender", name: "Lavender" },
  { id: "indigo", name: "Indigo" },
  { id: "midnight", name: "Midnight" },
  { id: "sky", name: "Sky" },
  { id: "ocean", name: "Ocean" },
  { id: "arctic", name: "Arctic" },
  { id: "teal", name: "Teal" },
  { id: "mint", name: "Mint" },
  { id: "forest", name: "Forest" },
  { id: "sage", name: "Sage" },
  { id: "olive", name: "Olive" },
  { id: "butter", name: "Butter" },
  { id: "amber", name: "Amber" },
  { id: "peach", name: "Peach" },
];

/**
 * Account and profile colors, in the order the pickers lay them out: ten hue
 * columns (red, orange, amber, lime, green, teal, blue, violet, pink, slate)
 * by four rows (light, bright, deep, dark), read left to right. The bright
 * row holds the original eight plus rose, so every existing account keeps a
 * checked swatch; new accounts get colors from ACCOUNT_PALETTE in
 * src-tauri/src/ops.rs, a subset of this. The UI renders the actual hex
 * (lib/accountColor.ts): dots and bars use it as is, and text on it is
 * lightened or darkened per theme until it passes WCAG AA (checked in
 * tests/themes.test.ts). Names are the tooltips and accessible names.
 */
export const ACCOUNT_COLOR_COLUMNS = 10;
export const ACCOUNT_COLORS: { hex: string; name: string }[] = [
  { hex: "#FC988A", name: "Blush" },
  { hex: "#FBA773", name: "Apricot" },
  { hex: "#FBCE5C", name: "Honey" },
  { hex: "#B2DB7D", name: "Pistachio" },
  { hex: "#79CBA8", name: "Mint" },
  { hex: "#73C6D7", name: "Aqua" },
  { hex: "#97B5FF", name: "Sky" },
  { hex: "#D39FF4", name: "Lilac" },
  { hex: "#F795B5", name: "Petal" },
  { hex: "#ABB9CC", name: "Silver" },

  { hex: "#E0685A", name: "Coral" },
  { hex: "#FF8B3D", name: "Orange" },
  { hex: "#E3A13B", name: "Amber" },
  { hex: "#8DBD42", name: "Lime" },
  { hex: "#2FA37A", name: "Green" },
  { hex: "#2BA3B8", name: "Teal" },
  { hex: "#4F7CFF", name: "Blue" },
  { hex: "#B06AD9", name: "Violet" },
  { hex: "#D9648F", name: "Rose" },
  { hex: "#7C8A9E", name: "Slate" },

  { hex: "#BD493D", name: "Red" },
  { hex: "#DC6B0D", name: "Rust" },
  { hex: "#C28204", name: "Ochre" },
  { hex: "#6F9D17", name: "Olive" },
  { hex: "#008660", name: "Emerald" },
  { hex: "#008396", name: "Lagoon" },
  { hex: "#3B64E5", name: "Cobalt" },
  { hex: "#924CB9", name: "Purple" },
  { hex: "#B74571", name: "Berry" },
  { hex: "#647387", name: "Steel" },

  { hex: "#96372D", name: "Brick" },
  { hex: "#9B4805", name: "Clay" },
  { hex: "#9E6A09", name: "Bronze" },
  { hex: "#5A8013", name: "Moss" },
  { hex: "#00694A", name: "Forest" },
  { hex: "#006776", name: "Petrol" },
  { hex: "#2C4DB6", name: "Navy" },
  { hex: "#723A92", name: "Plum" },
  { hex: "#903458", name: "Wine" },
  { hex: "#4B596C", name: "Charcoal" },
];

/** The palette name for a hex ("Blue"), or the hex itself for a color outside it. */
export function accountColorName(hex: string): string {
  return ACCOUNT_COLORS.find((c) => c.hex.toLowerCase() === hex.toLowerCase())?.name ?? hex.toUpperCase();
}

// Remembered locally so the theme is on <html> before settings load (no
// flash of Graphite at startup). Per-device convenience only; settings.json
// stays the source of truth.
const CACHE_KEY = "penguin.sidebarTheme";

export function applySidebarTheme(theme: SidebarTheme, matchAccent: boolean) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.dataset.sidebarTheme = theme;
  if (matchAccent) root.dataset.accent = "theme";
  else delete root.dataset.accent;
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify({ theme, matchAccent }));
  } catch {
    // Storage unavailable (private mode): the settings load applies it anyway.
  }
}

/** Apply the last-used theme before first paint. */
export function applyCachedSidebarTheme() {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return;
    const { theme, matchAccent } = JSON.parse(raw) as { theme: SidebarTheme; matchAccent: boolean };
    if (SIDEBAR_THEMES.some((t) => t.id === theme)) applySidebarTheme(theme, !!matchAccent);
  } catch {
    // Unreadable cache: Graphite until settings load.
  }
}

/**
 * Dark mode shades (Settings → General → Dark mode shade). The ramps live in
 * styles/penguin.css under [data-theme="dark"][data-dark-shade="…"]; light
 * mode ignores the attribute. Black is the original look.
 */
export const DARK_SHADES: { id: DarkShade; name: string }[] = [
  { id: "black", name: "Black" },
  { id: "charcoal", name: "Charcoal" },
  { id: "dim", name: "Dim" },
  { id: "navy", name: "Navy" },
];

const SHADE_KEY = "penguin.darkShade";

/** `<html data-dark-shade>`, remembered so the next launch paints with it. Unknown names are Black. */
export function applyDarkShade(shade: DarkShade) {
  const id: DarkShade = DARK_SHADES.some((s) => s.id === shade) ? shade : "black";
  if (typeof document !== "undefined") document.documentElement.dataset.darkShade = id;
  try {
    localStorage.setItem(SHADE_KEY, id);
  } catch {
    // Storage unavailable: the settings load applies it anyway.
  }
}

/**
 * Accent colors (Settings → General → Accent color), in the order macOS lists
 * its own, plus teal. `swatch` is the dot drawn in the picker. The tokens
 * live in styles/themes.css under [data-accent-color]; Blue is the design
 * system's default and has no rule.
 */
export const ACCENT_COLORS: { id: AccentColor; name: string; swatch: string }[] = [
  { id: "blue", name: "Blue", swatch: "#0090ff" },
  { id: "purple", name: "Purple", swatch: "#8e4ec6" },
  { id: "pink", name: "Pink", swatch: "#d6409f" },
  { id: "red", name: "Red", swatch: "#e5484d" },
  { id: "orange", name: "Orange", swatch: "#f76b15" },
  { id: "yellow", name: "Yellow", swatch: "#e2a336" },
  { id: "green", name: "Green", swatch: "#30a46c" },
  { id: "teal", name: "Teal", swatch: "#12a594" },
  { id: "graphite", name: "Graphite", swatch: "#8e8e93" },
];

export const CORNER_STYLES: { id: CornerStyle; name: string }[] = [
  { id: "rounded", name: "Rounded" },
  { id: "subtle", name: "Subtle" },
  { id: "square", name: "Square" },
];

const ACCENT_KEY = "penguin.accentCorners";

/**
 * `<html data-accent-color>` and `<html data-corners>`, unset at the
 * defaults (Blue, Rounded), remembered for the next launch's first paint.
 */
export function applyAccentAndCorners(accent: AccentColor, corners: CornerStyle) {
  const a: AccentColor = ACCENT_COLORS.some((c) => c.id === accent) ? accent : "blue";
  const k: CornerStyle = CORNER_STYLES.some((c) => c.id === corners) ? corners : "rounded";
  if (typeof document !== "undefined") {
    const ds = document.documentElement.dataset;
    if (a === "blue") delete ds.accentColor;
    else ds.accentColor = a;
    if (k === "rounded") delete ds.corners;
    else ds.corners = k;
  }
  try {
    localStorage.setItem(ACCENT_KEY, JSON.stringify({ accent: a, corners: k }));
  } catch {
    // Storage unavailable: the settings load applies it anyway.
  }
}

export function applyCachedAccentAndCorners() {
  try {
    const raw = localStorage.getItem(ACCENT_KEY);
    if (!raw) return;
    const { accent, corners } = JSON.parse(raw) as { accent: AccentColor; corners: CornerStyle };
    applyAccentAndCorners(accent, corners);
  } catch {
    // Unreadable cache: the defaults until settings load.
  }
}

/** Apply the last-used shade before first paint. */
export function applyCachedDarkShade() {
  try {
    const cached = localStorage.getItem(SHADE_KEY);
    if (cached) applyDarkShade(cached as DarkShade);
  } catch {
    // Unreadable cache: Black until settings load.
  }
}
