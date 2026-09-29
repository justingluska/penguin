// What you can do with a picture: Copy, Copy original web address, Save to
// Downloads, Save As…, Open in Preview, Show in Finder. Shared by the
// viewer's toolbar, keys and right-click menu and by an image attachment's
// card menu in the thread. Bytes come from Rust (image_viewer.rs,
// file_export.rs) or the attachment commands; see docs/SECURITY.md →
// "Image viewer".

import type { AttachmentMeta, AttachmentPreview, MessageView } from "../../lib/types";
import { api, asCommandError, isDemo, isMock } from "../../lib/api";
import { isMac } from "../../lib/keyboard";
import { bytes } from "../../lib/format";
import { copyText, copyTextLater } from "../../lib/clipboard";
import { toast, updateToast } from "../../components/Toast";
import {
  downloadAttachment,
  loadPreview,
  revealSaved,
  saveAttachmentAs,
  savedAttachmentPath,
} from "../thread/AttachmentPreview";
import { saveAllItems, type ViewerItem } from "./items";

export const openLabel = isMac ? "Open in Preview" : "Open";
/**
 * Save As…, Show in Finder and file drags need the Mac app (a native panel,
 * Finder, AppKit). Demo mode shows them as the Mac app does; the mock
 * answers them without touching the disk.
 */
export const nativeFiles = isMac && (!isMock || isDemo);

/** The attachment behind an item: the attachment itself, or an embedded picture's inline part. */
export function attachmentOf(it: ViewerItem): AttachmentMeta | null {
  return it.kind === "attachment" ? it.attachment : it.attachment;
}

async function previewImage(m: MessageView, a: AttachmentMeta): Promise<AttachmentPreview & { dataUrl: string }> {
  const p = await loadPreview(m, a);
  if (p.kind !== "image" || !p.dataUrl) {
    throw new Error(p.reason === "tooLarge" ? `${bytes(p.size)} is over the 20 MB preview limit` : "This file isn't an image Penguin can show");
  }
  return p as AttachmentPreview & { dataUrl: string };
}
export { previewImage };

/** A data: URL of the picture, for Copy. */
async function dataUrlOf(it: ViewerItem): Promise<string> {
  const a = attachmentOf(it);
  if (it.kind === "attachment") return (await previewImage(it.message, it.attachment)).dataUrl;
  if (!it.remote) return it.src;
  if (a) return (await previewImage(it.message, a)).dataUrl;
  return (await api.fetchMessageImage(it.src)).dataUrl;
}

function decodeBase64(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** WebKit's clipboard takes image/png only: re-encode anything else. */
async function pngBlob(dataUrl: string): Promise<Blob> {
  if (/^data:image\/png;base64,/i.test(dataUrl)) return new Blob([decodeBase64(dataUrl.slice(dataUrl.indexOf(",") + 1))], { type: "image/png" });
  const img = new Image();
  img.src = dataUrl;
  await img.decode();
  const c = document.createElement("canvas");
  c.width = img.naturalWidth;
  c.height = img.naturalHeight;
  c.getContext("2d")!.drawImage(img, 0, 0);
  return new Promise((resolve, reject) => c.toBlob((b) => (b ? resolve(b) : reject(new Error("Couldn't encode the picture"))), "image/png"));
}

export async function copyImage(it: ViewerItem) {
  try {
    if (typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) throw new Error("This system can't copy pictures");
    // The write starts inside the key press or click (WebKit requires a
    // gesture); the bytes follow as a promise.
    const blob = dataUrlOf(it).then(pngBlob);
    await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
    toast({ message: "Copied image" });
  } catch (e) {
    toast({ tone: "error", message: "Couldn't copy the image", detail: asCommandError(e).message });
  }
}

/** A remote picture's https address (never an embedded data: URL, which is the picture itself). */
export function copyImageAddress(it: ViewerItem) {
  if (it.kind === "body" && it.remote) void copyText(it.src, "Image address copied");
}

/**
 * The picture's file path on this Mac, saving it to Downloads first if it
 * isn't saved yet. Every picture has one (attachments and embedded pictures
 * have no web address), and an agent or a terminal can read the file.
 */
export function copyImagePath(it: ViewerItem) {
  // The clipboard write starts inside the click or key press (WebKit allows
  // it only then); the path follows once the picture is saved.
  const path = (async () => {
    const p = savedPath(it) ?? (await saveImage(it));
    if (!p) throw new Error(`${it.name} couldn't be saved`);
    return p;
  })();
  void copyTextLater(path, "File path copied");
}

/** Paths saved this session for body pictures, per item (attachments keep theirs in AttachmentPreview). */
const saved = new Map<string, string>();

/** Where this session saved the picture, if it did. */
export function savedPath(it: ViewerItem): string | null {
  const a = attachmentOf(it);
  return saved.get(it.key) ?? (a ? savedAttachmentPath(it.message, a) : null);
}

/** The saved toast's button: Show in Finder on the Mac, else Open. */
function savedAction(path: string, name: string) {
  return nativeFiles
    ? { label: "Show in Finder", run: () => void revealSaved(path, name) }
    : { label: "Open", run: () => void api.openPath(path).catch(() => undefined) };
}

export async function saveImage(it: ViewerItem): Promise<string | null> {
  const a = attachmentOf(it);
  if (a) {
    const path = await downloadAttachment(it.message, a);
    if (path) saved.set(it.key, path);
    return path;
  }
  if (it.kind !== "body") return null;
  try {
    const path = await api.saveMessageImage(it.src, it.name);
    saved.set(it.key, path);
    const name = path.split(/[\\/]/).pop() ?? it.name;
    toast({ message: `Saved ${name} to Downloads`, action: savedAction(path, name) });
    return path;
  } catch (e) {
    toast({ tone: "error", message: `Couldn't save ${it.name}`, detail: asCommandError(e).message });
    return null;
  }
}

/** Save As…: the system save panel, starting in Downloads. */
export async function saveImageAs(it: ViewerItem): Promise<string | null> {
  const a = attachmentOf(it);
  if (a) {
    const path = await saveAttachmentAs(it.message, a);
    if (path) saved.set(it.key, path);
    return path;
  }
  if (it.kind !== "body") return null;
  try {
    const path = await api.saveImageAs(it.src, it.name);
    if (!path) return null;
    saved.set(it.key, path);
    const name = path.split(/[\\/]/).pop() ?? it.name;
    toast({ message: `Saved ${name}`, action: savedAction(path, name) });
    return path;
  } catch (e) {
    toast({ tone: "error", message: `Couldn't save ${it.name}`, detail: asCommandError(e).message });
    return null;
  }
}

const plural = (n: number) => `${n} ${n === 1 ? "image" : "images"}`;

/**
 * Save all images: every picture of one message into a new folder in
 * Downloads named after the email (save_message_images). A progress toast
 * turns into the result, with Show in Finder for the folder.
 */
export async function saveAllImages(m: MessageView, items: ViewerItem[]): Promise<string | null> {
  const list = saveAllItems(items.filter((it) => it.message.accountId === m.accountId && it.message.id === m.id));
  if (list.length === 0) return null;
  const id = toast({ kind: "progress", message: `Saving ${plural(list.length)}…`, key: `save-all:${m.accountId}/${m.id}` });
  try {
    const r = await api.saveMessageImages(m.accountId, m.id, list);
    const folder = r.folder.split(/[\\/]/).pop() ?? "the folder";
    const message = r.saved === r.total ? `Saved ${plural(r.saved)} to Downloads` : `Saved ${r.saved} of ${r.total} images to Downloads`;
    updateToast(id, {
      kind: r.saved === r.total ? "action" : "error",
      message,
      detail: r.saved === r.total ? folder : `${r.total - r.saved} couldn't be saved. The log says why.`,
      action: savedAction(r.folder, folder),
      duration: r.saved === r.total ? 6000 : null,
    });
    return r.folder;
  } catch (e) {
    updateToast(id, { kind: "error", message: `Couldn't save the images`, detail: asCommandError(e).message, duration: null });
    return null;
  }
}

export async function openInPreview(it: ViewerItem) {
  const path = savedPath(it) ?? (await saveImage(it));
  if (!path) return;
  try {
    await api.openPath(path, true);
  } catch (e) {
    toast({ tone: "error", message: `Couldn't open ${it.name}`, detail: asCommandError(e).message });
  }
}

export function revealImage(it: ViewerItem) {
  const path = savedPath(it);
  if (path) void revealSaved(path, it.name);
}
