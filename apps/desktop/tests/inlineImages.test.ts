// Inline images in the composer (features/compose/inline.ts and the editor's
// inlineImage node): what the HTML and text parts say, which attachments go
// out, and that a saved draft's images come back as images.
import { test } from "node:test";
import assert from "node:assert/strict";
import { composerSchema } from "../src/features/compose/editor/schema.ts";
import { docCids, docToEmailHtml, docToText, type Doc } from "../src/features/compose/editor/serialize.ts";
import {
  asFile,
  fileRows,
  inlineWidth,
  isSafeCid,
  MAX_INLINE_IMAGE_BYTES,
  newContentId,
  referencedCids,
  sendableAttachments,
  splitDropped,
} from "../src/features/compose/inline.ts";
import { attachmentKey, withFiles } from "../src/features/compose/quote.ts";
import type { OutgoingAttachment } from "../src/lib/types.ts";

const img = (cid: string, alt = "chart.png", width: number | null = 600): Doc => ({ type: "inlineImage", attrs: { cid, alt, width } });
const doc = (...content: Doc[]): Doc => ({ type: "doc", content: [{ type: "paragraph", content }] });
const t = (text: string): Doc => ({ type: "text", text });

const pasted = (cid: string, name = "chart.png"): OutgoingAttachment => ({ kind: "file", filename: name, mimeType: "image/png", dataBase64: "iVBORw0K", contentId: cid });
const pdf: OutgoingAttachment = { kind: "file", filename: "plan.pdf", mimeType: "application/pdf", dataBase64: "JVBERi0x" };

test("an inline image in the HTML part is a cid: reference, sized, and '[image: …]' in the text part", () => {
  const d = doc(t("See "), img("img-00aa@penguin"), t(" above"));
  assert.equal(
    docToEmailHtml(d),
    '<p style="margin:0">See <img src="cid:img-00aa@penguin" alt="chart.png" width="600" style="max-width:100%;height:auto"> above</p>',
  );
  assert.equal(docToText(d), "See [image: chart.png] above");
  assert.deepEqual(docCids(d), ["img-00aa@penguin"]);
  // An id the backend wouldn't write is never serialized; alt text is escaped.
  assert.equal(docToEmailHtml(doc(img('x" onerror="y'), img("ok@x", 'a"<b>', null))), '<p style="margin:0"><img src="cid:ok@x" alt="a&quot;&lt;b&gt;" style="max-width:100%;height:auto"></p>');
});

test("fresh Content-IDs are unique and in the backend's safe alphabet", () => {
  const ids = new Set(Array.from({ length: 200 }, newContentId));
  assert.equal(ids.size, 200);
  for (const id of ids) assert.ok(isSafeCid(id) && /^img-[0-9a-f]{24}@penguin$/.test(id), id);
  for (const bad of ["", "a b", 'a"b', "a\r\nb", "x".repeat(201)]) assert.ok(!isSafeCid(bad), bad);
});

test("only images the HTML still shows are sent; the rest stay for undo", () => {
  const list = [pasted("img-1@penguin"), pdf, pasted("img-2@penguin", "gone.png")];
  const html = '<p><img src="cid:img-1@penguin" alt="x"></p>';
  assert.deepEqual(referencedCids(html), new Set(["img-1@penguin"]));
  assert.deepEqual(
    sendableAttachments(list, html).map((a) => a.filename),
    ["chart.png", "plan.pdf"],
  );
  assert.deepEqual(sendableAttachments(list, null).map((a) => a.filename), ["plan.pdf"]);
  // The attachment row shows files only, with their index in the list.
  assert.deepEqual(
    fileRows(list).map((r) => [r.a.filename, r.i]),
    [["plan.pdf", 1]],
  );
});

test("Send as attachment turns the image into an ordinary file", () => {
  const list = asFile([pasted("img-1@penguin"), pdf], "img-1@penguin");
  assert.equal(list[0].contentId, undefined);
  assert.equal(list[0].filename, "chart.png");
  assert.deepEqual(fileRows(list).map((r) => r.a.filename), ["chart.png", "plan.pdf"]);
});

test("pasted files: small images inline, big images and other files attach", () => {
  const f = (name: string, type: string, size: number) => ({ name, type, size });
  const s = splitDropped([f("a.png", "image/png", 10), f("b.pdf", "application/pdf", 10), f("c.jpg", "image/jpeg", MAX_INLINE_IMAGE_BYTES + 1), f("d.svg", "image/svg+xml", 10)]);
  assert.deepEqual(s.inline.map((x) => x.name), ["a.png"]);
  assert.deepEqual(s.tooBig.map((x) => x.name), ["c.jpg"]);
  assert.deepEqual(s.files.map((x) => x.name), ["c.jpg", "b.pdf", "d.svg"]);
  assert.equal(inlineWidth(2880), 600);
  assert.equal(inlineWidth(320.4), 320);
  assert.equal(inlineWidth(0), null);
});

test("two images with the same name and size are two attachments", () => {
  const a = pasted("img-1@penguin", "image.png");
  const b = pasted("img-2@penguin", "image.png");
  assert.notEqual(attachmentKey(a), attachmentKey(b));
  assert.equal(withFiles([a], [b]).length, 2);
  // A file (no contentId) keeps its old key, so forwarded files still dedupe.
  assert.equal(attachmentKey(pdf), `plan.pdf\u0000${6}`);
});

// A real DOM for parsing (reopening a draft).
const { Window } = await import("happy-dom");
const win = new Window();
(globalThis as { DOMParser?: unknown }).DOMParser = win.DOMParser;
const { htmlToDoc, withoutQuote } = await import("../src/features/compose/editor/dom.ts");

test("a reopened draft's images come back as images; other image sources never parse", () => {
  const body = docToEmailHtml(doc(t("See "), img("img-00aa@penguin")));
  const saved = `<div dir="auto">${body}<br><div class="gmail_quote"><div class="gmail_attr">On …</div><blockquote><p><img src="cid:img-q@penguin"></p></blockquote></div></div>`;
  const reopened = htmlToDoc(composerSchema(), withoutQuote(saved)).toJSON();
  assert.equal(docToEmailHtml(reopened), body, "same HTML part after a save and reopen");
  assert.deepEqual(docCids(reopened), ["img-00aa@penguin"], "the quote's image stays with the quote");
  const hostile = htmlToDoc(
    composerSchema(),
    '<p>a<img src="https://t.example/p.gif"><img src="data:image/png;base64,AAAA"><img src="cid:bad id"><img src="CID:ok@x" width="-3"></p>',
  ).toJSON();
  assert.equal(docToEmailHtml(hostile), '<p style="margin:0">a<img src="cid:ok@x" alt="" style="max-width:100%;height:auto"></p>');
});
