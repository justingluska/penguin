// Write with AI, suggested replies and snippet files in the mock
// (features/compose). A fake "on-device writer": it doesn't understand
// anything, it rearranges the text it's given and the thread's last message
// so every state of the composer can be seen and tested. It streams words the
// way the real one streams snapshots.
//
// URL switches (dev:mock), shared with the summaries mock:
//   ?ai=notEnabled | noDevice | notReady | old  → unavailable (Write and suggestions hide)
//   ?ai=refuse → write_with_ai fails the way a guardrail refusal does
//   ?ai=slow   → slower streaming
import type { MessageView, ReplySuggestions, SnippetFile, ThreadView, WriteRequest, WriteResult } from "../types";
import type { MockHandler } from "./index";
import { mockBackend } from "./index";
import { mailHandlers } from "./mail";

const EVENT = "penguin://write-progress";

function query(name: string): string | null {
  return typeof location === "undefined" ? null : new URLSearchParams(location.search).get(name);
}

const UNAVAILABLE = new Set(["notEnabled", "noDevice", "notReady", "old"]);
const available = () => !UNAVAILABLE.has(query("ai") ?? "");
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const running = new Map<string, { cancelled: boolean }>();

async function thread(accountId: string, threadId: string): Promise<ThreadView | null> {
  return (await mailHandlers.get_thread({ accountId, threadId })) as ThreadView | null;
}

function latest(t: ThreadView | null): MessageView | null {
  const msgs = (t?.messages ?? []).filter((m) => !m.labelIds.includes("DRAFT") && !m.labelIds.includes("SENT"));
  return msgs[msgs.length - 1] ?? null;
}

const first = (m: MessageView | null) => m?.from.name?.split(" ")[0] ?? null;

const TYPOS: Array<[RegExp, string]> = [
  [/\bteh\b/g, "the"],
  [/\brecieve/g, "receive"],
  [/\bi\b/g, "I"],
  [/\bdont\b/g, "don't"],
  [/\bcant\b/g, "can't"],
  [/\bim\b/gi, "I'm"],
  [/\bthx\b/gi, "thanks"],
  [/ {2,}/g, " "],
  [/\s+([,.!?])/g, "$1"],
];

function sentences(s: string): string[] {
  return s.replace(/\s+/g, " ").match(/[^.!?]+[.!?]*/g)?.map((x) => x.trim()).filter(Boolean) ?? [];
}

function capitalize(line: string): string {
  return line.replace(/^(\s*)([a-z])/, (_, a: string, b: string) => a + b.toUpperCase());
}

/** The fake rewrite for each action. */
function rewrite(req: WriteRequest, t: ThreadView | null): string {
  const text = req.text.trim();
  switch (req.action) {
    case "grammar":
      return text
        .split("\n")
        .map((l) => capitalize(TYPOS.reduce((acc, [re, to]) => acc.replace(re, to), l)))
        .join("\n");
    case "shorter": {
      const s = sentences(text);
      return s.length > 2 ? s.slice(0, Math.max(1, Math.ceil(s.length / 2))).join(" ") : text.split(/\s+/).slice(0, Math.max(4, Math.ceil(text.split(/\s+/).length * 0.6))).join(" ");
    }
    case "friendlier":
      return `${text.replace(/[.]\s*$/, "")}. Thanks so much, really appreciate it!`;
    case "formal":
      return text
        .replace(/\bHey\b/g, "Hello")
        .replace(/\bthanks\b/gi, "Thank you")
        .replace(/\bcan't\b/g, "cannot")
        .replace(/\bdon't\b/g, "do not")
        .replace(/\bI'm\b/g, "I am")
        .replace(/\bI'll\b/g, "I will")
        .replace(/!/g, ".");
    case "custom":
      return `${text}\n\n(${req.instruction.trim()})`;
    case "draft": {
      const m = latest(t);
      const name = first(m) ?? req.recipients[0]?.split(" ")[0] ?? null;
      const ask = req.instruction.trim().replace(/[.]+$/, "");
      const body = ask ? `${capitalize(ask)}.` : "Thanks for the note.";
      const lines = [name ? `Hi ${name},` : "Hi,", "", m ? `Thanks for your message about ${t?.subject || "this"}. ${body}` : body];
      if (text) lines.push("", text);
      // No sign-off: the composer's signature follows.
      return lines.join("\n").trimEnd();
    }
  }
}

function suggestionsFor(m: MessageView | null): string[] {
  const body = m?.bodyText ?? "";
  if (/\?/.test(body)) return ["Yes, that works for me.", "Not yet, I'll get back to you by Friday.", "Could you share a bit more detail?"];
  if (/\b(attached|attachment|deck|draft|doc)\b/i.test(body)) return ["Thanks, I'll take a look today.", "Got it, looks good to me.", "Thanks! A couple of notes to follow."];
  return ["Thanks for the update!", "Sounds good, talk soon.", "Great news, thanks for letting me know."];
}

// A few bytes per "file", so attaching a snippet's file works in the browser.
const files = new Map<string, { meta: SnippetFile; data: string }>([
  ["c".repeat(64), { meta: { id: "c".repeat(64), filename: "Northwind pricing 2026.pdf", mimeType: "application/pdf", size: 184_320 }, data: btoa("%PDF-1.4\n% mock pricing sheet\n") }],
]);

async function sha256Hex(data: string): Promise<string> {
  const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export const writingHandlers: Record<string, MockHandler> = {
  write_with_ai: async ({ request }): Promise<WriteResult> => {
    const req = request as WriteRequest;
    if (!available()) throw { code: "invalidInput", message: "Apple Intelligence isn't available on this Mac." };
    const t = req.threadId ? await thread(req.accountId, req.threadId) : null;
    const run = { cancelled: false };
    running.set(req.runId, run);
    try {
      await sleep(300); // the model loading
      if (query("ai") === "refuse") throw { code: "other", message: "Apple's on-device model declined to write this." };
      const full = rewrite(req, t);
      const words = full.split(/(\s+)/);
      let out = "";
      for (let i = 0; i < words.length; i++) {
        if (run.cancelled) throw { code: "cancelled", message: "Stopped" };
        out += words[i];
        if (i % 4 === 3 || i === words.length - 1) {
          mockBackend.emit(EVENT, { runId: req.runId, text: out });
          await sleep(query("ai") === "slow" ? 160 : 45);
        }
      }
      return { runId: req.runId, text: full };
    } finally {
      running.delete(req.runId);
    }
  },
  cancel_write: ({ runId }) => {
    const r = running.get(runId);
    if (r) r.cancelled = true;
    return !!r;
  },
  prewarm_writer: () => undefined,
  suggest_replies: async ({ accountId, threadId }): Promise<ReplySuggestions> => {
    if (!available()) throw { code: "invalidInput", message: "Apple Intelligence isn't available on this Mac." };
    const t = await thread(accountId, threadId);
    await sleep(query("ai") === "slow" ? 1800 : 700);
    return { accountId, threadId, version: String(t?.messages.length ?? 0), replies: suggestionsFor(latest(t)) };
  },
  save_snippet_file: async ({ filename, mimeType, dataBase64 }): Promise<SnippetFile> => {
    const id = await sha256Hex(dataBase64);
    const meta: SnippetFile = { id, filename: String(filename).slice(0, 200) || "Attachment", mimeType: mimeType || "application/octet-stream", size: Math.floor((dataBase64.length * 3) / 4) };
    files.set(id, { meta, data: dataBase64 });
    return meta;
  },
  read_snippet_file: ({ id }) => {
    const f = files.get(id);
    if (!f) throw { code: "notFound", message: "That file is no longer on this Mac." };
    return f.data;
  },
};
