// How far along each attachment download is, for the cards, the preview
// panel and the image viewer (bookkeeping in downloadBook.ts).
import { useEffect, useSyncExternalStore } from "react";
import { onAttachmentProgress } from "../../lib/api";
import type { AttachmentMeta, MessageView } from "../../lib/types";
import { DownloadBook, downloadKey, type AttachmentProgress } from "./downloadBook";

export { fractionOf, progressText, type AttachmentProgress } from "./downloadBook";

type Ref = Pick<MessageView, "accountId" | "id">;

const book = new DownloadBook();
const subs = new Set<() => void>();
let version = 0;
let started = false;

function changed() {
  version++;
  subs.forEach((f) => f());
}

function start() {
  if (started) return;
  started = true;
  void onAttachmentProgress((ev) => {
    book.apply(ev);
    changed();
  });
}

/** Show `a` as busy while `p` runs (a save, preview, Quick Look…); resolves as `p` does. */
export function track<T>(m: Ref, a: Pick<AttachmentMeta, "id">, p: Promise<T>): Promise<T> {
  start();
  const key = downloadKey(m.accountId, m.id, a.id);
  book.begin(key);
  changed();
  const done = () => {
    book.end(key);
    changed();
  };
  p.then(done, done);
  return p;
}

export function useAttachmentProgress(m: Ref, a: Pick<AttachmentMeta, "id">): AttachmentProgress | null {
  useEffect(start, []);
  useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => version,
  );
  return book.progress(downloadKey(m.accountId, m.id, a.id));
}
