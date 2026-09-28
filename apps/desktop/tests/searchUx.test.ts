// The search experience's pure parts: stable results (stable.ts), suggestions
// as you type (suggest.ts), "did you mean" (spelling.ts) and chips that remove
// the words behind them (nl.ts interpretDetailed / withoutPart).
import { test } from "node:test";
import assert from "node:assert/strict";
import { stabilize } from "../src/features/search/stable.ts";
import { suggestFor } from "../src/features/search/suggest.ts";
import { serverName } from "../src/features/search/serverName.ts";
import { correctWord, didYouMean, editDistance, forgetLexicon, isKnown, learn } from "../src/features/search/spelling.ts";
import { interpretDetailed, withoutPart } from "../src/features/search/nl.ts";
import type { Address, SearchHit } from "../src/lib/types.ts";

const GROUPS = ["ask", "top", "events", "attachments", "threads", "related", "server", "people"] as const;
const it = (id: string, group: string) => ({ id, group });
const ids = (xs: Array<{ id: string }>) => xs.map((x) => x.id);

test("stabilize: a new top result arriving later goes below the selected first row", () => {
  const shown = [it("a", "top"), it("b", "top"), it("t1", "threads"), it("t2", "threads")];
  const fresh = [it("n", "top"), it("a", "top"), it("b", "top"), it("t1", "threads"), it("t2", "threads")];
  const r = stabilize(shown, fresh, "a", GROUPS);
  assert.deepEqual(ids(r.items), ["a", "n", "b", "t1", "t2"]);
  assert.deepEqual([...r.entered], ["n"]);
  assert.equal(r.deferred, 0);
});

test("stabilize: nothing above or at a selection deep in the list moves", () => {
  const shown = [it("a", "top"), it("t1", "threads"), it("t2", "threads"), it("t3", "threads")];
  // Fresh ranking reorders, adds a top result and a newer thread above the selection.
  const fresh = [it("n", "top"), it("t2", "top"), it("a", "top"), it("t0", "threads"), it("t3", "threads"), it("t1", "threads"), it("r1", "related")];
  const r = stabilize(shown, fresh, "t2", GROUPS);
  assert.deepEqual(ids(r.items).slice(0, 3), ["a", "t1", "t2"], "rows up to the selection are untouched");
  assert.ok(!ids(r.items).includes("n"), "a new top result waits: its place is above the selection");
  assert.equal(r.deferred, 1);
  assert.deepEqual(ids(r.items), ["a", "t1", "t2", "t0", "t3", "r1"]);
  assert.equal(r.items.find((x) => x.id === "t2")!.group, "threads", "a shown row keeps its group");
});

test("stabilize: held-back rows appear once the selection moves up to them", () => {
  const shown = [it("a", "top"), it("t1", "threads"), it("t2", "threads")];
  const fresh = [it("a", "top"), it("n", "top"), it("t1", "threads"), it("t2", "threads")];
  assert.ok(!ids(stabilize(shown, fresh, "t2", GROUPS).items).includes("n"));
  assert.deepEqual(ids(stabilize(shown, fresh, "a", GROUPS).items), ["a", "n", "t1", "t2"]);
});

test("stabilize: rows that are gone are dropped; a vanished selection anchors on the row above it", () => {
  const shown = [it("a", "top"), it("t1", "threads"), it("t2", "threads"), it("t3", "threads")];
  const fresh = [it("a", "top"), it("n", "top"), it("t1", "threads"), it("t3", "threads")];
  const r = stabilize(shown, fresh, "t2", GROUPS);
  assert.deepEqual(ids(r.items), ["a", "t1", "t3"]);
  assert.equal(r.deferred, 1);
});

test("stabilize: nothing in common is simply the fresh list", () => {
  const r = stabilize([it("a", "top")], [it("b", "top"), it("c", "threads")], "a", GROUPS);
  assert.deepEqual(ids(r.items), ["b", "c"]);
});

// ---------------------------------------------------------------------------

const mike: Address = { name: "Mike Delgado", email: "mike@cedarpine.example" };
const osei: Address = { name: "Mike Osei", email: "mike@harborlabs.example" };
const priya: Address = { name: "Priya Natarajan", email: "priya@linden.example" };
const office: Address = { name: "Cedar & Pine Property", email: "office@cedarpine.example" };
const me: Address = { name: "Sam Okafor", email: "sam.okafor@gmail.example" };
const people = [mike, osei, priya, office, me].map((address, i) => ({ address, seen: 10 - i }));
const ctx = { people, recent: ["lease renewal", "from:priya has:pdf"], mine: [me.email] };

test("suggest: a name as you type offers the people it could be, most known first", () => {
  const s = suggestFor("mi", ctx);
  assert.deepEqual(
    s.filter((x) => x.kind === "person").map((x) => x.next),
    ["from:mike@cedarpine.example", "from:mike@harborlabs.example"],
  );
  assert.equal(s[0].address?.name, "Mike Delgado");
});

test("suggest: a plain-English lead-in picks the side and is replaced with the words", () => {
  assert.equal(suggestFor("invoice to pri", ctx)[0].next, "invoice to:priya@linden.example");
  assert.equal(suggestFor("lease from mike del", ctx)[0].next, "lease from:mike@cedarpine.example");
});

test("suggest: the last word of a phrase is a word, not a name, without a lead-in", () => {
  const ups: Address = { name: "UPS", email: "mcinfo@ups.example" };
  const theo: Address = { name: "Theo Laurent", email: "theo@laurent.example" };
  const c = { ...ctx, people: [...people, { address: ups, seen: 3 }, { address: theo, seen: 2 }] };
  assert.ok(!suggestFor("rent going up", c).some((x) => x.kind === "person"), "\"up\" isn't UPS");
  assert.ok(!suggestFor("fix the", c).some((x) => x.kind === "person"), "\"the\" isn't Theo");
  // A lone word, or one after a lead-in, still suggests.
  assert.ok(suggestFor("up", c).some((x) => x.label === "UPS"));
  assert.ok(suggestFor("rent from mi", c).some((x) => x.kind === "person"));
  assert.ok(suggestFor("invoice pri", c).some((x) => x.label === "Priya Natarajan"), "three letters of a name still suggest");
});

test("suggest: companies by domain, never free-mail providers or your own", () => {
  const s = suggestFor("cedar", ctx);
  assert.ok(s.some((x) => x.kind === "domain" && x.next === "domain:cedarpine.example"));
  assert.ok(!suggestFor("gmail", ctx).some((x) => x.kind === "domain"));
  assert.ok(!suggestFor("sam", ctx).some((x) => x.kind === "person"), "you aren't suggested to yourself");
});

test("suggest: recent searches that start the same way", () => {
  assert.deepEqual(
    suggestFor("lea", ctx).filter((x) => x.kind === "recent").map((x) => x.next),
    ["lease renewal"],
  );
});

test("suggest: nothing while an operator value is being typed (QueryInput lists those)", () => {
  assert.deepEqual(suggestFor("from:mi", ctx), []);
});

const hit = (from: Address, labelIds: string[], hasAttachments: boolean, n: number): SearchHit => ({
  accountId: "a",
  threadId: `t${n}`,
  messageId: `m${n}`,
  subject: "s",
  from,
  date: n,
  snippetHtml: "",
  matchCount: 1,
  labelIds,
  hasAttachments,
  unread: false,
  score: 1,
  matchedBy: ["words"],
});

test("suggest: refinements come from the results on screen, like Gmail's chips", () => {
  const hits = [hit(mike, ["INBOX"], true, 1), hit(mike, [], false, 2), hit(priya, [], false, 3)];
  const s = suggestFor("lease ", { ...ctx, hits });
  assert.deepEqual(
    s.filter((x) => x.kind === "refine").map((x) => [x.label, x.next]),
    [
      ["From Mike Delgado", "lease from:mike@cedarpine.example"],
      ["With attachments", "lease has:attachment"],
      ["In Inbox", "lease in:inbox"],
    ],
  );
  assert.ok(!suggestFor("lease in:inbox ", { ...ctx, hits }).some((x) => x.key === "in:inbox"), "not offered twice");
});

// ---------------------------------------------------------------------------

test("spelling: edit distance counts an adjacent swap as one", () => {
  assert.equal(editDistance("lesae", "lease"), 1);
  assert.equal(editDistance("leese", "lease"), 1);
  assert.equal(editDistance("invioce", "invoice"), 1);
  assert.equal(editDistance("kitten", "sitting"), 3);
});

test("spelling: corrects words no mail contains, leaves known words and prefixes alone", () => {
  forgetLexicon();
  learn("Lease renewal — 418 Alder St, Unit 3B");
  learn("Invoice 0932 — brand refresh phase 2");
  learn("Your lease is up for renewal on June 1.");
  assert.equal(correctWord("leese"), "lease");
  assert.equal(correctWord("renewel"), "renewal");
  assert.equal(correctWord("invoce"), "invoice");
  assert.equal(correctWord("lease"), null);
  assert.ok(isKnown("rene"), "a prefix of a known word is known (typing in progress)");
  assert.equal(correctWord("zzzzzz"), null);
  assert.equal(didYouMean("leese renewel from:mike"), "lease renewal from:mike");
  assert.equal(didYouMean("lease renewal"), null);
});

// ---------------------------------------------------------------------------

test("nl: each chip knows the words it came from, so removing it removes them", () => {
  const input = "from mike last week pdf";
  const r = interpretDetailed(input, { now: new Date(2026, 8, 24), isPerson: (w) => w === "mike" })!;
  assert.equal(r.query, 'from:mike date:"last week" has:pdf');
  assert.equal(withoutPart(input, r, 'date:"last week"'), "from mike pdf");
  assert.equal(withoutPart(input, r, "from:mike"), "last week pdf");
  assert.equal(withoutPart(input, r, "has:pdf"), "from mike last week");
  assert.equal(withoutPart(input, r, "is:unread"), null);
});

test("nl: typed operators pass through with their own span", () => {
  const input = "unread has:pdf";
  const r = interpretDetailed(input)!;
  assert.equal(r.query, "is:unread has:pdf");
  assert.equal(withoutPart(input, r, "has:pdf"), "unread");
  assert.equal(withoutPart(input, r, "is:unread"), "has:pdf");
});

test("server search is named for the accounts in scope", () => {
  assert.equal(serverName(["gmail", "gmail"]), "Gmail");
  assert.equal(serverName(["microsoft"]), "Outlook");
  assert.equal(serverName(["imap"]), "your mail server");
  assert.equal(serverName(["gmail", "imap"]), "the server");
});
