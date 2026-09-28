// The optimistic layer's pure parts (src/app/optimistic.ts): unread-count
// deltas, structural sharing of refreshed rows, and the pending-change log
// that keeps a racing read from flashing old state back.
import { test } from "node:test";
import assert from "node:assert/strict";
import { addDeltas, applyCounts, countDeltas, createOpLog, sameLabelLook, sameValue, shareRows, unreadLabels } from "../src/app/optimistic.ts";

const A = "a@x.example";
const B = "b@x.example";
const row = (threadId: string, unread: boolean, labelIds: string[], accountId = A) => ({ accountId, threadId, unread, labelIds, lastDate: 1 });
const label = (id: string, unreadCount: number | null, accountId = A) => ({ accountId, id, unreadCount });
const asObj = (m: Map<string, number>) => Object.fromEntries([...m].map(([k, v]) => [k.replace("\u0000", "/"), v]));

test("an unread thread counts toward its labels, not UNREAD; a trashed one only toward Trash", () => {
  assert.deepEqual(unreadLabels(row("t", true, ["INBOX", "UNREAD", "IMPORTANT"])), ["INBOX", "IMPORTANT"]);
  assert.deepEqual(unreadLabels(row("t", false, ["INBOX"])), []);
  assert.deepEqual(unreadLabels(row("t", true, ["INBOX", "TRASH", "Label_1"])), ["TRASH"]);
});

test("marking read takes one off every label the thread carries", () => {
  const before = row("t", true, ["INBOX", "UNREAD", "Label_1"]);
  const after = { ...before, unread: false, labelIds: ["INBOX", "Label_1"] };
  assert.deepEqual(asObj(countDeltas([{ before, after }])), { [`${A}/INBOX`]: -1, [`${A}/Label_1`]: -1 });
});

test("archiving an unread thread moves Inbox only; a read thread moves nothing", () => {
  const unread = row("t", true, ["INBOX", "UNREAD"]);
  assert.deepEqual(asObj(countDeltas([{ before: unread, after: { ...unread, labelIds: ["UNREAD"] } }])), { [`${A}/INBOX`]: -1 });
  const read = row("u", false, ["INBOX"]);
  assert.equal(countDeltas([{ before: read, after: { ...read, labelIds: [] } }]).size, 0);
});

test("deltas are per account and cancel out", () => {
  const a = row("t", true, ["INBOX", "UNREAD"], A);
  const b = row("t", true, ["INBOX", "UNREAD"], B);
  const d = countDeltas([
    { before: a, after: { ...a, unread: false } },
    { before: b, after: { ...b, unread: false } },
  ]);
  assert.deepEqual(asObj(d), { [`${A}/INBOX`]: -1, [`${B}/INBOX`]: -1 });
  assert.equal(addDeltas(d, d, -1).size, 0);
});

test("applyCounts moves only the labels named, never below zero, and keeps untouched objects", () => {
  const labels = [label("INBOX", 3), label("Label_1", 0), label("STARRED", null)];
  const d = addDeltas(new Map(), new Map([[`${A}\u0000INBOX`, -1], [`${A}\u0000Label_1`, -2], [`${A}\u0000STARRED`, 1]]));
  const out = applyCounts(labels, d);
  assert.deepEqual(out.map((l) => l.unreadCount), [2, 0, null]);
  assert.equal(out[2], labels[2]);
  assert.equal(applyCounts(labels, new Map()), labels);
});

test("shareRows keeps unchanged rows' objects, and the array when nothing changed", () => {
  const prev = [row("t1", true, ["INBOX"]), row("t2", false, ["INBOX"])];
  const same = prev.map((r) => ({ ...r, labelIds: [...r.labelIds] }));
  assert.equal(shareRows(prev, same), prev);
  const changed = [{ ...prev[0], unread: false }, { ...prev[1] }];
  const out = shareRows(prev, changed);
  assert.notEqual(out, prev);
  assert.notEqual(out[0], prev[0]);
  assert.equal(out[1], prev[1]);
  // A new row on top: the rest are reused.
  const withNew = [row("t0", true, ["INBOX"]), ...same];
  const out2 = shareRows(prev, withNew);
  assert.equal(out2[1], prev[0]);
  assert.equal(out2[2], prev[1]);
});

test("sameValue compares plain data deeply", () => {
  assert.ok(sameValue({ a: [1, { b: "x" }], c: null }, { a: [1, { b: "x" }], c: null }));
  assert.ok(!sameValue({ a: [1] }, { a: [1, 2] }));
  assert.ok(!sameValue({ a: 1 }, { a: 1, b: undefined }));
  assert.ok(!sameValue([1], { 0: 1 }));
});

test("a read that started before a change settled gets it re-applied; later reads don't", () => {
  const log = createOpLog<string>();
  const early = log.readBegin();
  const op = log.add(["k"], "mark read", new Map());
  const during = log.readBegin();
  assert.deepEqual(log.pending(early).map((o) => o.patch), ["mark read"]);
  assert.deepEqual(log.pending(during).map((o) => o.patch), ["mark read"]);
  log.settle(op);
  const after = log.readBegin();
  assert.deepEqual(log.pending(early).map((o) => o.patch), ["mark read"]);
  assert.deepEqual(log.pending(after), []);
  // Kept while an earlier read is still out, dropped once none can miss it.
  log.readEnd(early);
  log.readEnd(during);
  assert.equal(log.size(), 0);
  log.readEnd(after);
});

test("a failed change is dropped: no read re-applies it", () => {
  const log = createOpLog<string>();
  const read = log.readBegin();
  const op = log.add(["k"], "star", new Map());
  log.drop(op);
  assert.deepEqual(log.pending(read), []);
  log.readEnd(read);
  assert.equal(log.size(), 0);
});

test("with no reads out, a settled change is forgotten at once", () => {
  const log = createOpLog<string>();
  const op = log.add(["k"], "archive", new Map());
  assert.equal(log.size(), 1);
  log.settle(op);
  assert.equal(log.size(), 0);
});

test("label look: a count change keeps the look (rows don't re-render), a rename or recolor changes it", () => {
  const full = (id: string, unreadCount: number | null, extra: Partial<{ name: string; color: string | null; hidden: boolean }> = {}) => ({
    accountId: A, id, name: id, kind: "user" as const, color: null as string | null, hidden: false, unreadCount, ...extra,
  });
  const before = [full("INBOX", 3), full("Work", 1)];
  // Marking a conversation read: new label objects, only counts differ.
  const counted = applyCounts(before, new Map([[A + "\u0000INBOX", -1]]));
  assert.notEqual(counted, before);
  assert.ok(sameLabelLook(before, counted));
  assert.ok(!sameLabelLook(before, [before[0], full("Work", 1, { name: "Clients" })]));
  assert.ok(!sameLabelLook(before, [before[0], full("Work", 1, { color: "#16a765" })]));
  assert.ok(!sameLabelLook(before, [before[0], full("Work", 1, { hidden: true })]));
  assert.ok(!sameLabelLook(before, [before[0]]), "a label removed");
  assert.ok(!sameLabelLook(before, [before[1], before[0]]), "reordered");
});
