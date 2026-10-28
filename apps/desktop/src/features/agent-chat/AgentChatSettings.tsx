// Settings → AI → "Chat with Claude Code or Codex": off by default. Turning
// it on looks for the user's own CLIs on this Mac (src-tauri/src/agent_chat
// /detect.rs) and shows what it found; the user picks one, or types its
// path, and may save an API key for a CLI that isn't signed in. The key goes
// to the Keychain and only ever to that CLI's process.
import { useEffect, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { toast } from "../../components/Toast";
import { Icon } from "../../components/Icon";
import { Switch } from "../settings/parts";
import { openSettings } from "../settings/state";
import type { AgentChatConfigPatch, AgentChatSetup, AgentCliFound, AgentCliKind } from "../../lib/types";
import { refreshSetup, setChatSetup, useChat } from "./state";
import "./agentChat.css";

const PRODUCT: Record<AgentCliKind, string> = { claude: "Claude Code", codex: "Codex" };
const PROVIDER: Record<AgentCliKind, string> = { claude: "Anthropic", codex: "OpenAI" };
const KEY_VAR: Record<AgentCliKind, string> = { claude: "ANTHROPIC_API_KEY", codex: "CODEX_API_KEY" };
export const CHAT_LABEL = "Chat with Claude Code or Codex";

function where(f: AgentCliFound): string {
  return f.foundIn === "custom" ? "the path you entered" : f.foundIn === "path" ? "your PATH" : "its usual install folder";
}

export function AgentChatSettings() {
  const setup = useChat().setup;
  const [busy, setBusy] = useState<string | null>(null);
  useEffect(() => void refreshSetup(), []);

  const run = async (what: string, f: () => Promise<AgentChatSetup>) => {
    setBusy(what);
    try {
      setChatSetup(await f());
    } catch (e) {
      toast({ tone: "error", message: `Couldn't save: ${asCommandError(e).message}` });
    } finally {
      setBusy(null);
    }
  };
  const patch = (p: AgentChatConfigPatch, what = "save") => run(what, () => api.agentChatConfigSet(p));

  const on = setup?.enabled ?? false;
  return (
    <div className="st-achat">
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Chat with Claude Code or Codex</span>
          <p className="st-muted">
            Talk to the Claude Code or Codex you already use about your mail, in a panel beside it (⌘⇧I). Penguin runs your CLI with
            Penguin as its only tools, so what you ask and the mail it reads go to {setup?.cli ? PROVIDER[setup.cli] : "Anthropic or OpenAI"}{" "}
            through your own account. Off by default.
          </p>
        </div>
        <Switch label={CHAT_LABEL} on={on} disabled={busy !== null} onChange={(enabled) => void patch({ enabled }, enabled ? "detect" : "save")} />
      </div>
      {on && setup ? <Details setup={setup} busy={busy} patch={patch} run={run} /> : null}
      {busy === "detect" ? <p className="st-muted st-achat-note">Looking for Claude Code and Codex on this Mac…</p> : null}
    </div>
  );
}

function Details({
  setup,
  busy,
  patch,
  run,
}: {
  setup: AgentChatSetup;
  busy: string | null;
  patch: (p: AgentChatConfigPatch, what?: string) => Promise<void>;
  run: (what: string, f: () => Promise<AgentChatSetup>) => Promise<void>;
}) {
  const found = setup.found ?? [];
  const [path, setPath] = useState(setup.customPath ?? "");
  const [key, setKey] = useState("");
  const kind: AgentCliKind = setup.cli ?? setup.chosen ?? "claude";
  const keySaved = kind === "claude" ? setup.claudeKeySaved : setup.codexKeySaved;
  return (
    <>
      {setup.access === "off" ? (
        <p className="st-warn st-achat-note">
          Agents are off, so the chat can't read your mail.{" "}
          <button className="st-link" onClick={() => openSettings("diagnostics")}>
            Choose Read only or higher in Settings → Developer → Agents
          </button>
          .
        </p>
      ) : null}
      <div className="setting-row setting-tall">
        <div className="grow">
          <span className="setting-label">CLI to chat with</span>
          {found.length === 0 ? (
            <p className="st-muted">Penguin didn't find Claude Code or Codex. Install one, or enter where it is below.</p>
          ) : (
            <div className="st-achat-found" role="radiogroup" aria-label="CLI to chat with">
              {found.map((f) => {
                const picked = setup.cli === f.kind;
                return (
                  <button
                    key={f.kind}
                    role="radio"
                    aria-checked={picked}
                    className={"st-agent-level" + (picked ? " is-on" : "")}
                    onClick={() => void patch({ cli: f.kind })}
                  >
                    <span className="st-agent-radio" aria-hidden />
                    <span className="st-agent-level-text">
                      <span className="st-agent-level-title">
                        Found {PRODUCT[f.kind]}
                        {f.version ? ` ${f.version}` : ""}
                        {f.signedIn === true ? <span className="badge t-green">Signed in</span> : f.signedIn === false ? <span className="badge t-amber">May need signing in</span> : null}
                      </span>
                      <span className="st-muted mono truncate" title={f.path}>
                        {f.path} · on {where(f)}
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
        <button className="btn btn-secondary btn-sm" disabled={busy !== null} onClick={() => void run("detect", () => api.agentChatSetup(true))}>
          <Icon name="refresh" size="xs" /> Search again
        </button>
      </div>
      <div className="setting-row setting-tall">
        <div className="grow">
          <span className="setting-label">Path to the CLI</span>
          <p className="st-muted">Only if Penguin doesn't find it, e.g. /opt/homebrew/bin/claude. For {PRODUCT[setup.chosen ?? "claude"]}.</p>
        </div>
        <input
          className="input share-input mono st-achat-input"
          placeholder="Found automatically"
          spellCheck={false}
          autoComplete="off"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void patch({ customPath: path })}
        />
        <button className="btn btn-secondary btn-sm" disabled={busy !== null || path === (setup.customPath ?? "")} onClick={() => void patch({ customPath: path })}>
          Save
        </button>
      </div>
      <div className="setting-row setting-tall">
        <div className="grow">
          <span className="setting-label">API key</span>
          <p className="st-muted">
            Only if {PRODUCT[kind]} isn't signed in on this Mac. Kept in the macOS Keychain and given only to {PRODUCT[kind]}, as {KEY_VAR[kind]}.
            {keySaved ? " A key is saved." : ""}
          </p>
        </div>
        <input
          className="input share-input mono st-achat-input"
          type="password"
          placeholder={keySaved ? "Saved in the Keychain" : `${PROVIDER[kind]} API key`}
          spellCheck={false}
          autoComplete="off"
          value={key}
          onChange={(e) => setKey(e.target.value)}
        />
        {keySaved && !key ? (
          <button className="btn btn-ghost btn-sm" disabled={busy !== null} onClick={() => void run("key", () => api.agentChatApiKeySet(kind, null))}>
            Remove
          </button>
        ) : (
          <button
            className="btn btn-secondary btn-sm"
            disabled={busy !== null || !key.trim()}
            onClick={() =>
              void run("key", async () => {
                const s = await api.agentChatApiKeySet(kind, key);
                setKey("");
                return s;
              })
            }
          >
            Save
          </button>
        )}
      </div>
    </>
  );
}
