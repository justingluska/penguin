// Where the composer's preview of an attachment reads the exact file that
// will go out (compose/filePreview.tsx). Pure, so node --test loads it.
//
// - A file just added (picked, pasted, dropped) is only in the composer:
//   its own base64 bytes, checked in Rust (preview_outgoing_file).
// - A saved draft's attachment or a forwarded one is a ref to a part on a
//   stored message: preview_attachment reads that part, the same bytes the
//   send attaches (the backend fetches the ref when sending).
import type { OutgoingAttachment } from "../../lib/types.ts";

export type PreviewSource =
  | { kind: "bytes"; filename: string; mimeType: string; dataBase64: string }
  | { kind: "stored"; accountId: string; messageId: string; attachmentId: string };

/**
 * `accountId`: where a ref without its own account lives, the draft's
 * account for a reopened draft, else the sending account (as for inline
 * images, inlineSrc.ts).
 */
export function previewSource(a: OutgoingAttachment, accountId: string): PreviewSource {
  if (a.kind === "file") return { kind: "bytes", filename: a.filename, mimeType: a.mimeType, dataBase64: a.dataBase64 };
  return { kind: "stored", accountId: a.accountId ?? accountId, messageId: a.messageId, attachmentId: a.attachmentId };
}

/** The index in `list` (the composer's whole attachment list) to step to from `at`, over files only (inline images are in the text). */
export function stepFile(list: OutgoingAttachment[], at: number, delta: number): number {
  const files = list.flatMap((a, i) => (a.contentId ? [] : [i]));
  const pos = files.indexOf(at);
  if (pos < 0) return files[0] ?? -1;
  return files[Math.min(files.length - 1, Math.max(0, pos + delta))];
}
