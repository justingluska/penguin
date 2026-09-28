// Suggestions as you type plain words: people (with their avatar), companies
// by their mail domain, recent searches, and refinements drawn from the
// results on screen ("in Inbox", "with attachments", "from Mike Delgado").
// Each suggestion carries the whole next query, so accepting one (Tab or a
// click) is a single edit. Nothing here teaches syntax: the chips under the
// box say in words what an accepted suggestion does.
//
// Operator values (from:mi…) are completed by QueryInput's own list.
import type { Address, SearchHit } from "../../lib/types";
import { opValues, tokenize, type Token } from "./query.ts";

export type SuggestionKind = "person" | "domain" | "recent" | "refine";

export interface Suggestion {
  key: string;
  kind: SuggestionKind;
  /** What the row says ("Mike Delgado", "cedarpine.example", "In Inbox"). */
  label: string;
  /** Quieter second part (an address, "Anyone at", a count). */
  detail?: string;
  address?: Address;
  /** The whole query after accepting. */
  next: string;
}

export interface SuggestContext {
  /** People seen locally, best-known first. */
  people: Array<{ address: Address; seen: number }>;
  recent: string[];
  /** Your own addresses (never suggested as a person or company). */
  mine?: string[];
  /** Results on screen for this query (refinements come from them). */
  hits?: SearchHit[];
  /** Label ids that mean "in the inbox" (Gmail INBOX). */
  inboxLabel?: string;
}

/** Mail providers, not companies: "anyone at gmail" isn't a useful search. */
const FREE_MAIL = /^(gmail|googlemail|outlook|hotmail|live|msn|icloud|me|mac|yahoo|aol|proton|protonmail|pm|fastmail|gmx|mail|zoho|hey)\./;
/** Plain-English lead-ins that say which side the person is on. */
const LEADS: Record<string, "from" | "to" | "with"> = { from: "from", by: "from", to: "to", with: "with" };
const MAX = 6;

/** Everyday words that end a descriptive query ("rent going up", "notes from the") far more
 *  often than they start a name; they only suggest a person after a lead-in ("from the…"). */
const PLAIN = new Set(
  "the and for but not you are was our has had any all how why who off out new old get got can did its his her him one two".split(" "),
);

const lower = (s: string | null | undefined) => (s ?? "").toLowerCase();

function nameWords(a: Address): string[] {
  return lower(a.name).split(/\s+/).filter(Boolean);
}

/** Does `typed` (one or more words) start this person's name or address? */
function personMatches(a: Address, typed: string[]): boolean {
  const email = lower(a.email);
  // An address prefix needs three letters ("up" isn't updates@…); a name word needs two.
  if (typed.length === 1 && typed[0].length >= 3 && email.startsWith(typed[0])) return true;
  const words = nameWords(a);
  if (typed.length === 1) return words.some((w) => w.startsWith(typed[0]));
  // "mike del": consecutive name words, the last one a prefix.
  for (let i = 0; i + typed.length <= words.length; i++) {
    if (typed.every((t, j) => (j === typed.length - 1 ? words[i + j].startsWith(t) : words[i + j] === t))) return true;
  }
  return false;
}

function join(before: string, insert: string, after = ""): string {
  return `${before.replace(/\s+$/, "")} ${insert} ${after.replace(/^\s+/, "")}`.replace(/\s+/g, " ").trim();
}

/** The trailing plain words of the query (those a suggestion may replace), newest last. */
function trailingWords(toks: Token[]): Array<Extract<Token, { kind: "word" }>> {
  const out: Array<Extract<Token, { kind: "word" }>> = [];
  for (let i = toks.length - 1; i >= 0; i--) {
    const t = toks[i];
    if (t.kind === "space") continue;
    if (t.kind !== "word" || t.negated) break;
    out.unshift(t);
  }
  return out;
}

export function suggestFor(query: string, ctx: SuggestContext): Suggestion[] {
  const q = query;
  if (!q.trim()) return [];
  const toks = tokenize(q);
  const last = toks[toks.length - 1];
  // Completing an operator's value: QueryInput's list handles it.
  if (last && last.kind === "op") return [];
  const mine = new Set((ctx.mine ?? []).map(lower));
  const out: Suggestion[] = [];
  const seen = new Set<string>();
  const push = (s: Suggestion) => {
    if (seen.has(s.key) || s.next.trim() === q.trim() || out.length >= MAX) return;
    seen.add(s.key);
    out.push(s);
  };

  const typing = !!last && last.kind === "word" && !last.negated && !/\s$/.test(q);
  if (typing) {
    const words = trailingWords(toks);
    const w = lower(words[words.length - 1]?.text);
    // People: the last word, or the last two ("mike del"), after an optional lead-in.
    const already = new Set([...opValues(q, "from"), ...opValues(q, "to"), ...opValues(q, "with")]);
    for (const n of [2, 1]) {
      if (words.length < n) continue;
      const span = words.slice(words.length - n);
      const typed = span.map((t) => lower(t.text));
      if (typed.some((t) => t.length < (n === 1 ? 2 : 1)) || typed.some((t) => LEADS[t])) continue;
      const leadTok = words[words.length - n - 1];
      const lead = leadTok ? LEADS[lower(leadTok.text)] : undefined;
      // The last word of a longer phrase with no lead-in is usually just a word
      // ("rent going up" isn't UPS): a name there needs three letters and not an everyday word.
      if (n === 1 && !lead && words.length > 1 && (typed[0].length < 3 || PLAIN.has(typed[0]))) continue;
      const from = lead ? leadTok.start : span[0].start;
      const op = lead ?? "from";
      let found = 0;
      for (const p of ctx.people) {
        const email = lower(p.address.email);
        if (mine.has(email) || already.has(email) || !personMatches(p.address, typed)) continue;
        push({
          key: `p:${email}`,
          kind: "person",
          label: p.address.name?.trim() || p.address.email,
          detail: p.address.email,
          address: p.address,
          next: join(q.slice(0, from), `${op}:${email}`),
        });
        if (++found >= 3) break;
      }
      if (found) break;
    }
    // Companies: a domain whose name starts with the word ("cedar" → cedarpine.example).
    if (w.length >= 3 && !LEADS[w]) {
      const domains = new Map<string, number>();
      for (const p of ctx.people) {
        const d = lower(p.address.email.split("@")[1]);
        if (!d || FREE_MAIL.test(d) || mine.has(lower(p.address.email))) continue;
        if (!d.startsWith(w) && !d.split(/[.-]/).some((part) => part.startsWith(w))) continue;
        domains.set(d, (domains.get(d) ?? 0) + p.seen);
      }
      const wt = words[words.length - 1];
      const leadTok = words[words.length - 2];
      const from = leadTok && LEADS[lower(leadTok.text)] ? leadTok.start : wt.start;
      [...domains.entries()]
        .sort((a, b) => b[1] - a[1])
        .slice(0, 2)
        .forEach(([d]) => push({ key: `d:${d}`, kind: "domain", label: d, detail: "Anyone at", next: join(q.slice(0, from), `domain:${d}`) }));
    }
  }

  // Recent searches that start the same way (or contain every typed word).
  const typedWords = lower(q).split(/\s+/).filter(Boolean);
  for (const r of ctx.recent) {
    const lr = lower(r);
    if (lr === lower(q.trim())) continue;
    if (lr.startsWith(lower(q.trim())) || typedWords.every((t) => lr.includes(t))) push({ key: `r:${r}`, kind: "recent", label: r, next: r });
    if (out.filter((s) => s.kind === "recent").length >= 2) break;
  }

  // Refinements from the results on screen, like Gmail's search chips.
  const hits = ctx.hits ?? [];
  if (hits.length >= 3) {
    const has = opValues(q, "has");
    const inOps = opValues(q, "in");
    if (!opValues(q, "from").length) {
      const bySender = new Map<string, { a: Address; n: number }>();
      for (const h of hits) {
        const e = lower(h.from.email);
        if (mine.has(e)) continue;
        bySender.set(e, { a: h.from, n: (bySender.get(e)?.n ?? 0) + 1 });
      }
      const top = [...bySender.values()].sort((a, b) => b.n - a.n)[0];
      // Not when the same person is already offered as you type.
      if (top && top.n >= 2 && top.n < hits.length && !seen.has(`p:${lower(top.a.email)}`)) {
        push({ key: `f:${lower(top.a.email)}`, kind: "refine", label: `From ${top.a.name?.trim() || top.a.email}`, detail: `${top.n}`, address: top.a, next: join(q, `from:${lower(top.a.email)}`) });
      }
    }
    const withFiles = hits.filter((h) => h.hasAttachments).length;
    if (!has.length && withFiles > 0 && withFiles < hits.length) {
      push({ key: "has:attachment", kind: "refine", label: "With attachments", detail: `${withFiles}`, next: join(q, "has:attachment") });
    }
    const inbox = ctx.inboxLabel ?? "INBOX";
    const inInbox = hits.filter((h) => h.labelIds.includes(inbox)).length;
    if (!inOps.length && inInbox > 0 && inInbox < hits.length) {
      push({ key: "in:inbox", kind: "refine", label: "In Inbox", detail: `${inInbox}`, next: join(q, "in:inbox") });
    }
  }
  return out;
}
