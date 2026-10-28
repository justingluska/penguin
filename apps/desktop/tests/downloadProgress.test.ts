// Attachment download progress (features/thread/downloadBook.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { DownloadBook, downloadKey, fractionOf, progressText } from "../src/features/thread/downloadBook.ts";

const ev = (received: number, total: number | null, done = false) => ({ accountId: "a", messageId: "m", attachmentId: "x", received, total, done });
const key = downloadKey("a", "m", "x");

test("a click is busy before the first byte, then counts", () => {
  const b = new DownloadBook();
  assert.equal(b.progress(key), null);
  b.begin(key);
  assert.equal(b.progress(key), "waiting");
  b.apply(ev(512, 2048));
  assert.deepEqual(b.progress(key), { received: 512, total: 2048 });
  assert.equal(fractionOf(b.progress(key)), 0.25);
});

test("done holds the full count until the call returns, then clears", () => {
  const b = new DownloadBook();
  b.begin(key);
  b.apply(ev(100, 2048));
  b.apply(ev(2048, 2048, true));
  assert.deepEqual(b.progress(key), { received: 2048, total: 2048 });
  b.end(key);
  assert.equal(b.progress(key), null);
});

test("a download nobody here asked for (another window, a prefetch) shows and clears by events", () => {
  const b = new DownloadBook();
  b.apply(ev(0, 4096));
  assert.deepEqual(b.progress(key), { received: 0, total: 4096 });
  b.apply(ev(4096, 4096, true));
  assert.equal(b.progress(key), null);
});

test("a call that ends while events still say downloading isn't left spinning", () => {
  const b = new DownloadBook();
  b.begin(key);
  b.apply(ev(10, 100));
  b.end(key); // rejected; the backend's done event was lost
  assert.equal(b.progress(key), null);
});

test("a failed download keeps the last count until the call rejects", () => {
  const b = new DownloadBook();
  b.begin(key);
  b.apply(ev(300, 1000));
  b.apply(ev(0, 1000, true));
  assert.deepEqual(b.progress(key), { received: 300, total: 1000 });
  b.end(key);
  assert.equal(b.progress(key), null);
});

test("text: percent and sizes, bytes alone without a total, nothing while waiting", () => {
  assert.equal(progressText({ received: 1258291, total: 3565158 }), "35% · 1.2 MB of 3.4 MB");
  assert.equal(progressText({ received: 2048, total: null }), "2 KB");
  assert.equal(progressText({ received: 1258291, total: 3565158 }, true), "35% of 3.4 MB");
  assert.equal(progressText("waiting"), "");
  assert.equal(fractionOf("waiting"), null);
  assert.equal(fractionOf({ received: 5, total: null }), null);
});
