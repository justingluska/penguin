// One place that decides how a broken sync status is explained and fixed, so
// the status bar, sidebar popover, Settings and onboarding all offer the same
// labeled button: "Retry sync" for errors, "Reconnect" when the account has to
// sign in again (including a denied Keychain prompt). OWNER: settings agent.
import { useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { SyncStatus } from "../../lib/types";
import { toast } from "../../components/Toast";
import { openReconnect } from "./ReconnectModal";
import { accountById } from "../../app/store";
import { serviceName } from "../../lib/capabilities";

export interface SyncFix {
  kind: "retry" | "reconnect";
  label: "Retry sync" | "Reconnect";
  /** Short, readable explanation for the row / status line. */
  message: string;
  /** The raw error text, for a tooltip. */
  detail: string | null;
}

/** Keychain read refused (the macOS prompt was denied or cancelled). */
function keychainDenied(error: string | null): boolean {
  return !!error && /keychain access failed|user canceled the operation|errSecUserCanceled|-128\b|keychain.*(denied|not allowed)/i.test(error);
}

/** Readable text for an engine error string (raw text goes in `detail`); `service` names the provider ("Gmail", "iCloud"). */
export function readableSyncError(error: string | null, service = "Gmail"): string {
  const e = (error ?? "").toLowerCase();
  if (!e) return "Sync stopped with an error";
  if (e.includes("internal sync error") || e.includes("panicked")) return "Sync stopped unexpectedly";
  if (e.startsWith("network") || e.includes("timed out") || e.includes("dns") || e.includes("connection")) return `Can't reach ${service} right now`;
  if (e.includes("rate limited") || e.includes("http 429")) return `${service} is rate-limiting requests`;
  if (/http 5\d\d/.test(e)) return `${service} had a server error`;
  if (e.startsWith("store") || e.startsWith("database")) return "Couldn't save mail to the local database";
  if (e.includes("history expired")) return `Resyncing: ${service}'s change history expired`;
  return "Sync stopped with an error";
}

/** The fix for a status, or null when nothing needs fixing. */
export function syncFixFor(s: SyncStatus | undefined | null): SyncFix | null {
  if (!s) return null;
  if (s.phase === "needsReauth" || (s.phase === "error" && keychainDenied(s.error))) {
    return {
      kind: "reconnect",
      label: "Reconnect",
      message: keychainDenied(s.error) ? "Penguin couldn't read its saved sign-in" : "Needs you to sign in again",
      detail: s.error,
    };
  }
  if (s.phase === "error") {
    return { kind: "retry", label: "Retry sync", message: readableSyncError(s.error, serviceName(accountById(s.accountId))), detail: s.error };
  }
  return null;
}

/** Run a fix: Reconnect opens the sign-in modal; Retry calls the backend. */
export async function runSyncFix(fix: SyncFix, accountId: string): Promise<void> {
  if (fix.kind === "reconnect") {
    openReconnect(accountId);
    return;
  }
  try {
    await api.retrySyncAccount(accountId);
    toast({ kind: "success", message: "Sync restarted" });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't restart sync: ${asCommandError(e).message}` });
  }
}

/** The labeled fix button for a status; renders nothing when sync is healthy. */
export function SyncFixButton({
  status,
  className = "btn btn-secondary btn-sm",
}: {
  status: SyncStatus | undefined | null;
  className?: string;
}) {
  const [busy, setBusy] = useState(false);
  const fix = syncFixFor(status);
  if (!fix || !status) return null;
  const run = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (busy) return;
    setBusy(true);
    try {
      await runSyncFix(fix, status.accountId);
    } finally {
      setBusy(false);
    }
  };
  return (
    <button className={className} disabled={busy} onClick={(e) => void run(e)} title={fix.detail ?? undefined}>
      {busy ? <span className="st-spinner st-spinner-sm" aria-hidden="true" /> : null}
      {busy ? "Retrying…" : fix.label}
    </button>
  );
}
