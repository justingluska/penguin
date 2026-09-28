// Settings → Inbox: the Split Inbox (on/off, the splits: name, query, order,
// hide when empty, presets), Get to zero and the zero screen, and Floe mode.
// Every change writes Settings at once (lib/settings.ts is optimistic).
import { useEffect, useMemo, useRef, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { isMac } from "../../lib/keyboard";
import { num } from "../../lib/format";
import { updateSettings, useSettings } from "../../lib/settings";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { Section, Switch } from "../settings/parts";
import { matchPeople, seedPeople, usePeople } from "../search/people";
import { meta } from "../../app/store";
import { OTHER, newSplitId } from "../../app/splits";
import type { InboxSplit } from "../../lib/types";
import { SPLIT_PRESETS, peopleOf, splitFromPreset, vipQuery, type SplitPreset } from "./presets";
import { saveSplits } from "./state";
import "./split.css";

const MAX_SPLITS = 12;

const save = (patch: Parameters<typeof updateSettings>[0]) => void updateSettings(patch);

export function InboxSection() {
  const s = useSettings();
  return (
    <Section id="inbox" icon="inbox" title="Inbox">
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Split Inbox</span>
          <p className="st-muted">
            Tabs across the top of the inbox, each a search: a conversation goes to the first split it matches, and Other
            holds the rest. Tab and ⇧Tab move between them, 1–9 jump. Works with every account; Important uses Gmail's own
            marker.
          </p>
        </div>
        <Switch label="Split Inbox" on={s.inboxTabs} onChange={(inboxTabs) => save({ inboxTabs })} />
      </div>
      <SplitsEditor splits={s.inboxSplits} on={s.inboxTabs} />

      <h3 className="st-sub">Getting to zero</h3>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Get to zero</span>
          <p className="st-muted">
            Archive everything older than a day, a week or a month in one go, in a split or the whole inbox, keeping unread
            or starred mail if you like. From {isMac ? "⌘K" : "Ctrl+K"}, the ✓ in the list header or a split's menu; Z undoes it.
          </p>
        </div>
        <Switch label="Get to zero" on={s.getToZero} onChange={(getToZero) => save({ getToZero })} />
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Celebrate inbox zero</span>
          <p className="st-muted">The penguin on its floe, and how much you cleared today. Off shows a plain “No mail”.</p>
        </div>
        <Switch label="Celebrate inbox zero" on={s.zeroCelebration} onChange={(zeroCelebration) => save({ zeroCelebration })} />
      </div>

      <h3 className="st-sub">Floe</h3>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Start in Floe mode</span>
          <p className="st-muted">
            One calm column for flying through mail: no sidebar, threads open full width, the splits across the top.
            Toggle any time with {isMac ? "⌘⇧F" : "Ctrl+Shift+F"} or the \ key. Penguin reopens in the mode you last used.
          </p>
        </div>
        <Switch label="Start in Floe mode" on={s.floeMode} onChange={(floeMode) => save({ floeMode })} />
      </div>
    </Section>
  );
}

function SplitsEditor({ splits, on }: { splits: InboxSplit[]; on: boolean }) {
  const counts = meta.use((m) => m.splitCounts);
  const accounts = meta.use((m) => m.accounts);
  const mine = useMemo(() => accounts.map((a) => a.email), [accounts]);
  const [adding, setAdding] = useState<SplitPreset | null>(null);

  const update = (id: string, patch: Partial<InboxSplit>) => void saveSplits(splits.map((x) => (x.id === id ? { ...x, ...patch } : x)));
  const move = (i: number, d: -1 | 1) => {
    const next = [...splits];
    const [x] = next.splice(i, 1);
    next.splice(i + d, 0, x);
    void saveSplits(next);
  };
  const add = (p: SplitPreset) => {
    if (p.key === "vip") return setAdding(p);
    const sp = splitFromPreset(p, newSplitId(p.name, splits.map((x) => x.id)), mine);
    if (sp) void saveSplits([...splits, sp]);
  };
  const addCustom = () =>
    void saveSplits([...splits, { id: newSplitId("custom", splits.map((x) => x.id)), name: "New split", query: "from:", hideWhenEmpty: false }]);
  const full = splits.length >= MAX_SPLITS;

  return (
    <div className={"sp-editor" + (on ? "" : " is-off")} data-setting="splits">
      <h3 className="st-sub">Splits</h3>
      <div className="sp-list">
        {splits.map((sp, i) => (
          <SplitRow
            key={sp.id}
            split={sp}
            count={counts?.[sp.id]?.total}
            first={i === 0}
            last={i === splits.length - 1}
            onChange={(patch) => update(sp.id, patch)}
            onMove={(d) => move(i, d)}
            onRemove={() => void saveSplits(splits.filter((x) => x.id !== sp.id))}
          />
        ))}
        <div className="sp-row is-other">
          <span className="sp-grip">
            <Icon name="inbox" size="xs" />
          </span>
          <span className="sp-name-static">Other</span>
          <span className="sp-desc">
            Everything no split above claims{counts?.[OTHER] ? ` · ${num(counts[OTHER].total)} now` : ""}
          </span>
        </div>
      </div>
      {adding && (
        <VipPicker
          onCancel={() => setAdding(null)}
          onDone={(people) => {
            setAdding(null);
            const sp = splitFromPreset(adding, newSplitId("VIP", splits.map((x) => x.id)), mine, people);
            if (sp) void saveSplits([...splits, sp]);
          }}
        />
      )}
      <div className="sp-add" aria-label="Add a split">
        {SPLIT_PRESETS.filter((p) => p.key === "vip" || !splits.some((x) => x.query === p.query(mine))).map((p) => {
          const unavailable = p.key !== "vip" && p.query(mine) === null;
          return (
            <button
              key={p.key}
              className="btn btn-secondary btn-sm"
              disabled={full || unavailable}
              title={unavailable ? "None of your accounts is on a work domain" : p.blurb}
              onClick={() => add(p)}
            >
              <Icon name={p.icon} size="xs" />
              {p.name}
            </button>
          );
        })}
        <button className="btn btn-ghost btn-sm" disabled={full} onClick={addCustom}>
          <Icon name="plus" size="xs" />
          Custom search
        </button>
      </div>
      <p className="sp-hint">
        A split is any search: <code>from:@acme.example</code>, <code>label:clients</code>, <code>is:newsletter</code>,{" "}
        <code>has:invite OR subject:standup</code>. Add the sender of any message to a split with {isMac ? "⌘K" : "Ctrl+K"} → Add sender to
        a split.
      </p>
    </div>
  );
}

function SplitRow({
  split,
  count,
  first,
  last,
  onChange,
  onMove,
  onRemove,
}: {
  split: InboxSplit;
  count: number | undefined;
  first: boolean;
  last: boolean;
  onChange: (patch: Partial<InboxSplit>) => void;
  onMove: (d: -1 | 1) => void;
  onRemove: () => void;
}) {
  const [name, setName] = useState(split.name);
  const [query, setQuery] = useState(split.query);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setName(split.name), [split.name]);
  useEffect(() => setQuery(split.query), [split.query]);

  // An unreadable query (a date it can't read) says so under the row.
  useEffect(() => {
    let live = true;
    api
      .listThreads({ view: { kind: "inbox" }, tab: null, split: { include: split.query, exclude: [] }, accountId: null, limit: 1, before: null })
      .then(
        () => live && setError(null),
        (e) => live && setError(asCommandError(e).code === "invalidInput" ? asCommandError(e).message : null),
      );
    return () => {
      live = false;
    };
  }, [split.query]);

  const commitName = () => name.trim() !== split.name && onChange({ name: name.trim() || split.query });
  const commitQuery = () => {
    const q = query.trim().replace(/\s+/g, " ");
    if (!q) return setQuery(split.query);
    if (q !== split.query) onChange({ query: q });
  };
  const people = peopleOf(split.query);

  return (
    <div className="sp-row" data-split-row={split.id}>
      <span className="sp-grip" title={count !== undefined ? `${num(count)} in the inbox now` : undefined}>
        {count !== undefined ? <span className="tnum faint">{count > 999 ? "999+" : num(count)}</span> : <Icon name="list" size="xs" />}
      </span>
      <input
        aria-label="Split name"
        value={name}
        maxLength={30}
        onChange={(e) => setName(e.target.value)}
        onBlur={commitName}
        onKeyDown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
      />
      <input
        className="sp-query"
        aria-label="Split search"
        value={query}
        maxLength={500}
        spellCheck={false}
        onChange={(e) => setQuery(e.target.value)}
        onBlur={commitQuery}
        onKeyDown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
      />
      <span className="sp-acts">
        <button
          className={"btn btn-ghost btn-icon btn-sm" + (split.hideWhenEmpty ? " is-on" : "")}
          title={split.hideWhenEmpty ? "Hidden while empty (click to always show)" : "Always shown (click to hide while empty)"}
          aria-label="Hide when empty"
          aria-pressed={split.hideWhenEmpty}
          onClick={() => onChange({ hideWhenEmpty: !split.hideWhenEmpty })}
        >
          <Icon name={split.hideWhenEmpty ? "eyeoff" : "eye"} size="xs" />
        </button>
        <button className="btn btn-ghost btn-icon btn-sm" title="Move up (claims mail before the ones below)" aria-label="Move up" disabled={first} onClick={() => onMove(-1)}>
          <Icon name="up" size="xs" />
        </button>
        <button className="btn btn-ghost btn-icon btn-sm" title="Move down" aria-label="Move down" disabled={last} onClick={() => onMove(1)}>
          <Icon name="down" size="xs" />
        </button>
        <button className="btn btn-ghost btn-icon btn-sm" title="Remove split" aria-label="Remove split" onClick={onRemove}>
          <Icon name="x" size="xs" />
        </button>
      </span>
      {people && (
        <div className="sp-people">
          <PeopleChips emails={people} onChange={(list) => list.length && onChange({ query: vipQuery(list) })} />
        </div>
      )}
      {error && <span className="sp-error">{error}</span>}
    </div>
  );
}

/** The people of a VIP-style split, as chips, plus an input with suggestions. */
function PeopleChips({ emails, onChange }: { emails: string[]; onChange: (next: string[]) => void }) {
  const [text, setText] = useState("");
  const people = usePeople();
  useEffect(seedPeople, []);
  const sugg = text.trim() ? matchPeople(people, text, new Set(emails), 4) : [];
  const addEmail = (e: string) => {
    const v = e.trim().toLowerCase();
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v) || emails.includes(v)) return;
    onChange([...emails, v]);
    setText("");
  };
  return (
    <>
      {emails.map((e) => (
        <span className="chip" key={e}>
          {e}
          <button className="x" aria-label={`Remove ${e}`} disabled={emails.length === 1} onClick={() => onChange(emails.filter((x) => x !== e))}>
            <Icon name="x" size="xs" />
          </button>
        </span>
      ))}
      <input
        aria-label="Add a person"
        placeholder="Add a person…"
        value={text}
        list={undefined}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") addEmail(sugg[0]?.email ?? text);
        }}
      />
      {sugg.map((a) => (
        <button key={a.email} className="btn btn-ghost btn-sm" onClick={() => addEmail(a.email)}>
          {a.name ? `${a.name} · ${a.email}` : a.email}
        </button>
      ))}
    </>
  );
}

/** VIP: pick the people first (the split is a list of from: addresses). */
function VipPicker({ onDone, onCancel }: { onDone: (people: string[]) => void; onCancel: () => void }) {
  const [picked, setPicked] = useState<string[]>([]);
  const [text, setText] = useState("");
  const people = usePeople();
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    seedPeople();
    inputRef.current?.focus();
  }, []);
  const sugg = matchPeople(people, text, new Set(picked), 6);
  const add = (e: string) => {
    const v = e.trim().toLowerCase();
    if (/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v) && !picked.includes(v)) setPicked([...picked, v]);
    setText("");
  };
  return (
    <div className="sp-row sp-vip">
      <span className="sp-grip">
        <Icon name="user" size="xs" />
      </span>
      <span className="sp-name-static">VIP</span>
      <div className="sp-people" style={{ gridColumn: "3 / -1" }}>
        {picked.map((e) => (
          <span className="chip" key={e}>
            {e}
          </span>
        ))}
        <input
          ref={inputRef}
          aria-label="Add a person"
          placeholder="Name or address…"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              if (text.trim()) add(sugg[0]?.email ?? text);
              else if (picked.length) onDone(picked);
            }
            if (e.key === "Escape") {
              e.stopPropagation();
              onCancel();
            }
          }}
        />
        {sugg.slice(0, 4).map((a) => (
          <button key={a.email} className="btn btn-ghost btn-sm" onClick={() => add(a.email)}>
            {a.name ? `${a.name} · ${a.email}` : a.email}
          </button>
        ))}
        <button className="btn btn-primary btn-sm" disabled={picked.length === 0} onClick={() => onDone(picked)}>
          Add VIP split <Kbd>↵</Kbd>
        </button>
        <button className="btn btn-ghost btn-sm" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
