// Provider names and capability gating (src/lib/capabilities.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  INBOX,
  applyDelta,
  deltaCalls,
  inboxTabsAvailable,
  isFolderChoice,
  isLabelChoice,
  labelEditing,
  mailboxUnaffected,
  moveDelta,
  serviceName,
  shownIn,
  threadAbilities,
  undoDelta,
} from "../src/lib/capabilities.ts";
import type { Account, Capabilities } from "../src/lib/types.ts";

type P = Pick<Account, "provider" | "providerConfig">;
const imap = (host: string | null, server = "imap.harbor.example"): P => ({
  provider: "imap",
  providerConfig: { auth: "appPassword", host, imap: { host: server, port: 993, security: "tls", username: "sam@harbor.example" } },
});

test("the service is named the way people call it", () => {
  assert.equal(serviceName({ provider: "gmail", providerConfig: {} }), "Gmail");
  assert.equal(serviceName(imap("yahoo")), "Yahoo");
  assert.equal(serviceName(imap("icloud")), "iCloud");
  assert.equal(serviceName(imap("fastmail")), "Fastmail");
  // Gmail quick setup rides IMAP but is still Gmail.
  assert.equal(serviceName(imap("gmail", "imap.gmail.com")), "Gmail");
  assert.equal(serviceName(imap("imapGeneric", "127.0.0.1")), "Proton Mail Bridge");
  assert.equal(serviceName(imap("imapGeneric")), "imap.harbor.example");
  assert.equal(serviceName({ provider: "microsoft", providerConfig: { host: "outlookPersonal" } }), "Outlook");
  assert.equal(serviceName({ provider: "microsoft", providerConfig: {} }), "Microsoft");
  assert.equal(serviceName(null), "Gmail");
});

// The table in docs/PROVIDERS-IMPL.md §10 (the parts the UI gates on).
const caps = (c: Partial<Capabilities>) => ({ capabilities: { labels: false, folders: false, labelEdit: false, labelColors: false, inboxCategories: false, ...c } as Capabilities });
const GMAIL = caps({ labels: true, labelEdit: true, labelColors: true, inboxCategories: true });
const IMAP = caps({ folders: true });
const MICROSOFT = caps({ labels: true, folders: true });

test("Label as… needs labels on every account, Move to… needs folders on every account", () => {
  assert.deepEqual(threadAbilities([GMAIL]), { label: true, move: false });
  assert.deepEqual(threadAbilities([IMAP]), { label: false, move: true });
  assert.deepEqual(threadAbilities([MICROSOFT]), { label: true, move: true });
  // Mixed selections get only what they share.
  assert.deepEqual(threadAbilities([GMAIL, IMAP]), { label: false, move: false });
  assert.deepEqual(threadAbilities([GMAIL, MICROSOFT]), { label: true, move: false });
  assert.deepEqual(threadAbilities([IMAP, MICROSOFT]), { label: false, move: true });
  // Nothing selected, or an account that isn't loaded: nothing.
  assert.deepEqual(threadAbilities([]), { label: false, move: false });
  assert.deepEqual(threadAbilities([GMAIL, undefined]), { label: false, move: false });
});

test("folders are Move to… choices, never Label as… ones", () => {
  const folder = { kind: "user" as const, id: "f:Projects%2FGarden" };
  const category = { kind: "user" as const, id: "c:Blue" };
  const gmailLabel = { kind: "user" as const, id: "Label_12" };
  const inbox = { kind: "system" as const, id: "INBOX" };
  assert.ok(isLabelChoice(gmailLabel, GMAIL));
  assert.ok(!isLabelChoice(inbox, GMAIL));
  assert.ok(!isLabelChoice(folder, IMAP));
  assert.ok(isFolderChoice(folder, IMAP));
  assert.ok(isLabelChoice(category, MICROSOFT));
  assert.ok(!isLabelChoice(folder, MICROSOFT));
  assert.ok(isFolderChoice(folder, MICROSOFT));
  assert.ok(!isFolderChoice(category, MICROSOFT));
  assert.ok(!isFolderChoice(gmailLabel, GMAIL));
});

test("label editing and colors follow labelEdit and labelColors", () => {
  assert.deepEqual(labelEditing(GMAIL), { edit: true, colors: true });
  assert.deepEqual(labelEditing(IMAP), { edit: false, colors: false });
  assert.deepEqual(labelEditing(MICROSOFT), { edit: false, colors: false });
  assert.deepEqual(labelEditing(undefined), { edit: false, colors: false });
});

test("inbox tabs apply when some account in view has inbox categories", () => {
  assert.ok(inboxTabsAvailable([GMAIL, IMAP]));
  assert.ok(!inboxTabsAvailable([IMAP, MICROSOFT]));
  assert.ok(!inboxTabsAvailable([]));
});

test("Move to…: add the folder, leave the inbox and every other folder", () => {
  assert.deepEqual(moveDelta(["INBOX", "UNREAD"], "f:Receipts"), { add: ["f:Receipts"], remove: ["INBOX"] });
  assert.deepEqual(moveDelta(["f:Family", "STARRED"], "f:Receipts"), { add: ["f:Receipts"], remove: ["INBOX", "f:Family"] });
  // Already there: the target isn't removed.
  assert.deepEqual(moveDelta(["f:Receipts", "f:Family"], "f:Receipts"), { add: ["f:Receipts"], remove: ["INBOX", "f:Family"] });
  // Categories (Microsoft) aren't folders and stay.
  assert.deepEqual(moveDelta(["INBOX", "c:Blue"], "f:Receipts"), { add: ["f:Receipts"], remove: ["INBOX"] });
  // Back to the inbox: out of its folders.
  assert.deepEqual(moveDelta(["f:Family", "UNREAD"], INBOX), { add: ["INBOX"], remove: ["f:Family"] });
  assert.deepEqual(moveDelta(["UNREAD"], INBOX), { add: ["INBOX"], remove: [] });
});

test("a move applies, and its undo puts back exactly what changed", () => {
  const before = ["f:Family", "UNREAD"];
  const d = moveDelta(before, "f:Receipts");
  const after = applyDelta(before, d);
  assert.deepEqual(after.sort(), ["UNREAD", "f:Receipts"]);
  const back = undoDelta(before, d);
  assert.deepEqual(back, { add: ["f:Family"], remove: ["f:Receipts"] });
  assert.deepEqual(applyDelta(after, back).sort(), [...before].sort());
  // Moving where it already was undoes to nothing.
  assert.deepEqual(undoDelta(["INBOX"], moveDelta(["INBOX"], INBOX)), { add: [], remove: [] });
});

test("deltas become one call per label, adds and removes apart", () => {
  const a = { accountId: "a", threadId: "1" };
  const b = { accountId: "a", threadId: "2" };
  const calls = deltaCalls([
    { ref: a, delta: moveDelta(["INBOX"], "f:Receipts") },
    { ref: b, delta: moveDelta(["f:Family"], "f:Receipts") },
  ]);
  assert.deepEqual([...calls.add], [["f:Receipts", [a, b]]]);
  assert.deepEqual([...calls.remove], [["INBOX", [a, b]], ["f:Family", [b]]]);
});

test("a moved conversation leaves the views it no longer belongs to", () => {
  assert.ok(!shownIn({ kind: "inbox" }, ["f:Receipts"]));
  assert.ok(shownIn({ kind: "inbox" }, ["INBOX"]));
  assert.ok(!shownIn({ kind: "label", labelId: "f:Family" }, ["f:Receipts"]));
  assert.ok(shownIn({ kind: "label", labelId: "f:Receipts" }, ["f:Receipts"]));
  assert.ok(!shownIn({ kind: "done" }, ["INBOX"]));
  assert.ok(shownIn({ kind: "done" }, ["f:Receipts"]));
  assert.ok(shownIn({ kind: "starred" }, ["STARRED"]));
});

test("Remove account says whose mailbox stays untouched", () => {
  assert.equal(mailboxUnaffected({ provider: "gmail", providerConfig: {} }), "Your Gmail isn't affected.");
  assert.equal(mailboxUnaffected(imap("icloud")), "Your mail on iCloud isn't affected.");
});
