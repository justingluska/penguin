// The engines' failure bookkeeping (penguin-core sync_health.rs), for the mock
// backend: the same threshold, so `npm run dev:mock` shows what the app does.
// Scenarios (?mockSync=…) are in mock/mail.ts.
import type { SyncErrorKind, SyncStatus } from "../types";

const ALERT_FAILURES = 3;
const ALERT_AFTER_MS = 60_000;

/** A failed attempt; `retryInMs` null = the engine stops until someone acts. */
export function mockRecordFailure(s: SyncStatus, kind: SyncErrorKind, message: string, now: number, retryInMs: number | null) {
  const prev = s.failure ?? null;
  const count = prev ? prev.count + 1 : 1;
  const firstAt = prev ? prev.firstAt : now;
  const nextRetryAt = retryInMs === null ? null : now + retryInMs;
  const signIn = kind === "auth" || kind === "keychain";
  const alert = signIn || nextRetryAt === null || count >= ALERT_FAILURES || now - firstAt >= ALERT_AFTER_MS || !!prev?.alert;
  Object.assign(s, {
    failure: { kind, count, firstAt, lastAt: now, nextRetryAt, alert },
    recovered: null,
    phase: signIn ? "needsReauth" : "error",
    error: message,
    ratePerMin: null,
    etaSecs: null,
  } satisfies Partial<SyncStatus>);
}

/** Sync stored or checked mail again. */
export function mockRecordProgress(s: SyncStatus, now: number) {
  const f = s.failure;
  if (f) s.recovered = { at: now, since: f.firstAt, failures: f.count, kind: f.kind, alerted: f.alert };
  s.failure = null;
  Object.assign(s, { phase: "idle", error: null, lastSyncedAt: now } satisfies Partial<SyncStatus>);
}

/** "Retry now" woke the engine. */
export function mockRetryNow(s: SyncStatus, now: number) {
  if (s.failure) s.failure = { ...s.failure, nextRetryAt: now };
}

/** The engines' backoff: 10 s, 20 s, 40 s … 5 min (scaled for demos). */
export const mockBackoff = (count: number, scale = 1) => Math.min(5 * 60_000, 5_000 * 2 ** Math.min(count, 10)) * scale;
