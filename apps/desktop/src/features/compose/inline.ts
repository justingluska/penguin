// Inline images in the composer: an image pasted or dropped into the body is
// shown there and sent as a multipart/related part the HTML references by
// `cid:` (the backend decides the MIME: penguin-provider inline.rs). It is
// an attachment with a `contentId`; the editor holds an `inlineImage` node
// with the same id. Pure (no api, no DOM) so tests/inlineImages.test.ts
// runs it directly.
import type { OutgoingAttachment } from "../../lib/types";

/** Image types shown and sent inline (what the backend sends inline). */
export const INLINE_IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp", "image/avif"];

/** Bigger images are attached as files instead of shown in the message. */
export const MAX_INLINE_IMAGE_BYTES = 10 * 1024 * 1024;

/** An image wider than this is sent (and shown) at this width. */
export const INLINE_MAX_WIDTH = 600;

/** Ids the backend writes into HTML and Content-ID headers (penguin-render is_safe_cid). */
const SAFE_CID = /^[A-Za-z0-9._@+=-]{1,200}$/;

export function isSafeCid(cid: string | null | undefined): cid is string {
  return !!cid && SAFE_CID.test(cid);
}

export function isInlineImageType(mime: string): boolean {
  return INLINE_IMAGE_TYPES.includes(mime.trim().toLowerCase());
}

function hex(n: number): string {
  const bytes = new Uint8Array(n);
  const c = (globalThis as { crypto?: Crypto }).crypto;
  if (c?.getRandomValues) c.getRandomValues(bytes);
  else for (let i = 0; i < n; i++) bytes[i] = Math.floor(Math.random() * 256);
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** A fresh Content-ID, the same shape the backend mints (random, nothing about the machine). */
export function newContentId(): string {
  return `img-${hex(12)}@penguin`;
}

/** The Content-IDs an HTML part shows (`<img src="cid:…">`). */
export function referencedCids(html: string | null | undefined): Set<string> {
  const out = new Set<string>();
  if (!html) return out;
  for (const m of html.matchAll(/src="cid:([^"]+)"/gi)) out.add(m[1]);
  return out;
}

/** The attachment's inline image id, if it has one. */
export function contentIdOf(a: OutgoingAttachment): string | undefined {
  return a.contentId || undefined;
}

/**
 * What goes out with `html`: files, plus the inline images it still shows.
 * An image deleted from the body stays in the state (so ⌘Z brings it back)
 * but isn't sent.
 */
export function sendableAttachments(list: OutgoingAttachment[], html: string | null | undefined): OutgoingAttachment[] {
  const shown = referencedCids(html);
  return list.filter((a) => !a.contentId || shown.has(a.contentId));
}

/** "Send as attachment": the image with `cid` becomes an ordinary file. */
export function asFile(list: OutgoingAttachment[], cid: string): OutgoingAttachment[] {
  return list.map((a) => {
    if (a.contentId !== cid) return a;
    const { contentId: _drop, ...rest } = a;
    void _drop;
    return rest as OutgoingAttachment;
  });
}

/** The files the attachment row shows (inline images live in the body), with their index in `list`. */
export function fileRows(list: OutgoingAttachment[]): Array<{ a: OutgoingAttachment; i: number }> {
  return list.flatMap((a, i) => (a.contentId ? [] : [{ a, i }]));
}

/** The width an image goes in at: its own, capped at INLINE_MAX_WIDTH; null when unknown. */
export function inlineWidth(naturalWidth: number): number | null {
  return naturalWidth > 0 ? Math.min(Math.round(naturalWidth), INLINE_MAX_WIDTH) : null;
}

/**
 * Split pasted or dropped files: images that go in the body, images too big
 * for that (attached instead), and everything else (attached).
 */
export function splitDropped<F extends { type: string; size: number }>(files: F[]): { inline: F[]; tooBig: F[]; files: F[] } {
  const inline: F[] = [];
  const tooBig: F[] = [];
  const rest: F[] = [];
  for (const f of files) {
    if (!isInlineImageType(f.type)) rest.push(f);
    else if (f.size > MAX_INLINE_IMAGE_BYTES) tooBig.push(f);
    else inline.push(f);
  }
  return { inline, tooBig, files: [...tooBig, ...rest] };
}
