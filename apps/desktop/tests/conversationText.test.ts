// Copy conversation (features/thread/conversationText.ts): the plain text a
// conversation copies as, for pasting into an AI chat or a note.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  addressText,
  bodyParts,
  conversationText,
  htmlToText,
  messageText,
  plainUsable,
  splitQuoted,
  type CopyMessage,
  type CopyThread,
} from "../src/features/thread/conversationText.ts";

const priya = { name: "Priya Raman", email: "priya@linden.example" };
const sam = { name: "Sam Okafor", email: "sam@northwind.example" };
const marco = { name: "Marco Vitale", email: "marco@northwind.example" };

const HTML_DOC = (body: string) =>
  `<!DOCTYPE html><html class="pg-html"><head><meta charset="utf-8"><style>p{margin:0}</style></head><body><div class="pg-root">${body}</div></body></html>`;
const TEXT_DOC = (body: string) =>
  `<!DOCTYPE html><html class="pg-text"><head><meta charset="utf-8"><style>body{}</style></head><body><div class="pg-root">${body}</div></body></html>`;

let seq = 0;
function msg(over: Partial<CopyMessage>): CopyMessage {
  seq++;
  return {
    id: `m${seq}`,
    date: Date.UTC(2026, 8, 20 + seq, 14, 0),
    from: priya,
    to: [sam],
    cc: [],
    subject: "Brand review",
    snippet: "",
    bodyText: "",
    html: "",
    labelIds: [],
    attachments: [],
    ...over,
  };
}

const fmt = { formatDate: (ms: number) => new Date(ms).toISOString().slice(0, 16).replace("T", " ") };

test("the whole conversation: subject once, each message with its headers, oldest first", () => {
  const first = msg({
    id: "a",
    date: Date.UTC(2026, 8, 21, 9, 0),
    from: sam,
    to: [priya],
    cc: [marco],
    bodyText: "Hi Priya,\n\nKicking off the review. First look Thursday?\n\nSam",
    html: TEXT_DOC("Hi Priya,"),
  });
  const second = msg({
    id: "b",
    date: Date.UTC(2026, 8, 22, 16, 20),
    from: priya,
    to: [sam],
    bodyText: "Thursday works. Deck attached.\n\nPriya",
    attachments: [
      { filename: "Review_v7.pdf", inline: false },
      { filename: "logo.png", inline: true },
      { filename: "Lockups.png", inline: false },
    ],
  });
  // Handed over newest first: the copy still reads oldest first.
  const thread: CopyThread = { subject: "Brand review", messages: [second, first] };
  assert.equal(
    conversationText(thread, fmt),
    [
      "Subject: Brand review",
      "",
      "=== Message 1 of 2 ===",
      "From: Sam Okafor <sam@northwind.example>",
      "To: Priya Raman <priya@linden.example>",
      "Cc: Marco Vitale <marco@northwind.example>",
      "Date: 2026-09-21 09:00",
      "",
      "Hi Priya,",
      "",
      "Kicking off the review. First look Thursday?",
      "",
      "Sam",
      "",
      "=== Message 2 of 2 ===",
      "From: Priya Raman <priya@linden.example>",
      "To: Sam Okafor <sam@northwind.example>",
      "Date: 2026-09-22 16:20",
      "",
      "Thursday works. Deck attached.",
      "",
      "Priya",
      "",
      "Attachments: Review_v7.pdf, Lockups.png",
      "",
    ].join("\n"),
  );
});

test("one message: no numbering, and no Cc line without Cc", () => {
  const only = msg({ bodyText: "Plumber confirmed Thursday.", from: { name: null, email: "mike@mailbox.example" } });
  const out = conversationText({ subject: "", messages: [only] }, fmt);
  assert.ok(out.startsWith("Subject: Brand review\n\nFrom: mike@mailbox.example\nTo: Sam Okafor <sam@northwind.example>\nDate: "));
  assert.ok(!out.includes("Cc:"));
  assert.ok(!out.includes("==="));
});

test("addresses: name <email>, bare email, names with commas quoted", () => {
  assert.equal(addressText({ name: "Priya Raman", email: "priya@linden.example" }), "Priya Raman <priya@linden.example>");
  assert.equal(addressText({ name: null, email: "ops@linden.example" }), "ops@linden.example");
  assert.equal(addressText({ name: "ops@linden.example", email: "ops@linden.example" }), "ops@linden.example");
  assert.equal(addressText({ name: "Raman, Priya", email: "priya@linden.example" }), '"Raman, Priya" <priya@linden.example>');
});

test("reply history that repeats an earlier message is cut, so each message appears once", () => {
  const first = msg({ from: sam, to: [priya], bodyText: "Can you send the round two directions before Friday?\n\nSam" });
  const reply = msg({
    bodyText: [
      "Attached. B pushes the wordmark further.",
      "",
      "Priya",
      "",
      "On Mon, Sep 21, 2026 at 9:00 AM Sam Okafor <sam@northwind.example> wrote:",
      "> Can you send the round two directions before",
      "> Friday?",
      ">",
      "> Sam",
    ].join("\n"),
  });
  const out = conversationText({ subject: "Directions", messages: [first, reply] }, fmt);
  assert.equal(out.match(/round two directions/g)?.length, 1);
  assert.ok(out.endsWith("\n\nAttached. B pushes the wordmark further.\n\nPriya\n"), out);
  assert.ok(!out.includes("wrote:"));
});

test("history the thread doesn't have (a quote of older mail) stays", () => {
  const reply = msg({
    bodyText: "Looping you in, see below.\n\nOn Fri, Aug 7, 2026 at 3:10 PM Dana Wu <dana@acme.example> wrote:\n> The venue can hold 40 people.",
  });
  const out = conversationText({ subject: "Venue", messages: [reply] }, fmt);
  assert.ok(out.includes("The venue can hold 40 people."));
  assert.ok(out.includes("Dana Wu <dana@acme.example> wrote:"));
});

test("Gmail's HTML quote block is cut when it repeats the earlier message", () => {
  const first = msg({ from: sam, to: [priya], bodyText: "Is the warm gray staying in the secondary palette?" });
  const reply = msg({
    bodyText: "",
    html: HTML_DOC(
      `<div dir="ltr"><div>Keeping it, <a href="https://docs.linden.example/palette?utm_source=mail&amp;id=4">see the palette page</a>.</div></div>` +
        `<br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Mon, Sep 21, 2026 at 9:00 AM Sam Okafor &lt;<a href="mailto:sam@northwind.example">sam@northwind.example</a>&gt; wrote:<br></div>` +
        `<blockquote class="gmail_quote" style="margin:0 0 0 .8ex">Is the warm gray staying in the secondary palette?</blockquote></div>`,
    ),
  });
  const [, body] = conversationText({ subject: "Palette", messages: [first, reply] }, fmt).split("=== Message 2 of 2 ===");
  assert.ok(body.includes("Keeping it, see the palette page (https://docs.linden.example/palette?id=4)."), body);
  assert.ok(!body.includes("warm gray"));
  assert.ok(!body.includes("wrote:"));
});

test("a forward's payload is kept", () => {
  const fwd = msg({
    bodyText: "",
    html: HTML_DOC(
      `<div>FYI, below.</div><div class="gmail_quote"><div class="gmail_attr">---------- Forwarded message ---------<br>From: Dana Wu &lt;dana@acme.example&gt;<br>Subject: Offsite<br></div><br><div>Strategy Tuesday morning, retro Wednesday.</div></div>`,
    ),
  });
  const out = conversationText({ subject: "Fwd: Offsite", messages: [fwd] }, fmt);
  assert.ok(out.includes("FYI, below."));
  assert.ok(out.includes("Forwarded message"));
  assert.ok(out.includes("Strategy Tuesday morning, retro Wednesday."));
});

test("HTML to text: paragraphs, line breaks, lists, links as text (url)", () => {
  const { full } = htmlToText(
    HTML_DOC(
      `<h2>Plan</h2><p>Two changes:<br>both small.</p>` +
        `<ul><li>Re-timed milestones</li><li><p>Budget <b>unchanged</b></p><ol><li>Q4</li><li>Q1</li></ol></li></ul>` +
        `<p>Read <a href="https://notes.acme.example/r">the report</a>, mail <a href="mailto:dana@acme.example">Dana</a> or visit <a href="https://acme.example/">acme.example</a>.</p>` +
        `<p><a href="https://acme.example/home"><img src="cid:logo" alt="Acme"></a></p>` +
        `<pre>  keep   this\n  spacing</pre><p>Café &amp; more&nbsp;&mdash; done&#8230;</p>`,
    ),
  );
  assert.equal(
    full,
    [
      "Plan",
      "",
      "Two changes:",
      "both small.",
      "",
      "- Re-timed milestones",
      "- Budget unchanged",
      "",
      "  1. Q4",
      "  2. Q1",
      "",
      "Read the report (https://notes.acme.example/r), mail Dana (dana@acme.example) or visit acme.example.",
      "",
      "  keep   this",
      "  spacing",
      "",
      "Café & more — done…",
    ].join("\n"),
  );
});

test("HTML to text drops style, hidden preheaders and invisible filler", () => {
  const { full } = htmlToText(
    HTML_DOC(
      `<div style="display:none;max-height:0;overflow:hidden">Preview text you never see&zwnj;&nbsp;&zwnj;&nbsp;</div>` +
        `<span style="mso-hide: all">Outlook only</span>` +
        `<table><tr><td>Weekly</td><td>digest</td></tr><tr><td>Issue 12</td></tr></table>` +
        `<div class="pg-simplified">Simplified view: this message's formatting was too complex to display safely.</div>` +
        `<p>‌​</p><p>Hello</p>`,
    ),
  );
  assert.equal(full, "Weekly digest\nIssue 12\n\nHello");
});

test("the text/plain part is used when it's the real message, the HTML when it's a stand-in", () => {
  const html = HTML_DOC(`<p>Our new issue is out. <a href="https://news.acme.example/12">Read it here</a>.</p><p>Three short pieces and one long read.</p>`);
  const rich = htmlToText(html);
  // Real multipart plain text, links spelled out: keep it.
  assert.equal(plainUsable("Our new issue is out. Read it here: https://news.acme.example/12\n\nThree short pieces and one long read.", rich), true);
  // The backend's reading of an HTML-only message (links dropped): use the HTML.
  assert.equal(plainUsable("Our new issue is out. Read it here.\n\nThree short pieces and one long read.", rich), false);
  // "View this email in your browser": use the HTML.
  assert.equal(plainUsable("View this email in your browser.", rich), false);
  assert.equal(plainUsable("", rich), false);

  const m = msg({ bodyText: "Our new issue is out. Read it here.\n\nThree short pieces and one long read.", html });
  assert.equal(bodyParts(m).full, "Our new issue is out. Read it here (https://news.acme.example/12).\n\nThree short pieces and one long read.");
  // A plain-text message (penguin-render's text document): the plain part as is.
  const plain = msg({ bodyText: "See https://acme.example/x\n\nThanks", html: TEXT_DOC("See") });
  assert.equal(bodyParts(plain).full, "See https://acme.example/x\n\nThanks");
});

test("a message still downloading copies its snippet and says so", () => {
  const m = msg({ bodyPending: true, snippet: "Plumber confirmed Thursday between 10 and 12" });
  assert.ok(bodyParts(m).full.startsWith("Plumber confirmed Thursday between 10 and 12 …"));
  assert.ok(bodyParts(m).full.includes("Only the start of this message has downloaded"));
});

test("plain-text reply splitting mirrors the backend's rules", () => {
  assert.deepEqual(splitQuoted("Sounds good.\n\n> earlier line\nThanks"), { authored: "Sounds good.\n\nThanks", quoted: "earlier line" });
  assert.equal(splitQuoted("Yes.\n\n-----Original Message-----\nFrom: Dana\nold").authored, "Yes.");
  const outlook = "Works for me.\n\nFrom: Dana Wu <dana@acme.example>\nSent: Monday, September 21, 2026 9:00 AM\nTo: Sam\nSubject: Offsite\n\nold text";
  assert.equal(splitQuoted(outlook).authored, "Works for me.");
  const fwd = "FYI\n\n---------- Forwarded message ---------\nFrom: Dana Wu <dana@acme.example>\nDate: Mon, Sep 21, 2026\nSubject: Offsite\nTo: Sam\n\nPayload";
  assert.equal(splitQuoted(fwd).authored, fwd);
});

test("copying one message cuts the history it repeats and names it as a draft when it is one", () => {
  const first = msg({ id: "x1", from: sam, bodyText: "Can we move the stakeholder review to Monday?" });
  const draft = msg({
    id: "x2",
    from: sam,
    labelIds: ["DRAFT"],
    subject: "Re: Review date",
    bodyText: "Monday works.\n\nOn Tue, Sep 22, 2026 at 9:00 AM Sam Okafor <sam@northwind.example> wrote:\n> Can we move the stakeholder review to Monday?",
  });
  const out = messageText({ subject: "Review date", messages: [first, draft] }, "x2", fmt);
  assert.ok(out.startsWith("Subject: Re: Review date\n\nFrom: Sam Okafor <sam@northwind.example>\n"));
  assert.ok(out.includes("Draft: not sent yet"));
  assert.ok(out.includes("Monday works."));
  assert.ok(!out.includes("stakeholder review"));
  assert.equal(messageText({ subject: "x", messages: [first] }, "missing"), "");
});
