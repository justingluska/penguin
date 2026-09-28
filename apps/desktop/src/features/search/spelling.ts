// "Did you mean": the words of mail Penguin has shown (subjects, previews,
// names, file names) form a small lexicon in memory; a typed word that
// matches none of them is compared with its closest neighbours by edit
// distance. The UI offers a correction only after running it and finding
// mail, so a suggestion is never a dead end. Local, no model, no network.
import type { SearchResponse } from "../../lib/types";
import { tokenize } from "./query.ts";

const words = new Map<string, number>();
/** Sorted keys, rebuilt lazily after learning. */
let sorted: string[] | null = null;
const MAX_WORDS = 50_000;

/** Learn the words of a piece of text (subjects, previews, names…). */
export function learn(text: string | null | undefined): void {
  if (!text) return;
  for (const w of text.toLowerCase().match(/\p{L}[\p{L}'-]{2,}/gu) ?? []) {
    const t = w.replace(/['-]+$/, "");
    if (t.length < 3) continue;
    const n = words.get(t);
    if (n !== undefined) {
      words.set(t, n + 1);
      continue;
    }
    if (words.size >= MAX_WORDS) continue;
    words.set(t, 1);
    // Only a new word invalidates the sorted index.
    sorted = null;
  }
}

export function lexiconSize(): number {
  return words.size;
}

/** For tests. */
export function forgetLexicon(): void {
  words.clear();
  sorted = null;
}

/** A known word, or the start of one (the last word is matched as a prefix while typing). */
export function isKnown(w: string): boolean {
  const t = w.toLowerCase();
  if (words.has(t)) return true;
  sorted ??= [...words.keys()].sort();
  // Binary search for the first key >= t, then check it's an extension.
  let lo = 0;
  let hi = sorted.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid] < t) lo = mid + 1;
    else hi = mid;
  }
  return lo < sorted.length && sorted[lo].startsWith(t);
}

/** Optimal string alignment distance (Damerau–Levenshtein with adjacent swaps), capped. */
export function editDistance(a: string, b: string, cap = 3): number {
  if (Math.abs(a.length - b.length) > cap) return cap + 1;
  const prev2 = new Array<number>(b.length + 1).fill(0);
  let prev = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    const cur = [i];
    let rowMin = i;
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      let v = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) v = Math.min(v, prev2[j - 2] + 1);
      cur.push(v);
      rowMin = Math.min(rowMin, v);
    }
    for (let j = 0; j <= b.length; j++) prev2[j] = prev[j];
    prev = cur;
    if (rowMin > cap) return cap + 1;
  }
  return prev[b.length];
}

/** The closest known word to a word no mail contains, or null. */
export function correctWord(w: string): string | null {
  const t = w.toLowerCase();
  if (t.length < 4 || /\d/.test(t) || isKnown(t)) return null;
  const max = t.length <= 5 ? 1 : 2;
  let best: { w: string; d: number; n: number } | null = null;
  for (const [cand, n] of words) {
    if (Math.abs(cand.length - t.length) > max) continue;
    const d = editDistance(t, cand, max);
    if (d > max) continue;
    // Closer first; then the same first letter (people rarely mistype it); then the commoner word.
    const sameStart = cand[0] === t[0];
    let better = !best;
    if (best) {
      if (d !== best.d) better = d < best.d;
      else if (sameStart !== (best.w[0] === t[0])) better = sameStart;
      else better = n > best.n;
    }
    if (better) best = { w: cand, d, n };
  }
  return best?.w ?? null;
}

/** Answers not read yet: learning waits until a correction is needed, never on the typing path. */
const pending: SearchResponse[] = [];

/** Remember an answer to learn from later (cheap: no text is read now). */
export function learnLater(r: SearchResponse): void {
  pending.push(r);
  if (pending.length > 30) pending.shift();
}

function flushPending(): void {
  while (pending.length) learnFromResponse(pending.shift()!);
}

/** Learn what a search answer shows: subjects, previews, passages, names, file names. */
export function learnFromResponse(r: SearchResponse): void {
  for (const h of r.hits) {
    learn(h.subject);
    learn(h.snippetHtml.replace(/<[^>]*>/g, " ").replace(/&[a-z#0-9]+;/gi, " "));
    learn(h.passage);
    learn(h.from.name);
  }
  for (const a of r.attachments) learn(a.attachment.filename.replace(/[._-]+/g, " "));
  for (const p of r.people) learn(p.address.name);
}

/** The query with each unknown free word corrected, or null when nothing changes. */
export function didYouMean(query: string): string | null {
  flushPending();
  let out = "";
  let changed = false;
  for (const t of tokenize(query)) {
    if (t.kind === "word" && !t.negated && !t.text.includes(":")) {
      const fix = correctWord(t.text);
      if (fix) {
        out += fix;
        changed = true;
        continue;
      }
    }
    out += t.text;
  }
  return changed ? out : null;
}
