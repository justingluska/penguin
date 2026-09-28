// Get to zero, the pure part: the "older than" choices, what they mean as a
// cutoff, and which conversations a run archives. Tested in node
// (tests/zero.test.ts); the dialog is GetToZero.tsx.
import type { ThreadRef, ThreadSummary } from "../../lib/types";

export type Age = "day" | "week" | "twoWeeks" | "month" | "threeMonths" | "all";

const DAY = 86_400_000;

export const AGES: { age: Age; label: string; short: string }[] = [
  { age: "day", label: "Older than a day", short: "1 day" },
  { age: "week", label: "Older than a week", short: "1 week" },
  { age: "twoWeeks", label: "Older than two weeks", short: "2 weeks" },
  { age: "month", label: "Older than a month", short: "1 month" },
  { age: "threeMonths", label: "Older than three months", short: "3 months" },
  { age: "all", label: "Everything", short: "Everything" },
];

/** Conversations whose latest message is before this (ms) are archived; null = all of them. */
export function cutoff(age: Age, now: number): number | null {
  switch (age) {
    case "day":
      return now - DAY;
    case "week":
      return now - 7 * DAY;
    case "twoWeeks":
      return now - 14 * DAY;
    case "month":
      return now - 30 * DAY;
    case "threeMonths":
      return now - 91 * DAY;
    case "all":
      return null;
  }
}

export interface Keep {
  unread: boolean;
  starred: boolean;
}

/** Of rows already older than the cutoff, the ones to archive (kept ones stay), deduplicated. */
export function targets(rows: readonly ThreadSummary[], keep: Keep): ThreadRef[] {
  const seen = new Set<string>();
  const out: ThreadRef[] = [];
  for (const t of rows) {
    if ((keep.unread && t.unread) || (keep.starred && t.starred)) continue;
    const k = t.accountId + "\u0000" + t.threadId;
    if (seen.has(k)) continue;
    seen.add(k);
    out.push({ accountId: t.accountId, threadId: t.threadId });
  }
  return out;
}

// ---------------------------------------------------------------------------
// "Cleared today": how many conversations you archived or trashed from the
// inbox today, on this Mac. Shown on the zero screen. A per-device nicety in
// localStorage, never synced.
// ---------------------------------------------------------------------------
const KEY = "penguin.clearedToday";

function today(now = Date.now()): string {
  const d = new Date(now);
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
}

export function clearedToday(now = Date.now()): number {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as { day?: string; n?: number } | null;
    return raw?.day === today(now) && typeof raw.n === "number" ? raw.n : 0;
  } catch {
    // No storage (private window) or a corrupt entry: nothing counted.
    return 0;
  }
}

/** Count `n` more conversations cleared (a negative n takes an undo back). */
export function noteCleared(n: number, now = Date.now()) {
  if (!n) return;
  const next = Math.max(0, clearedToday(now) + n);
  try {
    localStorage.setItem(KEY, JSON.stringify({ day: today(now), n: next }));
  } catch {
    // No storage: the zero screen just doesn't show the tally.
  }
}
