// The chat panel: a drawer on the right of the main window where you talk
// to your own Claude Code or Codex about your mail (src-tauri/src/agent_chat,
// docs/AGENT-CHAT.md). It names the open conversation to the agent, which
// reads it with Penguin's MCP tools; the reply streams in with a chip for
// each tool it used. ⌘⇧I or ⌘K → "Chat with Claude Code or Codex" opens it.
import { Fragment, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { api, asCommandError } from "../../lib/api";
import { useUi } from "../../lib/ui";
import { Icon } from "../../components/Icon";
import { openSettings } from "../settings/state";
import type { AgentChatSetup, ThreadRef } from "../../lib/types";
import {
  closeChat,
  contextLabel,
  installAgentChat,
  newChat,
  refreshSetup,
  sendChat,
  setChatSetup,
  setIncludeContext,
  stopChat,
  useChat,
  type ChatMessage,
} from "./state";
import { blocks, type Span } from "./model";
import "./agentChat.css";

const PRODUCT = { claude: "Claude Code", codex: "Codex" } as const;

const SUGGESTIONS_WITH_THREAD = ["Summarize this conversation", "Draft a reply for me to review", "What do they need from me?"];
const SUGGESTIONS = ["What needs my attention today?", "Find the latest invoices", "Who haven't I replied to this week?"];

export function AgentChatHost() {
  useEffect(() => installAgentChat(), []);
  const open = useChat().open;
  return open ? <AgentChatPanel /> : null;
}

function AgentChatPanel() {
  const chat = useChat();
  const selected = useUi((s) => s.selected);
  const setup = chat.setup;
  const cli = setup?.cli ? setup.found?.find((f) => f.kind === setup.cli) : undefined;
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const send = (text: string) => {
    if (!text.trim() || chat.running) return;
    void sendChat(text);
    setDraft("");
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send(draft);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeChat();
    }
  };

  const ready = setup !== null && setup.blocked === null;
  return (
    <aside className="achat" aria-label="Chat with an agent">
      <header className="achat-head">
        <Icon name="sparkles" size="sm" />
        <div className="achat-title">
          <span>Chat</span>
          {cli ? (
            <span className="achat-sub">
              {PRODUCT[cli.kind]}
              {cli.version ? ` ${cli.version}` : ""}
            </span>
          ) : null}
        </div>
        <span className="grow" />
        <button className="btn btn-ghost btn-sm btn-icon" title="New chat" aria-label="New chat" onClick={newChat} disabled={!chat.messages.length}>
          <Icon name="plus" size="sm" />
        </button>
        <button className="btn btn-ghost btn-sm btn-icon" title="Close (Esc)" aria-label="Close chat" onClick={closeChat}>
          <Icon name="x" size="sm" />
        </button>
      </header>

      <Messages messages={chat.messages} empty={ready ? <Suggestions withThread={!!selected && chat.includeContext} onPick={send} /> : <Setup setup={setup} error={chat.setupError} />} />

      <footer className="achat-foot">
        {selected ? <ContextChip refOpen={selected} on={chat.includeContext} /> : null}
        <div className="achat-input">
          <textarea
            ref={inputRef}
            rows={1}
            value={draft}
            placeholder={ready ? `Ask ${cli ? PRODUCT[cli.kind] : "the agent"} about your mail…` : "Set up the chat first"}
            disabled={!ready}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKey}
            aria-label="Message"
          />
          {chat.running ? (
            <button className="btn btn-secondary btn-sm" onClick={stopChat} title="Stop the agent">
              Stop
            </button>
          ) : (
            <button className="btn btn-primary btn-sm btn-icon" onClick={() => send(draft)} disabled={!ready || !draft.trim()} aria-label="Send" title="Send (↵)">
              <Icon name="arrowup" size="sm" />
            </button>
          )}
        </div>
        <p className="achat-note">Runs your own {cli ? PRODUCT[cli.kind] : "CLI"}, which can use Penguin as Settings → Developer → Agents allows.</p>
      </footer>
    </aside>
  );
}

function Messages({ messages, empty }: { messages: ChatMessage[]; empty: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  // Follow the stream unless the user scrolled up to read.
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [messages]);
  return (
    <div
      className="achat-body"
      ref={ref}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
    >
      {messages.length === 0 ? empty : messages.map((m) => <Message key={m.id} m={m} />)}
    </div>
  );
}

function Message({ m }: { m: ChatMessage }) {
  if (m.role === "user") {
    const text = m.parts.map((p) => (p.kind === "text" ? p.text : "")).join("");
    return (
      <div className="achat-msg is-user">
        {m.about ? (
          <span className="achat-about truncate">
            <Icon name="mail" size="2xs" /> {m.about}
          </span>
        ) : null}
        <div className="achat-bubble">{text}</div>
      </div>
    );
  }
  return (
    <div className="achat-msg is-agent" aria-live={m.pending ? "polite" : undefined}>
      {m.parts.map((p, i) =>
        p.kind === "tool" ? (
          <span key={i} className="achat-tool" title={p.name}>
            <Icon name="check" size="2xs" />
            {p.label}
          </span>
        ) : (
          <AgentText key={i} text={p.text} />
        ),
      )}
      {m.pending ? (
        <span className="achat-typing" aria-label="Working">
          <span />
          <span />
          <span />
        </span>
      ) : null}
      {m.error ? <p className="achat-error">{m.error}</p> : null}
      {m.stopped ? <p className="achat-stopped">Stopped</p> : null}
    </div>
  );
}

function SpanView({ s }: { s: Span }) {
  if (s.kind === "bold") return <strong>{s.text}</strong>;
  if (s.kind === "code") return <code>{s.text}</code>;
  return <>{s.text}</>;
}

const line = (spans: Span[]) => spans.map((s, i) => <SpanView key={i} s={s} />);

function AgentText({ text }: { text: string }) {
  return (
    <div className="achat-text">
      {blocks(text).map((b, i) => {
        switch (b.kind) {
          case "heading":
            return <h4 key={i}>{line(b.spans)}</h4>;
          case "pre":
            return <pre key={i}>{b.text}</pre>;
          case "list": {
            const items = b.items.map((it, j) => <li key={j}>{line(it)}</li>);
            return b.ordered ? <ol key={i}>{items}</ol> : <ul key={i}>{items}</ul>;
          }
          case "para":
            return (
              <p key={i}>
                {b.lines.map((l, j) => (
                  <Fragment key={j}>
                    {j > 0 ? <br /> : null}
                    {line(l)}
                  </Fragment>
                ))}
              </p>
            );
        }
      })}
    </div>
  );
}

function ContextChip({ refOpen, on }: { refOpen: ThreadRef; on: boolean }) {
  if (!on) {
    return (
      <button className="achat-chip is-off" onClick={() => setIncludeContext(true)}>
        <Icon name="plus" size="2xs" /> Ask about the open conversation
      </button>
    );
  }
  return (
    <span className="achat-chip">
      <Icon name="mail" size="2xs" />
      <span className="truncate">{contextLabel(refOpen)}</span>
      <button className="achat-chip-x" aria-label="Don't include this conversation" title="Don't include this conversation" onClick={() => setIncludeContext(false)}>
        <Icon name="x" size="2xs" />
      </button>
    </span>
  );
}

function Suggestions({ withThread, onPick }: { withThread: boolean; onPick: (s: string) => void }) {
  return (
    <div className="achat-empty">
      <p className="achat-empty-title">Ask about your mail</p>
      <p className="achat-empty-note">
        {withThread ? "The agent reads the open conversation when your question is about it." : "The agent searches and reads your mail through Penguin."}
      </p>
      <div className="achat-suggest">
        {(withThread ? SUGGESTIONS_WITH_THREAD : SUGGESTIONS).map((s) => (
          <button key={s} className="achat-suggestion" onClick={() => onPick(s)}>
            {s}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Why the chat can't start, and the one thing to do about it. */
function Setup({ setup, error }: { setup: AgentChatSetup | null; error: string | null }) {
  const [busy, setBusy] = useState(false);
  if (error) return <p className="achat-error achat-setup">{error}</p>;
  if (!setup) return <p className="achat-setup achat-empty-note">Checking for Claude Code and Codex…</p>;
  const turnOn = async () => {
    setBusy(true);
    try {
      setChatSetup(await api.agentChatConfigSet({ enabled: true }));
    } catch (e) {
      setChatSetup({ ...setup, blocked: asCommandError(e).message });
    } finally {
      setBusy(false);
    }
  };
  const action = !setup.enabled ? (
    <button className="btn btn-primary btn-sm" disabled={busy} onClick={() => void turnOn()}>
      {busy ? "Looking for Claude Code and Codex…" : "Turn on and find my CLI"}
    </button>
  ) : setup.access === "off" ? (
    <button className="btn btn-secondary btn-sm" onClick={() => openSettings("diagnostics")}>
      Open Settings → Developer
    </button>
  ) : (
    <div className="row-flex">
      <button className="btn btn-secondary btn-sm" onClick={() => openSettings("ai")}>
        Open Settings → AI
      </button>
      <button className="btn btn-ghost btn-sm" onClick={() => void refreshSetup(true)}>
        Search again
      </button>
    </div>
  );
  return (
    <div className="achat-empty achat-setup">
      <p className="achat-empty-title">Chat with Claude Code or Codex</p>
      <p className="achat-empty-note">
        {setup.enabled
          ? setup.blocked
          : "Penguin can run the Claude Code or Codex you already use, with your mail as its tools. Penguin looks for them on this Mac when you turn this on."}
      </p>
      {action}
    </div>
  );
}
