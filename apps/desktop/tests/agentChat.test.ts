// The chat panel's pure parts (src/features/agent-chat/model.ts): backend
// events applied to the conversation, and the agent's Markdown read into
// blocks rendered as React elements.
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyEvent, blocks, spans, type ChatState } from "../src/features/agent-chat/model.ts";

const base = (): ChatState => ({
  open: true,
  chatId: "c1",
  sessionId: null,
  messages: [
    { id: 1, role: "user", parts: [{ kind: "text", text: "Summarize this" }] },
    { id: 2, role: "agent", parts: [], pending: true },
  ],
  running: true,
  includeContext: true,
  toldContext: null,
  setup: null,
  setupError: null,
});

const step = (s: ChatState, e: Parameters<typeof applyEvent>[1]): ChatState => ({ ...s, ...applyEvent(s, e) });

test("a streamed turn: session, text deltas, a tool chip between, then done", () => {
  let s = base();
  s = step(s, { kind: "session", chatId: "c1", sessionId: "sess-1" });
  s = step(s, { kind: "text", chatId: "c1", text: "Let me " });
  s = step(s, { kind: "text", chatId: "c1", text: "read it." });
  s = step(s, { kind: "tool", chatId: "c1", name: "thread_context", label: "Read a conversation" });
  s = step(s, { kind: "text", chatId: "c1", text: "Priya wants the deck." });
  s = step(s, { kind: "done", chatId: "c1", error: null, stopped: false });
  assert.equal(s.sessionId, "sess-1");
  assert.equal(s.running, false);
  const agent = s.messages[1];
  assert.equal(agent.pending, false);
  assert.deepEqual(agent.parts, [
    { kind: "text", text: "Let me read it." },
    { kind: "tool", name: "thread_context", label: "Read a conversation" },
    { kind: "text", text: "Priya wants the deck." },
  ]);
});

test("another chat's events change nothing", () => {
  const s = base();
  assert.deepEqual(applyEvent(s, { kind: "text", chatId: "old", text: "late" }), {});
  assert.deepEqual(applyEvent(s, { kind: "done", chatId: "old", error: null, stopped: true }), {});
});

test("a failed or stopped turn keeps what arrived and says why", () => {
  let s = step(base(), { kind: "text", chatId: "c1", text: "Partial" });
  s = step(s, { kind: "done", chatId: "c1", error: "Not logged in · Please run /login", stopped: false });
  assert.equal(s.messages[1].error, "Not logged in · Please run /login");
  assert.equal(s.messages[1].parts.length, 1);
  const stopped = step(base(), { kind: "done", chatId: "c1", error: null, stopped: true });
  assert.equal(stopped.messages[1].stopped, true);
  assert.equal(stopped.running, false);
});

test("bold and code spans; unmatched markers stay text", () => {
  assert.deepEqual(spans("Pay **$2,672** by `Oct 9`"), [
    { kind: "plain", text: "Pay " },
    { kind: "bold", text: "$2,672" },
    { kind: "plain", text: " by " },
    { kind: "code", text: "Oct 9" },
  ]);
  assert.deepEqual(spans("2 * 3 = 6 and a lone `tick"), [{ kind: "plain", text: "2 * 3 = 6 and a lone `tick" }]);
});

test("the agent's Markdown: paragraphs, lists, headings, code", () => {
  const b = blocks("## Invoices\nTwo are due:\n- **MS-049** from Julia\n- MS-050\n\n1. Pay\n2. File\n\n```\nraw <b>text</b>\n```\nDone.");
  assert.deepEqual(
    b.map((x) => x.kind),
    ["heading", "para", "list", "list", "pre", "para"],
  );
  assert.equal(b[2].kind === "list" && b[2].ordered, false);
  assert.equal(b[3].kind === "list" && b[3].ordered, true);
  // HTML stays text: the panel renders blocks as elements, never as HTML.
  assert.equal(b[4].kind === "pre" && b[4].text, "raw <b>text</b>");
  // Lines without a blank between are one paragraph with breaks.
  const p = blocks("one\ntwo\n\nthree");
  assert.equal(p.length, 2);
  assert.equal(p[0].kind === "para" && p[0].lines.length, 2);
});
