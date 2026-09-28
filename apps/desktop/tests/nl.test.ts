// Natural-language search (src/features/search/nl.ts): phrase → operator query.
import { test } from "node:test";
import assert from "node:assert/strict";
import { interpret } from "../src/features/search/nl.ts";
import { isStructured, tokenize } from "../src/features/search/query.ts";
import { parseDatePhrase } from "../src/features/search/dates.ts";

const NOW = new Date(2026, 8, 24, 10, 0); // Thu Sep 24, 2026
const known = new Set(["nick", "priya", "dana"]);
const nl = (q: string) => interpret(q, { now: NOW, isPerson: (w) => known.has(w) });

const REWRITES: Array<[string, string]> = [
  // The examples from the brief.
  ["new senders last week", 'is:new-sender date:"last week"'],
  ["emails I sent on august 27", 'in:sent date:"august 27"'],
  ["unanswered from nick this month", 'is:unanswered from:nick date:"this month"'],
  ["pdfs from acme.example", "has:pdf from:acme.example"],
  ['date:"august 27" sent', 'date:"august 27" in:sent'],
  // New contacts, both directions.
  ["new senders", "is:new-sender"],
  ["show me new people this month", 'is:new-sender date:"this month"'],
  ["first time senders yesterday", "is:new-sender date:yesterday"],
  ["new outbound", "is:first-outbound"],
  ["new recipients this year", 'is:first-outbound date:"this year"'],
  ["people I emailed for the first time last month", 'is:first-outbound date:"last month"'],
  ["first emails I sent in march", 'is:first-outbound date:"in march"'],
  // Replies.
  ["emails I haven't replied to", "is:unanswered"],
  ["unanswered", "is:unanswered"],
  ["needs a reply from dana", "is:unanswered from:dana"],
  ["waiting for a reply", "is:awaiting"],
  ["threads with no reply yet from last week", 'is:awaiting date:"last week"'],
  ["awaiting reply to priya", "is:awaiting to:priya"],
  ["replied to nick", "is:replied nick"],
  // Sent.
  ["sent to nick", "in:sent to:nick"],
  ["emails to priya I sent yesterday", "to:priya in:sent date:yesterday"],
  ["i emailed ana about the lease", "in:sent to:ana lease"],
  ["what i sent last week", 'what in:sent date:"last week"'],
  ["sent emails with attachments", "in:sent has:attachment"],
  // Files.
  ["photos from mom", "has:image from:mom"],
  ["pdfs or spreadsheets from dana", "has:pdf OR has:spreadsheet from:dana"],
  ["invites from last month", 'has:invite date:"last month"'],
  ["contract pdf from nick", "contract has:pdf from:nick"],
  ["large attachments from dana", "larger:5M from:dana"],
  ["emails larger than 10mb", "larger:10M"],
  ["attachments over 2 mb this year", 'has:attachment larger:2M date:"this year"'],
  ["verification codes today", "has:otp date:today"],
  // States and places.
  ["unread newsletters", "is:unread is:newsletter"],
  ["starred invoices", "is:starred invoices"],
  ["snoozed", "is:snoozed"],
  ["drafts about the budget", "in:drafts budget"],
  ["long threads with ana", "messages:>5 with:ana"],
  ["emails on weekends from nick", "day:weekend from:nick"],
  ["invoices on mondays", "invoices day:monday"],
  // Person + date is enough without a strong cue.
  ["mail from nick last week", 'from:nick date:"last week"'],
  ["lease from mike last spring", 'lease from:mike date:"last spring"'],
  // Operators already typed pass through; the words around them are read.
  ["from:mike unread", "from:mike is:unread"],
  ["label:clients new senders", "label:clients is:new-sender"],
  ["-newsletter unanswered", "-newsletter is:unanswered"],
  ["subject:(lease renewal) unread", "subject:(lease renewal) is:unread"],
  // A known person alone.
  ["from nick", "from:nick"],
  // Date forms added with the edge-case eval (docs/SEARCH-CASES.md).
  ["invoices I sent last quarter", 'invoices in:sent date:"last quarter"'],
  ["photos from nick last weekend", 'has:image from:nick date:"last weekend"'],
  ["unread from dana a couple of weeks ago", 'is:unread from:dana date:"a couple of weeks ago"'],
  ["pdfs from priya in q2", "has:pdf from:priya date:\"in q2\""],
  // Natural queries of the search-eval set (crates/penguin-eval), as the app runs them.
  ["photos from last summer", 'has:image date:"last summer"'],
  ["from:jess photos last summer", 'from:jess has:image date:"last summer"'],
  ["invoices I sent last year", 'invoices in:sent date:"last year"'],
];

for (const [q, want] of REWRITES) {
  test(`rewrites "${q}"`, () => {
    assert.equal(nl(q), want);
  });
}

const PLAIN: string[] = [
  "",
  "invoice",
  "lease renewal",
  "february",
  "last spring",
  "meeting notes from march",
  "from mike", // not a known correspondent
  "the pdf mike sent about the lease", // "mike sent": his mail, not yours
  "google docs",
  "promo codes",
  "we may need it",
  "INV-20417",
  "from:mike",
  'date:"last week"',
  "is:unanswered",
  "(from:ana OR from:bob) unread",
  "unread OR starred",
  "how to reply to a letter",
  // Pasted prose is content to find: "by May" is a date, not a person, and a
  // sentence break or a dozen words means text, not a command.
  "Monthly rent goes to $2,450 starting June 1. Please sign and send it back by May 15.",
  "i'm looking for the email from the landlord's agent with the lease renewal agreement that i had to sign and send back",
];

for (const q of PLAIN) {
  test(`leaves "${q}" alone`, () => {
    assert.equal(nl(q), null);
  });
}

test("the rewrite is itself stable (running it again changes nothing)", () => {
  for (const [q] of REWRITES) {
    const once = nl(q);
    assert.ok(once);
    assert.equal(nl(once!), null, `${q} → ${once}`);
  }
});

test("rewrites are plain operator queries the tokenizer reads as operators", () => {
  for (const [q] of REWRITES) {
    const out = nl(q)!;
    const ops = tokenize(out).filter((t) => t.kind === "op");
    assert.ok(ops.length > 0, out);
    assert.ok(isStructured(out));
  }
});

test("tokenizer: parentheses are their own tokens and cover the input", () => {
  const q = "-(from:ana OR from:bob) (lease) x)";
  const toks = tokenize(q);
  assert.equal(toks.map((t) => t.text).join(""), q);
  assert.deepEqual(
    toks.filter((t) => t.kind === "paren").map((t) => t.text),
    ["-(", ")", "(", ")"],
  );
  // A ")" with nothing open stays part of the word.
  assert.equal(toks[toks.length - 1].text, "x)");
  const op = toks.find((t) => t.kind === "op" && t.op === "from");
  assert.ok(op && op.kind === "op" && op.value === "ana");
});

test("date forms read like penguin-core's: day first, dotted, two-digit years, words, weekends, quarters", () => {
  const day = (y: number, m: number, dd: number) => ({ from: new Date(y, m, dd).getTime(), to: new Date(y, m, dd + 1).getTime() });
  assert.deepEqual(parseDatePhrase("18 august 2026", NOW), day(2026, 7, 18));
  assert.deepEqual(parseDatePhrase("18th of august", NOW), day(2026, 7, 18));
  assert.deepEqual(parseDatePhrase("18/08/2026", NOW), day(2026, 7, 18));
  assert.deepEqual(parseDatePhrase("18.08.2026", NOW), day(2026, 7, 18));
  assert.deepEqual(parseDatePhrase("05.03.2026", NOW), day(2026, 2, 5));
  assert.deepEqual(parseDatePhrase("05/03/2026", NOW), day(2026, 4, 3));
  assert.deepEqual(parseDatePhrase("8/18/26", NOW), day(2026, 7, 18));
  assert.deepEqual(parseDatePhrase("aug. 18", NOW), day(2026, 7, 18));
  assert.deepEqual(parseDatePhrase("two weeks ago", NOW), parseDatePhrase("2 weeks ago", NOW));
  assert.deepEqual(parseDatePhrase("a couple of weeks ago", NOW), parseDatePhrase("2 weeks ago", NOW));
  // Thu Sep 24, 2026: last weekend is Sep 19–20, this weekend Sep 26–27.
  assert.deepEqual(parseDatePhrase("last weekend", NOW), { from: new Date(2026, 8, 19).getTime(), to: new Date(2026, 8, 21).getTime() });
  assert.deepEqual(parseDatePhrase("this weekend", NOW), { from: new Date(2026, 8, 26).getTime(), to: new Date(2026, 8, 28).getTime() });
  assert.deepEqual(parseDatePhrase("last quarter", NOW), { from: new Date(2026, 3, 1).getTime(), to: new Date(2026, 6, 1).getTime() });
  assert.deepEqual(parseDatePhrase("q2", NOW), { from: new Date(2026, 3, 1).getTime(), to: new Date(2026, 6, 1).getTime() });
  assert.deepEqual(parseDatePhrase("q4", NOW), { from: new Date(2025, 9, 1).getTime(), to: new Date(2026, 0, 1).getTime() });
  assert.deepEqual(parseDatePhrase("q2 2025", NOW), { from: new Date(2025, 3, 1).getTime(), to: new Date(2025, 6, 1).getTime() });
  // Still not dates.
  for (const w of ["weekend", "a", "two", "quarter"]) assert.equal(parseDatePhrase(w, NOW), null, w);
});

test("date ranges with .. and glued month-day read like penguin-core's", () => {
  assert.deepEqual(parseDatePhrase("aug1..aug15", NOW), { from: new Date(2026, 7, 1).getTime(), to: new Date(2026, 7, 16).getTime() });
  assert.deepEqual(parseDatePhrase("aug27", NOW), { from: new Date(2026, 7, 27).getTime(), to: new Date(2026, 7, 28).getTime() });
});

test("tokenizer: aliases map to their operator", () => {
  const [folder] = tokenize("folder:inbox");
  assert.ok(folder.kind === "op" && folder.op === "in");
  const [p] = tokenize("participant:ana");
  assert.ok(p.kind === "op" && p.op === "with");
  // Other mail apps' spellings, as penguin-core reads them.
  for (const [q, op] of [
    ["deliveredto:me+shop@mail.example", "to"],
    ["participants:ana", "with"],
    ["attachment:roadmap", "filename"],
    ["hasattachment:yes", "has"],
  ] as const) {
    const [t] = tokenize(q);
    assert.ok(t.kind === "op" && t.op === op, q);
  }
});

test("tokenizer: Gmail braces, NOT/AROUND, smart quotes and operator groups read like penguin-core's", () => {
  const q = "-{from:ana from:bob} x}";
  const toks = tokenize(q);
  assert.equal(toks.map((t) => t.text).join(""), q);
  assert.deepEqual(
    toks.filter((t) => t.kind === "paren").map((t) => t.text),
    ["-{", "}"],
  );
  assert.equal(toks[toks.length - 1].text, "x}");
  for (const w of ["NOT", "AROUND", "AND", "OR"]) {
    assert.equal(tokenize(`rent ${w} june`)[2].kind, "or", w);
  }
  assert.equal(tokenize("rent not june")[2].kind, "word");
  const [phrase] = tokenize("“early termination” lease");
  assert.equal(phrase.kind, "phrase");
  assert.equal(phrase.text, "“early termination”");
  const [group] = tokenize("subject:(lease renewal) pdf");
  assert.ok(group.kind === "op" && group.op === "subject" && group.text === "subject:(lease renewal)");
});

// Logic and grouping are exact syntax: never reinterpreted as plain English.
for (const q of ["invoice NOT crestline", "{from:ana from:bob} unread", "rent AROUND 5 june"]) {
  test(`leaves structured "${q}" alone`, () => {
    assert.equal(nl(q), null);
  });
}
