// Context menus and dropdowns (components/menuModel.ts): which entries show,
// keyboard movement past separators and disabled items, and type-ahead.
import { test } from "node:test";
import assert from "node:assert/strict";
import { firstEnabled, gridStep, normalizeEntries, typeaheadMatch, type MenuEntry } from "../src/components/menuModel.ts";

const sep = { type: "separator" } as const;

test("falsy entries drop and separators collapse", () => {
  const out = normalizeEntries([sep, false, { label: "Open" }, sep, null, sep, { label: "Copy" }, sep, undefined]);
  assert.deepEqual(out.map((e) => (e.type === "separator" ? "-" : (e as { label: string }).label)), ["Open", "-", "Copy"]);
});

test("a header whose section emptied out goes too", () => {
  const out = normalizeEntries([{ label: "A" }, sep, { type: "header", label: "Profiles" }, false, sep, { label: "B" }]);
  assert.deepEqual(out.map((e) => e.type ?? "item"), ["item", "separator", "item"]);
  assert.deepEqual(normalizeEntries([{ type: "header", label: "Only" }]), []);
});

const items: MenuEntry[] = [
  { label: "Reply" },
  sep,
  { label: "Reply all", disabled: "Select one conversation" },
  { label: "Archive" },
  { label: "Snooze", disabled: true },
  { label: "Star" },
];

test("movement skips separators and disabled items, wrapping", () => {
  assert.equal(firstEnabled(items), 0);
  assert.equal(firstEnabled(items, 1, 1), 3);
  assert.equal(firstEnabled(items, 4, 1), 5);
  assert.equal(firstEnabled(items, 6 % items.length, 1), 0);
  assert.equal(firstEnabled(items, 2, -1), 0);
  assert.equal(firstEnabled([sep, { label: "x", disabled: true }]), -1);
});

test("type-ahead: a repeated letter cycles, a prefix holds, disabled never matches", () => {
  assert.equal(typeaheadMatch(items, -1, "r"), 0);
  assert.equal(typeaheadMatch(items, 0, "r"), 0); // "Reply all" is disabled: back to Reply
  assert.equal(typeaheadMatch(items, 0, "re"), 0);
  assert.equal(typeaheadMatch(items, 0, "s"), 5); // Snooze is disabled
  assert.equal(typeaheadMatch(items, -1, "zz"), -1);
  assert.equal(typeaheadMatch([{ label: null, text: "Blue" }], -1, "b"), 0);
});

test("grid movement (color pickers): 40 cells, 10 to a row", () => {
  const step = (i: number, key: string) => gridStep(i, key, 40, 10);
  assert.equal(step(-1, "ArrowDown"), 0); // nothing active yet: start at the first cell
  assert.equal(step(0, "ArrowRight"), 1);
  assert.equal(step(9, "ArrowRight"), 9); // row end: stay, no wrap to the next row
  assert.equal(step(11, "ArrowLeft"), 10);
  assert.equal(step(10, "ArrowLeft"), "left-edge"); // first column: a submenu closes
  assert.equal(step(3, "ArrowDown"), 13);
  assert.equal(step(33, "ArrowDown"), 33); // last row: stay
  assert.equal(step(13, "ArrowUp"), 3);
  assert.equal(step(3, "ArrowUp"), 3);
  assert.equal(step(24, "Home"), 20);
  assert.equal(step(24, "End"), 29);
  assert.equal(step(4, "Enter"), null); // not movement: the caller chooses
  assert.equal(step(4, "a"), null);
  // A short last row: End and → stop at the last cell.
  assert.equal(gridStep(20, "End", 23, 10), 22);
  assert.equal(gridStep(22, "ArrowRight", 23, 10), 22);
  assert.equal(gridStep(15, "ArrowDown", 23, 10), 15);
});
