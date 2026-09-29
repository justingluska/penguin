// How loudly to talk about an account's sync problem, and what "Copy
// details" puts on the clipboard. Pure: tests/syncHealth.test.ts.
//
// The engines record every failed attempt on the status (`failure`, see
// penguin-core sync_health.rs) and mark it `alert` once it needs the user or
// has lasted (3 attempts in a row or a minute). Until then the account is
// "retrying": a quiet line, no toast, no red dot. When sync makes progress
// again the streak moves to `recovered`.
//
// "Hide for 6 hours" snoozes one account's alert for one error kind, per
// device. A different kind breaks through (a network error that turns into a
// signed-out account), and sign-in problems can't be hidden at all.
import type { Account, SyncErrorKind, SyncStatus } from "./types";

export type SyncHealth =
  /** Syncing, or nothing wrong. */
  | "ok"
  /** A failure the engine is retrying on its own; not worth an alert yet. */
  | "retrying"
  /** Needs the user, or has lasted: the toast, the red dot, the fix button. */
  | "alert";

export const HIDE_MS = 6 * 60 * 60 * 1000;
/** How long "Back in sync" stays on the account's line after a recovery. */
export const RECOVERED_SHOWN_MS = 10 * 60 * 1000;

const broken = (s: SyncStatus) => s.phase === "error" || s.phase === "needsReauth";

/** The failure's kind; statuses from before failures were recorded count as "other" (or "auth" when signed out). */
export function errorKind(s: SyncStatus): SyncErrorKind {
  return s.failure?.kind ?? (s.phase === "needsReauth" ? "auth" : "other");
}

export function syncHealth(s: SyncStatus | null | undefined): SyncHealth {
  if (!s || !broken(s)) return "ok";
  // A status without `failure` (an older backend, a test double) has no
  // streak to wait on: it alerts, as every error did before.
  if (s.phase === "needsReauth" || !s.failure || s.failure.alert) return "alert";
  return "retrying";
}

/** Sign-in problems always show: they never fix themselves. */
export const hideable = (kind: SyncErrorKind) => kind !== "auth" && kind !== "keychain";

export interface SyncHide {
  accountId: string;
  /** The error kind that was hidden; another kind shows again. */
  kind: SyncErrorKind;
  /** When it shows again (ms). */
  until: number;
}

/** The hide that currently silences this status's alert, if any. */
export function activeHide(hides: readonly SyncHide[], s: SyncStatus | null | undefined, now = Date.now()): SyncHide | null {
  if (!s || syncHealth(s) !== "alert") return null;
  const kind = errorKind(s);
  if (!hideable(kind)) return null;
  return hides.find((h) => h.accountId === s.accountId && h.kind === kind && h.until > now) ?? null;
}

/** Alerting and not hidden: what the toast and the red status show. */
export const shouldAlert = (hides: readonly SyncHide[], s: SyncStatus | null | undefined, now = Date.now()) =>
  syncHealth(s) === "alert" && !activeHide(hides, s, now);

/** Add (or replace) the hide for this status's account and kind; expired ones are dropped. */
export function withHide(hides: readonly SyncHide[], s: SyncStatus, now = Date.now()): SyncHide[] {
  const kind = errorKind(s);
  return [
    ...hides.filter((h) => h.until > now && !(h.accountId === s.accountId && h.kind === kind)),
    { accountId: s.accountId, kind, until: now + HIDE_MS },
  ];
}

/** Hides that still matter (not expired). */
export const liveHides = (hides: readonly SyncHide[], now = Date.now()) => hides.filter((h) => h.until > now);

/** Parse stored hides, dropping anything malformed or expired. */
export function parseHides(raw: string | null, now = Date.now()): SyncHide[] {
  if (!raw) return [];
  try {
    const v: unknown = JSON.parse(raw);
    if (!Array.isArray(v)) return [];
    return v.filter(
      (h): h is SyncHide =>
        !!h &&
        typeof h === "object" &&
        typeof (h as SyncHide).accountId === "string" &&
        typeof (h as SyncHide).kind === "string" &&
        typeof (h as SyncHide).until === "number" &&
        (h as SyncHide).until > now,
    );
  } catch {
    // Unreadable storage is the same as nothing hidden.
    return [];
  }
}

/** "Retrying…", "Retrying in 20 s": the quiet line while the engine retries on its own. */
export function retryingText(s: SyncStatus, now = Date.now()): string {
  const at = s.failure?.nextRetryAt;
  if (at === null || at === undefined || at <= now + 1000) return "Retrying…";
  const secs = Math.ceil((at - now) / 1000);
  return secs < 60 ? `Retrying in ${secs} s` : `Retrying in ${Math.round(secs / 60)} min`;
}

/** The recovery worth a word on the account's line: recent, after at least one failed attempt. */
export function recentRecovery(s: SyncStatus | null | undefined, now = Date.now()) {
  const r = s?.recovered;
  if (!r || syncHealth(s) !== "ok" || now - r.at > RECOVERED_SHOWN_MS) return null;
  return r;
}

// ---------------------------------------------------------------------------
// Copy details
// ---------------------------------------------------------------------------

const EMAIL = /[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;

/**
 * Error text safe to paste anywhere: no addresses (the account's own shows
 * as "…@its-domain"; any other as "<address>"), no bearer tokens, key=value
 * secrets or long opaque strings, one line, at most 300 characters.
 */
export function redactError(text: string, ownEmail: string): string {
  const own = ownEmail.toLowerCase();
  const ownDomain = own.split("@")[1] ?? "";
  return text
    .replace(/\s+/g, " ")
    .replace(/\b(bearer|basic)\s+[^\s"',;]+/gi, "$1 <redacted>")
    .replace(/\b(access_token|refresh_token|id_token|token|password|passwd|secret|api_?key)(["']?\s*[:=]\s*["']?)[^\s"',;&}]+/gi, "$1$2<redacted>")
    .replace(EMAIL, (m) => (m.toLowerCase() === own ? `…@${ownDomain}` : "<address>"))
    .replace(/\b[A-Za-z0-9_\-./+=]{32,}\b/g, "<redacted>")
    .trim()
    .slice(0, 300);
}

const KIND_TEXT: Record<SyncErrorKind, string> = {
  network: "network (couldn't connect, timed out or the connection dropped)",
  server: "server error",
  rateLimited: "rate limited by the server",
  auth: "sign-in refused",
  keychain: "Keychain refused",
  storage: "local database",
  internal: "sync crashed",
  config: "account or app settings",
  other: "other",
};

function server(a: Account): string {
  if (a.provider === "gmail") return "gmail.googleapis.com (Gmail API)";
  if (a.provider === "microsoft") return "graph.microsoft.com (Microsoft Graph)";
  const imap = a.providerConfig?.imap;
  return imap ? `${imap.host}:${imap.port} (${imap.security})` : "unknown";
}

const iso = (ms: number | null | undefined) => (ms ? new Date(ms).toISOString() : "never");

export interface DiagnosticInput {
  account: Pick<Account, "email" | "provider" | "providerConfig">;
  status: SyncStatus;
  /** The service as the UI names it ("Yahoo", "Gmail"). */
  service: string;
  appVersion: string | null;
  osVersion: string | null;
  hiddenUntil?: number | null;
  now?: number;
}

/**
 * The text "Copy details" puts on the clipboard: enough to tell what broke
 * and since when, nothing private. The account appears only as its domain;
 * never the address, a token, a password or any mail.
 */
export function syncDiagnostic({ account, status: s, service, appVersion, osVersion, hiddenUntil, now = Date.now() }: DiagnosticInput): string {
  const f = s.failure ?? null;
  const domain = account.email.toLowerCase().split("@")[1] ?? "unknown";
  const health = syncHealth(s);
  const lines = [
    "Penguin sync problem",
    `account: an address at ${domain} (${account.provider}, ${service})`,
    `server: ${server(account as Account)}`,
    `state: ${s.phase}${health === "retrying" ? ", retrying quietly" : health === "alert" ? ", shown" : ""}${hiddenUntil ? `, hidden until ${iso(hiddenUntil)}` : ""}`,
    `error kind: ${KIND_TEXT[errorKind(s)]}`,
    `error: ${s.error ? redactError(s.error, account.email) : "none"}`,
    `first failure: ${f ? iso(f.firstAt) : "unknown"}`,
    `failed attempts in a row: ${f ? f.count : "unknown"}`,
    `latest failure: ${f ? iso(f.lastAt) : "unknown"}`,
    `next automatic retry: ${f ? (f.nextRetryAt ? iso(f.nextRetryAt) : "none (waiting for you)") : "unknown"}`,
    `last successful sync: ${iso(s.lastSyncedAt)}`,
    `app: Penguin ${appVersion ?? "unknown"}`,
    `os: ${osVersion ?? "unknown"}`,
    `copied: ${iso(now)}`,
  ];
  return lines.join("\n");
}
