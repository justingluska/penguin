// Stable results: when a fresh answer arrives for the query already on
// screen (matches by meaning landing after the words, the index getting
// further while it builds, Back to a search), nothing above or at the
// selected row moves. Rows already shown keep their place and their group;
// new rows slide in below the selection, at the spot their rank gives them;
// a new row whose group sits above the selection waits until the selection
// moves up to it or the query changes. A different query is a fresh list.
//
// Why: most failed re-finding searches had the right message on screen and
// the person didn't see it (Mackenzie et al., WWW '19), and a row that moves
// just before Enter opens the wrong mail. docs/SEARCH-UX.md has the sources.

export interface Placed {
  id: string;
  group: string;
}

export interface Stabilized<T extends Placed> {
  items: T[];
  /** Rows that weren't shown before (they get the slide-in). */
  entered: Set<string>;
  /** Rows held back because their place is above the selection. */
  deferred: number;
}

/**
 * Merge `fresh` (the new answer, in rank order, grouped as it should be
 * shown) into `shown` (what's on screen for the same query). `anchorId` is
 * the selected row; `groups` is the order groups are shown in.
 */
export function stabilize<T extends Placed>(shown: T[], fresh: T[], anchorId: string | null, groups: readonly string[]): Stabilized<T> {
  const byId = new Map(fresh.map((x) => [x.id, x]));
  const shownIds = new Set(shown.map((x) => x.id));
  // The anchor, or the nearest row above it that is still there.
  let anchor: T | null = null;
  const at = anchorId === null ? -1 : shown.findIndex((x) => x.id === anchorId);
  for (let i = at; i >= 0; i--) {
    if (byId.has(shown[i].id)) {
      anchor = shown[i];
      break;
    }
  }
  if (!anchor) {
    // Nothing selected survives: the top of the list is the anchor.
    const first = shown.find((x) => byId.has(x.id));
    if (!first) return { items: fresh, entered: new Set(fresh.map((x) => x.id)), deferred: 0 };
    anchor = first;
  }
  const rank = (g: string) => {
    const r = groups.indexOf(g);
    return r === -1 ? groups.length : r;
  };
  const anchorRank = rank(anchor.group);
  const order = [...new Set([...shown.map((x) => x.group), ...fresh.map((x) => x.group)])].sort((a, b) => rank(a) - rank(b));

  const items: T[] = [];
  const entered = new Set<string>();
  let deferred = 0;
  for (const g of order) {
    // Shown rows keep their group and order, with fresh data.
    const list: T[] = shown.filter((x) => x.group === g && byId.has(x.id)).map((x) => ({ ...byId.get(x.id)!, group: g }));
    const newcomers = fresh.filter((x) => x.group === g && !shownIds.has(x.id));
    if (newcomers.length && rank(g) < anchorRank) {
      deferred += newcomers.length;
    } else if (newcomers.length) {
      const freshOrder = fresh.filter((x) => x.group === g).map((x) => x.id);
      const floor = g === anchor.group ? list.findIndex((x) => x.id === anchor!.id) + 1 : 0;
      for (const n of newcomers) {
        // After the nearest row that precedes it in the fresh ranking.
        const before = freshOrder.slice(0, freshOrder.indexOf(n.id));
        let pos = 0;
        for (let i = list.length - 1; i >= 0; i--) {
          if (before.includes(list[i].id)) {
            pos = i + 1;
            break;
          }
        }
        list.splice(Math.max(pos, floor), 0, n);
        entered.add(n.id);
      }
    }
    items.push(...list);
  }
  return { items, entered, deferred };
}
