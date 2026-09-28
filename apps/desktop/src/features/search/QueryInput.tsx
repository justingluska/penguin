// The search box: a real <input> with transparent text over a mirror layer
// that renders the same characters with operator highlighting. Both layers use
// one font so the caret and selection line up exactly. Autocomplete covers
// the values of operators people type (from:mi…); plain words get the
// suggestion strip under the box instead (suggest.ts), never a list of
// operator names: nobody should need to learn them.
import { useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import type { Account, Address, Label } from "../../lib/types";
import { Avatar } from "../../components/Identity";
import { displayName, toneForColor } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { quoteIfNeeded, tokenAt, tokenize, type Operator, type Token } from "./query";
import { matchPeople, usePeople } from "./people";
import { dateSuggestions, parseDatePhrase } from "./dates";

const DAYS: Array<[string, string]> = [
  ["weekend", "Saturdays and Sundays"],
  ["weekday", "Monday to Friday"],
  ["monday", "Mondays"],
  ["tuesday", "Tuesdays"],
  ["wednesday", "Wednesdays"],
  ["thursday", "Thursdays"],
  ["friday", "Fridays"],
  ["saturday", "Saturdays"],
  ["sunday", "Sundays"],
];

const SIZES: Array<[string, string]> = [
  ["1M", "1 MB"],
  ["5M", "5 MB"],
  ["10M", "10 MB"],
  ["25M", "25 MB"],
  ["500K", "500 KB"],
];

const STATIC_VALUES: Partial<Record<Operator, Array<[string, string]>>> = {
  has: [
    ["attachment", "Any attachment"],
    ["pdf", "PDF"],
    ["image", "Image"],
    ["doc", "Document"],
    ["spreadsheet", "Spreadsheet"],
    ["presentation", "Presentation"],
    ["invite", "Calendar invite"],
    ["link", "A link in the text"],
    ["otp", "Verification code or sign-in link"],
    ["unsubscribe", "An unsubscribe link"],
  ],
  in: [
    ["inbox", "Inbox"],
    ["sent", "Sent"],
    ["drafts", "Drafts"],
    ["done", "Done (archived)"],
    ["snoozed", "Snoozed"],
    ["trash", "Trash"],
    ["spam", "Spam"],
    ["starred", "Starred"],
    ["important", "Important"],
    ["calendar", "Calendar events"],
    ["anywhere", "Everywhere, including Trash and Spam"],
  ],
  is: [
    ["unread", "Unread"],
    ["read", "Read"],
    ["starred", "Starred"],
    ["unstarred", "Not starred"],
    ["important", "Important"],
    ["snoozed", "Snoozed"],
    ["new-sender", "First mail ever from that address"],
    ["first-outbound", "First mail you ever sent them"],
    ["known-sender", "From someone you've written to"],
    ["unanswered", "Received, you haven't replied"],
    ["awaiting", "You wrote last, no reply yet"],
    ["replied", "Received, and you replied"],
    ["reply", "Not the first message of its thread"],
    ["newsletter", "Newsletters and bulk mail"],
    ["sent", "Sent by you"],
    ["event", "Calendar events"],
  ],
  category: [
    ["primary", "Primary"],
    ["promotions", "Promotions"],
    ["social", "Social"],
    ["updates", "Updates"],
    ["forums", "Forums"],
  ],
  larger: SIZES,
  smaller: SIZES,
  size: SIZES.map(([v, h]) => [`>${v}`, `Larger than ${h}`]),
  messages: [
    [">2", "Threads with 3+ messages"],
    [">5", "Threads with 6+ messages"],
    [">10", "Threads with 11+ messages"],
    ["1", "Single messages"],
  ],
  day: DAYS,
  type: [["event", "Calendar events"]],
};

/** from:/to: specials offered before people. */
const PERSON_SPECIALS: Partial<Record<Operator, Array<[string, string]>>> = {
  from: [
    ["me", "Sent by you"],
    ["new", "New senders (first contact)"],
  ],
  to: [
    ["me", "Addressed to you"],
    ["new", "First email you sent them"],
  ],
  cc: [["me", "You were cc'd"]],
};

interface Suggestion {
  key: string;
  label: string;
  hint?: string;
  dotTone?: string;
  /** People get their avatar. */
  person?: Address;
  start: number;
  end: number;
  insert: string;
}

interface AcState {
  items: Suggestion[];
  anchor: number;
}

function suggest(
  tok: Token | null,
  caret: number,
  q: string,
  ctx: { people: ReturnType<typeof usePeople>; accounts: Account[]; labels: Label[] },
): AcState | null {
  if (!tok || tok.kind !== "op" || caret < tok.valueStart) return null;
  const typed = q.slice(tok.valueStart, caret).replace(/^"/, "").toLowerCase();
  const full = tok.value.toLowerCase();
  const base = { start: tok.valueStart, end: tok.end };
  let items: Suggestion[] = [];
  switch (tok.op) {
    case "from":
    case "to":
    case "cc":
    case "bcc":
    case "with": {
      const specials = (PERSON_SPECIALS[tok.op] ?? []).filter(([v]) => v.startsWith(typed));
      items = [
        ...specials.map(([v, h]) => ({ key: `:${v}`, label: v, hint: h, ...base, insert: v })),
        ...matchPeople(ctx.people, typed, new Set(), 6 - specials.length).map((a) => ({
          key: a.email,
          label: displayName(a),
          hint: a.email,
          person: a,
          ...base,
          insert: a.email,
        })),
      ];
      break;
    }
    case "domain": {
      // Domains of people seen locally, most-seen first.
      const seen = new Set<string>();
      for (const p of ctx.people) {
        const d = p.address.email.split("@")[1]?.toLowerCase();
        if (!d || seen.has(d) || !d.startsWith(typed.replace(/^@/, ""))) continue;
        seen.add(d);
        items.push({ key: d, label: d, ...base, insert: d });
        if (items.length >= 8) break;
      }
      break;
    }
    case "label": {
      const seen = new Set<string>();
      for (const l of ctx.labels) {
        if (l.kind !== "user") continue;
        const n = l.name.toLowerCase();
        if (seen.has(n) || !n.startsWith(typed)) continue;
        seen.add(n);
        items.push({ key: n, label: l.name, dotTone: toneForColor(l.color), ...base, insert: quoteIfNeeded(l.name) });
      }
      items = items.slice(0, 8);
      break;
    }
    case "account":
      items = ctx.accounts
        .filter(
          (a) =>
            a.email.toLowerCase().startsWith(typed) ||
            (a.nickname ?? "").toLowerCase().startsWith(typed) ||
            (a.displayName ?? "").toLowerCase().startsWith(typed),
        )
        .map((a) => ({ key: a.id, label: a.email, hint: a.nickname ?? a.displayName ?? undefined, dotTone: accountTone(a.color), ...base, insert: a.email }));
      break;
    case "date":
    case "on":
    case "before":
    case "after":
    case "since":
    case "until": {
      // Multi-word values insert quoted: date:"last week".
      items = dateSuggestions()
        .filter((v) => v.startsWith(typed))
        .slice(0, 8)
        .map((v) => ({ key: v, label: v, ...base, insert: quoteIfNeeded(v) }));
      break;
    }
    default: {
      const vals = STATIC_VALUES[tok.op];
      if (vals) items = vals.filter(([v]) => v.startsWith(typed)).map(([v, h]) => ({ key: v, label: v, hint: h, ...base, insert: v }));
    }
  }
  // A value that is already complete needs no dropdown.
  if (items.length === 1 && items[0].insert.toLowerCase() === full) return null;
  if (items.length === 0) return null;
  return { anchor: tok.valueStart, items };
}

function Mirror({ tokens }: { tokens: Token[] }) {
  return (
    <>
      {tokens.map((t) => {
        switch (t.kind) {
          case "space":
            return <span key={t.start}>{t.text}</span>;
          case "or":
          case "paren":
            return (
              <span key={t.start} data-s={t.start} className="qm-op">
                {t.text}
              </span>
            );
          case "phrase":
          case "word":
            return (
              <span key={t.start} data-s={t.start} className={t.negated ? "qm-neg" : t.kind === "phrase" ? "qm-phrase" : undefined}>
                {t.text}
              </span>
            );
          case "op": {
            const keyEnd = t.valueStart;
            const negLen = t.negated ? 1 : 0;
            return (
              <span key={t.start} data-s={t.start} className={t.negated ? "qm-negop" : undefined}>
                {t.negated && <span className="qm-op">-</span>}
                <span className="qm-op">{t.text.slice(negLen, keyEnd - t.start)}</span>
                <span data-s={t.valueStart} className={t.op === "date" && parseDatePhrase(t.value) ? "qm-val qm-date" : "qm-val"}>
                  {t.text.slice(keyEnd - t.start)}
                </span>
              </span>
            );
          }
        }
      })}
    </>
  );
}

export function QueryInput({
  value,
  onChange,
  onKeyDown,
  inputRef,
  accounts,
  labels,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  /** Keys the autocomplete didn't consume. */
  onKeyDown: (e: KeyboardEvent<HTMLInputElement>) => void;
  inputRef: RefObject<HTMLInputElement | null>;
  accounts: Account[];
  labels: Label[];
  placeholder: string;
}) {
  const mirrorRef = useRef<HTMLDivElement>(null);
  const [caret, setCaret] = useState(value.length);
  const [acIndex, setAcIndex] = useState(0);
  const [dismissedAt, setDismissedAt] = useState<string | null>(null);
  const pendingCaret = useRef<number | null>(null);
  const people = usePeople();

  const tokens = useMemo(() => tokenize(value), [value]);
  const stateKey = `${value}\u0000${caret}`;
  const ac = useMemo(() => {
    if (dismissedAt === stateKey) return null;
    return suggest(tokenAt(tokens, caret), caret, value, { people, accounts, labels });
  }, [tokens, caret, value, people, accounts, labels, dismissedAt, stateKey]);
  const acKeys = ac?.items.map((i) => i.key).join("|");
  const idx = ac ? Math.min(acIndex, ac.items.length - 1) : 0;

  useLayoutEffect(() => setAcIndex(0), [acKeys]);

  // Only when the text or caret moved: reading scrollLeft forces a layout of
  // the whole overlay, and the overlay re-renders on every search answer.
  useLayoutEffect(() => {
    const input = inputRef.current;
    if (input && pendingCaret.current !== null) {
      input.setSelectionRange(pendingCaret.current, pendingCaret.current);
      setCaret(pendingCaret.current);
      pendingCaret.current = null;
    }
    syncScroll();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, caret]);

  function syncScroll() {
    if (mirrorRef.current && inputRef.current) mirrorRef.current.scrollLeft = inputRef.current.scrollLeft;
  }

  function readCaret() {
    const input = inputRef.current;
    if (input) setCaret(input.selectionStart ?? input.value.length);
    syncScroll();
  }

  function accept(s: Suggestion) {
    const after = value.slice(s.end);
    const sep = after.startsWith(" ") ? "" : " ";
    const next = value.slice(0, s.start) + s.insert + sep + after;
    pendingCaret.current = s.start + s.insert.length + 1;
    onChange(next);
  }

  function handleKey(e: KeyboardEvent<HTMLInputElement>) {
    if (ac && !e.metaKey && !e.ctrlKey && !e.altKey) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = ac.items.length;
        setAcIndex((idx + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
        return;
      }
      if ((e.key === "Tab" && !e.shiftKey) || e.key === "Enter") {
        e.preventDefault();
        accept(ac.items[idx]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissedAt(stateKey);
        return;
      }
    }
    onKeyDown(e);
  }

  // Place the dropdown under the token being completed.
  let acLeft = 0;
  if (ac && mirrorRef.current) {
    const el = mirrorRef.current.querySelector<HTMLElement>(`[data-s="${ac.anchor}"]`);
    if (el) acLeft = el.offsetLeft - mirrorRef.current.scrollLeft;
  }

  return (
    <div className="qi">
      <div className="qi-mirror" ref={mirrorRef} aria-hidden="true">
        <Mirror tokens={tokens} />
        {"​"}
      </div>
      <input
        ref={inputRef}
        className="qi-input"
        value={value}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        aria-label="Search mail"
        aria-autocomplete="list"
        aria-expanded={!!ac}
        onChange={(e) => {
          onChange(e.target.value);
          setCaret(e.target.selectionStart ?? e.target.value.length);
        }}
        onSelect={readCaret}
        onKeyUp={readCaret}
        onScroll={syncScroll}
        onKeyDown={handleKey}
      />
      {ac && (
        <div className="qi-ac panel menu" style={{ left: Math.max(0, acLeft - 10) }} role="listbox">
          {ac.items.map((s, i) => (
            <div
              key={s.key}
              role="option"
              aria-selected={i === idx}
              className={`menu-item${i === idx ? " active" : ""}`}
              onMouseDown={(e) => {
                e.preventDefault();
                accept(s);
              }}
            >
              {s.dotTone && <i className={`dot dot-sm t-${s.dotTone}`} />}
              {s.person && <Avatar person={s.person} size="xs" />}
              <span>{s.label}</span>
              {s.hint && <span className="qi-ac-hint truncate">{s.hint}</span>}
              {i === idx && <span className="kbd">↵</span>}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
