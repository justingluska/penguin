// Thread summary card states (features/summary/model.ts): cached, streaming,
// reading parts of a long thread, stale, error, cancel, and the ⇧S toggle.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  EMPTY,
  ago,
  cardView,
  footnote,
  reduce,
  sourceLabel,
  toggleIntent,
  unavailableText,
  type CardAction,
  type CardState,
} from "../src/features/summary/model.ts";
import type { AiSummary, AiUnavailableReason, MessageView, SummaryProgress } from "../src/lib/types.ts";

const T = 1_789_000_000_000;

function summary(over: Partial<AiSummary> = {}): AiSummary {
  return {
    accountId: "sam@penguin.example",
    threadId: "t1",
    version: "v1",
    gist: "Maya asked Sam to confirm the venue by Friday.",
    points: [{ text: "Lake house chosen", messageId: "m2" }],
    asks: [{ text: "Confirm the venue", messageId: "m1", due: "Friday" }],
    messageCount: 2,
    omitted: 0,
    createdAt: T,
    stale: false,
    ...over,
  };
}

function progress(over: Partial<SummaryProgress>): SummaryProgress {
  return { accountId: "sam@penguin.example", threadId: "t1", stage: "writing", step: 1, steps: 1, partial: null, ...over };
}

const run = (...actions: CardAction[]): CardState => actions.reduce(reduce, EMPTY);

test("nothing shows until a summary is cached or asked for", () => {
  assert.deepEqual(cardView(EMPTY), { kind: "none" });
  assert.deepEqual(cardView(run({ type: "cached", summary: null })), { kind: "none" });
});

test("a cached summary shows by itself; a stale one says so", () => {
  assert.deepEqual(cardView(run({ type: "cached", summary: summary() })), { kind: "summary", summary: summary(), stale: false });
  const stale = summary({ stale: true });
  assert.deepEqual(cardView(run({ type: "cached", summary: stale })), { kind: "summary", summary: stale, stale: true });
});

test("summarizing: starting, then the partial summary streams, then the result", () => {
  let s = run({ type: "start" });
  assert.deepEqual(cardView(s), { kind: "starting" });
  const partial = summary({ gist: "", asks: [] });
  s = reduce(s, { type: "progress", progress: progress({ partial }) });
  assert.deepEqual(cardView(s), { kind: "writing", summary: partial });
  // A snapshot without a partial keeps the last one on screen.
  s = reduce(s, { type: "progress", progress: progress({ partial: null }) });
  assert.deepEqual(cardView(s), { kind: "writing", summary: partial });
  s = reduce(s, { type: "done", summary: summary() });
  assert.deepEqual(cardView(s), { kind: "summary", summary: summary(), stale: false });
});

test("long threads show which part is being read", () => {
  const s = run({ type: "start" }, { type: "progress", progress: progress({ stage: "reading", step: 2, steps: 4 }) });
  assert.deepEqual(cardView(s), { kind: "reading", step: 2, steps: 4 });
});

test("progress after a run ended is ignored", () => {
  const s = run({ type: "cached", summary: summary() });
  assert.equal(reduce(s, { type: "progress", progress: progress({ partial: summary({ gist: "late" }) }) }), s);
});

test("a cached answer never interrupts a run in progress", () => {
  const s = run({ type: "start" });
  assert.equal(reduce(s, { type: "cached", summary: summary() }), s);
});

test("errors show with the message; Try again starts over", () => {
  let s = run({ type: "start" }, { type: "failed", message: "Apple's on-device model declined to summarize this conversation." });
  assert.deepEqual(cardView(s), { kind: "error", message: "Apple's on-device model declined to summarize this conversation." });
  s = reduce(s, { type: "start" });
  assert.deepEqual(cardView(s), { kind: "starting" });
});

test("cancelling returns to what was there before", () => {
  assert.deepEqual(cardView(run({ type: "start" }, { type: "cancelled" })), { kind: "none" });
  const regen = run({ type: "cached", summary: summary() }, { type: "start" }, { type: "cancelled" });
  assert.deepEqual(cardView(regen), { kind: "summary", summary: summary(), stale: false });
});

test("closing hides the card and it stays closed for the same summary", () => {
  let s = run({ type: "cached", summary: summary() }, { type: "close" });
  assert.deepEqual(cardView(s), { kind: "none" });
  s = reduce(s, { type: "cached", summary: summary() });
  assert.deepEqual(cardView(s), { kind: "none" }, "re-reading the cache doesn't reopen it");
  s = reduce(s, { type: "cached", summary: summary({ stale: true }) });
  assert.equal(cardView(s).kind, "summary", "the thread changed: show it again, marked stale");
  // Closing does nothing mid-run (Stop is the way out).
  const busy = run({ type: "start" });
  assert.equal(reduce(busy, { type: "close" }), busy);
});

test("⇧S: summarize, hide, show again; stale or failed means summarize", () => {
  assert.equal(toggleIntent(EMPTY), "summarize");
  const shown = run({ type: "cached", summary: summary() });
  assert.equal(toggleIntent(shown), "close");
  const hidden = reduce(shown, { type: "close" });
  assert.equal(toggleIntent(hidden), "show");
  assert.deepEqual(cardView(reduce(hidden, { type: "show" })).kind, "summary");
  assert.equal(toggleIntent(run({ type: "cached", summary: summary({ stale: true }) })), "summarize");
  assert.equal(toggleIntent(run({ type: "start" }, { type: "failed", message: "x" })), "summarize");
  assert.equal(toggleIntent(run({ type: "start" })), "nothing");
});

test("summaries turned off (cache answers null) hide what was shown", () => {
  const s = run({ type: "cached", summary: summary() }, { type: "cached", summary: null });
  assert.deepEqual(cardView(s), { kind: "none" });
});

function message(id: string, name: string | null, email: string): MessageView {
  return { id, from: { name, email } } as unknown as MessageView;
}

test("source links read '#n · first name', 'You' for your own mail", () => {
  const msgs = [
    message("m1", "Maya Lin", "maya@northwind.example"),
    message("m2", null, "sam@penguin.example"),
    message("m3", null, "ops@contoso.example"),
  ];
  const me = (e: string) => e === "sam@penguin.example";
  assert.equal(sourceLabel("m1", msgs, me), "#1 · Maya");
  assert.equal(sourceLabel("m2", msgs, me), "#2 · You");
  assert.equal(sourceLabel("m3", msgs, me), "#3 · ops");
  assert.equal(sourceLabel("gone", msgs, me), null);
  assert.equal(sourceLabel(null, msgs, me), null);
});

test("the footnote says how much was read and when", () => {
  assert.equal(footnote(summary(), T + 5_000), "Summarized 2 messages on this Mac · just now");
  assert.equal(
    footnote(summary({ messageCount: 38, omitted: 12 }), T + 3 * 3_600_000),
    "Summarized the first and latest 38 of 50 messages on this Mac · 3 h ago",
  );
  assert.equal(ago(T, T + 26 * 3_600_000), "yesterday");
  assert.equal(ago(T, T + 10 * 60_000), "10 min ago");
});

test("every unavailable reason has its own explanation", () => {
  const reasons: AiUnavailableReason[] = [
    "deviceNotEligible",
    "appleIntelligenceNotEnabled",
    "modelNotReady",
    "osTooOld",
    "unsupportedPlatform",
    "notBuilt",
    "unknown",
  ];
  const texts = new Set(reasons.map(unavailableText));
  assert.equal(texts.size, reasons.length);
  assert.match(unavailableText("appleIntelligenceNotEnabled"), /System Settings → Apple Intelligence & Siri/);
});
