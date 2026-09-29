// OWNER: ask agent (mounted by the search overlay, owned by ui-search).
//
// "Ask your inbox": when the query reads like a question ("when did I last
// email Priya?", or anything starting with "?") or the Search | Ask switch is on Ask,
// the overlay asks the backend (`ask`, deterministic, local) and shows this
// card above the results: a one-line answer, then the rich part for its kind
// (flight/stay/order/parcel/bill/booking cards, quoted sentences, the math
// behind a sum, a person card, a relationship sparkline), facts, cited
// messages, "did you mean" chips and follow-ups. Every claim cites a
// message; "How I got this" lists the exact queries run. Rich parts live in
// AskFacts.tsx; the "Understood as" chips and grouped rows in AskQuery.tsx.
//
// A question the grammar doesn't read exactly is then offered to Apple's
// on-device model (`ask_understand`, when Settings → AI allows it and this Mac
// can): it only turns the question into a query, which the backend checks
// and answers from local mail. Its answer replaces the first one.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { AskAnswer, AskCite, AskConfidence, AskItem, AskQuery, AskTimeline } from "../../lib/types";
import { displayName, shortDate } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { openPersonCard } from "../people/PersonCard";
import { tokenize } from "./query";
import { FactCards, Passages, PeopleCard, SumBlock } from "./AskFacts";
import { GroupRows, QueryChips } from "./AskQuery";
import { money } from "./askFormat";
import { wantsModel } from "./askQueryModel";
import "./ask.css";

const QUESTION_START =
  /^(when|who|whom|how|what|what's|whats|which|where|where's|why|did|do|does|have|has|am|are|is|was|show me the (latest|last)|latest|last (email|thing|time|invoice|receipt|reply)|track(ing)?|my (next |upcoming |last )?(flights?|hotel|stay|orders?|packages?|bills|reservations?|tickets)|upcoming (flights|bills|reservations)|cu[aá]ndo|cu[aá]nt[oa]s?|d[oó]nde|qui[eé]n|qu[eé]|cu[aá]l|mi (vuelo|pedido|paquete|hotel)|(total|sum|add) (cost|of|spent|spend|paid|for|my|up)|(first|oldest|earliest) (emails?|messages?|mail)|(his|her|their) (email|phone|number|address|contact))(\b|\s)/i;
/** "Priya's email", "Dana Whitfield's phone number". */
const CONTACT_OF = /^[^'’\s]+(\s[^'’\s]+){0,2}['’]s (email|phone|number|address|contact|cell|mobile)\b/i;

/** Does the input read like a question (mirror of penguin-core looks_like_question)? */
export function looksLikeQuestion(q: string): boolean {
  const t = q.trim();
  if (t.startsWith("?")) return t.length > 1;
  // Any search operator (is:unread, from:x, date:…) makes it a search.
  if (tokenize(t).some((tok) => tok.kind === "op")) return false;
  const words = t.split(/\s+/).filter(Boolean);
  if (words.length < 2) return false;
  return (
    QUESTION_START.test(t) ||
    (CONTACT_OF.test(t) && words.length <= 6) ||
    // "Linear total this year"
    (words.length >= 3 && words[1].toLowerCase() === "total") ||
    (t.endsWith("?") && words.length >= 3)
  );
}

/** "auto": ask when the query reads like a question; "ask"/"search": forced by the switch. */
export type AskMode = "auto" | "ask" | "search";

/** The person the last answer was about, so "when did he let us go" works. */
let lastPerson: string[] | null = null;

export interface AskState {
  answer: AskAnswer | null;
  /** The question `answer` belongs to. */
  question: string;
  loading: boolean;
  error: string | null;
  /** Apple's on-device model is reading the question. */
  understanding?: boolean;
  /** Ask again with an edited reading (the chips). */
  edit?: (query: AskQuery) => void;
}

/**
 * Ask `question` (debounced; stale answers dropped). Null question = off.
 * `initial` restores the answer Back from a thread handed back.
 */
export function useAsk(question: string | null, accountIds: string[] | null, initial?: AskAnswer | null): AskState {
  const [state, setState] = useState<AskState>({ answer: initial ?? null, question: initial?.question ?? "", loading: false, error: null });
  const seq = useRef(0);
  /** Back from a thread: show the restored answer without asking again. */
  const restored = useRef(initial ?? null);
  const key = accountIds?.join("\u0000") ?? "";
  const scopeRef = useRef<string[] | null>(accountIds);
  scopeRef.current = accountIds;
  const questionRef = useRef("");
  questionRef.current = state.question;
  const edit = useCallback((query: AskQuery) => {
    const q = questionRef.current;
    if (!q) return;
    const id = ++seq.current;
    setState((s) => ({ ...s, loading: true, understanding: false }));
    api
      .askQuery(q, query, { accountIds: scopeRef.current, person: lastPerson })
      .then((a) => {
        if (id === seq.current) setState({ answer: a, question: q, loading: false, error: null });
      })
      .catch((e: unknown) => {
        if (id === seq.current) setState((cur) => ({ ...cur, loading: false, error: asCommandError(e).message }));
      });
  }, []);
  useEffect(() => {
    const q = question?.trim().replace(/^\?\s*/, "") ?? "";
    if (!q) {
      seq.current++;
      setState((s) => (s.answer || s.loading || s.error ? { answer: null, question: "", loading: false, error: null } : s));
      return;
    }
    if (restored.current) {
      const r = restored.current;
      restored.current = null;
      if (r.question === q) return;
    }
    setState((s) => ({ ...s, loading: true }));
    const id = ++seq.current;
    const t = window.setTimeout(() => {
      api
        .ask(q, { accountIds, person: lastPerson })
        .then((a) => {
          if (id !== seq.current) return;
          if (a.person) lastPerson = a.person.emails;
          const model = wantsModel(a);
          setState({ answer: a, question: q, loading: false, error: null, understanding: model });
          if (!model) return;
          // The grammar only found sentences: let the on-device model read
          // the question (it answers null when it's off, can't run here, or
          // its reading doesn't check out).
          api
            .askUnderstand(q, { accountIds, person: lastPerson })
            .then((m) => {
              if (id !== seq.current) return;
              setState((s) => ({ ...s, answer: m ?? s.answer, understanding: false }));
            })
            .catch(() => {
              if (id === seq.current) setState((s) => ({ ...s, understanding: false }));
            });
        })
        .catch((e: unknown) => {
          if (id !== seq.current) return;
          setState({ answer: null, question: q, loading: false, error: asCommandError(e).message });
        });
    }, 120);
    return () => window.clearTimeout(t);
    // key stands for accountIds (a new array each render).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [question, key]);
  return { ...state, edit };
}

const CONFIDENCE: Record<AskConfidence, { label: string; title: string }> = {
  high: { label: "Exact", title: "Computed from every matching message in your local mail." },
  medium: { label: "Likely", title: "Found by fixed rules (a date or phrase in the text, a detected pattern). Check the cited messages." },
  low: { label: "Best guess", title: "Loosely matched, or a quoted sentence that may only partly answer it. Check the cited messages before relying on it." },
  none: { label: "No answer", title: "Nothing in your local mail matched." },
};

export interface AskCardProps {
  state: AskState;
  /** Open a cited message (thread at that message). */
  onOpen: (cite: AskCite, keep: boolean) => void;
  /** Ask another question (follow-ups, "did you mean"). */
  onAsk: (question: string) => void;
  /** Replace the query with a search. */
  onSearch: (query: string) => void;
  /** Keyboard selection among the cited items (index into answer.items), or -1. */
  activeItem: number;
  itemId: (i: number) => string;
  onHoverItem: (i: number) => void;
}

export function AskCard({ state, onOpen, onAsk, onSearch, activeItem, itemId, onHoverItem }: AskCardProps) {
  const [how, setHow] = useState(false);
  const [listOpen, setListOpen] = useState(false);
  const a = state.answer;
  if (state.error) {
    return (
      <div className="ask-card ask-none">
        <div className="ask-head">
          <Icon name="zap" size="xs" />
          <span>Answer</span>
        </div>
        <div className="ask-headline">Couldn't answer: {state.error}</div>
      </div>
    );
  }
  if (!a) {
    return state.loading ? (
      <div className="ask-card ask-loading" aria-busy="true">
        <div className="ask-head">
          <Icon name="zap" size="xs" />
          <span>Answering from your mail…</span>
        </div>
      </div>
    ) : null;
  }
  const conf = CONFIDENCE[a.confidence];
  const keep = (e: { metaKey: boolean; ctrlKey: boolean }) => e.metaKey || e.ctrlKey;
  const activeCite = activeItem >= 0 ? (a.items[activeItem] ?? null) : null;
  // A sum lists every email it added; keep that list folded until asked
  // for (or until the keyboard selection walks into it).
  const COLLAPSED = 3;
  const folded = (!!a.sum || a.intent === "query") && a.items.length > COLLAPSED && !listOpen && activeItem < COLLAPSED;
  const shownItems = folded ? a.items.slice(0, COLLAPSED) : a.items;
  const personCard = a.person && !a.person.company && (a.intent === "whoIs" || a.intent === "contactInfo");
  // The person card already shows their phone, address and (for contact
  // details) the addresses you've written with.
  const facts = personCard ? a.facts.filter((f) => f.label !== "Phone" && f.label !== "Address" && !(a.intent === "contactInfo" && f.label === "Email")) : a.facts;
  const allCards = personCard && a.intent === "whoIs" ? a.cards.filter((c) => c.fact.kind !== "contact") : a.cards;
  // A list answer ("Flights in 2025: 14") folds its cards like a sum's rows.
  const CARDS = 6;
  const cardsFolded = allCards.length > CARDS + 1 && !listOpen && activeItem < CARDS;
  const cards = cardsFolded ? allCards.slice(0, CARDS) : allCards;
  const rich = cards.length > 0 || a.passages.length > 0 || a.groups.length > 0;
  return (
    <section className={`ask-card${a.confidence === "none" ? " ask-none" : ""}${state.loading ? " ask-stale" : ""}`} aria-label="Answer">
      <div className="ask-head">
        <Icon name="zap" size="xs" />
        <span>Answer</span>
        <span className={`ask-conf ask-conf-${a.confidence}`} title={conf.title}>
          {conf.label}
        </span>
        <span className="grow" />
        <span className="faint" title="Answered on this Mac from the local index and the facts read from your mail. No language model writes the answer; nothing is sent anywhere.">
          {Math.max(1, Math.round(a.tookMs))} ms · on this Mac
        </span>
      </div>

      <div className="ask-headline">{a.headline}</div>
      {a.detail && <div className="ask-detail">{a.detail}</div>}
      {a.understood && state.edit && <QueryChips understood={a.understood} onEdit={state.edit} busy={state.loading} />}
      {state.understanding && (
        <div className="ask-understanding faint" aria-live="polite">
          <Icon name="sparkles" size="2xs" />
          Reading the question with Apple Intelligence, on this Mac…
        </div>
      )}
      {a.coverage && (
        <div className="ask-coverage faint">
          <Icon name="clock" size="2xs" />
          {a.coverage}
        </div>
      )}

      {personCard && a.person && <PeopleCard person={a.person} facts={a.facts} onOpen={onOpen} emails={a.intent === "contactInfo"} />}
      {a.sum && <SumBlock sum={a.sum} count={a.items.length} expanded={!folded} onToggle={() => setListOpen(!listOpen || folded)} />}
      <GroupRows groups={a.groups} measure={a.understood?.query.measure ?? (a.intent === "subscriptions" ? "money" : undefined)} compare={a.result?.kind === "compare"} onOpen={(c) => onOpen(c, false)} />
      <FactCards cards={cards} activeCite={activeCite} onOpen={onOpen} />
      {cardsFolded && (
        <button className="ask-more" onMouseDown={(e) => e.preventDefault()} onClick={() => setListOpen(true)}>
          {allCards.length - CARDS} more
        </button>
      )}
      <Passages passages={a.passages} activeCite={activeCite} onOpen={onOpen} />

      {a.candidates.length > 0 && (
        <div className="ask-chips ask-didyou">
          <span className="faint">Did you mean</span>
          {a.candidates.map((c) => (
            <button key={c.question} className="chip ask-chip" onMouseDown={(e) => e.preventDefault()} onClick={() => onAsk(c.question)}>
              {c.label}
            </button>
          ))}
        </div>
      )}

      {a.timeline && a.timeline.buckets.length > 1 && <Sparkline t={a.timeline} onOpen={(c) => onOpen(c, false)} />}

      {facts.length > 0 && (
        <dl className="ask-facts">
          {facts.map((f, i) => (
            <div key={`${f.label}-${i}`} className={`ask-fact${f.cite ? " is-cited" : ""}`}>
              <dt>{f.label}</dt>
              <dd>
                {f.cite ? (
                  <a onMouseDown={(e) => e.preventDefault()} onClick={(e) => onOpen(f.cite!, keep(e))} title="Open the message">
                    {f.value}
                  </a>
                ) : (
                  f.value
                )}
              </dd>
            </div>
          ))}
        </dl>
      )}

      {a.items.length > 0 && (
        <div className={`ask-items${rich ? " is-sources" : ""}`}>
          {rich && <div className="ask-items-head faint">{a.passages.length > 0 ? "Sources" : "Emails"}</div>}
          {shownItems.map((it, i) => (
            <CitedRow
              key={`${it.accountId}/${it.messageId}/${i}`}
              it={it}
              sid={itemId(i)}
              active={i === activeItem}
              onHover={() => onHoverItem(i)}
              onOpen={(k) => onOpen(it, k)}
            />
          ))}
          {folded && (
            <button className="ask-more" onMouseDown={(e) => e.preventDefault()} onClick={() => setListOpen(true)}>
              {a.items.length - COLLAPSED} more
            </button>
          )}
        </div>
      )}

      <div className="ask-foot">
        {a.person && (
          <button
            className="chip ask-chip"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => a.person && openPersonCard({ name: a.person.name, email: a.person.emails[0] })}
            title={a.person.emails.join(", ")}
            disabled={a.person.company}
          >
            <Icon name="user" size="2xs" />
            {a.person.label}
          </button>
        )}
        {a.followups.map((f) => (
          <button key={f.question} className="chip ask-chip" onMouseDown={(e) => e.preventDefault()} onClick={() => onAsk(f.question)} title={f.question}>
            {f.label}
          </button>
        ))}
        <span className="grow" />
        {a.searchQuery && (
          <button className="btn btn-ghost btn-sm" onMouseDown={(e) => e.preventDefault()} onClick={() => onSearch(a.searchQuery!)} title={a.searchQuery}>
            <Icon name="search" size="xs" />
            Show in search
          </button>
        )}
        <button className="btn btn-ghost btn-sm" onMouseDown={(e) => e.preventDefault()} onClick={() => setHow(!how)} aria-expanded={how}>
          <Icon name={how ? "up" : "down"} size="xs" />
          How I got this
        </button>
      </div>
      {how && (
        <ol className="ask-steps">
          {a.steps.map((s, i) => (
            <li key={i}>{s}</li>
          ))}
          {a.searchQuery && (
            <li>
              Search: <code>{a.searchQuery}</code>
            </li>
          )}
        </ol>
      )}
    </section>
  );
}

function CitedRow({ it, sid, active, onHover, onOpen }: { it: AskItem; sid: string; active: boolean; onHover: () => void; onOpen: (keep: boolean) => void }) {
  return (
    <a
      className={`ask-item${active ? " is-active" : ""}`}
      data-sid={sid}
      onMouseDown={(e) => e.preventDefault()}
      onMouseMove={onHover}
      onClick={(e) => onOpen(e.metaKey || e.ctrlKey)}
    >
      <span className="ask-item-who truncate">{it.sent ? "You" : displayName(it.from)}</span>
      <span className="ask-item-main">
        <span className="ask-item-subj truncate">{it.subject}</span>
        {it.note && <span className="ask-item-note truncate">{it.note}</span>}
      </span>
      {it.amount ? (
        <span className={`ask-amt${it.amount.value < 0 ? " neg" : ""}`}>{money(it.amount.value, it.amount.currency)}</span>
      ) : (
        <span />
      )}
      <span className="ask-item-date">{shortDate(it.date)}</span>
    </a>
  );
}



// --- sparkline ---------------------------------------------------------------

const H = 44;
const GAP = 2;

/** Stacked bars per period (them below, you above, 2px gap), marker dots underneath. */
function Sparkline({ t, onOpen }: { t: AskTimeline; onOpen: (c: AskCite) => void }) {
  const [hover, setHover] = useState<number | null>(null);
  const n = t.buckets.length;
  const max = Math.max(1, ...t.buckets.map((b) => b.fromThem + b.fromMe));
  const bw = Math.max(3, Math.min(14, Math.floor(560 / n) - GAP));
  const W = n * (bw + GAP);
  const scale = (v: number) => (v / max) * (H - 4);
  const bucketOf = (ms: number) => {
    let i = 0;
    while (i + 1 < n && t.buckets[i + 1].start <= ms) i++;
    return i;
  };
  const total = t.buckets.reduce((s, b) => ({ them: s.them + b.fromThem, me: s.me + b.fromMe }), { them: 0, me: 0 });
  const hb = hover !== null ? t.buckets[hover] : null;
  return (
    <div className="ask-spark">
      <div className="ask-spark-legend">
        <span>
          <i className="sw them" /> From them <b>{total.them}</b>
        </span>
        <span>
          <i className="sw me" /> From you <b>{total.me}</b>
        </span>
        <span className="grow" />
        <span className="faint">{hb ? `${hb.label} · ${hb.fromThem} from them · ${hb.fromMe} from you` : `Per ${t.unit}`}</span>
      </div>
      <svg className="ask-spark-svg" viewBox={`0 0 ${W} ${H + 14}`} width={W} height={H + 14} role="img" aria-label={`Messages per ${t.unit}`} onMouseLeave={() => setHover(null)}>
        {t.buckets.map((b, i) => {
          const x = i * (bw + GAP);
          const hThem = scale(b.fromThem);
          const hMe = scale(b.fromMe);
          const gap = hThem > 0 && hMe > 0 ? GAP : 0;
          return (
            <g key={b.start} onMouseEnter={() => setHover(i)} className={hover === i ? "on" : undefined}>
              <rect className="hit" x={x} y={0} width={bw + GAP} height={H} />
              {hThem > 0 && <rect className="them" x={x} y={H - hThem} width={bw} height={hThem} rx={Math.min(2, bw / 2)} />}
              {hMe > 0 && <rect className="me" x={x} y={H - hThem - gap - hMe} width={bw} height={hMe} rx={Math.min(2, bw / 2)} />}
              {b.fromThem + b.fromMe === 0 && <rect className="zero" x={x} y={H - 1} width={bw} height={1} />}
            </g>
          );
        })}
        {t.markers.map((m) => {
          const i = bucketOf(m.date);
          const cx = i * (bw + GAP) + bw / 2;
          return (
            <g key={`${m.label}-${m.date}`} className="mk" onClick={() => m.cite && onOpen(m.cite)}>
              <title>{`${m.label} · ${shortDate(m.date)}`}</title>
              <circle cx={cx} cy={H + 8} r={4} className={m.label === "Handoff" ? "mk-end" : "mk-dot"} />
            </g>
          );
        })}
      </svg>
      <div className="ask-spark-axis faint">
        <span>{t.buckets[0].label}</span>
        <span>{t.buckets[n - 1].label}</span>
      </div>
    </div>
  );
}
