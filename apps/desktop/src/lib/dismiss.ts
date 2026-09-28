// Click-away for transient popovers (person card, event, sync details,
// switchers, small menus): a press anywhere outside closes them.
//
// Two things a plain window "mousedown" listener misses:
// - The message body is a no-script sandboxed iframe. WebKit never runs
//   parent listeners for presses inside it; the press only moves focus into
//   the frame, which blurs our window. So a window blur that leaves an iframe
//   focused counts as a press outside. (A blur from switching apps leaves no
//   iframe focused and keeps the popover.)
// - The button that opened the popover. Closing on its press, then its click
//   reopening, looks like "clicking it again does nothing". Pass it as inside
//   and let its own click toggle.
import { useEffect, useRef, type RefObject } from "react";

export type Inside = RefObject<Element | null> | (() => Element | null | undefined);

/** Call `dismiss` on a press outside `isInside`. Capture phase, so a handler that stops propagation can't keep it open. */
export function watchOutside(isInside: (t: Node) => boolean, dismiss: () => void): () => void {
  const onDown = (e: MouseEvent) => {
    if (e.target instanceof Node && !isInside(e.target)) dismiss();
  };
  let timer = 0;
  const onBlur = () => {
    clearTimeout(timer);
    // The frame becomes activeElement once the blur settles.
    timer = window.setTimeout(() => {
      const f = document.activeElement;
      if (f instanceof HTMLIFrameElement && !isInside(f)) dismiss();
    }, 0);
  };
  window.addEventListener("mousedown", onDown, true);
  window.addEventListener("blur", onBlur);
  return () => {
    clearTimeout(timer);
    window.removeEventListener("mousedown", onDown, true);
    window.removeEventListener("blur", onBlur);
  };
}

function resolve(i: Inside): Element | null | undefined {
  return typeof i === "function" ? i() : i.current;
}

/**
 * While `active`, a press outside every `inside` element (or matching
 * `ignore`, a selector for things that float outside but belong to the
 * popover, like a right-click menu opened from it) calls `onDismiss`.
 */
export function useDismiss(active: boolean, onDismiss: () => void, inside: Inside[], ignore?: string) {
  const latest = useRef({ onDismiss, inside, ignore });
  latest.current = { onDismiss, inside, ignore };
  useEffect(() => {
    if (!active) return;
    return watchOutside(
      (t) => {
        const { inside, ignore } = latest.current;
        if (inside.some((i) => resolve(i)?.contains(t))) return true;
        return !!ignore && t instanceof Element && !!t.closest(ignore);
      },
      () => latest.current.onDismiss(),
    );
  }, [active]);
}
