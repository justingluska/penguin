// Get to zero: archive every inbox conversation older than a day, a week…
// or all of it, in the open split or the whole inbox, keeping unread or
// starred ones if you like. Opened from ⌘K, the list header and a split's
// menu; one confirm, then the usual Archived toast with Undo (Z). Keys in the
// dialog: 1–6 pick the age, U / S toggle keeping unread / starred, W the
// scope, Enter archives. Settings → Inbox → "Get to zero" hides it.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { api, asCommandError } from "../../lib/api";
import { num } from "../../lib/format";
import { getUi } from "../../lib/ui";
import { currentSettings } from "../../lib/settings";
import { Modal } from "../../components/Modal";
import { Kbd } from "../../components/Kbd";
import { toast } from "../../components/Toast";
import { currentSplit, inboxQuery } from "../../app/store";
import { archive } from "../../app/actions";
import { splitTabs } from "../../app/splits";
import type { ThreadRef, ThreadSummary } from "../../lib/types";
import { AGES, cutoff, targets, type Age, type Keep } from "./zero";
import "./zero.css";

let open = false;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

/** Whether Get to zero can run now: on in Settings, over the inbox. */
export function canGetToZero(): boolean {
  return currentSettings().getToZero && getUi().view.kind === "inbox" && getUi().surface === "mail";
}

export function openGetToZero() {
  if (!currentSettings().getToZero) return;
  if (getUi().view.kind !== "inbox") return;
  open = true;
  emit();
}

function close() {
  open = false;
  emit();
}

export function GetToZeroHost() {
  const isOpen = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => open,
  );
  return isOpen ? <Dialog /> : null;
}

/** Every inbox conversation older than the cutoff in scope, paged from the backend. */
async function olderThan(wholeInbox: boolean, before: number | null): Promise<ThreadSummary[]> {
  const q = inboxQuery(wholeInbox);
  const PAGE = 1000;
  const out: ThreadSummary[] = [];
  let cursor = before;
  for (;;) {
    const page = await api.listThreads({ ...q, limit: PAGE, before: cursor });
    out.push(...page);
    if (page.length < PAGE) break;
    const next = page[page.length - 1].lastDate;
    if (next === cursor) break;
    cursor = next;
  }
  return out;
}

function Dialog() {
  const split = currentSplit();
  const splitName = split === null ? null : splitTabs(currentSettings().inboxSplits).find((t) => t.id === split)?.name ?? null;
  const [age, setAge] = useState<Age>("twoWeeks");
  const [keep, setKeep] = useState<Keep>({ unread: false, starred: true });
  const [whole, setWhole] = useState(split === null);
  const [rows, setRows] = useState<ThreadSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  // Count what the choice covers (again whenever it changes).
  useEffect(() => {
    let live = true;
    setRows(null);
    setError(null);
    olderThan(whole, cutoff(age, Date.now())).then(
      (r) => live && setRows(r),
      (e) => live && setError(asCommandError(e).message),
    );
    return () => {
      live = false;
    };
  }, [age, whole]);

  const refs: ThreadRef[] | null = rows && targets(rows, keep);
  const kept = rows && refs ? rows.length - refs.length : 0;
  const where = whole || !splitName ? "the inbox" : splitName;

  const run = () => {
    if (!refs || refs.length === 0) return;
    close();
    archive(refs);
    if (refs.length > 200) toast({ message: `Archiving ${num(refs.length)} conversations; Z undoes it` });
  };

  // The dialog's own keys. Modal holds every key back from the app in the
  // capture phase; this listener, added after it, still sees them.
  const hasScope = splitName !== null;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const i = Number(e.key);
      if (Number.isInteger(i) && i >= 1 && i <= AGES.length) setAge(AGES[i - 1].age);
      else if (e.key === "u" || e.key === "U") setKeep((k) => ({ ...k, unread: !k.unread }));
      else if (e.key === "s" || e.key === "S") setKeep((k) => ({ ...k, starred: !k.starred }));
      else if ((e.key === "w" || e.key === "W") && hasScope) setWhole((w) => !w);
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [hasScope]);

  return (
    <Modal label="Get to zero" onClose={close} onEnter={run} className="gz" initialFocus={confirmRef}>
      <div className="gz-body">
        <h2 className="st-confirm-title">Get to zero</h2>
        <p className="st-confirm-body">
          Archive conversations in {where} you haven't touched lately. They stay searchable in Done and come back if someone replies.
        </p>
        {splitName && (
          <div className="gz-scope" role="radiogroup" aria-label="Where">
            <button role="radio" aria-checked={!whole} className={"gz-opt" + (!whole ? " is-on" : "")} onClick={() => setWhole(false)}>
              {splitName}
            </button>
            <button role="radio" aria-checked={whole} className={"gz-opt" + (whole ? " is-on" : "")} onClick={() => setWhole(true)}>
              Whole inbox
            </button>
            <Kbd>W</Kbd>
          </div>
        )}
        <div className="gz-ages" role="radiogroup" aria-label="How old">
          {AGES.map((a, i) => (
            <button key={a.age} role="radio" aria-checked={age === a.age} className={"gz-age" + (age === a.age ? " is-on" : "")} onClick={() => setAge(a.age)}>
              <span>{a.label}</span>
              <Kbd>{String(i + 1)}</Kbd>
            </button>
          ))}
        </div>
        <div className="gz-keep">
          <label className="gz-check">
            <input type="checkbox" checked={keep.unread} onChange={() => setKeep((k) => ({ ...k, unread: !k.unread }))} />
            Keep unread <Kbd>U</Kbd>
          </label>
          <label className="gz-check">
            <input type="checkbox" checked={keep.starred} onChange={() => setKeep((k) => ({ ...k, starred: !k.starred }))} />
            Keep starred <Kbd>S</Kbd>
          </label>
        </div>
        <p className="gz-count" aria-live="polite">
          {error
            ? `Couldn't count them: ${error}`
            : refs === null
              ? "Counting…"
              : refs.length === 0
                ? `Nothing to archive${kept ? ` (keeping ${num(kept)})` : ""}.`
                : `${num(refs.length)} ${refs.length === 1 ? "conversation" : "conversations"} will be archived${kept ? `, ${num(kept)} kept` : ""}.`}
        </p>
        <div className="st-confirm-actions">
          <button className="btn btn-ghost" onClick={close}>
            Cancel
          </button>
          <button ref={confirmRef} className="btn btn-primary" disabled={!refs || refs.length === 0} onClick={run}>
            {refs && refs.length > 0 ? `Archive ${num(refs.length)}` : "Archive"}
            <Kbd>↵</Kbd>
          </button>
        </div>
      </div>
    </Modal>
  );
}
