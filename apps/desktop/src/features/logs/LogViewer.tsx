// View log: the newest ~1 MB of penguin.log in a dialog, newest first.
// Opened from Settings → Developer. Everything the backend logs, plus what
// the UI reports through logClientEvent (failed commands, error toasts,
// uncaught script errors), so "what just went wrong" has one place to look.
//
//   openLogViewer()  →  <LogViewerHost/> (mounted by App)
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { api, asCommandError } from "../../lib/api";
import { copyText } from "../../lib/clipboard";
import { Icon } from "../../components/Icon";
import { toast } from "../../components/Toast";
import { Choice } from "../settings/parts";
import { isProblem, parseLog, type LogEntry } from "./parse";
import "./logs.css";

let open = false;
const subs = new Set<() => void>();
const set = (v: boolean) => {
  open = v;
  subs.forEach((f) => f());
};
export const openLogViewer = () => set(true);

export function LogViewerHost() {
  const isOpen = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => open,
  );
  return isOpen ? <LogViewer onClose={() => set(false)} /> : null;
}

type Filter = "problems" | "all";
/** Rows rendered at once; the filter and search narrow the rest. */
const SHOWN = 1500;

const fmtTime = (at: number | null) =>
  at === null
    ? ""
    : new Date(at).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });

function LogViewer({ onClose }: { onClose: () => void }) {
  const [entries, setEntries] = useState<LogEntry[] | null>(null);
  const [lines, setLines] = useState<string[]>([]);
  const [meta, setMeta] = useState<{ path: string | null; truncated: boolean }>({ path: null, truncated: false });
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("problems");
  const [q, setQ] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    try {
      const t = await api.readLog();
      setLines(t.lines);
      setEntries(parseLog(t.lines).reverse());
      setMeta({ path: t.path, truncated: t.truncated });
      setError(null);
    } catch (e) {
      setError(asCommandError(e).message);
    }
  }, []);

  useEffect(() => {
    void load();
    searchRef.current?.focus();
    // Capture phase, ahead of Settings' own keys: Esc closes only this.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [load, onClose]);

  const shown = useMemo(() => {
    if (!entries) return [];
    const needle = q.trim().toLowerCase();
    return entries.filter(
      (e) => (filter === "all" || isProblem(e)) && (!needle || e.text.toLowerCase().includes(needle) || e.target.toLowerCase().includes(needle)),
    );
  }, [entries, filter, q]);
  const problems = useMemo(() => entries?.filter(isProblem).length ?? 0, [entries]);

  const copy = () => {
    // The raw lines, oldest first, for a bug report (ids and error text only).
    const text = filter === "all" && !q.trim() ? lines.join("\n") : [...shown].reverse().map(raw).join("\n");
    void copyText(text, "Log copied");
  };

  return (
    <div className="st-scrim lv-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="st-confirm lv" role="dialog" aria-modal="true" aria-label="Log">
        <header className="lv-head">
          <h3>Log</h3>
          <Choice
            label="Show"
            value={filter}
            options={[
              { value: "problems", label: `Problems${entries ? ` · ${problems}` : ""}` },
              { value: "all", label: "Everything" },
            ]}
            onChange={setFilter}
          />
          <input
            ref={searchRef}
            className="input lv-search"
            placeholder="Filter"
            value={q}
            spellCheck={false}
            onChange={(e) => setQ(e.target.value)}
          />
          <span className="grow" />
          <button className="btn btn-ghost btn-sm btn-icon" title="Refresh" aria-label="Refresh" onClick={() => void load()}>
            <Icon name="refresh" size="xs" />
          </button>
          <button className="btn btn-ghost btn-sm btn-icon" title="Copy" aria-label="Copy" onClick={copy} disabled={!entries?.length}>
            <Icon name="copy" size="xs" />
          </button>
          <button
            className="btn btn-ghost btn-sm btn-icon"
            title="Show in Finder"
            aria-label="Show in Finder"
            onClick={() => api.revealPath("log").catch((e) => toast({ tone: "error", message: asCommandError(e).message }))}
          >
            <Icon name="external" size="xs" />
          </button>
          <button className="btn btn-ghost btn-sm btn-icon" title="Close · Esc" aria-label="Close" onClick={onClose}>
            <Icon name="x" size="xs" />
          </button>
        </header>
        <div className="lv-body" tabIndex={0}>
          {error ? (
            <p className="lv-empty">Couldn't read the log: {error}</p>
          ) : !entries ? (
            <p className="lv-empty st-muted">Loading…</p>
          ) : shown.length === 0 ? (
            <p className="lv-empty st-muted">
              {q.trim() ? "Nothing matches." : filter === "problems" ? "No warnings or errors. Everything's been fine." : "The log is empty."}
            </p>
          ) : (
            <ol className="lv-list">
              {shown.slice(0, SHOWN).map((e, i) => (
                <li key={i} className={"lv-row" + (e.level ? ` is-${e.level.toLowerCase()}` : "")}>
                  <span className="lv-time tnum">{fmtTime(e.at)}</span>
                  <span className="lv-level">{e.level ?? ""}</span>
                  <span className="lv-text">
                    {e.target && <span className="lv-target">{e.target === "penguin_ui" ? "app" : e.target}</span>}
                    {e.text}
                  </span>
                </li>
              ))}
            </ol>
          )}
        </div>
        <footer className="lv-foot st-muted">
          <span>
            {shown.length > SHOWN ? `Newest ${SHOWN} of ${shown.length}` : `${shown.length} ${shown.length === 1 ? "entry" : "entries"}`}
            {meta.truncated && " · older lines are in the file"}
          </span>
          <span className="lv-path truncate" title={meta.path ?? undefined}>
            {meta.path}
          </span>
        </footer>
      </div>
    </div>
  );
}

const raw = (e: LogEntry) =>
  [e.at === null ? "" : new Date(e.at).toISOString(), e.level ?? "", e.target ? `${e.target}:` : "", e.text].filter(Boolean).join(" ");
