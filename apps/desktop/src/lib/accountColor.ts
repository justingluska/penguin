// Account and profile colors, rendered as the actual hex the user picked.
// OWNER: themes.
//
// Labels still snap to the design system's named tones (toneForColor in
// format.ts); accounts don't, so all 40 palette colors (lib/themes.ts) stay
// distinct. accountTone(hex) returns the tone suffix for the usual
// `t-${…}` class, "acct ac-<hex>", and registers one rule per color:
//
//   .ac-4f7cff { --acct: #4f7cff; --acct-on-dark: …; --acct-on-light: …; --acct-ink: … }
//
// styles/themes.css turns that into the tone variables every tinted thing
// already reads (--t3/--t4/--t5 fills, --t11 text) plus --tm, the "mark"
// color for dots, bars and swatches. Marks are the hex itself. Text is the
// hex moved along OKLCH lightness (hue and chroma kept, chroma trimmed only
// to stay in sRGB) just far enough to reach WCAG AA on the lightest dark
// surface or the darkest light surface it sits on, fill tint included.
// tests/themes.test.ts checks the palette against every sidebar theme.

/** Fill tints (share of the account color over the surface): --t3, --t4, --t5 in styles/themes.css. */
export const ACCT_TINTS = { dark: [0.14, 0.2, 0.26], light: [0.1, 0.15, 0.22] } as const;

/**
 * Worst-case surfaces for account-colored text, per mode: [surface, tint].
 * Tinted text sits on --t3 in the sidebar (account tiles, monograms; up to a
 * hovered row) and on --t4/--t5 in the calendar and lists (app surfaces).
 * The dark references are the Dim shade's (styles/penguin.css 1c), the
 * lightest dark mode: a hovered row in its Graphite sidebar, and its
 * elevated fill (--gray-2).
 */
const TEXT_REFS = {
  dark: [
    ["#363e4a", ACCT_TINTS.dark[0]],
    ["#2f3640", ACCT_TINTS.dark[2]],
  ],
  light: [
    ["#d4d4d4", ACCT_TINTS.light[0]],
    ["#ebebeb", ACCT_TINTS.light[2]],
  ],
} as const;

/** WCAG AA for body text, plus a hair so rounding to #rrggbb never lands under it. */
const TARGET = 4.6;

export type RGB = [number, number, number];

export function parseHex(hex: string | null | undefined): RGB | null {
  const m = /^#?([0-9a-f]{6})$/i.exec((hex ?? "").trim());
  if (!m) return null;
  const v = parseInt(m[1], 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

export function toHex(c: RGB): string {
  return "#" + c.map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, "0")).join("");
}

const toLinear = (v: number) => {
  const x = v / 255;
  return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
};
const fromLinear = (v: number) => 255 * (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);

export function luminance(c: RGB): number {
  const [r, g, b] = c.map(toLinear);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrast(a: RGB, b: RGB): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** `fg` at `alpha` over opaque `bg`: what color-mix(in srgb, fg N%, transparent) looks like on it. */
export function over(fg: RGB, alpha: number, bg: RGB): RGB {
  return [0, 1, 2].map((i) => fg[i] * alpha + bg[i] * (1 - alpha)) as RGB;
}

function toOklch(c: RGB): [number, number, number] {
  const [r, g, b] = c.map(toLinear);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [L, Math.hypot(A, B), Math.atan2(B, A)];
}

function oklchToLinear(L: number, C: number, h: number): RGB {
  const a = C * Math.cos(h);
  const b = C * Math.sin(h);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}

/** OKLCH to sRGB, trimming chroma (not hue or lightness) until it fits the gamut. */
function fromOklch(L: number, C: number, h: number): RGB {
  const fits = (c: RGB) => c.every((v) => v >= -1e-6 && v <= 1 + 1e-6);
  let lin = oklchToLinear(L, C, h);
  if (!fits(lin)) {
    let lo = 0;
    let hi = C;
    for (let i = 0; i < 24; i++) {
      const mid = (lo + hi) / 2;
      if (fits(oklchToLinear(L, mid, h))) lo = mid;
      else hi = mid;
    }
    lin = oklchToLinear(L, lo, h);
  }
  return lin.map((v) => Math.round(fromLinear(Math.min(1, Math.max(0, v))))) as RGB;
}

function passes(text: RGB, color: RGB, refs: readonly (readonly [string, number])[]): boolean {
  return refs.every(([surface, tint]) => {
    const bg = parseHex(surface)!;
    return contrast(text, over(color, tint, bg)) >= TARGET;
  });
}

/** The account color as text for one mode: the hex itself when it already passes, else nudged lighter (dark) or darker (light). */
function textShade(color: RGB, mode: "dark" | "light"): RGB {
  const refs = TEXT_REFS[mode];
  if (passes(color, color, refs)) return color;
  const [L0, C, h] = toOklch(color);
  const step = mode === "dark" ? 0.005 : -0.005;
  for (let L = L0 + step; L > 0 && L < 1; L += step) {
    const c = fromOklch(L, C, h);
    if (passes(c, color, refs)) return c;
  }
  return mode === "dark" ? [255, 255, 255] : [0, 0, 0];
}

export interface AccountShades {
  /** Dots, bars, swatches: the color as picked. */
  mark: string;
  /** Text in dark mode (and its tinted fills). */
  onDark: string;
  /** Text in light mode. */
  onLight: string;
  /** Black or white, whichever reads better on the mark (the picker's check). */
  ink: string;
}

const shadeCache = new Map<string, AccountShades | null>();

export function accountShades(hex: string | null | undefined): AccountShades | null {
  const key = (hex ?? "").trim().toLowerCase();
  const hit = shadeCache.get(key);
  if (hit !== undefined) return hit;
  const c = parseHex(key);
  const out = c && {
    mark: toHex(c),
    onDark: toHex(textShade(c, "dark")),
    onLight: toHex(textShade(c, "light")),
    ink: contrast(c, [0, 0, 0]) >= contrast(c, [255, 255, 255]) ? "#000000" : "#ffffff",
  };
  shadeCache.set(key, out);
  return out;
}

// One <style> rule per color in use, added the first time it renders.
let sheet: CSSStyleSheet | null = null;
const registered = new Set<string>();

function register(key: string, s: AccountShades) {
  if (registered.has(key) || typeof document === "undefined") return;
  registered.add(key);
  if (!sheet) {
    const el = document.createElement("style");
    el.dataset.owner = "account-colors";
    document.head.appendChild(el);
    sheet = el.sheet;
    if (!sheet) return;
  }
  sheet.insertRule(
    `.ac-${key}{--acct:${s.mark};--acct-on-dark:${s.onDark};--acct-on-light:${s.onLight};--acct-ink:${s.ink}}`,
    sheet.cssRules.length,
  );
}

/**
 * Tone suffix for an account or profile color, for `t-${accountTone(hex)}`:
 * "acct ac-4f7cff" (the real color, theme-aware), or "gray" when unset.
 */
export function accountTone(hex: string | null | undefined): string {
  const s = accountShades(hex);
  if (!s) return "gray";
  const key = s.mark.slice(1);
  register(key, s);
  return `acct ac-${key}`;
}
