// Search as you type, without flicker or wasted work.
//
// - Every keystroke searches at once (local index: no debounce, no frame wait).
// - At most one search is in flight. Keystrokes typed meanwhile collapse into
//   the newest query, sent the moment the answer comes back, so a slower
//   search (words + meaning) never queues work for text already replaced;
//   the answer that was in flight still shows, as the closest there is.
// - An answer with nothing in it doesn't replace results on screen right
//   away: the previous rows stay (dimmed) for a moment in case the next key
//   brings matches back, so typing through a word doesn't flash "No mail".
// - While search by meaning is still indexing, the same query is quietly run
//   again every few seconds; new matches slide in below the selection
//   (stable.ts) and the "Getting smarter" line updates.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { SearchResponse } from "../../lib/types";
import { rememberFromResponse } from "./people";
import { learnLater } from "./spelling";

/** How long an empty answer waits before replacing results (Raycast: never flash "No results" mid-typing). */
const EMPTY_GRACE_MS = 220;
/** Re-run the query this often while meaning search is still indexing. */
const INDEXING_REFRESH_MS = 3000;

export interface InstantSearch {
  resp: SearchResponse | null;
  /** The query `resp` answers. */
  respQuery: string;
  error: string | null;
  /** The rows on screen are from an earlier query (an empty answer is being held back). */
  stale: boolean;
  /** Run the current query again (same query: results merge in place). */
  refresh: () => void;
}

interface Want {
  key: string;
  text: string;
  scope: string[] | null;
}

const isEmpty = (r: SearchResponse) => r.hits.length === 0 && r.attachments.length === 0 && (r.events?.length ?? 0) === 0;

export function useInstantSearch(text: string, scopeIds: string[] | null, limit: number, initial?: { resp: SearchResponse | null; respQuery: string }): InstantSearch {
  const [resp, setResp] = useState<SearchResponse | null>(initial?.resp ?? null);
  const [respQuery, setRespQuery] = useState(initial?.respQuery ?? "");
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const want = useRef<Want | null>(null);
  const inFlight = useRef<Want | null>(null);
  /** The key last sent (a refresh clears it to send the same query again). */
  const sent = useRef<string | null>(null);
  const shown = useRef<SearchResponse | null>(initial?.resp ?? null);
  const emptyTimer = useRef<number | null>(null);
  const alive = useRef(true);

  const apply = useCallback((w: Want, r: SearchResponse) => {
    if (emptyTimer.current !== null) {
      window.clearTimeout(emptyTimer.current);
      emptyTimer.current = null;
    }
    const commit = () => {
      emptyTimer.current = null;
      shown.current = r;
      setResp(r);
      setRespQuery(w.text);
      setError(null);
      setStale(false);
    };
    rememberFromResponse(r);
    // Words for "did you mean", read only when a correction is needed.
    learnLater(r);
    if (isEmpty(r) && shown.current && !isEmpty(shown.current)) {
      setStale(true);
      emptyTimer.current = window.setTimeout(commit, EMPTY_GRACE_MS);
    } else {
      commit();
    }
  }, []);

  const pump = useCallback(() => {
    const w = want.current;
    if (inFlight.current || !w || sent.current === w.key) return;
    inFlight.current = w;
    sent.current = w.key;
    api
      .search({ query: w.text, accountId: null, accountIds: w.scope, limit })
      .then(
        (r) => {
          // Answers arrive in order (one in flight), so even one for text since
          // extended is the newest there is: show it while the latest is sent.
          if (alive.current && want.current) apply(w, r);
        },
        (e: unknown) => {
          if (!alive.current || want.current?.key !== w.key) return;
          setError(asCommandError(e).message);
        },
      )
      .finally(() => {
        inFlight.current = null;
        if (alive.current) pump();
      });
  }, [apply, limit]);

  const scopeKey = scopeIds?.join("\u0000") ?? "";
  useEffect(() => {
    const key = `${scopeKey}\u0001${text}`;
    if (!text.trim()) {
      want.current = null;
      sent.current = null;
      if (emptyTimer.current !== null) window.clearTimeout(emptyTimer.current);
      emptyTimer.current = null;
      shown.current = null;
      setResp(null);
      setRespQuery("");
      setError(null);
      setStale(false);
      return;
    }
    want.current = { key, text, scope: scopeIds };
    pump();
    // scopeKey stands for scopeIds (a new array each render).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, scopeKey, pump]);

  const refresh = useCallback(() => {
    if (!want.current) return;
    sent.current = null;
    pump();
  }, [pump]);

  // Meaning search still indexing: run the query again now and then.
  const indexing = resp?.semantic === "indexing" && respQuery === text && !!text.trim();
  useEffect(() => {
    if (!indexing) return;
    const t = window.setInterval(refresh, INDEXING_REFRESH_MS);
    return () => window.clearInterval(t);
  }, [indexing, refresh]);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      if (emptyTimer.current !== null) window.clearTimeout(emptyTimer.current);
    };
  }, []);

  return { resp, respQuery, error, stale, refresh };
}
