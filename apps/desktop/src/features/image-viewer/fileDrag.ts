// Drag a picture or an attachment out of the app as a real file: to Finder,
// the Desktop, Messages, Slack. WKWebView's own drag of an <img> carries
// only a URL or a picture of the page, so the element's HTML dragstart is
// cancelled and a native file drag (NSDraggingSession) starts instead:
//
//   1. the file is written by Rust (file_export.rs prepare_*_drag) into this
//      session's drag-out directory, once per item;
//   2. start_file_drag begins the session from where the pointer is. Rust
//      accepts only a path it wrote there.
//
// The first drag of a remote picture fetches it again (like Save); if the
// button was released before the file was ready, the next drag starts at
// once. This is the pattern tauri-plugin-drag documents (dragstart →
// preventDefault → startDrag); see docs/SECURITY.md → "Drag out and Save As".
import type { DragEvent as ReactDragEvent } from "react";
import type { AttachmentMeta, DragFile, MessageView } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { toast } from "../../components/Toast";
import { nativeFiles } from "./actions";
import { attachmentKey, type ViewerItem } from "./items";

/** Native file drags exist only in the Mac app (not the browser mock or Linux). */
export const canDragFiles = nativeFiles;

export interface DragSource {
  key: string;
  name: string;
  prepare: () => Promise<DragFile>;
}

export function itemDragSource(it: ViewerItem): DragSource {
  if (it.kind === "attachment") return attachmentDragSource(it.message, it.attachment, it.key);
  // An embedded picture whose inline part is known: that part's bytes and name.
  if (it.attachment) return attachmentDragSource(it.message, it.attachment, it.key);
  const src = it.src;
  return { key: it.key, name: it.name, prepare: () => api.prepareImageDrag(src, it.name) };
}

export function attachmentDragSource(m: MessageView, a: AttachmentMeta, key = attachmentKey(m, a)): DragSource {
  return { key, name: a.filename, prepare: () => api.prepareAttachmentDrag(m.accountId, m.id, a.id) };
}

/** Files written this session, per item: a second drag starts at once. */
const ready = new Map<string, Promise<DragFile>>();

function prepared(src: DragSource): Promise<DragFile> {
  let p = ready.get(src.key);
  if (!p) {
    p = src.prepare();
    // A failure isn't kept, so the next drag retries.
    p.catch(() => ready.delete(src.key));
    ready.set(src.key, p);
  }
  return p;
}

// Whether the primary button is still down. A native drag must begin while
// it is; after a release AppKit would end it at once, wherever the pointer is.
let buttonDown = false;
if (typeof window !== "undefined") {
  const down = (e: PointerEvent | MouseEvent) => {
    if (e.button === 0) buttonDown = true;
  };
  const up = (e: PointerEvent | MouseEvent) => {
    if (e.button === 0) buttonDown = false;
  };
  window.addEventListener("pointerdown", down, true);
  window.addEventListener("mousedown", down, true);
  window.addEventListener("pointerup", up, true);
  window.addEventListener("mouseup", up, true);
  window.addEventListener("blur", () => (buttonDown = false));
}

/**
 * An element's dragstart: cancel WebKit's drag and start a native file drag
 * of `src`. Outside the Mac app it does nothing, so the browser's own drag
 * (a URL) still works in the mock.
 */
export function dragOut(e: ReactDragEvent, src: DragSource) {
  if (!canDragFiles) return;
  e.preventDefault();
  e.stopPropagation();
  const known = ready.has(src.key);
  void (async () => {
    try {
      const file = await prepared(src);
      if (!buttonDown) {
        // Released while a remote picture or an attachment was downloading.
        if (!known) toast({ message: `${file.name} is ready to drag`, detail: "Drag it again to drop it in Finder or another app." });
        return;
      }
      await api.startFileDrag(file.path);
    } catch (err) {
      toast({ tone: "error", message: `Couldn't drag ${src.name}`, detail: asCommandError(err).message });
    }
  })();
}
