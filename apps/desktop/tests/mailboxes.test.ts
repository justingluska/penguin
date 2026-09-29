// The sidebar's mailboxes (src/lib/mailboxes.ts): Penguin's names under All
// accounts, the provider's inside one picked account, and when Spam shows.
import { test } from "node:test";
import assert from "node:assert/strict";
import { hasSpamFolder, labelsTitle, mailboxFlavor, mailboxName, serviceTag, sidebarMailboxes } from "../src/lib/mailboxes.ts";
import type { Account } from "../src/lib/types.ts";

type A = Pick<Account, "provider" | "providerConfig">;
const gmail: A = { provider: "gmail", providerConfig: {} };
const gmailImap: A = { provider: "imap", providerConfig: { host: "gmail", imap: { host: "imap.gmail.com", port: 993, security: "tls", username: "sam@gmail.example" } } };
const fastmail: A = { provider: "imap", providerConfig: { host: "fastmail" } };
const icloud: A = { provider: "imap", providerConfig: { host: "icloud" } };
const yahoo: A = { provider: "imap", providerConfig: { host: "yahoo" } };
const ownServer: A = { provider: "imap", providerConfig: { imap: { host: "mail.okafor.example", port: 993, security: "tls", username: "sam" } } };
const outlook: A = { provider: "microsoft", providerConfig: { host: "outlookPersonal" } };

const rows = (a: A | null, spam = true) => sidebarMailboxes(a, spam).map((m) => `${m.view.kind}:${m.label}`);

test("All accounts: Penguin's names, Spam after Trash's neighbours, no All mail row", () => {
  assert.deepEqual(rows(null), [
    "inbox:Inbox",
    "replyLater:Reply Later",
    "followUp:Follow up",
    "starred:Starred",
    "snoozed:Snoozed",
    "sent:Sent",
    "drafts:Drafts",
    "done:Done",
    "spam:Spam",
    "trash:Trash",
  ]);
  assert.ok(!rows(null, false).some((r) => r.startsWith("spam")));
});

test("one Gmail account: Done and All Mail, Spam, Trash; Gmail quick setup counts as Gmail", () => {
  for (const a of [gmail, gmailImap]) {
    assert.equal(mailboxFlavor(a), "gmail");
    assert.deepEqual(rows(a).slice(-4), ["done:Done", "all:All Mail", "spam:Spam", "trash:Trash"]);
  }
});

test("one IMAP account: Archive is Done's view; Spam is called what the service calls it", () => {
  assert.deepEqual(rows(fastmail).slice(-3), ["done:Archive", "spam:Spam", "trash:Trash"]);
  assert.equal(mailboxName("spam", icloud), "Junk");
  assert.equal(mailboxName("spam", yahoo), "Spam");
  assert.equal(mailboxName("all", fastmail), "All mail");
  // No Junk folder on the server: no Spam row.
  assert.deepEqual(rows(ownServer, false).slice(-2), ["done:Archive", "trash:Trash"]);
});

test("one Microsoft account: Outlook's folder names", () => {
  assert.deepEqual(rows(outlook).slice(-5), ["sent:Sent Items", "drafts:Drafts", "done:Archive", "spam:Junk Email", "trash:Deleted Items"]);
  assert.equal(mailboxName("inbox", outlook), "Inbox");
});

test("names outside the fixed mailboxes are left to the caller", () => {
  assert.equal(mailboxName("label", gmail), null);
  assert.equal(mailboxName("smart", null), null);
  assert.equal(mailboxName("all", null), "All mail");
});

test("Spam shows when an account in view has a Spam folder, or before labels load", () => {
  const labels = [
    { accountId: "a", id: "INBOX" },
    { accountId: "a", id: "SPAM" },
    { accountId: "b", id: "INBOX" },
  ];
  assert.equal(hasSpamFolder(labels, null, true), true);
  assert.equal(hasSpamFolder(labels, ["b"], true), false);
  assert.equal(hasSpamFolder(labels, ["a", "b"], true), true);
  assert.equal(hasSpamFolder([], ["b"], false), true);
});

test("the labels section is named for what the account has", () => {
  assert.equal(labelsTitle(gmail, [{ id: "Label_1" }]), "Labels");
  assert.equal(labelsTitle(fastmail, [{ id: "f:Family" }]), "Folders");
  assert.equal(labelsTitle(outlook, [{ id: "f:AAMk" }, { id: "c:Blue" }]), "Folders & categories");
  assert.equal(labelsTitle(outlook, [{ id: "c:Blue" }]), "Categories");
  assert.equal(labelsTitle(null, [{ id: "f:Family" }]), "Folders");
  assert.equal(labelsTitle(null, [{ id: "f:Family" }, { id: "Label_1" }]), "Labels");
});

test("the scoped header names the service, or IMAP for a server by host name", () => {
  assert.equal(serviceTag(gmail), "Gmail");
  assert.equal(serviceTag(fastmail), "Fastmail");
  assert.equal(serviceTag(outlook), "Outlook");
  assert.equal(serviceTag(ownServer), "IMAP");
});
