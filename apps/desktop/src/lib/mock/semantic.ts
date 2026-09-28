// OWNER: ui-search agent.
//
// A stand-in for search by meaning in the mock backend: hand-made groups of
// related words play the part of the embedding model, so "rent going up"
// finds "Rent increase notice" and "extend the lease at the same rate"
// without sharing the words. Good enough to build and screenshot the UI
// (passages as snippets, results arriving while mail is still being
// indexed); the real ranking lives in penguin-core / penguin-semantic.
//
// Knobs: ?semantic=ready|indexing|off (URL) or localStorage
// penguin.mock.semantic. "indexing" starts at 42% and climbs while the page
// is open, so the "Getting smarter" line and late-arriving matches can be seen.

export type SemanticState = "ready" | "indexing" | "off";

/** Related words and phrases, one idea per group. */
const GROUPS: string[][] = [
  ["rent", "lease", "landlord", "apartment", "tenancy", "renewal", "renew", "renews", "unit", "housing", "flat"],
  ["going up", "increase", "raise", "higher", "hike", "rate", "change", "more expensive", "price rise"],
  ["bill", "bills", "invoice", "payment", "paid", "receipt", "pay", "cost", "charge", "spend", "money", "owe"],
  ["trip", "flight", "boarding", "travel", "vacation", "holiday", "departs", "seats", "airline", "fly", "plane"],
  ["plumber", "leak", "broken", "fix", "repair", "water heater", "service window", "maintenance"],
  ["investor", "investors", "seed", "term sheet", "data room", "pro-rata", "board seat", "funding", "fundraising", "vc", "round"],
  ["podcast", "episode", "recording", "sponsor", "show", "audio", "mic"],
  ["brand", "deck", "palette", "lockups", "logo", "design", "review"],
  ["contract", "signed", "countersigned", "signature", "agreement", "paperwork", "sign"],
  ["hiring", "recruit", "recruiting", "candidates", "hire", "headcount"],
  ["photos", "pictures", "pics", "sunrise", "uploaded", "album"],
  ["meeting", "agenda", "offsite", "sync", "retro", "schedule", "calendar", "invitation"],
  ["signups", "retention", "users", "growth", "cohort", "traction"],
  ["parking", "car", "spot", "garage"],
  ["keys", "move-in", "move in", "moving", "inspection", "damage"],
];

function readMode(): SemanticState {
  try {
    const p = new URLSearchParams(window.location.search).get("semantic");
    const v = p ?? localStorage.getItem("penguin.mock.semantic");
    if (v === "indexing" || v === "off" || v === "ready") return v;
  } catch {
    // No window/storage (tests): the default.
  }
  return "ready";
}

const mode = readMode();
const startedAt = Date.now();

/** The mock's semantic state right now (indexing climbs ~2% every 3 s, then it's ready). */
export function semanticState(): { semantic: SemanticState; semanticProgress: number | null } {
  if (mode !== "indexing") return { semantic: mode, semanticProgress: null };
  const p = Math.min(1, 0.42 + Math.floor((Date.now() - startedAt) / 3000) * 0.02);
  return p >= 1 ? { semantic: "ready", semanticProgress: null } : { semantic: "indexing", semanticProgress: p };
}

/** Is this message embedded yet? A stable pseudo-random share of mail, growing with progress. */
export function embedded(id: string): boolean {
  const { semantic, semanticProgress } = semanticState();
  if (semantic === "off") return false;
  if (semantic === "ready") return true;
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619);
  return ((h >>> 0) % 1000) / 1000 < (semanticProgress ?? 0);
}

const norm = (s: string) => ` ${s.toLowerCase().replace(/[^a-z0-9$-]+/g, " ").trim()} `;

/** The groups a text touches (a word or phrase of the group appears as a whole word). */
function groupsIn(text: string): Set<number> {
  const t = norm(text);
  const out = new Set<number>();
  GROUPS.forEach((g, i) => {
    if (g.some((w) => t.includes(` ${w} `))) out.add(i);
  });
  return out;
}

/** Sentences of a body (each at most 240 characters) with their groups, worked out once per message. */
const passageCache = new Map<string, Array<{ text: string; groups: Set<number> }>>();
function passages(subject: string, body: string): Array<{ text: string; groups: Set<number> }> {
  const key = `${subject}\u0000${body}`;
  const hit = passageCache.get(key);
  if (hit) return hit;
  const out = [subject];
  for (const s of body.split(/(?<=[.!?])\s+/)) {
    const t = s.trim();
    if (t) out.push(t.length > 240 ? `${t.slice(0, 239)}…` : t);
  }
  const list = out.map((text) => ({ text, groups: groupsIn(text) }));
  passageCache.set(key, list);
  return list;
}

/** The query's groups, worked out once per search rather than once per message. */
let lastQuery: { q: string; want: Set<number> } = { q: "", want: new Set() };

export interface MeaningMatch {
  passage: string;
  /** 0–1: how much of the query's meaning the passage covers. */
  strength: number;
}

/**
 * The passage of a message closest in meaning to the query words, when it
 * covers the query's ideas (all of them for one or two, two thirds beyond).
 */
export function meaningMatch(queryWords: string[], subject: string, body: string): MeaningMatch | null {
  const q = queryWords.join(" ");
  if (q !== lastQuery.q) lastQuery = { q, want: groupsIn(q) };
  const want = lastQuery.want;
  if (want.size === 0) return null;
  const need = want.size <= 2 ? want.size : Math.ceil((want.size * 2) / 3);
  let best: MeaningMatch | null = null;
  for (const p of passages(subject, body)) {
    let got = 0;
    for (const g of p.groups) if (want.has(g)) got++;
    if (got < need) continue;
    const strength = got / want.size;
    // The subject is a passage too, but a body sentence explains more.
    if (!best || strength > best.strength || (strength === best.strength && best.passage === subject)) best = { passage: p.text, strength };
  }
  return best;
}
