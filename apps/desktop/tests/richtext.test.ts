// Rich-text composer (features/compose/editor): the document → HTML and
// text/plain parts, link rules, and signature switching on the real schema.
import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorState } from "@tiptap/pm/state";
import { composerSchema } from "../src/features/compose/editor/schema.ts";
import { docToEmailHtml, docToText, isDocEmpty, linkAllowed, normalizeHref, textToDoc, type Doc } from "../src/features/compose/editor/serialize.ts";
import { findSignature, setSignature, signatureNode } from "../src/features/compose/editor/signature.ts";
import { assignDefaults, autoInserts, defaultSignature, lockedAccount } from "../src/features/compose/editor/pick.ts";

const t = (text: string, ...marks: Array<string | { type: string; attrs: Record<string, unknown> }>): Doc => ({
  type: "text",
  text,
  ...(marks.length ? { marks: marks.map((m) => (typeof m === "string" ? { type: m } : m)) } : {}),
});
const p = (...content: Doc[]): Doc => (content.length ? { type: "paragraph", content } : { type: "paragraph" });
const link = (href: string) => ({ type: "link", attrs: { href } });
const li = (...content: Doc[]): Doc => ({ type: "listItem", content });

/** One of everything the toolbar can make. */
const FORMATTED: Doc = {
  type: "doc",
  content: [
    p(),
    p(t("Hi "), t("Dana", "bold"), t(", see "), t("the plan", link("https://acme.example/plan")), t(" & "), t("x < y", "code"), t(".")),
    { type: "heading", attrs: { level: 2 }, content: [t("Agenda")] },
    { type: "bulletList", content: [li(p(t("Budget ", "italic"), t("review", "underline"))), li(p(t("old", "strike"), t(" new")))] },
    { type: "orderedList", attrs: { start: 3 }, content: [li(p(t("three"))), li(p(t("four")), { type: "bulletList", content: [li(p(t("nested")))] })] },
    { type: "blockquote", content: [p(t("quoted")), p(t("twice"))] },
    { type: "codeBlock", content: [t("let a = 1;\nlet b = <2>;")] },
    { type: "horizontalRule" },
    p(t("Mail "), t("dana@acme.example", link("mailto:dana@acme.example")), t(" or "), t("acme.example", link("https://acme.example"))),
    p(t("line one"), { type: "hardBreak" }, t("line two")),
    p(),
    { type: "signature", content: [p(t("--")), p(t("Ana Ruiz", "bold")), p(t("Acme", link("https://acme.example")))] },
    p(),
    p(),
  ],
};

test("text/plain: lists, quotes, code, links as text (url), signature dash", () => {
  assert.equal(
    docToText(FORMATTED),
    [
      "Hi Dana, see the plan (https://acme.example/plan) & x < y.",
      "Agenda",
      "- Budget review",
      "- old new",
      "3. three",
      "4. four",
      "   - nested",
      "> quoted",
      "> twice",
      "let a = 1;",
      "let b = <2>;",
      "----------",
      "Mail dana@acme.example or acme.example",
      "line one",
      "line two",
      "",
      "-- ",
      "Ana Ruiz",
      "Acme (https://acme.example)",
    ].join("\n"),
  );
});

test("HTML part: escaped, inline-styled, blank edges trimmed, signature marked", () => {
  const html = docToEmailHtml(FORMATTED);
  assert.ok(html.startsWith('<p style="margin:0">Hi <strong>Dana</strong>, see <a href="https://acme.example/plan">the plan</a> &amp; <code style="'), html);
  assert.ok(html.includes(">x &lt; y</code>"));
  assert.ok(html.includes('<h2 style="margin:0.6em 0 0.3em;font-size:1.25em;line-height:1.3">Agenda</h2>'));
  assert.ok(html.includes("<li><em>Budget </em><u>review</u></li>"), "one-paragraph items stay inline");
  assert.ok(html.includes('<ol start="3" style="margin:0.25em 0;padding-left:1.6em">'));
  assert.ok(html.includes('<blockquote style="margin:0.25em 0 0.25em 0.8ex;border-left:1px solid #ccc;padding-left:1ex;color:#555">'));
  assert.ok(html.includes("<code>let a = 1;\nlet b = &lt;2&gt;;</code></pre>"));
  assert.ok(html.includes("line one<br>line two"));
  assert.ok(html.endsWith('<div class="penguin-signature"><p style="margin:0">--</p><p style="margin:0"><strong>Ana Ruiz</strong></p><p style="margin:0"><a href="https://acme.example">Acme</a></p></div>'), html);
  assert.ok(!html.startsWith('<p style="margin:0"><br>'), "leading blank line dropped");
  // Every style value stays inside penguin-render's outgoing allowlist charset (no parens, backslashes, <, &, ;).
  for (const m of html.matchAll(/style="([^"]*)"/g)) {
    for (const decl of m[1].split(";")) assert.match(decl.split(":").slice(1).join(":"), /^[A-Za-z0-9 #.,%'"-]+$/, decl);
  }
});

test("links: only http(s) with a host and mailto survive, everywhere", () => {
  for (const ok of ["https://acme.example", "http://acme.example/a?b=1#c", "mailto:dana@acme.example"]) assert.ok(linkAllowed(ok), ok);
  for (const bad of ["javascript:alert(1)", " JavaScript:alert(1)", "data:text/html,x", "vbscript:x", "file:///etc/passwd", "//acme.example", "https://", "cid:x", "", null])
    assert.ok(!linkAllowed(bad), String(bad));
  assert.equal(normalizeHref("acme.example/agenda"), "https://acme.example/agenda");
  assert.equal(normalizeHref("dana@acme.example"), "mailto:dana@acme.example");
  assert.equal(normalizeHref("javascript:alert(1)"), null);
  const evil: Doc = { type: "doc", content: [p(t("click", link("javascript:alert(1)")), t(" <img src=x onerror=alert(1)>"))] };
  assert.equal(docToEmailHtml(evil), '<p style="margin:0">click &lt;img src=x onerror=alert(1)&gt;</p>');
  assert.equal(docToText(evil), "click <img src=x onerror=alert(1)>");
});

test("plain text ↔ doc for older drafts; empty docs", () => {
  const doc = textToDoc("Hi\n\nBye");
  assert.equal(docToText(doc), "Hi\n\nBye");
  assert.equal(docToEmailHtml(doc), '<p style="margin:0">Hi</p><p style="margin:0"><br></p><p style="margin:0">Bye</p>');
  assert.ok(isDocEmpty(textToDoc("\n\n")));
  assert.ok(isDocEmpty(null));
  // How email HTML's <p><br></p> parses: still a blank line, not two.
  assert.equal(docToText({ type: "doc", content: [p(t("a")), p({ type: "hardBreak" }), p(t("b"))] }), "a\n\nb");
});

test("the schema drops what the composer can't make", () => {
  const schema = composerSchema();
  assert.deepEqual(Object.keys(schema.marks).sort(), ["bold", "code", "italic", "link", "strike", "underline"]);
  for (const n of ["image", "table", "iframe", "script"]) assert.ok(!schema.nodes[n], n);
  assert.ok(schema.nodes.signature);
});

const settings = {
  signatures: [
    { id: "work", name: "Work", html: "<p><strong>Ana Ruiz</strong></p>" },
    { id: "home", name: "Home", html: "<p>— Ana</p>" },
  ],
  signatureDefaults: { "ana@acme.example": "work", "ana@home.example": "home", "gone@x.example": "deleted" } as Record<string, string>,
  signatureInsert: { newMessages: true, replies: false, forwards: true },
};

test("which signature an account uses, and when it goes in", () => {
  assert.equal(defaultSignature(settings, "ana@acme.example")?.id, "work");
  assert.equal(defaultSignature(settings, "Ana@Home.example")?.id, "home");
  assert.equal(defaultSignature(settings, "gone@x.example"), null, "a default pointing at a deleted signature is none");
  assert.equal(defaultSignature(settings, "other@x.example"), null);
  assert.equal(autoInserts(settings, "new"), true);
  assert.equal(autoInserts(settings, "reply"), false);
  assert.equal(autoInserts(settings, "replyAll"), false);
  assert.equal(autoInserts(settings, "forward"), true);
});

test("signature switching: insert, swap in place, separator, remove", () => {
  const schema = composerSchema();
  const work = [schema.node("paragraph", null, schema.text("Ana Ruiz", [schema.marks.bold.create()]))];
  const home = [schema.node("paragraph", null, schema.text("— Ana"))];
  let state = EditorState.create({ doc: schema.nodeFromJSON(textToDoc("")) });
  const apply = (f: (tr: typeof state.tr) => unknown) => {
    const tr = state.tr;
    f(tr);
    state = state.apply(tr);
  };
  const text = () => docToText(state.doc.toJSON());

  // Fresh message: caret line, blank line, signature.
  apply((tr) => setSignature(tr, signatureNode(schema, work, false)));
  assert.equal(state.doc.childCount, 3);
  assert.equal(text(), "Ana Ruiz", "blank edges are trimmed in the text part");
  assert.equal(findSignature(state.doc)?.pos, state.doc.content.size - findSignature(state.doc)!.node.nodeSize);

  // Typing above it, then switching From: replaced in place, my text kept.
  apply((tr) => tr.insertText("Hello", 1));
  apply((tr) => setSignature(tr, signatureNode(schema, home, true)));
  assert.equal(text(), "Hello\n\n-- \n— Ana");
  assert.equal(state.doc.childCount, 3, "no second signature");

  // A signature that already starts with a dash line doesn't get another.
  const dashed = [schema.node("paragraph", null, schema.text("--")), ...home];
  apply((tr) => setSignature(tr, signatureNode(schema, dashed, true)));
  assert.equal(text(), "Hello\n\n-- \n— Ana");

  // An account without one: the block and its blank line go.
  apply((tr) => setSignature(tr, null));
  assert.equal(findSignature(state.doc), null);
  assert.equal(state.doc.childCount, 1);
  assert.equal(text(), "Hello");

  // Nothing to do: no change.
  const tr = state.tr;
  setSignature(tr, null);
  assert.equal(tr.docChanged, false);
  assert.equal(signatureNode(schema, [], true), null, "an empty signature is none");
});

// A real DOM for the HTML-parsing half (what the editor does on reopen/paste).
const { Window } = await import("happy-dom");
const win = new Window();
(globalThis as { DOMParser?: unknown }).DOMParser = win.DOMParser;
const { htmlToDoc, htmlToBlocks, withoutQuote } = await import("../src/features/compose/editor/dom.ts");

test("draft round trip: doc → saved HTML part → reopened doc keeps the formatting", () => {
  const schema = composerSchema();
  const body = docToEmailHtml(FORMATTED);
  // What toDraft saves: font wrapper, then the quoted original.
  const saved =
    `<div dir="auto" style="font-family:'Inter', sans-serif;font-size:15px">${body}` +
    `<br><div class="gmail_quote"><div>On Sep 24, Priya wrote:</div><blockquote><p>original</p></blockquote></div></div>`;
  const reopened = htmlToDoc(schema, withoutQuote(saved));
  assert.equal(docToEmailHtml(reopened.toJSON()), body, "same HTML part after a save and reopen");
  assert.equal(docToText(reopened.toJSON()), docToText(FORMATTED), "same text part");
  assert.ok(findSignature(reopened), "the signature block comes back as a signature");
  assert.ok(!docToText(reopened.toJSON()).includes("original"), "the quote stays out of the editor");
});

test("pasted Google Docs HTML (after the sanitizer) keeps only real formatting", () => {
  // penguin-render's sanitize_compose_html output for a Docs clipboard
  // (see google_docs_paste_keeps_formatting_styles_only in compose.rs).
  const clean =
    '<b style="font-weight:normal"><p dir="ltr"><span style="font-weight:700">Agenda</span> for <span style="font-style:italic;text-decoration:underline">Thursday</span></p><ul><li dir="ltr"><p dir="ltr">Budget <a>evil</a> <a href="https://acme.example/b">sheet</a></p></li></ul></b>';
  const doc = htmlToDoc(composerSchema(), clean).toJSON();
  assert.equal(
    docToEmailHtml(doc),
    '<p style="margin:0"><strong>Agenda</strong> for <em><u>Thursday</u></em></p><ul style="margin:0.25em 0;padding-left:1.6em"><li>Budget evil <a href="https://acme.example/b">sheet</a></li></ul>',
    "the Docs wrapper <b style=font-weight:normal> doesn't bold everything; href-less links drop",
  );
  // Even unsanitized, the schema parser keeps no scripts, images or bad links.
  const raw = htmlToDoc(composerSchema(), '<p onclick="x()">a<script>alert(1)</script><img src=x onerror=alert(1)><a href="javascript:alert(1)">b</a><iframe src="https://evil.example"></iframe></p>').toJSON();
  assert.equal(docToEmailHtml(raw), '<p style="margin:0">ab</p>');
});

test("signature HTML from settings parses into blocks (nested signatures flattened)", () => {
  const schema = composerSchema();
  const blocks = htmlToBlocks(schema, '<div class="penguin-signature"><p><strong>Ana</strong></p><p>Acme</p></div>');
  assert.deepEqual(blocks.map((b) => b.type.name), ["paragraph", "paragraph"]);
  const sig = signatureNode(schema, blocks, false)!;
  assert.equal(docToText({ type: "doc", content: [sig.toJSON()] }), "Ana\nAcme");
});

test("assignDefaults sets the signature on exactly the ticked accounts", () => {
  const before = { "a@x.example": "work", "b@x.example": "work", "c@x.example": "home" };
  assert.deepEqual(assignDefaults(before, "work", ["B@x.example", "d@x.example"]), {
    "b@x.example": "work",
    "c@x.example": "home",
    "d@x.example": "work",
  });
  // Deleting: no accounts keep it, others are untouched.
  assert.deepEqual(assignDefaults(before, "work", []), { "c@x.example": "home" });
});

test("lockedAccount holds replies and forwards to the thread's account", () => {
  const thread = { accountId: "a@x.example", threadId: "t1" };
  for (const mode of ["reply", "replyAll", "forward"] as const) {
    assert.equal(lockedAccount({ lockReplyAccount: true }, { mode, thread }), "a@x.example");
    assert.equal(lockedAccount({ lockReplyAccount: false }, { mode, thread }), null);
  }
  assert.equal(lockedAccount({ lockReplyAccount: true }, { mode: "new", thread }), null);
  assert.equal(lockedAccount({ lockReplyAccount: true }, { mode: "reply" }), null);
});

test("pasted plain text keeps its blank lines: a paragraph per line, an empty one per blank line", async () => {
  const { plainTextSlice } = await import("../src/features/compose/editor/textPaste.ts");
  const schema = composerSchema();
  const slice = plainTextSlice(schema, "Hi Dana,\n\nThe plan is attached.\nSee you Monday.\n\nSam\n");
  const lines = slice.content.content.map((n) => n.textContent);
  assert.deepEqual(lines, ["Hi Dana,", "", "The plan is attached.", "See you Monday.", "", "Sam"]);
  assert.equal(slice.openStart, 1);
  assert.equal(slice.openEnd, 1);
});
