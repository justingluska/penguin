// The theme on <html>, for every window's root (App.tsx, windowShell.tsx).
import { useEffect } from "react";
import { getUi, setUi, useUi } from "../lib/ui";

/** Theme: follow the system until the user presses T; mirror onto <html>. */
export function useTheme() {
  const theme = useUi((s) => s.theme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!mq) return;
    const onChange = () => {
      if (getUi().themeSource === "system") setUi({ theme: mq.matches ? "dark" : "light" });
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
}
