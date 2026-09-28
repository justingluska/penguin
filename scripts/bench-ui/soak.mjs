// Long-session soak: scripted use in rounds (triage with j/k, star, archive;
// open a thread and back; three searches; a reply opened and discarded;
// switching views; scrolling the list), and after each round a forced GC and
// the JS heap, DOM nodes, documents and event listeners. A leak shows as a
// number that keeps climbing; caches (the 60-thread LRU, avatars, compiled
// code) fill and then level off.
//
//   cd scripts/bench-ui && node soak.mjs [--rounds 15] [--dist prebuilt-dir] [--port N] [--snapshot]
//
// --snapshot also diffs heap snapshots (round 3 → end) by constructor.

import { rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { build, serve } from "./common.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const desktop = resolve(here, "../../apps/desktop");
const argv = process.argv.slice(2);
const opt = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const DIST = opt("--dist", null);
const ROUNDS = Number(opt("--rounds", "15"));
const PORT = Number(opt("--port", "0"));
const SNAPSHOT = argv.includes("--snapshot");

async function main() {
  const dist = DIST ? resolve(DIST) : build(desktop);
  const server = await serve(dist, PORT);
  const browser = await chromium.launch({ headless: true });
  try {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    page.on("pageerror", (e) => console.log("page error:", e.message));
    await page.goto(`http://127.0.0.1:${server.address().port}/?mockThreads=5000`);
    await page.waitForSelector('[role="option"][data-thread]');
    await page.waitForTimeout(1500);
    const cdp = await ctx.newCDPSession(page);
    await cdp.send("HeapProfiler.enable");
    const rows = [];
    const sample = async (label) => {
      await cdp.send("HeapProfiler.collectGarbage");
      await cdp.send("HeapProfiler.collectGarbage");
      const { usedSize } = await cdp.send("Runtime.getHeapUsage");
      const c = await cdp.send("Memory.getDOMCounters");
      const row = { label, heapMiB: usedSize / 1048576, nodes: c.nodes, documents: c.documents, listeners: c.jsEventListeners };
      rows.push(row);
      console.log(`${label.padEnd(9)} heap ${row.heapMiB.toFixed(2)} MiB  DOM nodes ${row.nodes}  documents ${row.documents}  listeners ${row.listeners}`);
    };
    const snapshot = async () => {
      const chunks = [];
      const onChunk = (m) => chunks.push(m.chunk);
      cdp.on("HeapProfiler.addHeapSnapshotChunk", onChunk);
      await cdp.send("HeapProfiler.takeHeapSnapshot", { reportProgress: false });
      cdp.off("HeapProfiler.addHeapSnapshotChunk", onChunk);
      const snap = JSON.parse(chunks.join(""));
      const f = snap.snapshot.meta.node_fields;
      const types = snap.snapshot.meta.node_types[0];
      const [W, iT, iN, iS] = [f.length, f.indexOf("type"), f.indexOf("name"), f.indexOf("self_size")];
      const agg = new Map();
      for (let i = 0; i < snap.nodes.length; i += W) {
        const t = types[snap.nodes[i + iT]];
        const name = t === "object" || t === "closure" || t === "native" ? snap.strings[snap.nodes[i + iN]] : `(${t})`;
        const a = agg.get(name) ?? { n: 0, size: 0 };
        a.n++;
        a.size += snap.nodes[i + iS];
        agg.set(name, a);
      }
      return agg;
    };
    await page.locator('[role="option"][data-thread]').first().click();
    await page.waitForTimeout(500);
    await sample("start");
    let early = null;
    for (let r = 1; r <= ROUNDS; r++) {
      for (let i = 0; i < 40; i++) {
        await page.keyboard.press(i % 8 === 7 ? "k" : "j");
        await page.waitForTimeout(40);
        if (i % 13 === 5) await page.keyboard.press("s");
        if (i % 17 === 9) {
          await page.keyboard.press("e");
          await page.waitForTimeout(80);
        }
      }
      await page.keyboard.press("Enter");
      await page.waitForTimeout(250);
      await page.keyboard.press("j");
      await page.waitForTimeout(150);
      await page.keyboard.press("Escape");
      await page.waitForTimeout(250);
      for (const q of ["invoice", "from:priya", "quarterly report"]) {
        await page.keyboard.press("/");
        await page.waitForTimeout(200);
        await page.keyboard.type(q, { delay: 60 });
        await page.waitForTimeout(300);
        await page.keyboard.press("Escape");
        await page.waitForTimeout(80);
        if (await page.locator(".search.panel").count()) {
          await page.keyboard.press("Escape");
          await page.waitForTimeout(150);
        }
      }
      await page.keyboard.press("r");
      await page.waitForTimeout(500);
      await page.keyboard.type("hello", { delay: 30 });
      await page.keyboard.press("Escape");
      await page.waitForTimeout(300);
      if (await page.locator(".composer").count()) {
        await page.keyboard.press("Escape");
        await page.waitForTimeout(300);
      }
      for (const k of ["g", "s"]) await page.keyboard.press(k);
      await page.waitForTimeout(300);
      for (const k of ["g", "i"]) await page.keyboard.press(k);
      await page.waitForTimeout(300);
      await page.evaluate(async () => {
        const el = document.querySelector(".list-scroll");
        if (!el) return;
        for (let i = 0; i < 60; i++) {
          el.scrollTop += 120;
          await new Promise((res) => requestAnimationFrame(res));
        }
        el.scrollTop = 0;
      });
      await page.waitForTimeout(300);
      await page.locator('[role="option"][data-thread]').first().click().catch(() => {});
      await sample(`round ${r}`);
      if (SNAPSHOT && r === 3) early = await snapshot();
    }
    const last = rows[rows.length - 1];
    const mid = rows[Math.min(3, rows.length - 1)];
    console.log(`heap ${rows[0].heapMiB.toFixed(2)} → ${last.heapMiB.toFixed(2)} MiB over ${ROUNDS} rounds (${(last.heapMiB - mid.heapMiB).toFixed(2)} MiB after round 3)`);
    if (SNAPSHOT && early) {
      const late = await snapshot();
      const diff = [...late].map(([k, v]) => [k, v.n - (early.get(k)?.n ?? 0), v.size - (early.get(k)?.size ?? 0)]).sort((a, b) => b[2] - a[2]);
      console.log("growth since round 3 by constructor (count, bytes):");
      for (const [k, dn, ds] of diff.slice(0, 20)) console.log(String(dn).padStart(8), String(ds).padStart(10), k.slice(0, 90));
    }
    await ctx.close();
  } finally {
    await browser.close();
    server.close();
    if (!DIST) rmSync(dist, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
