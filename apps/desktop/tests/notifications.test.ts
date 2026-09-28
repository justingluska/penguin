// New-mail notifications, the UI half (features/notifications/rules.ts):
// per-account defaults and what counts as "on screen".
import { test } from "node:test";
import assert from "node:assert/strict";
import { accountNotifies, notifyContextOf, withAccount, type ScreenInput } from "../src/features/notifications/rules.ts";
import type { NotificationSettings } from "../src/lib/types.ts";

const N = (accounts: Record<string, boolean> = {}): NotificationSettings => ({ enabled: true, accounts, knownSendersOnly: false });
const A = "sam@northwind.example";
const B = "sam@harbor.example";
const P = "sam@home.example";

test("every account notifies by default except the ones hidden from All Inboxes; a switch wins", () => {
  const hidden = [P];
  assert.equal(accountNotifies(N(), hidden, A), true);
  assert.equal(accountNotifies(N(), hidden, P), false);
  assert.equal(accountNotifies(N({ [P]: true, [A]: false }), hidden, P), true);
  assert.equal(accountNotifies(N({ [P]: true, [A]: false }), hidden, A), false);
});

test("flipping a switch stores only what differs from the default", () => {
  const hidden = [P];
  let n = withAccount(N(), hidden, A, false);
  assert.deepEqual(n.accounts, { [A]: false });
  n = withAccount(n, hidden, A, true);
  assert.deepEqual(n.accounts, {});
  n = withAccount(n, hidden, P, true);
  assert.deepEqual(n.accounts, { [P]: true });
  assert.equal(n.enabled, true, "the rest of the section is kept");
});

const screen = (over: Partial<ScreenInput> = {}): ScreenInput => ({
  surface: "mail",
  view: { kind: "inbox" },
  overlay: null,
  threadOpen: false,
  selected: null,
  accountFilter: null,
  scope: null,
  accountIds: [A, B, P],
  ...over,
});

test("the inbox list on screen covers the accounts it shows", () => {
  assert.deepEqual(notifyContextOf(screen()).inboxAccounts, [A, B, P]);
  // All Inboxes leaving one out, or a profile.
  assert.deepEqual(notifyContextOf(screen({ scope: [A, B] })).inboxAccounts, [A, B]);
  // One account picked.
  assert.deepEqual(notifyContextOf(screen({ accountFilter: B })).inboxAccounts, [B]);
  assert.deepEqual(notifyContextOf(screen({ accountFilter: B, scope: [A, B] })).inboxAccounts, [B]);
  assert.deepEqual(notifyContextOf(screen({ accountFilter: P, scope: [A, B] })).inboxAccounts, []);
});

test("other views, an open thread, overlays and the calendar don't show the inbox", () => {
  const t = { accountId: A, threadId: "t1" };
  assert.deepEqual(notifyContextOf(screen({ view: { kind: "starred" } })), { inboxAccounts: [], thread: null });
  assert.deepEqual(notifyContextOf(screen({ threadOpen: true, selected: t })), { inboxAccounts: [], thread: t });
  // The reading pane's thread counts as shown, next to the list.
  assert.deepEqual(notifyContextOf(screen({ selected: t })), { inboxAccounts: [A, B, P], thread: t });
  assert.deepEqual(notifyContextOf(screen({ overlay: "compose", selected: t })), { inboxAccounts: [], thread: null });
  assert.deepEqual(notifyContextOf(screen({ surface: "calendar" })), { inboxAccounts: [], thread: null });
});
