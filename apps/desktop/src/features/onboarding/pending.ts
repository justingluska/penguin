// Unfinished Add account setups (Settings.pendingSetups): recorded by the
// flow as it moves (flow.tsx), given the last sign-in/connect error, cleared
// when the account is added or the user removes it. Shown in Settings →
// Accounts and on the Add account start screen, each with Resume.
import { currentSettings, updateSettings, useSetting } from "../../lib/settings";
import type { PendingSetup, SetupKind } from "../../lib/types";

const PROVIDER_NAMES: Record<SetupKind, string> = {
  google: "Google",
  microsoft: "Microsoft",
  yahoo: "Yahoo",
  aol: "AOL",
  icloud: "iCloud",
  fastmail: "Fastmail",
  proton: "Proton Mail",
  imap: "IMAP",
  unsupported: "Other",
};

export function providerName(kind: SetupKind): string {
  return PROVIDER_NAMES[kind] ?? "Other";
}

export function usePendingSetups(): PendingSetup[] {
  return useSetting("pendingSetups");
}

function write(list: PendingSetup[]) {
  updateSettings({ pendingSetups: list }).catch((e) => console.warn("penguin: couldn't save the unfinished setup", e));
}

const key = (email: string) => email.trim().toLowerCase();

/** The flow reached `step` of `kind` for `email`: add or update its entry (a new provider starts it over). */
export function notePendingSetup(email: string, kind: SetupKind, step: string): void {
  const e = key(email);
  const list = currentSettings().pendingSetups;
  const old = list.find((p) => p.email === e);
  if (old && old.kind === kind && old.step === step) return;
  const entry: PendingSetup =
    old && old.kind === kind ? { ...old, step } : { email: e, kind, step, startedAt: Date.now(), lastError: null };
  write([entry, ...list.filter((p) => p.email !== e)]);
}

/** A sign-in or connect attempt for `email` failed (no-op without an entry). */
export function notePendingError(email: string, error: string): void {
  const e = key(email);
  const list = currentSettings().pendingSetups;
  if (!list.some((p) => p.email === e && p.lastError !== error)) return;
  write(list.map((p) => (p.email === e ? { ...p, lastError: error } : p)));
}

/** Added, or removed by the user. */
export function clearPendingSetup(email: string): void {
  const e = key(email);
  const list = currentSettings().pendingSetups;
  if (list.some((p) => p.email === e)) write(list.filter((p) => p.email !== e));
}
