// Person card: click a name (thread header, recipients, compose chips,
// search people). A calm popover anchored to the name, with "Open full" for
// a centered modal. Everything comes from person_summary, which reads only
// the local index, so it opens instantly and works offline.
//
//   openPersonCard(address, anchorEl?)   — open (popover when anchored)
//   usePersonSummary(email)              — cached summary, shared by callers
//   <PersonCardCompact person={addr} />  — header + counts, for hover popovers
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import type { Address, PersonSummary } from "../../lib/types";
import { getUi, openThread, setUi, subscribeUi } from "../../lib/ui";
import { useDismiss } from "../../lib/dismiss";
import { api, asCommandError } from "../../lib/api";
import { registerShortcuts } from "../../lib/keyboard";
import { bytes, displayName, fileExt, fileTone, monthYear, shortDate } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { Icon } from "../../components/Icon";
import { Avatar, accountName } from "../../components/Identity";
import { toast } from "../../components/Toast";
import { accountById, isMe, meta } from "../../app/store";
import { meName } from "../../lib/me";
import { openComposeTo } from "../compose";
import { PersonMeetings } from "../calendar/PersonMeetings";
import { showTargetMenu } from "../../components/ContextMenu";
import { personMenu } from "./personMenu";

// ---------------------------------------------------------------------------
// Summary cache (shared by the card, the compact card and hover popovers)
// ---------------------------------------------------------------------------
const TTL_MS = 60_000;
const CACHE_MAX = 40;
type Entry = { at: number; data?: PersonSummary; error?: string; promise?: Promise<void> };
const cache = new Map<string, Entry>();
const cacheSubs = new Set<() => void>();
let cacheVersion = 0;
const bump = () => {
  cacheVersion++;
  cacheSubs.forEach((f) => f());
};

function load(email: string): Entry {
  const key = email.trim().toLowerCase();
  const hit = cache.get(key);
  if (hit && (hit.promise || Date.now() - hit.at < TTL_MS)) return hit;
  const entry: Entry = { at: Date.now(), data: hit?.data };
  entry.promise = api.personSummary(key).then(
    (data) => {
      entry.data = data;
      entry.error = undefined;
    },
    (e) => {
      entry.error = asCommandError(e).message;
    },
  ).finally(() => {
    entry.promise = undefined;
    entry.at = Date.now();
    bump();
  });
  cache.delete(key);
  cache.set(key, entry);
  while (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value!);
  return entry;
}

export function usePersonSummary(email: string | null): { data: PersonSummary | null; loading: boolean; error: string | null } {
  useSyncExternalStore(
    (cb) => {
      cacheSubs.add(cb);
      return () => cacheSubs.delete(cb);
    },
    () => cacheVersion,
  );
  const [entry, setEntry] = useState<Entry | null>(null);
  useEffect(() => {
    setEntry(email ? load(email) : null);
  }, [email]);
  const e = email ? cache.get(email.trim().toLowerCase()) ?? entry : null;
  return { data: e?.data ?? null, loading: !!e?.promise && !e?.data, error: e?.error ?? null };
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------
interface CardState {
  person: Address;
  /** Popover anchor (viewport coords); null = modal. */
  rect: DOMRect | null;
  /** The element it was opened from: pressing it again closes the card. */
  anchor: HTMLElement | null;
  returnFocus: HTMLElement | null;
}
let state: CardState | null = null;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

export function openPersonCard(person: Address, anchor?: HTMLElement | DOMRect | null) {
  const el = anchor instanceof HTMLElement ? anchor : null;
  if (el && state?.rect && state.anchor === el) return closePersonCard();
  const rect = anchor instanceof HTMLElement ? anchor.getBoundingClientRect() : anchor ?? null;
  state = { person, rect, anchor: el, returnFocus: el ?? (document.activeElement as HTMLElement | null) };
  void load(person.email);
  emit();
}

export function closePersonCard() {
  if (!state) return;
  const back = state.returnFocus;
  state = null;
  emit();
  back?.focus?.({ preventScroll: true });
}

export function isPersonCardOpen(): boolean {
  return state !== null;
}

function expand() {
  if (state) {
    state = { ...state, rect: null };
    emit();
  }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------
const POPOVER_W = 380;

export function PersonCardHost() {
  const s = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => state,
  );
  // Another overlay opening (compose, search) takes over.
  useEffect(() => {
    if (!s) return;
    let last = getUi().overlay;
    return subscribeUi(() => {
      const o = getUi().overlay;
      if (o !== last && o !== null) closePersonCard();
      last = o;
    });
  }, [s]);
  if (!s) return null;
  return <Card key={s.person.email + (s.rect ? "p" : "m")} s={s} />;
}

function Card({ s }: { s: CardState }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const popover = s.rect !== null;

  // Place the popover under (or above) the name, inside the window.
  useLayoutEffect(() => {
    if (!s.rect || !ref.current) return;
    const h = ref.current.offsetHeight;
    const margin = 12;
    const left = Math.min(Math.max(margin, s.rect.left), window.innerWidth - POPOVER_W - margin);
    const below = s.rect.bottom + 6;
    const top = below + h + margin > window.innerHeight ? Math.max(margin, s.rect.top - h - 6) : below;
    setPos({ left, top });
  }, [s.rect]);

  // The name it hangs from toggles it (PersonLink → openPersonCard).
  useDismiss(popover, closePersonCard, [ref, () => s.anchor]);
  useEffect(() => {
    ref.current?.focus({ preventScroll: true });
    return registerShortcuts([
      { id: "person.close", keys: "escape", label: "Close person card", group: "People", hidden: true, allowInInput: true, when: isPersonCardOpen, run: closePersonCard },
    ]);
  }, [popover]);

  const body = <CardBody person={s.person} popover={popover} />;
  if (popover) {
    return (
      <div
        ref={ref}
        className="panel pc-card pc-popover"
        role="dialog"
        aria-label={`About ${displayName(s.person)}`}
        tabIndex={-1}
        style={{ left: pos?.left ?? -9999, top: pos?.top ?? -9999, width: POPOVER_W }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            closePersonCard();
          } else if (e.key !== "Tab") {
            // The card has the keyboard: j/e/# etc. mustn't act on the thread behind it.
            e.stopPropagation();
          }
        }}
      >
        {body}
      </div>
    );
  }
  return (
    <>
      <div className="scrim" onMouseDown={closePersonCard} />
      <div className="overlay-host center pc-host" onMouseDown={(e) => e.target === e.currentTarget && closePersonCard()}>
        <div
          ref={ref}
          className="panel pc-card pc-modal"
          role="dialog"
          aria-modal="true"
          aria-label={`About ${displayName(s.person)}`}
          tabIndex={-1}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              closePersonCard();
            } else if (e.key !== "Tab") {
              // The card has the keyboard: j/e/# etc. mustn't act on the thread behind it.
              e.stopPropagation();
            }
          }}
        >
          {body}
        </div>
      </div>
    </>
  );
}

async function copyEmail(email: string) {
  try {
    await navigator.clipboard.writeText(email);
    toast({ message: "Email copied" });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't copy: ${asCommandError(e).message}` });
  }
}

function contactRange(d: PersonSummary): string | null {
  if (d.firstContact == null || d.lastContact == null) return null;
  const a = monthYear(d.firstContact);
  const b = shortDate(d.lastContact);
  return `Since ${a} · last ${b}`;
}

/** Name, address, domain — shared by the full and compact cards. */
function CardHeader({ person, data, big }: { person: Address; data: PersonSummary | null; big: boolean }) {
  // Yourself: your Google profile name (that address's account first).
  const mine = isMe(person.email);
  const accounts = meta.use((m) => m.accounts);
  const name = (mine ? meName(accounts, person.email) : null) ?? data?.name ?? person.name ?? null;
  const shown = name ?? person.email;
  return (
    <div className="pc-head">
      <Avatar person={{ name, email: person.email }} size={big ? "2xl" : "lg"} photo={!mine} />
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="pc-name">
          {shown}
          {mine && <span className="badge t-gray pc-you">You</span>}
        </div>
        <div className="pc-email">
          <span className="pc-email-text">{person.email}</span>
          <button className="btn btn-ghost btn-sm btn-icon" title="Copy email" aria-label="Copy email" onClick={() => void copyEmail(person.email)}>
            <Icon name="draft" size="xs" />
          </button>
        </div>
        {(data?.domain || person.email.includes("@")) && (
          <div className="pc-domain faint small">
            <Icon name="globe" size="xs" />
            {data?.domain ?? person.email.split("@")[1]}
          </div>
        )}
      </div>
    </div>
  );
}

function counts(d: PersonSummary): string {
  const parts = [];
  if (d.messagesFrom) parts.push(`${d.messagesFrom.toLocaleString()} from them`);
  if (d.messagesTo) parts.push(`${d.messagesTo.toLocaleString()} from you`);
  return parts.join(" · ") || "No mail yet";
}

function CardBody({ person, popover }: { person: Address; popover: boolean }) {
  const { data, loading, error } = usePersonSummary(person.email);
  const accounts = meta.use((m) => m.accounts);
  const me = isMe(person.email);
  const usual = data?.accounts.find((a) => accountById(a.accountId));

  const go = (accountId: string, threadId: string, messageId: string | null = null) => {
    closePersonCard();
    openThread({ accountId, threadId }, messageId);
  };

  return (
    <>
      <CardHeader person={person} data={data} big />
      {data && data.otherAddresses.length > 0 && (
        <div className="pc-also faint small">
          Also writes from{" "}
          {data.otherAddresses.map((a, i) => (
            <span key={a.email}>
              {i > 0 && ", "}
              <button className="pc-inline-link" onClick={() => openPersonCard(a, null)}>
                {a.email}
              </button>
            </span>
          ))}
        </div>
      )}

      <div className="pc-section">
        <div className="pc-title">In your mail</div>
        {error ? (
          <div className="pc-error small">Couldn't read your mail: {error}</div>
        ) : !data && loading ? (
          <div className="pc-loading">
            <span className="sk" style={{ width: "70%" }} />
            <span className="sk" style={{ width: "45%" }} />
          </div>
        ) : data ? (
          <>
            <div className="pc-fact">
              <Icon name="history" size="xs" />
              <span>{contactRange(data) ?? "You haven't exchanged mail yet"}</span>
            </div>
            <div className="pc-fact">
              <Icon name="mail" size="xs" />
              <span>{counts(data)}</span>
            </div>
            {data.accounts.length > 0 && (
              <div className="pc-accounts">
                {data.accounts.map((a) => {
                  const acct = accountById(a.accountId);
                  if (!acct) return null;
                  return (
                    <span key={a.accountId} className="pc-acct" title={acct.email}>
                      <i className={`dot dot-sm t-${accountTone(acct.color)}`} />
                      {accountName(acct, accounts)}
                      <span className="faint">{a.count.toLocaleString()}</span>
                    </span>
                  );
                })}
              </div>
            )}
          </>
        ) : null}
      </div>

      {!me && <PersonMeetings email={person.email} variant="card" />}

      {data && data.recentThreads.length > 0 && (
        <div className="pc-section">
          <div className="pc-title">Recent threads</div>
          {data.recentThreads.map((t) => (
            <button key={t.accountId + t.threadId} className="pc-row" onClick={() => go(t.accountId, t.threadId)}>
              <span className="grow pc-ellipsis">{t.subject || "(no subject)"}</span>
              <span className="faint tnum small">{shortDate(t.date)}</span>
            </button>
          ))}
        </div>
      )}

      {data && data.recentAttachments.length > 0 && !popover && (
        <div className="pc-section">
          <div className="pc-title">Shared files</div>
          {data.recentAttachments.map((f) => (
            <button
              key={f.messageId + f.attachment.id}
              className={"pc-row " + fileTone(f.attachment.filename, f.attachment.mimeType)}
              onClick={() => go(f.accountId, f.threadId, f.messageId)}
            >
              <span className="mini-ico">{fileExt(f.attachment.filename)}</span>
              <span className="grow pc-ellipsis">{f.attachment.filename}</span>
              <span className="faint small">{bytes(f.attachment.size)}</span>
            </button>
          ))}
        </div>
      )}

      <div className="pc-actions">
        {!me && (
          <button
            className="btn btn-primary btn-sm"
            onClick={() => {
              closePersonCard();
              void openComposeTo({ name: data?.name ?? person.name ?? null, email: person.email }, usual?.accountId);
            }}
          >
            <Icon name="compose" size="xs" />
            New email
          </button>
        )}
        <button
          className="btn btn-secondary btn-sm"
          onClick={() => {
            closePersonCard();
            setUi({ overlay: "search", searchPrefill: `from:${person.email} OR to:${person.email}` });
          }}
        >
          <Icon name="search" size="xs" />
          All mail with them
        </button>
        <span className="grow" />
        {!me && (
          <button
            className="btn btn-ghost btn-sm btn-icon"
            title="Find in Google Contacts"
            aria-label="Find in Google Contacts"
            onClick={() => void api.openExternal(`https://contacts.google.com/search/${encodeURIComponent(person.email)}`)}
          >
            <Icon name="users" size="xs" />
          </button>
        )}
        {popover && (
          <button className="btn btn-ghost btn-sm" onClick={expand} title="Open the full card">
            <Icon name="expand" size="xs" />
            Open full
          </button>
        )}
      </div>
    </>
  );
}

/** Header + counts only, sized for a hover popover (compose chips). */
export function PersonCardCompact({ person }: { person: Address }) {
  const { data } = usePersonSummary(person.email);
  return (
    <div className="pc-compact">
      <CardHeader person={person} data={data} big={false} />
      {data && (
        <div className="pc-fact small">
          <Icon name="mail" size="xs" />
          <span>
            {counts(data)}
            {data.lastContact != null ? ` · last ${shortDate(data.lastContact)}` : ""}
          </span>
        </div>
      )}
    </div>
  );
}

/** A name that opens the person card (thread headers, recipient lists). */
export function PersonLink({ person, label, className }: { person: Address; label?: string; className?: string }) {
  return (
    <button
      type="button"
      className={"person-link" + (className ? " " + className : "")}
      title={person.email}
      onClick={(e) => {
        e.stopPropagation();
        openPersonCard(person, e.currentTarget);
      }}
      onContextMenu={(e) => showTargetMenu(e, personMenu(person, e.currentTarget), { label: person.email })}
      onKeyDown={(e) => {
        // Claim Enter/Space so list shortcuts don't also act.
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          e.stopPropagation();
          openPersonCard(person, e.currentTarget);
        }
      }}
    >
      {label ?? displayName(person)}
    </button>
  );
}
