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
//  - else  → "No preview" panel with Download / Open
import { useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import type { AttachmentMeta, AttachmentPreview as Preview, MessageView } from "../../lib/types";
import { getUi, setUi, useUi } from "../../lib/ui";
import { api, asCommandError } from "../../lib/api";
import { registerShortcuts } from "../../lib/keyboard";
import { bytes, displayName, fileExt, fileTone, messageTime } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Keys, Kbd } from "../../components/Kbd";
import { toast } from "../../components/Toast";
import { useKeyTip } from "../../lib/shortcutHints";
import { copyTextLater } from "../../lib/clipboard";

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
    p = api.previewAttachment(m.accountId, m.id, a.id);
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
    const path = await api.saveAttachment(m.accountId, m.id, a.id);
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
    await api.copyAttachmentFile(m.accountId, m.id, a.id);
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
                {bytes(preview?.size ?? a.size)} · {displayName(m.from)} · {messageTime(m.date)}
              </div>
            </div>
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
              error={error}
              hint="Download it, or open it in its default app."
              actions={
                <>
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
export function PreviewContent({ file, preview, error, hint, actions }: { file: PreviewFile; preview?: Preview; error?: string; hint: string; actions?: ReactNode }) {
  if (error) return <NoPreview file={file} title="Couldn't load the preview" body={error} actions={actions} />;
  if (!preview) {
    return (
      <div className="ap-loading">
        <span className="sk" style={{ width: 180, height: 12 }} />
        <span className="faint small">Loading preview…</span>
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
    case "text":
      return (
        <div className="ap-text v-scroll">
          <pre>{p.text}</pre>
          {p.truncated && <div className="ap-trunc faint small">Showing the first 1 MB of the file</div>}
        </div>
      );
    default: {
      const ext = fileExt(file.filename).toLowerCase();
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

/** PDF bytes (already verified as %PDF- by Rust) → blob: URL → WebKit's viewer. */
function PdfFrame({ dataUrl, title }: { dataUrl: string; title: string }) {
  const url = useMemo(() => {
    const b64 = dataUrl.slice(dataUrl.indexOf(",") + 1);
    const bin = atob(b64);
    const buf = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
    // The type is fixed here, never taken from the attachment's claimed MIME.
    return URL.createObjectURL(new Blob([buf], { type: "application/pdf" }));
  }, [dataUrl]);
  useEffect(() => () => URL.revokeObjectURL(url), [url]);
  return <iframe className="ap-pdf" src={url} title={title} />;
}
