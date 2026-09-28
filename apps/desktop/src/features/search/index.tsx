// OWNER: ui-search agent. Mounted once by App; renders when ui.overlay === "search".
//
// The search overlay (design/03-search.html). Every keystroke searches the
// local index at once, one search in flight at a time (useInstantSearch);
// answers for the query already on screen merge in without moving anything
// at or above the selection (stable.ts). Plain words get suggestions under
// the box (suggest.ts) and plain English becomes chips (nl.ts); nothing asks
// anyone to learn operators. Design notes and sources: docs/SEARCH-UX.md.
import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { api, onSyncStatus } from "../../lib/api";
import { getUi, openThread as openThreadView, setUi, useUi } from "../../lib/ui";
import type { Account, AttachmentHit, CalendarEvent, Label, PersonHit, SearchChip, SearchHit, SearchResponse, SyncStatus } from "../../lib/types";
import { bytes, displayName, fileExt, fileTone, initials, num, personTone, shortDate, toneForColor } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { Icon, type IconName } from "../../components/Icon";
import { QueryInput } from "./QueryInput";
import { currentDate, isDateOp, opValues, removeRaw, setDate, setOp, toggleOp, tokenize } from "./query";
import { sanitizeSnippet } from "./sanitize";
import { addRecent, clearRecents, removeRecent, toggleSaved, useSearchLists } from "./storage";
import { seedPeople, usePeople } from "./people";
import { interpretDetailed, withoutPart } from "./nl";
import { SearchTips } from "./SearchTips";
import { DATE_PRESETS, HAS_OPTIONS, useFacetCounts } from "./facets";
import { applyDevParams, devParam } from "./dev";
import { dateChipLabel, dateHint, parseDatePhrase } from "./dates";
import { Avatar, accountName } from "../../components/Identity";
import { useActiveProfile } from "../../app/profiles";
import { listOrderedAccounts } from "../../app/store";
import {
  ServerResultsGroup,
  ServerSearchButton,
  serverName,
  coverageText,
  shouldSuggestServer,
  useCoverage,
  useServerSearch,
  type ServerSearchState,
} from "./ServerResults";
import "./search.css";
import { showTargetMenu } from "../../components/ContextMenu";
import { searchItemMenu } from "./searchMenus";
import { openRuleEditor } from "../rules/state";
import { EventResults, eventItemId } from "../calendar/EventResults";
import { openEvent } from "../calendar/state";
import { AskCard, looksLikeQuestion, useAsk, type AskMode } from "./AskCard";
import type { AskAnswer, AskCite } from "../../lib/types";
import { useInstantSearch } from "./useInstantSearch";
import { stabilize } from "./stable";
import { suggestFor, type Suggestion } from "./suggest";
import { didYouMean } from "./spelling";
import { useSetting } from "../../lib/settings";
import { isPinned } from "../smart/catalog";
import { pinSearchToSidebar } from "../smart/actions";

/** Last query of this session, offered (selected) when the overlay reopens. */
let lastQuery = "";

/**
 * What Back from a thread opened here restores (via lib/ui's nav stack): the
 * same query and scope, the results as they were (shown at once, refreshed
 * quietly), the Gmail group, the scroll position and the selected result.
 */
interface SearchSnapshot {
  query: string;
  unscoped: boolean;
  resp: SearchResponse | null;
  respQuery: string;
  sel: number;
  /** The selected result (kept across the quiet refresh). */
  selId?: string | null;
  scrollTop: number;
  server: ServerSearchState;
  /** Ask: the answer on screen and the Search | Ask mode. */
  ask?: AskAnswer | null;
  askMode?: AskMode;
}

/** The snapshot Back handed to this overlay, if any (cleared once mounted). */
function pendingSnapshot(): SearchSnapshot | null {
  return getUi().overlayRestore as SearchSnapshot | null;
}

/** Open the overlay, optionally with a query (used by ⌘K "Search mail for…"). */
export function openSearch(query?: string) {
  setUi({ overlay: "search", searchPrefill: query ?? null });
}

export function SearchOverlay() {
  const overlay = useUi((s) => s.overlay);
  useEffect(applyDevParams, []);
  if (overlay !== "search") return null;
  return <SearchPanel />;
}

// ---------------------------------------------------------------------------

type Item =
  | { group: "ask"; id: string; cite: AskCite }
  | { group: "top" | "threads" | "related" | "server" | "corrected"; id: string; hit: SearchHit }
  | { group: "attachments"; id: string; att: AttachmentHit }
  | { group: "people"; id: string; person: PersonHit }
  | { group: "events"; id: string; event: CalendarEvent }
  | { group: "recent" | "saved"; id: string; query: string }
  | { group: "fix"; id: string; label: string; query: string; unscope?: boolean };

/** The order groups are shown (and walked by ↑↓) in. */
const GROUP_ORDER = ["ask", "corrected", "top", "events", "attachments", "threads", "related", "server", "fix", "people", "recent", "saved"] as const;

const THREADS_SHOWN = 80;
/** Top results: the relevance band above the newest-first list (Mackenzie et al.'s hybrid; Apple Mail's "Top Results"). */
const TOP_SHOWN = 3;
/** File cards: one row, unless the query is about files. */
const ATTACHMENTS_SHOWN = 3;
const ATTACHMENTS_SHOWN_FILES = 6;
const NO_ENTERED = new Set<string>();

/** Matched by its words (older backends leave matchedBy empty: words). */
const byWords = (h: SearchHit) => !h.matchedBy?.length || h.matchedBy.includes("words");

function SearchPanel() {
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);
  // Back from a thread opened from here: restore instead of starting fresh.
  const [snap] = useState(pendingSnapshot);
  /** Esc right after a restore closes (the "second Esc"), until the query changes. */
  const restoredQuery = useRef(snap?.query ?? null);
  const [query, setQuery] = useState(() => snap?.query ?? devParam("q") ?? getUi().searchPrefill ?? lastQuery);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [labels, setLabels] = useState<Label[]>([]);
  const [sync, setSync] = useState<Record<string, SyncStatus>>({});
  const [flash, setFlash] = useState<string | null>(null);
  // Default scope: the active profile's accounts. Removing the chip searches
  // every account for the rest of this search (the profile stays active).
  const active = useActiveProfile();
  const [unscoped, setUnscoped] = useState(snap?.unscoped ?? false);
  const profile = unscoped ? null : active;
  const scopeIds = profile?.accountIds ?? null;
  const scopeKey = scopeIds?.join("\u0000") ?? "";
  const lists = useSearchLists();
  const smartViews = useSetting("smartViews");
  // Ask: automatic for a question-shaped query or a "?" prefix, or forced by the Search | Ask switch.
  const [askMode, setAskMode] = useState<AskMode>(snap?.askMode ?? "auto");
  const askActive = askMode === "ask" || (askMode === "auto" && looksLikeQuestion(query));
  const ask = useAsk(askActive ? query : null, scopeIds, snap?.ask);
  // Plain English ("from mike last week pdf") read as filters, shown as
  // chips, unless the user asked for the words (per query) or it's a question.
  const knownPeople = usePeople();
  const [literal, setLiteral] = useState<Set<string>>(() => new Set());
  const nl = useMemo(() => {
    if (askActive || literal.has(query.trim())) return null;
    const isPerson = (w: string) =>
      knownPeople.some((p) => p.address.email.toLowerCase().startsWith(`${w}@`) || (p.address.name ?? "").toLowerCase().split(/\s+/).includes(w));
    return interpretDetailed(query, { isPerson });
  }, [query, askActive, literal, knownPeople]);
  const nlQuery = nl?.query ?? null;
  // While asking, the results below the answer are the mail behind it.
  const searchText = askActive ? (ask.answer?.searchQuery ?? "") : (nlQuery ?? query);
  /** The structured query facets and chips edit (the interpretation, when there is one). */
  const editable = nlQuery ?? query;
  const [tipsOpen, setTipsOpen] = useState(false);

  // --- lifecycle -----------------------------------------------------------
  useEffect(() => {
    const input = inputRef.current;
    if (input) {
      input.focus();
      // The previous query is pre-selected for instant overwrite (a restored
      // search keeps the caret at the end: you came back to it).
      if (devParam("q") === null && !snap) input.select();
      else input.setSelectionRange(input.value.length, input.value.length);
    }
    if (getUi().searchPrefill !== null) setUi({ searchPrefill: null });
    if (getUi().overlayRestore !== null) setUi({ overlayRestore: null });
    seedPeople();
    listOrderedAccounts().then(setAccounts).catch(() => setAccounts([]));
    api.listLabels(null).then(setLabels).catch(() => setLabels([]));
    api
      .syncStatus()
      .then((list) => setSync(Object.fromEntries(list.map((s) => [s.accountId, s]))))
      .catch(() => undefined);
    const un = onSyncStatus((s) => setSync((m) => ({ ...m, [s.accountId]: s })));
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    lastQuery = query;
  }, [query]);

  // --- query → results -------------------------------------------------------
  const search = useInstantSearch(searchText, scopeIds, THREADS_SHOWN + 1, snap ? { resp: snap.resp, respQuery: snap.respQuery } : undefined);
  const { resp, respQuery, error } = search;
  const counts = useFacetCounts(searchText, resp, respQuery, scopeIds);

  // --- derived --------------------------------------------------------------
  const terms = useMemo(() => freeTerms(searchText), [searchText]);
  // Free text is never a date; offer to turn a date-like phrase into date:.
  const [ignoredHints, setIgnoredHints] = useState<Set<string>>(() => new Set());
  const hint = useMemo(() => {
    const words = tokenize(query).filter(
      (t): t is Extract<ReturnType<typeof tokenize>[number], { kind: "word" }> =>
        t.kind === "word" && !t.negated && !/[:"]/.test(t.text),
    );
    const h = dateHint(words, query);
    return h && !nlQuery && !ignoredHints.has(h.raw.toLowerCase()) ? h : null;
  }, [query, ignoredHints, nlQuery]);
  function applyHint() {
    if (!hint) return;
    edit(query.slice(0, hint.start) + hint.rewrite + query.slice(hint.end));
  }
  const accById = useMemo(() => Object.fromEntries(accounts.map((a) => [a.id, a])), [accounts]);
  // Facets and account: suggestions offer only the accounts in scope.
  const scopedAccounts = useMemo(
    () => (scopeIds ? accounts.filter((a) => scopeIds.includes(a.id)) : accounts),
    [accounts, scopeKey],
  );
  const serverLabel = serverName(scopedAccounts.map((a) => a.provider));
  const labelById = useMemo(() => new Map(labels.map((l) => [`${l.accountId}/${l.id}`, l])), [labels]);
  const mine = useMemo(() => accounts.map((a) => a.email.toLowerCase()), [accounts]);
  const empty = !query.trim();
  const hits = resp?.hits ?? [];
  // Top results by rank; then what matched the words, newest first; then what
  // is only close in meaning, closest first.
  const top = useMemo(() => hits.slice(0, hits.length <= TOP_SHOWN + 1 ? hits.length : TOP_SHOWN), [hits]);
  const threads = useMemo(
    () => hits.slice(top.length, THREADS_SHOWN + 1).filter(byWords).sort((a, b) => b.date - a.date),
    [hits, top],
  );
  const related = useMemo(() => hits.slice(top.length, THREADS_SHOWN + 1).filter((h) => !byWords(h)), [hits, top]);
  const anyWords = hits.some(byWords);
  const fileIntent = /(^|\s)-?(has|filename|larger|smaller|size):/.test(searchText);
  const atts = useMemo(() => (resp?.attachments ?? []).slice(0, fileIntent ? ATTACHMENTS_SHOWN_FILES : ATTACHMENTS_SHOWN), [resp, fileIntent]);
  const people = useMemo(() => resp?.people ?? [], [resp]);
  const events = useMemo(() => resp?.events ?? [], [resp]);
  // While asking, the answer card speaks for an empty result list.
  const noResults = !!resp && !empty && !askActive && hits.length === 0 && atts.length === 0 && events.length === 0;
  // "Did you mean": a word no mail contains, corrected, run, and shown when it finds mail.
  const correction = useCorrection(noResults || (!!resp && !empty && !askActive && !anyWords) ? editable : null, scopeIds);
  const candidates = useMemo(
    () => (noResults ? broaden(editable, resp?.chips ?? [], !!profile) : []),
    [noResults, editable, resp, profile],
  );
  const fixes = useVerifiedFixes(candidates, scopeIds);
  // "Also search Gmail" (mail outside the fully-downloaded sync window).
  const server = useServerSearch(searchText, scopeIds, snap?.server);
  const coverage = useCoverage(scopeIds);
  const suggestServer = !empty && events.length === 0 && shouldSuggestServer(resp, coverage, hits.length);
  const localKeys = useMemo(() => new Set(hits.map((h) => `${h.accountId}/${h.threadId}`)), [hits]);
  // Gmail matches not already among the local hits (keyboard-selectable, after Threads).
  const serverHits = useMemo(
    () => (server.state.status === "done" ? server.state.resp.hits.filter((h) => !localKeys.has(`${h.accountId}/${h.threadId}`)) : []),
    [server.state, localKeys],
  );
  // Suggestions under the box as you type (people, companies, recents, refinements).
  const suggestions = useMemo(
    () => (askActive || tipsOpen ? [] : suggestFor(query, { people: knownPeople, recent: lists.recent, mine, hits: respQuery === searchText ? hits : [] })),
    [query, askActive, tipsOpen, knownPeople, lists.recent, mine, hits, respQuery, searchText],
  );

  const fresh: Item[] = useMemo(() => {
    if (empty) {
      return [
        ...lists.recent.map((q): Item => ({ group: "recent", id: `r:${q}`, query: q })),
        ...lists.saved.map((q): Item => ({ group: "saved", id: `s:${q}`, query: q })),
      ];
    }
    const hitItem = (group: "top" | "threads" | "related" | "server" | "corrected", h: SearchHit): Item => ({ group, id: `h:${h.accountId}/${h.threadId}`, hit: h });
    const serverItems = serverHits.map((h): Item => ({ ...hitItem("server", h), id: `g:${h.accountId}/${h.threadId}` }));
    const askItems = askActive && ask.answer ? ask.answer.items.map((c, i): Item => ({ group: "ask", id: `k:${i}`, cite: c })) : [];
    const fixed = correction ? correction.resp.hits.slice(0, THREADS_SHOWN).map((h) => ({ ...hitItem("corrected", h), id: `c:${h.accountId}/${h.threadId}` })) : [];
    if (noResults)
      return [
        ...askItems,
        ...fixed,
        ...serverItems,
        ...(fixes ?? []).map((f): Item => ({ group: "fix", id: `f:${f.unscope ? "*" : f.query}`, label: f.label, query: f.query, unscope: f.unscope })),
      ];
    const out: Item[] = [...askItems];
    top.forEach((h) => out.push(hitItem("top", h)));
    events.forEach((e) => out.push({ group: "events", id: eventItemId(e), event: e }));
    atts.forEach((a) => out.push({ group: "attachments", id: `a:${a.messageId}/${a.attachment.id}`, att: a }));
    threads.forEach((h) => out.push(hitItem("threads", h)));
    related.forEach((h) => out.push(hitItem("related", h)));
    out.push(...serverItems);
    people.forEach((p) => out.push({ group: "people", id: `p:${p.address.email}`, person: p }));
    return out;
  }, [empty, noResults, fixes, correction, top, events, atts, threads, related, serverHits, people, lists, askActive, ask.answer]);

  // Stable results: a new answer for the query on screen merges in below the
  // selection; a new query starts a fresh list with the first row selected.
  const displayKey = empty ? "" : `${respQuery}\u0001${scopeKey}\u0001${askActive ? "ask" : "search"}\u0001${noResults ? "none" : "some"}`;
  const [sel, setSel] = useState<{ key: string; id: string | null; pos: number }>(() => ({
    key: snap ? displayKey : "",
    id: snap?.selId ?? null,
    pos: snap?.sel ?? 0,
  }));
  const selNow = sel.key === displayKey ? sel : { key: displayKey, id: null, pos: 0 };
  const shownRef = useRef<{ key: string; items: Item[] } | null>(null);
  const prevShown = shownRef.current;
  const anchorId = selNow.id ?? prevShown?.items[selNow.pos]?.id ?? null;
  const { items, entered } = useMemo(() => {
    if (!prevShown || prevShown.key !== displayKey || empty) return { items: fresh, entered: NO_ENTERED };
    return stabilize(prevShown.items, fresh, anchorId, GROUP_ORDER);
    // prevShown is what was painted last (a ref); fresh, the key and the anchor decide.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fresh, displayKey, anchorId, empty]);
  useLayoutEffect(() => {
    shownRef.current = { key: displayKey, items };
  }, [items, displayKey]);
  const index = useMemo(() => new Map(items.map((it, i) => [it.id, i])), [items]);
  const selIdx = Math.max(0, Math.min((selNow.id !== null ? index.get(selNow.id) : undefined) ?? selNow.pos, items.length - 1));
  const selId = items[selIdx]?.id;
  const select = (i: number) => setSel({ key: displayKey, id: items[i]?.id ?? null, pos: i });
  /** Rows in each group, in the (stable) order shown. */
  const shown = useMemo(() => {
    const g: Record<string, Item[]> = {};
    for (const it of items) (g[it.group] ??= []).push(it);
    const hitsOf = (k: string) => (g[k] ?? []).map((it) => (it as { hit: SearchHit }).hit);
    return {
      top: hitsOf("top"),
      threads: hitsOf("threads"),
      related: hitsOf("related"),
      corrected: hitsOf("corrected"),
      atts: (g.attachments ?? []).map((it) => (it as { att: AttachmentHit }).att),
      events: (g.events ?? []).map((it) => (it as { event: CalendarEvent }).event),
    };
  }, [items]);

  // A restored search reopens at the scroll position it had (before the
  // keep-selection-visible effect below runs).
  useLayoutEffect(() => {
    if (snap && resultsRef.current) resultsRef.current.scrollTop = snap.scrollTop;
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once, on mount
  }, []);

  useEffect(() => {
    if (!selId) return;
    const el = document.querySelector<HTMLElement>(`[data-sid="${CSS.escape(selId)}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [selId]);

  /** Rows move under a still pointer when results change; only a real pointer move selects. */
  const pointer = useRef<{ x: number; y: number } | null>(null);

  // --- actions --------------------------------------------------------------
  function close() {
    setUi({ overlay: null });
  }

  function openThread(accountId: string, threadId: string, messageId: string, keep: boolean) {
    addRecent(query);
    const restore: SearchSnapshot = {
      query,
      unscoped,
      resp,
      respQuery,
      sel: selIdx,
      selId: selId ?? null,
      scrollTop: resultsRef.current?.scrollTop ?? 0,
      // An in-flight Gmail search answers the panel that asked; start it over.
      server: server.state.status === "loading" ? { status: "idle" } : server.state,
      ask: ask.answer,
      askMode,
    };
    // Back from the thread returns here. ⌘Enter keeps the overlay up too.
    openThreadView({ accountId, threadId }, messageId, { kind: "search", restore }, keep);
  }

  /** Select a chip's source text in the box (to fix an unreadable date:). */
  function selectRaw(raw: string) {
    const at = query.indexOf(raw);
    const input = inputRef.current;
    if (at === -1 || !input) return;
    input.focus();
    input.setSelectionRange(at, at + raw.length);
  }

  function edit(next: string) {
    setQuery(next);
    requestAnimationFrame(() => {
      const input = inputRef.current;
      if (!input) return;
      input.focus();
      input.setSelectionRange(next.length, next.length);
    });
  }

  function run(it: Item, keep: boolean) {
    switch (it.group) {
      case "ask":
        openThread(it.cite.accountId, it.cite.threadId, it.cite.messageId, keep);
        break;
      case "top":
      case "threads":
      case "related":
      case "corrected":
      case "server":
        openThread(it.hit.accountId, it.hit.threadId, it.hit.messageId, keep);
        break;
      case "events":
        addRecent(query);
        close();
        openEvent(it.event);
        break;
      case "attachments":
        openThread(it.att.accountId, it.att.threadId, it.att.messageId, keep);
        break;
      case "people": {
        // Pivot to all mail from this person; drop bare words that named them.
        const name = (it.person.address.name ?? "").toLowerCase();
        const kept = tokenize(query)
          .filter((t) => !(t.kind === "word" && !t.negated && name.split(/\s+/).some((w) => w.startsWith(t.text.toLowerCase()))))
          .map((t) => t.text)
          .join("");
        edit(setOp(kept, "from", it.person.address.email));
        break;
      }
      case "fix":
        if (it.unscope) setUnscoped(true);
        else edit(it.query);
        break;
      case "recent":
      case "saved":
        edit(it.query);
        break;
    }
  }

  /** Take a suggestion: the whole next query, caret at the end, ready for more. */
  function accept(s: Suggestion) {
    edit(s.kind === "recent" ? s.next : `${s.next} `);
  }

  /** Remove a chip: the words behind it when plain English made it, else its operator. */
  function removeChip(c: SearchChip) {
    if (askActive) {
      setAskMode("search");
      edit(removeRaw(searchText, c.raw));
      return;
    }
    const words = nl ? withoutPart(query, nl, c.raw) : null;
    edit(words ?? removeRaw(editable, c.raw));
  }

  function save() {
    if (!query.trim()) return;
    const saved = toggleSaved(query);
    setFlash(saved ? "Saved" : "Removed from saved");
    window.setTimeout(() => setFlash(null), 1400);
  }

  /** ⌘⇧S: pin this search to the sidebar as a view (features/smart); it's saved too. */
  function pin() {
    if (!query.trim()) return;
    if (!lists.saved.includes(query.trim())) toggleSaved(query);
    if (pinSearchToSidebar(query, false)) {
      setFlash("Pinned");
      window.setTimeout(() => setFlash(null), 1400);
    }
  }

  function insertTip(q: string) {
    setTipsOpen(false);
    edit(query.trim() ? `${query.trim()} ${q}` : q);
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key === "/") {
      e.preventDefault();
      setTipsOpen((o) => !o);
      return;
    }
    if (e.key === "Escape" && tipsOpen) {
      e.preventDefault();
      setTipsOpen(false);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      // Came back to this search from a thread: Esc goes on to the inbox.
      if (query && query !== restoredQuery.current) edit("");
      else close();
      return;
    }
    if (mod && e.shiftKey && e.key.toLowerCase() === "s") {
      e.preventDefault();
      pin();
      return;
    }
    if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      save();
      return;
    }
    if (mod && e.key === "ArrowUp") {
      e.preventDefault();
      if (lists.recent[0]) edit(lists.recent[0]);
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!items.length) return;
      const d = e.key === "ArrowDown" ? 1 : -1;
      select((selIdx + d + items.length) % items.length);
      return;
    }
    if (e.key === "Tab" && !e.shiftKey && hint) {
      e.preventDefault();
      applyHint();
      return;
    }
    if (e.key === "Tab" && !e.shiftKey && suggestions.length) {
      e.preventDefault();
      accept(suggestions[0]);
      return;
    }
    if (e.key === "Tab") {
      if (!items.length) return;
      e.preventDefault();
      select(nextGroup(items, selIdx, e.shiftKey ? -1 : 1));
      return;
    }
    if (mod && e.shiftKey && e.key === "Enter") {
      e.preventDefault();
      server.run();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const it = items[selIdx];
      if (it) run(it, mod);
      return;
    }
  }

  // --- render helpers --------------------------------------------------------
  const itemProps = (id: string): ItemProps => {
    const i = index.get(id) ?? -1;
    return {
      "data-sid": id,
      onMouseDown: (e: { preventDefault(): void }) => e.preventDefault(), // keep focus in the box
      onMouseMove: (e?: { clientX: number; clientY: number }) => {
        // Only a pointer that moved selects: rows sliding under a still one don't.
        if (e) {
          const p = pointer.current;
          pointer.current = { x: e.clientX, y: e.clientY };
          if (!p || (p.x === e.clientX && p.y === e.clientY)) return;
        }
        if (i >= 0 && i !== selIdx) select(i);
      },
      onClick: (e: { metaKey: boolean; ctrlKey: boolean }) => i >= 0 && run(items[i], e.metaKey || e.ctrlKey),
      onContextMenu: (e: ReactMouseEvent<HTMLElement>) => {
        if (i < 0) return;
        select(i);
        showTargetMenu(e, searchItemMenu(items[i], { open: (keep) => run(items[i], keep), edit, saved: lists.saved }));
      },
      active: i === selIdx,
      // New rows for the query on screen slide in (search.css).
      "data-enter": entered.has(id) ? "" : undefined,
    };
  };

  const serverGroup = (
    <ServerResultsGroup
      state={server.state}
      server={serverLabel}
      localKeys={localKeys}
      renderHit={(h) => (
        <ThreadRow key={`g:${h.accountId}/${h.threadId}`} hit={h} terms={terms} acc={accById[h.accountId]} {...itemProps(`g:${h.accountId}/${h.threadId}`)} />
      )}
    />
  );

  const saved = lists.saved.includes(query.trim());
  const pinned = isPinned(smartViews, query);
  const syncing = Object.values(sync).filter((s) => s.phase === "backfilling");
  const nAccounts = scopedAccounts.length;
  const smarter = resp?.semantic === "indexing" ? resp.semanticProgress ?? null : undefined;
  const [bestTop, ...moreTop] = shown.top;

  return (
    <>
      <div className="scrim" onMouseDown={close} />
      <div className="overlay-host sx-host" onMouseDown={(e) => e.target === e.currentTarget && close()}>
        <section className="search panel sx" role="dialog" aria-label="Search">
          {/* Query row */}
          <div className="q-row">
            <Icon name="search" className="q-ico" />
            <QueryInput
              value={query}
              onChange={setQuery}
              onKeyDown={onKey}
              inputRef={inputRef}
              accounts={scopedAccounts}
              labels={labels}
              placeholder={
                askActive
                  ? "Ask about your mail — when, who, how much…"
                  : profile
                    ? `Search ${profile.name}: people, topics, files — or ask a question`
                    : "Search mail: people, topics, files — or ask a question"
              }
            />
            <span className="sx-mode" role="group" aria-label="Search or ask">
              <button className={askActive ? "" : "on"} onMouseDown={(e) => e.preventDefault()} onClick={() => setAskMode("search")} title="Search">
                Search
              </button>
              <button className={askActive ? "on" : ""} onMouseDown={(e) => e.preventDefault()} onClick={() => setAskMode("ask")} title="Ask a question about your mail (or start with ?)">
                Ask
              </button>
            </span>
            <button className="btn btn-ghost btn-sm" onMouseDown={(e) => e.preventDefault()} onClick={save} disabled={empty} title="Save this search">
              <Icon name="pin" size="xs" />
              {flash ?? (saved ? "Saved" : "Save")}
              <span className="kbd">⌘S</span>
            </button>
            <button
              className="btn btn-ghost btn-sm btn-icon"
              onMouseDown={(e) => e.preventDefault()}
              onClick={pin}
              disabled={empty || pinned}
              title={pinned ? "In the sidebar" : "Pin to the sidebar as a view (⌘⇧S)"}
              aria-label="Pin to sidebar"
            >
              <Icon name="sidebar" size="xs" />
            </button>
            <button
              className="btn btn-ghost btn-sm"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                close();
                openRuleEditor({ condition: editable.trim(), profileId: profile?.id ?? null });
              }}
              disabled={empty}
              title="Create a rule that acts on mail matching this search"
            >
              <Icon name="wand" size="xs" />
              Rule
            </button>
            <button className="kbd sx-esc" onClick={close} title="Close">
              Esc
            </button>
          </div>

          {/* Suggestions as you type (Tab takes the first), or the date offer. The
              line is kept while there's a query, so results never jump as
              suggestions come and go. */}
          {!empty && !tipsOpen && !hint && (suggestions.length > 0 ? <SuggestStrip items={suggestions} onAccept={accept} /> : <div className="sx-suggest" aria-hidden="true" />)}
          {/* A date-like word: offer it as a date, in words. */}
          {hint && (
            <div className="sx-date-hint" role="status">
              <Icon name="calendar" size="2xs" />
              <span>Use “{hint.raw.trim()}” as a date?</span>
              <button className="sx-date-hint-go" onMouseDown={(e) => e.preventDefault()} onClick={applyHint}>
                <span>{hintLabel(hint.rewrite)}</span>
                <span className="kbd">⇥</span>
              </button>
              <button
                className="btn btn-ghost btn-sm btn-icon"
                title="Keep it as a word"
                aria-label="Keep it as a word"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => setIgnoredHints((s) => new Set(s).add(hint.raw.toLowerCase()))}
              >
                <Icon name="x" size="2xs" />
              </button>
            </div>
          )}
          <div className={"chips-row" + (askActive && !empty ? " is-ask" : "")}>
            {profile && (
              <span className="chip sx-scope" title={`Searching ${profile.accountIds.length === 1 ? "1 account" : `${profile.accountIds.length} accounts`} in this profile`}>
                <i className={`dot dot-sm t-${accountTone(profile.color)}`} />
                <b>Profile:</b> {profile.name}
                <button
                  className="x"
                  aria-label={`Search all accounts instead of ${profile.name}`}
                  title="Search all accounts"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => setUnscoped(true)}
                >
                  <Icon name="x" size="2xs" />
                </button>
              </span>
            )}
            {!empty && resp && askActive && resp.chips.length > 0 && <span className="parsed-label">Mail behind the answer</span>}
            {!empty && resp && (
              <div className="sx-chips">
                {resp.chips.map((c, i) => (
                  <ChipView key={`${c.raw}-${i}`} chip={c} onRemove={() => removeChip(c)} onEdit={() => selectRaw(c.raw)} />
                ))}
                {nlQuery && (
                  <button
                    className="btn btn-ghost btn-sm sx-literal"
                    title="Search for these words exactly as typed"
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => setLiteral((s) => new Set(s).add(query.trim()))}
                  >
                    Just the words
                  </button>
                )}
              </div>
            )}
            {empty && (
              <span className="parsed-label">
                {profile
                  ? `Searches every message in ${profile.name} on this Mac, instantly. Nothing is sent anywhere.`
                  : "Searches every message on this Mac, instantly. Nothing is sent anywhere."}
              </span>
            )}
          </div>

          <div className="s-body">
            <Facets
              query={editable}
              accounts={scopedAccounts}
              labels={labels}
              labelById={labelById}
              hits={hits}
              counts={counts}
              stale={!!counts && counts.query !== searchText}
              saved={lists.saved}
              empty={empty}
              edit={edit}
            />

            <div className={`results sx-results${search.stale ? " is-stale" : ""}`} ref={resultsRef}>
              {tipsOpen && <SearchTips onInsert={insertTip} onClose={() => setTipsOpen(false)} />}
              {/* The answer slot: a question gets its answer above the mail. */}
              {!tipsOpen && askActive && (
                <AskCard
                  state={ask}
                  onOpen={(c, keep) => openThread(c.accountId, c.threadId, c.messageId, keep)}
                  onAsk={(q) => {
                    setAskMode("ask");
                    edit(q);
                  }}
                  onSearch={(q) => {
                    setAskMode("search");
                    edit(q);
                  }}
                  activeItem={items[selIdx]?.group === "ask" ? selIdx - (index.get("k:0") ?? 0) : -1}
                  itemId={(i) => `k:${i}`}
                  onHoverItem={(i) => {
                    const j = index.get(`k:${i}`);
                    if (j !== undefined && j !== selIdx) select(j);
                  }}
                />
              )}
              {!tipsOpen && error && (
                <div className="sx-error">
                  <Icon name="info" size="sm" />
                  <span>Search failed: {error}</span>
                </div>
              )}
              {tipsOpen ? null : empty ? (
                <EmptyState recent={lists.recent} saved={lists.saved} itemProps={itemProps} />
              ) : noResults ? (
                <>
                  {server.state.status !== "idle" && serverGroup}
                  <NoResults
                    query={query}
                    correction={correction && shown.corrected.length ? correction.query : null}
                    corrected={shown.corrected}
                    renderHit={(h) => <ThreadRow key={`c:${h.accountId}/${h.threadId}`} hit={h} terms={freeTerms(correction?.query ?? "")} acc={accById[h.accountId]} {...itemProps(`c:${h.accountId}/${h.threadId}`)} />}
                    onCorrect={() => correction && edit(correction.query)}
                    fixes={fixes}
                    itemProps={itemProps}
                    syncing={syncing}
                    smarter={smarter}
                    accById={accById}
                    gmailFound={server.state.status === "done" ? server.state.resp.hits.length : null}
                    showQueries={!nlQuery && tokenize(query).some((t) => t.kind === "op")}
                  />
                  {suggestServer && server.state.status === "idle" && !shown.corrected.length && (
                    <div className="sx-note faint">
                      Nothing on this Mac. Older mail is {coverage?.olderMail === "none" ? "not downloaded" : "stored as headers only"}: try{" "}
                      <a onMouseDown={(e) => e.preventDefault()} onClick={server.run}>
                        searching Gmail
                      </a>{" "}
                      (⌘⇧↵).
                    </div>
                  )}
                </>
              ) : (
                <>
                  {correction && !anyWords && (
                    <button className="sx-dym" onMouseDown={(e) => e.preventDefault()} onClick={() => edit(correction.query)}>
                      Did you mean <b>{correction.query}</b>?
                      <span className="faint tnum">{correction.resp.hits.length === 1 ? "1 thread" : `${correction.resp.hits.length} threads`}</span>
                    </button>
                  )}
                  {bestTop && (
                    <>
                      <div className="group-head">
                        <span>{anyWords ? "Top results" : "Closest matches"}</span>
                        <span className="gh-hint">
                          {anyWords ? (
                            <>
                              <span className="kbd">↵</span> Open
                            </>
                          ) : (
                            "No mail has all of these words"
                          )}
                        </span>
                      </div>
                      <BestMatch
                        hit={bestTop}
                        terms={terms}
                        acc={accById[bestTop.accountId]}
                        files={(resp?.attachments ?? []).filter((a) => a.threadId === bestTop.threadId && a.accountId === bestTop.accountId).slice(0, 2)}
                        {...itemProps(`h:${bestTop.accountId}/${bestTop.threadId}`)}
                      />
                      {moreTop.length > 0 && (
                        <div className="hits sx-top-more">
                          {moreTop.map((h) => (
                            <TopRow key={`${h.accountId}/${h.threadId}`} hit={h} terms={terms} acc={accById[h.accountId]} {...itemProps(`h:${h.accountId}/${h.threadId}`)} />
                          ))}
                        </div>
                      )}
                    </>
                  )}
                  {shown.events.length > 0 && (
                    <>
                      <div className="group-head">
                        <span>
                          Calendar <span className="gh-count">{shown.events.length}</span>
                        </span>
                        <span className="gh-hint">Events on your calendars</span>
                      </div>
                      <EventResults events={shown.events} accounts={accById} itemProps={itemProps} />
                    </>
                  )}
                  {shown.atts.length > 0 && (
                    <>
                      <div className="group-head">
                        <span>
                          Attachments{" "}
                          <span className="gh-count">{(resp?.attachments.length ?? 0) > shown.atts.length ? `${shown.atts.length}+` : shown.atts.length}</span>
                        </span>
                        <span className="gh-hint">By file name</span>
                      </div>
                      <div className="att-row">
                        {shown.atts.map((a) => (
                          <AttachmentCard key={`${a.messageId}/${a.attachment.id}`} a={a} terms={terms} {...itemProps(`a:${a.messageId}/${a.attachment.id}`)} />
                        ))}
                      </div>
                    </>
                  )}
                  {shown.threads.length > 0 && (
                    <>
                      <div className="group-head">
                        <span>
                          Threads <span className="gh-count">{shown.threads.length}</span>
                        </span>
                        <span className="gh-hint">Newest first</span>
                      </div>
                      <div className="hits">
                        {shown.threads.map((h) => (
                          <ThreadRow key={`${h.accountId}/${h.threadId}`} hit={h} terms={terms} acc={accById[h.accountId]} {...itemProps(`h:${h.accountId}/${h.threadId}`)} />
                        ))}
                        <OutsideDate query={editable} counts={counts} shown={hits.length} edit={edit} />
                      </div>
                    </>
                  )}
                  {shown.threads.length === 0 && bestTop && <OutsideDate query={editable} counts={counts} shown={hits.length} edit={edit} />}
                  {shown.related.length > 0 && (
                    <>
                      <div className="group-head">
                        <span>
                          Related <span className="gh-count">{shown.related.length}</span>
                        </span>
                        <span className="gh-hint">Closest first</span>
                      </div>
                      <div className="hits">
                        {shown.related.map((h) => (
                          <ThreadRow key={`${h.accountId}/${h.threadId}`} hit={h} terms={terms} acc={accById[h.accountId]} {...itemProps(`h:${h.accountId}/${h.threadId}`)} />
                        ))}
                      </div>
                    </>
                  )}
                  {serverGroup}
                </>
              )}
            </div>

            <aside className="s-side">
              {!empty && people.length > 0 && (
                <>
                  <div className="group-head" style={{ marginTop: 0 }}>
                    <span>
                      People <span className="gh-count">{people.length}</span>
                    </span>
                    <span className="gh-hint">Show their mail</span>
                  </div>
                  <div className="people">
                    {people.map((p) => {
                      const ip = itemProps(`p:${p.address.email}`);
                      const chosen = opValues(editable, "from").includes(p.address.email.toLowerCase());
                      return (
                        <a key={p.address.email} className={`person${ip.active ? " on" : ""}`} {...omitActive(ip)}>
                          <Avatar person={p.address} />
                          <div className="grow" style={{ minWidth: 0 }}>
                            <div className="p-name">{displayName(p.address)}</div>
                            <div className="faint truncate" style={{ fontSize: 12 }}>
                              {p.address.email} · {p.messageCount} {p.messageCount === 1 ? "message" : "messages"}
                            </div>
                          </div>
                          {chosen && <Icon name="check" size="xs" className="accent-ico" />}
                        </a>
                      );
                    })}
                  </div>
                </>
              )}

              {empty ? (
                <FrequentPeople people={knownPeople} mine={mine} onPick={(a) => edit(`from:${a.email.toLowerCase()} `)} />
              ) : (
                lists.recent.length > 0 && (
                  <>
                    <RecentsHead style={{ marginTop: people.length ? 18 : 0 }}>
                      <span className="kbd">⌘↑</span>
                    </RecentsHead>
                    <div className="recents">
                      {lists.recent.slice(0, 5).map((q) => (
                        <a key={q} className="recent" onMouseDown={(e) => e.preventDefault()} onClick={() => edit(q)}>
                          <Icon name="history" size="xs" />
                          <HighlightedQuery q={q} />
                        </a>
                      ))}
                    </div>
                  </>
                )
              )}
            </aside>
          </div>

          <footer className="s-foot">
            <span className="hint">
              <span className="kbd-group">
                <span className="kbd">↑</span>
                <span className="kbd">↓</span>
              </span>
              Navigate
            </span>
            <span className="hint">
              <span className="kbd">↵</span>Open at match
            </span>
            <span className="hint">
              <span className="kbd">⌘↵</span>Open, keep search
            </span>
            {suggestions.length > 0 ? (
              <span className="hint">
                <span className="kbd">Tab</span>Take suggestion
              </span>
            ) : (
              <span className="hint">
                <span className="kbd">Tab</span>Next group
              </span>
            )}
            <span className="hint">
              <span className="kbd">Esc</span>
              {query ? "Clear" : "Close"}
            </span>
            <span className="grow" />
            {/* How fast and over how much: a quiet footer note, not a banner over the results. */}
            {resp && !empty && (
              <span
                className="sx-stats"
                title={
                  `Searched ${num(resp.indexedMessages)} messages indexed on this Mac in ${Math.round(resp.tookMs)} ms` +
                  (syncing.length ? ". Some accounts are still downloading older mail." : "")
                }
              >
                <span className="sx-stats-ms">{Math.round(resp.tookMs)} ms</span> · {num(resp.indexedMessages)} local
                {syncing.length > 0 && <span className="sx-syncing"> · still syncing</span>}
              </span>
            )}
            {smarter !== undefined && <IndexingLine progress={smarter} />}
            <span className="faint">
              {profile
                ? `${profile.name} · ${nAccounts === 1 ? "1 account" : `${nAccounts} accounts`}`
                : nAccounts === 1
                  ? "1 account"
                  : `All ${nAccounts || ""} accounts`.replace("  ", " ")}{" "}
              {server.state.status === "done" || server.state.status === "loading" ? `· local + ${serverLabel} search` : "· 100% local"}
              {coverage ? ` · ${coverageText(coverage)}` : ""}
            </span>
            <ServerSearchButton state={server.state} onRun={server.run} suggest={suggestServer} server={serverLabel} />
          </footer>
        </section>
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

/** Props that make an element a keyboard/mouse-selectable result. */
interface ItemProps {
  "data-sid": string;
  "data-enter"?: string;
  onMouseDown: (e: { preventDefault(): void }) => void;
  onMouseMove: (e?: { clientX: number; clientY: number }) => void;
  onClick: (e: { metaKey: boolean; ctrlKey: boolean }) => void;
  onContextMenu: (e: ReactMouseEvent<HTMLElement>) => void;
  active: boolean;
}

function omitActive(p: ItemProps) {
  const { active: _a, ...rest } = p;
  return rest;
}

// Chip labels come from penguin-core already worded for display ("From Mike
// Delgado", "Has PDF", "Last spring · Mar 1 – May 31, 2026"); the UI only
// adds an icon and dims the leading operator word, as in the mockup.
const CHIP_ICON: Record<string, IconName> = {
  has: "file",
  filename: "clip",
  date: "calendar",
  before: "calendar",
  after: "calendar",
  subject: "type",
  label: "tag",
  in: "inbox",
  is: "mail",
  account: "at",
  exclude: "minus",
  error: "info",
};
const PERSON_KINDS = new Set(["from", "to", "cc", "bcc"]);

function splitLabel(chip: SearchChip): [string, string] {
  const { kind, label } = chip;
  if (kind === "date") {
    const i = label.indexOf(" · ");
    return i === -1 ? ["", label] : [label.slice(0, i), label.slice(i + 3)];
  }
  if (kind === "text" || kind === "is" || kind === "in" || kind === "error") return ["", label];
  const m = /^(Older than|Newer than|Not \S+|\S+) (.+)$/.exec(label);
  return m ? [m[1], m[2]] : ["", label];
}

function ChipView({ chip, onRemove, onEdit }: { chip: SearchChip; onRemove: () => void; onEdit: () => void }) {
  const [lead, rest] = splitLabel(chip);
  const icon = CHIP_ICON[chip.kind];
  const person = PERSON_KINDS.has(chip.kind);
  // A date:/other value penguin-core couldn't read: say so gently, and a click
  // selects it in the box to fix (it constrains nothing meanwhile).
  if (chip.kind === "error") {
    return (
      <span className="chip chip-error" title={`${chip.raw} · click to edit`} onMouseDown={(e) => e.preventDefault()} onClick={onEdit} role="button">
        <Icon name="info" size="2xs" />
        {chip.label}
        <span className="mono chip-error-raw">{chip.raw}</span>
        <button
          className="x"
          aria-label={`Remove ${chip.raw}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
        >
          <Icon name="x" size="2xs" />
        </button>
      </span>
    );
  }
  return (
    <span className="chip" title={chip.raw}>
      {person && <span className={`avatar avatar-xs t-${personTone(rest)}`}>{initials({ name: rest, email: rest })}</span>}
      {icon && <Icon name={icon} size="2xs" />}
      {lead && <b>{lead}</b>} {rest}
      <button
        className="x"
        aria-label={`Remove ${chip.label}`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={onRemove}
      >
        <Icon name="x" size="2xs" />
      </button>
    </span>
  );
}

function BestMatch({ hit, terms, acc, files, ...ip }: { hit: SearchHit; terms: string[]; acc?: Account; files: AttachmentHit[] } & ItemProps) {
  const { active, ...rest } = ip;
  return (
    <article className={`tophit sx-row${active ? " is-active" : ""}`} {...rest}>
      <div className="th-top">
        <AccDot acc={acc} />
        <Avatar person={hit.from} size="sm" />
        <span className="th-from">{displayName(hit.from)}</span>
        <span className="faint truncate">{hit.from.email}</span>
        <span className="grow" />
        {hit.matchCount > 1 && <span className="badge t-gray">{hit.matchCount} matches in thread</span>}
        <span className="faint tnum" style={{ fontSize: 12 }}>
          {new Date(hit.date).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })}
        </span>
      </div>
      <h3 className="th-subj">
        <Highlight text={hit.subject || "(no subject)"} terms={terms} />
      </h3>
      <p className="th-snip">
        <Snippet hit={hit} terms={terms} quoted />
      </p>
      {files.length > 0 && (
        <div className="th-files">
          {files.map((f) => (
            <span key={f.attachment.id} className={`att-chip ${fileTone(f.attachment.filename, f.attachment.mimeType)}`}>
              <span className="mini-ico">{fileExt(f.attachment.filename)}</span>
              <span className="truncate">
                <Highlight text={f.attachment.filename} terms={terms} />
              </span>
            </span>
          ))}
        </div>
      )}
    </article>
  );
}

function AttachmentCard({ a, terms, ...ip }: { a: AttachmentHit; terms: string[] } & ItemProps) {
  const { active, ...rest } = ip;
  const f = a.attachment;
  return (
    <div className={`file sx-row ${fileTone(f.filename, f.mimeType)}${active ? " is-active" : ""}`} title={`${f.filename} — from ${displayName(a.from)}`} {...rest}>
      <span className="file-ico">{fileExt(f.filename)}</span>
      <div className="grow">
        <div className="fname truncate">
          <Highlight text={f.filename} terms={terms} />
        </div>
        <div className="fmeta">
          {shortDate(a.date)} · {bytes(f.size)} · {displayName(a.from)}
        </div>
      </div>
    </div>
  );
}

function ThreadRow({ hit, terms, acc, ...ip }: { hit: SearchHit; terms: string[]; acc?: Account } & ItemProps) {
  const { active, ...rest } = ip;
  return (
    <div className={`hit sx-row${active ? " is-active" : ""}${hit.unread ? " sx-unread" : ""}`} {...rest}>
      <span className="hit-from">
        <AccDot acc={acc} />
        <Avatar person={hit.from} size="xs" />
        <span className="truncate">{displayName(hit.from)}</span>
      </span>
      <span className="hit-main truncate">
        <span className="hit-subj">
          <Highlight text={hit.subject || "(no subject)"} terms={terms} />
        </span>
        <span className="hit-snip">
          {" — "}
          <Snippet hit={hit} terms={terms} quoted />
        </span>
      </span>
      <span className="hit-meta">
        {hit.matchCount > 1 && (
          <span className="badge t-gray" title={`${hit.matchCount} matching messages in this thread`}>
            {hit.matchCount} matches
          </span>
        )}
        {hit.hasAttachments && <Icon name="clip" size="xs" />}
        <span className="tnum">{shortDate(hit.date)}</span>
      </span>
    </div>
  );
}

/** Top results after the first: sender and date, subject, and the passage that matched. */
function TopRow({ hit, terms, acc, ...ip }: { hit: SearchHit; terms: string[]; acc?: Account } & ItemProps) {
  const { active, ...rest } = ip;
  return (
    <div className={`sx-toprow sx-row${active ? " is-active" : ""}${hit.unread ? " sx-unread" : ""}`} {...rest}>
      <Avatar person={hit.from} size="sm" />
      <div className="sx-toprow-main">
        <div className="sx-toprow-line">
          <span className="sx-toprow-subj truncate">
            <Highlight text={hit.subject || "(no subject)"} terms={terms} />
          </span>
          <span className="grow" />
          {hit.matchCount > 1 && <span className="badge t-gray">{hit.matchCount} matches</span>}
          {hit.hasAttachments && <Icon name="clip" size="xs" className="faint" />}
          <span className="faint tnum sx-toprow-date">{shortDate(hit.date)}</span>
        </div>
        <div className="sx-toprow-snip truncate">
          <AccDot acc={acc} />
          <span className="sx-toprow-from">{displayName(hit.from)}</span>
          <span className="faint"> · </span>
          <Snippet hit={hit} terms={terms} />
        </div>
      </div>
    </div>
  );
}

/**
 * The snippet: the words' own excerpt (marked by the index) when the words
 * matched there, else the passage that matched in meaning, with any query
 * words in it marked. No label says which: the passage explains itself.
 */
function Snippet({ hit, terms, quoted }: { hit: SearchHit; terms: string[]; quoted?: boolean }) {
  const words = byWords(hit) && /<mark>/i.test(hit.snippetHtml);
  const [o, c] = quoted ? ["“", "”"] : ["", ""];
  if (!words && hit.passage) {
    return (
      <span className="sx-passage">
        {o}
        <Highlight text={hit.passage} terms={terms} />
        {c}
      </span>
    );
  }
  return <span dangerouslySetInnerHTML={{ __html: `${o}${sanitizeSnippet(hit.snippetHtml)}${c}` }} />;
}

function OutsideDate({ query, counts, shown, edit }: { query: string; counts: ReturnType<typeof useFacetCounts>; shown: number; edit: (q: string) => void }) {
  const cur = currentDate(query);
  if (!cur || !counts || counts.query !== query) return null;
  const any = counts.byDate.any;
  if (any === undefined || any <= shown) return null;
  return (
    <button className="hit-more sx-more" onMouseDown={(e) => e.preventDefault()} onClick={() => edit(setDate(query, null))}>
      <Icon name="history" size="xs" />
      Older matches outside “{cur}”: {any - shown}
      <span className="grow" />
      <span className="faint">Search any time</span>
    </button>
  );
}

function AccDot({ acc }: { acc?: Account }) {
  return <i className={`dot dot-sm t-${acc ? accountTone(acc.color) : "gray"}`} title={acc?.email} />;
}

function Facets({
  query,
  accounts,
  labels,
  labelById,
  hits,
  counts,
  stale,
  saved,
  empty,
  edit,
}: {
  query: string;
  accounts: Account[];
  labels: Label[];
  labelById: Map<string, Label>;
  hits: SearchHit[];
  counts: ReturnType<typeof useFacetCounts>;
  stale: boolean;
  saved: string[];
  empty: boolean;
  edit: (q: string) => void;
}) {
  const accSel = opValues(query, "account");
  const hasSel = opValues(query, "has");
  const labelSel = opValues(query, "label");
  const date = currentDate(query);
  const dates = [...DATE_PRESETS];
  if (date && !dates.some(([p]) => p === date)) dates.splice(1, 0, [date, date.charAt(0).toUpperCase() + date.slice(1)]);
  const n = (v: number | undefined) => (empty || v === undefined ? null : <span className={`n${stale ? " stale" : ""}`}>{v}</span>);

  // Labels present in the current results, most common first.
  const labelCounts = new Map<string, { label: Label; n: number }>();
  for (const h of hits) {
    for (const id of h.labelIds) {
      const l = labelById.get(`${h.accountId}/${id}`);
      if (!l || l.kind !== "user") continue;
      const k = l.name.toLowerCase();
      const cur = labelCounts.get(k);
      labelCounts.set(k, { label: l, n: (cur?.n ?? 0) + 1 });
    }
  }
  const labelRows = [...labelCounts.values()].sort((a, b) => b.n - a.n).slice(0, 6);
  // Keep selected labels visible even when they matched nothing.
  for (const v of labelSel) {
    if (!labelRows.some((r) => r.label.name.toLowerCase() === v)) {
      const l = labels.find((x) => x.name.toLowerCase() === v);
      if (l) labelRows.push({ label: l, n: 0 });
    }
  }
  const allAcc = accSel.length === 0;
  const total = counts?.byAccount ? Object.values(counts.byAccount).reduce((a, b) => a + b, 0) : undefined;

  const pd = (e: { preventDefault(): void }) => e.preventDefault();
  return (
    <aside className="facets sx-facets">
      <div className="facet">
        <div className="facet-title">Account</div>
        <a className={`facet-item${allAcc ? " on" : ""}`} onMouseDown={pd} onClick={() => edit(setOp(query, "account", null))}>
          <Check on={allAcc} />
          All accounts{n(total)}
        </a>
        {accounts.map((a) => {
          const on = accSel.includes(a.email.toLowerCase());
          const c = counts?.byAccount?.[a.id] ?? (counts?.byAccount ? 0 : undefined);
          return (
            <a
              key={a.id}
              className={`facet-item${on ? " on" : ""}${!empty && c === 0 ? " dim" : ""}`}
              onMouseDown={pd}
              onClick={() => edit(on ? setOp(query, "account", null) : setOp(query, "account", a.email))}
              title={a.email}
            >
              <Check on={on} />
              <i className={`dot dot-sm t-${accountTone(a.color)}`} />
              <span className="truncate">{accountName(a, accounts)}</span>
              {n(c)}
            </a>
          );
        })}
      </div>

      <div className="facet">
        <div className="facet-title">Date</div>
        {dates.map(([p, label]) => {
          const on = (date ?? null) === p && !(p === null && hasBeforeAfter(query));
          return (
            <a key={label} className={`facet-item${on ? " on" : ""}`} onMouseDown={pd} onClick={() => edit(setDate(query, p))}>
              <span className={`radio${on ? " on" : ""}`} />
              {label}
              {n(counts?.byDate[p ?? "any"])}
            </a>
          );
        })}
        <a
          className={`facet-item${hasBeforeAfter(query) && !date ? " on" : ""}`}
          onMouseDown={pd}
          onClick={() => edit(`${setDate(query, null)} after:`.trim())}
          title="Type a date after after: (YYYY-MM-DD)"
        >
          <span className={`radio${hasBeforeAfter(query) && !date ? " on" : ""}`} />
          Custom range…
        </a>
      </div>

      <div className="facet">
        <div className="facet-title">Has</div>
        {HAS_OPTIONS.map(([v, label]) => {
          const on = hasSel.includes(v);
          const c = counts?.byHas[v];
          return (
            <a key={v} className={`facet-item${on ? " on" : ""}${!empty && c === 0 && !on ? " dim" : ""}`} onMouseDown={pd} onClick={() => edit(toggleOp(query, "has", v))}>
              <Check on={on} />
              {label}
              {n(c)}
            </a>
          );
        })}
      </div>

      {labelRows.length > 0 && (
        <div className="facet">
          <div className="facet-title">Label</div>
          {labelRows.map(({ label: l, n: c }) => {
            const on = labelSel.includes(l.name.toLowerCase());
            return (
              <a key={l.name} className={`facet-item${on ? " on" : ""}`} onMouseDown={pd} onClick={() => edit(toggleOp(query, "label", l.name))}>
                <Check on={on} />
                <span className={`label-sq t-${toneForColor(l.color)}`} style={{ margin: "0 2px 0 0" }} />
                <span className="truncate">{l.name}</span>
                {n(c)}
              </a>
            );
          })}
        </div>
      )}

      {saved.length > 0 && (
        <div className="facet">
          <div className="facet-title">Saved</div>
          {saved.slice(0, 6).map((q) => (
            <a key={q} className={`facet-item${q === query.trim() ? " on" : ""}`} onMouseDown={pd} onClick={() => edit(q)} title={q}>
              <Icon name="pin" size="2xs" className="faint" />
              <span className="mono q-mini">{q}</span>
            </a>
          ))}
        </div>
      )}
    </aside>
  );
}

function hasBeforeAfter(q: string) {
  return tokenize(q).some((t) => t.kind === "op" && isDateOp(t.op));
}

function Check({ on }: { on: boolean }) {
  return <span className="check">{on && <Icon name="check" size="2xs" />}</span>;
}

function EmptyState({ recent, saved, itemProps }: { recent: string[]; saved: string[]; itemProps: (id: string) => ItemProps }) {
  if (!recent.length && !saved.length) {
    return (
      <div className="sx-empty">
        <div className="sx-empty-title">Find anything you've already seen</div>
        <p className="sx-empty-copy">
          Type a few words you remember — a topic, a name, roughly when. Results update as you type, straight from the index on
          this Mac.
        </p>
      </div>
    );
  }
  const row = (q: string, kind: "recent" | "saved") => {
    const ip = itemProps(`${kind === "recent" ? "r" : "s"}:${q}`);
    const { active, ...rest } = ip;
    return (
      <div key={`${kind}:${q}`} className={`hit sx-row sx-qrow${active ? " is-active" : ""}`} {...rest}>
        <Icon name={kind === "recent" ? "history" : "pin"} size="xs" className="faint" />
        <HighlightedQuery q={q} />
        {kind === "recent" ? (
          <button
            className="btn btn-ghost btn-sm btn-icon sx-forget"
            title="Remove from recent"
            onMouseDown={(e) => e.preventDefault()}
            onClick={(e) => {
              e.stopPropagation();
              removeRecent(q);
            }}
          >
            <Icon name="x" size="2xs" />
          </button>
        ) : (
          <span />
        )}
      </div>
    );
  };
  return (
    <>
      {recent.length > 0 && (
        <>
          <RecentsHead>
            <span className="kbd">↵</span> Search again
          </RecentsHead>
          <div className="hits">{recent.map((q) => row(q, "recent"))}</div>
        </>
      )}
      {saved.length > 0 && (
        <>
          <div className="group-head">
            <span>Saved searches</span>
            <span className="gh-hint">
              <span className="kbd">⌘S</span> Save the current search
            </span>
          </div>
          <div className="hits">{saved.map((q) => row(q, "saved"))}</div>
        </>
      )}
    </>
  );
}

/** "Recent searches" heading: its key hint, then Clear (questions asked in Ask are in the same list). */
function RecentsHead({ style, children }: { style?: CSSProperties; children: ReactNode }) {
  return (
    <div className="group-head" style={style}>
      <span>Recent searches</span>
      <span className="gh-end">
        <span className="gh-hint">{children}</span>
        <button
          className="gh-clear"
          title="Forget recent searches and questions on this Mac"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => clearRecents()}
        >
          Clear
        </button>
      </span>
    </div>
  );
}

/** Before typing: the people you hear from most, one click from their mail. */
function FrequentPeople({ people, mine, onPick }: { people: Array<{ address: { name: string | null; email: string }; seen: number }>; mine: string[]; onPick: (a: { name: string | null; email: string }) => void }) {
  const list = people.filter((p) => !mine.includes(p.address.email.toLowerCase())).slice(0, 7);
  if (!list.length) return null;
  return (
    <>
      <div className="group-head" style={{ marginTop: 0 }}>
        <span>People</span>
        <span className="gh-hint">Their mail</span>
      </div>
      <div className="people">
        {list.map((p) => (
          <a key={p.address.email} className="person" onMouseDown={(e) => e.preventDefault()} onClick={() => onPick(p.address)} title={`Mail from ${displayName(p.address)}`}>
            <Avatar person={p.address} />
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="p-name">{displayName(p.address)}</div>
              <div className="faint truncate" style={{ fontSize: 12 }}>
                {p.address.email}
              </div>
            </div>
          </a>
        ))}
      </div>
    </>
  );
}

/** Suggestions under the box as you type; the first is taken with Tab. */
function SuggestStrip({ items, onAccept }: { items: Suggestion[]; onAccept: (s: Suggestion) => void }) {
  return (
    <div className="sx-suggest" role="listbox" aria-label="Suggestions">
      {items.map((s, i) => (
        <button
          key={s.key}
          role="option"
          aria-selected={i === 0}
          className={`sx-sug sx-sug-${s.kind}${i === 0 ? " on" : ""}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onAccept(s)}
          title={s.kind === "person" ? `Mail from ${s.label} (${s.detail})` : s.kind === "domain" ? `Mail with anyone at ${s.label}` : s.label}
        >
          {s.kind === "person" && s.address && <Avatar person={s.address} size="xs" />}
          {s.kind === "domain" && <span className="sx-sug-tile">{s.label.charAt(0).toUpperCase()}</span>}
          {s.kind === "recent" && <Icon name="history" size="2xs" />}
          {s.kind === "refine" && (s.address ? <Avatar person={s.address} size="xs" /> : <Icon name="plus" size="2xs" />)}
          {s.kind === "domain" && <span className="faint">{s.detail}</span>}
          <span className="sx-sug-label">{s.label}</span>
          {s.kind === "person" && <span className="faint sx-sug-detail">{s.detail}</span>}
          {s.kind === "refine" && s.detail && <span className="faint tnum">{s.detail}</span>}
          {i === 0 && <span className="kbd">⇥</span>}
        </button>
      ))}
    </div>
  );
}

/** One quiet line while search by meaning is still reading the mailbox. */
function IndexingLine({ progress }: { progress: number | null }) {
  const pct = progress === null ? null : Math.max(0, Math.min(99, Math.floor(progress * 100)));
  return (
    <span
      className="sx-smarter"
      role="status"
      title="Penguin is still reading your mail on this Mac so it can find what you mean, not only the words you type. Search works meanwhile."
    >
      <svg className="sx-smarter-ring" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="6" />
        <circle cx="8" cy="8" r="6" pathLength="100" strokeDasharray={`${pct ?? 30} 100`} />
      </svg>
      {pct === null ? "Getting smarter: still indexing your mail" : `Getting smarter: ${pct}% of your mail indexed`}
    </span>
  );
}

/** "date:february" → "Feb 1 – 28, 2026": the date hint says what it means, not the operator. */
function hintLabel(rewrite: string): string {
  const v = rewrite.replace(/^date:/, "").replace(/^"|"$/g, "");
  const r = parseDatePhrase(v);
  return r ? dateChipLabel(v, r) : v;
}

function NoResults({
  query,
  correction,
  corrected,
  renderHit,
  onCorrect,
  fixes,
  itemProps,
  syncing,
  smarter,
  accById,
  gmailFound,
  showQueries,
}: {
  /** Show each wider search's query (only for someone who typed operators). */
  showQueries: boolean;
  query: string;
  /** A corrected spelling that finds mail (its threads are `corrected`, shown here). */
  correction: string | null;
  corrected: SearchHit[];
  renderHit: (h: SearchHit) => ReactNode;
  onCorrect: () => void;
  /** Threads "Also search Gmail" found (shown above), or null if it hasn't run. */
  gmailFound: number | null;
  fixes: Fix[] | null;
  itemProps: (id: string) => ItemProps;
  syncing: SyncStatus[];
  /** Search by meaning still indexing: its progress (null = unknown); undefined = not indexing. */
  smarter: number | null | undefined;
  accById: Record<string, Account>;
}) {
  return (
    <div className="sx-none">
      {correction ? (
        <>
          <div className="sx-empty-title">
            No mail has “{query}”. Showing results for{" "}
            <button className="sx-dym-go" onMouseDown={(e) => e.preventDefault()} onClick={onCorrect} title="Search for this instead">
              {correction}
            </button>
          </div>
          <div className="hits sx-corrected">{corrected.map(renderHit)}</div>
          {fixes && fixes.length > 0 && <p className="sx-empty-copy">Or widen the search:</p>}
        </>
      ) : (
        <>
          <div className="sx-empty-title">No mail matches “{query}” on this Mac</div>
          <p className="sx-empty-copy">
            {gmailFound
              ? `Gmail found ${gmailFound} older ${gmailFound === 1 ? "thread" : "threads"} above; they're now saved on this Mac.${fixes?.length ? " These wider searches also find mail:" : ""}`
              : fixes === null
                ? "That doesn't mean it isn't there. Checking wider searches…"
                : fixes.length
                  ? "That doesn't mean it isn't there. These wider searches do find mail:"
                  : "Wider searches (any time, all accounts, Trash and Spam) found nothing either. Try other words: a name, or what it was about."}
          </p>
        </>
      )}
      {smarter !== undefined && (
        <p className="sx-empty-copy faint">
          Penguin is still reading your mail so it can find what you mean, not only your words
          {smarter === null ? "." : ` (${Math.floor(smarter * 100)}% done).`}
        </p>
      )}
      {syncing.length > 0 && (
        <div className="callout t-amber sx-sync-note">
          <Icon name="refresh" size="sm" />
          <div>
            Still downloading older mail:{" "}
            {syncing
              .map((s) => `${accById[s.accountId]?.email ?? "an account"} (${num(s.indexed)}${s.totalEstimate ? ` of ~${num(s.totalEstimate)}` : ""})`)
              .join(", ")}
            . Messages that haven't synced yet can't match.
          </div>
        </div>
      )}
      <div className="hits sx-fixes">
        {(fixes ?? []).map((f) => {
          const ip = itemProps(`f:${f.unscope ? "*" : f.query}`);
          const { active, ...rest } = ip;
          return (
            <div key={f.unscope ? "*" : f.query} className={`hit sx-row sx-qrow${active ? " is-active" : ""}`} {...rest}>
              <Icon name={f.unscope ? "at" : "search"} size="xs" className="faint" />
              <span className="sx-fix">
                {f.label}
                {!f.unscope && showQueries && (
                  <>
                    <span className="faint"> — </span>
                    <HighlightedQuery q={f.query} />
                  </>
                )}
              </span>
              <span className="faint tnum sx-fix-n">{f.n}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

interface Fix {
  label: string;
  query: string;
  n: number;
  /** Search every account instead of the profile (same query). */
  unscope?: boolean;
}

type Candidate = Omit<Fix, "n">;

/** Run each broadened query and keep the ones that actually find mail. */
function useVerifiedFixes(candidates: Candidate[], accountIds: string[] | null): Fix[] | null {
  const [out, setOut] = useState<{ key: string; fixes: Fix[] } | null>(null);
  const key = candidates.map((c) => `${c.unscope ? "*" : ""}${c.query}`).join("\u0000") + "\u0001" + (accountIds?.join("\u0000") ?? "");
  useEffect(() => {
    if (!candidates.length) return;
    let alive = true;
    Promise.all(
      candidates.map((c) =>
        api
          .search({ query: c.query, accountId: null, accountIds: c.unscope ? null : accountIds, limit: 100 })
          .then((r) => ({ ...c, n: r.hits.length }))
          .catch(() => ({ ...c, n: 0 })),
      ),
    ).then((list) => {
      if (alive) setOut({ key, fixes: list.filter((f) => f.n > 0).sort((a, b) => b.n - a.n) });
    });
    return () => {
      alive = false;
    };
    // `key` is the candidate list's identity; the array itself is rebuilt each render.
  }, [key]);
  if (!candidates.length) return [];
  return out && out.key === key ? out.fixes : null;
}

/**
 * "Did you mean": the query with each word no mail contains replaced by its
 * nearest known word (spelling.ts), run once; kept only when it finds mail
 * by its words (NN/g: a suggestion must never lead to nothing).
 */
function useCorrection(query: string | null, accountIds: string[] | null): { query: string; resp: SearchResponse } | null {
  const [out, setOut] = useState<{ typed: string; query: string; resp: SearchResponse } | null>(null);
  const scope = accountIds?.join("\u0000") ?? "";
  useEffect(() => {
    if (!query) return;
    let alive = true;
    // Secondary work (a lexicon scan and a second search), so it waits for a
    // real pause in typing (longer than the gap between keys), so the
    // keystroke's own search and paint always go first.
    const t = window.setTimeout(() => {
      const fix = didYouMean(query);
      if (!fix) return;
      api
        .search({ query: fix, accountId: null, accountIds, limit: THREADS_SHOWN })
        .then((resp) => alive && setOut(resp.hits.some(byWords) ? { typed: query, query: fix, resp } : null))
        .catch(() => alive && setOut(null));
    }, 300);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
    // `scope` stands for accountIds (a new array each render).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, scope]);
  return query && out?.typed === query ? out : null;
}

/** Ways to broaden a query that found nothing (`scoped`: a profile limits the accounts). */
function broaden(query: string, chips: SearchChip[], scoped: boolean): Candidate[] {
  const out: Candidate[] = [];
  const push = (label: string, q: string) => {
    const t = q.trim();
    if (t && t !== query.trim() && !out.some((o) => !o.unscope && o.query === t)) out.push({ label, query: t });
  };
  for (const c of chips) {
    if (c.kind === "date" || c.kind === "before" || c.kind === "after") push("Any time", removeRaw(query, c.raw));
  }
  if (scoped) out.push({ label: "Search all accounts", query: query.trim(), unscope: true });
  if (opValues(query, "account").length) push("All accounts", setOp(query, "account", null));
  if (!opValues(query, "in").includes("anywhere")) push("Include Trash and Spam", `${setOp(query, "in", null)} in:anywhere`);
  for (const c of chips) {
    if (c.kind !== "date" && c.kind !== "before" && c.kind !== "after") push(`Without “${c.label}”`, removeRaw(query, c.raw));
  }
  // Drop the last free word (often a typo or a word the mail never used).
  const toks = tokenize(query).filter((t) => t.kind !== "space");
  const words = toks.filter((t) => t.kind === "word" && !t.negated);
  if (words.length && toks.length > 1) {
    const w = words[words.length - 1];
    push(`Without “${w.text}”`, query.slice(0, w.start) + query.slice(w.end));
  }
  // Last resort: only the words, or only the people, with no other filters.
  const free = words.map((w) => w.text).join(" ");
  if (free) push(`Only the words “${free}”`, free);
  const whoChips = chips.filter((c) => c.kind === "from" || c.kind === "to");
  if (whoChips.length) push(`Only ${whoChips.map((c) => `“${c.label}”`).join(" and ")}, any time`, whoChips.map((c) => c.raw).join(" "));
  return out.slice(0, 8);
}

/** Free-text terms in the query (for client-side subject/filename highlighting). */
function freeTerms(q: string): string[] {
  const out: string[] = [];
  for (const t of tokenize(q)) {
    if ((t.kind === "word" || t.kind === "phrase") && !t.negated) {
      const v = t.text.replace(/"/g, "").trim().toLowerCase();
      if (v.length >= 2) out.push(v);
    } else if (t.kind === "op" && (t.op === "subject" || t.op === "filename") && t.value.length >= 2) {
      out.push(t.value.toLowerCase());
    }
  }
  return out;
}

/** Common words that aren't worth marking in a subject or passage. */
const UNMARKED = new Set(["the", "and", "for", "with", "from", "about", "that", "this", "what", "when", "who", "how", "did", "does", "was", "were", "are", "you", "your", "our", "my", "me", "to", "of", "in", "on", "at", "an", "is", "it", "i"]);

/** One compiled pattern per query (every row on screen shares it), not one per row per render. */
let markCache: { key: string; re: RegExp | null } = { key: "", re: null };
function markRegex(terms: string[]): RegExp | null {
  const key = terms.join("\u0000");
  if (markCache.key === key) return markCache.re;
  const marked = terms.filter((t) => t.length >= 3 && !UNMARKED.has(t));
  const alt = marked.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  const re = marked.length ? new RegExp(`((?<![\\p{L}\\p{N}])(?:${alt})[\\p{L}\\p{N}]*)`, "giu") : null;
  markCache = { key, re };
  return re;
}

/** Marks the query's words where a word starts with one ("rent" marks "rent", "rents", not "current"). */
function Highlight({ text, terms }: { text: string; terms: string[] }) {
  const re = markRegex(terms);
  if (!re) return <>{text}</>;
  const parts = text.split(re);
  return (
    <>
      {parts.map((p, i) => (i % 2 === 1 ? <mark key={i}>{p}</mark> : <Fragment key={i}>{p}</Fragment>))}
    </>
  );
}

/** A saved/recent query rendered with the same operator colors as the box. */
function HighlightedQuery({ q }: { q: string }): ReactNode {
  return (
    <span className="mono q-mini sx-q">
      {tokenize(q).map((t) =>
        t.kind === "op" ? (
          <Fragment key={t.start}>
            <span className="qm-op">{t.text.slice(0, t.valueStart - t.start)}</span>
            {t.op === "date" ? <span className="qm-date">{t.text.slice(t.valueStart - t.start)}</span> : t.text.slice(t.valueStart - t.start)}
          </Fragment>
        ) : (
          <Fragment key={t.start}>{t.text}</Fragment>
        ),
      )}
    </span>
  );
}

function nextGroup(items: Item[], from: number, dir: 1 | -1): number {
  const g = items[from]?.group;
  if (dir === 1) {
    for (let i = from + 1; i < items.length; i++) if (items[i].group !== g) return i;
    return 0;
  }
  // Shift+Tab: start of the current group, or of the previous one if already there.
  let start = from;
  while (start > 0 && items[start - 1].group === g) start--;
  if (start !== from) return start;
  if (start === 0) {
    let s = items.length - 1;
    while (s > 0 && items[s - 1].group === items[items.length - 1].group) s--;
    return s;
  }
  let s = start - 1;
  const pg = items[s].group;
  while (s > 0 && items[s - 1].group === pg) s--;
  return s;
}
