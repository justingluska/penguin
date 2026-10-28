// Pair iPhone's dialog (features/settings/pairModel.ts): how backend events
// move it, when closing must stop the Mac's listener, and the wording.
import { test } from "node:test";
import assert from "node:assert/strict";
import { formatLeft, nextPhase, pairingLive, plural, sendLabel, type PairPhase } from "../src/features/settings/pairModel.ts";
import type { PairAccount, PairStart } from "../src/lib/types.ts";

const account = (email: string, status: PairAccount["status"]): PairAccount => ({ email, name: null, color: "#4F7CFF", status, reason: null });
const start: PairStart = {
  sessionId: "s1",
  qr: { size: 29, path: "M0 0h1v1H0z" },
  expiresAt: 0,
  hosts: ["192.168.1.20"],
  port: 50123,
  accounts: [account("ada@floe.example", "token")],
  macClientId: null,
};
const phone = { name: "iPhone", model: "iPhone17,1", system: "iOS 26.0", appVersion: "ios-1.0" };

test("a phone connects, the user sends, the phone answers", () => {
  let p: PairPhase = { kind: "showing", start };
  p = nextPhase(p, { state: "connected", sessionId: "s1", phone, code: "482 193", accounts: start.accounts, clientNote: null });
  assert.equal(p.kind, "connected");
  assert.equal(p.kind === "connected" && p.code, "482 193");
  assert.equal(p.kind === "connected" && p.answered, false);
  p = nextPhase(p, { state: "sending", sessionId: "s1" });
  assert.equal(p.kind, "sending");
  assert.ok(pairingLive(p), "closing while sending still stops the listener");
  p = nextPhase(p, { state: "done", sessionId: "s1", phoneName: "iPhone", imported: ["ada@floe.example"], failed: [], signIn: [] });
  assert.deepEqual(p, { kind: "done", phoneName: "iPhone", imported: ["ada@floe.example"], failed: [], signIn: [] });
  assert.ok(!pairingLive(p));
});

test("out-of-order events don't skip the Send prompt", () => {
  const showing: PairPhase = { kind: "showing", start };
  assert.equal(nextPhase(showing, { state: "sending", sessionId: "s1" }), showing);
  const done: PairPhase = { kind: "done", phoneName: "iPhone", imported: [], failed: [], signIn: [] };
  assert.equal(nextPhase(done, { state: "connected", sessionId: "s1", phone, code: "1", accounts: [], clientNote: null }), done);
});

test("expiry and refusals end the pairing", () => {
  const p = nextPhase({ kind: "showing", start }, { state: "failed", sessionId: "s1", message: "The pairing code expired.", expired: true });
  assert.deepEqual(p, { kind: "error", message: "The pairing code expired." });
  assert.ok(!pairingLive(p));
  assert.deepEqual(nextPhase({ kind: "showing", start }, { state: "declined", sessionId: "s1" }), { kind: "declined" });
});

test("wording", () => {
  assert.equal(sendLabel([account("a@floe.example", "token"), account("b@floe.example", "token"), account("c@floe.example", "signIn")]), "Send 2 accounts");
  assert.equal(sendLabel([account("a@floe.example", "token")]), "Send 1 account");
  assert.equal(sendLabel([account("a@floe.example", "alreadyOnPhone"), account("b@floe.example", "signIn")]), "Send the list");
  assert.equal(plural(3, "IMAP or Outlook account"), "3 IMAP or Outlook accounts");
  assert.equal(formatLeft(120_000), "2:00");
  assert.equal(formatLeft(61_500), "1:02");
  assert.equal(formatLeft(-5), "0:00");
});
