// Preview an attachment before it goes out: click a chip or its eye, or
// Space on a focused chip (like Quick Look). The same preview as a received
// attachment (thread/AttachmentPreview.tsx PreviewContent), fed from the
// exact file the send will attach: the composer's own bytes for a file just
// added, the stored part for a saved draft's or a forward's (previewSource.ts).
// ←/→ step through the message's files; Space or Esc closes; Remove takes
// the file off the message.
//
// It's the composer's own layer, not the app overlay (ui.overlay): the main
// window's composer *is* that overlay, and a second one would close it.
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import type { AttachmentPreview as Preview, OutgoingAttachment } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { bytes as fmtBytes, fileExt, fileTone } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { PreviewContent } from "../thread/AttachmentPreview";
import { attachmentSize } from "./draft";
import { fileRows } from "./inline";
import { previewSource, stepFile } from "./previewSource";

// Attachments are immutable values in the composer's state: one preview per
// object while it's attached, so ←/→ back and forth doesn't reload.
const cache = new WeakMap<OutgoingAttachment, Promise<Preview>>();

function load(a: OutgoingAttachment, accountId: string): Promise<Preview> {
  let p = cache.get(a);
  if (!p) {
    const src = previewSource(a, accountId);
    p = src.kind === "bytes" ? api.previewOutgoingFile(src.filename, src.mimeType, src.dataBase64) : api.previewAttachment(src.accountId, src.messageId, src.attachmentId);
    // A failure isn't kept, so opening it again retries.
    p.catch(() => cache.delete(a));
    cache.set(a, p);
  }
  return p;
}

export function FilePreview({
  list,
  at,
  accountId,
  onStep,
  onRemove,
  onClose,
}: {
  /** Everything attached (inline images too); `at` is an index into it. */
  list: OutgoingAttachment[];
  at: number;
  /** Where a stored part without its own account lives (the draft's account, else the sending one). */
  accountId: string;
  onStep: (at: number) => void;
  onRemove: (at: number) => void;
  onClose: () => void;
}) {
  const a = list[at];
  const [state, setState] = useState<{ a: OutgoingAttachment; preview?: Preview; error?: string } | null>(null);
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    if (!a) return;
    let live = true;
    setState({ a });
    load(a, accountId).then(
      (preview) => live && setState({ a, preview }),
      (e) => live && setState({ a, error: asCommandError(e).message }),
    );
    return () => {
      live = false;
    };
  }, [a, accountId]);

  // Keys go to the preview while it's up.
  useEffect(() => {
    ref.current?.focus({ preventScroll: true });
  }, []);

  if (!a) return null;
  const files = fileRows(list);
  const pos = files.findIndex((r) => r.i === at);
  const shown = state?.a === a ? state : { a };

  function onKey(e: KeyboardEvent) {
    // Nothing here reaches the composer (Esc would close it) or the app's keys.
    e.stopPropagation();
    const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
    if (e.key === "Escape" || (plain && e.key === " ")) {
      e.preventDefault();
      onClose();
    } else if (plain && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      onStep(stepFile(list, at, e.key === "ArrowLeft" ? -1 : 1));
    }
  }

  return createPortal(
    <>
      <div className="scrim cmp-preview-scrim" onMouseDown={onClose} />
      <div className="overlay-host center cmp-preview-host" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
        <section ref={ref} className="panel att-preview" role="dialog" aria-modal="true" aria-label={`Preview of ${a.filename}`} tabIndex={-1} onKeyDown={onKey}>
          <header className="ap-bar">
            <span className={"mini-ico " + fileTone(a.filename, a.mimeType)}>{fileExt(a.filename)}</span>
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="ap-name truncate">{a.filename}</div>
              <div className="ap-meta truncate">{fmtBytes(shown.preview?.size ?? attachmentSize(a))} · Attached to this message, not sent yet</div>
            </div>
            {files.length > 1 && (
              <div className="ap-nav">
                <button className="btn btn-ghost btn-sm btn-icon" title="Previous (←)" disabled={pos <= 0} onClick={() => onStep(stepFile(list, at, -1))}>
                  <Icon name="left" size="xs" />
                </button>
                <span className="faint tnum small">
                  {pos + 1} of {files.length}
                </span>
                <button className="btn btn-ghost btn-sm btn-icon" title="Next (→)" disabled={pos >= files.length - 1} onClick={() => onStep(stepFile(list, at, 1))}>
                  <Icon name="right" size="xs" />
                </button>
              </div>
            )}
            <span className="head-sep" />
            <button className="btn btn-ghost btn-sm" onClick={() => onRemove(at)} title="Take this file off the message">
              <Icon name="trash" size="xs" />
              Remove
            </button>
            <button className="btn btn-ghost btn-sm" onClick={onClose}>
              Close
              <Kbd>Esc</Kbd>
            </button>
          </header>
          <div className="ap-body">
            <PreviewContent file={a} preview={shown.preview} error={shown.error} hint="Penguin can't show this kind of file. It goes out exactly as attached." />
          </div>
        </section>
      </div>
    </>,
    document.body,
  );
}
