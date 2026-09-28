// OWNER: ask agent. Questions read as queries, in the Ask card:
//
// - "Understood as": the reading as small chips (flights · to Lisbon ·
//   August 2026 · How many). Each can be changed: the subject and the
//   operation from a menu, the timeframe by typing, a place, store, person
//   or grouping removed with ×. A change asks again with the edited query
//   (`ask_query`); the answer is still counted from local mail.
// - Groups: grouped counts or sums ("by month"), and the two sides of a
//   comparison, as rows with a bar; the winner is marked.
import { useEffect, useRef, useState } from "react";
import type { AskCite, AskGroup, AskQuery, AskUnderstood, QueryGroup, QueryOp, QuerySubject } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { GROUPS, OPS, SUBJECTS, groupValue, queryChips, sourceNote, withOp, withSubject, withoutPart, type QueryChip } from "./askQueryModel";

export function QueryChips({ understood, onEdit, busy }: { understood: AskUnderstood; onEdit: (q: AskQuery) => void; busy: boolean }) {
  const q = understood.query;
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(q.timeframe ?? "");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (editing) input.current?.focus();
  }, [editing]);
  useEffect(() => setText(q.timeframe ?? ""), [q.timeframe]);
  const chips = queryChips(understood);
  const stop = (e: { preventDefault: () => void }) => e.preventDefault();
  const commitTime = () => {
    setEditing(false);
    const t = text.trim();
    if (t !== (q.timeframe ?? "")) onEdit({ ...q, timeframe: t || null });
  };
  const chip = (c: QueryChip) => {
    if (c.key === "subject") {
      return (
        <label key="subject" className="ask-q-chip ask-q-select" title="What the question is about">
          <select value={q.subject} disabled={busy} onChange={(e) => onEdit(withSubject(q, e.target.value as QuerySubject))} aria-label="Subject">
            {SUBJECTS.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </select>
        </label>
      );
    }
    if (c.key === "op") {
      return (
        <label key="op" className="ask-q-chip ask-q-select" title="What to work out">
          <select value={q.op} disabled={busy} onChange={(e) => onEdit(withOp(q, e.target.value as QueryOp))} aria-label="Operation">
            {OPS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
      );
    }
    if (c.key === "groupBy") {
      return (
        <label key="groupBy" className="ask-q-chip ask-q-select" title="Group the answer">
          <select value={q.groupBy ?? ""} disabled={busy} onChange={(e) => onEdit({ ...q, groupBy: (e.target.value || null) as QueryGroup | null })} aria-label="Grouping">
            {GROUPS.map((g) => (
              <option key={g.label} value={g.value ?? ""}>
                {g.label}
              </option>
            ))}
          </select>
        </label>
      );
    }
    if (c.key === "timeframe" && editing) {
      return (
        <input
          key="timeframe"
          ref={input}
          className="ask-q-chip ask-q-input"
          value={text}
          placeholder="august, last year, since march…"
          aria-label="Timeframe"
          onChange={(e) => setText(e.target.value)}
          onBlur={commitTime}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") commitTime();
            if (e.key === "Escape") {
              setText(q.timeframe ?? "");
              setEditing(false);
            }
          }}
        />
      );
    }
    return (
      <span key={c.key} className={`ask-q-chip${c.key === "timeframe" ? " is-time" : ""}`}>
        {c.key === "timeframe" ? (
          <button className="ask-q-text" disabled={busy} onMouseDown={stop} onClick={() => setEditing(true)} title="Change the dates">
            {c.label}
          </button>
        ) : (
          <span className="ask-q-text">{c.label}</span>
        )}
        {c.removable && (
          <button className="ask-q-x" disabled={busy} aria-label={`Remove ${c.label}`} title="Remove" onMouseDown={stop} onClick={() => onEdit(withoutPart(q, c.key))}>
            <Icon name="x" size="2xs" />
          </button>
        )}
      </span>
    );
  };
  return (
    <div className={`ask-q${busy ? " is-busy" : ""}`} aria-label="How the question was read">
      <span className="ask-q-label faint" title={sourceNote(understood)}>
        {understood.source === "model" && <Icon name="sparkles" size="2xs" />}
        Understood as
      </span>
      {chips.map(chip)}
      {!q.timeframe && !editing && (
        <button className="ask-q-chip ask-q-add" disabled={busy} onMouseDown={stop} onClick={() => setEditing(true)} title="Limit to dates">
          <Icon name="plus" size="2xs" />
          Dates
        </button>
      )}
      {!q.timeframe && editing && chip({ key: "timeframe", label: "", removable: false })}
      <span className="ask-q-source faint">{sourceNote(understood)}</span>
    </div>
  );
}

/** Grouped counts or sums, or the sides of a comparison: one row each, with a bar. */
export function GroupRows({ groups, measure, onOpen, compare }: { groups: AskGroup[]; measure: AskQuery["measure"] | undefined; onOpen: (c: AskCite) => void; compare: boolean }) {
  const [all, setAll] = useState(false);
  if (groups.length === 0) return null;
  const vals = groups.map((g) => groupValue(g, measure));
  const max = Math.max(1e-9, ...vals.map((v) => Math.abs(v.value)));
  const LIMIT = 8;
  const shown = all ? groups : groups.slice(0, LIMIT);
  return (
    <div className={`ask-groups${compare ? " is-compare" : ""}`} role="table" aria-label={compare ? "Comparison" : "Groups"}>
      {shown.map((g, i) => (
        <div
          key={`${g.label}-${i}`}
          className={`ask-group${g.best ? " is-best" : ""}${g.cites.length ? " is-cited" : ""}`}
          role="row"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => g.cites[0] && onOpen(g.cites[0])}
          title={g.cites.length ? `${g.cites.length} ${g.cites.length === 1 ? "email" : "emails"}: open the first` : undefined}
        >
          <span className="ask-group-label truncate" role="cell">
            {g.best && <Icon name="check" size="2xs" />}
            {g.label}
          </span>
          <span className="ask-group-bar" role="cell" aria-hidden>
            <i style={{ width: `${Math.max(2, (Math.abs(vals[i].value) / max) * 100)}%` }} />
          </span>
          <span className="ask-group-value" role="cell">
            {vals[i].text}
          </span>
        </div>
      ))}
      {groups.length > LIMIT && !all && (
        <button className="ask-more" onMouseDown={(e) => e.preventDefault()} onClick={() => setAll(true)}>
          {groups.length - LIMIT} more
        </button>
      )}
    </div>
  );
}
