// Which window this page is, fixed for the page's life: the main window, a
// conversation or a composer (lib/windowRoute.ts reads the query; inside
// Tauri the label is the window's own).
import { MAIN_LABEL, parseWindowRoute, type WindowRoute } from "./windowRoute";

export const windowRoute: WindowRoute = typeof location === "undefined" ? { kind: "main", label: MAIN_LABEL } : parseWindowRoute(location.search);

const tauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** This window's label: Tauri's own inside the app, the route's in a browser. */
export const thisWindowLabel: string = (() => {
  if (tauri) {
    const internals = (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWebview?: { label?: string } } } }).__TAURI_INTERNALS__;
    const label = internals?.metadata?.currentWebview?.label;
    if (label) return label;
  }
  return windowRoute.label;
})();

export const isMainWindow = thisWindowLabel === MAIN_LABEL;
