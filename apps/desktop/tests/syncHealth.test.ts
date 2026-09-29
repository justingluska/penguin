// When a sync problem is shown, hidden, and what "Copy details" copies
// (src/lib/syncHealth.ts). The threshold itself is set by the engines
// (penguin-core sync_health.rs); these check how the UI reads it.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  HIDE_MS,
  activeHide,
  hideable,
  liveHides,
  parseHides,
  recentRecovery,
  redactError,
  retryingText,
  shouldAlert,
  syncDiagnostic,
  syncHealth,
  withHide,
  type SyncHide,
} from "../src/lib/syncHealth.ts";
import type { Account, SyncErrorKind, SyncStatus } from "../src/lib/types.ts";

const T0 = 1_790_000_000_000;
const ID = "acc-okafor";

function status(patch: Partial<SyncStatus> = {}): SyncStatus {
  return {
    accountId: ID,
    phase: "idle",
    indexed: 3_214,
    totalEstimate: 3_214,
    lastSyncedAt: T0 - 60_000,
    error: null,
    ratePerMin: null,
    etaSecs: null,
    failure: null,
    recovered: null,
    ...patch,
  };
}

function failing(count: number, alert: boolean, kind: SyncErrorKind = "network", patch: Partial<SyncStatus> = {}): SyncStatus {
  return status({
    phase: kind === "auth" || kind === "keychain" ? "needsReauth" : "error",
    error: "network: couldn't reach imap.fastmail.com:993: operation timed out",
    failure: { kind, count, firstAt: T0 - 30_000, lastAt: T0, nextRetryAt: kind === "auth" ? null : T0 + 20_000, alert },
    ...patch,
  });
}

const account: Pick<Account, "email" | "provider" | "providerConfig"> = {
  email: "sam@okafor.example",
  provider: "imap",
  providerConfig: {
    auth: "appPassword",
    host: "fastmail",
    imap: { host: "imap.fastmail.com", port: 993, security: "tls", username: "sam@okafor.example" },
  },
};

test("a failure the engine is still retrying is quiet; once it alerts it shows", () => {
  assert.equal(syncHealth(status()), "ok");
  assert.equal(syncHealth(failing(1, false)), "retrying");
  assert.equal(syncHealth(failing(2, false)), "retrying");
  assert.equal(syncHealth(failing(3, true)), "alert");
  assert.equal(shouldAlert([], failing(1, false), T0), false);
  assert.equal(shouldAlert([], failing(3, true), T0), true);
});

test("sign-in problems and statuses without a failure record alert at once", () => {
  assert.equal(syncHealth(failing(1, true, "auth")), "alert");
  // An older backend, or a status built by hand: every error alerted before.
  assert.equal(syncHealth(status({ phase: "error", error: "http 500: backendError" })), "alert");
  assert.equal(syncHealth(status({ phase: "needsReauth", error: "token revoked" })), "alert");
  // needsReauth always alerts, whatever the record says.
  assert.equal(syncHealth(failing(1, false, "auth")), "alert");
});

test("the quiet line says when the next try is", () => {
  const s = failing(1, false);
  assert.equal(retryingText(s, T0), "Retrying in 20 s");
  assert.equal(retryingText(s, T0 + 19_500), "Retrying…");
  assert.equal(retryingText({ ...s, failure: { ...s.failure!, nextRetryAt: T0 + 150_000 } }, T0), "Retrying in 3 min");
});

test("hide for 6 hours silences that account's alert of that kind, until it runs out", () => {
  const s = failing(3, true);
  const hides = withHide([], s, T0);
  assert.deepEqual(hides, [{ accountId: ID, kind: "network", until: T0 + HIDE_MS }]);
  assert.ok(activeHide(hides, s, T0 + 1));
  assert.equal(shouldAlert(hides, s, T0 + HIDE_MS - 1), false);
  assert.equal(shouldAlert(hides, s, T0 + HIDE_MS), true, "shows again when the time is up");
  assert.deepEqual(liveHides(hides, T0 + HIDE_MS), []);
  // Other accounts are unaffected.
  assert.equal(shouldAlert(hides, { ...s, accountId: "acc-northwind" }, T0 + 1), true);
});

test("a different kind of problem breaks through a hide; sign-in problems always show", () => {
  const hides = withHide([], failing(3, true, "network"), T0);
  assert.equal(shouldAlert(hides, failing(4, true, "server"), T0 + 1), true);
  assert.equal(shouldAlert(hides, failing(1, true, "auth"), T0 + 1), true);
  assert.equal(shouldAlert(hides, failing(1, true, "keychain"), T0 + 1), true);
  assert.equal(hideable("auth"), false);
  assert.equal(hideable("keychain"), false);
  assert.equal(hideable("network"), true);
  // Even a hand-made hide of a sign-in kind never applies.
  const forged: SyncHide[] = [{ accountId: ID, kind: "auth", until: T0 + HIDE_MS }];
  assert.equal(shouldAlert(forged, failing(1, true, "auth"), T0 + 1), true);
});

test("hiding again replaces the old hide, and stored hides are validated", () => {
  const once = withHide([], failing(3, true), T0);
  const twice = withHide(once, failing(5, true), T0 + 1_000);
  assert.equal(twice.length, 1);
  assert.equal(twice[0].until, T0 + 1_000 + HIDE_MS);
  const stored = JSON.stringify([...twice, { accountId: 3 }, { accountId: "x", kind: "network", until: T0 - 1 }]);
  assert.deepEqual(parseHides(stored, T0 + 2_000), twice);
  assert.deepEqual(parseHides("not json", T0), []);
  assert.deepEqual(parseHides(null, T0), []);
});

test("a recovery is shown for a while after sync is back", () => {
  const back = status({ recovered: { at: T0, since: T0 - 40_000, failures: 3, kind: "network", alerted: true } });
  assert.equal(recentRecovery(back, T0 + 60_000)?.failures, 3);
  assert.equal(recentRecovery(back, T0 + 11 * 60_000), null);
  assert.equal(recentRecovery(failing(1, false), T0), null);
});

test("copy details names the server, the failure and the versions, and nothing private", () => {
  const s = failing(3, true, "network", {
    error:
      "network: couldn't reach imap.fastmail.com:993 for sam@okafor.example: Bearer ya29.a0AfH6SMBx access_token=abc123secret password=hunter2 from bea@lark.example",
  });
  const text = syncDiagnostic({ account, status: s, service: "Fastmail", appVersion: "0.1.0", osVersion: "macOS 26.0.1", now: T0 });
  assert.match(text, /account: an address at okafor\.example \(imap, Fastmail\)/);
  assert.match(text, /server: imap\.fastmail\.com:993 \(tls\)/);
  assert.match(text, /error kind: network/);
  assert.match(text, /first failure: 2026-/);
  assert.match(text, /failed attempts in a row: 3/);
  assert.match(text, /last successful sync: 2026-/);
  assert.match(text, /app: Penguin 0\.1\.0/);
  assert.match(text, /os: macOS 26\.0\.1/);
  // No address but the account's domain, no tokens or passwords.
  assert.doesNotMatch(text, /sam@/);
  assert.doesNotMatch(text, /bea@|lark\.example/);
  assert.doesNotMatch(text, /ya29|abc123secret|hunter2/);
  assert.match(text, /…@okafor\.example/);
  assert.match(text, /<address>/);
  // The IMAP username is the address itself: it must not leak through the server line.
  assert.equal(text.match(/okafor\.example/g)?.length, 2);
});

test("copy details never includes someone else's address or the account's own", () => {
  const s = failing(1, false, "server", { error: 'IMAP FETCH failed: [ALERT] message "Quarterly numbers" from priya@acme.example rejected' });
  const text = syncDiagnostic({ account, status: s, service: "Fastmail", appVersion: null, osVersion: null, now: T0 });
  assert.doesNotMatch(text, /priya@|sam@okafor/);
  assert.match(text, /app: Penguin unknown/);
  // Error text is capped and one line.
  const long = redactError("x ".repeat(400), account.email);
  assert.ok(long.length <= 300 && !long.includes("\n"));
});

test("redaction keeps what helps: hosts, ports and status codes", () => {
  assert.equal(
    redactError("http 503: backendError (gmail.googleapis.com) code 503", "sam@okafor.example"),
    "http 503: backendError (gmail.googleapis.com) code 503",
  );
  assert.equal(redactError("couldn't reach imap.mail.yahoo.com:993: timed out", "sam@okafor.example"), "couldn't reach imap.mail.yahoo.com:993: timed out");
});
