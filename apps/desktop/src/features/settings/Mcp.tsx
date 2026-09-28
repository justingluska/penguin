// Settings → Developer → MCP server and command-line tool. The MCP server
// (`penguin-cli mcp`) lets AI tools read the local mail index, read-only; the
// backend (mcp_info, settings.mcp, cli_install_status, install_cli) belongs to
// app-integration. This is the UI. OWNER: settings agent.
import { useCallback, useEffect, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { updateSettings, useSetting } from "../../lib/settings";
import type { CliLinkStatus, McpInfo } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { toast } from "../../components/Toast";
import { Switch } from "./parts";

export function McpPanel() {
  const mcp = useSetting("mcp");
  const [info, setInfo] = useState<McpInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setInfo(await api.mcpInfo());
      setError(null);
    } catch (e) {
      setError(asCommandError(e).message);
    }
  }, []);

  // Refetch when the toggle changes: `enabled` and the snippets follow it.
  useEffect(() => {
    void load();
  }, [load, mcp.enabled]);

  const toggle = (enabled: boolean) =>
    updateSettings({ mcp: { ...mcp, enabled } }).catch((e) =>
      toast({ tone: "error", message: `Couldn't save: ${asCommandError(e).message}` }),
    );

  return (
    <div className="st-mcp">
      <h3 className="st-sub">AI tools (MCP)</h3>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Enable MCP server</span>
          <p className="st-muted">
            Lets AI tools like Claude read your local mail index. Read-only: it can't send, delete, or change mail.
          </p>
          <p className="st-muted">
            After changing this, restart or reconnect your MCP client (Claude Desktop, or <span className="mono">/mcp</span> in
            Claude Code).
          </p>
        </div>
        <Switch label="Enable MCP server" on={mcp.enabled} onChange={toggle} />
      </div>
      {error ? <p className="st-error">Couldn't read the MCP setup: {error}</p> : null}
      {mcp.enabled && info ? (
        <div className="st-mcp-body">
          {info.cliPath == null ? (
            <p className="st-mcp-warn">
              <Icon name="info" size="xs" />
              Build penguin-cli first: this build doesn't include it, so the commands below won't run yet.
            </p>
          ) : null}
          <Snippet label="Claude Code" text={info.claudeCodeCommand} />
          <Snippet label="Claude Desktop (claude_desktop_config.json)" text={info.claudeDesktopConfig} />
          {info.auditLogPath ? (
            <p className="st-muted">
              Tool calls are logged (names and counts only) to <span className="mono st-mcp-path">{info.auditLogPath}</span>
            </p>
          ) : null}
        </div>
      ) : null}
      <CliInstallRow />
    </div>
  );
}

/** "Command-line tool: penguin" → symlink ~/.local/bin/penguin to the bundled CLI. */
function CliInstallRow() {
  const [status, setStatus] = useState<CliLinkStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // cli_install_status runs the login shell (up to 3 s): once per open.
  useEffect(() => {
    let live = true;
    api.cliInstallStatus().then(
      (s) => live && setStatus(s),
      (e) => live && setError(asCommandError(e).message),
    );
    return () => {
      live = false;
    };
  }, []);

  const install = async () => {
    setBusy(true);
    try {
      const s = await api.installCli();
      setStatus(s);
      setError(null);
      toast({ message: `Installed penguin at ${s.linkPath}` });
    } catch (e) {
      setError(asCommandError(e).message);
    } finally {
      setBusy(false);
    }
  };

  const noCli = status != null && status.target == null;
  return (
    <>
      <div className="setting-row setting-tall">
        <div className="min0">
          <span className="setting-label">
            Command-line tool: <span className="mono">penguin</span>
          </span>
          <p className="st-muted">
            {noCli
              ? "This build has no penguin-cli."
              : status
                ? `Links ${status.linkPath} to the tool inside Penguin. No admin needed.`
                : "Checking…"}
          </p>
          {status?.conflict ? <p className="st-muted is-warn">{status.conflict}</p> : null}
          {error ? <p className="st-error">{error}</p> : null}
        </div>
        {status?.installed && !status.conflict ? (
          <span className="st-installed">
            <Icon name="check" size="xs" />
            Installed
          </span>
        ) : (
          <button className="btn btn-secondary btn-sm" disabled={!status || noCli || busy} onClick={() => void install()}>
            {busy ? "Installing…" : status?.conflict ? "Replace link" : "Install"}
          </button>
        )}
      </div>
      {status?.installed && status.onPath !== true ? (
        <div className="st-mcp-body">
          <p className="st-muted">
            {status.onPath === false ? "~/.local/bin isn't on your PATH. Run:" : "Couldn't check your PATH. If `penguin` isn't found, run:"}
          </p>
          <Snippet label="Terminal" text={status.pathLine} />
        </div>
      ) : null}
    </>
  );
}

function Snippet({ label, text }: { label: string; text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      toast({ tone: "error", message: `Couldn't copy: ${asCommandError(e).message}` });
    }
  };
  return (
    <div className="st-snippet">
      <div className="st-snippet-head">
        <span>{label}</span>
        <button className="btn btn-ghost btn-sm" onClick={() => void copy()}>
          <Icon name={copied ? "check" : "sheet"} size="xs" />
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre className="st-snippet-code mono">{text}</pre>
    </div>
  );
}
