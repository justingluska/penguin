// A promise-based confirm dialog for actions started outside a screen that
// has its own (context menus: remove account, delete label…).
//
//   if (await confirmAction({ title, body, confirm: "Remove" })) …
//
// Renders Settings' ConfirmDialog (same look everywhere). While it's up the
// app's single-key shortcuts are held back so E or # can't act behind it.
import { useEffect, useSyncExternalStore } from "react";
import { ConfirmDialog } from "../features/settings/parts";

interface Pending {
  title: string;
  body: string;
  confirm: string;
  resolve: (ok: boolean) => void;
}

let pending: Pending | null = null;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

export function confirmAction(opts: { title: string; body: string; confirm: string }): Promise<boolean> {
  pending?.resolve(false);
  return new Promise((resolve) => {
    pending = { ...opts, resolve };
    emit();
  });
}

function settle(ok: boolean) {
  const p = pending;
  pending = null;
  emit();
  p?.resolve(ok);
}

export function ConfirmHost() {
  const p = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => pending,
  );
  useEffect(() => {
    if (!p) return;
    // Capture phase: the dialog's buttons still get Enter/Space/Tab as
    // default actions, but no app shortcut sees the key.
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.key === "Escape") return;
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [p]);
  if (!p) return null;
  return (
    <div className="app-confirm">
      <ConfirmDialog title={p.title} body={p.body} confirm={p.confirm} onConfirm={() => settle(true)} onCancel={() => settle(false)} />
    </div>
  );
}
