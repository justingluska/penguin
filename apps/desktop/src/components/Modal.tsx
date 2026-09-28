// A centered dialog in the app's confirm style (.st-scrim / .st-confirm,
// the same look as Settings' ConfirmDialog and app/confirm.tsx), portaled to
// <body> so a transformed ancestor (the thread pane) can't clip it.
//
// Keyboard: Esc closes; Enter runs `onEnter` unless focus is on a
// button or link (which Enter activates as usual: focus the confirm button
// to make Enter confirm). While it's up, no other
// key reaches the app's single-key shortcuts (E, #, Enter to open…).
import { Fragment, useEffect, useRef, useSyncExternalStore, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { Icon } from "./Icon";
import "./modal.css";

export function Modal({
  label,
  onClose,
  onEnter,
  initialFocus,
  className,
  role = "dialog",
  children,
}: {
  /** Accessible name (usually the title). */
  label: string;
  onClose: () => void;
  onEnter?: () => void;
  /** Focused on open; defaults to the dialog itself. */
  initialFocus?: RefObject<HTMLElement | null>;
  className?: string;
  role?: "dialog" | "alertdialog";
  children: ReactNode;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const handlers = useRef({ onClose, onEnter });
  handlers.current = { onClose, onEnter };

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    (initialFocus?.current ?? boxRef.current)?.focus({ preventScroll: true });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        handlers.current.onClose();
        return;
      }
      if (e.metaKey || e.ctrlKey) return;
      e.stopPropagation();
      if (e.key === "Enter" && handlers.current.onEnter) {
        const t = e.target as HTMLElement | null;
        const onControl = !!t?.closest("button, a, input, textarea, select, summary");
        if (!onControl) {
          e.preventDefault();
          handlers.current.onEnter();
        }
      }
      if (e.key === "Tab") trapTab(e, boxRef.current);
    };
    // Capture phase: runs before the app's keyboard layer.
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      if (before && document.contains(before)) before.focus({ preventScroll: true });
    };
    // Mount/unmount only; handlers are read through the ref.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return createPortal(
    <div className="st-scrim app-modal" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={boxRef}
        className={"st-confirm modal" + (className ? " " + className : "")}
        role={role}
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
      >
        {children}
      </div>
    </div>,
    document.body,
  );
}

// One app-level slot for dialogs opened from inside other views (a message's
// privacy row, the Unsubscribe button). Rendering them in ModalHost, outside
// the thread's React tree, keeps the message's own handlers (its context
// menu, click-to-collapse) from seeing events that happen in the dialog.
type Render = (close: () => void) => ReactNode;
let current: { render: Render; id: number; onReplaced?: () => void } | null = null;
let nextId = 1;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

/**
 * Show `render(close)` in the app's modal slot, replacing any open one
 * (whose `onReplaced` runs, so a pending answer can settle).
 */
export function openModal(render: Render, onReplaced?: () => void): void {
  const prev = current;
  current = { render, id: nextId++, onReplaced };
  emit();
  prev?.onReplaced?.();
}

export function closeModal(): void {
  current = null;
  emit();
}

export function ModalHost() {
  const c = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => current,
  );
  if (!c) return null;
  const close = () => {
    if (current?.id === c.id) closeModal();
  };
  return <Fragment key={c.id}>{c.render(close)}</Fragment>;
}

/**
 * A URL on one line, monospace and selectable, cut in the middle when it
 * doesn't fit (the host and the end both stay visible). Selecting it copies
 * the whole thing; `copy` adds a Copy button.
 */
export function UrlLine({ url, copy }: { url: string; copy?: () => void }) {
  const tailLen = Math.min(24, Math.floor(url.length / 2));
  return (
    <div className="modal-url">
      <span className="modal-url-text" title={url}>
        <span className="modal-url-head">{url.slice(0, url.length - tailLen)}</span>
        <span className="modal-url-tail">{url.slice(url.length - tailLen)}</span>
      </span>
      {copy ? (
        <button type="button" className="btn btn-ghost btn-sm" onClick={copy} aria-label="Copy link">
          <Icon name="copy" size="xs" />
          Copy
        </button>
      ) : null}
    </div>
  );
}

/** Keep Tab inside the dialog. */
function trapTab(e: KeyboardEvent, box: HTMLElement | null) {
  if (!box) return;
  const items = Array.from(
    box.querySelectorAll<HTMLElement>('button:not([disabled]), a[href], input:not([disabled]), [tabindex]:not([tabindex="-1"])'),
  );
  if (!items.length) return;
  const first = items[0];
  const last = items[items.length - 1];
  const at = document.activeElement;
  if (e.shiftKey && (at === first || at === box)) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && at === last) {
    e.preventDefault();
    first.focus();
  }
}
