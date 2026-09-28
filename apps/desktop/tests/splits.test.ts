// Split Inbox (src/app/splits.ts, src/features/split/presets.ts) and Get to
// zero (src/features/zero/zero.ts): which tabs show, what a tab's list asks
// the backend for, moving between tabs, the presets, what a Get to zero run
// archives, and the mock's query matcher (src/lib/mock/split.ts), which
// stands in for penguin-core store_split.rs in mock mode.
import { test } from "node:test";
import assert from "node:assert/strict";
import { OTHER, countLabel, cycleSplit, effectiveSplitId, newSplitId, nextWithMail, splitFilterFor, splitTabs, visibleTabs } from "../src/app/splits.ts";
import { SPLIT_PRESETS, peopleOf, splitFromPreset, vipQuery } from "../src/features/split/presets.ts";
import { AGES, cutoff, targets } from "../src/features/zero/zero.ts";
import { inSplit, matchesQuery } from "../src/lib/mock/split.ts";
import type { InboxSplit, ThreadSummary } from "../src/lib/types.ts";

const split = (id: string, query: string, hideWhenEmpty = false): InboxSplit => ({ id, name: id.toUpperCase(), query, hideWhenEmpty });
const SPLITS = [split("vip", "from:maya@acme.example"), split("news", "is:newsletter", true), split("cal", "has:invite")];

test("the tabs are the splits in order, then Other", () => {
  const tabs = splitTabs(SPLITS);
  assert.deepEqual(
    tabs.map((t) => t.id),
    ["vip", "news", "cal", OTHER],
  );
  assert.equal(tabs[3].name, "Other");
  assert.equal(tabs[3].query, null);
  // A split without a name shows its query.
  assert.equal(splitTabs([{ ...split("x", "label:clients"), name: "" }])[0].name, "label:clients");
});

test("a tab's list excludes what the splits before it claim", () => {
  assert.deepEqual(splitFilterFor(SPLITS, "vip"), { include: "from:maya@acme.example", exclude: [] });
  assert.deepEqual(splitFilterFor(SPLITS, "cal"), { include: "has:invite", exclude: ["from:maya@acme.example", "is:newsletter"] });
  assert.deepEqual(splitFilterFor(SPLITS, OTHER), { include: null, exclude: ["from:maya@acme.example", "is:newsletter", "has:invite"] });
  // No splits: Other is the whole inbox.
  assert.deepEqual(splitFilterFor([], OTHER), { include: null, exclude: [] });
});

test("a remembered tab that no longer exists falls back to the first", () => {
  assert.equal(effectiveSplitId(SPLITS, "news"), "news");
  assert.equal(effectiveSplitId(SPLITS, OTHER), OTHER);
  assert.equal(effectiveSplitId(SPLITS, "gone"), "vip");
  assert.equal(effectiveSplitId(SPLITS, null), "vip");
  assert.equal(effectiveSplitId([], null), OTHER);
});

test("hide-when-empty tabs leave only when known empty and not current", () => {
  const tabs = splitTabs(SPLITS);
  const counts = { vip: { total: 2, unread: 1 }, news: { total: 0, unread: 0 }, cal: { total: 0, unread: 0 }, [OTHER]: { total: 5, unread: 0 } };
  assert.deepEqual(
    visibleTabs(tabs, counts, "vip").map((t) => t.id),
    ["vip", "cal", OTHER],
  );
  // The current tab stays even when empty; before the counts arrive, all show.
  assert.equal(visibleTabs(tabs, counts, "news").length, 4);
  assert.equal(visibleTabs(tabs, null, "vip").length, 4);
});

test("Tab and ⇧Tab wrap around the shown tabs", () => {
  const tabs = splitTabs(SPLITS);
  assert.equal(cycleSplit(tabs, "vip", 1), "news");
  assert.equal(cycleSplit(tabs, OTHER, 1), "vip");
  assert.equal(cycleSplit(tabs, "vip", -1), OTHER);
  assert.equal(cycleSplit(tabs, "gone", 1), "vip");
  assert.equal(cycleSplit([], "vip", 1), "vip");
});

test("the zero screen offers the next tab that still has mail", () => {
  const tabs = splitTabs(SPLITS);
  const counts = { vip: { total: 0, unread: 0 }, news: { total: 0, unread: 0 }, cal: { total: 3, unread: 1 }, [OTHER]: { total: 5, unread: 0 } };
  assert.equal(nextWithMail(tabs, counts, "vip")?.id, "cal");
  assert.equal(nextWithMail(tabs, counts, OTHER)?.id, "cal");
  assert.equal(nextWithMail(tabs, { ...counts, cal: { total: 0, unread: 0 }, [OTHER]: { total: 0, unread: 0 } }, "vip"), null);
  assert.equal(nextWithMail(tabs, null, "vip"), null);
});

test("tab counts: total, hidden at zero, capped at 999+", () => {
  assert.equal(countLabel({ total: 0, unread: 0 }, false), null);
  assert.equal(countLabel(undefined, false), null);
  assert.equal(countLabel({ total: 12, unread: 3 }, false), "12");
  assert.equal(countLabel({ total: 12, unread: 3 }, true), "12+");
  assert.equal(countLabel({ total: 1200, unread: 3 }, false), "999+");
});

test("new split ids are slugs, numbered when taken", () => {
  assert.equal(newSplitId("VIP", []), "vip");
  assert.equal(newSplitId("VIP", ["vip", "vip-2"]), "vip-3");
  assert.equal(newSplitId("Board & Investors!", []), "board-investors");
  assert.equal(newSplitId("✨", []), "split");
});

test("presets: the defaults match settings.rs, VIP is people, Team needs a work domain", () => {
  const q = (key: string, mine: string[] = []) => SPLIT_PRESETS.find((p) => p.key === key)!.query(mine);
  // The same three as default_inbox_splits() in src-tauri/src/settings.rs.
  assert.equal(q("important"), "is:important -is:newsletter -has:invite");
  assert.equal(q("calendar"), "has:invite OR from:calendar-notification@google.com OR from:@calendly.com");
  assert.equal(q("news"), "is:newsletter");
  assert.equal(q("team", ["sam@gmail.com"]), null);
  assert.equal(q("team", ["sam@northwind.example", "sam@gmail.com", "s@northwind.example", "s@harbor.example"]), "from:@northwind.example OR from:@harbor.example");
  const vip = SPLIT_PRESETS.find((p) => p.key === "vip")!;
  assert.equal(splitFromPreset(vip, "vip", [], []), null);
  assert.deepEqual(splitFromPreset(vip, "vip", [], ["Maya@Acme.example", "lee@board.example"]), {
    id: "vip",
    name: "VIP",
    query: "from:maya@acme.example OR from:lee@board.example",
    hideWhenEmpty: false,
  });
});

test("a people split reads back as people; anything else doesn't", () => {
  assert.deepEqual(peopleOf(vipQuery(["a@x.example", "b@y.example"])), ["a@x.example", "b@y.example"]);
  assert.equal(peopleOf("from:@acme.example"), null);
  assert.equal(peopleOf("from:a@x.example is:unread"), null);
  assert.equal(peopleOf(""), null);
});

// ---------------------------------------------------------------------------
// Get to zero

const DAY = 86_400_000;
const NOW = new Date(2026, 8, 27, 12, 0).getTime();

function row(id: string, over: Partial<ThreadSummary> = {}): ThreadSummary {
  return {
    accountId: "sam@northwind.example",
    threadId: id,
    subject: `Subject ${id}`,
    snippet: "",
    participants: [],
    messageCount: 1,
    unread: false,
    starred: false,
    hasAttachments: false,
    labelIds: ["INBOX"],
    lastDate: NOW - DAY * 10,
    ...over,
  };
}

test("get to zero: ages are cutoffs, everything is none", () => {
  assert.equal(cutoff("day", NOW), NOW - DAY);
  assert.equal(cutoff("week", NOW), NOW - 7 * DAY);
  assert.equal(cutoff("month", NOW), NOW - 30 * DAY);
  assert.equal(cutoff("all", NOW), null);
  assert.equal(AGES.length, 6);
  assert.equal(new Set(AGES.map((a) => a.age)).size, AGES.length);
});

test("get to zero: keeps unread or starred when asked, once per conversation", () => {
  const rows = [row("a"), row("b", { unread: true }), row("c", { starred: true }), row("a"), row("d", { unread: true, starred: true })];
  const ids = (keep: { unread: boolean; starred: boolean }) => targets(rows, keep).map((r) => r.threadId);
  assert.deepEqual(ids({ unread: false, starred: false }), ["a", "b", "c", "d"]);
  assert.deepEqual(ids({ unread: true, starred: false }), ["a", "c"]);
  assert.deepEqual(ids({ unread: false, starred: true }), ["a", "b"]);
  assert.deepEqual(ids({ unread: true, starred: true }), ["a"]);
});

// ---------------------------------------------------------------------------
// The mock's matcher

test("mock split matcher: words, operators, OR, negation and groups", () => {
  const t = row("x", {
    subject: "Weekly digest",
    snippet: "Top stories",
    participants: [{ name: "News Desk", email: "digest@news.example" }],
    labelIds: ["INBOX", "CATEGORY_UPDATES", "UNREAD"],
    unread: true,
  });
  assert.ok(matchesQuery(t, "is:newsletter"));
  assert.ok(!matchesQuery(t, "is:important -is:newsletter -has:invite"));
  assert.ok(matchesQuery(t, "from:@news.example"));
  assert.ok(matchesQuery(t, "from:maya@acme.example OR from:digest"));
  assert.ok(!matchesQuery(t, "-(is:newsletter OR from:maya)"));
  assert.ok(matchesQuery(t, "digest is:unread"));
  assert.ok(matchesQuery(t, 'subject:"weekly"'));
  // Earlier splits claim it first.
  assert.ok(!inSplit(t, { include: null, exclude: ["is:newsletter"] }));
  assert.ok(inSplit(t, { include: "is:newsletter", exclude: ["from:maya@acme.example"] }));
});
