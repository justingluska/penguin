// Makes the pictures in a loaded message body open the image viewer.
// Runs in the app's realm against the same-origin, script-less frame
// document (like the auto-height and dark-body passes); nothing here runs
// inside the frame. See bridge.ts for how a click comes back.
//
// - A picture that isn't inside a link is wrapped in our own anchor, styled
//   `display: contents` so it adds no box and the layout doesn't move. A
//   click anywhere on the picture opens the viewer, and right-click still
//   offers the native image items.
// - A picture inside the sender's link keeps its click for the link (what
//   Gmail, Apple Mail and Superhuman do). It gets a small expand button in
//   its top-right corner instead, shown on hover with CSS alone (`:has`),
//   placed in a zero-size layer at the end of the body so nothing reflows.
// - Tiny pictures (icons, spacers, social badges) are left alone, and so is
//   anything not yet loaded; the next pass (after a layout change or late
//   image load) picks those up.

import { imageSource, imageUrl, MAX_FRAME_IMAGES } from "./bridge";

/** Smaller rendered pictures aren't worth a viewer. */
export const MIN_SIDE = 40;
const ZOOM_SIZE = 26;
const ZOOM_INSET = 6;
const IMG_ATTR = "data-pg-iv-img";
const WRAP_ATTR = "data-pg-iv";
const ZOOM_ATTR = "data-pg-iv-zoom";
const LAYER_TAG = "pg-iv-layer";
const STYLE_ID = "pg-iv-style";

// Specific selectors and !important: the email's own stylesheet can't
// reach data-pg-* in markup (the sanitizer drops data-* attributes), but
// its broad rules (`a { … !important }`) must not restyle ours.
const BASE_CSS = `
html a[${WRAP_ATTR}]{display:contents!important;cursor:zoom-in!important}
html a[${WRAP_ATTR}]>img{cursor:zoom-in!important}
html ${LAYER_TAG}{display:block!important;position:relative!important;width:0!important;height:0!important;margin:0!important;padding:0!important;border:0!important;float:none!important;overflow:visible!important}
html ${LAYER_TAG}>a[${ZOOM_ATTR}]{position:absolute!important;box-sizing:border-box!important;display:flex!important;align-items:center!important;justify-content:center!important;width:${ZOOM_SIZE}px!important;height:${ZOOM_SIZE}px!important;margin:0!important;padding:0!important;border:0!important;border-radius:7px!important;background:rgba(18,18,20,.74)!important;color:#fff!important;box-shadow:0 1px 4px rgba(0,0,0,.28)!important;text-decoration:none!important;cursor:zoom-in!important;opacity:0!important;transition:opacity 140ms cubic-bezier(.2,.8,.2,1)!important;z-index:2147483647!important}
html ${LAYER_TAG}>a[${ZOOM_ATTR}]:hover{opacity:1!important;background:rgba(18,18,20,.9)!important}
html ${LAYER_TAG}>a[${ZOOM_ATTR}]>svg{width:15px!important;height:15px!important;display:block!important;fill:none!important;stroke:currentColor!important;stroke-width:2!important;stroke-linecap:round!important;stroke-linejoin:round!important}
@media (prefers-reduced-motion: reduce){html ${LAYER_TAG}>a[${ZOOM_ATTR}]{transition:none!important}}
`;

// The app's "expand" icon (components/Icon.tsx).
const EXPAND_PATHS = ["M15 3h6v6", "M9 21H3v-6", "M21 3l-7 7", "M3 21l7-7"];

export interface DecorateState {
  nonce: string;
  images: HTMLImageElement[];
}

/**
 * One pass over the document: decorate pictures that became eligible and
 * re-place the corner buttons. Idempotent; cheap enough to run on every
 * layout change. Returns whether anything new was decorated.
 */
export function decorate(doc: Document, state: DecorateState): boolean {
  const imgs = doc.images;
  if (imgs.length === 0) return false;
  let added = false;
  let hoverRules = "";
  for (const img of Array.from(imgs)) {
    if (img.hasAttribute(IMG_ATTR) || state.images.length >= MAX_FRAME_IMAGES) continue;
    if (!imageSource(img, doc)) continue;
    const r = img.getBoundingClientRect();
    if (r.width < MIN_SIDE || r.height < MIN_SIDE) continue;
    const index = state.images.length;
    state.images.push(img);
    img.setAttribute(IMG_ATTR, String(index));
    const link = img.parentElement?.closest("a[href]");
    if (!link) {
      const a = anchor(doc, state.nonce, index, WRAP_ATTR);
      img.replaceWith(a);
      a.append(img);
    } else {
      const a = anchor(doc, state.nonce, index, ZOOM_ATTR);
      a.title = "View image";
      a.setAttribute("aria-label", "View image");
      a.append(expandIcon(doc));
      layer(doc).append(a);
      hoverRules += `html:has(img[${IMG_ATTR}="${index}"]:hover) ${LAYER_TAG}>a[${ZOOM_ATTR}="${index}"]{opacity:1!important}\n`;
    }
    added = true;
  }
  if (added) style(doc).textContent += hoverRules;
  place(doc, state);
  return added;
}

function anchor(doc: Document, nonce: string, index: number, attr: string): HTMLAnchorElement {
  const a = doc.createElement("a");
  a.setAttribute("href", imageUrl(nonce, index));
  // Like every link in a message: a click is a new-window request the
  // native link guard sees; nothing navigates the frame or the app.
  a.setAttribute("target", "_blank");
  a.setAttribute("rel", "noopener noreferrer");
  a.setAttribute(attr, String(index));
  return a;
}

function expandIcon(doc: Document): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = doc.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  for (const d of EXPAND_PATHS) {
    const p = doc.createElementNS(ns, "path");
    p.setAttribute("d", d);
    svg.append(p);
  }
  return svg;
}

function style(doc: Document): HTMLStyleElement {
  let s = doc.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (!s || s.tagName !== "STYLE") {
    s = doc.createElement("style");
    s.id = STYLE_ID;
    s.textContent = BASE_CSS;
    (doc.head ?? doc.documentElement).append(s);
  }
  return s;
}

/** The corner buttons' container: last child of the message root, so it
 * scrolls with a wide message and its (0×0) box changes no layout. */
function layer(doc: Document): HTMLElement {
  let l = doc.querySelector(LAYER_TAG) as HTMLElement | null;
  if (!l) {
    style(doc);
    l = doc.createElement(LAYER_TAG);
    ((doc.querySelector(".pg-root") as HTMLElement | null) ?? doc.body).append(l);
  }
  return l;
}

function place(doc: Document, state: DecorateState) {
  const l = doc.querySelector(LAYER_TAG);
  if (!l) return;
  const origin = l.getBoundingClientRect();
  for (const a of Array.from(l.children) as HTMLElement[]) {
    const img = state.images[Number(a.getAttribute(ZOOM_ATTR))];
    const r = img?.isConnected ? img.getBoundingClientRect() : null;
    if (!r || r.width < MIN_SIDE || r.height < MIN_SIDE) {
      a.style.setProperty("visibility", "hidden", "important");
      continue;
    }
    a.style.removeProperty("visibility");
    a.style.setProperty("left", `${Math.round(r.right - origin.left - ZOOM_SIZE - ZOOM_INSET)}px`, "important");
    a.style.setProperty("top", `${Math.round(r.top - origin.top + ZOOM_INSET)}px`, "important");
  }
}

/** The viewer anchor a click in the frame landed on, if any. */
export function viewerAnchor(target: EventTarget | null): HTMLAnchorElement | null {
  const el = target as Element | null;
  const a = el && typeof el.closest === "function" ? el.closest("a[href]") : null;
  return a && (a.hasAttribute(WRAP_ATTR) || a.hasAttribute(ZOOM_ATTR)) ? (a as HTMLAnchorElement) : null;
}
