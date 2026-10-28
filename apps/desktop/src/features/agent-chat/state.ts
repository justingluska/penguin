// The chat panel's state: open or not, the conversation so far, the CLI's
// session, and the setup (src-tauri/src/agent_chat). One chat at a time;
// "New chat" starts a fresh session. The open mail conversation is named to
// the agent by id when it changes, so follow-ups stay about what you're
// reading. Nothing here is persisted: a chat lasts until the app quits.
import { useSyncExternalStore } from "react";
import { api, asCommandError, onAgentChat } from "../../lib/api";
import { getUi } from "../../lib/ui";
import { cachedThread, listSummary } from "../../app/store";
import type { AgentChatContext, AgentChatSetup, ThreadRef } from "../../lib/types";
import { applyEvent, type ChatMessage, type ChatState } from "./model";

export type { ChatMessage, ChatPart, ChatState } from "./model";

let seq = 0;
const newChatId = () => `chat-${Date.now().toString(36)}-${(++seq).toString(36)}`;

let state: ChatState = {
  open: false,
  chatId: newChatId(),
  sessionId: null,
  messages: [],
  running: false,
  includeContext: true,
  toldContext: null,
  setup: null,
  setupError: null,
};
const subs = new Set<() => void>();
function set(patch: Partial<ChatState> | ((s: ChatState) => Partial<ChatState>)) {
  state = { ...state, ...(typeof patch === "function" ? patch(state) : patch) };
  subs.forEach((f) => f());
}

export function getChat(): ChatState {
  return state;
}

export function useChat(): ChatState {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => state,
  );
}

export async function refreshSetup(detect = false): Promise<AgentChatSetup | null> {
  try {
    const setup = await api.agentChatSetup(detect);
    set({ setup, setupError: null });
    return setup;
  } catch (e) {
    set({ setupError: asCommandError(e).message });
    return null;
  }
}

export function setChatSetup(setup: AgentChatSetup) {
  set({ setup, setupError: null });
}

export function openChat() {
  set({ open: true });
  void refreshSetup();
}

export function closeChat() {
  set({ open: false });
}

export function toggleChat() {
  if (state.open) closeChat();
  else openChat();
}

/** Forget this chat (stopping a running turn) and start another. */
export function newChat() {
  if (state.running) void api.agentChatStop().catch(() => {});
  set({ chatId: newChatId(), sessionId: null, messages: [], running: false, toldContext: null, includeContext: true });
}

export function setIncludeContext(on: boolean) {
  set({ includeContext: on });
}

/** The conversation open in the main window, if any. */
export function currentRef(): ThreadRef | null {
  return getUi().selected;
}

/** What the context chip says for `ref`. */
export function contextLabel(ref: ThreadRef): string {
  const subject = cachedThread(ref)?.subject ?? listSummary(ref)?.subject ?? "";
  return subject.trim() || "This conversation";
}

const refKey = (r: ThreadRef | null) => (r ? `${r.accountId}/${r.threadId}` : "");

let nextId = 0;

function patchAgent(f: (m: ChatMessage) => ChatMessage) {
  set((s) => {
    const last = s.messages[s.messages.length - 1];
    if (!last || last.role !== "agent") return {};
    return { messages: [...s.messages.slice(0, -1), f(last)] };
  });
}

/** Ask the agent `text`, about the open conversation unless the chip was removed. */
export async function sendChat(text: string): Promise<void> {
  const message = text.trim();
  if (!message || state.running) return;
  const ref = state.includeContext ? currentRef() : null;
  const context: AgentChatContext | null = ref ? { accountId: ref.accountId, threadId: ref.threadId } : null;
  const key = refKey(ref);
  const contextChanged = state.toldContext === null || state.toldContext !== key;
  const chatId = state.chatId;
  set((s) => ({
    running: true,
    messages: [
      ...s.messages,
      { id: ++nextId, role: "user", parts: [{ kind: "text", text: message }], about: ref ? contextLabel(ref) : null },
      { id: ++nextId, role: "agent", parts: [], pending: true },
    ],
  }));
  try {
    await api.agentChatSend({ chatId, message, sessionId: state.sessionId, context, contextChanged });
    if (state.chatId === chatId) set({ toldContext: key });
  } catch (e) {
    if (state.chatId !== chatId) return;
    patchAgent((m) => ({ ...m, pending: false, error: asCommandError(e).message }));
    set({ running: false });
    void refreshSetup();
  }
}

export function stopChat() {
  if (state.running) void api.agentChatStop().catch(() => {});
}

let installed = false;
/** Listen for chat events once, from the main window. */
export function installAgentChat(): () => void {
  if (installed) return () => {};
  installed = true;
  const un = onAgentChat((e) => set((s) => applyEvent(s, e)));
  return () => {
    installed = false;
    void un.then((f) => f());
  };
}
