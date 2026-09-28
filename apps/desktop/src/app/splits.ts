// Split Inbox, the pure part: which tabs the inbox has, which one a list
// query asks for, and moving between them. The splits are Settings.inboxSplits
// (a query each, in tab order); a conversation goes to the first split that
// matches, and Other holds the rest (penguin-core store_split.rs). The UI
// (features/split) and the store (app/store.ts) both read these helpers.
import type { InboxSplit, SplitCount, SplitFilter } from "../lib/types";

/** Other's id: never a valid split id (settings.rs allows [A-Za-z0-9_-] only). */
export const OTHER = "~other";

export interface SplitTab {
  id: string;
  name: string;
  /** null = Other. */
  query: string | null;
  hideWhenEmpty: boolean;
}

/** Every tab: the splits in order, then Other. */
export function splitTabs(splits: readonly InboxSplit[]): SplitTab[] {
  return [
    ...splits.map((s) => ({ id: s.id, name: s.name || s.query, query: s.query, hideWhenEmpty: s.hideWhenEmpty })),
    { id: OTHER, name: "Other", query: null, hideWhenEmpty: false },
  ];
}

/** The tab a remembered id means now: itself if it still exists, else the first. */
export function effectiveSplitId(splits: readonly InboxSplit[], id: string | null): string {
  if (id === OTHER || (id !== null && splits.some((s) => s.id === id))) return id;
  return splits[0]?.id ?? OTHER;
}

/** The list filter for a tab: its query, minus what the splits before it claim. */
export function splitFilterFor(splits: readonly InboxSplit[], id: string): SplitFilter {
  const i = splits.findIndex((s) => s.id === id);
  if (i < 0) return { include: null, exclude: splits.map((s) => s.query) };
  return { include: splits[i].query, exclude: splits.slice(0, i).map((s) => s.query) };
}

/**
 * The tabs shown: "hide when empty" ones leave while they have nothing, but
 * never the current one (you'd lose your place) and not before the counts
 * are known.
 */
export function visibleTabs(tabs: readonly SplitTab[], counts: Readonly<Record<string, SplitCount>> | null, current: string): SplitTab[] {
  return tabs.filter((t) => !t.hideWhenEmpty || t.id === current || !counts || (counts[t.id]?.total ?? 1) > 0);
}

/** Tab / ⇧Tab: the next or previous shown tab, wrapping around. */
export function cycleSplit(tabs: readonly SplitTab[], current: string, dir: 1 | -1): string {
  if (tabs.length === 0) return current;
  const i = tabs.findIndex((t) => t.id === current);
  if (i < 0) return tabs[0].id;
  return tabs[(i + dir + tabs.length) % tabs.length].id;
}

/** The next tab after `current` that still has something in it (for the zero screen), or null. */
export function nextWithMail(tabs: readonly SplitTab[], counts: Readonly<Record<string, SplitCount>> | null, current: string): SplitTab | null {
  if (!counts) return null;
  const i = tabs.findIndex((t) => t.id === current);
  for (let k = 1; k < tabs.length; k++) {
    const t = tabs[(i + k) % tabs.length];
    if ((counts[t.id]?.total ?? 0) > 0) return t;
  }
  return null;
}

/** A tab's count as shown: total conversations, like the inbox; hidden at 0, "999+" past 999 or past the cap. */
export function countLabel(c: SplitCount | undefined, more: boolean): string | null {
  if (!c || c.total <= 0) return null;
  if (c.total > 999) return "999+";
  return more ? `${c.total}+` : String(c.total);
}

/** A fresh split id from a name ("VIP" → "vip", "vip-2" when taken). */
export function newSplitId(name: string, taken: readonly string[]): string {
  const base =
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40) || "split";
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}
