// The user's account order (Settings.accountOrder): dragged in the sidebar's
// Accounts section or Settings → Accounts, or moved with "Move up" / "Move
// down". Every list of accounts follows it: the store sorts meta.accounts
// here once (app/store.ts setAccounts), so the sidebar, the switcher, the
// Settings pages, ⌥1–⌥9 and compose's From menu all read the same order.
//
// Pure rules here (tests/accountOrder.test.ts).

/**
 * `accounts` in the saved order: listed ids first, in list order; accounts
 * not in the list keep their incoming order after them (a new account goes
 * last). Ids in the list that aren't accounts are ignored. Returns the same
 * array when nothing moves, so memoized readers don't re-render.
 */
export function orderAccounts<T extends { id: string }>(accounts: T[], order: string[]): T[] {
  if (order.length === 0 || accounts.length < 2) return accounts;
  const rank = new Map<string, number>();
  order.forEach((id, i) => {
    if (!rank.has(id)) rank.set(id, i);
  });
  const listed = accounts.filter((a) => rank.has(a.id)).sort((a, b) => rank.get(a.id)! - rank.get(b.id)!);
  const rest = accounts.filter((a) => !rank.has(a.id));
  const out = [...listed, ...rest];
  return out.every((a, i) => a === accounts[i]) ? accounts : out;
}

/**
 * Move `id` so it sits at `toIndex` of `visible` (counted without `id`
 * itself), where `visible` is the list the user sees: every account, or a
 * profile's accounts in the global order. Accounts that aren't visible keep
 * their places; the visible ones are re-dealt into the slots they held, so a
 * move inside a profile only reorders that profile's accounts relative to
 * each other. Returns the new full order of `all` (every account id).
 */
export function moveWithin(all: string[], visible: string[], id: string, toIndex: number): string[] {
  const shown = new Set(visible.filter((v) => all.includes(v)));
  if (!shown.has(id)) return all;
  const view = all.filter((v) => shown.has(v));
  const without = view.filter((v) => v !== id);
  const at = Math.max(0, Math.min(without.length, Math.round(toIndex)));
  const next = [...without.slice(0, at), id, ...without.slice(at)];
  let k = 0;
  return all.map((v) => (shown.has(v) ? next[k++] : v));
}

/** Move `id` one place up (-1) or down (+1) within `visible`; unchanged at either end. */
export function moveBy(all: string[], visible: string[], id: string, delta: -1 | 1): string[] {
  const view = all.filter((v) => visible.includes(v));
  const i = view.indexOf(id);
  if (i < 0 || i + delta < 0 || i + delta >= view.length) return all;
  return moveWithin(all, visible, id, i + delta);
}

/** Whether `id` can move up (-1) or down (+1) within `visible`. */
export function canMove(all: string[], visible: string[], id: string, delta: -1 | 1): boolean {
  return moveBy(all, visible, id, delta) !== all;
}

/**
 * Where a dragged row lands, from the pointer's y and the other rows' boxes
 * (in display order, the dragged row left out): the index among those rows
 * it goes before. Past a row's midpoint counts as after it.
 */
export function dropIndex(pointerY: number, rows: { top: number; height: number }[]): number {
  let i = 0;
  while (i < rows.length && pointerY > rows[i].top + rows[i].height / 2) i++;
  return i;
}

/** Same ids in the same order. */
export function sameOrder(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}
