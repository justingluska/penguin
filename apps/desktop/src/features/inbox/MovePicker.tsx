// V (and L on accounts without labels) — Move to…: the folder picker for
// accounts whose labels are folders (IMAP, Microsoft; Account.capabilities
// .folders). A conversation lives in one folder, so picking one moves it
// there (app/actions.ts moveTo). Offers the Inbox and each account's
// folders; with a selection across accounts a folder is offered by name and
// used in each account that has it. Type to filter, ↑↓, ↵ moves, Esc closes.
import { useEffect, useMemo, useState } from "react";
import type { Label, ThreadRef } from "../../lib/types";
import { setUi, useUi } from "../../lib/ui";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { accountById, cachedThread, list, meta } from "../../app/store";
import { moveTo, targets } from "../../app/actions";
import { INBOX, isFolderChoice } from "../../lib/capabilities";

export function MovePicker() {
  const open = useUi((s) => s.overlay === "move");
  if (!open) return null;
  return <Picker />;
}

export interface FolderOption {
  /** "Inbox", or the folder's path ("Projects/Garden"). */
  name: string;
  inbox: boolean;
  /** The folder's id in each account that has it. */
  idByAccount: Map<string, string>;
}

/** Inbox first, then the targets' accounts' folders by name. */
export function folderOptions(labels: readonly Label[], accountIds: ReadonlySet<string>): FolderOption[] {
  const byName = new Map<string, Map<string, string>>();
  for (const l of labels) {
    if (!accountIds.has(l.accountId) || !isFolderChoice(l, accountById(l.accountId))) continue;
    const m = byName.get(l.name) ?? new Map<string, string>();
    m.set(l.accountId, l.id);
    byName.set(l.name, m);
  }
  const inbox: FolderOption = { name: "Inbox", inbox: true, idByAccount: new Map([...accountIds].map((a) => [a, INBOX])) };
  const folders = [...byName]
    .map(([name, idByAccount]): FolderOption => ({ name, inbox: false, idByAccount }))
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));
  return [inbox, ...folders];
}

/** Move `refs` to the option's folder in each account. */
export function moveToOption(refs: ThreadRef[], o: FolderOption) {
  moveTo(refs, (acct) => o.idByAccount.get(acct) ?? null, o.name);
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
    return (r: ThreadRef) => byKey.get(r.accountId + "\u0000" + r.threadId) ?? cachedThread(r)?.labelIds ?? [];
  }, [items]);

  const options = useMemo(() => {
    const all = folderOptions(labels, new Set(refs.map((r) => r.accountId)));
    const needle = q.trim().toLowerCase();
    return needle ? all.filter((o) => o.name.toLowerCase().includes(needle)) : all;
  }, [labels, refs, q]);

  useEffect(() => setActive(0), [q]);

  const close = () => setUi({ overlay: null });
  const pick = (o: FolderOption) => {
    close();
    moveToOption(refs, o);
  };
  /** Where every target already is (checked). */
  const here = (o: FolderOption) =>
    refs.length > 0 &&
    refs.every((r) => {
      const id = o.idByAccount.get(r.accountId);
      return !!id && labelsOf(r).includes(id);
    });

  return (
    <>
      <div className="scrim soft" onMouseDown={close} />
      <div className="overlay-host" onMouseDown={(e) => e.target === e.currentTarget && close()}>
        <section className="palette panel label-picker" role="dialog" aria-label={refs.length > 1 ? `Move ${refs.length} conversations` : "Move conversation"}>
          <div className="pal-input">
            <Icon name="folder" size="sm" className="faint" />
            <input
              className="pal-field"
              autoFocus
              placeholder={refs.length > 1 ? `Move ${refs.length} conversations to…` : "Move to…"}
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
                  if (o) pick(o);
                }
              }}
            />
          </div>
          <div className="pal-list">
            {options.map((o, i) => (
              <button
                key={o.inbox ? "\u0000inbox" : o.name}
                className={"menu-item" + (i === active ? " active" : "")}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(o)}
              >
                <Icon name={o.inbox ? "inbox" : "folder"} size="xs" className="faint" />
                <span className="grow">{o.name}</span>
                {here(o) && <Icon name="check" size="xs" />}
              </button>
            ))}
            {options.length === 0 && <div className="pal-group">No folder matches “{q}”</div>}
          </div>
          <footer className="pal-foot">
            <span className="hint">
              <Kbd>↵</Kbd>Move
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
