// Facet counts: for each facet group, re-run the query with that group's own
// operators removed (so choosing "Image" can show how many there would be).
// Runs only after the results settle, sequentially, and gives up as soon as
// the query changes. Everything is a local index read.
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import type { SearchResponse } from "../../lib/types";
import { currentDate, isDateOp, setDate, setOp, tokenize } from "./query";

export const HAS_OPTIONS: Array<[string, string]> = [
  ["pdf", "PDF"],
  ["image", "Image"],
  ["doc", "Document"],
  ["spreadsheet", "Spreadsheet"],
  ["presentation", "Presentation"],
];

export const DATE_PRESETS: Array<[string | null, string]> = [
  [null, "Any time"],
  ["past week", "Past week"],
  ["past month", "Past month"],
  ["past 12 months", "Past 12 months"],
];

export interface FacetCounts {
  query: string;
  byAccount: Record<string, number> | null;
  byHas: Record<string, number>;
  byDate: Record<string, number>;
}

const COUNT_LIMIT = 500;
const cache = new Map<string, number | SearchResponse>();

async function run(q: string, accountIds: string[] | null): Promise<SearchResponse> {
  const k = `r:${accountIds?.join(",") ?? ""}:${q}`;
  const hit = cache.get(k);
  if (hit && typeof hit !== "number") return hit;
  const r = await api.search({ query: q, accountId: null, accountIds, limit: COUNT_LIMIT });
  if (cache.size > 200) cache.clear();
  cache.set(k, r);
  return r;
}

function hasTextBeyondDate(q: string) {
  return tokenize(q).some((t) => t.kind !== "space" && !(t.kind === "op" && isDateOp(t.op)));
}

/** `accountIds` is the search scope (a profile's accounts), as in the main query. */
export function useFacetCounts(
  query: string,
  resp: SearchResponse | null,
  respQuery: string,
  accountIds: string[] | null = null,
): FacetCounts | null {
  const scopeKey = accountIds?.join("\u0000") ?? "";
  const [counts, setCounts] = useState<FacetCounts | null>(null);

  useEffect(() => {
    if (!resp || respQuery !== query || !query.trim()) return;
    let cancelled = false;
    const timer = window.setTimeout(async () => {
      const out: FacetCounts = { query, byAccount: null, byHas: {}, byDate: {} };
      try {
        // Accounts: one query without account:, grouped by account.
        const noAcc = setOp(query, "account", null);
        if (noAcc.trim()) {
          const r = noAcc === query ? resp : await run(noAcc, accountIds);
          if (cancelled) return;
          out.byAccount = {};
          for (const h of r.hits) out.byAccount[h.accountId] = (out.byAccount[h.accountId] ?? 0) + 1;
        }
        // Has: one query per type.
        const noHas = setOp(query, "has", null);
        for (const [v] of HAS_OPTIONS) {
          const r = await run(`${noHas} has:${v}`.trim(), accountIds);
          if (cancelled) return;
          out.byHas[v] = r.hits.length;
        }
        // Date presets (only when there's something besides a date to search).
        if (hasTextBeyondDate(query)) {
          const cur = currentDate(query);
          const phrases = DATE_PRESETS.map(([p]) => p);
          if (cur && !phrases.includes(cur)) phrases.push(cur);
          for (const p of phrases) {
            const vq = setDate(query, p);
            const r = vq === query ? resp : await run(vq, accountIds);
            if (cancelled) return;
            out.byDate[p ?? "any"] = r.hits.length;
          }
        }
        if (!cancelled) setCounts(out);
      } catch {
        // Counts are decoration; a failed count query leaves them blank.
        if (!cancelled) setCounts(null);
      }
    }, 280);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
    // scopeKey stands for accountIds (a new array each render); resp already reflects it.
  }, [query, resp, respQuery, scopeKey]);

  // Previous counts stay visible (the UI dims them) until the new ones land,
  // so numbers don't flash on every keystroke.
  return query.trim() ? counts : null;
}
