// Attachment preview modal. Clicking an attachment card opens it; ←/→ step
// through the message's attachments, ⌘S downloads, ⌘O opens in the default
// app, Esc closes. What renders is decided in Rust (preview_attachment):
//  - image → <img src="data:…"> (bytes sniffed server-side)
//  - pdf   → WebKit's PDF viewer in an iframe on a blob: URL we mint from the
//            verified bytes. The iframe can't be sandboxed: WebKit disables
//            its PDF plugin in sandboxed frames. CSP allows blob: frames only,
//            and frame-ancestors must name the app origin: a blob: document
//            inherits the app's CSP, so 'none' (or 'self', which WebKit
//            doesn't match for custom schemes) makes it refuse to be framed.
//  - text  → escaped in a <pre> (HTML/SVG attachments included: never markup)
//  - video / audio → the system player on a blob: URL (type sniffed in Rust)
//  - document → a Word file as HTML Penguin wrote (every character of the
//            file escaped, src-tauri/src/preview_docs.rs) in a sandboxed
//            frame with no scripts and a deny-by-default CSP
//  - sheet → a spreadsheet's (or CSV's) first rows as a table, sheet tabs
//  - else  → "No preview" panel with Quick Look (macOS) / Download / Open
import { useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import type { AttachmentMeta, AttachmentPreview as Preview, MessageView, SheetPreview } from "../../lib/types";
import { getUi, setUi, useUi } from "../../lib/ui";
import { api, asCommandError } from "../../lib/api";
import { registerShortcuts } from "../../lib/keyboard";
import { bytes, displayName, fileExt, fileTone, messageTime } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Keys, Kbd } from "../../components/Kbd";
import { isMac } from "../../lib/keyboard";
import { toast } from "../../components/Toast";
import { useKeyTip } from "../../lib/shortcutHints";
import { copyTextLater } from "../../lib/clipboard";
import { ProgressRing } from "../../components/ProgressRing";
import { fractionOf, progressText, track, useAttachmentProgress, type AttachmentProgress } from "./attachmentProgress";

// ---------------------------------------------------------------------------
// Open/close + shared state
// ---------------------------------------------------------------------------
interface Target {
  message: MessageView;
  index: number;
}
let target: Target | null = null;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

function files(m: MessageView): AttachmentMeta[] {
  return m.attachments.filter((a) => !a.inline);
}

export function openAttachmentPreview(message: MessageView, attachmentId: string) {
  const index = Math.max(0, files(message).findIndex((a) => a.id === attachmentId));
  target = { message, index };
  emit();
  setUi({ overlay: "attachment" });
}

function closePreview() {
  if (getUi().overlay === "attachment") setUi({ overlay: null });
}

function step(delta: number) {
  if (!target) return;
  const n = files(target.message).length;
  const index = Math.min(n - 1, Math.max(0, target.index + delta));
  if (index === target.index) return;
  target = { ...target, index };
  emit();
}

// Previews are small-ish and immutable: keep the last few for instant ←/→.
const CACHE_MAX = 8;
const previewCache = new Map<string, Promise<Preview>>();
export function loadPreview(m: MessageView, a: AttachmentMeta): Promise<Preview> {
  const k = `${m.accountId}\u0000${m.id}\u0000${a.id}`;
  let p = previewCache.get(k);
  if (!p) {
    p = track(m, a, api.previewAttachment(m.accountId, m.id, a.id));
    // A failed load isn't cached, so reopening retries.
    p.catch(() => previewCache.delete(k));
    previewCache.set(k, p);
    while (previewCache.size > CACHE_MAX) previewCache.delete(previewCache.keys().next().value!);
  }
  return p;
}

// Save/open reuse one saved copy per attachment this session.
// Keyed by account + message + attachment: ids like `part:2` repeat across messages.
const savedPaths = new Map<string, string>();
const savedKey = (m: MessageView, a: AttachmentMeta) => `${m.accountId}\u0000${m.id}\u0000${a.id}`;

/** Where this session saved the attachment (Save, Save As), for Open and Show in Finder. */
export function savedAttachmentPath(m: MessageView, a: AttachmentMeta): string | null {
  return savedPaths.get(savedKey(m, a)) ?? null;
}

/** Save As…: the system save panel (macOS), starting in Downloads. Null when cancelled. */
export async function saveAttachmentAs(m: MessageView, a: AttachmentMeta): Promise<string | null> {
  try {
    const path = await api.saveAttachmentAs(m.accountId, m.id, a.id);
    if (!path) return null;
    savedPaths.set(savedKey(m, a), path);
    const name = path.split(/[\\/]/).pop() ?? a.filename;
    toast({ message: `Saved ${name}`, action: { label: "Show in Finder", run: () => void revealSaved(path, name) } });
    return path;
  } catch (e) {
    toast({ tone: "error", message: `Couldn't save ${a.filename}: ${asCommandError(e).message}` });
    return null;
  }
}

/** Show in Finder: a file this session saved. */
export async function revealSaved(path: string, name: string) {
  try {
    await api.revealSavedPath(path);
  } catch (e) {
    toast({ tone: "error", message: `Couldn't show ${name}: ${asCommandError(e).message}` });
  }
}

export async function downloadAttachment(m: MessageView, a: AttachmentMeta): Promise<string | null> {
  try {
    const path = await track(m, a, api.saveAttachment(m.accountId, m.id, a.id));
    savedPaths.set(savedKey(m, a), path);
    toast({ message: `Saved ${a.filename} to Downloads`, action: { label: "Open", run: () => void openSaved(path, a) } });
    return path;
  } catch (e) {
    toast({ tone: "error", message: `Couldn't save ${a.filename}: ${asCommandError(e).message}` });
    return null;
  }
}

async function openSaved(path: string, a: AttachmentMeta) {
  try {
    await api.openPath(path);
  } catch (e) {
    toast({ tone: "error", message: `Couldn't open ${a.filename}: ${asCommandError(e).message}` });
  }
}

/**
 * Copy: the file on the clipboard as Finder copies one (Mac app), so a paste
 * in Finder, Slack, Mail or a composer gives the file.
 */
export async function copyAttachmentFile(m: MessageView, a: AttachmentMeta) {
  try {
    await track(m, a, api.copyAttachmentFile(m.accountId, m.id, a.id));
    toast({ message: `Copied ${a.filename}`, detail: "Paste it in Finder, a chat or a message." });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't copy ${a.filename}`, detail: asCommandError(e).message });
  }
}

/** Copy File Path: where the file is on this Mac, saving it to Downloads first if this session hasn't. */
export function copyAttachmentPath(m: MessageView, a: AttachmentMeta) {
  // The clipboard write starts inside the menu click (WebKit's rule); the
  // path follows once the file is saved.
  const path = (async () => {
    const p = savedPaths.get(savedKey(m, a)) ?? (await downloadAttachment(m, a));
    if (!p) throw new Error(`${a.filename} couldn't be saved`);
    return p;
  })();
  void copyTextLater(path, "File path copied");
}

/** macOS Quick Look (decks, Pages, Numbers, anything the preview can't show). */
export async function quickLook(m: MessageView, a: AttachmentMeta) {
  try {
    await track(m, a, api.quickLookAttachment(m.accountId, m.id, a.id));
  } catch (e) {
    toast({ tone: "error", message: `Couldn't open ${a.filename} in Quick Look`, detail: asCommandError(e).message });
  }
}

/** Open in the default app: it needs a file on disk, so save first (once). */
export async function openInDefaultApp(m: MessageView, a: AttachmentMeta) {
  const path = savedPaths.get(savedKey(m, a)) ?? (await downloadAttachment(m, a));
  if (path) await openSaved(path, a);
}

// ---------------------------------------------------------------------------
// Modal
// ---------------------------------------------------------------------------
export function AttachmentPreviewHost() {
  const open = useUi((s) => s.overlay === "attachment");
  const t = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => target,
  );
  if (!open || !t) return null;
  return <Modal t={t} />;
}

function Modal({ t }: { t: Target }) {
  const tip = useKeyTip();
  const list = files(t.message);
  const a = list[t.index];
  const [state, setState] = useState<{ id: string; preview?: Preview; error?: string }>({ id: a.id });

  useEffect(() => {
    let live = true;
    setState({ id: a.id });
    loadPreview(t.message, a).then(
      (preview) => live && setState({ id: a.id, preview }),
      (e) => live && setState({ id: a.id, error: asCommandError(e).message }),
    );
    // Warm the next one so → is instant.
    const next = list[t.index + 1];
    if (next && next.size <= 8 * 1024 * 1024) {
      loadPreview(t.message, next).catch(() => {
        // Nothing to show yet: a failed load isn't cached, so the error
        // surfaces (and retries) if the user steps to that attachment.
      });
    }
    return () => {
      live = false;
    };
  }, [t.message, a.id]);

  useEffect(
    () =>
      registerShortcuts([
        { id: "att.close", keys: "escape", label: "Close preview", group: "Preview", hidden: true, when: isOpen, run: closePreview },
        { id: "att.prev", keys: "arrowleft", label: "Previous attachment", group: "Preview", when: isOpen, run: () => step(-1) },
        { id: "att.next", keys: "arrowright", label: "Next attachment", group: "Preview", when: isOpen, run: () => step(1) },
        { id: "att.save", keys: "mod+s", label: "Download attachment", group: "Preview", when: isOpen, run: () => current() && void downloadAttachment(...current()!) },
        { id: "att.open", keys: "mod+o", label: "Open in default app", group: "Preview", when: isOpen, run: () => current() && void openInDefaultApp(...current()!) },
      ]),
    [],
  );

  const preview = state.id === a.id ? state.preview : undefined;
  const error = state.id === a.id ? state.error : undefined;
  const m = t.message;
  const progress = useAttachmentProgress(m, a);
  const counted = progressText(progress);

  return (
    <>
      <div className="scrim" onMouseDown={closePreview} />
      <div className="overlay-host center" onMouseDown={(e) => e.target === e.currentTarget && closePreview()}>
        <section className="panel att-preview" role="dialog" aria-label={`Preview of ${a.filename}`}>
          <header className="ap-bar">
            <span className={"mini-ico " + fileTone(a.filename, a.mimeType)}>{fileExt(a.filename)}</span>
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="ap-name truncate">{a.filename}</div>
              <div className="ap-meta truncate">
                {preview && counted ? `Downloading ${counted}` : `${bytes(preview?.size ?? a.size)} · ${displayName(m.from)} · ${messageTime(m.date)}`}
              </div>
            </div>
            {/* A Download, Open or Quick Look still fetching the file. */}
            {preview && progress && <ProgressRing value={fractionOf(progress)} label={`Downloading ${a.filename}`} />}
            {list.length > 1 && (
              <div className="ap-nav">
                <button className="btn btn-ghost btn-sm btn-icon" title={tip("Previous", "←")} disabled={t.index === 0} onClick={() => step(-1)}>
                  <Icon name="left" size="xs" />
                </button>
                <span className="faint tnum small">
                  {t.index + 1} of {list.length}
                </span>
                <button className="btn btn-ghost btn-sm btn-icon" title={tip("Next", "→")} disabled={t.index === list.length - 1} onClick={() => step(1)}>
                  <Icon name="right" size="xs" />
                </button>
              </div>
            )}
            <span className="head-sep" />
            {isMac && (
              <button className="btn btn-ghost btn-sm" title="Look at it in macOS Quick Look" onClick={() => void quickLook(m, a)}>
                <Icon name="eye" size="xs" />
                Quick Look
              </button>
            )}
            <button className="btn btn-ghost btn-sm" onClick={() => void downloadAttachment(m, a)}>
              <Icon name="download" size="xs" />
              Download
              <Keys keys="mod+s" />
            </button>
            <button className="btn btn-ghost btn-sm" onClick={() => void openInDefaultApp(m, a)}>
              <Icon name="external" size="xs" />
              Open
              <Keys keys="mod+o" />
            </button>
            <button className="btn btn-ghost btn-sm" onClick={closePreview}>
              Close
              <Kbd>Esc</Kbd>
            </button>
          </header>
          <div className="ap-body">
            <PreviewContent
              file={a}
              preview={preview}
              progress={progress}
              error={error}
              hint={isMac ? "Look at it in Quick Look, download it, or open it in its default app." : "Download it, or open it in its default app."}
              actions={
                <>
                  {isMac && (
                    <button className="btn btn-primary" onClick={() => void quickLook(m, a)}>
                      <Icon name="eye" size="sm" />
                      Quick Look
                    </button>
                  )}
                  <button className="btn btn-secondary" onClick={() => void downloadAttachment(m, a)}>
                    <Icon name="download" size="sm" />
                    Download
                    <Keys keys="mod+s" />
                  </button>
                  <button className="btn btn-ghost" onClick={() => void openInDefaultApp(m, a)}>
                    <Icon name="external" size="sm" />
                    Open in default app
                  </button>
                </>
              }
            />
          </div>
        </section>
      </div>
    </>
  );
}

const isOpen = () => getUi().overlay === "attachment" && target !== null;
function current(): [MessageView, AttachmentMeta] | null {
  if (!target) return null;
  const a = files(target.message)[target.index];
  return a ? [target.message, a] : null;
}

/** A file to preview: an attachment, or a file in the composer. */
interface PreviewFile {
  filename: string;
  mimeType: string;
}

/**
 * The body of a preview: loading, the picture, PDF or text, or a "No
 * preview" panel with `hint` and `actions` (the thread's Download, Open).
 * Shared by the thread's modal and the composer's (compose/filePreview.tsx).
 */
export function PreviewContent({
  file,
  preview,
  progress,
  error,
  hint,
  actions,
}: {
  file: PreviewFile;
  preview?: Preview;
  /** The attachment's download, while it runs (thread previews). */
  progress?: AttachmentProgress | null;
  error?: string;
  hint: string;
  actions?: ReactNode;
}) {
  if (error) return <NoPreview file={file} title="Couldn't load the preview" body={error} actions={actions} />;
  if (!preview) {
    const counted = progressText(progress ?? null);
    return (
      <div className="ap-loading">
        {progress ? <ProgressRing value={fractionOf(progress)} size={32} label={`Downloading ${file.filename}`} /> : <span className="sk" style={{ width: 180, height: 12 }} />}
        <span className="faint small tnum">{counted ? `Downloading ${counted}` : progress ? "Downloading…" : "Loading preview…"}</span>
      </div>
    );
  }
  const p = preview;
  switch (p.kind) {
    case "image":
      return (
        <div className="ap-image">
          <img src={p.dataUrl ?? undefined} alt={file.filename} draggable={false} />
        </div>
      );
    case "pdf":
      return <PdfFrame dataUrl={p.dataUrl ?? ""} title={file.filename} />;
    case "video":
      return <MediaPlayer dataUrl={p.dataUrl ?? ""} type={p.mimeType} video title={file.filename} />;
    case "audio":
      return <MediaPlayer dataUrl={p.dataUrl ?? ""} type={p.mimeType} video={false} title={file.filename} file={file} />;
    case "document":
      return <DocumentFrame html={p.html ?? ""} truncated={p.truncated} title={file.filename} />;
    case "sheet":
      return <SheetView sheets={p.sheets ?? []} truncated={p.truncated} />;
    case "text":
      return (
        <div className="ap-text v-scroll">
          <pre>{p.text}</pre>
          {p.truncated && <div className="ap-trunc faint small">Showing the first 1 MB of the file</div>}
        </div>
      );
    default: {
      // The real extension (fileExt is the icon's short label: "PPT" for .pptx).
      const ext = (/\.([^.\s]+)$/.exec(file.filename)?.[1] ?? fileExt(file.filename)).toLowerCase();
      const title =
        p.reason === "tooLarge"
          ? "Too large to preview"
          : p.reason === "unreadable"
            ? "This file can't be previewed"
            : `No preview for .${ext}`;
      const body =
        p.reason === "tooLarge"
          ? `${bytes(p.size)} is over the 20 MB preview limit.`
          : p.reason === "unreadable"
            ? `Its contents don't look like a ${ext.toUpperCase()} file.`
            : hint;
      return <NoPreview file={file} title={title} body={body} actions={actions} />;
    }
  }
}

function NoPreview({ file, title, body, actions }: { file: PreviewFile; title: string; body: string; actions?: ReactNode }) {
  return (
    <div className="ap-none">
      <span className={"file-ico ap-bigico " + fileTone(file.filename, file.mimeType)}>{fileExt(file.filename)}</span>
      <div className="ap-none-title">{title}</div>
      <div className="ap-none-body">{body}</div>
      {actions && (
        <div className="row-flex" style={{ gap: 8, marginTop: 8 }}>
          {actions}
        </div>
      )}
    </div>
  );
}

/** Verified bytes (a data: URL from Rust) as a blob: URL of `type`, revoked on unmount. */
function useBlobUrl(dataUrl: string, type: string): string {
  const url = useMemo(() => {
    const b64 = dataUrl.slice(dataUrl.indexOf(",") + 1);
    const bin = atob(b64);
    const buf = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
    return URL.createObjectURL(new Blob([buf], { type }));
  }, [dataUrl, type]);
  useEffect(() => () => URL.revokeObjectURL(url), [url]);
  return url;
}

/** PDF bytes (already verified as %PDF- by Rust) → blob: URL → WebKit's viewer. */
function PdfFrame({ dataUrl, title }: { dataUrl: string; title: string }) {
  // The type is fixed here, never taken from the attachment's claimed MIME.
  const url = useBlobUrl(dataUrl, "application/pdf");
  return <iframe className="ap-pdf" src={url} title={title} />;
}

/** Audio or video Rust recognized from its bytes (`type` is that sniffed type), in the system player. */
function MediaPlayer({ dataUrl, type, video, title, file }: { dataUrl: string; type: string; video: boolean; title: string; file?: PreviewFile }) {
  const url = useBlobUrl(dataUrl, type);
  if (video) {
    return (
      <div className="ap-media">
        <video src={url} controls playsInline preload="metadata" aria-label={title} />
      </div>
    );
  }
  return (
    <div className="ap-media ap-audio">
      {file && <span className={"file-ico ap-bigico " + fileTone(file.filename, file.mimeType)}>{fileExt(file.filename)}</span>}
      <div className="ap-none-title">{title}</div>
      <audio src={url} controls preload="metadata" aria-label={title} />
    </div>
  );
}

/** Styles for a Word document's page (the frame has no access to the app's). */
const DOC_STYLE = `
:root { color-scheme: light; }
body { margin: 0; padding: 28px 16px 40px; background: #eef0f2; font: 15px/1.6 -apple-system, "Helvetica Neue", Helvetica, Arial, sans-serif; color: #1d1f21; }
article { max-width: 760px; margin: 0 auto; padding: 56px 64px; background: #fff; border-radius: 6px; box-shadow: 0 1px 3px rgba(0,0,0,.08), 0 8px 24px -12px rgba(0,0,0,.18); white-space: pre-wrap; overflow-wrap: anywhere; tab-size: 4; }
p { margin: 0 0 .7em; min-height: 1em; }
h1 { font-size: 1.7em; line-height: 1.25; margin: .4em 0 .5em; }
h2 { font-size: 1.35em; line-height: 1.3; margin: .9em 0 .4em; }
h3, h4, h5, h6 { font-size: 1.1em; margin: .9em 0 .35em; }
ul { margin: 0 0 .7em; padding-left: 1.5em; white-space: normal; }
li { white-space: pre-wrap; }
table { border-collapse: collapse; margin: .4em 0 1em; width: 100%; white-space: normal; }
td { border: 1px solid #d6d9dc; padding: 6px 8px; vertical-align: top; }
td p { margin: 0; }
.trunc { margin-top: 24px; color: #6e7679; font-size: 12px; text-align: center; }
`;

/**
 * A Word document as Penguin wrote it (preview_docs.rs: text only, escaped;
 * the only tags are ones it chose). Still shown like email: in a frame with
 * no scripts, no same-origin and a CSP that loads nothing.
 */
function DocumentFrame({ html, truncated, title }: { html: string; truncated: boolean; title: string }) {
  const doc = useMemo(
    () =>
      `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'"><style>${DOC_STYLE}</style></head><body><article>${html}</article>${
        truncated ? `<div class="trunc">The rest of this document is too long to preview. Download it to read it all.</div>` : ""
      }</body></html>`,
    [html, truncated],
  );
  return <iframe className="ap-doc" sandbox="" srcDoc={doc} title={title} />;
}

/** Spreadsheet columns as Excel names them: A … Z, AA … */
function colName(i: number): string {
  let s = "";
  for (let n = i + 1; n > 0; n = Math.floor((n - 1) / 26)) s = String.fromCharCode(65 + ((n - 1) % 26)) + s;
  return s;
}

/** A spreadsheet's (or CSV's) first rows, a tab per sheet. React escapes every cell. */
function SheetView({ sheets, truncated }: { sheets: SheetPreview[]; truncated: boolean }) {
  const [at, setAt] = useState(0);
  const sheet = sheets[Math.min(at, sheets.length - 1)];
  if (!sheet || sheet.rows.length === 0) return <div className="ap-none"><div className="ap-none-title">This spreadsheet is empty</div></div>;
  const cols = sheet.rows.reduce((n, r) => Math.max(n, r.length), 0);
  const more = sheet.totalRows > sheet.rows.length || sheet.totalCols > cols;
  return (
    <div className="ap-sheet">
      <div className="ap-sheet-scroll v-scroll">
        <table className="ap-grid">
          <thead>
            <tr>
              <th className="ap-corner" />
              {Array.from({ length: cols }, (_, i) => (
                <th key={i}>{colName(i)}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {sheet.rows.map((r, i) => (
              <tr key={i}>
                <th className="tnum">{i + 1}</th>
                {Array.from({ length: cols }, (_, j) => (
                  <td key={j} className={/^-?[\d.,]+%?$/.test(r[j] ?? "") ? "tnum ap-num" : undefined}>
                    {r[j] ?? ""}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {(more || truncated) && (
          <div className="ap-trunc faint small">
            Showing {sheet.rows.length.toLocaleString()} of {sheet.totalRows.toLocaleString()} rows
            {sheet.totalCols > cols ? ` and ${cols} of ${sheet.totalCols} columns` : ""}. Open it to see everything.
          </div>
        )}
      </div>
      {sheets.length > 1 && (
        <div className="ap-sheet-tabs" role="tablist">
          {sheets.map((sh, i) => (
            <button key={sh.name + i} role="tab" aria-selected={i === at} className={"ap-sheet-tab" + (i === at ? " is-on" : "")} onClick={() => setAt(i)}>
              {sh.name}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
