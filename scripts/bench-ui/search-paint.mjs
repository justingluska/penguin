// Search input-to-paint: Penguin's production bundle with the mock backend,
// typed into by headless Chromium through Playwright, one key at a time.
//
//   cd scripts/bench-ui && npm ci && node search-paint.mjs [--reps 5] [--out file.json] [--port 1495]
//                                                          [--shots dir]   (screenshots of the key states)
//
// Needs apps/desktop/node_modules and Playwright's headless shell, like bench.mjs.
//
// Two numbers per keystroke, both measured inside the page from the keydown's
// Event.timeStamp to the frame after the DOM changed (requestAnimationFrame,
// then a macrotask, i.e. after paint):
//   echo    → the typed character painted in the box (the "did it hear me" frame)
//   results → the search response for that key painted in the results list
// The mock backend answers after a simulated 4 ms IPC hop without SQLite, so
// this is the web layer's cost; add crates/penguin-core's bench for the rest.

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const desktop = join(root, "apps/desktop");
const argv = process.argv.slice(2);
const opt = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const REPS = Number(opt("--reps", "5"));
const PORT = Number(opt("--port", "1495"));
const outPath = opt("--out", null);
const shotsDir = opt("--shots", null);

/** What people type: words, a name, plain English, operators, a question. */
const QUERIES = ["invoice", "lease renewal", "from mike last week pdf", "has:pdf lease", "quarterly report", "when did i last email priya"];

function stats(v) {
  if (!v.length) return { n: 0 };
  const s = [...v].sort((a, b) => a - b);
  const pct = (p) => s[Math.round((s.length - 1) * p)];
  const r = (x) => Math.round(x * 10) / 10;
  // Superhuman buckets latency by share of events under 50 ms (fast) and 100 ms (ok): docs/SEARCH-UX.md.
  const under = (ms) => r((100 * s.filter((x) => x < ms).length) / s.length);
  return { n: s.length, p50: r(pct(0.5)), p95: r(pct(0.95)), max: r(s[s.length - 1]), mean: r(s.reduce((a, b) => a + b, 0) / s.length), under50Pct: under(50), under100Pct: under(100) };
}

function build() {
  const vite = join(desktop, "node_modules/.bin/vite");
  if (!existsSync(vite)) throw new Error("apps/desktop/node_modules is missing: run `npm ci` in apps/desktop first.");
  const out = mkdtempSync(join(tmpdir(), "penguin-search-paint-"));
  const r = spawnSync(vite, ["build", "--outDir", out, "--emptyOutDir", "--logLevel", "warn"], {
    cwd: desktop,
    env: { ...process.env, VITE_MOCK: "1" },
    stdio: ["ignore", "inherit", "inherit"],
  });
  if (r.status !== 0) throw new Error("vite build failed");
  return out;
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".woff2": "font/woff2", ".woff": "font/woff", ".json": "application/json" };
function serve(dir) {
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    let p = join(dir, decodeURIComponent(url.pathname));
    if (!p.startsWith(dir) || !existsSync(p) || statSync(p).isDirectory()) p = join(dir, "index.html");
    res.writeHead(200, { "content-type": TYPES[extname(p)] ?? "application/octet-stream", "cache-control": "no-store" });
    res.end(readFileSync(p));
  });
  return new Promise((ok, fail) => {
    server.once("error", fail);
    server.listen(PORT, "127.0.0.1", () => ok(server));
  });
}

// Installed before any app script runs.
function instrument() {
  window.__penguinBench = true; // the mock marks each response (src/lib/mock/index.ts)
  const afterPaint = (cb) => requestAnimationFrame(() => setTimeout(cb, 0));
  const b = (window.__sp = { key: null, echo: null, resultsDom: [], results: [] });
  addEventListener(
    "keydown",
    (e) => {
      if (e.key.length !== 1) return;
      b.key = e.timeStamp;
      b.echo = null;
      b.resultsDom = [];
      b.results = [];
    },
    true,
  );
  addEventListener(
    "input",
    () => {
      const t0 = b.key;
      if (t0 !== null) afterPaint(() => b.key === t0 && (b.echo = performance.now() - t0));
    },
    true,
  );
  const mo = new MutationObserver((recs) => {
    const t0 = b.key;
    if (t0 === null) return;
    if (!recs.some((r) => (r.target.nodeType === 1 ? r.target : r.target.parentElement)?.closest?.(".sx-results"))) return;
    const t = performance.now();
    b.resultsDom.push(t);
    afterPaint(() => b.key === t0 && b.results.push({ dom: t, paint: performance.now() }));
  });
  addEventListener("DOMContentLoaded", () => mo.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true }));
}

async function openSearch(page) {
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await page.waitForTimeout(120);
  await page.keyboard.press("/");
  await page.waitForSelector(".search.panel input", { timeout: 5000 });
  await page.keyboard.press("Meta+a");
  await page.keyboard.press("Backspace");
  await page.waitForTimeout(200);
}

async function measure(page) {
  const echo = [];
  const results = [];
  let missed = 0;
  /** key → search sent (render, effect), sent → answered (IPC hop + mock), answer → DOM, DOM → paint. */
  const parts = [[], [], [], []];
  for (let rep = 0; rep < REPS; rep++) {
    for (const q of QUERIES) {
      await openSearch(page);
      for (const ch of q) {
        await page.keyboard.type(ch);
        // A fast typist's cadence; everything the UI does for this key lands inside it.
        await page.waitForTimeout(160);
        const t = await page.evaluate(() => {
          const b = window.__sp;
          // The overlay's own search (limit 81), not facet counts, wider searches or spelling checks.
          const resp = performance
            .getEntriesByName("penguin:ipc:search")
            .find((e) => e.startTime >= b.key && (e.detail?.request?.limit ?? 81) === 81);
          const sent = performance
            .getEntriesByName("penguin:ipc-send:search")
            .find((e) => e.startTime >= b.key && (e.detail?.request?.limit ?? 81) === 81);
          performance.clearMarks();
          const painted = resp ? b.results.find((r) => r.dom >= resp.startTime) : null;
          return {
            echo: b.echo,
            results: painted ? painted.paint - b.key : null,
            resp: !!resp,
            parts:
              painted && sent && sent.startTime <= resp.startTime
                ? [sent.startTime - b.key, resp.startTime - sent.startTime, painted.dom - resp.startTime, painted.paint - painted.dom]
                : null,
          };
        });
        if (t.echo !== null) echo.push(t.echo);
        if (t.results !== null) results.push(t.results);
        else if (t.resp) missed++;
        if (t.parts) t.parts.forEach((v, i) => parts[i].push(v));
      }
    }
  }
  return {
    keystrokeToEchoPaintMs: stats(echo),
    keystrokeToResultsPaintMs: stats(results),
    breakdownMs: { keyToSend: stats(parts[0]), sendToAnswer: stats(parts[1]), answerToDom: stats(parts[2]), domToPaint: stats(parts[3]) },
    responsesWithoutResultChange: missed,
  };
}

/** Screenshots of the key states, dark then light. */
async function shoot(browser, base) {
  mkdirSync(shotsDir, { recursive: true });
  const shots = [];
  for (const scheme of ["dark", "light"]) {
    const suffix = scheme === "dark" ? "" : "-light";
    const open = async (params) => {
      const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, reducedMotion: "reduce", colorScheme: scheme });
      await ctx.addInitScript(() => {
        try {
          localStorage.setItem("penguin.search.recent", JSON.stringify(["lease renewal", "from:priya has:pdf", "when did i last email priya?", "board deck"]));
        } catch {}
      });
      const page = await ctx.newPage();
      await page.goto(`${base}?${new URLSearchParams(params)}`);
      await page.waitForSelector('[role="option"][data-thread]', { timeout: 30_000 });
      return { ctx, page };
    };
    const snap = async (page, name) => {
      await page.waitForTimeout(600);
      const path = resolve(shotsDir, `${name}${suffix}.png`);
      await page.locator(".search.panel").screenshot({ path });
      shots.push(path);
    };
    const typeQuery = async (page, q) => {
      await openSearch(page);
      await page.keyboard.type(q, { delay: 25 });
    };
    const { ctx, page } = await open({ semantic: "ready" });
    await openSearch(page);
    await snap(page, "01-empty");
    await typeQuery(page, "mi");
    await snap(page, "02-suggestions-people");
    await typeQuery(page, "from mike this year pdf");
    await snap(page, "03-plain-english-chips");
    await typeQuery(page, "rent going up");
    await snap(page, "04-meaning-passages");
    await typeQuery(page, "lease");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await snap(page, "05-top-results-selection");
    await typeQuery(page, "when did i last email priya");
    await snap(page, "06-ask-answer");
    await typeQuery(page, "leese renewel");
    await snap(page, "07-did-you-mean");
    await typeQuery(page, "zqxv budget");
    await snap(page, "08-no-results-widen");
    await ctx.close();
    if (scheme === "dark") {
      const ix = await open({ semantic: "indexing" });
      await typeQuery(ix.page, "rent going up");
      await snap(ix.page, "09-indexing-line");
      await ix.ctx.close();
    }
  }
  return shots;
}

async function main() {
  const dist = build();
  const server = await serve(dist);
  const base = `http://127.0.0.1:${PORT}/`;
  const browser = await chromium.launch({ headless: true });
  let out;
  try {
    if (shotsDir) {
      out = { shots: await shoot(browser, base) };
    } else {
      const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1, reducedMotion: "reduce" });
      const page = await ctx.newPage();
      await page.addInitScript(instrument);
      await page.goto(base);
      await page.waitForSelector('[role="option"][data-thread]', { timeout: 30_000 });
      out = { browser: `Chromium ${browser.version()}`, reps: REPS, queries: QUERIES, ...(await measure(page)) };
      await ctx.close();
    }
  } finally {
    await browser.close();
    server.close();
    rmSync(dist, { recursive: true, force: true });
  }
  const json = JSON.stringify(out, null, 2);
  if (outPath) writeFileSync(outPath, json);
  console.log(json);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
