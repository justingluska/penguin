// Smart views (src/features/smart): what the sidebar shows for a given
// Settings.smartViews, the edits Settings, the sidebar and search make
// (toggle, count, reorder, pin, rename, remove), how a row reads, the mock's
// settings normalization (mirrors settings.rs) and the Settings search.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SMART_VIEW_DEFS,
  customKey,
  fileKindOf,
  filesView,
  inboxLikeView,
  isPinned,
  moveView,
  moveViewBy,
  nameForQuery,
  pinSearch,
  removeCustom,
  renameCustom,
  setCount,
  setShown,
  sidebarKeyOf,
  sidebarSmartItems,
  smartDef,
  smartTitle,
} from "../src/features/smart/catalog.ts";
import { fmtAmounts, relDays, smartLine } from "../src/features/smart/format.ts";
import { normalizeSmartViews } from "../src/lib/mock/smartSettings.ts";
import { searchSettings } from "../src/features/settings/catalog.ts";
import { SMART_VIEWS, type SmartRow, type SmartViewSettings } from "../src/lib/types.ts";

const OFF: SmartViewSettings = { shown: [], counts: [], custom: [] };
const row = (r: Partial<SmartRow> & { kind: string; title: string }): SmartRow => ({
  amount: null,
  at: null,
  end: null,
  reference: null,
  status: null,
  detail: null,
  group: null,
  ...r,
});
// Sunday, Sep 27, 2026, 3 PM local.
const NOW = new Date(2026, 8, 27, 15, 0).getTime();

test("every built-in view has a definition, in the backend's order", () => {
  assert.deepEqual(
    SMART_VIEW_DEFS.map((d) => d.id),
    [...SMART_VIEWS],
  );
  for (const d of SMART_VIEW_DEFS) {
    assert.ok(d.label && d.blurb && d.empty.title && d.empty.body && d.counts, d.id);
  }
  // Only the views over inbox mail archive rows out, like the inbox.
  assert.deepEqual(
    SMART_VIEW_DEFS.filter((d) => d.inboxLike).map((d) => d.id),
    ["newsletters", "people"],
  );
  assert.ok(inboxLikeView({ kind: "smart", labelId: "newsletters" }));
  assert.ok(!inboxLikeView({ kind: "smart", labelId: "receipts" }));
  assert.ok(!inboxLikeView({ kind: "inbox" }));
});

test("the sidebar is empty until a view is turned on, then follows the saved order", () => {
  assert.deepEqual(sidebarSmartItems(OFF), []);
  let s = setShown(OFF, "bills", true);
  s = setShown(s, "receipts", true);
  s = setShown(s, "travel", true);
  // Turning one on puts it in the sidebar at once, last.
  assert.deepEqual(
    sidebarSmartItems(s).map((i) => i.label),
    ["Bills", "Receipts", "Travel"],
  );
  const bills = sidebarSmartItems(s)[0];
  assert.deepEqual(bills.view, { kind: "smart", labelId: "bills" });
  assert.equal(bills.icon, "bill");
  assert.equal(bills.count, false);
  assert.equal(bills.countId, "bills");
  // Turning it on twice changes nothing; off takes it out.
  assert.equal(setShown(s, "bills", true), s);
  assert.deepEqual(
    sidebarSmartItems(setShown(s, "receipts", false)).map((i) => i.key),
    ["bills", "travel"],
  );
  // Unknown ids in a hand-edited file are skipped.
  assert.deepEqual(sidebarSmartItems({ ...s, shown: ["horoscopes", ...s.shown] }).length, 3);
});

test("counts are off by default and toggle per view", () => {
  let s = setShown(OFF, "packages", true);
  assert.equal(sidebarSmartItems(s)[0].count, false);
  s = setCount(s, "packages", true);
  assert.equal(sidebarSmartItems(s)[0].count, true);
  assert.equal(setCount(s, "packages", true), s);
  assert.deepEqual(setCount(s, "packages", false).counts, []);
});

test("views reorder by drag index and by one step", () => {
  const s: SmartViewSettings = { ...OFF, shown: ["receipts", "travel", "packages", "bills"] };
  assert.deepEqual(moveView(s, "bills", 0).shown, ["bills", "receipts", "travel", "packages"]);
  assert.deepEqual(moveView(s, "receipts", 3).shown, ["travel", "packages", "bills", "receipts"]);
  assert.deepEqual(moveView(s, "receipts", 99).shown, ["travel", "packages", "bills", "receipts"]);
  assert.equal(moveView(s, "receipts", 0), s);
  assert.equal(moveView(s, "files", 1), s);
  assert.deepEqual(moveViewBy(s, "travel", -1).shown, ["travel", "receipts", "packages", "bills"]);
  assert.deepEqual(moveViewBy(s, "travel", 1).shown, ["receipts", "packages", "travel", "bills"]);
  assert.equal(moveViewBy(s, "receipts", -1), s);
});

test("a saved search pins to the sidebar once, renames, and unpins", () => {
  const seq = [0.1, 0.2, 0.3];
  const rand = () => seq.shift() ?? 0.9;
  const r = pinSearch(OFF, "  from:priya   has:attachment ", rand)!;
  assert.ok(r);
  const c = r.settings.custom[0];
  assert.deepEqual(c, { id: r.id, name: "from:priya has:attachment", query: "from:priya has:attachment" });
  assert.deepEqual(r.settings.shown, [customKey(r.id)]);
  assert.ok(isPinned(r.settings, "from:priya  has:attachment"));
  // Pinning the same search again returns it unchanged.
  const again = pinSearch(r.settings, "from:priya has:attachment", rand)!;
  assert.equal(again.settings, r.settings);
  assert.equal(again.id, r.id);
  assert.equal(pinSearch(OFF, "   "), null);

  const [item] = sidebarSmartItems(r.settings);
  assert.deepEqual(item.view, { kind: "query", labelId: "from:priya has:attachment" });
  assert.equal(item.countId, "query:from:priya has:attachment");
  assert.equal(item.icon, "search");
  assert.equal(sidebarKeyOf(item.view, r.settings), customKey(r.id));
  assert.equal(smartTitle(item.view, r.settings), "from:priya has:attachment");

  const renamed = renameCustom(r.settings, r.id, "  Files   from Priya ");
  assert.equal(renamed.custom[0].name, "Files from Priya");
  assert.equal(sidebarSmartItems(renamed)[0].label, "Files from Priya");
  assert.equal(smartTitle(item.view, renamed), "Files from Priya");
  // A blank name falls back to the query.
  assert.equal(renameCustom(renamed, r.id, "  ").custom[0].name, "from:priya has:attachment");

  const counted = setCount(renamed, customKey(r.id), true);
  const gone = removeCustom(counted, r.id);
  assert.deepEqual(gone, OFF);
  assert.ok(!isPinned(gone, "from:priya has:attachment"));
  assert.equal(nameForQuery("x".repeat(60)).length, 40);
});

test("Files keeps one sidebar row across its type filters", () => {
  const s = setShown(OFF, "files", true);
  const pdf = filesView("pdf");
  assert.deepEqual(pdf, { kind: "smart", labelId: "files:pdf" });
  assert.equal(sidebarKeyOf(pdf, s), "files");
  assert.equal(fileKindOf(pdf), "pdf");
  assert.equal(fileKindOf(filesView(null)), null);
  assert.equal(fileKindOf({ kind: "smart", labelId: "files:exe" }), null);
  assert.equal(smartDef("files:pdf")?.label, "Files");
  assert.equal(smartTitle(pdf, s), "Files");
  assert.equal(sidebarKeyOf({ kind: "inbox" }, s), null);
});

test("rows read as the fact: merchant and amount, route and date, due and overdue", () => {
  const receipt = smartLine(row({ kind: "receipt", title: "Paperleaf", amount: { value: 64, currency: "USD" }, reference: "PL-2291", detail: "Linen notebook" }), NOW)!;
  assert.equal(receipt.title, "Paperleaf");
  assert.deepEqual(receipt.facts, ["Linen notebook", "#PL-2291"]);
  assert.match(receipt.amount!, /64\.00/);
  assert.equal(receipt.chip, null);
  const refund = smartLine(row({ kind: "receipt", title: "Paperleaf", amount: { value: -12, currency: "USD" }, status: "refunded" }), NOW)!;
  assert.ok(refund.credit);
  assert.equal(refund.chip?.label, "Refund");

  const flight = smartLine(row({ kind: "flight", title: "SFO → LIS", at: "2026-10-02T19:05", reference: "QX7P2K", detail: "NL 238", status: "upcoming" }), NOW)!;
  assert.equal(flight.facts[1], "NL 238");
  assert.equal(flight.facts[2], "QX7P2K");
  assert.match(flight.facts[0], /Oct 2/);
  assert.deepEqual(flight.chip, { label: "In 5 days", tone: "blue" });
  assert.equal(smartLine(row({ kind: "flight", title: "x", at: "2026-10-02", status: "cancelled" }), NOW)!.chip?.label, "Cancelled");

  const overdue = smartLine(row({ kind: "bill", title: "Ledgerly", at: "2026-09-24", status: "overdue", reference: "INV-311" }), NOW)!;
  assert.deepEqual(overdue.chip, { label: "Overdue · 3 days ago", tone: "red" });
  assert.deepEqual(overdue.facts, ["Invoice INV-311"]);
  const due = smartLine(row({ kind: "bill", title: "Brightwave", at: "2026-09-29", status: "due" }), NOW)!;
  assert.deepEqual(due.chip, { label: "Due in 2 days", tone: "amber" });
  assert.equal(smartLine(row({ kind: "bill", title: "Aquafon", status: "paid" }), NOW)!.chip?.label, "Paid");

  const parcel = smartLine(row({ kind: "parcel", title: "Hearth & Loom", status: "outForDelivery", at: "2026-09-27", detail: "UPS" }), NOW)!;
  assert.deepEqual(parcel.chip, { label: "Out for delivery", tone: "amber" });
  assert.equal(parcel.facts[0], "UPS");
  const delivered = smartLine(row({ kind: "parcel", title: "Lumen Books", status: "delivered", at: "2026-09-25" }), NOW)!;
  assert.ok(!delivered.facts.some((f) => f.startsWith("Expected")));

  const sub = smartLine(row({ kind: "subscription", title: "Tunely", amount: { value: 9.99, currency: "USD" }, at: "2026-09-12", end: "2026-10-12", status: "active", detail: "Monthly · 4 charges" }), NOW)!;
  assert.equal(sub.facts[0], "Monthly · 4 charges");
  assert.equal(sub.chip?.label, "Active");
  // Invites and codes keep their own chips.
  assert.equal(smartLine(row({ kind: "invite", title: "" }), NOW), null);
  assert.equal(smartLine(row({ kind: "code", title: "" }), NOW), null);
});

test("relative days and per-currency totals", () => {
  assert.equal(relDays(0), "Today");
  assert.equal(relDays(1), "Tomorrow");
  assert.equal(relDays(-1), "Yesterday");
  assert.equal(relDays(5), "In 5 days");
  assert.equal(relDays(21), "In 3 weeks");
  assert.equal(relDays(-3), "3 days ago");
  const t = fmtAmounts([
    { value: 96.5, currency: "USD" },
    { value: 30, currency: "EUR" },
  ]);
  assert.ok(t.includes("96.50") && t.includes("30.00") && t.includes(" · "), t);
});

test("the mock normalizes Settings.smartViews like settings.rs", () => {
  const v = normalizeSmartViews({
    shown: ["bills", "receipts", "horoscopes", "bills", "custom:gone"],
    counts: ["bills", "nope", "custom:vip"],
    custom: [
      { id: "vip", name: "  From   the board ", query: " from:board@linden.example   is:unread " },
      { id: "vip", name: "duplicate", query: "x" },
      { id: "bad id!", name: "x", query: "x" },
      { id: "empty", name: "x", query: "   " },
      { id: "unnamed", name: "", query: "has:pdf" },
    ],
  });
  assert.deepEqual(v.shown, ["bills", "receipts", "custom:vip", "custom:unnamed"]);
  assert.deepEqual(v.counts, ["bills", "custom:vip"]);
  assert.deepEqual(v.custom, [
    { id: "vip", name: "From the board", query: "from:board@linden.example is:unread" },
    { id: "unnamed", name: "has:pdf", query: "has:pdf" },
  ]);
  assert.deepEqual(normalizeSmartViews(null), OFF);
});

test("Settings search finds the views page and each view", () => {
  const hits = (q: string) => searchSettings(q).flatMap((g) => g.hits.map((h) => `${g.page.id}:${h.entry?.label}`));
  assert.ok(hits("receipts").includes("views:Receipts"));
  assert.ok(hits("flights").includes("views:Travel"));
  assert.ok(hits("tracking").includes("views:Packages"));
  assert.ok(hits("pin search").includes("views:Pinned searches"));
  assert.ok(searchSettings("smart views").some((g) => g.page.id === "views"));
});
