// Verification codes (penguin-core otp.rs finds them at ingest; ThreadSummary
// and MessageView carry `otp`). This module holds the shared bits: freshness,
// copying, and ⌘⇧C. OWNER: otp.
//
// Codes are never logged or sent anywhere: the only thing done with one is
// writing it to the clipboard on an explicit click or key press.
import { useSyncExternalStore } from "react";
import type { Otp, ThreadRef } from "../../lib/types";
import { registerShortcuts } from "../../lib/keyboard";
import { getUi } from "../../lib/ui";
import { list } from "../../app/store";
import { toast } from "../../components/Toast";

/** Services give codes 5–30 minutes; past this the pill dims ("expired?"). */
export const FRESH_MS = 20 * 60_000;
/** Past this the row shows its snippet again: a day-old code is dead. */
export const SHOW_MS = 24 * 60 * 60_000;

export function otpAge(otp: Otp, now: number): number {
  return Math.max(0, now - otp.date);
}

/**
 * Display groups for a code: digits split for reading ("918 273",
 * "4071 8325", "0418 273"); anything with letters or separators as is.
 * Copying always uses the raw code.
 */
export function groupCode(code: string): string[] {
  if (!/^\d+$/.test(code) || code.length < 6) return [code];
  const head = code.length === 6 || code.length === 9 ? 3 : 4;
  const out: string[] = [];
  for (let i = 0; i < code.length; i += head) out.push(code.slice(i, i + head));
  return out;
}

/** A code worth showing in a list row right now. */
export function rowCode(otp: Otp | null | undefined, now: number): Otp | null {
  if (!otp) return null;
  if (otp.kind === "code" && !otp.code) return null;
  return otpAge(otp, now) < SHOW_MS ? otp : null;
}

// One shared 15 s clock, ticking only while something shows a code.
let now = Date.now();
let timer: ReturnType<typeof setInterval> | null = null;
const subs = new Set<() => void>();
function subscribe(cb: () => void) {
  subs.add(cb);
  if (!timer) {
    now = Date.now();
    timer = setInterval(() => {
      now = Date.now();
      subs.forEach((f) => f());
    }, 15_000);
  }
  return () => {
    subs.delete(cb);
    if (subs.size === 0 && timer) {
      clearInterval(timer);
      timer = null;
    }
  };
}

/** Current time, re-rendering every 15 s (for fresh → "expired?"). */
export function useNow(enabled = true): number {
  return useSyncExternalStore(enabled ? subscribe : noopSubscribe, () => (enabled ? now : 0));
}
const noopSubscribe = () => () => {};

/** Copy a code (inside a user gesture) and confirm with a toast. */
export async function copyCode(code: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(code);
    toast({ kind: "success", message: `Copied ${code}`, key: "otp-copied" });
    return true;
  } catch (e) {
    toast({ kind: "error", message: "Couldn't copy the code", detail: String((e as Error)?.message ?? e) });
    return false;
  }
}

// The open thread's banner registers its code so ⌘⇧C copies what's on
// screen; otherwise the selected row's code is used.
type BannerCode = { ref: ThreadRef; code: string };
let bannerCode: BannerCode | null = null;
export function setBannerCode(next: BannerCode) {
  bannerCode = next;
}
export function clearBannerCode(entry: BannerCode) {
  if (bannerCode === entry) bannerCode = null;
}

function selectedCode(): string | null {
  const ui = getUi();
  const sel = ui.selected;
  if (!sel) return null;
  if (ui.threadOpen && bannerCode && bannerCode.ref.accountId === sel.accountId && bannerCode.ref.threadId === sel.threadId) {
    return bannerCode.code;
  }
  const t = list.get().items.find((i) => i.accountId === sel.accountId && i.threadId === sel.threadId);
  const otp = rowCode(t?.otp, Date.now());
  return otp?.kind === "code" ? otp.code : null;
}

const noOverlay = () => {
  const o = getUi().overlay;
  return o === null || o === "command";
};

export const COPY_KEYS = "mod+shift+c";

/** ⌘⇧C: copy the selected conversation's verification code. */
export function registerOtpShortcuts(): () => void {
  return registerShortcuts([
    {
      id: "otp.copy",
      keys: COPY_KEYS,
      label: "Copy verification code",
      group: "Triage",
      when: () => noOverlay() && getUi().surface === "mail" && selectedCode() !== null,
      run: () => {
        const code = selectedCode();
        if (code) void copyCode(code);
      },
    },
  ]);
}
