// Unsubscribe button copy and actions per offer (features/unsubscribe/model.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { senderCondition, senderLabel, unsubscribeCopy } from "../src/features/unsubscribe/model.ts";
import type { UnsubscribeOffer } from "../src/lib/types.ts";

const base: UnsubscribeOffer = {
  method: "oneClick",
  source: "header",
  domain: "news.list.example",
  mailto: null,
  verified: true,
  needsCheck: false,
  unsubscribed: null,
  linkUrl: null,
};

test("one-click posts from the app and names the domain", () => {
  const c = unsubscribeCopy(base, "Harbor Weekly");
  assert.equal(c.action, "oneClick");
  assert.equal(c.question, "Unsubscribe from Harbor Weekly?");
  assert.match(c.explain[0], /request directly to news\.list\.example\. No page opens/);
  assert.equal(c.url, null);
  assert.match(c.tooltip, /news\.list\.example/);
  assert.equal(c.editFirst, false);
  assert.equal(c.label, "Unsubscribe");
});

test("mailto sends directly only for a verified sender", () => {
  const mailto = { to: "leave@list.example", subject: "unsubscribe", body: "" };
  const ok = unsubscribeCopy({ ...base, method: "mailto", mailto, domain: "list.example" }, "Digest", "sam@mail.example");
  assert.equal(ok.action, "mailto");
  assert.equal(ok.verb, "Send");
  assert.equal(ok.editFirst, true);
  assert.equal(ok.explain[0], "Penguin will send an email from sam@mail.example to leave@list.example with the subject “unsubscribe”. The sender's list removes you when it arrives.");
  assert.deepEqual(ok.mail, { from: "sam@mail.example", to: "leave@list.example", subject: "unsubscribe" });
  const unverified = unsubscribeCopy({ ...base, method: "mailto", mailto, domain: "list.example", verified: false }, "Digest");
  assert.equal(unverified.action, "compose");
  assert.equal(unverified.editFirst, false);
  assert.match(unverified.explain.join(" "), /won't send .* from this account to leave@list\.example/);
});

test("links open the page, and say when the link came from the body", () => {
  const url = "https://news.list.example/u?t=abc";
  const header = unsubscribeCopy({ ...base, method: "link", linkUrl: url }, "Lumen");
  assert.equal(header.action, "link");
  assert.equal(header.verb, "Open page");
  assert.equal(header.url, url);
  assert.match(header.explain[0], /opens news\.list\.example in your browser/);
  assert.match(header.explain[1], /unsubscribe header/);
  const body = unsubscribeCopy({ ...base, method: "link", source: "body", linkUrl: url }, "Lumen");
  assert.match(body.explain[1], /in the message's text/);
});

test("remembered unsubscribes show as done, except an opened page", () => {
  assert.equal(unsubscribeCopy({ ...base, unsubscribed: { at: 1, method: "oneClick" } }, "X").label, "Unsubscribed");
  assert.equal(unsubscribeCopy({ ...base, unsubscribed: { at: 1, method: "mailto" } }, "X").done, true);
  const opened = unsubscribeCopy({ ...base, method: "link", unsubscribed: { at: 1, method: "link" } }, "X");
  assert.equal(opened.done, false);
  assert.equal(opened.label, "Unsubscribe");
});

test("sender label and auto-archive condition", () => {
  assert.equal(senderLabel({ name: "  Weekly ", email: "w@list.example" }), "Weekly");
  assert.equal(senderLabel({ name: null, email: "w@list.example" }), "w@list.example");
  assert.equal(senderCondition("News@List.example"), "from:news@list.example in:inbox");
  assert.equal(senderCondition('"odd name"@list.example'), 'from:"odd name@list.example" in:inbox');
});
