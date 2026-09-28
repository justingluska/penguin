// Composer font (Settings → Compose). OWNER: floe.
//
// Scoped to the compose editor, never the app chrome. Fonts are self-hosted
// @fontsource files (the CSP blocks remote fonts), imported on demand so
// startup only pays for Inter, which the UI already loads. The choice is
// pushed onto <html> as --compose-font / --compose-font-size; the composer's
// CSS reads them. Outgoing HTML gets the same family as a fallback stack via
// composeFontCss(): recipients without the font see their own sans or serif.
import type { ComposeFont } from "./types";
import { currentSettings, subscribeSettings } from "./settings";
import { getUi, subscribeUi } from "./ui";

export interface ComposeFontInfo {
  id: ComposeFont;
  name: string;
  /** CSS family name as declared by the @fontsource files. */
  family: string;
  serif: boolean;
  note: string;
}

export const COMPOSE_FONTS: ComposeFontInfo[] = [
  { id: "inter", name: "Inter", family: "Inter", serif: false, note: "Clean and neutral. The default." },
  { id: "geist", name: "Geist", family: "Geist", serif: false, note: "Geometric and crisp." },
  { id: "ibmPlexSans", name: "IBM Plex Sans", family: "IBM Plex Sans", serif: false, note: "Warm, a little technical." },
  { id: "sourceSerif4", name: "Source Serif 4", family: "Source Serif 4", serif: true, note: "A classic letter serif." },
  { id: "literata", name: "Literata", family: "Literata", serif: true, note: "Editorial, made for screens." },
  { id: "iaWriterQuattro", name: "iA Writer Quattro", family: "iA Writer Quattro", serif: false, note: "Typewriter rhythm for focused writing." },
  { id: "atkinsonHyperlegible", name: "Atkinson Hyperlegible", family: "Atkinson Hyperlegible", serif: false, note: "Maximum character distinction." },
];

export const COMPOSE_FONT_SIZE = { min: 12, max: 20, def: 15 };

// Static strings so Vite can split each font into its own lazily fetched chunk.
const LOADERS: Record<ComposeFont, () => Promise<unknown>> = {
  inter: async () => {}, // already in styles/fonts.css
  geist: () => import("@fontsource/geist/latin-400.css"),
  ibmPlexSans: () => import("@fontsource/ibm-plex-sans/latin-400.css"),
  sourceSerif4: () => import("@fontsource/source-serif-4/latin-400.css"),
  literata: () => import("@fontsource/literata/latin-400.css"),
  iaWriterQuattro: () => import("@fontsource/ia-writer-quattro/latin-400.css"),
  atkinsonHyperlegible: () => import("@fontsource/atkinson-hyperlegible/latin-400.css"),
};

const SANS_FALLBACK = `-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif`;
const SERIF_FALLBACK = `Georgia, 'Times New Roman', Times, serif`;

export function fontInfo(id: ComposeFont): ComposeFontInfo {
  return COMPOSE_FONTS.find((f) => f.id === id) ?? COMPOSE_FONTS[0];
}

/** The chosen family, then system fallbacks of the same kind. Single-quoted: safe in a style="" attribute. */
export function composeFontStack(id: ComposeFont): string {
  const f = fontInfo(id);
  return `'${f.family}', ${f.serif ? SERIF_FALLBACK : SANS_FALLBACK}`;
}

export function clampFontSize(n: number): number {
  return Number.isFinite(n) ? Math.round(Math.min(COMPOSE_FONT_SIZE.max, Math.max(COMPOSE_FONT_SIZE.min, n))) : COMPOSE_FONT_SIZE.def;
}

/** Inline CSS for the outgoing HTML wrapper, from the current settings. */
export function composeFontCss(): string {
  const s = currentSettings();
  return `font-family:${composeFontStack(s.composeFont)};font-size:${clampFontSize(s.composeFontSize)}px`;
}

const loaded = new Map<ComposeFont, Promise<unknown>>();

/** Fetch a font's CSS (and so its woff2 once text uses it). Idempotent. */
export function loadComposeFont(id: ComposeFont): Promise<unknown> {
  let p = loaded.get(id);
  if (!p) {
    p = (LOADERS[id] ?? LOADERS.inter)().catch((e) => {
      // Missing chunk (e.g. a stale dev build): the fallback stack still renders.
      loaded.delete(id);
      console.warn(`penguin: could not load compose font ${id}`, e);
    });
    loaded.set(id, p);
  }
  return p;
}

function apply() {
  const s = currentSettings();
  const root = document.documentElement.style;
  root.setProperty("--compose-font", composeFontStack(s.composeFont));
  root.setProperty("--compose-font-size", `${clampFontSize(s.composeFontSize)}px`);
  // Inter's alternates (as in the app UI); other fonts' stylistic sets mean something else.
  root.setProperty("--compose-font-features", s.composeFont === "inter" ? `"cv11", "ss01"` : "normal");
  // Fetch the file only once the composer is actually on screen.
  if (getUi().overlay === "compose") void loadComposeFont(s.composeFont);
}

if (typeof document !== "undefined") {
  apply();
  subscribeSettings(apply);
  let wasCompose = false;
  subscribeUi(() => {
    const isCompose = getUi().overlay === "compose";
    if (isCompose && !wasCompose) void loadComposeFont(currentSettings().composeFont);
    wasCompose = isCompose;
  });
}
