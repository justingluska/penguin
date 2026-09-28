// "Dark email bodies" (Settings → General, experimental): darkens an HTML
// message's document in place, from the parent. OWNER: security-render agent.
//
// No script runs in the message frame, so this runs in the app, through the
// same-origin access MessageBody already uses to measure the frame. It only
// ever writes inline color properties it computed itself (rgb() strings),
// the media attribute of penguin-render's dark sheet and one data attribute
// on <html>; everything it changes is recorded so restore() puts it back.
//
// Three cases, in order:
// 1. The message supports a dark canvas (it ships
//    `@media (prefers-color-scheme: dark)` rules or declares
//    `color-scheme: … dark`; penguin-render then emits its
//    `<style class="pg-dark-css" media="not all">`): switch that sheet on and
//    put the base styles on their dark canvas. The sender's own dark design
//    beats anything we could compute; the pass below then only darkens what
//    their rules left light.
// 2. The message is already mostly dark: leave it alone.
// 3. Otherwise, adapt it: one pass reads every element's computed colors,
//    then darkColors.ts maps light fills (and all-light gradients) to dark
//    ones of the same hue and moves text and borders to the light side of
//    their new background at about their old contrast. Images are never
//    touched, and neither is anything drawn on a background image, since
//    its colors can't be known.
//
// We chose this over the `filter: invert(1) hue-rotate(180deg)` trick: that
// shifts brand colors (hue-rotate is only an approximation), turns white into
// pure black, needs every image and background image re-inverted (and breaks
// when they nest), and makes WebKit composite the whole, possibly
// 40,000 px tall, document through a filter.

import {
  DARK_CANVAS,
  WHITE,
  adjustBorder,
  adjustText,
  darkBackground,
  darkGradient,
  formatColor,
  isDark,
  over,
  parseColor,
  type RGBA,
} from "./darkColors";

/** What darken() did: used the message's own dark styles, adapted its
 * colors, or left it alone because it is already dark or too large. */
export type DarkMode = "native" | "adapted" | "dark" | "skipped";

/** Past this many elements the pass is skipped (the message stays light);
 * real newsletters have a few thousand at most. */
const MAX_ELEMENTS = 20000;
/** Share of the painted background area that must be dark for a message to
 * count as already dark. */
const DARK_SHARE = 0.6;
const SIDES = ["top", "right", "bottom", "left"] as const;
/** Leaf elements without text or colors of their own worth changing. */
const SKIP = new Set(["IMG", "BR", "WBR", "HEAD", "STYLE", "META", "TITLE"]);

interface Change {
  style: CSSStyleDeclaration;
  prop: string;
  value: string;
  priority: string;
}

interface Applied {
  mode: DarkMode;
  changes: Change[];
  sheet: HTMLStyleElement | null;
}

const applied = new WeakMap<Document, Applied>();

interface Rec {
  el: HTMLElement;
  parent: number;
  color: RGBA | null;
  bg: RGBA | null;
  /** Computed background-image, "" for none. Unless it is an all-light
   * gradient, the element and everything in it keep their colors. */
  image: string;
  borders: (RGBA | null)[];
  area: number;
}

/** Darken `doc` (idempotent). Returns what it did, or null when the
 * document can't be read. */
export function darken(doc: Document): DarkMode | null {
  const prev = applied.get(doc);
  if (prev) return prev.mode;
  const view = doc.defaultView;
  const root = doc.documentElement;
  if (!view || !root) return null;

  const sheet = doc.head?.querySelector<HTMLStyleElement>("style.pg-dark-css") ?? null;
  const changes: Change[] = [];
  const done = (mode: DarkMode): DarkMode => {
    applied.set(doc, { mode, changes, sheet });
    return mode;
  };
  const tooLarge = root.getElementsByTagName("*").length > MAX_ELEMENTS;
  if (sheet) {
    sheet.media = "all";
    root.dataset.pgScheme = "dark";
    // The sender's dark rules can't reach everything: <body> colors live on
    // our .pg-root wrapper, and some light-mode inline colors have no dark
    // override. The adapting pass then darkens only what is still light.
    if (!tooLarge) adapt(collect(root, view), changes, false);
    return done("native");
  }
  if (tooLarge) return done("skipped");
  const recs = collect(root, view);
  if (darkShare(recs) >= DARK_SHARE) return done("dark");
  write(root, "color-scheme", "dark", changes);
  adapt(recs, changes, true);
  return done("adapted");
}

/** Undo darken(); a no-op for a document it never touched. */
export function restore(doc: Document): void {
  const done = applied.get(doc);
  if (!done) return;
  applied.delete(doc);
  if (done.sheet) {
    done.sheet.media = "not all";
    delete doc.documentElement.dataset.pgScheme;
  }
  for (let i = done.changes.length - 1; i >= 0; i--) {
    const c = done.changes[i];
    if (c.value) c.style.setProperty(c.prop, c.value, c.priority);
    else c.style.removeProperty(c.prop);
  }
}

/** Every element's colors, parents before children. */
function collect(root: HTMLElement, view: Window): Rec[] {
  const recs: Rec[] = [];
  const stack: [Element, number][] = [[root, -1]];
  while (stack.length) {
    const [el, parent] = stack.pop()!;
    if (SKIP.has(el.tagName) || !("style" in el)) continue;
    const cs = view.getComputedStyle(el);
    const bg = parseColor(cs.backgroundColor);
    const borders = SIDES.map((s) =>
      cs.getPropertyValue(`border-${s}-style`) !== "none" && parseFloat(cs.getPropertyValue(`border-${s}-width`)) > 0
        ? parseColor(cs.getPropertyValue(`border-${s}-color`))
        : null,
    );
    let area = 0;
    if (bg && bg.a >= 0.5) {
      const r = el.getBoundingClientRect();
      area = r.width * r.height;
    }
    const index = recs.length;
    recs.push({
      el: el as HTMLElement,
      parent,
      color: parseColor(cs.color),
      bg: bg && bg.a > 0 ? bg : null,
      image: cs.backgroundImage === "none" ? "" : cs.backgroundImage,
      borders,
      area,
    });
    for (let c = el.lastElementChild; c; c = c.previousElementSibling) stack.push([c, index]);
  }
  return recs;
}

/** Share of the visible background area (each fill minus the fills painted
 * over it) that is dark. */
function darkShare(recs: Rec[]): number {
  const visible = recs.map((r) => r.area);
  const fillParent = new Int32Array(recs.length).fill(-1);
  recs.forEach((r, i) => {
    if (r.parent < 0) return;
    const p = recs[r.parent].area > 0 ? r.parent : fillParent[r.parent];
    fillParent[i] = p;
    if (r.area > 0 && p >= 0) visible[p] -= r.area;
  });
  let dark = 0;
  let total = 0;
  recs.forEach((r, i) => {
    if (r.area <= 0 || !r.bg) return;
    const v = Math.max(0, visible[i]);
    total += v;
    if (isDark(over(r.bg, WHITE))) dark += v;
  });
  return total > 0 ? dark / total : 0;
}

/** `lift`: raise near-black fills that sat on a light background off the
 * dark canvas (not for a sender's own dark design). */
function adapt(recs: Rec[], changes: Change[], lift: boolean): void {
  // Effective (opaque) background behind each element, before and after.
  const oldBg: RGBA[] = new Array(recs.length);
  const newBg: RGBA[] = new Array(recs.length);
  const island = new Uint8Array(recs.length);
  // Mail repeats a handful of color combinations; the contrast searches
  // are the expensive part, so each combination is solved once.
  const memo = new Map<string, string | null>();
  const key = (c: RGBA) => `${c.r},${c.g},${c.b},${c.a}`;
  const cached = (k: string, f: () => RGBA | null): string | null => {
    let v = memo.get(k);
    if (v === undefined) {
      const c = f();
      v = c ? formatColor(c) : null;
      memo.set(k, v);
    }
    return v;
  };
  recs.forEach((r, i) => {
    const p = r.parent;
    const pOld = p < 0 ? WHITE : oldBg[p];
    const pNew = p < 0 ? DARK_CANVAS : newBg[p];
    const gradient = r.image && !(p >= 0 && island[p]) ? darkGradient(r.image) : null;
    if ((p >= 0 && island[p]) || (r.image && !gradient)) {
      island[i] = 1;
      oldBg[i] = newBg[i] = r.bg ? over(r.bg, pOld) : pOld;
      return;
    }
    let own = r.bg;
    if (r.bg) {
      const mapped = darkBackground(r.bg, lift && p >= 0 && !isDark(oldBg[p]));
      if (mapped) {
        write(r.el, "background-color", formatColor(mapped), changes);
        own = mapped;
      }
    }
    oldBg[i] = r.bg ? over(r.bg, pOld) : pOld;
    newBg[i] = own ? over(own, pNew) : pNew;
    if (gradient) {
      write(r.el, "background-image", gradient.css, changes);
      oldBg[i] = gradient.before;
      newBg[i] = gradient.after;
    }
    const fg = r.color;
    if (fg) {
      const c = cached(`t${key(fg)}|${key(oldBg[i])}|${key(newBg[i])}`, () => adjustText(fg, oldBg[i], newBg[i]));
      if (c) write(r.el, "color", c, changes);
    }
    r.borders.forEach((b, s) => {
      const c = b && cached(`b${key(b)}|${key(pOld)}|${key(pNew)}`, () => adjustBorder(b, pOld, pNew));
      if (c) write(r.el, `border-${SIDES[s]}-color`, c, changes);
    });
  });
}

function write(el: HTMLElement, prop: string, value: string, changes: Change[]): void {
  const style = el.style;
  changes.push({ style, prop, value: style.getPropertyValue(prop), priority: style.getPropertyPriority(prop) });
  style.setProperty(prop, value, "important");
}
