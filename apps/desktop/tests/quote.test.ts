// Replies and forwards (features/compose/quote.ts, quoteDom.ts): the quoted
// original in Gmail's markup with its formatting, the text alternative, and
// which files a forward carries.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  attachmentKey,
  forwardAttachments,
  forwardFiles,
  hasAllFiles,
  pickMessage,
  quoteHtml,
  quotedText,
  withFiles,
  withoutFiles,
  type Quote,
} from "../src/features/compose/quote.ts";
import type { AttachmentMeta } from "../src/lib/types.ts";

const att = (id: string, filename: string, size: number, inline = false): AttachmentMeta => ({
  id,
  filename,
  mimeType: "application/pdf",
  size,
  contentId: inline ? `${id}@acme.example` : null,
  inline,
});

// What quote_sources returns for a formatted original (sanitize_quoted_html output).
const ORIGINAL_HTML = '<h2>Plan</h2><p><b>Two</b> changes:</p><ul><li><i>re-timed</i> milestones</li></ul><p><a href="https://docs.acme.example/r">report</a></p>';

const forward: Quote = {
  forward: true,
  header: "---------- Forwarded message ----------\nFrom: Dana Reyes <dana@acme.example>\nDate: Sep 22, 2026, 9:41 AM\nSubject: Scope v3\nTo: sam@northwind.example",
  text: "Plan\n\nTwo changes:\n- re-timed milestones",
  html: ORIGINAL_HTML,
};

test("a forward quotes the original's HTML in Gmail's forward markup", () => {
  assert.equal(
    quoteHtml(forward),
    '<br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">---------- Forwarded message ----------<br>From: Dana Reyes &lt;dana@acme.example&gt;<br>Date: Sep 22, 2026, 9:41 AM<br>Subject: Scope v3<br>To: sam@northwind.example<br></div><br><br>' +
      ORIGINAL_HTML +
      "</div>",
  );
  // The text part keeps the plain header block and text.
  assert.equal(quotedText(forward), `${forward.header}\n\n${forward.text}`);
});

test("a reply quotes the original in Gmail's blockquote; its text part is >-prefixed", () => {
  const reply: Quote = { forward: false, header: "On Sep 22, 2026, Dana Reyes <dana@acme.example> wrote:", text: "Plan\n> earlier", html: ORIGINAL_HTML };
  assert.equal(
    quoteHtml(reply),
    '<br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Sep 22, 2026, Dana Reyes &lt;dana@acme.example&gt; wrote:<br></div><blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex">' +
      ORIGINAL_HTML +
      "</blockquote></div>",
  );
  // Nested quotes (a reply to a reply) deepen.
  assert.equal(quotedText(reply), "On Sep 22, 2026, Dana Reyes <dana@acme.example> wrote:\n> Plan\n>> earlier");
});

test("without HTML (text-only mail, or not loaded) the text is quoted, escaped", () => {
  const q: Quote = { forward: false, header: "On x, a@b.example wrote:", text: "a <b> & c\nline 2\n\npara", html: null };
  assert.match(quoteHtml(q), /<blockquote [^>]*><p>a &lt;b&gt; &amp; c<br>line 2<\/p><p>para<\/p><\/blockquote>/);
});

test("forwarding a conversation from the bottom brings the files further up", () => {
  // Dana's message has the files; my short reply (the one forwarded) has none.
  const dana = { id: "m0", attachments: [att("a1", "Scope_of_work_v3.pdf", 1000), att("a2", "Report.docx", 2000), att("logo", "logo.png", 10, true)] };
  const mine = { id: "m1", attachments: [] };
  const { attachments, earlier } = forwardAttachments([], mine, [dana]);
  assert.deepEqual(
    attachments.map((a) => a.kind === "gmail" && [a.messageId, a.attachmentId, a.filename]),
    [
      ["m0", "a1", "Scope_of_work_v3.pdf"],
      ["m0", "a2", "Report.docx"],
    ],
    "attached by default; inline images stay out",
  );
  assert.equal(earlier.length, 2);
  assert.ok(hasAllFiles(attachments, earlier), "the switch shows on");
});

test("a forwarded message with its own files offers the earlier ones, off", () => {
  const v2 = { id: "m0", attachments: [att("a1", "Deck_v2.pdf", 1000), att("a9", "Budget.xlsx", 50)] };
  const v3 = { id: "m1", attachments: [att("b1", "Deck_v3.pdf", 1200), att("b2", "Budget.xlsx", 50)] };
  const { attachments, earlier } = forwardAttachments([], v3, [v2]);
  assert.deepEqual(attachments.map((a) => a.filename), ["Deck_v3.pdf", "Budget.xlsx"]);
  assert.deepEqual(earlier.map((a) => a.filename), ["Deck_v2.pdf"], "the same Budget.xlsx isn't offered twice");
  assert.ok(!hasAllFiles(attachments, earlier));
  // Switching on and off adds and removes exactly those.
  const on = withFiles(attachments, earlier);
  assert.deepEqual(on.map((a) => a.filename), ["Deck_v3.pdf", "Budget.xlsx", "Deck_v2.pdf"]);
  assert.deepEqual(withoutFiles(on, earlier).map((a) => a.filename), ["Deck_v3.pdf", "Budget.xlsx"]);
});

test("files the user attached stay; a file sent twice is offered once, from its newest message", () => {
  const picked = { kind: "file" as const, filename: "notes.txt", mimeType: "text/plain", dataBase64: "bm90ZXM=" };
  const files = forwardFiles({ id: "m2", attachments: [] }, [
    { id: "m0", date: 1, attachments: [att("x", "Plan.pdf", 7)] },
    { id: "m1", date: 2, attachments: [att("y", "Plan.pdf", 7), att("z", "Map.png", 3)] },
  ]);
  assert.deepEqual(files.earlier.map((a) => a.kind === "gmail" && a.messageId + "/" + a.attachmentId), ["m1/y", "m1/z"]);
  const { attachments } = forwardAttachments([picked], { id: "m2", attachments: [] }, [
    { id: "m1", attachments: [att("y", "Plan.pdf", 7)] },
  ]);
  assert.deepEqual(attachments.map(attachmentKey), [attachmentKey(picked), "Plan.pdf\u00007"]);
});

test("the message answered is the one asked for, else the latest that isn't my draft", () => {
  const msgs = [
    { id: "a", labelIds: ["INBOX"] },
    { id: "b", labelIds: ["SENT"] },
    { id: "c", labelIds: ["DRAFT"] },
  ];
  assert.equal(pickMessage(msgs, "a")?.id, "a");
  assert.equal(pickMessage(msgs, undefined)?.id, "b");
  assert.equal(pickMessage(msgs, "gone")?.id, "b");
  assert.equal(pickMessage([{ id: "d", labelIds: ["DRAFT"] }], undefined)?.id, "d");
});

// A reopened draft (HTML through the composer allowlist) gives its quote back.
const { Window } = await import("happy-dom");
(globalThis as { DOMParser?: unknown }).DOMParser = new Window().DOMParser;
const { quoteFromHtml } = await import("../src/features/compose/quoteDom.ts");

test("a reopened reply or forward gets its quoted original's HTML back", () => {
  const wrap = (q: Quote) => `<div dir="auto"><p>My note</p>${quoteHtml(q)}</div>`;
  assert.equal(quoteFromHtml(wrap(forward)), ORIGINAL_HTML);
  assert.equal(quoteFromHtml(wrap({ ...forward, forward: false, header: "On x, y wrote:" })), ORIGINAL_HTML);
  // Round trip: re-wrapping the extracted HTML gives the same part.
  const reopened: Quote = { ...forward, html: quoteFromHtml(wrap(forward)) };
  assert.equal(quoteHtml(reopened), quoteHtml(forward));
  assert.equal(quoteFromHtml("<p>no quote</p>"), null);
  // Drafts saved before quotes carried HTML fall back to the text part.
  assert.equal(quoteFromHtml('<div class="gmail_quote"><p>header</p><p>text</p></div>'), null);
});
