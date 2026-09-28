// Image viewer: a full-window lightbox for the pictures in a conversation,
// both in message bodies (inline cid: parts and remote images the user let
// load) and image attachments. Opened by a click on a picture (bridge.ts,
// decorate.ts), an image attachment card, or its context menu.
//
// Shows only what the app already has: a body picture's own sanitized
// source (data: or an https URL that loaded in the frame), or an
// attachment's sniffed preview bytes. Save / Copy / Open in Preview go
// through Rust (image_viewer.rs) and the attachment commands.
//
// Keys: ←/→ step, Esc closes, Z toggles fit / 100%, = and - zoom, ⌘C copies,
// ⌘S saves, ⌘O opens in Preview. Scroll or pinch zooms around the pointer,
// click toggles fit / 100%. Right-click opens the picture menu (imageMenu.ts).
//
// Dragging (fileDrag.ts): at fit, dragging the picture drags it out as a
// file (Finder, the Desktop, another app). Zoomed in, a drag pans and
// ⌥-drag drags the file out. The "Drag" grip in the bar always drags the file.

import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type DragEvent as ReactDragEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import type { AttachmentMeta, MessageView } from "../../lib/types";
import { asCommandError, onImageOpen } from "../../lib/api";
import { getUi, setUi, useUi } from "../../lib/ui";
import { registerShortcuts } from "../../lib/keyboard";
import { bytes, displayName, messageTime } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Keys, Kbd } from "../../components/Kbd";
import { showContextMenu } from "../../components/ContextMenu";
import { toast } from "../../components/Toast";
import { useKeyTip } from "../../lib/shortcutHints";
import { loadPreview } from "../thread/AttachmentPreview";
import { copyImage, copyImageAddress, nativeFiles, openInPreview, openLabel, previewImage, revealImage, saveAllImages, saveImage, saveImageAs, savedPath } from "./actions";
import { canDragFiles, dragOut, itemDragSource } from "./fileDrag";
import { imageMenu } from "./imageMenu";
import { framesInOrder, resolveImageRequest } from "./bridge";
import { attachmentKey, bodyKey, buildItems, canSaveAll, frameBodyImages, type MessageImages, type ViewerItem } from "./items";
import "./imageViewer.css";

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------
interface State {
  items: ViewerItem[];
  index: number;
}
let state: State | null = null;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

const isOpen = () => getUi().overlay === "image" && state !== null;

/** Other overlays (compose, search…) own the screen; a stray open waits. */
function canOpen(): boolean {
  const o = getUi().overlay;
  return o === null || o === "image";
}

export function openImageViewer(items: ViewerItem[], index: number) {
  if (items.length === 0 || !canOpen()) return;
  state = { items, index: Math.min(items.length - 1, Math.max(0, index)) };
  emit();
  setUi({ overlay: "image" });
}

function close() {
  if (getUi().overlay === "image") setUi({ overlay: null });
}

function step(delta: number) {
  if (!state) return;
  const index = Math.min(state.items.length - 1, Math.max(0, state.index + delta));
  if (index === state.index) return;
  state = { ...state, index };
  emit();
}

const sameMessage = (a: MessageView, b: MessageView) => a.accountId === b.accountId && a.id === b.id;

/** Every picture in `anchor`'s conversation that is on screen, in order. */
function conversationItems(anchor: MessageView): ViewerItem[] {
  const groups: MessageImages[] = framesInOrder()
    .map((f) => ({ message: f.message(), body: frameBodyImages(f) }))
    .filter((g) => g.message.accountId === anchor.accountId && g.message.threadId === anchor.threadId);
  if (!groups.some((g) => sameMessage(g.message, anchor))) groups.push({ message: anchor, body: [] });
  groups.sort((a, b) => a.message.date - b.message.date);
  return buildItems(groups);
}

/**
 * A click on a picture in a message body (the Rust event's payload, or an
 * anchor href from the in-frame listener). Validated by bridge.ts; anything
 * that doesn't name a picture on screen is dropped.
 */
export function openFromRequest(raw: unknown, from?: { doc: unknown }) {
  const hit = resolveImageRequest(raw, from);
  if (!hit) {
    console.warn("penguin: ignored an image-viewer request that names no picture on screen");
    return;
  }
  const m = hit.entry.message();
  const items = conversationItems(m);
  const i = items.findIndex((x) => x.key === bodyKey(m, hit.index));
  if (i >= 0) openImageViewer(items, i);
}

/** An image attachment's card or menu: the viewer, starting at it. */
export function openImageAttachment(m: MessageView, a: AttachmentMeta) {
  const items = conversationItems(m);
  openImageViewer(items, items.findIndex((x) => x.key === attachmentKey(m, a)));
}

const current = (): ViewerItem | null => (state ? state.items[state.index] ?? null : null);

// ---------------------------------------------------------------------------
// Zoom controller (the mounted viewer registers itself for the shortcuts)
// ---------------------------------------------------------------------------
interface Controller {
  toggle: () => void;
  zoomBy: (factor: number) => void;
}
let controller: Controller | null = null;

export function ImageViewerHost() {
  const open = useUi((s) => s.overlay === "image");
  const st = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => state,
  );

  // Always registered (the ? sheet lists them); live only while open.
  useEffect(
    () =>
      registerShortcuts([
        { id: "iv.close", keys: "escape", label: "Close image", group: "Image viewer", hidden: true, when: isOpen, run: close },
        { id: "iv.prev", keys: "arrowleft", label: "Previous image", group: "Image viewer", when: isOpen, run: () => step(-1) },
        { id: "iv.next", keys: "arrowright", label: "Next image", group: "Image viewer", when: isOpen, run: () => step(1) },
        { id: "iv.zoom", keys: "z", label: "Zoom to 100% / fit", group: "Image viewer", when: isOpen, run: () => controller?.toggle() },
        { id: "iv.in", keys: "=", label: "Zoom in", group: "Image viewer", when: isOpen, run: () => controller?.zoomBy(1.25) },
        { id: "iv.out", keys: "-", label: "Zoom out", group: "Image viewer", when: isOpen, run: () => controller?.zoomBy(0.8) },
        { id: "iv.copy", keys: "mod+c", label: "Copy image", group: "Image viewer", when: isOpen, run: () => current() && void copyImage(current()!) },
        { id: "iv.save", keys: "mod+s", label: "Save image", group: "Image viewer", when: isOpen, run: () => current() && void saveImage(current()!) },
        { id: "iv.preview", keys: "mod+o", label: openLabel, group: "Image viewer", when: isOpen, run: () => current() && void openInPreview(current()!) },
      ]),
    [],
  );

  // Clicks on pictures in message bodies, relayed by the native link guard.
  useEffect(() => {
    let off: (() => void) | null = null;
    let live = true;
    onImageOpen((payload) => openFromRequest(payload)).then((u) => (live ? (off = u) : u()));
    return () => {
      live = false;
      off?.();
    };
  }, []);

  if (!open || !st) return null;
  return <Viewer st={st} />;
}

// ---------------------------------------------------------------------------
// The viewer
// ---------------------------------------------------------------------------
type Src = { key: string; url?: string; error?: string };
interface View {
  /** null: fit to the window. */
  scale: number | null;
  x: number;
  y: number;
  animate: boolean;
}
const FIT: View = { scale: null, x: 0, y: 0, animate: true };
/** Room around a fitted picture (the bar is above the stage). */
const PAD = 28;
const MAX_SCALE = 8;

function Viewer({ st }: { st: State }) {
  const tip = useKeyTip();
  const item = st.items[st.index];
  const rootRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState<Src>({ key: "" });
  const [nat, setNat] = useState<{ key: string; w: number; h: number } | null>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [view, setView] = useState<View>(FIT);
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ id: number; x0: number; y0: number; px: number; py: number; moved: boolean; onImage: boolean } | null>(null);

  // Keys must reach the app, not a message frame that had focus.
  useEffect(() => rootRef.current?.focus({ preventScroll: true }), []);

  // The picture's source.
  useEffect(() => {
    let live = true;
    setView(FIT);
    if (item.kind === "body") {
      setSrc({ key: item.key, url: item.src });
    } else {
      setSrc({ key: item.key });
      previewImage(item.message, item.attachment).then(
        (p) => live && setSrc({ key: item.key, url: p.dataUrl }),
        (e) => live && setSrc({ key: item.key, error: asCommandError(e).message }),
      );
    }
    // Warm the neighbours' attachment previews so ←/→ are instant.
    for (const n of [st.items[st.index + 1], st.items[st.index - 1]]) {
      if (n?.kind === "attachment" && n.size <= 8 * 1024 * 1024) loadPreview(n.message, n.attachment).catch(() => undefined);
    }
    return () => {
      live = false;
    };
  }, [item.key]);

  useLayoutEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const measure = () => setBox({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const size = nat && nat.key === item.key ? nat : item.kind === "body" && item.width > 0 ? { key: item.key, w: item.width, h: item.height } : null;
  const fit = size && box.w > 0 ? Math.min(1, (box.w - 2 * PAD) / size.w, (box.h - 2 * PAD) / size.h) : 1;
  const scale = view.scale ?? fit;
  const zoomed = view.scale !== null && scale > fit + 1e-3;

  const clamp = (x: number, y: number, s: number) => {
    if (!size) return { x: 0, y: 0 };
    const mx = Math.max(0, (size.w * s - box.w) / 2 + PAD);
    const my = Math.max(0, (size.h * s - box.h) / 2 + PAD);
    const fx = size.w * s > box.w - 2 * PAD ? mx : 0;
    const fy = size.h * s > box.h - 2 * PAD ? my : 0;
    return { x: Math.min(fx, Math.max(-fx, x)), y: Math.min(fy, Math.max(-fy, y)) };
  };

  /** Zoom to `next`, keeping the picture point under (px, py) (stage-centre coords) in place. */
  const zoomTo = (next: number, px: number, py: number, animate: boolean) => {
    const s = Math.min(Math.max(next, fit), Math.max(MAX_SCALE, fit));
    if (s <= fit + 1e-3) {
      setView({ scale: null, x: 0, y: 0, animate });
      return;
    }
    const ux = (px - view.x) / scale;
    const uy = (py - view.y) / scale;
    setView({ scale: s, ...clamp(px - ux * s, py - uy * s, s), animate });
  };
  const toggle = (px = 0, py = 0) => (zoomed ? setView(FIT) : zoomTo(fit < 1 ? 1 : 2, px, py, true));

  const ctl = useRef<Controller>({ toggle: () => {}, zoomBy: () => {} });
  ctl.current = { toggle: () => toggle(), zoomBy: (f) => zoomTo(scale * f, 0, 0, true) };
  useEffect(() => {
    const c: Controller = { toggle: () => ctl.current.toggle(), zoomBy: (f) => ctl.current.zoomBy(f) };
    controller = c;
    return () => {
      if (controller === c) controller = null;
    };
  }, []);

  // Wheel / pinch zooms around the pointer. Non-passive, to keep the page still.
  const wheel = useRef<(e: WheelEvent) => void>(() => {});
  wheel.current = (e: WheelEvent) => {
    e.preventDefault();
    if (!size) return;
    const r = stageRef.current!.getBoundingClientRect();
    const dy = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? r.height : 1);
    const factor = Math.exp(-dy * (e.ctrlKey ? 0.01 : 0.002));
    zoomTo(scale * factor, e.clientX - r.left - r.width / 2, e.clientY - r.top - r.height / 2, false);
  };
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const h = (e: WheelEvent) => wheel.current(e);
    el.addEventListener("wheel", h, { passive: false });
    return () => el.removeEventListener("wheel", h);
  }, []);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const onImage = target.classList.contains("iv-img");
    drag.current = { id: e.pointerId, x0: e.clientX, y0: e.clientY, px: view.x, py: view.y, moved: false, onImage };
    // At fit (or with ⌥ when zoomed) a drag on the picture takes it out as a
    // file: WebKit decides on mouse-down whether an element is draggable, so
    // set it here, and don't capture the pointer (that would stop the drag).
    const exporting = onImage && canDragFiles && (!zoomed || e.altKey);
    if (onImage) (target as HTMLImageElement).draggable = exporting;
    if (!exporting) e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    const dx = e.clientX - d.x0;
    const dy = e.clientY - d.y0;
    if (!d.moved && Math.hypot(dx, dy) < 4) return;
    d.moved = true;
    if (!zoomed || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
    setDragging(true);
    setView({ scale: view.scale, ...clamp(d.px + dx, d.py + dy, scale), animate: false });
  };
  const onPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    drag.current = null;
    setDragging(false);
    if (d.moved) return;
    if (d.onImage) {
      const r = stageRef.current!.getBoundingClientRect();
      toggle(e.clientX - r.left - r.width / 2, e.clientY - r.top - r.height / 2);
    } else if (!zoomed) close();
  };

  const onDragStart = (e: ReactDragEvent) => {
    drag.current = null;
    setDragging(false);
    dragOut(e, itemDragSource(item));
  };

  const m = item.message;
  const url = src.key === item.key ? src.url : undefined;
  const error = src.key === item.key ? src.error : undefined;
  const dims = size ? `${size.w} × ${size.h}` : null;
  const byteSize = item.size != null ? bytes(item.size) : null;
  const n = st.items.length;

  const onContextMenu = (e: ReactMouseEvent) => {
    // The bar's own buttons and the arrows keep the menu to the picture.
    drag.current = null;
    setDragging(false);
    // Save All covers the pictures of this picture's message.
    const siblings = st.items.filter((x) => sameMessage(x.message, m));
    showContextMenu(
      e,
      imageMenu(
        item,
        { where: "viewer", saved: savedPath(item) !== null, nativeFiles, zoomed, canZoom: !!size, broken: !!error, openLabel, saveAllCount: canSaveAll(siblings) ? siblings.length : 0 },
        {
          saveAll: () => void saveAllImages(m, siblings),
          copy: () => void copyImage(item),
          copyAddress: () => copyImageAddress(item),
          save: () => void saveImage(item),
          saveAs: () => void saveImageAs(item),
          openInPreview: () => void openInPreview(item),
          reveal: () => revealImage(item),
          toggleZoom: () => toggle(),
          close,
        },
      ),
      { label: `Image ${item.name}` },
    );
  };

  return (
    <div className="iv" ref={rootRef} tabIndex={-1} role="dialog" aria-modal="true" aria-label={`Image: ${item.name}`} onContextMenu={onContextMenu}>
      <header className="iv-bar">
        <div className="iv-title grow">
          <div className="iv-name truncate">{item.name}</div>
          <div className="iv-meta truncate">
            {[dims, byteSize, displayName(m.from), messageTime(m.date)].filter(Boolean).join(" · ")}
          </div>
        </div>
        {n > 1 && (
          <span className="iv-count tnum">
            {st.index + 1} of {n}
          </span>
        )}
        <span className="iv-zoom tnum" title={tip("Zoom to 100% / fit", "z")}>
          <button type="button" className="iv-btn" onClick={() => toggle()} disabled={!size}>
            <Icon name={zoomed ? "shrink" : "expand"} size="xs" />
            {Math.round(scale * 100)}%
          </button>
        </span>
        <span className="iv-sep" />
        {canDragFiles && (
          <button
            type="button"
            className="iv-btn iv-grip"
            draggable
            onDragStart={(e) => dragOut(e, itemDragSource(item))}
            onClick={() => toast({ message: "Drag this to Finder, the Desktop or another app" })}
            title="Drag to Finder or another app"
            disabled={!!error}
          >
            <Icon name="grip" size="xs" />
            Drag
          </button>
        )}
        <button type="button" className="iv-btn" onClick={() => void copyImage(item)} title={tip("Copy image", "mod+c")} disabled={!!error}>
          <Icon name="copy" size="xs" />
          Copy
        </button>
        <button type="button" className="iv-btn" onClick={() => void saveImage(item)} title={tip("Save to Downloads", "mod+s")}>
          <Icon name="download" size="xs" />
          Save
          <Keys keys="mod+s" />
        </button>
        <button type="button" className="iv-btn" onClick={() => void openInPreview(item)} title={tip(openLabel, "mod+o")}>
          <Icon name="external" size="xs" />
          {openLabel}
        </button>
        <button type="button" className="iv-btn" onClick={close} aria-label="Close">
          <Icon name="x" size="xs" />
          <Kbd>Esc</Kbd>
        </button>
      </header>
      <div
        className={"iv-stage" + (zoomed ? " is-zoomed" : "") + (dragging ? " is-dragging" : "") + (size && fit >= 1 && !zoomed ? " is-small" : "")}
        ref={stageRef}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => {
          drag.current = null;
          setDragging(false);
        }}
      >
        {error ? (
          <div className="iv-msg">
            <div className="iv-msg-title">Couldn't show this image</div>
            <div className="iv-msg-body">{error}</div>
          </div>
        ) : !url ? (
          <div className="iv-msg">
            <span className="iv-spinner" aria-hidden="true" />
            <span className="iv-msg-body">Loading image…</span>
          </div>
        ) : (
          <img
            key={item.key}
            className={"iv-img" + (view.animate ? " is-animated" : "")}
            src={url}
            alt={item.name}
            // Set per press in onPointerDown (a file drag at fit or with ⌥).
            draggable={false}
            onDragStart={onDragStart}
            // Never tell an image host where it was shown.
            referrerPolicy="no-referrer"
            onLoad={(e) => setNat({ key: item.key, w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
            style={
              size
                ? { width: size.w, height: size.h, transform: `translate(calc(-50% + ${view.x}px), calc(-50% + ${view.y}px)) scale(${scale})` }
                : { visibility: "hidden" }
            }
          />
        )}
        {n > 1 && (
          <>
            <button
              type="button"
              className="iv-nav iv-prev"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={() => step(-1)}
              disabled={st.index === 0}
              aria-label="Previous image"
              title={tip("Previous", "←")}
            >
              <Icon name="left" size="sm" />
            </button>
            <button
              type="button"
              className="iv-nav iv-next"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={() => step(1)}
              disabled={st.index === n - 1}
              aria-label="Next image"
              title={tip("Next", "→")}
            >
              <Icon name="right" size="sm" />
            </button>
          </>
        )}
      </div>
    </div>
  );
}
