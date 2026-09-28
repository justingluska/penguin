// Compose and reply speed (docs/COMPOSE-SPEED.md): snippet variables and
// extras, instant replies, Write with AI's requests and in-editor
// suggestions, send-later presets and times typed in words.
import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorState, TextSelection } from "@tiptap/pm/state";
import {
  companyFromEmail,
  expandSnippet,
  matchSnippets,
  nameParts,
  snippetExtras,
  unfilledPlaceholders,
} from "../src/features/compose/snippets.ts";
import { instantChoices, setComposeIntent, takeComposeIntent, wantsSuggestions } from "../src/features/compose/instant.ts";
import { buildRequest, isEditInstruction, promptPlaceholder, writerUnavailableText } from "../src/features/compose/writePrompt.ts";
import { parseWhen, sendLaterPresets } from "../src/features/compose/when.ts";
import { aiTargetOf, rangeText, textContent, writingRange } from "../src/features/compose/editor/aiSuggest.ts";
import { composerSchema } from "../src/features/compose/editor/schema.ts";
import type { Address, InstantReplies, Snippet } from "../src/lib/types.ts";

const priya: Address = { name: "Priya Raman", email: "priya@mail.northwind.example" };
const ctx = { to: [priya], replyingTo: { name: "Dana Ortiz", email: "dana@harbor.example" }, myName: "Sam Okafor", now: new Date(2026, 8, 24) };

// ---------------------------------------------------------------------------
// Snippets
// ---------------------------------------------------------------------------

test("snippet variables fill from the first recipient, the sender and me", () => {
  const r = expandSnippet("{first_name} {last_name} / {full_name} at {company}; {sender_name}; {my_name} ({my_first_name}), {date}", ctx);
  assert.equal(r.text, "Priya Raman / Priya Raman at Northwind; Dana Ortiz; Sam Okafor (Sam), September 24, 2026");
  assert.equal(r.selStart, r.text.length);
});

test("the caret lands on {cursor}, else the first unknown placeholder is selected", () => {
  const c = expandSnippet("Hi {first_name}, {cursor} thanks", ctx);
  assert.equal(c.text, "Hi Priya,  thanks");
  assert.equal(c.selStart, 10);
  const noName = expandSnippet("Hi {first_name}, see you {day}", { ...ctx, to: [{ name: null, email: "ops@northwind.example" }] });
  assert.equal(noName.text, "Hi {first_name}, see you {day}");
  assert.deepEqual([noName.selStart, noName.selEnd], [3, 15]);
});

test("names split sensibly and company comes from a work domain only", () => {
  assert.deepEqual(nameParts({ name: "Raman, Priya", email: "p@x.example" }), { first: "Priya", last: "Raman", full: "Priya Raman" });
  assert.deepEqual(nameParts({ name: "Cher", email: "c@x.example" }), { first: "Cher", last: null, full: "Cher" });
  assert.deepEqual(nameParts({ name: "priya@x.example", email: "priya@x.example" }), { first: null, last: null, full: null });
  assert.equal(companyFromEmail("a@northwind.example"), "Northwind");
  assert.equal(companyFromEmail("a@mail.blue-harbor.example"), "Blue Harbor");
  assert.equal(companyFromEmail("a@acme.co.uk"), "Acme");
  assert.equal(companyFromEmail("someone@gmail.com"), null);
  assert.equal(companyFromEmail("someone@icloud.com"), null);
  assert.equal(companyFromEmail("nodomain"), null);
});

test("unfilled placeholders are found once each, braces in code are not all placeholders", () => {
  assert.deepEqual(unfilledPlaceholders("See you {day}. {day}? And {day before}."), ["{day}", "{day before}"]);
  assert.deepEqual(unfilledPlaceholders("const a = {};\nfn({ x: 1 })"), []);
  assert.deepEqual(unfilledPlaceholders("All done."), []);
});

const snip = (over: Partial<Snippet>): Snippet => ({ id: "s", trigger: "s", title: "S", body: "", uses: 0, subject: "", cc: [], bcc: [], attachments: [], ...over });

test("snippets match by trigger first, then name and text, most used first", () => {
  const list = [snip({ id: "a", trigger: "price", title: "Pricing", uses: 1 }), snip({ id: "b", trigger: "thx", title: "Thanks", body: "pricing attached", uses: 9 }), snip({ id: "c", trigger: "pr", title: "PR review", uses: 3 })];
  assert.deepEqual(matchSnippets(list, "pr").map((s) => s.id), ["c", "a", "b"]);
  assert.deepEqual(matchSnippets(list, "").map((s) => s.id), ["b", "c", "a"]);
  assert.deepEqual(matchSnippets(list, "thanks").map((s) => s.id), ["b"]);
});

test("a snippet's subject, Cc and Bcc are added once, without duplicates or bad addresses", () => {
  const parse = (raw: string): Address | null => {
    const m = /^(?:(.*?)\s*<([^>]+)>|([^\s<>]+@[^\s<>]+))$/.exec(raw.trim());
    return m ? { name: m[1] ?? null, email: (m[2] ?? m[3]).toLowerCase() } : null;
  };
  const s = snip({ subject: "Pricing", cc: ["Priya <priya@mail.northwind.example>", "Ops <ops@northwind.example>", "not an address"], bcc: ["ops@northwind.example", "crm@harbor.example"] });
  const x = snippetExtras(s, { subject: "", to: [priya], cc: [], bcc: [] }, parse);
  assert.equal(x.subject, "Pricing");
  assert.deepEqual(x.cc.map((a) => a.email), ["ops@northwind.example"]);
  assert.deepEqual(x.bcc.map((a) => a.email), ["crm@harbor.example"]);
  const kept = snippetExtras(s, { subject: "Re: quote", to: [], cc: [], bcc: [] }, parse);
  assert.equal(kept.subject, null);
});

// ---------------------------------------------------------------------------
// Instant replies
// ---------------------------------------------------------------------------

const ir = (over: Partial<InstantReplies> = {}): InstantReplies => ({ enabled: true, replies: ["Sounds good, thanks!", "Thanks, got it."], aiSuggestions: false, ...over });

test("instant choices: mine first, then the model's without repeats, nine at most", () => {
  assert.deepEqual(instantChoices(ir(), ["ignored"]).map((c) => [c.n, c.text, c.ai]), [
    [1, "Sounds good, thanks!", false],
    [2, "Thanks, got it.", false],
  ]);
  const both = instantChoices(ir({ aiSuggestions: true }), ["Thanks, got it", "Yes, Thursday works.", "  "]);
  assert.deepEqual(both.map((c) => [c.n, c.text, c.ai]), [
    [1, "Sounds good, thanks!", false],
    [2, "Thanks, got it.", false],
    [3, "Yes, Thursday works.", true],
  ]);
  assert.equal(instantChoices(ir({ replies: Array.from({ length: 12 }, (_, i) => `R${i}`) })).length, 9);
  assert.deepEqual(instantChoices(ir({ enabled: false, aiSuggestions: true }), ["x"]), []);
});

test("suggestions are asked for replies with nothing written, when on and available", () => {
  const on = ir({ aiSuggestions: true });
  assert.equal(wantsSuggestions(on, "reply", "", true), true);
  assert.equal(wantsSuggestions(on, "replyAll", "  \n", true), true);
  assert.equal(wantsSuggestions(on, "forward", "", true), false);
  assert.equal(wantsSuggestions(on, "reply", "Hi Dana", true), false);
  assert.equal(wantsSuggestions(on, "reply", "", false), false);
  assert.equal(wantsSuggestions(ir(), "reply", "", true), false);
});

test("a compose intent is for the composer that opens next, and only briefly", () => {
  setComposeIntent({ text: "Thanks!" });
  const t0 = Date.now();
  assert.deepEqual(takeComposeIntent(t0), { text: "Thanks!", ai: undefined });
  // React may render a new component twice: the same intent again, briefly.
  assert.deepEqual(takeComposeIntent(t0 + 200), { text: "Thanks!", ai: undefined });
  assert.equal(takeComposeIntent(t0 + 1500), null);
  assert.equal(takeComposeIntent(t0 + 1600), null);
  setComposeIntent({ ai: true });
  assert.equal(takeComposeIntent(Date.now() + 6000), null);
});

// ---------------------------------------------------------------------------
// Write with AI
// ---------------------------------------------------------------------------

const req = (over: Partial<Parameters<typeof buildRequest>[0]>) =>
  buildRequest({ runId: "w1", accountId: "sam@northwind.example", threadId: "t1", preset: null, instruction: "", scope: "writing", text: "", subject: "Re: Q4", recipients: ["Priya Raman"], myName: "Sam Okafor", ...over });

test("the bar's request: presets rewrite, instructions draft or edit", () => {
  assert.equal(req({ instruction: "yes to Thursday, ask for the agenda" }).action, "draft");
  assert.equal(req({ preset: "shorter", text: "A long message.", instruction: "ignored" }).action, "shorter");
  assert.equal(req({ preset: "shorter", text: "A long message.", instruction: "ignored" }).instruction, "");
  assert.equal(req({ scope: "selection", text: "this bit", instruction: "warmer" }).action, "custom");
  assert.equal(req({ text: "Draft so far", instruction: "make it warmer" }).action, "custom");
  assert.equal(req({ text: "Draft so far", instruction: "mention the budget too" }).action, "draft");
  assert.equal(req({ instruction: "x".repeat(900) }).instruction.length, 500);
  assert.equal(req({ text: "y".repeat(20_000), preset: "grammar" }).text.length, 12_000);
  assert.ok(isEditInstruction("Translate to Spanish"));
  assert.ok(!isEditInstruction("say yes"));
  assert.match(promptPlaceholder("writing", false, true), /reply/);
  assert.match(promptPlaceholder("selection", true, true), /selection/);
});

test("the unavailable reasons read as writing, not summaries", () => {
  for (const r of ["deviceNotEligible", "appleIntelligenceNotEnabled", "modelNotReady", "osTooOld", "unsupportedPlatform", "notBuilt", "unknown"] as const) {
    const t = writerUnavailableText(r);
    assert.ok(t.length > 10 && !/summar/i.test(t), r);
  }
  assert.match(writerUnavailableText("appleIntelligenceNotEnabled"), /System Settings/);
});

const schema = composerSchema();
const para = (text?: string) => schema.nodes.paragraph.create(null, text ? schema.text(text) : null);
const sig = () => schema.nodes.signature.create(null, [para("-- "), para("Sam")]);

test("Write with AI works on the selection, else everything above the signature", () => {
  const doc = schema.nodes.doc.create(null, [para("Hi Priya,"), para(), para("Thursday works."), para(), sig()]);
  const w = writingRange(doc);
  assert.equal(rangeText(doc, w.from, w.to), "Hi Priya,\n\nThursday works.");
  const whole = aiTargetOf(EditorState.create({ doc }));
  assert.equal(whole.scope, "writing");
  assert.equal(whole.text, "Hi Priya,\n\nThursday works.");
  // A selection that runs into the signature is clipped to the writing.
  const from = doc.content.size - sig().nodeSize - para("Thursday works.").nodeSize - para().nodeSize + 1;
  const state = EditorState.create({ doc, selection: TextSelection.create(doc, from, doc.content.size - 2) });
  const sel = aiTargetOf(state);
  assert.equal(sel.scope, "selection");
  assert.equal(sel.text, "Thursday works.");
  assert.ok(sel.to <= w.to);
});

test("suggested text becomes paragraphs, or inline text inside one", () => {
  assert.deepEqual(textContent("One line", true), [{ type: "text", text: "One line" }]);
  assert.deepEqual(textContent("Hi,\n\nThanks", false), [
    { type: "paragraph", content: [{ type: "text", text: "Hi," }] },
    { type: "paragraph" },
    { type: "paragraph", content: [{ type: "text", text: "Thanks" }] },
  ]);
  assert.equal(textContent("Two\nlines", true).length, 2);
});

// ---------------------------------------------------------------------------
// Send later
// ---------------------------------------------------------------------------

test("send-later presets follow the clock and the morning hour", () => {
  const wed = new Date(2026, 8, 23, 10, 7); // Wed 10:07
  const p = sendLaterPresets(wed, 9);
  assert.deepEqual(p.map((x) => x.id), ["hour", "today", "tomorrow", "tomorrowAfternoon", "monday"]);
  assert.equal(new Date(p[0].at).getHours(), 11);
  assert.equal(new Date(p[0].at).getMinutes(), 10);
  assert.equal(new Date(p[2].at).getHours(), 9);
  assert.equal(new Date(p[4].at).getDay(), 1);
  // Late afternoon: no "This afternoon". Sunday: Monday is tomorrow, so no "Monday morning".
  assert.ok(!sendLaterPresets(new Date(2026, 8, 23, 17, 0)).some((x) => x.id === "today"));
  assert.ok(!sendLaterPresets(new Date(2026, 8, 27, 9, 0)).some((x) => x.id === "monday"));
});

test("send times typed in words", () => {
  const now = new Date(2026, 8, 23, 10, 7); // Wed Sep 23, 10:07
  const at = (s: string) => {
    const ms = parseWhen(s, now, 8);
    return ms === null ? null : new Date(ms);
  };
  const is = (d: Date | null, y: number, mo: number, day: number, h: number, mi = 0) =>
    assert.deepEqual(d && [d.getFullYear(), d.getMonth(), d.getDate(), d.getHours(), d.getMinutes()], [y, mo, day, h, mi]);
  is(at("tomorrow 9am"), 2026, 8, 24, 9);
  is(at("tomorrow"), 2026, 8, 24, 8);
  is(at("tomorrow afternoon"), 2026, 8, 24, 13);
  is(at("fri 3pm"), 2026, 8, 25, 15);
  is(at("Friday at 3:30 pm"), 2026, 8, 25, 15, 30);
  is(at("wed 9"), 2026, 8, 30, 9); // the next Wednesday, not today
  is(at("monday"), 2026, 8, 28, 8);
  is(at("next week"), 2026, 8, 28, 8);
  is(at("3pm"), 2026, 8, 23, 15);
  is(at("9am"), 2026, 8, 24, 9); // already past today
  is(at("at 3"), 2026, 8, 23, 15);
  is(at("tonight"), 2026, 8, 23, 21);
  is(at("oct 3 10:30"), 2026, 9, 3, 10, 30);
  is(at("3 october"), 2026, 9, 3, 8);
  is(at("9/1"), 2027, 8, 1, 8); // past this year: next year
  is(at("in 2 hours"), 2026, 8, 23, 12, 7);
  is(at("in 30 min"), 2026, 8, 23, 10, 37);
  is(at("in a day"), 2026, 8, 24, 10, 7);
  assert.equal(parseWhen("", now), null);
  assert.equal(parseWhen("whenever", now), null);
  assert.equal(parseWhen("feb 30", now), null);
  assert.equal(parseWhen("25pm", now), null);
});
