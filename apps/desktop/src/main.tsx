// First: demo mode swaps localStorage before any module can read it.
import "./lib/demo";
import React from "react";
import ReactDOM from "react-dom/client";
import "./styles/fonts.css";
import "./styles/penguin.css";
import "./styles/app.css";
import "./styles/themes.css";
import "./styles/list-styles.css";
import App from "./App";
import { getUi } from "./lib/ui";
import { logClientEvent } from "./lib/api";

// Theme before first paint (no inline script: the CSP forbids it).
document.documentElement.dataset.theme = getUi().theme;

// Inside the desktop app the window uses an overlay title bar (see
// tauri.conf.json: titleBarStyle "Overlay"), so the page must reserve the
// traffic-light strip itself and provide the drag region.
if ("__TAURI_INTERNALS__" in window) document.documentElement.classList.add("overlay-titlebar");

// Script errors nothing caught go to penguin.log (Settings → Developer → View log).
window.addEventListener("error", (e) => {
  logClientEvent({ level: "error", source: "window", what: e.filename ? `${e.filename.split("/").pop()}:${e.lineno}` : "script", message: e.message });
});
window.addEventListener("unhandledrejection", (e) => {
  const r = e.reason;
  logClientEvent({ level: "error", source: "window", what: "unhandled promise", message: r instanceof Error ? `${r.name}: ${r.message}` : String(r) });
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
