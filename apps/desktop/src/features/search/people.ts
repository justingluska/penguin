// People seen locally (search responses, thread participants). Feeds from:/to:
// autocomplete in search and recipient autocomplete in compose. In memory only.
import { useSyncExternalStore } from "react";
import { api } from "../../lib/api";
import type { Address, SearchResponse } from "../../lib/types";
import { learn } from "./spelling";

interface Person {
  address: Address;
  /** Rough weight: how often we've seen them. */
  seen: number;
}

const people = new Map<string, Person>();
let snapshot: Person[] = [];
const subs = new Set<() => void>();
let seeded = false;

function key(a: Address) {
  return a.email.trim().toLowerCase();
}

export function rememberPeople(list: Address[], weight = 1) {
  let changed = false;
  for (const a of list) {
    if (!a?.email) continue;
    const k = key(a);
    const cur = people.get(k);
    if (cur) {
      cur.seen += weight;
      if (!cur.address.name && a.name) cur.address = a;
    } else {
      people.set(k, { address: a, seen: weight });
    }
    changed = true;
  }
  if (changed) {
    snapshot = [...people.values()].sort((a, b) => b.seen - a.seen);
    subs.forEach((f) => f());
  }
}

export function rememberFromResponse(r: SearchResponse) {
  rememberPeople(r.people.map((p) => p.address), 3);
  rememberPeople(r.hits.map((h) => h.from));
  rememberPeople(r.attachments.map((a) => a.from));
}

/** Seed from the unified inbox once (a local read, no network). */
export function seedPeople(): void {
  if (seeded) return;
  seeded = true;
  api
    .listThreads({ view: { kind: "inbox" }, tab: null, accountId: null, limit: 300, before: null })
    .then((ts) => {
      // Opened before the first sync (or during onboarding): try again next time.
      if (ts.length === 0) seeded = false;
      rememberPeople(ts.flatMap((t) => t.participants));
      // "Did you mean" knows the words of recent mail before the first search.
      for (const t of ts) {
        learn(t.subject);
        learn(t.snippet);
      }
    })
    .catch(() => {
      // The list may be empty before the first sync; autocomplete fills in as
      // search responses arrive.
      seeded = false;
    });
}

export function usePeople(): Person[] {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => snapshot,
  );
}

/** Prefix match on any name word or the address, best-known first. */
export function matchPeople(list: Person[], q: string, exclude: Set<string> = new Set(), limit = 6): Address[] {
  const n = q.trim().toLowerCase();
  const out: Address[] = [];
  for (const p of list) {
    const k = key(p.address);
    if (exclude.has(k)) continue;
    const name = (p.address.name ?? "").toLowerCase();
    if (!n || k.startsWith(n) || name.startsWith(n) || name.split(/\s+/).some((w) => w.startsWith(n))) {
      out.push(p.address);
      if (out.length >= limit) break;
    }
  }
  return out;
}
