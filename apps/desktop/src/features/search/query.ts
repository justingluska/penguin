// Client-side query tokenizer. It only drives syntax highlighting, autocomplete
// and facet editing in the search box; the authoritative parse (chips, SQL)
// happens in penguin-core. Keep the operator list in sync with its parser
// (penguin_core::query::OPERATOR_KEYS) and docs/SEARCH.md.

import { parseDatePhrase } from "./dates.ts";

/** Operators, in the order autocomplete offers them. */
export const OPERATORS = [
  "from",
  "to",
  "cc",
  "bcc",
  "with",
  "domain",
  "subject",
  "has",
  "is",
  "in",
  "label",
  "category",
  "account",
  "date",
  "on",
  "before",
  "after",
  "since",
  "until",
  "older_than",
  "newer_than",
  "filename",
  "larger",
  "smaller",
  "size",
  "messages",
  "day",
  "type",
] as const;
export type Operator = (typeof OPERATORS)[number];

const OP_SET = new Set<string>(OPERATORS);

/** Aliases the parser accepts but autocomplete doesn't offer. */
const OP_ALIASES: Record<string, Operator> = {
  older: "older_than",
  newer: "newer_than",
  folder: "in",
  participant: "with",
  // Other mail apps' spellings (Gmail deliveredto:, Outlook/KQL participants:,
  // attachment:, hasattachment:yes).
  deliveredto: "to",
  participants: "with",
  attachment: "filename",
  attachmentnames: "filename",
  hasattachment: "has",
  hasattachments: "has",
};

/** Straight and typographic quotes (smart quotes, „…“), as the parser reads them. */
const OPEN_QUOTE = /["“„”]/;
const CLOSE_QUOTE = /["”“]/;

function closeQuote(q: string, from: number): number {
  for (let k = from; k < q.length; k++) if (CLOSE_QUOTE.test(q[k])) return k;
  return -1;
}

/** Uppercase words the parser reads as logic: OR, AND, NOT, AROUND/NEAR. */
const LOGIC = new Set(["OR", "AND", "NOT", "AROUND", "NEAR"]);

/**
 * Free text is never a date (so "february" can be searched as a word): dates
 * come only from `date:value`, before:/after: and older_than:/newer_than:.
 * The "Use 'february' as a date?" hint lives in dates.ts.
 */

export type Token =
  | { kind: "space"; start: number; end: number; text: string }
  /** `(`, `-(`, `{`, `-{`, or a `)` / `}` that closes one. */
  | { kind: "paren"; start: number; end: number; text: string }
  | { kind: "word"; start: number; end: number; text: string; negated: boolean }
  | { kind: "phrase"; start: number; end: number; text: string; negated: boolean }
  /** A logic word: OR, AND, NOT, AROUND/NEAR (uppercase). */
  | { kind: "or"; start: number; end: number; text: string }
  | {
      kind: "op";
      start: number;
      end: number;
      text: string;
      negated: boolean;
      op: Operator;
      /** Offset where the value starts (after the colon). */
      valueStart: number;
      value: string;
    };

/** Split a query into tokens that cover every character exactly once. */
export function tokenize(q: string): Token[] {
  const out: Token[] = [];
  let depth = 0;
  let i = 0;
  while (i < q.length) {
    const c = q[i];
    if (/\s/.test(c)) {
      let j = i;
      while (j < q.length && /\s/.test(q[j])) j++;
      out.push({ kind: "space", start: i, end: j, text: q.slice(i, j) });
      i = j;
      continue;
    }
    // Grouping, as penguin-core reads it: "(" / "-(" and Gmail's "{" / "-{"
    // open, ")" or "}" closes an open one.
    if (c === "(" || c === "{" || (c === "-" && (q[i + 1] === "(" || q[i + 1] === "{"))) {
      const end = c === "-" ? i + 2 : i + 1;
      out.push({ kind: "paren", start: i, end, text: q.slice(i, end) });
      depth++;
      i = end;
      continue;
    }
    if ((c === ")" || c === "}") && depth > 0) {
      out.push({ kind: "paren", start: i, end: i + 1, text: c });
      depth--;
      i++;
      continue;
    }
    const negated = c === "-" && i + 1 < q.length && !/\s/.test(q[i + 1]);
    const bodyStart = negated ? i + 1 : i;
    if (OPEN_QUOTE.test(q[bodyStart] ?? "")) {
      const close = closeQuote(q, bodyStart + 1);
      const end = close === -1 ? q.length : close + 1;
      out.push({ kind: "phrase", start: i, end, text: q.slice(i, end), negated });
      i = end;
      continue;
    }
    let j = bodyStart;
    // `subject:(dinner movie)`: the operator's value runs to the matching ")".
    const group = /^[A-Za-z_]+:\(/.exec(q.slice(bodyStart));
    if (group) {
      let d = 0;
      for (let k = bodyStart + group[0].length - 1; k < q.length; k++) {
        if (q[k] === "(") d++;
        else if (q[k] === ")" && --d === 0) {
          j = k + 1;
          break;
        }
      }
    }
    while (j < q.length && !/\s/.test(q[j]) && !((q[j] === ")" || q[j] === "}") && depth > 0)) {
      // A quoted operator value may contain spaces: subject:"q4 plan"
      if (OPEN_QUOTE.test(q[j])) {
        const close = closeQuote(q, j + 1);
        j = close === -1 ? q.length : close + 1;
        continue;
      }
      j++;
    }
    const text = q.slice(i, j);
    const body = q.slice(bodyStart, j);
    const colon = body.indexOf(":");
    const rawKey = colon > 0 ? body.slice(0, colon).toLowerCase() : "";
    const key = OP_ALIASES[rawKey] ?? rawKey;
    if (colon > 0 && OP_SET.has(key)) {
      out.push({
        kind: "op",
        start: i,
        end: j,
        text,
        negated,
        op: key as Operator,
        valueStart: bodyStart + colon + 1,
        value: body.slice(colon + 1).replace(/^["“„”]|["”“]$/g, ""),
      });
    } else if (LOGIC.has(text)) {
      out.push({ kind: "or", start: i, end: j, text });
    } else {
      out.push({ kind: "word", start: i, end: j, text, negated });
    }
    i = j;
  }
  return out;
}

/** Token under (or immediately before) the caret, ignoring whitespace. */
export function tokenAt(tokens: Token[], caret: number): Token | null {
  for (const t of tokens) {
    if (t.kind === "space") continue;
    if (caret > t.start && caret <= t.end) return t;
  }
  return null;
}

/** Remove a substring (a chip's `raw`) from the query and tidy whitespace. */
export function removeRaw(q: string, raw: string): string {
  const idx = q.indexOf(raw);
  if (idx === -1) {
    // Fall back to a case-insensitive match; the parser may normalize case.
    const li = q.toLowerCase().indexOf(raw.toLowerCase());
    if (li === -1) return q;
    return tidy(q.slice(0, li) + q.slice(li + raw.length));
  }
  return tidy(q.slice(0, idx) + q.slice(idx + raw.length));
}

function tidy(q: string): string {
  return q.replace(/\s{2,}/g, " ").trim();
}

/** Values of every `op:` token in the query (lowercased). */
export function opValues(q: string, op: Operator): string[] {
  return tokenize(q)
    .filter((t): t is Extract<Token, { kind: "op" }> => t.kind === "op" && t.op === op && !t.negated)
    .map((t) => t.value.toLowerCase());
}

/** Toggle `op:value` in the query (add if absent, remove if present). */
export function toggleOp(q: string, op: Operator, value: string): string {
  const toks = tokenize(q);
  const hit = toks.find((t) => t.kind === "op" && t.op === op && !t.negated && t.value.toLowerCase() === value.toLowerCase());
  if (hit) return tidy(q.slice(0, hit.start) + q.slice(hit.end));
  return tidy(`${q} ${op}:${quoteIfNeeded(value)}`);
}

/** Replace every `op:` token with a single `op:value` (or remove them if value is null). */
export function setOp(q: string, op: Operator, value: string | null): string {
  let next = q;
  for (;;) {
    const t = tokenize(next).find((x) => x.kind === "op" && x.op === op && !x.negated);
    if (!t) break;
    next = next.slice(0, t.start) + next.slice(t.end);
  }
  next = tidy(next);
  return value === null ? next : tidy(`${next} ${op}:${quoteIfNeeded(value)}`);
}

/** Replace any date:/before:/after:/older_than:/newer_than: with `date:<phrase>` (or remove them when null). */
export function setDate(q: string, phrase: string | null): string {
  let next = q;
  for (;;) {
    const t = tokenize(next).find((x) => x.kind === "op" && !x.negated && isDateOp(x.op));
    if (!t) break;
    next = next.slice(0, t.start) + next.slice(t.end);
  }
  next = tidy(next);
  return phrase === null ? next : tidy(`${next} date:${quoteIfNeeded(phrase)}`);
}

/** The `date:` value, lowercased, if it reads as a date (null otherwise). */
export function currentDate(q: string): string | null {
  const t = tokenize(q).find((x) => x.kind === "op" && x.op === "date" && !x.negated);
  if (!t || t.kind !== "op") return null;
  const v = t.value.toLowerCase();
  return parseDatePhrase(v) ? v : null;
}

export function quoteIfNeeded(v: string): string {
  return /\s/.test(v) ? `"${v}"` : v;
}

export function isDateOp(op: Operator): boolean {
  return (
    op === "date" ||
    op === "on" ||
    op === "before" ||
    op === "after" ||
    op === "since" ||
    op === "until" ||
    op === "older_than" ||
    op === "newer_than"
  );
}

/** True when the query uses any operator, quotes or grouping (it's already structured). */
export function isStructured(q: string): boolean {
  return tokenize(q).some((t) => t.kind === "op" || t.kind === "or" || t.kind === "paren" || t.kind === "phrase" || (t.kind === "word" && t.negated));
}
