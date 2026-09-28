// Unread filter refresh merge (src/app/unreadFilter.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { keepReadHere } from "../src/app/unreadFilter.ts";

const row = (id: string, lastDate: number, unread: boolean) => ({ accountId: "a@x.example", threadId: id, lastDate, unread });
const ids = (rs: { threadId: string }[]) => rs.map((r) => r.threadId);
const none = () => false;

test("a thread read here stays in place, in date order", () => {
  const onScreen = [row("t5", 50, true), row("t4", 40, false), row("t3", 30, true)];
  const fresh = [row("t5", 50, true), row("t3", 30, true)];
  assert.deepEqual(ids(keepReadHere(fresh, onScreen, -Infinity, none)), ["t5", "t4", "t3"]);
});

test("rows that left while still unread, or are being removed, are not kept", () => {
  const onScreen = [row("t5", 50, true), row("t4", 40, false), row("t3", 30, true)];
  // t5 was archived elsewhere (still unread on screen); t4 is being trashed here.
  const fresh = [row("t3", 30, true)];
  const removed = (k: string) => k.endsWith("t4");
  assert.deepEqual(ids(keepReadHere(fresh, onScreen, -Infinity, removed)), ["t3"]);
});

test("only rows inside the refreshed window are merged", () => {
  const onScreen = [row("t5", 50, false), row("t2", 20, false)];
  const fresh = [row("t6", 60, true), row("t4", 40, true)];
  // The window ends at 40: t2 belongs to the paged tail the caller keeps.
  assert.deepEqual(ids(keepReadHere(fresh, onScreen, 40, none)), ["t6", "t5", "t4"]);
});

test("nothing to keep returns the fresh page itself", () => {
  const fresh = [row("t1", 10, true)];
  assert.equal(keepReadHere(fresh, [row("t1", 10, true)], -Infinity, none), fresh);
});
