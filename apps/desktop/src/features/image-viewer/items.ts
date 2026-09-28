// What the image viewer steps through: the pictures in each open message's
// body, then that message's image attachments, message by message in thread
// order. Pure (no DOM), so tests can run it in Node.

import type { AttachmentMeta, MessageView, SaveAllItem } from "../../lib/types";
import { framesInOrder, imageSource, isRemoteSrc, type FrameEntry, type ImgLike } from "./bridge.ts";

export interface BodyImage {
  /** Index in the frame (the number in its viewer anchor). */
  index: number;
  src: string;
  width: number;
  height: number;
  alt: string;
}

export type ViewerItem =
  | {
      kind: "body";
      key: string;
      message: MessageView;
      src: string;
      remote: boolean;
      width: number;
      height: number;
      name: string;
      /** Bytes, when known without a fetch (an embedded picture). */
      size: number | null;
      /** The inline part an embedded picture came from, when it can be told. */
      attachment: AttachmentMeta | null;
    }
  | { kind: "attachment"; key: string; message: MessageView; attachment: AttachmentMeta; name: string; size: number };

/** Image formats the attachment preview renders (sniffed in Rust). */
const VIEWABLE_EXT = /\.(png|jpe?g|gif|webp)$/i;
const VIEWABLE_MIME = /^image\/(png|jpeg|pjpeg|gif|webp)$/i;

/** An attachment the viewer shows instead of the file preview panel. */
export function isViewableAttachment(a: AttachmentMeta): boolean {
  return VIEWABLE_MIME.test(a.mimeType) || VIEWABLE_EXT.test(a.filename);
}

const messageKey = (m: MessageView) => `${m.accountId}/${m.id}`;
export const attachmentKey = (m: MessageView, a: AttachmentMeta) => `att:${messageKey(m)}/${a.id}`;
export const bodyKey = (m: MessageView, index: number) => `body:${messageKey(m)}/${index}`;

/** Decoded size of a base64 data: URL, without decoding it. */
export function dataUrlBytes(src: string): number | null {
  const comma = src.indexOf(",");
  if (!src.startsWith("data:") || comma < 0 || !src.slice(0, comma).endsWith(";base64")) return null;
  const n = src.length - comma - 1;
  const pad = src.endsWith("==") ? 2 : src.endsWith("=") ? 1 : 0;
  return Math.max(0, Math.floor((n * 3) / 4) - pad);
}

export function dataUrlMime(src: string): string | null {
  const m = /^data:(image\/[a-z0-9.+-]+);base64,/i.exec(src);
  return m ? m[1].toLowerCase() : null;
}

const EXT: Record<string, string> = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/bmp": "bmp", "image/avif": "avif", "image/x-icon": "ico" };

/**
 * The inline part an embedded picture came from: the only inline image
 * attachment with that exact byte size (and a compatible type). The renderer
 * turns `cid:` into data: URLs, so this is how Save keeps the real file name.
 */
export function matchInlinePart(m: MessageView, src: string): AttachmentMeta | null {
  const size = dataUrlBytes(src);
  const mime = dataUrlMime(src);
  if (size == null) return null;
  const hits = m.attachments.filter((a) => (a.inline || a.contentId) && a.size === size && (!mime || !a.mimeType.startsWith("image/") || a.mimeType.toLowerCase() === mime));
  return hits.length === 1 ? hits[0] : null;
}

/** A readable file name for a picture from its URL (never the query). */
export function nameFromUrl(src: string): string {
  if (!isRemoteSrc(src)) return "";
  try {
    const last = new URL(src).pathname.split("/").filter(Boolean).pop() ?? "";
    const name = decodeURIComponent(last).replace(/[\u0000-\u001f/\\:]/g, "_").trim();
    return name.length > 0 && name.length <= 120 ? name : "";
  } catch {
    return "";
  }
}

function bodyName(img: BodyImage, part: AttachmentMeta | null): string {
  if (part) return part.filename;
  const fromUrl = nameFromUrl(img.src);
  if (fromUrl) return fromUrl;
  const alt = img.alt.replace(/\s+/g, " ").trim().slice(0, 60).replace(/[/\\:]/g, "_");
  const mime = dataUrlMime(img.src);
  const ext = (mime && EXT[mime]) || "png";
  if (alt) return /\.[a-z0-9]{2,5}$/i.test(alt) ? alt : `${alt}.${ext}`;
  return `image-${img.index + 1}.${ext}`;
}

/** An image attachment as a viewer item (its card's menu uses the same actions). */
export function attachmentItem(m: MessageView, a: AttachmentMeta): ViewerItem {
  return { kind: "attachment", key: attachmentKey(m, a), message: m, attachment: a, name: a.filename, size: a.size };
}

export interface MessageImages {
  message: MessageView;
  /** Pictures in its rendered body (empty when the body isn't open). */
  body: BodyImage[];
}

export function buildItems(groups: MessageImages[]): ViewerItem[] {
  const out: ViewerItem[] = [];
  const seen = new Set<string>();
  for (const { message: m, body } of groups) {
    if (seen.has(messageKey(m))) continue;
    seen.add(messageKey(m));
    // An attachment a body picture already shows (its inline part) is listed once.
    const shown = new Set<string>();
    for (const img of body) {
      const part = isRemoteSrc(img.src) ? null : matchInlinePart(m, img.src);
      out.push({
        kind: "body",
        key: bodyKey(m, img.index),
        message: m,
        src: img.src,
        remote: isRemoteSrc(img.src),
        width: img.width,
        height: img.height,
        name: bodyName(img, part),
        size: part?.size ?? dataUrlBytes(img.src),
        attachment: part,
      });
      if (part) shown.add(part.id);
    }
    for (const a of m.attachments) {
      if (a.inline || shown.has(a.id) || !isViewableAttachment(a)) continue;
      out.push(attachmentItem(m, a));
    }
  }
  return out;
}

/** The pictures a decorated frame shows, in anchor order (loaded and viewable only). */
export function frameBodyImages(f: Pick<FrameEntry<ImgLike>, "images" | "doc">): BodyImage[] {
  const out: BodyImage[] = [];
  f.images.forEach((img, index) => {
    const src = imageSource(img, f.doc);
    if (src) out.push({ index, src, width: img.naturalWidth, height: img.naturalHeight, alt: img.getAttribute("alt") ?? "" });
  });
  return out;
}

// ---------------------------------------------------------------------------
// Save all images (one message)
// ---------------------------------------------------------------------------

/** "Save all images" shows from this many pictures up. */
export const SAVE_ALL_MIN = 2;

/** One message's pictures, as the viewer lists them (body pictures, then image attachments). */
export function messageItems(m: MessageView, body: BodyImage[]): ViewerItem[] {
  return buildItems([{ message: m, body }]);
}

/** A message's pictures as they are on screen now: its open body's (if any) and its image attachments. */
export function onScreenMessageItems(m: MessageView): ViewerItem[] {
  const frame = framesInOrder().find((f) => {
    const fm = f.message();
    return fm.accountId === m.accountId && fm.id === m.id;
  });
  return messageItems(m, frame ? frameBodyImages(frame) : []);
}

/** Whether a message's pictures get "Save all images". */
export const canSaveAll = (items: ViewerItem[]) => items.length >= SAVE_ALL_MIN;

/**
 * What save_message_images gets: an attachment (or an embedded picture's
 * identified inline part) by id, any other picture by its source. Rust
 * checks each one again, as it does for a single save.
 */
export function saveAllItems(items: ViewerItem[]): SaveAllItem[] {
  const out: SaveAllItem[] = [];
  const ids = new Set<string>();
  for (const it of items) {
    const a = it.attachment;
    if (a) {
      if (!ids.has(a.id)) out.push({ kind: "attachment", attachmentId: a.id });
      ids.add(a.id);
    } else if (it.kind === "body") out.push({ kind: "body", src: it.src, name: it.name });
  }
  return out;
}
