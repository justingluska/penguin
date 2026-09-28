// Questions read as queries (features/search/askQuery.ts): the "Understood
// as" chips, their edits, grouped rows, and when the on-device model is asked.
import { test } from "node:test";
import assert from "node:assert/strict";
import { groupValue, queryChips, sourceNote, wantsModel, withOp, withSubject, withoutPart } from "../src/features/search/askQueryModel.ts";
import type { AskAnswer, AskGroup, AskQuery, AskUnderstood } from "../src/lib/types.ts";

const q = (p: Partial<AskQuery>): AskQuery => ({
  subject: "flights",
  op: "count",
  measure: "items",
  groupBy: null,
  timeframe: null,
  compare: [],
  place: null,
  merchant: null,
  person: null,
  direction: null,
  field: null,
  tense: "any",
  ...p,
});
const u = (query: AskQuery, rangeLabel: string | null = null): AskUnderstood => ({ query, source: "grammar", summary: "", rangeLabel, compareLabels: [] });

test("chips read like the question: what, where, when, how", () => {
  const chips = queryChips(u(q({ place: "Lisbon", direction: "to", timeframe: "in august" }), "Aug 1 – Aug 31, 2026"));
  assert.deepEqual(
    chips.map((c) => [c.key, c.label, c.removable]),
    [
      ["subject", "Flights", false],
      ["place", "to Lisbon", true],
      ["timeframe", "Aug 1 – Aug 31, 2026", true],
      ["op", "How many", false],
    ],
  );
  const money = queryChips(u(q({ subject: "spending", op: "max", measure: "money", groupBy: "month", merchant: "Swiftcab" })));
  assert.deepEqual(
    money.map((c) => c.label),
    ["Spending", "at Swiftcab", "Most", "Money", "By month"],
  );
  // A total of money doesn't repeat "Money".
  assert.ok(!queryChips(u(q({ subject: "spending", op: "sum", measure: "money" }))).some((c) => c.key === "measure"));
});

test("removing and changing parts keeps the query valid", () => {
  const base = q({ place: "Lisbon", direction: "to", timeframe: "2025", groupBy: "month" });
  assert.equal(withoutPart(base, "place").place, null);
  assert.equal(withoutPart(base, "place").direction, null);
  assert.equal(withoutPart(base, "timeframe").timeframe, null);
  assert.equal(withoutPart(base, "groupBy").groupBy, null);
  // Nights belong to stays; money doesn't apply to packages.
  assert.equal(withSubject(q({ subject: "stays", measure: "nights" }), "flights").measure, "items");
  assert.equal(withSubject(q({ subject: "orders", measure: "money" }), "parcels").measure, "items");
  assert.equal(withSubject(q({ subject: "messages", person: "Priya" }), "orders").person, null);
  // A total is of money, a count of items.
  assert.equal(withOp(q({ subject: "orders" }), "sum").measure, "money");
  assert.equal(withOp(q({ subject: "orders", measure: "money" }), "count").measure, "items");
  assert.equal(withOp(q({ subject: "parcels" }), "sum").measure, "items");
});

test("group values and where a reading came from", () => {
  const g: AskGroup = { label: "August 2026", count: 4, totals: [{ value: 42.5, currency: "USD", count: 4 }], nights: 7, cites: [], start: null, best: true };
  assert.equal(groupValue(g, "items").value, 4);
  assert.equal(groupValue(g, "money").value, 42.5);
  assert.match(groupValue(g, "money").text, /42\.50/);
  assert.equal(groupValue(g, "nights").text, "7 nights");
  assert.match(sourceNote({ ...u(q({})), source: "model" }), /Apple Intelligence/);
});

test("only answers the grammar didn't read exactly go to the model", () => {
  const a = (p: Partial<AskAnswer>) => ({ intent: "passage", confidence: "low", understood: null, ...p }) as AskAnswer;
  assert.equal(wantsModel(a({})), true);
  assert.equal(wantsModel(a({ intent: "unknown" })), true);
  assert.equal(wantsModel(a({ intent: "flight" })), false);
  assert.equal(wantsModel(a({ intent: "query", understood: u(q({})) })), false);
  assert.equal(wantsModel(a({ intent: "when", confidence: "none" })), true);
  assert.equal(wantsModel(a({ intent: "when", confidence: "high" })), false);
});
