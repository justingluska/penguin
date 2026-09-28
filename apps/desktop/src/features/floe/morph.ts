// Moving between the pane layout and Floe without swapping the shell: the
// list (and its header) stays mounted and keeps its place while the sidebar
// slides out to the left, the reading pane out to the right and the status
// bar down, and the list grows into the space (floe.css, "Morph"). OWNER: floe.
//
// The reading pane is frozen at its width for the slide (--preview-w), so it
// moves instead of reflowing its message at every frame. Once in Floe the
// panes unmount, as before, so the hidden reading pane doesn't render the
// cursor's thread; leaving mounts them collapsed first, then slides them in.
import { useEffect, useLayoutEffect, useState, type RefObject } from "react";
import { getLayout } from "../../lib/layout";
import { useFloe } from "./state";
import "./floe.css";

/** Matches --floe-morph in floe.css. */
const MORPH_MS = 280;

const reducedMotion = () => typeof window !== "undefined" && !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

export interface FloeMorph {
  /** The Floe layout is applied (the .is-floe class). */
  floe: boolean;
  /** A slide is running (the .is-morphing class). */
  morphing: boolean;
  /** Render the sidebar, reading pane and status bar. */
  panes: boolean;
  /** Floe's roomier rows: from the start of entering to the end of leaving, so rows change once. */
  floeRows: boolean;
}

export function useFloeMorph(appRef: RefObject<HTMLElement | null>): FloeMorph {
  const want = useFloe();
  const [floe, setFloe] = useState(want);
  const [morphing, setMorphing] = useState(false);

  // The setting changed: freeze the reading pane's width, then start the slide
  // (entering) or mount the panes still collapsed (leaving; the next effect slides them in).
  useLayoutEffect(() => {
    if (floe === want) return;
    const app = appRef.current;
    if (!app || reducedMotion()) {
      setFloe(want);
      setMorphing(false);
      return;
    }
    app.style.setProperty("--preview-w", `${previewWidth(app, want)}px`);
    setMorphing(true);
    if (want) setFloe(true);
  }, [want, floe, appRef]);

  // Leaving: the panes just mounted under .is-floe. Resolve their collapsed
  // style first (the reflow), so dropping the class animates from there.
  useLayoutEffect(() => {
    if (!morphing || want || !floe) return;
    void appRef.current?.offsetWidth;
    setFloe(false);
  }, [morphing, want, floe, appRef]);

  useEffect(() => {
    if (!morphing) return;
    const t = setTimeout(() => setMorphing(false), MORPH_MS + 40);
    return () => clearTimeout(t);
  }, [morphing, floe]);

  return { floe, morphing, panes: !floe || morphing, floeRows: want || morphing };
}

/** The reading pane's width: as it is now (entering), or as the pane layout will give it (leaving). */
function previewWidth(app: HTMLElement, entering: boolean): number {
  if (entering) return app.querySelector<HTMLElement>(".preview")?.offsetWidth ?? 0;
  const l = getLayout();
  // .preview's min-width in the pane layout is 320px (app.css).
  return Math.max(320, app.clientWidth - (l.sidebarCollapsed ? 0 : l.sidebarW) - l.listW);
}
