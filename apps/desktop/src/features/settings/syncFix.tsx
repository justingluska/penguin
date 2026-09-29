// One place that decides how a broken sync status is explained and fixed, so
// the status bar, sidebar popover, Settings, onboarding and the sync toast all
// offer the same actions:
// - "Retry now" (errors) or "Reconnect" (the account has to sign in again,
//   including a denied Keychain prompt);
// - "Copy details": a short diagnostic with nothing private in it;
// - "Hide for 6 hours": per device, per account and error kind.
// Failures the engine is still retrying on its own (lib/syncHealth.ts) are
// not "broken" yet: they get a quiet "Retrying…" line and no fix.
// OWNER: settings agent.
import { useEffect, useState, useSyncExternalStore } from "react";
import { api, asCommandError } from "../../lib/api";
import type { SyncErrorKind, SyncStatus } from "../../lib/types";
import { toast } from "../../components/Toast";
import { openReconnect } from "./ReconnectModal";
import { accountById, meta } from "../../app/store";
import { accountName } from "../../components/Identity";
import { serviceName } from "../../lib/capabilities";
import { copyTextLater } from "../../lib/clipboard";
import { activeHide, errorKind, hideable, syncDiagnostic, syncHealth } from "../../lib/syncHealth";
import { getSyncHides, hideSyncAlert, unhideSync, useSyncHides } from "../../lib/syncHides";

export interface SyncFix {
  kind: "retry" | "reconnect";
  label: "Retry now" | "Reconnect";
  /** Short, readable explanation for the row / status line. */
  message: string;
  /** The raw error text, for a tooltip. */
  detail: string | null;
}

/** Keychain read refused (the macOS prompt was denied or cancelled). */
function keychainDenied(s: SyncStatus): boolean {
  if (s.failure) return s.failure.kind === "keychain";
  return !!s.error && /keychain access failed|user canceled the operation|errSecUserCanceled|-128\b|keychain.*(denied|not allowed)/i.test(s.error);
}

/** Readable text for an error kind; statuses without one fall back to reading the text. */
export function readableSyncError(error: string | null, service = "Gmail", kind: SyncErrorKind | null = null): string {
  switch (kind) {
    case "network":
      return `Can't reach ${service} right now`;
    case "server":
      return `${service} had a server error`;
    case "rateLimited":
      return `${service} is rate-limiting requests`;
    case "storage":
      return "Couldn't save mail to the local database";
    case "internal":
      return "Sync stopped unexpectedly";
    default:
      break;
  }
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

/**
 * The fix for a status that needs one (an alert), or null: healthy, or a
 * failure the engine is still retrying on its own. Hides don't change the
 * fix; callers that announce (toast, status bar) check `isHidden`.
 */
export function syncFixFor(s: SyncStatus | undefined | null): SyncFix | null {
  if (!s || syncHealth(s) !== "alert") return null;
  if (s.phase === "needsReauth" || (s.phase === "error" && keychainDenied(s))) {
    return {
      kind: "reconnect",
      label: "Reconnect",
      message: keychainDenied(s) ? "Penguin couldn't read its saved sign-in" : "Needs you to sign in again",
      detail: s.error,
    };
  }
  const service = serviceName(accountById(s.accountId));
  return { kind: "retry", label: "Retry now", message: readableSyncError(s.error, service, s.failure?.kind ?? null), detail: s.error };
}

/** This status's alert is hidden on this device right now. */
export const isHidden = (s: SyncStatus | null | undefined) => !!activeHide(getSyncHides(), s);

// ---------------------------------------------------------------------------
// Retry now: one retry per account at a time, followed to its outcome.
// ---------------------------------------------------------------------------

export type RetryOutcome = "synced" | "failed" | "waiting";

/** How long a retry is followed before it's reported as still going. */
const RETRY_FOLLOW_MS = 45_000;
const inflight = new Map<string, Promise<RetryOutcome>>();
const lastOutcome = new Map<string, { outcome: RetryOutcome; at: number }>();
const retrySubs = new Set<() => void>();
const retryChanged = () => retrySubs.forEach((f) => f());

export function subscribeRetries(cb: () => void): () => void {
  retrySubs.add(cb);
  return () => retrySubs.delete(cb);
}
export const isRetrying = (accountId: string) => inflight.has(accountId);
export const retryOutcome = (accountId: string) => lastOutcome.get(accountId) ?? null;

export function useRetrying(accountId: string | null | undefined): boolean {
  return useSyncExternalStore(subscribeRetries, () => (accountId ? inflight.has(accountId) : false));
}

/**
 * The outcome of a retry started at `startedAt`, read from the account's
 * status: synced once its failure is gone, failed once another attempt failed.
 */
export function retryResult(s: SyncStatus | undefined, startedAt: number): RetryOutcome | null {
  if (!s) return null;
  if (s.failure) return s.failure.lastAt >= startedAt ? "failed" : null;
  if (s.phase === "error" || s.phase === "needsReauth") return null;
  return "synced";
}

/**
 * Wake the account's sync now and follow it: "synced", "failed" (another
 * attempt failed), or "waiting" (no answer yet; it keeps going). A second
 * call while one runs gets the same promise, so the button can't be spammed.
 */
export function retryNow(accountId: string): Promise<RetryOutcome> {
  const running = inflight.get(accountId);
  if (running) return running;
  const startedAt = Date.now();
  const p = (async (): Promise<RetryOutcome> => {
    try {
      await api.retrySyncAccount(accountId);
    } catch (e) {
      toast({ tone: "error", message: `Couldn't retry sync: ${asCommandError(e).message}` });
      return "failed";
    }
    return await new Promise<RetryOutcome>((resolve) => {
      const check = () => {
        const r = retryResult(meta.get().sync[accountId], startedAt);
        if (!r) return false;
        done(r);
        return true;
      };
      const unsub = meta.subscribe(() => void check());
      const timer = setTimeout(() => done("waiting"), RETRY_FOLLOW_MS);
      function done(r: RetryOutcome) {
        unsub();
        clearTimeout(timer);
        resolve(r);
      }
      check();
    });
  })().then((outcome) => {
    inflight.delete(accountId);
    lastOutcome.set(accountId, { outcome, at: Date.now() });
    retryChanged();
    return outcome;
  });
  inflight.set(accountId, p);
  retryChanged();
  return p;
}

const nameOf = (id: string) => {
  const { accounts } = meta.get();
  const a = accounts.find((x) => x.id === id);
  return a ? accountName(a, accounts) : id;
};

/** Say how a retry went, for places with no button to show it (menus). */
export function announceRetry(accountId: string, outcome: RetryOutcome) {
  const name = nameOf(accountId);
  if (outcome === "synced") toast({ kind: "success", key: `sync:retried:${accountId}`, message: `${name} is syncing again` });
  else if (outcome === "waiting") toast({ key: `sync:retried:${accountId}`, message: `${name}: still trying`, detail: "Sync keeps retrying on its own" });
  else {
    const s = meta.get().sync[accountId];
    const msg = s ? (syncFixFor(s)?.message ?? readableSyncError(s.error, serviceName(accountById(accountId)), s.failure?.kind ?? null)) : "Sync failed again";
    toast({ tone: "error", key: `sync:retried:${accountId}`, message: `${name}: still failing`, detail: msg, title: s?.error ?? undefined });
  }
}

/** Run a fix: Reconnect opens the sign-in modal; Retry wakes sync and follows it. */
export async function runSyncFix(fix: SyncFix, accountId: string, opts: { announce?: boolean } = {}): Promise<void> {
  if (fix.kind === "reconnect") {
    openReconnect(accountId);
    return;
  }
  const outcome = await retryNow(accountId);
  if (opts.announce) announceRetry(accountId, outcome);
}

// ---------------------------------------------------------------------------
// Copy details / Hide
// ---------------------------------------------------------------------------

/** The diagnostic for these accounts ("Copy details"); nothing private (see syncDiagnostic). */
export async function syncDetailsText(accountIds: string[]): Promise<string> {
  let appVersion: string | null = null;
  let osVersion: string | null = null;
  try {
    const d = await api.diagnostics();
    appVersion = d.appVersion;
    osVersion = d.osVersion;
  } catch (e) {
    // The rest of the report still helps; say why these are missing.
    appVersion = `unknown (${asCommandError(e).message})`;
  }
  const { sync } = meta.get();
  const hides = getSyncHides();
  return accountIds
    .map((id) => {
      const account = accountById(id);
      const status = sync[id];
      if (!account || !status) return `Penguin sync problem\naccount: unknown (${id ? "removed" : "none"})`;
      return syncDiagnostic({
        account,
        status,
        service: serviceName(account),
        appVersion,
        osVersion,
        hiddenUntil: activeHide(hides, status)?.until ?? null,
      });
    })
    .join("\n\n");
}

export function copySyncDetails(accountIds: string[]): Promise<void> {
  return copyTextLater(syncDetailsText(accountIds), "Sync details copied");
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/** Hide these accounts' alerts for 6 hours (sign-in problems can't be hidden). */
export function hideSyncAlerts(accountIds: string[]) {
  const { sync } = meta.get();
  let until = 0;
  for (const id of accountIds) {
    const s = sync[id];
    if (s && hideable(errorKind(s))) until = hideSyncAlert(s).until;
  }
  if (until) {
    toast({
      key: "sync:hidden",
      message: `Sync alert hidden until ${clock(until)}`,
      detail: "Settings → Sync can show it again. A new kind of problem still shows.",
      actions: [{ label: "Undo", run: () => accountIds.forEach((id) => unhideSync(id)) }],
    });
  }
}

export const canHide = (s: SyncStatus | null | undefined) => !!s && syncHealth(s) === "alert" && hideable(errorKind(s));

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/** How long a finished retry's result stays on its button. */
const OUTCOME_SHOWN_MS = 4_000;

/** The labeled fix button for a status; renders nothing when there's nothing to fix. */
export function SyncFixButton({
  status,
  className = "btn btn-secondary btn-sm",
  quiet = false,
}: {
  status: SyncStatus | undefined | null;
  className?: string;
  /** Offer "Retry now" while the engine is still retrying quietly, too. */
  quiet?: boolean;
}) {
  const accountId = status?.accountId ?? null;
  const retrying = useRetrying(accountId);
  const [, bump] = useState(0);
  const recent = accountId ? retryOutcome(accountId) : null;
  const showOutcome = !retrying && recent && Date.now() - recent.at < OUTCOME_SHOWN_MS ? recent.outcome : null;
  useEffect(() => {
    if (!showOutcome) return;
    const t = setTimeout(() => bump((n) => n + 1), OUTCOME_SHOWN_MS);
    return () => clearTimeout(t);
  }, [showOutcome, recent?.at]);
  let fix = syncFixFor(status);
  if (!fix && quiet && status && syncHealth(status) === "retrying") fix = { kind: "retry", label: "Retry now", message: "", detail: status.error };
  if (!fix && showOutcome === "synced") {
    return (
      <span className={className + " sync-outcome is-ok"} role="status">
        Synced
      </span>
    );
  }
  if (!fix || !status) return null;
  const run = (e: React.MouseEvent) => {
    e.stopPropagation();
    void runSyncFix(fix, status.accountId);
  };
  const label = retrying ? "Retrying…" : showOutcome === "failed" ? "Didn't work · Retry" : showOutcome === "waiting" ? "Still trying · Retry" : fix.label;
  return (
    <button
      className={className}
      disabled={retrying}
      aria-busy={retrying || undefined}
      onClick={run}
      title={fix.detail ?? undefined}
    >
      {retrying ? <span className="st-spinner st-spinner-sm" aria-hidden="true" /> : null}
      {label}
    </button>
  );
}

/**
 * The full set for one account's alert: the fix, Copy details, and Hide for
 * 6 hours (or Show, once hidden). For the sidebar popover and Settings.
 */
export function SyncActions({
  status,
  quiet = false,
  hide = true,
}: {
  status: SyncStatus | undefined | null;
  quiet?: boolean;
  /** Offer Hide / Show alert (off where there's no room; Settings → Sync has them). */
  hide?: boolean;
}) {
  const hides = useSyncHides();
  if (!status) return null;
  const health = syncHealth(status);
  if (health === "ok" || (health === "retrying" && !quiet)) return null;
  const hidden = activeHide(hides, status);
  return (
    <span className="sync-actions">
      <SyncFixButton status={status} quiet={quiet} />
      <button className="btn btn-ghost btn-sm" onClick={(e) => (e.stopPropagation(), void copySyncDetails([status.accountId]))}>
        Copy details
      </button>
      {!hide ? null : hidden ? (
        <button className="btn btn-ghost btn-sm" title={`Hidden until ${clock(hidden.until)}`} onClick={(e) => (e.stopPropagation(), unhideSync(status.accountId, hidden.kind))}>
          Show alert
        </button>
      ) : canHide(status) ? (
        <button className="btn btn-ghost btn-sm" onClick={(e) => (e.stopPropagation(), hideSyncAlerts([status.accountId]))}>
          Hide 6 h
        </button>
      ) : null}
    </span>
  );
}

export { clock as syncClock };
