// Chat with Claude Code or Codex in the mock (src-tauri/src/agent_chat/).
// No CLI runs: "detection" finds a signed-in Claude Code and a Codex that
// isn't signed in, and a turn streams a scripted reply about the open
// conversation (or the inbox) with the tool chips the real agent shows.
//
// URL switches (dev:mock):
//   ?chat=on    → the chat starts turned on (with ?agents=read or higher, ready)
//   ?chat=none  → nothing found on this Mac
//   ?chat=fail  → every turn fails the way a signed-out CLI does
import type { AgentChatConfigPatch, AgentChatRequest, AgentChatSetup, AgentCliFound, AgentCliKind, Settings, ThreadSummary, ThreadView } from "../types";
import type { MockHandler } from "./index";
import { mockBackend } from "./index";
import { mailHandlers } from "./mail";

const EVENT = "penguin://agent-chat";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const query = (name: string) => (typeof location === "undefined" ? null : new URLSearchParams(location.search).get(name));

let enabled = query("chat") === "on";
let chosen: AgentCliKind | null = null;
let customPath: string | null = null;
let found: AgentCliFound[] | null = null;
const keys: Record<AgentCliKind, boolean> = { claude: false, codex: false };
let turn = 0;
/** The running turn's chat, for Stop. */
let running: string | null = null;

function detect(): AgentCliFound[] {
  if (query("chat") === "none") return [];
  return [
    { kind: "claude", path: customPath ?? "/Users/sam/.local/bin/claude", version: "2.1.289", signedIn: true, foundIn: customPath ? "custom" : "common" },
    { kind: "codex", path: "/opt/homebrew/bin/codex", version: "0.159.0", signedIn: keys.codex, foundIn: "path" },
  ];
}

async function access(): Promise<Settings["mcp"]["access"]> {
  return ((await mailHandlers.get_settings({})) as Settings).mcp.access;
}

async function setup(): Promise<AgentChatSetup> {
  if (enabled && !found) found = detect();
  const a = await access();
  const cli = found ? (found.find((f) => f.kind === chosen) ?? found[0] ?? null) : null;
  const blocked = !enabled
    ? "Chat is off. Turn it on in Settings → AI."
    : a === "off"
      ? "Agents are off, so the chat has no way to read your mail. In Settings → Developer → Agents, choose Read only or higher."
      : found && found.length === 0
        ? "Penguin didn't find Claude Code or Codex on this Mac. Install one, or enter its path in Settings → AI."
        : null;
  return {
    enabled,
    cli: cli?.kind ?? null,
    chosen,
    customPath,
    found,
    claudeKeySaved: keys.claude,
    codexKeySaved: keys.codex,
    access: a,
    toolsReady: true,
    blocked,
  };
}

function emit(e: unknown) {
  mockBackend.emit(EVENT, e);
}

async function stream(chatId: string, mine: number, text: string) {
  const words = text.split(/(\s+)/);
  for (let i = 0; i < words.length; i += 3) {
    if (turn !== mine) return false;
    emit({ kind: "text", chatId, text: words.slice(i, i + 3).join("") });
    await sleep(35);
  }
  return true;
}

async function reply(req: AgentChatRequest, mine: number) {
  const { chatId } = req;
  const tool = (name: string, label: string) => emit({ kind: "tool", chatId, name, label });
  const done = (error: string | null = null) => {
    if (turn !== mine) return;
    running = null;
    emit({ kind: "done", chatId, error, stopped: false });
  };
  if (!req.sessionId) emit({ kind: "session", chatId, sessionId: `mock-session-${mine}` });
  await sleep(500);
  if (query("chat") === "fail") return done("Not logged in · Please run /login");
  const ask = req.message.toLowerCase();
  if (req.context) {
    tool("thread_context", "Read a conversation");
    await sleep(700);
    const t = (await mailHandlers.get_thread({ accountId: req.context.accountId, threadId: req.context.threadId })) as ThreadView | null;
    const last = t?.messages[t.messages.length - 1];
    const who = last?.from.name ?? last?.from.email ?? "the sender";
    if (/draft|reply|answer|write/.test(ask)) {
      tool("create_draft", "Wrote a draft");
      await sleep(600);
      if (!(await stream(chatId, mine, `I saved a reply to ${who} in Drafts. It thanks them, confirms the details in “${t?.subject ?? "the thread"}”, and asks one follow-up question. Open it from Drafts to review before sending.`))) return;
      return done();
    }
    const n = t?.messages.length ?? 1;
    if (!(await stream(chatId, mine, `“${t?.subject ?? "This conversation"}” has ${n} message${n === 1 ? "" : "s"}. The latest is from ${who}${last?.snippet ? `, who writes: “${last.snippet.slice(0, 110)}${last.snippet.length > 110 ? "…" : ""}”` : "."}\n\nWant me to draft a reply?`))) return;
    return done();
  }
  tool("search", "Searched mail");
  await sleep(700);
  const rows = (await mailHandlers.list_threads({ query: { view: { kind: "inbox" }, tab: null, accountId: null, limit: 3, before: null } })) as ThreadSummary[];
  const lines = rows.map((r) => `- **${r.subject || "(no subject)"}** from ${r.participants[0]?.name ?? r.participants[0]?.email ?? "someone"}`).join("\n");
  if (!(await stream(chatId, mine, `Here's what's newest in your inbox:\n${lines}\n\nAsk me about any of them, or open one and I'll read it.`))) return;
  done();
}

export const agentChatHandlers: Record<string, MockHandler> = {
  agent_chat_setup: async ({ detect: again }) => {
    if (enabled && again) found = detect();
    return setup();
  },
  agent_chat_config_set: async ({ patch }) => {
    const p = patch as AgentChatConfigPatch;
    if (p.enabled !== undefined) enabled = p.enabled;
    if (p.cli !== undefined) chosen = p.cli === "" ? null : p.cli;
    if (p.customPath !== undefined) {
      const v = p.customPath.trim();
      if (v && !v.startsWith("/") && !v.startsWith("~/")) throw { code: "invalidInput", message: "Enter the full path to the CLI, like /opt/homebrew/bin/claude." };
      customPath = v || null;
    }
    if (enabled) found = detect();
    return setup();
  },
  agent_chat_api_key_set: async ({ cli, key }) => {
    const k = (key as string | null)?.trim() ?? "";
    if (k && /\s/.test(k)) throw { code: "invalidInput", message: "That doesn't look like an API key: paste just the key, on one line." };
    keys[cli as AgentCliKind] = !!k;
    found = detect();
    return setup();
  },
  agent_chat_send: async ({ request }) => {
    const req = request as AgentChatRequest;
    const s = await setup();
    if (s.blocked) throw { code: "invalidInput", message: s.blocked };
    const mine = ++turn;
    running = req.chatId;
    void reply(req, mine);
  },
  agent_chat_stop: () => {
    const chatId = running;
    turn++;
    running = null;
    if (chatId) emit({ kind: "done", chatId, error: null, stopped: true });
    return !!chatId;
  },
};
