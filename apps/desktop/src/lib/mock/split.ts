// Split Inbox in mock mode (features/split): the inbox divided by the splits'
// queries, and the tab counts. Queries are matched by a small version of the
// search language (matchesQuery): words, from:/to:/with:/domain:, is:, has:,
// category:, subject:, OR, -negation and (groups); anything it doesn't know
// (dates) matches everything, which is enough for the mock mailbox.
import type { ListQuery, SplitCounts, SplitFilter, ThreadSummary } from "../types";
import type { MockHandler } from "./index";

const NEWSLETTER_LABELS = ["CATEGORY_PROMOTIONS", "CATEGORY_UPDATES", "CATEGORY_FORUMS", "CATEGORY_SOCIAL"];

type Node = { kind: "and" | "or"; parts: Node[] } | { kind: "not"; part: Node } | { kind: "term"; op: string; val: string };

function tokenize(q: string): string[] {
  const out: string[] = [];
  const re = /\s*(\(|\)|-?[a-z_]+:"[^"]*"|-?"[^"]*"|[^\s()]+)/gi;
  for (const m of q.matchAll(re)) out.push(m[1]);
  return out;
}

function parse(q: string): Node {
  const toks = tokenize(q);
  let i = 0;
  const and = (): Node => {
    const parts: Node[] = [];
    while (i < toks.length && toks[i] !== ")") parts.push(or());
    return { kind: "and", parts };
  };
  const or = (): Node => {
    const parts = [unary()];
    while (toks[i] === "OR") {
      i++;
      parts.push(unary());
    }
    return parts.length === 1 ? parts[0] : { kind: "or", parts };
  };
  const unary = (): Node => {
    let t = toks[i++] ?? "";
    if (t === "-(" || (t === "-" && toks[i] === "(")) {
      if (t === "-") i++;
      const inner = and();
      i++;
      return { kind: "not", part: inner };
    }
    if (t === "(") {
      const inner = and();
      i++;
      return inner;
    }
    const neg = t.startsWith("-") && t.length > 1;
    if (neg) t = t.slice(1);
    const c = t.indexOf(":");
    const term: Node =
      c > 0 ? { kind: "term", op: t.slice(0, c).toLowerCase(), val: t.slice(c + 1).replace(/"/g, "").toLowerCase() } : { kind: "term", op: "", val: t.replace(/"/g, "").toLowerCase() };
    return neg ? { kind: "not", part: term } : term;
  };
  return and();
}

function termMatches(t: ThreadSummary, op: string, val: string): boolean {
  const people = t.participants.map((p) => `${p.email} ${p.name ?? ""}`.toLowerCase());
  const has = (l: string) => t.labelIds.includes(l);
  switch (op) {
    case "from":
    case "to":
    case "cc":
    case "with":
    case "participant":
      return people.some((p) => p.includes(val));
    case "domain":
      return people.some((p) => p.includes("@" + val.replace(/^@/, "")));
    case "is":
      if (val === "important") return has("IMPORTANT");
      if (val === "newsletter" || val === "newsletters") return t.labelIds.some((l) => NEWSLETTER_LABELS.includes(l));
      if (val === "unread") return t.unread;
      if (val === "read") return !t.unread;
      if (val === "starred") return t.starred;
      if (val === "inbox") return has("INBOX");
      // The mock keeps no first contacts: a person is anyone who isn't bulk mail.
      if (val === "known-sender" || val === "known") return !t.labelIds.some((l) => NEWSLETTER_LABELS.includes(l));
      return true;
    case "has":
      if (val === "attachment" || val === "pdf") return t.hasAttachments;
      if (val === "invite") return !!t.invite || /^(invitation|updated invitation):/i.test(t.subject);
      return true;
    case "in":
      return val === "inbox" ? has("INBOX") : true;
    case "category":
      return has("CATEGORY_" + val.toUpperCase());
    case "label":
      return t.labelIds.some((l) => l.toLowerCase().includes(val));
    case "subject":
      return t.subject.toLowerCase().includes(val);
    case "":
      return `${t.subject} ${t.snippet}`.toLowerCase().includes(val);
    default:
      // Dates and the rest: no constraint in the mock.
      return true;
  }
}

function evalNode(t: ThreadSummary, n: Node): boolean {
  switch (n.kind) {
    case "and":
      return n.parts.every((p) => evalNode(t, p));
    case "or":
      return n.parts.some((p) => evalNode(t, p));
    case "not":
      return !evalNode(t, n.part);
    case "term":
      return termMatches(t, n.op, n.val);
  }
}

/** Whether a conversation matches a query, the mock's way. */
export function matchesQuery(t: ThreadSummary, query: string): boolean {
  return evalNode(t, parse(query));
}

/** Whether a conversation belongs to this split: its query, and none of the earlier ones. */
export function inSplit(t: ThreadSummary, split: SplitFilter): boolean {
  if (split.include !== null && !matchesQuery(t, split.include)) return false;
  return !split.exclude.some((q) => matchesQuery(t, q));
}

/** list_threads with `split`: the inbox rows, filtered. */
export function withSplits(inner: MockHandler): MockHandler {
  return async (args) => {
    const q = args.query as ListQuery;
    if (q.view.kind !== "inbox" || !q.split) return inner(args);
    const split = q.split;
    const rows = (await inner({ ...args, query: { ...q, split: null, tab: null, limit: 100_000 } })) as ThreadSummary[];
    return rows.filter((t) => inSplit(t, split)).slice(0, q.limit);
  };
}

export function splitHandlers(listThreads: MockHandler): Record<string, MockHandler> {
  return {
    split_counts: async ({ queries, accountIds }): Promise<SplitCounts> => {
      const qs = queries as string[];
      const rows = (await listThreads({
        query: { view: { kind: "inbox" }, tab: null, accountId: null, accountIds: accountIds ?? null, limit: 100_000, before: null },
      })) as ThreadSummary[];
      const splits = [...qs, null].map(() => ({ total: 0, unread: 0 }));
      for (const t of rows) {
        let i = qs.findIndex((q) => matchesQuery(t, q));
        if (i < 0) i = qs.length;
        splits[i].total++;
        if (t.unread) splits[i].unread++;
      }
      return { splits, more: false };
    },
  };
}
