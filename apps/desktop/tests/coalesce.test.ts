// mail-changed bursts → list re-reads (src/app/coalesce.ts), on a fake clock.
import { test } from "node:test";
import assert from "node:assert/strict";
import { coalesce, type Timers } from "../src/app/coalesce.ts";

function clock() {
  let now = 0;
  let id = 0;
  const due = new Map<number, { at: number; f: () => void }>();
  const timers: Timers = {
    setTimeout: (f, ms) => {
      due.set(++id, { at: now + ms, f });
      return id;
    },
    clearTimeout: (t) => void due.delete(t as number),
    now: () => now,
  };
  /** Advance to `t`, firing timers in order (and letting settled runs' callbacks go). */
  const advance = async (t: number) => {
    for (;;) {
      const next = [...due.entries()].filter(([, d]) => d.at <= t).sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      due.delete(next[0]);
      now = next[1].at;
      next[1].f();
      await flush();
    }
    now = t;
    await flush();
  };
  return { timers, advance };
}
const flush = () => new Promise<void>((r) => setImmediate(r));

/** A run that takes `ms` on the fake clock: resolved by the test via `finish`. */
function runs() {
  const started: number[] = [];
  const open: (() => void)[] = [];
  return {
    started,
    run: (now: () => number) => () => {
      started.push(now());
      return new Promise<void>((r) => open.push(r));
    },
    finishAll: async () => {
      while (open.length) open.shift()!();
      await flush();
    },
  };
}

test("one event after a quiet spell runs once, after the short delay", async () => {
  const c = clock();
  const r = runs();
  const q = coalesce(r.run(c.timers.now), { delay: 60, minGap: 250 }, c.timers);
  q.request();
  await c.advance(59);
  assert.deepEqual(r.started, []);
  await c.advance(60);
  assert.deepEqual(r.started, [60]);
});

test("a burst inside the delay is one run", async () => {
  const c = clock();
  const r = runs();
  const q = coalesce(r.run(c.timers.now), { delay: 60, minGap: 250 }, c.timers);
  q.request();
  await c.advance(20);
  q.request();
  await c.advance(40);
  q.request();
  await c.advance(200);
  await r.finishAll();
  await c.advance(1000);
  assert.deepEqual(r.started, [60]);
});

test("events during a run queue exactly one follow-up, not one each", async () => {
  const c = clock();
  const r = runs();
  const q = coalesce(r.run(c.timers.now), { delay: 60, minGap: 250 }, c.timers);
  q.request();
  await c.advance(60); // run 1 starts, still in flight
  q.request();
  q.request();
  q.request();
  await c.advance(500);
  assert.equal(r.started.length, 1, "nothing starts while one is in flight");
  await r.finishAll();
  await c.advance(1000);
  assert.equal(r.started.length, 2);
  await r.finishAll();
  await c.advance(3000);
  assert.equal(r.started.length, 2, "and nothing after that without new events");
});

test("a steady storm is paced to minGap, however fast the events", async () => {
  const c = clock();
  const r = runs();
  const q = coalesce(r.run(c.timers.now), { delay: 60, minGap: 250 }, c.timers);
  for (let t = 0; t < 2000; t += 50) {
    await c.advance(t);
    q.request();
    await r.finishAll(); // runs are quick
  }
  await c.advance(2500);
  await r.finishAll();
  // 40 events over 2 s → about 2000/250 runs, each at least minGap after the last.
  assert.ok(r.started.length <= 10 && r.started.length >= 7, `runs: ${r.started}`);
  for (let i = 1; i < r.started.length; i++) assert.ok(r.started[i] - r.started[i - 1] >= 250, `gap before run ${i}: ${r.started}`);
});

test("the last event of a storm is still followed by a run", async () => {
  const c = clock();
  const r = runs();
  const q = coalesce(r.run(c.timers.now), { delay: 60, minGap: 250 }, c.timers);
  q.request();
  await c.advance(60);
  await r.finishAll();
  q.request(); // lands 10 ms after the previous run started
  await c.advance(70);
  await c.advance(400);
  await r.finishAll();
  assert.deepEqual(r.started, [60, 310]);
});

test("dispose cancels a pending run and ignores later requests", async () => {
  const c = clock();
  const r = runs();
  const q = coalesce(r.run(c.timers.now), { delay: 60, minGap: 250 }, c.timers);
  q.request();
  q.dispose();
  q.request();
  await c.advance(1000);
  assert.deepEqual(r.started, []);
});
