// Draggable divider between two panes. While dragging it writes the width
// straight to a CSS variable on the app shell (no React render per frame) and
// commits once on release. Double-click resets to the default width.
import { useRef } from "react";
import { clamp } from "../lib/layout";

export function Splitter({
  cssVar,
  value,
  min,
  max,
  onCommit,
  onReset,
  label,
}: {
  /** CSS custom property on `.app` that sizes the pane to the left. */
  cssVar: string;
  value: number;
  min: number;
  /** Evaluated at drag start so it can depend on the window size. */
  max: () => number;
  onCommit: (w: number) => void;
  onReset: () => void;
  label: string;
}) {
  const drag = useRef<{ x: number; w: number; max: number; last: number; host: HTMLElement } | null>(null);

  return (
    <div
      className="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      title={`Drag to resize · double-click to reset`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        const host = e.currentTarget.closest<HTMLElement>(".app");
        if (!host) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { x: e.clientX, w: value, max: Math.max(min, max()), last: value, host };
        document.documentElement.classList.add("is-resizing");
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        const w = clamp(d.w + e.clientX - d.x, { min, max: d.max });
        if (w === d.last) return;
        d.last = w;
        d.host.style.setProperty(cssVar, `${w}px`);
      }}
      onPointerUp={(e) => end(e.currentTarget, e.pointerId)}
      onPointerCancel={(e) => end(e.currentTarget, e.pointerId)}
      onDoubleClick={onReset}
    />
  );

  function end(el: HTMLElement, pointerId: number) {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    if (el.hasPointerCapture(pointerId)) el.releasePointerCapture(pointerId);
    document.documentElement.classList.remove("is-resizing");
    if (d.last !== d.w) onCommit(d.last);
  }
}
