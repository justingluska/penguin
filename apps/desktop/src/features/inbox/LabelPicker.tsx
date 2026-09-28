// L — label picker for the cursor thread, or for every selected thread
// (app/selection.ts). Labels belong to an account, so with a selection across
// accounts a label is offered by name and applied in each account that has it.
// Checked = every target has it; a dash = some do. Type to filter, ↑↓ to
// move, ↵ to toggle, Esc to close.
import { useEffect, useMemo, useState } from "react";
import type { Label, ThreadRef } from "../../lib/types";
import { setUi, useUi } from "../../lib/ui";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { LabelSwatch } from "../../components/Identity";
import { accountById, cachedThread, list, meta } from "../../app/store";
import { setLabel, targets } from "../../app/actions";
import { isLabelChoice } from "../../lib/capabilities";

export function LabelPicker() {
  const open = useUi((s) => s.overlay === "label");
  if (!open) return null;
  return <Picker />;
}

interface Option {
  name: string;
  color: string | null;
  /** This label's id in each target account that has it. */
  idByAccount: Map<string, string>;
  /** How many targets carry it. */
  on: number;
  /** How many targets could carry it (their account has the label). */
  of: number;
}

function Picker() {
  // Fixed for this opening: the selection, else the cursor thread.
  const [refs] = useState<ThreadRef[]>(targets);
  const labels = meta.use((m) => m.labels);
  const items = list.use((l) => l.items);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);

  const labelsOf = useMemo(() => {
    const byKey = new Map(items.map((t) => [t.accountId + "\u0000" + t.threadId, t.labelIds]));
    return (r: ThreadRef) => new Set(byKey.get(r.accountId + "\u0000" + r.threadId) ?? cachedThread(r)?.labelIds ?? []);
  }, [items]);

  const options = useMemo(() => {
    const accounts = new Set(refs.map((r) => r.accountId));
    const byName = new Map<string, { color: string | null; idByAccount: Map<string, string> }>();
    for (const l of labels as Label[]) {
      // Labels only: an account's folders are Move to… (and IMAP has no labels at all).
      if (!accounts.has(l.accountId) || !isLabelChoice(l, accountById(l.accountId))) continue;
      if (q && !l.name.toLowerCase().includes(q.toLowerCase())) continue;
      const o = byName.get(l.name) ?? { color: l.color, idByAccount: new Map() };
      o.idByAccount.set(l.accountId, l.id);
      byName.set(l.name, o);
    }
    const sets = refs.map((r) => ({ r, ids: labelsOf(r) }));
    return [...byName]
      .map(([name, o]): Option => {
        let on = 0;
        let of = 0;
        for (const { r, ids } of sets) {
          const id = o.idByAccount.get(r.accountId);
          if (!id) continue;
          of++;
          if (ids.has(id)) on++;
        }
        return { name, color: o.color, idByAccount: o.idByAccount, on, of };
      })
      .sort((a, b) => a.name.localeCompare(b.name));
  }, [labels, refs, labelsOf, q]);

  useEffect(() => setActive(0), [q]);

  const close = () => setUi({ overlay: null });
  // Everyone has it: take it off. Otherwise put it on everyone who can have it.
  const toggle = (o: Option) => {
    const add = o.on < o.of;
    const byAccount = new Map<string, ThreadRef[]>();
    for (const r of refs) {
      if (!o.idByAccount.has(r.accountId)) continue;
      byAccount.set(r.accountId, [...(byAccount.get(r.accountId) ?? []), r]);
    }
    for (const [acct, rs] of byAccount) setLabel(rs, o.idByAccount.get(acct)!, add);
  };

  return (
    <>
      <div className="scrim soft" onMouseDown={close} />
      <div className="overlay-host" onMouseDown={(e) => e.target === e.currentTarget && close()}>
        <section className="palette panel label-picker" role="dialog" aria-label={refs.length > 1 ? `Label ${refs.length} conversations` : "Label conversation"}>
          <div className="pal-input">
            <Icon name="tag" size="sm" className="faint" />
            <input
              className="pal-field"
              autoFocus
              placeholder={refs.length > 1 ? `Label ${refs.length} conversations as…` : "Label as…"}
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  close();
                } else if (e.key === "ArrowDown") {
                  e.preventDefault();
                  setActive((a) => Math.min(options.length - 1, a + 1));
                } else if (e.key === "ArrowUp") {
                  e.preventDefault();
                  setActive((a) => Math.max(0, a - 1));
                } else if (e.key === "Enter") {
                  e.preventDefault();
                  const o = options[active];
                  if (o) toggle(o);
                }
              }}
            />
          </div>
          <div className="pal-list">
            {options.map((o, i) => (
              <button
                key={o.name}
                className={"menu-item" + (i === active ? " active" : "")}
                onMouseEnter={() => setActive(i)}
                onClick={() => toggle(o)}
                aria-pressed={o.on === 0 ? false : o.on === o.of ? true : "mixed"}
              >
                <LabelSwatch color={o.color} />
                <span className="grow">{o.name}</span>
                {o.on > 0 && o.on === o.of && <Icon name="check" size="xs" />}
                {o.on > 0 && o.on < o.of && <Icon name="minus" size="xs" />}
              </button>
            ))}
            {options.length === 0 && (
              <div className="pal-group">{q ? `No label matches “${q}”` : refs.length > 1 ? "These accounts have no labels" : "This account has no labels"}</div>
            )}
          </div>
          <footer className="pal-foot">
            <span className="hint">
              <Kbd>↵</Kbd>Toggle
            </span>
            <span className="grow" />
            <span className="hint">
              <Kbd>Esc</Kbd>Close
            </span>
          </footer>
        </section>
      </div>
    </>
  );
}
