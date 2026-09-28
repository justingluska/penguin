// Whether the Welcome setup is showing. It opens by itself once on a new
// install (WelcomeHost), and again from ⌘K "Set up Penguin…" or Settings →
// General → "Set up Penguin again".
import { useSyncExternalStore } from "react";
import { getUi, setUi } from "../../lib/ui";

let open = false;
const subs = new Set<() => void>();
const notify = () => subs.forEach((f) => f());

export function openWelcome() {
  if (getUi().overlay) setUi({ overlay: null });
  open = true;
  notify();
}

export function closeWelcome() {
  if (!open) return;
  open = false;
  notify();
}

export function isWelcomeOpen(): boolean {
  return open;
}

export function useWelcomeOpen(): boolean {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => open,
  );
}
