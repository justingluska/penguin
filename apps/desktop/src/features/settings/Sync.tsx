// Settings → Sync: how much mail is downloaded in full (the sync window),
// what happens to older mail, per-account coverage, and "Free up space".
// OWNER: sync-window agent. Moving the slider or changing "Older mail" only
// stages a value (with a preview line); SyncConfirm spells out the cost and
// nothing is saved until it's confirmed. Cancel puts the slider back.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { updateSettings, useSettings } from "../../lib/settings";
import {
  SYNC_WINDOW_CHOICES,
  type OlderMail,
  type SyncCoverage,
  type SyncWindowMonths,
  type WindowEstimate,
} from "../../lib/types";
import { bytes, eta, num } from "../../lib/format";
import { meta } from "../../app/store";
import { accountName } from "../../components/Identity";
import { toast } from "../../components/Toast";
import { Choice, ConfirmDialog, Section } from "./parts";
import { SyncConfirm, windowLabel, windowPhrase, windowRank, type PendingChange } from "./SyncConfirm";
import "./sync.css";

export { windowLabel, windowPhrase } from "./SyncConfirm";

/** Slider order: shortest first, "everything" last. */
const STOPS: SyncWindowMonths[] = [...SYNC_WINDOW_CHOICES];

/**
 * Estimates per window, fetched once per value while Settings is open
 * (the backend caches Gmail's answer for 30 minutes as well).
 */
function useEstimateCache(wanted: SyncWindowMonths[]) {
  const cache = useRef(new Map<SyncWindowMonths, WindowEstimate[]>());
  const asked = useRef(new Set<SyncWindowMonths>());
  const [, bump] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const key = wanted.join(",");
  useEffect(() => {
    let live = true;
    // Debounced so dragging across stops doesn't ask for each one.
    const t = setTimeout(() => {
      for (const m of wanted) {
        if (asked.current.has(m)) continue;
        asked.current.add(m);
        api.syncWindowEstimate(m).then(
          (rows) => {
            cache.current.set(m, rows);
            if (live) bump((n) => n + 1);
          },
          (e) => {
            asked.current.delete(m);
            if (live) setError(asCommandError(e).message);
          },
        );
      }
    }, 200);
    return () => {
      live = false;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const get = useCallback((m: SyncWindowMonths) => cache.current.get(m) ?? null, []);
  return { get, error };
}

function save(patch: Parameters<typeof updateSettings>[0]) {
  return updateSettings(patch).catch((e) => {
    toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` });
    throw e;
  });
}

/** The preview under the slider for a staged (not yet saved) window. */
function PreviewLine({ rows, months, saved }: { rows: WindowEstimate[] | null; months: SyncWindowMonths; saved: SyncWindowMonths }) {
  if (!rows) return <span className="sw-estimate st-muted">Calculating…</span>;
  const ok = rows.filter((r) => !r.error);
  if (ok.length === 0) return <span className="sw-estimate st-muted">Estimate unavailable</span>;
  const total = ok.reduce((n, r) => n + r.inWindow, 0);
  const secs = Math.max(0, ...ok.map((r) => r.etaSecs));
  const left = ok.reduce((n, r) => n + Math.max(0, r.inWindow - r.haveFull), 0);
  const staged = months !== saved;
  const tail =
    staged && windowRank(months) < windowRank(saved)
      ? "nothing is deleted"
      : left === 0
        ? "already downloaded"
        : `${num(left)} to download · ${eta(secs)} at your quota`;
  return (
    <span className={"sw-estimate tnum" + (staged ? " is-staged" : "")}>
      {staged ? `${windowLabel(months)} (not saved): ` : ""}≈ {num(total)} messages {months === 0 ? "in total" : "in window"} · {tail}
    </span>
  );
}

export function SyncSection() {
  const s = useSettings();
  const accounts = meta.use((m) => m.accounts);
  const sync = meta.use((m) => m.sync);
  const [draft, setDraft] = useState<SyncWindowMonths>(s.syncWindowMonths);
  useEffect(() => setDraft(s.syncWindowMonths), [s.syncWindowMonths]);
  const [pending, setPending] = useState<PendingChange | null>(null);
  const estimates = useEstimateCache(draft === s.syncWindowMonths ? [draft] : [draft, s.syncWindowMonths]);
  const rows = estimates.get(draft);
  const savedRows = estimates.get(s.syncWindowMonths);
  const [coverage, setCoverage] = useState<SyncCoverage | null>(null);
  const [freeing, setFreeing] = useState<{ messages: number } | null>(null);
  const [busy, setBusy] = useState(false);

  const loadCoverage = useCallback(() => api.syncCoverage().then(setCoverage, () => {}), []);
  useEffect(() => {
    void loadCoverage();
    const t = setInterval(() => void loadCoverage(), 5000);
    return () => clearInterval(t);
  }, [loadCoverage, s.syncWindowMonths, s.olderMail]);

  // Releasing the slider on a new stop stages the change for confirmation.
  const stage = (m: SyncWindowMonths) => {
    setDraft(m);
    if (m !== s.syncWindowMonths) setPending({ kind: "window", months: m });
  };
  const cancel = useCallback(() => {
    setPending(null);
    setDraft(s.syncWindowMonths);
  }, [s.syncWindowMonths]);
  const confirm = async (change: PendingChange, freeUp: boolean) => {
    setPending(null);
    try {
      if (change.kind === "older") {
        await save({ olderMail: change.mode });
        return;
      }
      await save({ syncWindowMonths: change.months });
      if (freeUp && change.months !== 0) {
        setBusy(true);
        const r = await api.freeUpSpace(false, null, change.months);
        toast({ message: `Removed ${num(r.messages)} old message bodies · ${bytes(Math.max(0, r.bytesBefore - r.bytesAfter))} freed` });
      }
    } catch (e) {
      toast({ tone: "error", message: `Couldn't apply the change: ${asCommandError(e).message}` });
      setDraft(s.syncWindowMonths);
    } finally {
      setBusy(false);
      void loadCoverage();
    }
  };

  const idx = Math.max(0, STOPS.indexOf(draft));
  const olderEta = savedRows ? Math.max(0, ...savedRows.filter((r) => !r.error).map((r) => r.olderHeadersEtaSecs)) : null;
  const olderCount = savedRows ? savedRows.reduce((n, r) => n + (r.error ? 0 : r.older), 0) : null;
  const error = estimates.error;

  const askFree = async () => {
    try {
      const r = await api.freeUpSpace(true);
      if (r.messages === 0) {
        toast({ message: "Nothing to free: no full messages older than the window" });
        return;
      }
      setFreeing({ messages: r.messages });
    } catch (e) {
      toast({ tone: "error", message: asCommandError(e).message });
    }
  };
  const free = async () => {
    setFreeing(null);
    setBusy(true);
    try {
      const r = await api.freeUpSpace(false);
      const saved = Math.max(0, r.bytesBefore - r.bytesAfter);
      toast({ message: `Removed ${num(r.messages)} old message bodies · ${bytes(saved)} freed` });
      setCoverage(await api.syncCoverage());
    } catch (e) {
      toast({ tone: "error", message: `Couldn't free up space: ${asCommandError(e).message}` });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section id="sync" icon="refresh" title="Sync">
      <div className="setting-row setting-tall sw-window">
        <div className="grow min0">
          <label className="setting-label" htmlFor="sw-slider">
            Download in full
          </label>
          <p className="st-muted">
            Messages from {windowPhrase(draft)}, bodies included. New mail always arrives in full.
          </p>
          <div className="sw-slider">
            <input
              id="sw-slider"
              type="range"
              min={0}
              max={STOPS.length - 1}
              step={1}
              value={idx}
              aria-valuetext={windowLabel(draft)}
              onChange={(e) => setDraft(STOPS[Number(e.target.value)])}
              onPointerUp={() => stage(draft)}
              // Keyboard: arrows move the pending value; Enter (or leaving the slider) confirms it.
              onKeyUp={(e) => (e.key === "Enter" ? stage(draft) : undefined)}
              onBlur={() => (!pending && draft !== s.syncWindowMonths ? stage(draft) : undefined)}
            />
            <div className="sw-stops" aria-hidden="true">
              {STOPS.map((m, i) => (
                <button
                  key={m}
                  tabIndex={-1}
                  className={"sw-stop" + (i === idx ? " on" : "")}
                  onClick={() => stage(m)}
                >
                  {m === 0 ? "All" : m === 12 ? "1y" : m === 24 ? "2y" : `${m}mo`}
                </button>
              ))}
            </div>
          </div>
          {error && !rows ? (
            <span className="sw-estimate st-muted">Estimate unavailable: {error}</span>
          ) : (
            <PreviewLine rows={rows} months={draft} saved={s.syncWindowMonths} />
          )}
        </div>
      </div>

      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Older mail</span>
          <p className="st-muted">
            {s.olderMail === "headers"
              ? "Sender, subject, labels and a preview, fetched after the window finishes. Bodies download when you open a message."
              : s.olderMail === "none"
                ? "Not downloaded. “Also search Gmail” in search still finds it."
                : "Downloaded in full after the window finishes. Uses three times the quota of headers."}
            {s.syncWindowMonths !== 0 && olderCount != null && s.olderMail !== "none"
              ? ` About ${num(olderCount)} messages${olderEta ? `, ${eta(olderEta)} for headers` : ""}.`
              : ""}
          </p>
        </div>
        <Choice<OlderMail>
          label="Older mail"
          value={s.olderMail}
          onChange={(mode) => mode !== s.olderMail && setPending({ kind: "older", mode })}
          options={[
            { value: "headers", label: "Headers" },
            { value: "none", label: "Skip" },
            { value: "full", label: "Full" },
          ]}
        />
      </div>

      {coverage && coverage.accounts.length > 0 ? (
        <div className="setting-row setting-tall sw-coverage">
          <div className="grow min0">
            <span className="setting-label">Downloaded</span>
            <table className="sw-table tnum">
              <tbody>
                {inAccountOrder(coverage.accounts, accounts).map((c) => {
                  const a = accounts.find((x) => x.id === c.accountId);
                  const st = sync[c.accountId];
                  const state =
                    st?.phase === "backfilling" && st.stage === "window"
                      ? `Downloading ${windowPhrase(coverage.windowMonths)}${st.etaSecs ? ` · ${eta(st.etaSecs)}` : ""}`
                      : st?.phase === "backfilling" && st.stage === "older"
                        ? `Older mail${st.etaSecs ? ` · ${eta(st.etaSecs)}` : ""}`
                        : !c.windowComplete
                          ? "Waiting to download"
                          : c.olderComplete
                            ? "Complete"
                            : "Older mail pending";
                  return (
                    <tr key={c.accountId}>
                      <td className="truncate">{a ? accountName(a, accounts) : c.accountId}</td>
                      <td>{num(c.full)} full</td>
                      <td>{num(c.headersOnly)} headers only</td>
                      <td className="st-muted">{state}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      ) : null}

      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Free up space</span>
          <p className="st-muted">
            Removes stored bodies older than the window. They download again when you open the message.
          </p>
        </div>
        <button className="btn btn-ghost btn-sm" disabled={busy || s.syncWindowMonths === 0} onClick={() => void askFree()}>
          {busy ? "Freeing…" : "Free up space"}
        </button>
      </div>

      {pending ? (
        <SyncConfirm
          change={pending}
          current={{ months: s.syncWindowMonths, older: s.olderMail }}
          estimates={estimates.get}
          coverage={coverage}
          accounts={accounts}
          onConfirm={(freeUp) => void confirm(pending, freeUp)}
          onCancel={cancel}
        />
      ) : null}

      {freeing ? (
        <ConfirmDialog
          title={`Remove ${num(freeing.messages)} message bodies?`}
          body={`Bodies of mail older than ${windowLabel(s.syncWindowMonths).toLowerCase()} are removed from this Mac, then the database is compacted (this can take a minute). Your Gmail isn't affected.`}
          confirm="Free up space"
          onConfirm={() => void free()}
          onCancel={() => setFreeing(null)}
        />
      ) : null}
    </Section>
  );
}

/** Per-account rows in the user's account order (meta.accounts is sorted by Settings.accountOrder). */
function inAccountOrder<T extends { accountId: string }>(rows: T[], accounts: { id: string }[]): T[] {
  const pos = new Map(accounts.map((a, i) => [a.id, i]));
  return [...rows].sort((x, y) => (pos.get(x.accountId) ?? Infinity) - (pos.get(y.accountId) ?? Infinity));
}
