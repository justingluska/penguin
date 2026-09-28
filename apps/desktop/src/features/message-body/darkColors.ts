// Color math for "Dark email bodies" (Settings → General, experimental).
// OWNER: security-render agent. Pure functions, no DOM: darkBody.ts reads a
// message's computed colors and asks these what each should become.
//
// The mapping works in OKLCH, so a color keeps its hue while its lightness
// flips: light backgrounds become dark ones, and text and borders are moved
// to the lighter side of their new background with about the contrast they
// had on the old one (WCAG 2 ratios, floored at AA for text). Dark and vivid
// backgrounds (brand headers, buttons) keep their color, and so does
// anything drawn on them.

export interface RGBA {
  /** 0–255 */
  r: number;
  g: number;
  b: number;
  /** 0–1 */
  a: number;
}

interface Oklch {
  l: number;
  c: number;
  h: number;
}

/** The dark canvas a white message body turns into. Matches the
 * `data-pg-scheme=dark` canvas in penguin-render's BASE_CSS. */
export const DARK_CANVAS: RGBA = { r: 0x1c, g: 0x1c, b: 0x1e, a: 1 };
export const WHITE: RGBA = { r: 255, g: 255, b: 255, a: 1 };

// ---------------------------------------------------------------------------
// Parsing and formatting (computed-style serializations only)
// ---------------------------------------------------------------------------

const NUM = String.raw`[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?`;
const FN_RE = new RegExp(String.raw`^(rgba?|color|oklab|oklch)\(\s*(.*?)\s*\)$`, "i");
const PART_RE = new RegExp(String.raw`^(${NUM})(%|deg)?$`, "i");

function part(s: string): { n: number; pct: boolean } | null {
  const m = PART_RE.exec(s.trim());
  return m ? { n: parseFloat(m[1]), pct: m[2] === "%" } : null;
}

/**
 * A color as getComputedStyle serializes it: `rgb()`/`rgba()` (comma or
 * space syntax), `color(srgb …)`, `oklab()` or `oklch()`, or `transparent`.
 * Anything else (lab(), lch(), other color spaces, keywords) returns null
 * and is left alone by the caller.
 */
export function parseColor(input: string): RGBA | null {
  const s = input.trim().toLowerCase();
  if (s === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  const m = FN_RE.exec(s);
  if (!m) return null;
  let fn = m[1];
  let body = m[2];
  if (fn === "color") {
    const sp = /^srgb\s+(.*)$/.exec(body);
    if (!sp) return null;
    fn = "srgb";
    body = sp[1];
  }
  const [main, alphaPart] = body.includes("/") ? body.split("/") : [body, undefined];
  const bits = main.split(/[\s,]+/).filter(Boolean);
  let alphaStr = alphaPart;
  if (bits.length === 4 && alphaStr === undefined) alphaStr = bits.pop();
  if (bits.length !== 3) return null;
  const ps = bits.map(part);
  if (ps.some((p) => p == null)) return null;
  const [x, y, z] = ps as { n: number; pct: boolean }[];
  let a = 1;
  if (alphaStr !== undefined) {
    const ap = part(alphaStr);
    if (!ap) return null;
    a = ap.pct ? ap.n / 100 : ap.n;
  }
  a = clamp(a, 0, 1);
  switch (fn) {
    case "rgb":
    case "rgba": {
      const ch = (p: { n: number; pct: boolean }) => clamp(p.pct ? (p.n * 255) / 100 : p.n, 0, 255);
      return { r: ch(x), g: ch(y), b: ch(z), a };
    }
    case "srgb": {
      const ch = (p: { n: number; pct: boolean }) => clamp((p.pct ? p.n / 100 : p.n) * 255, 0, 255);
      return { r: ch(x), g: ch(y), b: ch(z), a };
    }
    case "oklab": {
      const l = x.pct ? x.n / 100 : x.n;
      const aa = y.pct ? (y.n * 0.4) / 100 : y.n;
      const bb = z.pct ? (z.n * 0.4) / 100 : z.n;
      return { ...fromOklab(l, aa, bb), a };
    }
    case "oklch": {
      const l = x.pct ? x.n / 100 : x.n;
      const c = y.pct ? (y.n * 0.4) / 100 : y.n;
      return { ...fromOklch({ l, c, h: z.n }), a };
    }
  }
  return null;
}

export function formatColor(c: RGBA): string {
  const r = Math.round(c.r);
  const g = Math.round(c.g);
  const b = Math.round(c.b);
  if (c.a >= 0.999) return `rgb(${r}, ${g}, ${b})`;
  return `rgba(${r}, ${g}, ${b}, ${Math.round(c.a * 1000) / 1000})`;
}

// ---------------------------------------------------------------------------
// sRGB ⇄ OKLab/OKLCH (Björn Ottosson's matrices)
// ---------------------------------------------------------------------------

function toLinear(v: number): number {
  const c = v / 255;
  return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

function fromLinear(v: number): number {
  const c = v <= 0.0031308 ? v * 12.92 : 1.055 * Math.pow(v, 1 / 2.4) - 0.055;
  return c * 255;
}

export function toOklch(c: RGBA): Oklch {
  const r = toLinear(c.r);
  const g = toLinear(c.g);
  const b = toLinear(c.b);
  const l_ = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m_ = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s_ = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_;
  const A = 1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_;
  const B = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_;
  const chroma = Math.hypot(A, B);
  return { l: L, c: chroma, h: chroma < 1e-4 ? 0 : (Math.atan2(B, A) * 180) / Math.PI };
}

/** Unclamped linear sRGB for an OKLab color. */
function oklabToLinear(L: number, A: number, B: number): [number, number, number] {
  const l = Math.pow(L + 0.3963377774 * A + 0.2158037573 * B, 3);
  const m = Math.pow(L - 0.1055613458 * A - 0.0638541728 * B, 3);
  const s = Math.pow(L - 0.0894841775 * A - 1.291485548 * B, 3);
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}

function fromOklab(L: number, A: number, B: number): Omit<RGBA, "a"> {
  const [r, g, b] = oklabToLinear(L, A, B);
  return { r: clamp(fromLinear(r), 0, 255), g: clamp(fromLinear(g), 0, 255), b: clamp(fromLinear(b), 0, 255) };
}

function inGamut(L: number, A: number, B: number): boolean {
  const eps = 1e-4;
  return oklabToLinear(L, A, B).every((v) => v >= -eps && v <= 1 + eps);
}

/** OKLCH → sRGB, reducing chroma (not lightness or hue) to fit the gamut. */
export function fromOklch({ l, c, h }: Oklch): Omit<RGBA, "a"> {
  const L = clamp(l, 0, 1);
  const rad = (h * Math.PI) / 180;
  const ab = (ch: number): [number, number] => [ch * Math.cos(rad), ch * Math.sin(rad)];
  let [A, B] = ab(c);
  if (!inGamut(L, A, B)) {
    let lo = 0;
    let hi = c;
    for (let i = 0; i < 18; i++) {
      const mid = (lo + hi) / 2;
      const [a2, b2] = ab(mid);
      if (inGamut(L, a2, b2)) lo = mid;
      else hi = mid;
    }
    [A, B] = ab(lo);
  }
  return fromOklab(L, A, B);
}

// ---------------------------------------------------------------------------
// Contrast (WCAG 2)
// ---------------------------------------------------------------------------

export function luminance(c: RGBA): number {
  return 0.2126 * toLinear(c.r) + 0.7152 * toLinear(c.g) + 0.0722 * toLinear(c.b);
}

export function contrast(a: RGBA, b: RGBA): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** `top` painted over an opaque `bottom`. */
export function over(top: RGBA, bottom: RGBA): RGBA {
  const a = top.a;
  if (a >= 0.999) return { ...top, a: 1 };
  return {
    r: top.r * a + bottom.r * (1 - a),
    g: top.g * a + bottom.g * (1 - a),
    b: top.b * a + bottom.b * (1 - a),
    a: 1,
  };
}

export function sameColor(a: RGBA, b: RGBA): boolean {
  return Math.abs(a.r - b.r) < 0.5 && Math.abs(a.g - b.g) < 0.5 && Math.abs(a.b - b.b) < 0.5 && Math.abs(a.a - b.a) < 0.005;
}

export function isDark(c: RGBA): boolean {
  return toOklch(c).l < 0.5;
}

// ---------------------------------------------------------------------------
// The mapping
// ---------------------------------------------------------------------------

const CANVAS_L = toOklch(DARK_CANVAS).l;
/** Backgrounds below this lightness already read as dark. */
const DARK_L = 0.5;
/** Near-black neutral fills (black buttons, dark bands) are lifted to this
 * so they still stand out from the dark canvas. */
const LIFTED_L = 0.33;
/** Chroma above this is a brand color; it keeps its fill unless very light. */
const VIVID_C = 0.1;
const VIVID_MAX_L = 0.8;
/** Text on its background below this contrast was meant to be invisible
 * (preheaders, spacer text) and stays that way. */
const HIDDEN_CONTRAST = 1.4;
const AA = 4.5;

/**
 * The dark replacement for a background color, or null to keep it. Light
 * fills flip to dark ones of the same hue (white becomes the canvas, light
 * grays and tints become slightly raised dark surfaces). Dark fills, and
 * vivid mid-tone ones like a brand-colored button, keep their color. With
 * `lift` (the fill sat on a light background), a near-black neutral fill
 * such as a black button is raised a little so it still stands out from
 * the dark canvas.
 */
export function darkBackground(c: RGBA, lift = false): RGBA | null {
  if (c.a < 0.02) return null;
  const { l, c: chroma, h } = toOklch(c);
  if (l < DARK_L) {
    if (lift && chroma < 0.04 && l < LIFTED_L) return { ...fromOklch({ l: LIFTED_L, c: chroma, h }), a: c.a };
    return null;
  }
  if (chroma > VIVID_C && l < VIVID_MAX_L) return null;
  const nl = Math.min(CANVAS_L + (1 - l) * 0.6, 0.45);
  return { ...fromOklch({ l: nl, c: Math.min(chroma, 0.06), h }), a: c.a };
}

const COLOR_FN_RE = /\b(?:rgba?|color|oklab|oklch)\([^()]*\)/gi;

/**
 * A computed `background-image` that is only gradients of light colors
 * (a pale banner or card), darkened stop by stop like a background color.
 * `before` and `after` are the average stop colors, for judging text on it.
 * Null (keep it as it is) when it has an image, a dark or vivid stop, or a
 * color we can't read.
 */
export function darkGradient(css: string): { css: string; before: RGBA; after: RGBA } | null {
  if (!/gradient\(/i.test(css) || /url\(/i.test(css)) return null;
  const stops = css.match(COLOR_FN_RE);
  if (!stops) return null;
  const before: RGBA[] = [];
  const after: RGBA[] = [];
  for (const s of stops) {
    const c = parseColor(s);
    const d = c && darkBackground(c);
    if (!c || !d) return null;
    before.push(over(c, WHITE));
    after.push(over(d, DARK_CANVAS));
  }
  let i = 0;
  const mapped = css.replace(COLOR_FN_RE, () => formatColor(darkBackground(parseColor(stops[i++])!)!));
  // Anything left that looks like a color (a keyword) wasn't mapped.
  if (/\b(?:white|black|transparent|currentcolor|#)/i.test(css.replace(COLOR_FN_RE, ""))) return null;
  return { css: mapped, before: mean(before), after: mean(after) };
}

function mean(cs: RGBA[]): RGBA {
  const n = cs.length;
  return {
    r: cs.reduce((s, c) => s + c.r, 0) / n,
    g: cs.reduce((s, c) => s + c.g, 0) / n,
    b: cs.reduce((s, c) => s + c.b, 0) / n,
    a: 1,
  };
}

/** The contrast text should have on its new background: at least AA, and
 * strong originals are compressed (21:1 → about 14:1) so black body text
 * doesn't glare as pure white, while their order is kept. */
export function textTarget(original: number): number {
  if (original <= AA) return AA;
  return AA * Math.pow(Math.min(original, 21) / AA, 0.74);
}

/**
 * The same hue and chroma at the lightness that gives `target` contrast on
 * `bg`, on the side of `bg` that `lighter` picks. The best it can do when
 * that contrast is out of reach.
 */
export function withContrast(fg: RGBA, bg: RGBA, target: number, lighter: boolean): RGBA {
  const { c, h } = toOklch(fg);
  const at = (l: number): RGBA => ({ ...fromOklch({ l, c, h }), a: 1 });
  let lo = lighter ? toOklch(bg).l : 0;
  let hi = lighter ? 1 : toOklch(bg).l;
  // Contrast grows toward the far end; find the lightness closest to bg
  // that still reaches the target.
  for (let i = 0; i < 22; i++) {
    const mid = (lo + hi) / 2;
    const ok = contrast(at(mid), bg) >= target;
    if (lighter === ok) hi = mid;
    else lo = mid;
  }
  return at(lighter ? hi : lo);
}

/**
 * A foreground color (text or a border) whose background changed from
 * `oldBg` to `newBg` (both opaque). Keeps it when its background didn't
 * change, or when `keepIfReadable` and it reads at least as well as it
 * should; otherwise moves it to the other side of the new background at
 * about its old contrast (`target` maps the old ratio to the new one). The
 * result is opaque: a translucent color is judged as it looked.
 */
export function adjustForeground(
  fg: RGBA,
  oldBg: RGBA,
  newBg: RGBA,
  target: (old: number) => number,
  keepIfReadable: boolean,
): RGBA | null {
  if (fg.a < 0.02 || sameColor(oldBg, newBg)) return null;
  const want = target(contrast(over(fg, oldBg), oldBg));
  if (keepIfReadable && contrast(over(fg, newBg), newBg) >= want) return null;
  return withContrast(fg, newBg, want, luminance(newBg) < 0.18);
}

/** Text; text that was invisible on its old background (a preheader in
 * white on white) takes the new background's color so it stays that way. */
export function adjustText(fg: RGBA, oldBg: RGBA, newBg: RGBA): RGBA | null {
  if (fg.a >= 0.02 && !sameColor(oldBg, newBg) && contrast(over(fg, oldBg), oldBg) < HIDDEN_CONTRAST) {
    return sameColor(fg, newBg) ? null : { ...newBg, a: fg.a };
  }
  return adjustForeground(fg, oldBg, newBg, textTarget, true);
}

/** Borders keep their old contrast with what's around them (a light
 * hairline stays a faint one instead of turning into a bright line), capped
 * so dark rules don't turn into bright bars. */
export function adjustBorder(fg: RGBA, oldBg: RGBA, newBg: RGBA): RGBA | null {
  return adjustForeground(fg, oldBg, newBg, (old) => Math.min(Math.max(old, 1.2), 3), false);
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}
