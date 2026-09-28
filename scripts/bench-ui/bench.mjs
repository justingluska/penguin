// UI benchmarks: Penguin's production bundle with the mock backend, driven
// by headless Chromium through Playwright.
//
//   cd scripts/bench-ui && npm ci && node bench.mjs [--out file.json] [--quick]
//     [--threads 5000] [--port N] [--dist prebuilt-dir] [--startup-runs N] [--only startup,storm]
//
// Needs apps/desktop/node_modules (npm ci there) and Playwright's headless
// shell (npx playwright-core install chromium-headless-shell).
//
// What this measures is the web layer: React rendering, the virtualized
// list, keyboard handling, the sandboxed message iframe. The backend is the
// in-browser mock (src/lib/mock), which answers after a simulated 4 ms IPC hop
// and does not run SQLite; backend timings come from
// crates/penguin-core/examples/bench.rs. Chromium on Linux is not WKWebView on
// macOS (the real app), so treat these as the web layer's cost on this
// machine, not as the app's numbers on a Mac.
//
// Timings are taken inside the page: from the input event's timestamp
// (Event.timeStamp, set when the browser received it) to the frame after the
// DOM changed (requestAnimationFrame, then a macrotask, i.e. after paint).

import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, rmSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { cpus, totalmem, release, platform, arch, loadavg } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { chromium } from "playwright-core";
import { build, serve, stats } from "./common.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const desktop = join(root, "apps/desktop");

const argv = process.argv.slice(2);
const flag = (name) => argv.includes(name);
const opt = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const quick = flag("--quick");
const outPath = resolve(opt("--out", join(root, "target/bench/ui.json")));
const RUNS = { startup: Number(opt("--startup-runs", quick ? 3 : 10)), jk: quick ? 40 : 200, clicks: quick ? 10 : 40, searchReps: quick ? 1 : 3 };
// --only startup,storm: just those sections (for quicker A/B runs). Sections:
// startup, keyboard, click, search, scroll, storm.
const ONLY = opt("--only", null)?.split(",") ?? null;
const run = (section) => !ONLY || ONLY.includes(section);
const LIST_THREADS = Number(opt("--threads", "5000"));
const PORT = Number(opt("--port", "0")); // 0 = any free port
// A prebuilt mock bundle (VITE_MOCK=1 vite build --outDir DIR) instead of
// building this checkout: for before/after comparisons of two builds.
const DIST = opt("--dist", null);

function bundleSize(dir) {
  const sum = { js: 0, jsGzip: 0, css: 0, cssGzip: 0 };
  const walk = (d) => {
    for (const f of readdirSync(d)) {
      const p = join(d, f);
      if (statSync(p).isDirectory()) walk(p);
      else if (/\.(js|css)$/.test(f)) {
        const buf = readFileSync(p);
        const k = f.endsWith(".js") ? "js" : "css";
        sum[k] += buf.length;
        sum[k + "Gzip"] += gzipSync(buf, { level: 9 }).length;
      }
    }
  };
  walk(join(dir, "assets"));
  return Object.fromEntries(Object.entries(sum).map(([k, v]) => [k + "KiB", Math.round(v / 102.4) / 10]));
}

// ----- in-page instrumentation -----
// Installed before any app script runs.
function instrument() {
  const w = window;
  w.__penguinBench = true; // the mock backend marks each response (src/lib/mock/index.ts)
  const afterPaint = (cb) => requestAnimationFrame(() => setTimeout(cb, 0));
  w.__bench = { firstRow: null, firstRowPaint: null, samples: [], pending: null, mutations: [] };
  const mo = new MutationObserver(() => {
    if (w.__bench.firstRow === null && document.querySelector('[role="option"][data-thread]')) {
      w.__bench.firstRow = performance.now();
      afterPaint(() => (w.__bench.firstRowPaint = performance.now()));
    }
  });
  mo.observe(document, { childList: true, subtree: true });
  // Selection moves: keydown/mousedown → aria-selected flips → next paint.
  addEventListener("keydown", (e) => (w.__bench.pending = { t0: e.timeStamp, key: e.key }), true);
  addEventListener("mousedown", (e) => (w.__bench.pending = { t0: e.timeStamp, key: "click" }), true);
  const sel = new MutationObserver((recs) => {
    const p = w.__bench.pending;
    if (!p || p.selDone) return;
    if (recs.some((r) => r.attributeName === "aria-selected" && r.target.getAttribute("aria-selected") === "true" && r.target.matches('[role="option"][data-thread]'))) {
      p.selDone = true;
      const t1 = performance.now();
      afterPaint(() => {
        p.selPaint = performance.now() - p.t0;
        p.selDom = t1 - p.t0;
      });
      watchPane(p);
    }
  });
  // Reading pane: the selected conversation's subject is shown (checked now,
  // then once per frame), then every message frame in it has loaded since
  // the input, then one paint.
  const watchPane = (p) => {
    const check = () => {
      const want = document.querySelector('[role="option"][aria-selected="true"] .subj')?.textContent?.trim();
      const h = document.querySelector(".preview-body .subject")?.textContent?.trim();
      if (p.paneSubject === undefined && h && want && h === want) p.paneSubject = performance.now() - p.t0;
      const frames = [...document.querySelectorAll(".preview-body iframe.mb-frame")];
      if (p.paneSubject === undefined || !frames.length || !frames.every((f) => (w.__bench.frameLoads.get(f) ?? -1) >= p.t0)) return false;
      afterPaint(() => (p.paneFull = performance.now() - p.t0));
      return true;
    };
    if (check()) return;
    const start = performance.now();
    const loop = () => {
      if (p !== w.__bench.pending || check()) return;
      if (performance.now() - start > 5000) p.paneTimeout = true;
      else requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  };
  addEventListener("DOMContentLoaded", () => sel.observe(document.body, { attributes: true, subtree: true, attributeFilter: ["aria-selected"] }));
  // Generic "settled" tracking for search: every DOM mutation's time.
  const any = new MutationObserver((recs) => {
    const t = performance.now();
    const inResults = recs.some((r) => (r.target.nodeType === 1 ? r.target : r.target.parentElement)?.closest?.(".sx-results"));
    w.__bench.mutations.push({ t, inResults });
  });
  addEventListener("DOMContentLoaded", () => any.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true }));
  // Message frames (srcdoc iframes): when each last finished loading.
  w.__bench.frameLoads = new WeakMap();
  // An iframe's load event doesn't propagate to the parent window, so hook
  // each frame as it appears (MutationObserver callbacks run before the
  // srcdoc navigation can finish).
  const hooked = new WeakSet();
  const hook = () =>
    document.querySelectorAll("iframe").forEach((f) => {
      if (hooked.has(f)) return;
      hooked.add(f);
      f.addEventListener("load", () => w.__bench.frameLoads.set(f, performance.now()));
    });
  addEventListener("DOMContentLoaded", () => new MutationObserver(hook).observe(document.body, { childList: true, subtree: true }));
  w.__bench.longTasks = [];
  try {
    new PerformanceObserver((l) => l.getEntries().forEach((e) => w.__bench.longTasks.push(e.duration))).observe({ type: "longtask", buffered: true });
  } catch {}
}

async function newPage(browser, url) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1, reducedMotion: "reduce" });
  const page = await ctx.newPage();
  await page.addInitScript(instrument);
  await page.goto(url);
  await page.waitForFunction(() => window.__bench.firstRowPaint !== null, null, { timeout: 30_000 });
  return { ctx, page };
}

async function startup(browser, base) {
  const rows = { fcp: [], dcl: [], firstRow: [], firstRowPaint: [], heapMiB: [], longTasksMs: [], launchJsKiB: [], launchCssKiB: [] };
  for (let i = 0; i < RUNS.startup; i++) {
    const { ctx, page } = await newPage(browser, base);
    const m = await page.evaluate(() => {
      const nav = performance.getEntriesByType("navigation")[0];
      const fcp = performance.getEntriesByName("first-contentful-paint")[0];
      return {
        fcp: fcp?.startTime ?? null,
        dcl: nav.domContentLoadedEventEnd,
        firstRow: window.__bench.firstRow,
        firstRowPaint: window.__bench.firstRowPaint,
        heap: performance.memory?.usedJSHeapSize ?? null,
        longTasks: window.__bench.longTasks.reduce((a, b) => a + b, 0),
        // Code the page fetched before its first rows painted (what launch
        // parses and runs), leaving out the mock backend's own chunk, which
        // the real app doesn't have.
        launch: performance
          .getEntriesByType("resource")
          .filter((e) => e.startTime < window.__bench.firstRowPaint && !/\/mock-[^/]*\.js$/.test(e.name))
          .reduce((a, e) => {
            if (/\.js$/.test(e.name)) a.js += e.decodedBodySize;
            if (/\.css$/.test(e.name)) a.css += e.decodedBodySize;
            return a;
          }, { js: 0, css: 0 }),
      };
    });
    rows.fcp.push(m.fcp);
    rows.dcl.push(m.dcl);
    rows.firstRow.push(m.firstRow);
    rows.firstRowPaint.push(m.firstRowPaint);
    if (m.heap) rows.heapMiB.push(m.heap / 1048576);
    rows.longTasksMs.push(m.longTasks);
    rows.launchJsKiB.push(m.launch.js / 1024);
    rows.launchCssKiB.push(m.launch.css / 1024);
    await ctx.close();
  }
  return Object.fromEntries(Object.entries(rows).map(([k, v]) => [k, stats(v)]));
}

const paneDone = () => window.__bench.pending && (window.__bench.pending.paneFull !== undefined || window.__bench.pending.paneTimeout);

/** j/k: keydown → selected row painted, and → reading pane showing that thread. */
async function keyboardNav(page) {
  await page.locator('[role="option"][data-thread]').first().click();
  await page.waitForTimeout(300);
  const sel = [], selDom = [], subj = [], pane = [];
  let missed = 0;
  for (let i = 0; i < RUNS.jk; i++) {
    // Mostly down, with some back-and-forth like real triage.
    const key = i % 10 < 7 ? "j" : "k";
    await page.evaluate(() => (window.__bench.pending = null));
    await page.keyboard.press(key);
    await page.waitForFunction(paneDone, null, { timeout: 10_000 });
    const p = await page.evaluate(() => window.__bench.pending);
    sel.push(p.selPaint);
    selDom.push(p.selDom);
    if (p.paneTimeout) missed++;
    else {
      subj.push(p.paneSubject);
      pane.push(p.paneFull);
    }
    await page.waitForTimeout(40);
  }
  return { selectionDomMs: stats(selDom), selectionPaintMs: stats(sel), paneSubjectMs: stats(subj), readingPaneMs: stats(pane), timedOut: missed };
}

/** Clicking a row further down the list: mousedown → pane rendered. */
async function clickOpen(page) {
  const subj = [], out = [];
  let missed = 0;
  for (let i = 0; i < RUNS.clicks; i++) {
    // Never the selected row: clicking it changes nothing, so nothing to time.
    const rows = page.locator('[role="option"][data-thread]:not([aria-selected="true"])');
    const n = await rows.count();
    const target = rows.nth((i * 7 + 5) % n);
    await target.scrollIntoViewIfNeeded();
    await page.evaluate(() => (window.__bench.pending = null));
    await target.click();
    await page.waitForFunction(paneDone, null, { timeout: 10_000 });
    const p = await page.evaluate(() => window.__bench.pending);
    if (p.paneTimeout) missed++;
    else {
      subj.push(p.paneSubject);
      out.push(p.paneFull);
    }
    await page.waitForTimeout(60);
  }
  return { paneSubjectMs: stats(subj), readingPaneMs: stats(out), timedOut: missed };
}

/** Search as you type: each keystroke → the results panel's last DOM change → paint. */
async function searchTyping(page) {
  const queries = ["invoice", "from:priya", "quarterly report", "has:pdf lease"];
  const results = [], settled = [];
  for (let rep = 0; rep < RUNS.searchReps; rep++) {
    for (const q of queries) {
      await page.keyboard.press("Escape");
      await page.waitForTimeout(150);
      await page.keyboard.press("/");
      await page.waitForSelector(".search.panel input, .search.panel [contenteditable]", { timeout: 5000 });
      await page.waitForTimeout(250);
      for (const ch of q) {
        await page.evaluate(() => {
          window.__bench.mutations = [];
          window.__bench.lastKey = null;
          addEventListener("keydown", (e) => (window.__bench.lastKey = e.timeStamp), { capture: true, once: true });
        });
        await page.keyboard.type(ch);
        // Typing cadence of a fast typist (~100 ms per key); anything the UI
        // does in response lands inside this window.
        await page.waitForTimeout(160);
        const t = await page.evaluate(async () => {
          const t0 = window.__bench.lastKey;
          const muts = window.__bench.mutations.filter((m) => m.t >= t0);
          // The first search response after the key, then the first change
          // to the results list after it. A response identical to what is
          // shown changes nothing: then the response time is the answer.
          const resp = performance.getEntriesByName("penguin:ipc:search").find((e) => e.startTime >= t0);
          performance.clearMarks();
          if (!resp) return { results: null, settled: null };
          const shown = muts.find((m) => m.inResults && m.t >= resp.startTime);
          return {
            results: (shown ? shown.t : resp.startTime) - t0,
            settled: muts.length ? Math.max(...muts.map((m) => m.t)) - t0 : null,
          };
        });
        if (t.results !== null) results.push(t.results);
        if (t.settled !== null) settled.push(t.settled);
      }
    }
  }
  await page.keyboard.press("Escape");
  // DOM-change times (no paint wait): keystroke → the search response rendered
  // into the results list, and → the last change anywhere in the page (hints,
  // Ask card) before the next key. Facet counts wait 280 ms by design and
  // mostly fall outside the window.
  return { keystrokeToResultsMs: stats(results), keystrokeToSettledMs: stats(settled) };
}

/** Scroll the list continuously (≈3,600 px/s, a fast flick) and record frame intervals. */
async function scrollFrames(page) {
  await page.keyboard.press("Escape");
  const r = await page.evaluate(async () => {
    const el = document.querySelector('.list-scroll[role="listbox"]');
    el.scrollTop = 0;
    await new Promise((r) => setTimeout(r, 300));
    const frames = [];
    const lt0 = window.__bench.longTasks.length;
    let last = performance.now();
    const end = last + 5000;
    await new Promise((done) => {
      const step = (now) => {
        frames.push(now - last);
        last = now;
        el.scrollTop += 60;
        if (now < end) requestAnimationFrame(step);
        else done();
      };
      requestAnimationFrame(step);
    });
    const rows = document.querySelectorAll('[role="option"][data-thread]').length;
    return { frames: frames.slice(1), scrolledPx: el.scrollTop, domRows: rows, longTasks: window.__bench.longTasks.slice(lt0) };
  });
  const over = (ms) => r.frames.filter((f) => f > ms).length;
  return {
    frameMs: stats(r.frames),
    frames: r.frames.length,
    framesOver20ms: over(20),
    framesOver33ms: over(33.4),
    scrolledPx: r.scrolledPx,
    rowsInDom: r.domRows,
    longTasks: r.longTasks.length,
  };
}

/**
 * A backfill's event storm: mail-changed every 100 ms for 8 s (a fast IMAP
 * backfill commits about that often) while the list holds ~500 rows and you
 * press j every 200 ms. Counts the list re-reads it causes (IPC calls and
 * rows asked for) and times j → selection painted during it.
 */
async function mailChangedStorm(page) {
  await page.keyboard.press("Escape");
  // Page the list to ~500 rows, as after scrolling a while.
  for (let i = 0; i < 12; i++) {
    await page.evaluate(() => {
      const el = document.querySelector('.list-scroll[role="listbox"]');
      el.scrollTop = el.scrollHeight;
    });
    await page.waitForTimeout(120);
  }
  await page.evaluate(() => (document.querySelector('.list-scroll[role="listbox"]').scrollTop = 0));
  await page.waitForTimeout(300);
  await page.locator('[role="option"][data-thread]').first().click();
  await page.waitForTimeout(300);
  await page.evaluate(() => {
    performance.clearMarks();
    window.__bench.stormLt = window.__bench.longTasks.length;
    const acc = document.querySelector('[role="option"][data-thread]').dataset.account;
    let n = 0;
    window.__bench.storm = setInterval(() => {
      window.__penguinMock.emit("penguin://mail-changed", { accountId: acc, threadIds: [`storm-${n++}`] });
    }, 100);
    window.__bench.stormEvents = () => n;
  });
  const sel = [];
  const t0 = Date.now();
  while (Date.now() - t0 < 8000) {
    await page.evaluate(() => (window.__bench.pending = null));
    await page.keyboard.press("j");
    await page.waitForTimeout(200);
    const p = await page.evaluate(() => window.__bench.pending);
    if (p?.selPaint !== undefined) sel.push(p.selPaint);
  }
  const r = await page.evaluate(() => {
    clearInterval(window.__bench.storm);
    const calls = performance.getEntriesByName("penguin:ipc-send:list_threads", "mark");
    return {
      events: window.__bench.stormEvents(),
      listCalls: calls.length,
      rowsAsked: calls.reduce((a, m) => a + (m.detail?.query?.limit ?? m.detail?.limit ?? 0), 0),
      longTasks: window.__bench.longTasks.length - window.__bench.stormLt,
    };
  });
  await page.waitForTimeout(500);
  return { ...r, selectionPaintMs: stats(sel) };
}

/** Same fields as the backend's contention(): load, CPU pressure (Linux PSI), available memory. */
function contention() {
  const read = (f) => {
    try {
      return readFileSync(f, "utf8");
    } catch {
      return "";
    }
  };
  const psi = read("/proc/pressure/cpu").match(/^some .*avg60=([\d.]+)/m);
  const avail = read("/proc/meminfo").match(/^MemAvailable:\s+(\d+)/m);
  return {
    loadAvg1m: Math.round(loadavg()[0] * 100) / 100,
    cpuPressureSomeAvg60Pct: psi ? Number(psi[1]) : null,
    memAvailableGiB: avail ? Math.round((Number(avail[1]) / 1048576) * 10) / 10 : null,
  };
}

function gitCommit() {
  try {
    return execFileSync("git", ["rev-parse", "--short=12", "HEAD"], { cwd: root }).toString().trim();
  } catch {
    return null;
  }
}

async function main() {
  const dist = DIST ? resolve(DIST) : build(desktop);
  const server = await serve(dist, PORT);
  const base = `http://127.0.0.1:${server.address().port}/`;
  const browser = await chromium.launch({ headless: true });
  const result = {
    schema: 1,
    kind: "ui",
    date: new Date().toISOString(),
    git: { commit: gitCommit() },
    machine: { os: `${platform()} ${release()}`, arch: arch(), cpu: cpus()[0]?.model, logicalCores: cpus().length, ramGiB: Math.round((totalmem() / 1073741824) * 10) / 10 },
    contention: { start: contention() },
    browser: `Chromium ${browser.version()} (headless shell, Playwright ${JSON.parse(readFileSync(join(here, "node_modules/playwright-core/package.json"), "utf8")).version})`,
    viewport: "1440×900 @1x",
    mockThreads: LIST_THREADS,
    quick,
  };
  try {
    result.bundle = bundleSize(dist);
    console.error("startup…");
    if (run("startup")) result.startup = await startup(browser, base);
    if (!ONLY || ONLY.some((x) => x !== "startup")) {
      const { ctx, page } = await newPage(browser, `${base}?mockThreads=${LIST_THREADS}`);
      if (run("keyboard")) {
        console.error("j/k…");
        result.keyboard = await keyboardNav(page);
      }
      if (run("click")) {
        console.error("click to open…");
        result.clickOpen = await clickOpen(page);
      }
      if (run("search")) {
        console.error("search…");
        result.search = await searchTyping(page);
      }
      if (run("scroll")) {
        console.error("scroll…");
        result.scroll = await scrollFrames(page);
      }
      if (run("storm")) {
        console.error("mail-changed storm…");
        result.storm = await mailChangedStorm(page);
      }
      await ctx.close();
    }
  } finally {
    await browser.close();
    server.close();
    if (!DIST) rmSync(dist, { recursive: true, force: true });
  }
  result.contention.end = contention();
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ startup: result.startup, keyboard: result.keyboard, clickOpen: result.clickOpen, search: result.search, scroll: result.scroll, storm: result.storm }));
  console.error(`wrote ${outPath}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
