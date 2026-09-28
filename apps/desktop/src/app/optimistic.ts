// The optimistic layer's pure parts: unread-count deltas, structural sharing
// of refreshed rows, and the log of changes the backend may not have yet.
// Pure (no store, no React) so it can be unit-tested (tests/optimistic.test.ts).
// Keep this file free of imports and non-erasable TS syntax: the tests run it
// under Node's type stripping.

// ---------------------------------------------------------------------------
// Unread counts
// ---------------------------------------------------------------------------

/** What a thread contributes to its labels' unread counts. */
export interface CountRow {
  accountId: string;
  unread: boolean;
  labelIds: string[];
}

const pairKey = (accountId: string, labelId: string) => accountId + "\u0000" + labelId;

/**
 * The labels an unread thread counts toward, as the backend counts them
 * (store.rs list_labels: unread threads carrying the label). A thread that is
 * all trash or spam only counts there.
 */
export function unreadLabels(r: CountRow): string[] {
  if (!r.unread) return [];
  const dead = r.labelIds.includes("TRASH") || r.labelIds.includes("SPAM");
  return r.labelIds.filter((l) => l !== "UNREAD" && (!dead || l === "TRASH" || l === "SPAM"));
}

/** Per (account, label): how the unread counts move when each `before` becomes `after` (null: unchanged). */
export function countDeltas(pairs: { before: CountRow; after: CountRow }[]): Map<string, number> {
  const out = new Map<string, number>();
  const add = (accountId: string, labels: string[], n: number) => {
    for (const l of labels) {
      const k = pairKey(accountId, l);
      const v = (out.get(k) ?? 0) + n;
      if (v === 0) out.delete(k);
      else out.set(k, v);
    }
  };
  for (const { before, after } of pairs) {
    add(before.accountId, unreadLabels(before), -1);
    add(after.accountId, unreadLabels(after), 1);
  }
  return out;
}

/** a + sign·b, dropping zeros. */
export function addDeltas(a: Map<string, number>, b: Map<string, number>, sign = 1): Map<string, number> {
  const out = new Map(a);
  for (const [k, n] of b) {
    const v = (out.get(k) ?? 0) + sign * n;
    if (v === 0) out.delete(k);
    else out.set(k, v);
  }
  return out;
}

/**
 * Labels with the deltas applied (never below zero). Labels whose count
 * doesn't move keep their object, and with nothing to apply the array itself
 * is returned, so readers that compare by identity don't re-render.
 */
export function applyCounts<L extends { accountId: string; id: string; unreadCount: number | null }>(
  labels: L[],
  d: Map<string, number>,
): L[] {
  if (d.size === 0) return labels;
  let changed = false;
  const out = labels.map((l) => {
    const n = d.get(pairKey(l.accountId, l.id));
    if (!n || l.unreadCount === null) return l;
    changed = true;
    return { ...l, unreadCount: Math.max(0, l.unreadCount + n) };
  });
  return changed ? out : labels;
}

// ---------------------------------------------------------------------------
// Structural sharing
// ---------------------------------------------------------------------------

/** Deep equality for plain JSON-like data (what the backend sends). */
/**
 * Whether two label lists draw the same chips: the same labels in the same
 * order with the same name, kind, color and visibility. Unread counts don't
 * count, so a count change doesn't re-render every row (store.labelLook).
 */
export function sameLabelLook<L extends { accountId: string; id: string; name: string; kind: string; color: string | null; hidden: boolean }>(a: L[], b: L[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i], y = b[i];
    if (x === y) continue;
    if (x.accountId !== y.accountId || x.id !== y.id || x.name !== y.name || x.kind !== y.kind || x.color !== y.color || x.hidden !== y.hidden) return false;
  }
  return true;
}

export function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!sameValue(a[i], b[i])) return false;
    return true;
  }
  if (Array.isArray(b)) return false;
  const ka = Object.keys(a as object);
  const kb = Object.keys(b as object);
  if (ka.length !== kb.length) return false;
  for (const k of ka) {
    if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
    if (!sameValue((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k])) return false;
  }
  return true;
}

/**
 * `next`, reusing each row of `prev` that is unchanged (same thread, equal
 * fields), so memoized rows skip re-rendering after a refresh. When every row
 * is reused in the same order, `prev` itself comes back.
 */
export function shareRows<T extends { accountId: string; threadId: string }>(prev: T[], next: T[]): T[] {
  if (prev.length === 0) return next;
  const old = new Map<string, T>();
  for (const t of prev) old.set(t.accountId + "\u0000" + t.threadId, t);
  let same = prev.length === next.length;
  const out = next.map((t, i) => {
    const o = old.get(t.accountId + "\u0000" + t.threadId);
    const keep = o !== undefined && sameValue(o, t) ? o : t;
    if (keep !== prev[i]) same = false;
    return keep;
  });
  return same ? prev : out;
}

// ---------------------------------------------------------------------------
// Pending changes
// ---------------------------------------------------------------------------

/**
 * An optimistic change: shown at once, while the backend call runs. `settled`
 * is the tick at which the backend had it (null: still running).
 */
export interface Op<P> {
  id: number;
  keys: Set<string>;
  patch: P | null;
  counts: Map<string, number>;
  settled: number | null;
}

/**
 * The changes a backend read may not reflect yet. A read (list page, thread,
 * labels) notes when it started; any change still running then, or settled
 * after it started, is re-applied over its answer so a slow read never
 * flashes the old state back. Settled changes no read can miss are dropped.
 */
export function createOpLog<P>() {
  let tick = 0;
  let nextId = 0;
  let nextRead = 0;
  let ops: Op<P>[] = [];
  const reads = new Map<number, number>();

  const gc = () => {
    let min = Infinity;
    for (const t of reads.values()) min = Math.min(min, t);
    if (ops.some((o) => o.settled !== null && o.settled <= min)) ops = ops.filter((o) => o.settled === null || o.settled > min);
  };

  return {
    add(keys: Iterable<string>, patch: P | null, counts: Map<string, number>): Op<P> {
      const op: Op<P> = { id: ++nextId, keys: new Set(keys), patch, counts, settled: null };
      ops.push(op);
      return op;
    },
    /** The backend has it: reads from now on reflect it. */
    settle(op: Op<P>) {
      if (op.settled !== null) return;
      op.settled = ++tick;
      gc();
    },
    /** It never happened (the call failed): forget it. */
    drop(op: Op<P>) {
      ops = ops.filter((o) => o !== op);
    },
    readBegin(): number {
      const id = ++nextRead;
      reads.set(id, tick);
      return id;
    },
    readEnd(read: number) {
      reads.delete(read);
      gc();
    },
    /** Changes a read begun as `read` may miss, oldest first. */
    pending(read: number): Op<P>[] {
      const start = reads.get(read) ?? tick;
      return ops.filter((o) => o.settled === null || o.settled > start);
    },
    size(): number {
      return ops.length;
    },
  };
}
