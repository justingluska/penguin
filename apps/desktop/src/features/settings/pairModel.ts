// Pair iPhone's pure part (PairIphone.tsx): the dialog's phases, how each
// penguin://pair-iphone event moves them, and the wording.
import type { PairAccount, PairEvent, PairPhone, PairStart } from "../../lib/types";

export type PairPhase =
  | { kind: "starting" }
  | { kind: "showing"; start: PairStart }
  | { kind: "connected"; start: PairStart; phone: PairPhone; code: string; accounts: PairAccount[]; clientNote: string | null; answered: boolean }
  | { kind: "sending"; start: PairStart; phone: PairPhone }
  | { kind: "done"; phoneName: string; imported: string[]; failed: { email: string; reason: string }[]; signIn: string[] }
  | { kind: "declined" }
  | { kind: "error"; message: string };

/** Fold an event into the phase. Events out of order (a late "connected") are ignored. */
export function nextPhase(p: PairPhase, e: PairEvent): PairPhase {
  switch (e.state) {
    case "connected":
      return p.kind === "showing"
        ? { kind: "connected", start: p.start, phone: e.phone, code: e.code, accounts: e.accounts, clientNote: e.clientNote, answered: false }
        : p;
    case "sending":
      return p.kind === "connected" ? { kind: "sending", start: p.start, phone: p.phone } : p;
    case "done":
      return { kind: "done", phoneName: e.phoneName, imported: e.imported, failed: e.failed, signIn: e.signIn };
    case "declined":
      return { kind: "declined" };
    case "failed":
      return { kind: "error", message: e.message };
  }
}

/** Whether closing the dialog must stop the Mac's listener. */
export function pairingLive(p: PairPhase): boolean {
  return p.kind === "starting" || p.kind === "showing" || p.kind === "connected" || p.kind === "sending";
}

export function plural(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

export function statusLabel(status: PairAccount["status"]): string {
  switch (status) {
    case "token":
      return "Moves";
    case "alreadyOnPhone":
      return "Already there";
    case "signIn":
      return "Sign in on iPhone";
  }
}

/** The Send button. */
export function sendLabel(accounts: PairAccount[]): string {
  const moving = accounts.filter((a) => a.status === "token").length;
  return moving > 0 ? `Send ${plural(moving, "account")}` : "Send the list";
}

/** "1:42". */
export function formatLeft(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
