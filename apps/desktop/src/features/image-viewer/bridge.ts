// The trusted bridge between a picture in a message body and the image
// viewer. See docs/SECURITY.md → "Image viewer".
//
// A message body is a sandboxed srcdoc iframe with no script, and WebKit
// never runs parent listeners for its clicks. So after a body loads, the
// app (decorate.ts, from MessageBody) wraps each picture in an anchor of its
// own: `penguin-image://open/<nonce>/<index>`, target=_blank. A click on it
// arrives here one of two ways:
//  - macOS/WebKit: as a new-window navigation that the Rust link guard
//    (src-tauri/src/image_viewer.rs) cancels, validates and re-emits as the
//    `penguin://image-open` event `{ nonce, index }`;
//  - Chromium (the browser mock): through MessageBody's in-frame click
//    listener, which cancels the navigation and passes the href.
//
// Whatever the path, the request is only honored when it names a nonce this
// module minted for a frame that is still mounted with the same document,
// and an index the app assigned there to an <img> still in that document
// whose source is one the sanitized document was allowed to show. The email
// can't produce such a URL at all (the sanitizer keeps only http/https/mailto
// hrefs); the nonce and index checks mean even a forged event can do nothing
// but open a picture that is already on screen.
//
// No DOM access at import time: tests run this module in Node.

import type { MessageView } from "../../lib/types";

export const IMAGE_SCHEME = "penguin-image";
/** Upper bound on pictures decorated per message (and the index grammar: 4 digits). */
export const MAX_FRAME_IMAGES = 200;
const MAX_INDEX = 9999;

const NONCE_RE = /^[0-9a-f]{32}$/;
const URL_RE = /^penguin-image:\/\/open\/([0-9a-f]{32})\/(0|[1-9][0-9]{0,3})$/;

export interface ImageRequest {
  nonce: string;
  index: number;
}

/** 128 random bits, lowercase hex: one per frame document. */
export function newNonce(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

export function imageUrl(nonce: string, index: number): string {
  if (!NONCE_RE.test(nonce) || !Number.isInteger(index) || index < 0 || index > MAX_INDEX) throw new Error("bad image request");
  return `${IMAGE_SCHEME}://open/${nonce}/${index}`;
}

/**
 * The request schema, strictly: either the href of one of our anchors or the
 * Rust event's payload, an object with exactly `nonce` (32 lowercase hex) and
 * `index` (an integer 0–9999). Anything else is null.
 */
export function parseImageRequest(input: unknown): ImageRequest | null {
  if (typeof input === "string") {
    if (input.length > 80) return null;
    const m = URL_RE.exec(input);
    return m ? { nonce: m[1], index: Number(m[2]) } : null;
  }
  if (!input || typeof input !== "object" || Array.isArray(input)) return null;
  if (Object.getPrototypeOf(input) !== Object.prototype) return null;
  const keys = Object.keys(input).sort();
  if (keys.length !== 2 || keys[0] !== "index" || keys[1] !== "nonce") return null;
  const { nonce, index } = input as { nonce: unknown; index: unknown };
  if (typeof nonce !== "string" || !NONCE_RE.test(nonce)) return null;
  if (typeof index !== "number" || !Number.isInteger(index) || index < 0 || index > MAX_INDEX) return null;
  return { nonce, index };
}

/** The data: image types penguin-render embeds (never SVG). */
const DATA_IMAGE_RE = /^data:image\/(png|jpeg|gif|webp|bmp|avif|x-icon);base64,/i;
const BASE64_RE = /^[A-Za-z0-9+/]*={0,2}$/;

/**
 * A picture source the viewer may show: a base64 raster data: URL (an inline
 * cid: part the renderer embedded) or a plain https URL. The latter is only
 * in the document when the user's image setting let remote images load, and
 * `resolveImageRequest` also requires that it actually loaded in the frame.
 */
export function isViewableSrc(src: unknown): src is string {
  if (typeof src !== "string" || src.length === 0) return false;
  const m = DATA_IMAGE_RE.exec(src);
  if (m) return BASE64_RE.test(src.slice(m[0].length));
  if (!/^https:\/\//i.test(src)) return false;
  try {
    const u = new URL(src);
    return u.protocol === "https:" && !u.username && !u.password && !!u.hostname;
  } catch {
    return false;
  }
}

export const isRemoteSrc = (src: string) => /^https:/i.test(src);

// ---------------------------------------------------------------------------
// Registry of decorated frames
// ---------------------------------------------------------------------------

/** The parts of the DOM the registry reads (duck-typed so tests can fake them). */
export interface ImgLike {
  readonly isConnected: boolean;
  readonly ownerDocument: unknown;
  readonly complete: boolean;
  readonly naturalWidth: number;
  readonly naturalHeight: number;
  readonly currentSrc: string;
  getAttribute(name: string): string | null;
}

export interface FrameEntry<I extends ImgLike = HTMLImageElement> {
  nonce: string;
  /** The srcdoc document the anchors were written into. */
  doc: unknown;
  frame: { readonly isConnected: boolean; readonly contentDocument: unknown; compareDocumentPosition?(other: Node): number };
  /** The message shown in the frame (latest render). */
  message: () => MessageView;
  /** Index = the number in that picture's anchor. Only ever appended to. */
  images: I[];
}

const frames = new Map<string, FrameEntry<ImgLike>>();

export function registerFrame<I extends ImgLike>(entry: FrameEntry<I>): () => void {
  frames.set(entry.nonce, entry as unknown as FrameEntry<ImgLike>);
  return () => {
    if (frames.get(entry.nonce) === (entry as unknown)) frames.delete(entry.nonce);
  };
}

function live(f: FrameEntry<ImgLike>): boolean {
  return f.frame.isConnected && f.frame.contentDocument === f.doc;
}

/** The source an <img> shows, if the viewer may show it too. */
export function imageSource(img: ImgLike, doc: unknown): string | null {
  if (!img.isConnected || img.ownerDocument !== doc) return null;
  const src = img.currentSrc || img.getAttribute("src") || "";
  if (!isViewableSrc(src)) return null;
  // Only a picture that really rendered: a remote one proves the frame's own
  // CSP and the image setting let it load.
  if (!img.complete || img.naturalWidth === 0) return null;
  return src;
}

/**
 * The frame and picture a request names, or null. `from`: the document a
 * click was seen in (the Chromium listener path); the nonce must be that
 * document's own, so one message can't open another's pictures.
 */
export function resolveImageRequest(raw: unknown, from?: { doc: unknown }): { entry: FrameEntry<ImgLike>; index: number; src: string } | null {
  const req = parseImageRequest(raw);
  if (!req) return null;
  const entry = frames.get(req.nonce);
  if (!entry || !live(entry)) return null;
  if (from && from.doc !== entry.doc) return null;
  const img = entry.images[req.index];
  if (!img) return null;
  const src = imageSource(img, entry.doc);
  return src ? { entry, index: req.index, src } : null;
}

/** Mounted frames, in on-screen (document) order. */
export function framesInOrder(): FrameEntry<ImgLike>[] {
  const list = [...frames.values()].filter(live);
  return list.sort((a, b) => {
    const pos = a.frame.compareDocumentPosition?.(b.frame as unknown as Node) ?? 0;
    return pos & 4 /* FOLLOWING */ ? -1 : pos & 2 /* PRECEDING */ ? 1 : 0;
  });
}

/** Tests only. */
export function _resetFrames() {
  frames.clear();
}
