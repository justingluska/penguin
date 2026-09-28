// The pieces a smart view adds to the normal thread list: the fact line a
// row shows in place of its snippet, and the slim header over the list.
import type { MailboxView, SmartRow } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { setUi } from "../../lib/ui";
import { goTo } from "../../app/shortcuts";
import { FILE_FILTERS, fileKindOf, filesView } from "./catalog";
import { smartLine, statText } from "./format";
import { useSmartInfo } from "./state";
import "./smart.css";

/** The row's fact: title · facts, a status chip, the amount. Null when the row keeps its own chip. */
export function SmartLine({ row, stacked }: { row: SmartRow; stacked: boolean }) {
  const p = smartLine(row);
  if (!p) return null;
  return (
    <span className={"smart-slot" + (stacked ? " is-stacked" : "")}>
      {!stacked && <span className="smart-sep"> — </span>}
      <span className="smart-title">{p.title}</span>
      {/* Three lines are tight: the stacked row keeps the first fact. */}
      {(stacked ? p.facts.slice(0, 1) : p.facts).map((f, i) => (
        <span key={i} className="smart-fact">
          {f}
        </span>
      ))}
      {p.chip && <span className={`smart-chip t-${p.chip.tone}`}>{p.chip.label}</span>}
      {p.amount && <span className={"smart-amt tnum" + (p.credit ? " is-credit" : "")}>{p.amount}</span>}
    </span>
  );
}

/**
 * Over a smart view's list: its key figures (this month's total, what's
 * due, what's on the way), the Files filters, and a note while the fact
 * scanner is still reading. Nothing for views without figures.
 */
export function SmartHeader({ view, scope }: { view: MailboxView; scope: string[] | null }) {
  const info = useSmartInfo(view, scope);
  if (view.kind === "query") return <QueryHeader query={view.labelId} />;
  if (view.kind !== "smart") return null;
  if (view.labelId.startsWith("files")) return <FileFilters view={view} />;
  if (!info || (info.stats.length === 0 && !info.note)) return null;
  return (
    <div className="smart-head" role="status" aria-live="polite">
      {info.stats.map((s, i) => (
        <div key={i} className={"smart-stat" + (s.tone ? ` is-${s.tone}` : "")}>
          <span className="smart-stat-label">{s.label}</span>
          <span className="smart-stat-value tnum">{statText(s)}</span>
        </div>
      ))}
      {info.note && (
        <div className="smart-note">
          <Icon name="info" size="xs" />
          {info.note}
        </div>
      )}
    </div>
  );
}

function FileFilters({ view }: { view: MailboxView }) {
  const on = fileKindOf(view);
  return (
    <div className="smart-head smart-chips" role="radiogroup" aria-label="File type">
      {FILE_FILTERS.map((f) => (
        <button
          key={f.label}
          role="radio"
          aria-checked={on === f.kind}
          className={"chip smart-filter" + (on === f.kind ? " is-on" : "")}
          onClick={() => goTo(filesView(f.kind))}
        >
          {f.label}
        </button>
      ))}
    </div>
  );
}

function QueryHeader({ query }: { query: string }) {
  return (
    <div className="smart-head">
      <button className="smart-query" title="Edit this search" onClick={() => setUi({ overlay: "search", searchPrefill: query })}>
        <Icon name="search" size="xs" />
        <span className="truncate">{query}</span>
      </button>
    </div>
  );
}
