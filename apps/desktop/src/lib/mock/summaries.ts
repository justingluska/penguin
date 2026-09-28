// Thread summaries in the mock (features/summary). OWNER: summaries.
// A fake "on-device model": it reads the open thread's own messages and
// streams a summary built from them (first sentences, questions, weekday
// deadlines), citing real message ids so the card's source links work. It
// is not a model, just enough to show every state of the card.
//
// URL switches (dev:mock):
//   ?ai=notEnabled | noDevice | notReady | old  → summary_availability says so
//   ?ai=refuse   → summarize_thread fails as a refusal
//   ?ai=slow     → a long thread's "reading part n of m" steps are slower
import type { AiAvailability, AiSummary, AiUnavailableReason, MessageView, SummaryProgress, ThreadView } from "../types";
import type { MockHandler } from "./index";
import { mockBackend } from "./index";
import { mailHandlers } from "./mail";

const EVENT = "penguin://summary-progress";

function query(name: string): string | null {
  return typeof location === "undefined" ? null : new URLSearchParams(location.search).get(name);
}

const REASONS: Record<string, AiUnavailableReason> = {
  notEnabled: "appleIntelligenceNotEnabled",
  noDevice: "deviceNotEligible",
  notReady: "modelNotReady",
  old: "osTooOld",
};

function availability(): AiAvailability {
  const reason = REASONS[query("ai") ?? ""] ?? null;
  return { available: reason === null, reason, contextTokens: reason ? 0 : 4096 };
}

const cache = new Map<string, AiSummary>();
const running = new Map<string, { cancelled: boolean }>();
const key = (accountId: string, threadId: string) => `${accountId}\u0000${threadId}`;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** What version_key covers: the messages and their text. */
function version(t: ThreadView): string {
  let h = 0;
  for (const m of t.messages) {
    if (m.labelIds.includes("DRAFT")) continue;
    for (const c of m.id + "\u0000" + m.bodyText) h = (Math.imul(h, 31) + c.charCodeAt(0)) | 0;
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

async function thread(accountId: string, threadId: string): Promise<ThreadView | null> {
  return (await mailHandlers.get_thread({ accountId, threadId })) as ThreadView | null;
}

function firstSentence(text: string): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const cut = flat.search(/[.!?](\s|$)/);
  const s = cut > 0 ? flat.slice(0, cut + 1) : flat;
  return s.length > 110 ? s.slice(0, 108).trimEnd() + "…" : s;
}

const WEEKDAY = /\b(monday|tuesday|wednesday|thursday|friday|saturday|sunday|tomorrow|tonight|end of (the )?(day|week))\b/i;

function who(m: MessageView): string {
  return m.from.name?.split(" ")[0] || m.from.email.split("@")[0];
}

/** The whole fake summary for a thread. */
function compose(t: ThreadView): AiSummary {
  const msgs = t.messages.filter((m) => !m.labelIds.includes("DRAFT") && m.bodyText.trim());
  const points = msgs.slice(-5).map((m) => ({ text: `${who(m)}: ${firstSentence(m.bodyText)}`, messageId: m.id }));
  const asks = msgs
    .filter((m) => m.bodyText.includes("?"))
    .slice(-3)
    .map((m) => {
      const q = m.bodyText.replace(/\s+/g, " ").match(/[^.!?]*\?/)?.[0].trim() ?? "Reply";
      return { text: `${who(m)} asks: ${q}`, messageId: m.id, due: m.bodyText.match(WEEKDAY)?.[0] ?? null };
    });
  const people = [...new Set(msgs.map(who))];
  const names = people.length > 2 ? `${people.slice(0, 2).join(", ")} and others` : people.join(" and ");
  const gist =
    msgs.length <= 1
      ? `${names || "Someone"} wrote about “${t.subject || "(no subject)"}”. ${firstSentence(msgs[0]?.bodyText ?? "")}`
      : `${msgs.length} messages between ${names} about “${t.subject || "(no subject)"}”. The latest: ${firstSentence(msgs[msgs.length - 1].bodyText)}`;
  return {
    accountId: t.accountId,
    threadId: t.threadId,
    version: version(t),
    gist: gist.trim(),
    points,
    asks,
    messageCount: msgs.length,
    omitted: 0,
    createdAt: Date.now(),
    stale: false,
  };
}

/** The summary as the model would stream it: points, then asks, then the gist (Swift property order). */
function partials(full: AiSummary): AiSummary[] {
  const out: AiSummary[] = [];
  const base = { ...full, gist: "", points: [] as AiSummary["points"], asks: [] as AiSummary["asks"] };
  for (let i = 1; i <= full.points.length; i++) out.push({ ...base, points: full.points.slice(0, i) });
  for (let i = 1; i <= full.asks.length; i++) out.push({ ...base, points: full.points, asks: full.asks.slice(0, i) });
  const words = full.gist.split(" ");
  for (let i = 4; i < words.length; i += 4) out.push({ ...full, gist: words.slice(0, i).join(" ") });
  out.push(full);
  return out;
}

function progress(p: SummaryProgress) {
  mockBackend.emit(EVENT, p);
}

export const summaryHandlers: Record<string, MockHandler> = {
  summary_availability: () => availability(),
  cached_summary: async ({ accountId, threadId }) => {
    const s = cache.get(key(accountId, threadId));
    if (!s) return null;
    const t = await thread(accountId, threadId);
    return t ? { ...s, stale: version(t) !== s.version } : null;
  },
  summarize_thread: async ({ accountId, threadId }) => {
    const a = availability();
    if (!a.available) throw { code: "invalidInput", message: "Apple Intelligence isn't available on this Mac." };
    const t = await thread(accountId, threadId);
    if (!t) throw { code: "notFound", message: "This conversation is no longer available." };
    const k = key(accountId, threadId);
    running.get(k) && (running.get(k)!.cancelled = true);
    const run = { cancelled: false };
    running.set(k, run);
    try {
      const full = compose(t);
      // Long threads: "Reading part n of m" first, like the map steps.
      const parts = t.messages.length > 6 ? Math.ceil(t.messages.length / 6) : 0;
      const steps = parts + 1;
      for (let i = 1; i <= parts; i++) {
        progress({ accountId, threadId, stage: "reading", step: i, steps, partial: null });
        await sleep(query("ai") === "slow" ? 1200 : 450);
        if (run.cancelled) throw { code: "cancelled", message: "Summary cancelled" };
      }
      await sleep(350); // the model loading, as on a first run
      if (query("ai") === "refuse") {
        throw { code: "other", message: "Apple's on-device model declined to summarize this conversation." };
      }
      for (const p of partials(full)) {
        if (run.cancelled) throw { code: "cancelled", message: "Summary cancelled" };
        progress({ accountId, threadId, stage: "writing", step: steps, steps, partial: p });
        await sleep(90);
      }
      cache.set(k, full);
      return full;
    } finally {
      if (running.get(k) === run) running.delete(k);
    }
  },
  cancel_summary: ({ accountId, threadId }) => {
    const r = running.get(key(accountId, threadId));
    if (r) r.cancelled = true;
    return !!r;
  },
  prewarm_summarizer: () => undefined,
};
