// Pointer-based drag to reorder a vertical list (the sidebar's accounts,
// Settings → Accounts). The picked-up row follows the pointer, a line marks
// where it will land, and releasing calls onMove(id, toIndex) with the index
// counted among the other rows (app/accountOrder.ts moveWithin). A press
// that moves less than DRAG_THRESHOLD px stays a click; a real drag
// swallows the click that follows. Esc cancels.
import { useCallback, useEffect, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import { dropIndex } from "../app/accountOrder";

/** Pixels the pointer must travel before a press becomes a drag. */
export const DRAG_THRESHOLD = 4;

interface Press {
  id: string;
  pointerId: number;
  startX: number;
  startY: number;
  dragging: boolean;
}

export interface DragState {
  id: string;
  /** Pointer travel since the press, for the lifted row's transform. */
  dy: number;
  /** Where the row lands, among the other rows. */
  index: number;
  /** The drop line's offset from the top of the list container, px. */
  lineY: number;
}

export interface DragReorder {
  drag: DragState | null;
  /** Ref for the list container (position: relative; the drop line is placed in it). */
  listRef: (el: HTMLElement | null) => void;
  /** Ref for a row, by id. */
  rowRef: (id: string) => (el: HTMLElement | null) => void;
  /** Starts a press on the row (or on its handle). */
  onPointerDown: (id: string, e: ReactPointerEvent) => void;
  /** Style for a row: the lifted one follows the pointer. */
  rowStyle: (id: string) => CSSProperties | undefined;
}

export function useDragReorder(ids: string[], onMove: (id: string, toIndex: number) => void): DragReorder {
  const [drag, setDrag] = useState<DragState | null>(null);
  const list = useRef<HTMLElement | null>(null);
  const rows = useRef(new Map<string, HTMLElement>());
  const press = useRef<Press | null>(null);
  const shown = useRef<DragState | null>(null);
  const latest = useRef({ ids, onMove });
  latest.current = { ids, onMove };
  const refFns = useRef(new Map<string, (el: HTMLElement | null) => void>());

  const measure = useCallback((id: string, y: number): Omit<DragState, "dy"> | null => {
    const box = list.current?.getBoundingClientRect();
    if (!box) return null;
    const others = latest.current.ids
      .filter((x) => x !== id)
      .map((x) => rows.current.get(x)?.getBoundingClientRect())
      .filter((r): r is DOMRect => !!r);
    const index = dropIndex(y, others);
    let lineY: number;
    if (others.length === 0) lineY = 0;
    else if (index < others.length) {
      // Halfway into the gap above the row it goes before.
      const prevBottom = index > 0 ? others[index - 1].bottom : others[0].top;
      lineY = (prevBottom + others[index].top) / 2 - box.top;
    } else lineY = others[others.length - 1].bottom - box.top;
    return { id, index, lineY };
  }, []);

  const end = useCallback((commit: boolean) => {
    const p = press.current;
    press.current = null;
    const d = shown.current;
    shown.current = null;
    setDrag(null);
    document.documentElement.classList.remove("is-reordering");
    if (!p?.dragging) return;
    // The release lands on the lifted row: don't let it count as a click.
    const swallow = (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("click", swallow, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    if (!commit || !d) return;
    const from = latest.current.ids.indexOf(d.id);
    if (from >= 0 && d.index !== from) latest.current.onMove(d.id, d.index);
  }, []);

  useEffect(() => {
    const move = (e: PointerEvent) => {
      const p = press.current;
      if (!p || e.pointerId !== p.pointerId) return;
      const dx = e.clientX - p.startX;
      const dy = e.clientY - p.startY;
      if (!p.dragging) {
        if (Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
        p.dragging = true;
        document.documentElement.classList.add("is-reordering");
      }
      e.preventDefault();
      const m = measure(p.id, e.clientY);
      if (!m) return;
      shown.current = { ...m, dy };
      setDrag(shown.current);
    };
    const up = (e: PointerEvent) => {
      if (press.current && e.pointerId === press.current.pointerId) end(true);
    };
    const cancel = (e: PointerEvent) => {
      if (press.current && e.pointerId === press.current.pointerId) end(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape" && press.current?.dragging) {
        e.preventDefault();
        e.stopPropagation();
        end(false);
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", key, true);
      document.documentElement.classList.remove("is-reordering");
    };
  }, [measure, end]);

  const onPointerDown = useCallback((id: string, e: ReactPointerEvent) => {
    // Primary button only; a right-click opens the menu instead.
    if (e.button !== 0 || e.ctrlKey || latest.current.ids.length < 2) return;
    press.current = { id, pointerId: e.pointerId, startX: e.clientX, startY: e.clientY, dragging: false };
  }, []);

  const listRef = useCallback((el: HTMLElement | null) => {
    list.current = el;
  }, []);

  const rowRef = useCallback((id: string) => {
    let fn = refFns.current.get(id);
    if (!fn) {
      fn = (el: HTMLElement | null) => {
        if (el) rows.current.set(id, el);
        else rows.current.delete(id);
      };
      refFns.current.set(id, fn);
    }
    return fn;
  }, []);

  const rowStyle = useCallback(
    (id: string): CSSProperties | undefined => (drag?.id === id ? { transform: `translateY(${drag.dy}px)` } : undefined),
    [drag],
  );

  return { drag, listRef, rowRef, onPointerDown, rowStyle };
}
