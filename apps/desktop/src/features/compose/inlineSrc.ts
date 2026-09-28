// What the composer shows for each inline image, by Content-ID: a data: URL
// made from the pasted bytes, or, for an image already on a saved draft, one
// the backend reads through preview_attachment (bytes sniffed as an image,
// cached). The editor document only ever holds the cid, so nothing in a
// reopened draft can point the editor at a URL of its choosing.
import { api, asCommandError } from "../../lib/api";
import type { OutgoingAttachment } from "../../lib/types";
import { isInlineImageType } from "./inline";

export interface InlineSrc {
  url: string | null;
  error: string | null;
}

const entries = new Map<string, InlineSrc>();
const subs = new Map<string, Set<() => void>>();

function set(cid: string, e: InlineSrc) {
  entries.set(cid, e);
  subs.get(cid)?.forEach((f) => f());
}

export function inlineSrc(cid: string): InlineSrc | undefined {
  return entries.get(cid);
}

/** Show `url` (a data: URL we made) for `cid`. */
export function setInlineSrc(cid: string, url: string) {
  set(cid, { url, error: null });
}

export function subscribeInlineSrc(cid: string, f: () => void): () => void {
  let s = subs.get(cid);
  if (!s) subs.set(cid, (s = new Set()));
  s.add(f);
  return () => {
    s.delete(f);
  };
}

/**
 * Make sure `a` (an inline image) has something to show: its own bytes, or
 * the stored part read through the backend. `accountId` is where a saved
 * draft's part lives when the ref doesn't say.
 */
export function loadInlineSrc(a: OutgoingAttachment, accountId: string) {
  const cid = a.contentId;
  if (!cid || entries.has(cid)) return;
  if (a.kind === "file") {
    if (isInlineImageType(a.mimeType)) setInlineSrc(cid, `data:${a.mimeType.toLowerCase()};base64,${a.dataBase64}`);
    else set(cid, { url: null, error: "not an image" });
    return;
  }
  set(cid, { url: null, error: null });
  api
    .previewAttachment(a.accountId ?? accountId, a.messageId, a.attachmentId)
    .then((p) => {
      if (p.kind === "image" && p.dataUrl) setInlineSrc(cid, p.dataUrl);
      else set(cid, { url: null, error: p.reason ?? "not an image" });
    })
    .catch((e) => set(cid, { url: null, error: asCommandError(e).message }));
}
