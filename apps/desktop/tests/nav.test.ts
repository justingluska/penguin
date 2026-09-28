// Back navigation (lib/ui.ts): a thread opened from a surface returns there.
import { test } from "node:test";
import assert from "node:assert/strict";
import { getUi, goBack, leaveMarkedUnread, openThread, setUi } from "../src/lib/ui.ts";

const row = { accountId: "a", threadId: "list-row" };
const hit = { accountId: "a", threadId: "search-hit" };
const reset = () => setUi({ selected: row, selectedByApp: false, threadOpen: false, overlay: null, navStack: [], overlayRestore: null });

test("back from a thread opened from search reopens search with its snapshot", () => {
  reset();
  setUi({ overlay: "search" });
  openThread(hit, "m1", { kind: "search", restore: { query: "from:ana" } });
  let s = getUi();
  assert.equal(s.threadOpen, true);
  assert.equal(s.overlay, null);
  assert.deepEqual(s.selected, hit);
  goBack();
  s = getUi();
  assert.equal(s.threadOpen, false);
  assert.equal(s.overlay, "search");
  assert.deepEqual(s.overlayRestore, { query: "from:ana" });
  assert.deepEqual(s.selected, row, "the list selection from before comes back");
  assert.equal(s.navStack.length, 0);
});

test("back without an origin goes to the list", () => {
  reset();
  openThread(hit);
  goBack();
  assert.equal(getUi().threadOpen, false);
  assert.equal(getUi().overlay, null);
});

test("⌘Enter (keep) leaves the overlay up and still returns to search", () => {
  reset();
  setUi({ overlay: "search" });
  openThread(hit, null, { kind: "search", restore: 1 }, true);
  assert.equal(getUi().overlay, "search");
  openThread({ accountId: "a", threadId: "second" }, null, { kind: "search", restore: 2 }, true);
  assert.equal(getUi().navStack.length, 1, "another result from the same search replaces the entry");
  setUi({ overlay: null }); // Esc closes search; the thread stays
  goBack();
  assert.equal(getUi().overlay, "search");
  assert.equal(getUi().overlayRestore, 2);
  assert.deepEqual(getUi().selected, row, "selection from before the first result");
});

test("leaving the thread view any other way forgets the origin", () => {
  reset();
  openThread(hit, null, { kind: "search", restore: 1 });
  setUi({ threadOpen: false }); // e.g. switching views
  assert.equal(getUi().navStack.length, 0);
  openThread(hit);
  goBack();
  assert.equal(getUi().overlay, null);
});

test("a thread opened through openThread is outside the list; any other selection is inside", () => {
  reset();
  assert.equal(getUi().selectedOutside, false);
  openThread(hit, null, { kind: "search", restore: 1 });
  assert.equal(getUi().selectedOutside, true);
  setUi({ contextPanel: false }); // unrelated changes keep it
  assert.equal(getUi().selectedOutside, true);
  goBack();
  assert.equal(getUi().selectedOutside, false, "back on the list selection");
  openThread(hit);
  setUi({ selected: row, selectedByApp: false }); // j/k, a click on a row
  assert.equal(getUi().selectedOutside, false);
});

test("marked unread in the full view: back to the list, cursor on it, not reopened", () => {
  reset();
  setUi({ threadOpen: true, selectedByApp: false }); // Enter on the list row
  leaveMarkedUnread([row]);
  const s = getUi();
  assert.equal(s.threadOpen, false);
  assert.deepEqual(s.selected, row);
  assert.equal(s.selectedByApp, true, "the reading pane shows it without marking it read");
});

test("marked unread in the full view opened from search: back to search", () => {
  reset();
  setUi({ overlay: "search" });
  openThread(hit, null, { kind: "search", restore: 1 });
  leaveMarkedUnread([hit]);
  assert.equal(getUi().threadOpen, false);
  assert.equal(getUi().overlay, "search");
  assert.deepEqual(getUi().selected, row);
  assert.equal(getUi().selectedByApp, false, "the earlier list selection keeps how it was placed");
});

test("marked unread in the reading pane, or another thread: nothing moves", () => {
  reset();
  leaveMarkedUnread([row]);
  assert.deepEqual(getUi().selected, row);
  assert.equal(getUi().selectedByApp, false);
  setUi({ threadOpen: true });
  leaveMarkedUnread([hit]);
  assert.equal(getUi().threadOpen, true);
});
