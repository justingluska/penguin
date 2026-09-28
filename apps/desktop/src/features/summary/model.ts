// Thread summary card: its states and what each one shows. Pure (no api,
// no React), so tests/summary.test.ts runs it in node. OWNER: summaries.
//
//   none ─ Summarize / cached found ─▶ starting ─▶ reading n/m ─▶ writing ─▶ summary
//                                          │            │             │
//                                          └── cancel ──┴── error ────┘
//   summary ─ thread changed ─▶ summary (stale, "Update")
//
// A cached summary opens the card by itself; a stale one opens it marked
// stale. Cancelling returns to whatever was shown before.
import type { AiSummary, AiUnavailableReason, MessageView, SummaryProgress } from "../../lib/types";

export interface RunState {
  stage: "starting" | "reading" | "writing";
  step: number;
  steps: number;
  /** The summary so far (writing). */
  partial: AiSummary | null;
}

export interface CardState {
  /** The last complete summary: from the cache or the last run. */
  summary: AiSummary | null;
  run: RunState | null;
  error: string | null;
  /** The card is on screen. */
  open: boolean;
}

export const EMPTY: CardState = { summary: null, run: null, error: null, open: false };

export type CardAction =
  /** cached_summary answered (null: none, or summaries are off). */
  | { type: "cached"; summary: AiSummary | null }
  | { type: "start" }
  | { type: "progress"; progress: SummaryProgress }
  | { type: "done"; summary: AiSummary }
  | { type: "failed"; message: string }
  | { type: "cancelled" }
  /** The shortcut on a closed card with a current summary. */
  | { type: "show" }
  /** The ✕, or the shortcut on an open summary. */
  | { type: "close" };

export function reduce(s: CardState, a: CardAction): CardState {
  switch (a.type) {
    case "cached":
      // A run in progress wins over what the cache says.
      if (s.run) return s;
      if (!a.summary) return s.summary ? { ...s, summary: null, open: false } : s;
      // Keep a closed card closed for the same summary; a different one
      // (new, or now stale) opens it.
      if (s.summary && !s.open && s.summary.version === a.summary.version && s.summary.stale === a.summary.stale) {
        return { ...s, summary: a.summary };
      }
      return { ...s, summary: a.summary, open: true, error: null };
    case "start":
      return { ...s, run: { stage: "starting", step: 0, steps: 0, partial: null }, error: null, open: true };
    case "progress": {
      // Late events from a run that was cancelled or already finished.
      if (!s.run) return s;
      const p = a.progress;
      const partial = p.stage === "writing" ? (p.partial ?? s.run.partial) : null;
      return { ...s, run: { stage: p.stage, step: p.step, steps: p.steps, partial } };
    }
    case "done":
      return { summary: a.summary, run: null, error: null, open: true };
    case "failed":
      return { ...s, run: null, error: a.message, open: true };
    case "cancelled":
      return { ...s, run: null, error: null, open: s.summary !== null };
    case "show":
      return s.summary ? { ...s, error: null, open: true } : s;
    case "close":
      return s.run ? s : { ...s, error: null, open: false };
  }
}

export type CardView =
  | { kind: "none" }
  | { kind: "starting" }
  | { kind: "reading"; step: number; steps: number }
  | { kind: "writing"; summary: AiSummary }
  | { kind: "summary"; summary: AiSummary; stale: boolean }
  | { kind: "error"; message: string };

export function cardView(s: CardState): CardView {
  if (!s.open) return { kind: "none" };
  if (s.run) {
    if (s.run.stage === "writing" && s.run.partial) return { kind: "writing", summary: s.run.partial };
    if (s.run.stage === "reading") return { kind: "reading", step: s.run.step, steps: s.run.steps };
    return { kind: "starting" };
  }
  if (s.error) return { kind: "error", message: s.error };
  if (s.summary) return { kind: "summary", summary: s.summary, stale: s.summary.stale };
  return { kind: "none" };
}

/** What the shortcut (⇧S) and the toolbar button do in this state. */
export function toggleIntent(s: CardState): "summarize" | "show" | "close" | "nothing" {
  if (s.run) return "nothing";
  if (s.open && s.summary && !s.summary.stale && !s.error) return "close";
  if (!s.open && s.summary && !s.summary.stale) return "show";
  return "summarize";
}

/** "#3 · Maya": the source message's place in the thread and its sender. */
export function sourceLabel(messageId: string | null, messages: MessageView[], me: (email: string) => boolean): string | null {
  if (!messageId) return null;
  const i = messages.findIndex((m) => m.id === messageId);
  if (i < 0) return null;
  const from = messages[i].from;
  const who = me(from.email) ? "You" : (from.name?.trim().split(/\s+/)[0] || from.email.split("@")[0]);
  return `#${i + 1} · ${who}`;
}

/** The card's footnote: how much it read, and when. */
export function footnote(summary: AiSummary, now: number): string {
  const n = summary.messageCount;
  const read =
    summary.omitted > 0 ? `the first and latest ${n} of ${n + summary.omitted} messages` : `${n} ${n === 1 ? "message" : "messages"}`;
  return `Summarized ${read} on this Mac · ${ago(summary.createdAt, now)}`;
}

export function ago(at: number, now: number): string {
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  return d === 1 ? "yesterday" : `${d} days ago`;
}

/** Why the Summarize action is hidden; shown once, in Settings → AI. Mirrors unavailable_text in src-tauri/src/summary/mod.rs. */
export function unavailableText(reason: AiUnavailableReason | null): string {
  switch (reason) {
    case null:
      return "Summaries are available.";
    case "deviceNotEligible":
      return "This Mac can't run Apple Intelligence, which summaries use. It needs a Mac with Apple silicon (M1 or later).";
    case "appleIntelligenceNotEnabled":
      return "Turn on Apple Intelligence in System Settings → Apple Intelligence & Siri to summarize conversations.";
    case "modelNotReady":
      return "Apple Intelligence is still getting ready on this Mac (downloading its model). Summaries will work once it's done.";
    case "osTooOld":
      return "Summaries need macOS 26 or later with Apple Intelligence.";
    case "unsupportedPlatform":
      return "Summaries run on a Mac with Apple Intelligence.";
    case "notBuilt":
      return "This build of Penguin was made without Apple's Foundation Models SDK (Xcode 26 or later).";
    case "unknown":
      return "Apple Intelligence isn't available on this Mac right now.";
  }
}
