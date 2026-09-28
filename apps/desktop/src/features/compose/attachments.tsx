// Composer attachments: picked (⌘⇧A or the paperclip), dropped on the
// composer, or pasted into the body. Files travel as base64 until the first
// autosave re-points them at the Gmail draft (see autosave.ts). Pasted and
// dropped images become inline images instead (inline.ts): an attachment
// with a contentId, shown in the text rather than in the attachment row.
import type { OutgoingAttachment } from "../../lib/types";
import { bytes as fmtBytes, fileExt, fileTone } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { attachmentSize, MAX_ATTACHMENT_BYTES } from "./draft";
import { fileRows, inlineWidth, newContentId } from "./inline";
import type { InlineImageAttrs } from "./editor/handle";

function readAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => {
      const url = String(r.result ?? "");
      resolve(url.slice(url.indexOf(",") + 1));
    };
    r.onerror = () => reject(r.error ?? new Error(`Couldn't read ${file.name}`));
    r.readAsDataURL(file);
  });
}

/** A name for a pasted image, which arrives as "image.png". */
function pastedName(file: File): string {
  if (file.name && file.name !== "image.png") return file.name;
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  const ext = file.type.split("/")[1]?.replace("jpeg", "jpg") || "png";
  return `Pasted image ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} at ${pad(d.getHours())}.${pad(d.getMinutes())}.${pad(d.getSeconds())}.${ext}`;
}

/** An image's natural width (0 when it can't be decoded). */
async function naturalWidth(dataUrl: string): Promise<number> {
  try {
    const img = new Image();
    img.src = dataUrl;
    await img.decode();
    return img.naturalWidth;
  } catch {
    return 0;
  }
}

/** An inline image read from a file: its attachment and the editor node's attributes, plus what to show. */
export interface InlinePicture {
  attrs: InlineImageAttrs;
  dataUrl: string;
}

/**
 * Read files into attachments, refusing the batch if it would push the total
 * (files and inline images) past 25 MB. Files in `inline` become inline
 * images (a fresh contentId each, and `pictures` for the editor). Returns
 * the error message instead of throwing.
 */
export async function filesToAttachments(
  files: File[],
  current: OutgoingAttachment[],
  inline: File[] = [],
): Promise<{ added: OutgoingAttachment[]; pictures: InlinePicture[]; error: string | null }> {
  const have = current.reduce((n, a) => n + attachmentSize(a), 0);
  const adding = files.reduce((n, f) => n + f.size, 0);
  if (have + adding > MAX_ATTACHMENT_BYTES) {
    const what = inline.length ? (inline.length === files.length ? "images" : "attachments and images") : "attachments";
    return {
      added: [],
      pictures: [],
      error: `That would make ${fmtBytes(have + adding)} of ${what}; the limit is 25 MB. Share big files as a link instead.`,
    };
  }
  try {
    const pictures: InlinePicture[] = [];
    const added = await Promise.all(
      files.map(async (f): Promise<OutgoingAttachment> => {
        const filename = pastedName(f);
        const mimeType = f.type || "application/octet-stream";
        const dataBase64 = await readAsBase64(f);
        if (!inline.includes(f)) return { kind: "file", filename, mimeType, dataBase64 };
        const contentId = newContentId();
        const dataUrl = `data:${mimeType};base64,${dataBase64}`;
        pictures.push({ attrs: { cid: contentId, alt: filename, width: inlineWidth(await naturalWidth(dataUrl)) }, dataUrl });
        return { kind: "file", filename, mimeType, dataBase64, contentId };
      }),
    );
    // In the order they were pasted or dropped.
    pictures.sort((a, b) => added.findIndex((x) => x.contentId === a.attrs.cid) - added.findIndex((x) => x.contentId === b.attrs.cid));
    return { added, pictures, error: null };
  } catch (e) {
    return { added: [], pictures: [], error: e instanceof Error ? e.message : String(e) };
  }
}

/** Loading the original a reply or forward quotes (draft.ts loadOriginal). */
export type OriginalStatus = { kind: "loading"; downloading: boolean } | { kind: "error"; message: string } | null;

/**
 * Under the attachment chips: the original still downloading (a forward of
 * mail older than the sync window), a failed download with Retry, and a
 * conversation forward's "Also attach N files from earlier messages".
 */
export function OriginalFiles({
  status,
  forward,
  earlier,
  earlierOn,
  onEarlier,
  onRetry,
  onSkip,
}: {
  status: OriginalStatus;
  forward: boolean;
  earlier: OutgoingAttachment[];
  earlierOn: boolean;
  onEarlier: (on: boolean) => void;
  onRetry: () => void;
  onSkip: () => void;
}) {
  const what = forward ? "the original message and its attachments" : "the original message";
  if (status?.kind === "loading" && status.downloading) {
    return (
      <div className="cmp-orig" role="status">
        <span className="spinner" aria-hidden="true" />
        Downloading {what}…
      </div>
    );
  }
  if (status?.kind === "error") {
    return (
      <div className="cmp-orig cmp-orig-error" role="alert">
        <Icon name="info" size="xs" />
        <span className="grow">
          Couldn't download {what}: {status.message}
        </span>
        <button className="btn btn-ghost btn-sm" onClick={onRetry}>
          Retry
        </button>
        <button className="btn btn-ghost btn-sm" onClick={onSkip} title={forward ? "Send without the original's attachments and formatting" : "Quote the original as plain text"}>
          {forward ? "Send without them" : "Use plain text"}
        </button>
      </div>
    );
  }
  if (!earlier.length) return null;
  const n = earlier.length;
  return (
    <div className="cmp-orig">
      <button
        className="cmp-orig-switch"
        role="switch"
        aria-checked={earlierOn}
        onClick={() => onEarlier(!earlierOn)}
        title={earlier.map((a) => a.filename).join("\n")}
      >
        <span className={"toggle" + (earlierOn ? " on" : "")}>
          <span />
        </span>
        Also attach {n} {n === 1 ? "file" : "files"} from earlier messages
        <span className="faint tnum">{fmtBytes(earlier.reduce((s, a) => s + attachmentSize(a), 0))}</span>
      </button>
    </div>
  );
}

/**
 * The attachment row: files only (inline images are in the text). `list` is
 * what will be sent, so the total counts the images too; `onRemove` gets the
 * index in `list`.
 */
export function AttachmentList({ list, onRemove }: { list: OutgoingAttachment[]; onRemove: (index: number) => void }) {
  const rows = fileRows(list);
  if (rows.length === 0) return null;
  const total = list.reduce((n, a) => n + attachmentSize(a), 0);
  return (
    <div className="cmp-atts" aria-label="Attachments">
      {rows.map(({ a, i }) => (
        <span key={`${a.filename}-${i}`} className={`cmp-att ${fileTone(a.filename, a.mimeType)}`} title={a.filename}>
          <span className="mini-ico">{fileExt(a.filename)}</span>
          <span className="cmp-att-name truncate">{a.filename}</span>
          <span className="cmp-att-size tnum">{fmtBytes(attachmentSize(a))}</span>
          <button className="cmp-att-x" aria-label={`Remove ${a.filename}`} onClick={() => onRemove(i)}>
            <Icon name="x" size="2xs" />
          </button>
        </span>
      ))}
      {list.length > 1 && (
        <span className="cmp-atts-total tnum" title={rows.length < list.length ? "Files and the images in the message" : undefined}>
          {fmtBytes(total)} of 25 MB
        </span>
      )}
    </div>
  );
}
