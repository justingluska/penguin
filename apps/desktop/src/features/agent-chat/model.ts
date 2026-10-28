// Pure parts of the chat panel (tests/agentChat.test.ts): applying backend
// events to the conversation, and reading the agent's plain-text Markdown
// into blocks the panel renders as React elements (never as HTML).
import type { AgentChatEvent, AgentChatSetup } from "../../lib/types";

export type ChatPart = { kind: "text"; text: string } | { kind: "tool"; name: string; label: string };

export interface ChatMessage {
  id: number;
  role: "user" | "agent";
  parts: ChatPart[];
  /** Agent turns: still streaming. */
  pending?: boolean;
  error?: string | null;
  stopped?: boolean;
  /** User turns: the conversation it was asked about, as shown. */
  about?: string | null;
}

export interface ChatState {
  open: boolean;
  chatId: string;
  sessionId: string | null;
  messages: ChatMessage[];
  running: boolean;
  /** Send the open conversation with the question (the chip's ×). */
  includeContext: boolean;
  /** The conversation the agent was last told about ("" = none); null = nothing sent yet. */
  toldContext: string | null;
  setup: AgentChatSetup | null;
  setupError: string | null;
}

const lastAgent = (s: ChatState) => {
  const last = s.messages[s.messages.length - 1];
  return last?.role === "agent" ? last : null;
};

/** Apply one backend event to the chat it belongs to (other chats' events are ignored). */
export function applyEvent(s: ChatState, e: AgentChatEvent): Partial<ChatState> {
  if (e.chatId !== s.chatId) return {};
  if (e.kind === "session") return { sessionId: e.sessionId };
  const last = lastAgent(s);
  if (!last) return {};
  const replace = (m: ChatMessage) => ({ messages: [...s.messages.slice(0, -1), m] });
  switch (e.kind) {
    case "text": {
      const parts = [...last.parts];
      const tail = parts[parts.length - 1];
      if (tail?.kind === "text") parts[parts.length - 1] = { kind: "text", text: tail.text + e.text };
      else parts.push({ kind: "text", text: e.text });
      return replace({ ...last, parts });
    }
    case "tool":
      return replace({ ...last, parts: [...last.parts, { kind: "tool", name: e.name, label: e.label }] });
    case "done":
      return { running: false, ...replace({ ...last, pending: false, error: e.error, stopped: e.stopped }) };
  }
}

/** A run of text: plain, **bold** or `code`. */
export type Span = { kind: "plain" | "bold" | "code"; text: string };
export type Block = { kind: "para"; lines: Span[][] } | { kind: "list"; ordered: boolean; items: Span[][] } | { kind: "heading"; spans: Span[] } | { kind: "pre"; text: string };

/** `**bold**` and `` `code` `` in one line; anything unmatched stays plain. */
export function spans(line: string): Span[] {
  const out: Span[] = [];
  const re = /(\*\*[^*]+\*\*|`[^`]+`)/g;
  let at = 0;
  for (const m of line.matchAll(re)) {
    if (m.index! > at) out.push({ kind: "plain", text: line.slice(at, m.index) });
    const t = m[0];
    out.push(t.startsWith("**") ? { kind: "bold", text: t.slice(2, -2) } : { kind: "code", text: t.slice(1, -1) });
    at = m.index! + t.length;
  }
  if (at < line.length) out.push({ kind: "plain", text: line.slice(at) });
  return out;
}

/** The agent's Markdown, the subset a chat reply uses: paragraphs, lists, headings, fenced code. */
export function blocks(text: string): Block[] {
  const out: Block[] = [];
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.trimStart().startsWith("```")) {
      const body: string[] = [];
      for (i++; i < lines.length && !lines[i].trimStart().startsWith("```"); i++) body.push(lines[i]);
      out.push({ kind: "pre", text: body.join("\n") });
      continue;
    }
    if (!line.trim()) continue;
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    if (heading) {
      out.push({ kind: "heading", spans: spans(heading[1]) });
      continue;
    }
    const item = /^\s*(?:([-*•])|(\d+)[.)])\s+(.*)$/.exec(line);
    if (item) {
      const ordered = !!item[2];
      const prev = out[out.length - 1];
      if (prev?.kind === "list" && prev.ordered === ordered) prev.items.push(spans(item[3]));
      else out.push({ kind: "list", ordered, items: [spans(item[3])] });
      continue;
    }
    const prev = out[out.length - 1];
    // Consecutive lines are one paragraph (with line breaks), as in a chat.
    if (prev?.kind === "para" && lines[i - 1]?.trim()) prev.lines.push(spans(line));
    else out.push({ kind: "para", lines: [spans(line)] });
  }
  return out;
}
