// Developer / Diagnostics: where Penguin keeps things, how big they are, and
// how each account's sync is doing. Counts, sizes and paths only.
// OWNER: settings agent.
import { useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { Diagnostics, QuotaStats, RevealTarget, TableSizes } from "../../lib/types";
import { ago, bytes, num } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { toast } from "../../components/Toast";
import { Section, phaseLabel } from "./parts";
import { GmailQuotaField } from "./GmailQuota";
import { McpPanel } from "./Mcp";
import { RuleHooksPanel } from "../rules/RulesSection";
import { openLogViewer } from "../logs/LogViewer";
import { DemoModeRow } from "./DemoMode";

export function DiagnosticsPanel({
  diag,
  error,
  onRefresh,
}: {
  diag: Diagnostics | null;
  error: string | null;
  onRefresh: () => Promise<void>;
}) {
  const [tables, setTables] = useState<TableSizes | null>(null);
  const [measuring, setMeasuring] = useState(false);

  const measure = async () => {
    setMeasuring(true);
    try {
      setTables(await api.diagnosticsTableSizes());
    } catch (e) {
      toast({ tone: "error", message: `Couldn't measure tables: ${asCommandError(e).message}` });
    } finally {
      setMeasuring(false);
    }
  };

  const reveal = (target: RevealTarget) =>
    api.revealPath(target).catch((e) => toast({ tone: "error", message: asCommandError(e).message }));

  const copy = async () => {
    if (!diag) return;
    try {
      await navigator.clipboard.writeText(JSON.stringify(redacted(diag, tables), null, 2));
      toast({ message: "Diagnostics copied" });
    } catch (e) {
      toast({ tone: "error", message: `Couldn't copy: ${asCommandError(e).message}` });
    }
  };

  return (
    <Section
      id="diagnostics"
      icon="database"
      title="Developer"
      className="st-diag"
      badge={
        <span className="st-diag-actions">
          {diag ? <span className="st-muted tnum">read in {diag.tookMs.toFixed(1)} ms</span> : null}
          <button className="btn btn-ghost btn-sm" onClick={() => void onRefresh()}>
            <Icon name="refresh" size="xs" />
            Refresh
          </button>
          <button className="btn btn-secondary btn-sm" disabled={!diag} onClick={() => void copy()}>
            <Icon name="sheet" size="xs" />
            Copy diagnostics
          </button>
        </span>
      }
    >
      <DemoModeRow />
      <GmailQuotaField diag={diag} />
      <McpPanel />
      <RuleHooksPanel />
      <h3 className="st-sub">Diagnostics</h3>
      {error ? <p className="st-error">Couldn't read diagnostics: {error}</p> : null}
      {!diag ? (
        <p className="st-muted">Reading…</p>
      ) : (
        <>
          <div className="st-diag-grid">
            <dl className="st-kv">
              <Row k="Version" v={diag.appVersion} />
              <Row k="OAuth client" v={diag.oauthClientIdTail ? `…${diag.oauthClientIdTail}` : "not configured"} mono />
              <Row k="Messages" v={num(diag.totalMessages)} />
              <Row k="Threads" v={num(diag.totalThreads)} />
              <Row k="Trackers removed" v={`${num(diag.trackersRemovedSession)} this session`} />
            </dl>
            <dl className="st-kv">
              <Row k="penguin.db" v={bytes(diag.dbBytes)} />
              <Row k="WAL" v={bytes(diag.walBytes)} />
              <Row
                k="Pages"
                v={`${num(diag.pageCount)} × ${bytes(diag.pageSize)} · ${num(diag.freelistCount)} free`}
              />
              <Row k="Inline images" v={bytes(diag.inlineCacheBytes)} />
              <Row k="Log" v={bytes(diag.logBytes)} />
            </dl>
          </div>

          <h3 className="st-sub">Locations</h3>
          <dl className="st-kv st-paths">
            <PathRow k="Data" path={diag.dataDir} onReveal={() => void reveal("data")} />
            {diag.configDir !== diag.dataDir ? (
              <PathRow k="Config" path={diag.configDir} onReveal={() => void reveal("config")} />
            ) : null}
            <PathRow k="Cache" path={diag.cacheDir} onReveal={() => void reveal("cache")} />
            {diag.logDir ? <PathRow k="Logs" path={diag.logDir} onReveal={() => void reveal("log")} /> : null}
          </dl>
          <div className="st-log-row">
            <button className="btn btn-secondary btn-sm" onClick={openLogViewer}>
              <Icon name="file" size="xs" />
              View log
            </button>
            <span className="st-muted">Everything that went wrong: failed actions, error messages, sync problems. Ids and error text only, never mail content.</span>
          </div>

          <h3 className="st-sub">Accounts</h3>
          <div className="st-table-wrap">
            <table className="st-table tnum">
              <thead>
                <tr>
                  <th>Account</th>
                  <th>Phase</th>
                  <th className="num" title="Messages stored locally; headers-only ones (outside the sync window) have no body">Stored</th>
                  <th className="num">Gmail total</th>
                  <th className="num">msgs/min</th>
                  <th>Backfill</th>
                  <th>History</th>
                  <th className="num">Failed</th>
                  <th>Keychain</th>
                  <th className="num" title="This account's messages.get calls in the last minute, and its fair share of the quota budget">Gets/min · share</th>
                  <th className="num">Images</th>
                  <th>Last sync</th>
                </tr>
              </thead>
              <tbody>
                {diag.accounts.map((a) => (
                  <tr key={a.accountId} title={a.error ?? undefined}>
                    <td className="st-cell-email">{a.email}</td>
                    <td className={a.phase === "error" || a.phase === "needsReauth" ? "is-bad" : ""}>{phaseLabel(a.phase)}</td>
                    <td
                      className="num"
                      title={`${num(a.messagesStored - a.headersOnlyMessages)} full · ${num(a.headersOnlyMessages)} headers only`}
                    >
                      {num(a.messagesStored)}
                      {a.headersOnlyMessages ? (
                        <span className="st-muted"> · {num(a.headersOnlyMessages)} hdr</span>
                      ) : null}
                    </td>
                    <td className="num">
                      {a.gmailTotal == null ? "–" : num(a.gmailTotal)}
                      {a.gmailTotal ? <span className="st-muted"> · {pct(a.messagesStored, a.gmailTotal)}</span> : null}
                    </td>
                    <td className="num">{a.msgsPerMinute == null ? "–" : num(Math.round(a.msgsPerMinute))}</td>
                    <td>{a.backfillDone ? "done" : "running"}</td>
                    <td>{a.historyIdPresent ? "yes" : "no"}</td>
                    <td className={"num" + (a.failedMessageIds ? " is-bad" : "")}>{num(a.failedMessageIds)}</td>
                    <td className={a.keychain === "missing" ? "is-bad" : ""}>{a.keychain}</td>
                    <td
                      className="num"
                      title={
                        a.quota
                          ? a.quota.scope === "perAccount"
                            ? `Own budget: ${budgetLine(a.quota)}`
                            : `Fair share of the shared budget across ${a.quota.activeAccounts} active account(s)`
                          : "No Gmail calls yet"
                      }
                    >
                      {a.quota ? `${num(a.quota.accountGetsLastMin)} · ${a.quota.accountGetsPerSec.toFixed(1)}/s` : "–"}
                    </td>
                    <td className="num">{bytes(a.inlineCacheBytes)}</td>
                    <td>{a.lastSyncedAt ? ago(a.lastSyncedAt) : "never"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <QuotaLine diag={diag} />

          <div className="st-sub-row">
            <h3 className="st-sub">Tables and indexes</h3>
            <span className="grow" />
            {tables ? <span className="st-muted tnum">measured in {tables.tookMs.toFixed(0)} ms</span> : null}
            <button className="btn btn-ghost btn-sm" disabled={measuring} onClick={() => void measure()}>
              <Icon name="database" size="xs" />
              {measuring ? "Measuring…" : tables ? "Measure again" : "Measure sizes"}
            </button>
          </div>
          {tables ? (
            <TableBars tables={tables} />
          ) : (
            <p className="st-muted">Reads every page of the database (dbstat), so it runs only when you ask.</p>
          )}
        </>
      )}
    </Section>
  );
}

/** "6,000 u/min · 452 gets/min · 12.1 u/get · 1 throttle" for one budget. */
function budgetLine(q: QuotaStats): string {
  const throttles = q.throttleEpisodes === 1 ? "1 throttle" : `${num(q.throttleEpisodes)} throttles`;
  return (
    `${num(Math.round(q.unitsLastMin))} of ${num(q.unitsPerMin)} u/min used · ${num(q.getsLastMin)} gets/min · ` +
    `${q.getCost.toFixed(1)} u/get · max ${q.getsPerSec.toFixed(1)} gets/s · ${throttles}`
  );
}

/** The Gmail quota budget, once: it is shared by every account (the default). */
function QuotaLine({ diag }: { diag: Diagnostics }) {
  const shared = diag.accounts.map((a) => a.quota).find((q) => q?.scope === "shared");
  if (!shared) return null;
  return (
    <p className={"st-muted st-quota tnum" + (shared.throttleEpisodes ? " is-warn" : "")}>
      Gmail quota (shared by {shared.activeAccounts === 1 ? "1 active account" : `${shared.activeAccounts} active accounts`}):{" "}
      {budgetLine(shared)}
    </p>
  );
}

function Row({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="st-kv-row">
      <dt>{k}</dt>
      <dd className={"tnum" + (mono ? " mono" : "")}>{v}</dd>
    </div>
  );
}

function PathRow({ k, path, onReveal }: { k: string; path: string; onReveal: () => void }) {
  return (
    <div className="st-kv-row">
      <dt>{k}</dt>
      <dd className="mono st-path" title={path}>
        {path}
      </dd>
      <button className="btn btn-ghost btn-sm" onClick={onReveal}>
        Reveal in Finder
      </button>
    </div>
  );
}

function TableBars({ tables }: { tables: TableSizes }) {
  const max = Math.max(1, ...tables.tables.map((t) => t.bytes));
  const total = tables.tables.reduce((n, t) => n + t.bytes, 0);
  return (
    <div className="st-bars">
      {tables.tables.map((t) => (
        <div className="st-bar-row" key={t.name}>
          <span className="st-bar-name mono">{t.name}</span>
          <span className={"badge st-kind t-" + (t.kind === "fts" ? "violet" : t.kind === "index" ? "blue" : "gray")}>{t.kind}</span>
          <span className="st-bar">
            <span style={{ width: `${(t.bytes / max) * 100}%` }} />
          </span>
          <span className="st-bar-size tnum">{bytes(t.bytes)}</span>
          <span className="st-bar-pct tnum st-muted">{pct(t.bytes, total)}</span>
        </div>
      ))}
    </div>
  );
}

function pct(part: number, whole: number): string {
  if (!whole) return "–";
  const p = (part / whole) * 100;
  return p >= 10 || p === 0 ? `${Math.round(p)}%` : `${p.toFixed(1)}%`;
}

/**
 * The clipboard copy: counts, sizes and paths. Account addresses are reduced
 * to their domain so a pasted report doesn't carry mailbox names.
 */
function redacted(d: Diagnostics, tables: TableSizes | null) {
  return {
    ...d,
    accounts: d.accounts.map((a, i) => ({
      ...a,
      accountId: `account-${i + 1}`,
      email: `…@${a.email.split("@")[1] ?? "?"}`,
      error: a.error ? "(present)" : null,
    })),
    tables: tables?.tables ?? null,
    copiedAt: new Date().toISOString(),
  };
}
