// "Also search Gmail" and the coverage note for the search overlay.
// OWNER: sync-window agent (mounted by features/search/index.tsx).
//
// Local search covers full text inside the sync window and headers/previews
// for older mail (per Settings → Sync). This asks Gmail's servers instead:
// matches it finds are stored locally (headers-only) and shown in their own
// "From Gmail" group; afterwards they're local and show up in normal search.
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { api, asCommandError } from "../../lib/api";
import type { SearchHit, SearchResponse, ServerSearchResponse, SyncCoverage } from "../../lib/types";
import { num } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { windowLabel } from "../settings/Sync";
import "./server.css";
export { serverName } from "./serverName";

export type ServerSearchState =
  | { status: "idle" }
  | { status: "loading"; query: string }
  | { status: "done"; query: string; resp: ServerSearchResponse }
  | { status: "error"; query: string; message: string };

/**
 * Server search for the current query; resets whenever the query or scope
 * changes. `initial` restores an earlier state (Back to a search), kept until
 * the query or scope actually changes.
 */
export function useServerSearch(query: string, scopeIds: string[] | null, initial?: ServerSearchState) {
  const [state, setState] = useState<ServerSearchState>(initial ?? { status: "idle" });
  const seq = useRef(0);
  const scopeKey = scopeIds?.join("\u0000") ?? "";
  const restored = useRef(!!initial);
  useEffect(() => {
    if (restored.current) {
      restored.current = false;
      return;
    }
    seq.current++;
    setState({ status: "idle" });
  }, [query, scopeKey]);
  const run = useCallback(() => {
    const q = query.trim();
    if (!q) return;
    const id = ++seq.current;
    setState({ status: "loading", query: q });
    api.searchServer(q, scopeIds).then(
      (resp) => id === seq.current && setState({ status: "done", query: q, resp }),
      (e) => id === seq.current && setState({ status: "error", query: q, message: asCommandError(e).message }),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, scopeKey]);
  return { state, run };
}

/** What's downloaded where (for the footer note and the suggestion). */
export function useCoverage(scopeIds: string[] | null): SyncCoverage | null {
  const [cov, setCov] = useState<SyncCoverage | null>(null);
  const scopeKey = scopeIds?.join("\u0000") ?? "";
  useEffect(() => {
    let live = true;
    api.syncCoverage(scopeIds).then(
      (c) => live && setCov(c),
      () => {},
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeKey]);
  return cov;
}

/** "Full text: last 6 months · headers: older" (or "Full text: all mail"). */
export function coverageText(cov: SyncCoverage): string {
  if (cov.windowMonths === 0) return "Full text: all mail";
  const window = `Full text: last ${windowLabel(cov.windowMonths).toLowerCase()}`;
  const older = cov.olderMail === "none" ? "older: not downloaded" : cov.olderMail === "full" ? "older: full" : "headers: older";
  const catching = cov.accounts.some((a) => !a.windowComplete) ? " (still downloading)" : "";
  return `${window}${catching} · ${older}`;
}

/**
 * Offer Gmail when local search came up empty and the query can reach mail
 * outside the fully-downloaded window.
 */
export function shouldSuggestServer(resp: SearchResponse | null, cov: SyncCoverage | null, localHits: number): boolean {
  if (!resp || !cov || localHits > 0) return false;
  if (cov.windowMonths === 0 && cov.accounts.every((a) => a.windowComplete)) return false;
  const after = resp.afterMs ?? null;
  return after == null || after < cov.windowStartMs;
}

export function ServerSearchButton({ state, onRun, suggest, server = "Gmail" }: { state: ServerSearchState; onRun: () => void; suggest: boolean; server?: string }) {
  const loading = state.status === "loading";
  return (
    <button
      className={"btn btn-sm " + (suggest && state.status === "idle" ? "btn-primary" : "btn-ghost")}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onRun}
      disabled={loading}
      title={`Search ${server} too, for mail that isn't fully downloaded on this Mac`}
    >
      {loading ? <span className="spinner" aria-hidden="true" /> : <Icon name="cloud" size="xs" />}
      {loading ? `Searching ${server}…` : `Also search ${server}`}
      <span className="kbd">⌘⇧↵</span>
    </button>
  );
}

/**
 * The "From Gmail (older mail)" group. `localKeys` holds `account/thread`
 * of local hits so threads already shown above aren't repeated.
 */
export function ServerResultsGroup({
  state,
  localKeys,
  renderHit,
  server = "Gmail",
}: {
  server?: string;
  state: ServerSearchState;
  localKeys: Set<string>;
  renderHit: (hit: SearchHit) => ReactNode;
}) {
  if (state.status === "idle") return null;
  const head = (right: ReactNode) => (
    <div className="group-head">
      <span>
        From {server} <span className="gh-count">older mail</span>
      </span>
      <span className="gh-hint">{right}</span>
    </div>
  );
  if (state.status === "loading") {
    return (
      <>
        {head(
          <>
            <span className="spinner" aria-hidden="true" /> Searching Gmail…
          </>,
        )}
      </>
    );
  }
  if (state.status === "error") {
    return (
      <>
        {head("")}
        <div className="sx-error">
          <Icon name="info" size="sm" />
          <span>Gmail search failed: {state.message}</span>
        </div>
      </>
    );
  }
  const extra = state.resp.hits.filter((h) => !localKeys.has(`${h.accountId}/${h.threadId}`));
  const failed = state.resp.accounts.filter((a) => a.error);
  const estimate = state.resp.accounts.reduce((n, a) => n + a.estimate, 0);
  return (
    <>
      {head(
        extra.length === 0
          ? "Nothing more on Gmail"
          : `${num(extra.length)} more${estimate > state.resp.hits.length ? ` of ~${num(estimate)}` : ""} · saved on this Mac`,
      )}
      {extra.length > 0 && <div className="hits">{extra.map(renderHit)}</div>}
      {state.resp.approximate && <div className="sx-note faint">Gmail can't express every filter here, so these may be broader.</div>}
      {failed.length > 0 && (
        <div className="sx-note faint">
          Couldn't reach Gmail for {failed.length === 1 ? "1 account" : `${failed.length} accounts`}: {failed[0].error}
        </div>
      )}
    </>
  );
}
