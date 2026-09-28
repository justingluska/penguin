// "Add account" for people who are already set up: the same email-first flow
// as setup (flow.tsx) in a modal, without the rail. Mounted once by App;
// anyone opens it with openAddAccount(). The dialog and the setup flow behind
// it are their own chunk (AddAccountDialog.tsx, see lib/lazy.ts).
import { useSyncExternalStore } from "react";
import type { Account, PendingSetup } from "../../lib/types";
import { lazyScreen } from "../../lib/lazy";

let open = false;
let notify: ((a: Account) => void) | undefined;
let resumeWith: PendingSetup | null = null;
const subs = new Set<() => void>();
const setOpen = (v: boolean) => {
  open = v;
  subs.forEach((f) => f());
};

/**
 * Show the Add account modal; `onAdded` runs for each account signed in.
 * `resume`: an unfinished setup, opened at the step it stopped at.
 */
export function openAddAccount(onAdded?: (a: Account) => void, resume?: PendingSetup): void {
  notify = onAdded;
  resumeWith = resume ?? null;
  setOpen(true);
}

export function AddAccountModal() {
  const isOpen = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => open,
  );
  return isOpen ? <Dialog onClose={() => setOpen(false)} /> : null;
}

const Dialog = lazyScreen(() => import("./AddAccountDialog").then((m) => m.AddAccountDialog));

/** The setup the open dialog resumes (openAddAccount's `resume`), if any. */
export function addAccountResume(): PendingSetup | null {
  return resumeWith;
}

/** Tell openAddAccount's caller about an account the dialog signed in. */
export function notifyAccountAdded(a: Account): void {
  notify?.(a);
}

