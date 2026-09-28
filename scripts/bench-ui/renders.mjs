// Re-render census: which React components render, and how often, for each
// scripted action (launch, idle, j, scrolling, archive, search typing,
// compose). A regression check for needless re-renders: a j press should
// re-render the two rows whose selection changed, not the list.
//
//   cd scripts/bench-ui && node renders.mjs [--dist prebuilt-dir] [--threads 5000] [--port N] [--detail]
//
// Counting uses React's DevTools hook (onCommitFiberRoot), installed before
// the app loads, and the same rule as React DevTools: a component rendered
// in a commit if its fiber is new or did work (the PerformedWork flag), and a
// subtree whose child pointer didn't change was skipped. It works on the
// production bundle; component names are readable when the bundle is built
// with `--minify false`, which this tool does unless given --dist.

import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { serve } from "./common.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const desktop = resolve(here, "../../apps/desktop");
const argv = process.argv.slice(2);
const opt = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const DIST = opt("--dist", null);
const THREADS = Number(opt("--threads", "5000"));
const PORT = Number(opt("--port", "0"));
const DETAIL = argv.includes("--detail");

function buildReadable() {
  const vite = join(desktop, "node_modules/.bin/vite");
  if (!existsSync(vite)) throw new Error("apps/desktop/node_modules is missing: run `npm ci` in apps/desktop first.");
  const out = mkdtempSync(join(tmpdir(), "penguin-renders-"));
  const r = spawnSync(vite, ["build", "--minify", "false", "--outDir", out, "--emptyOutDir", "--logLevel", "warn"], {
    cwd: desktop,
    env: { ...process.env, VITE_MOCK: "1" },
    stdio: ["ignore", "inherit", "inherit"],
  });
  if (r.status !== 0) throw new Error("vite build failed");
  return out;
}

function hook() {
  const counts = new Map();
  let commits = 0;
  let per = [];
  let cur = null;
  window.__renders = {
    reset() {
      counts.clear();
      commits = 0;
      per = [];
    },
    get: () => ({ commits, per, all: [...counts].sort((a, b) => b[1] - a[1]) }),
  };
  const nameOf = (f) => {
    const t = f.type;
    if (typeof t === "function") return t.displayName || t.name || "anonymous";
    if (t && typeof t === "object") {
      const inner = t.type || t.render;
      return t.displayName || (inner ? inner.displayName || inner.name || "anonymous" : null);
    }
    return null;
  };
  // FunctionComponent 0, ClassComponent 1, ForwardRef 11, Memo 14, SimpleMemo 15
  const isComponent = (f) => f.tag === 0 || f.tag === 1 || f.tag === 11 || f.tag === 14 || f.tag === 15;
  const add = (f) => {
    const n = nameOf(f);
    if (!n) return;
    counts.set(n, (counts.get(n) ?? 0) + 1);
    cur.set(n, (cur.get(n) ?? 0) + 1);
  };
  const mounted = (f) => {
    for (; f; f = f.sibling) {
      if (isComponent(f)) add(f);
      mounted(f.child);
    }
  };
  const updated = (f) => {
    for (; f; f = f.sibling) {
      const prev = f.alternate;
      if (!prev) {
        if (isComponent(f)) add(f);
        mounted(f.child);
        continue;
      }
      if (isComponent(f) && f.flags & 1) add(f);
      if (f.child !== prev.child) updated(f.child);
    }
  };
  window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
    supportsFiber: true,
    renderers: new Map(),
    inject(r) {
      this.renderers.set(1, r);
      return 1;
    },
    onCommitFiberRoot(_id, root) {
      commits++;
      cur = new Map();
      updated(root.current.child);
      per.push([...cur].map(([k, v]) => `${k}×${v}`).join(" "));
    },
    onCommitFiberUnmount() {},
    onPostCommitFiberRoot() {},
    checkDCE() {},
  };
}

const report = (label, r) => {
  const total = r.all.reduce((a, [, n]) => a + n, 0);
  console.log(`\n== ${label}: ${r.commits} commits, ${total} component renders`);
  for (const [n, c] of r.all.slice(0, 15)) console.log(String(c).padStart(7), n);
  if (DETAIL) r.per.forEach((p, i) => console.log(`   commit ${i}: ${p}`));
};

async function main() {
  const dist = DIST ? resolve(DIST) : buildReadable();
  const server = await serve(dist, PORT);
  const browser = await chromium.launch({ headless: true });
  try {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    await page.addInitScript(hook);
    await page.goto(`http://127.0.0.1:${server.address().port}/?mockThreads=${THREADS}`);
    await page.waitForSelector('[role="option"][data-thread]');
    report("launch → first rows", await page.evaluate(() => window.__renders.get()));
    await page.waitForTimeout(1500);
    const measure = async (label, fn, settle = 400) => {
      await page.evaluate(() => window.__renders.reset());
      await fn();
      await page.waitForTimeout(settle);
      report(label, await page.evaluate(() => window.__renders.get()));
    };
    await measure("idle 3 s (sync-status ticks)", () => page.waitForTimeout(3000), 0);
    await page.locator('[role="option"][data-thread]').first().click();
    await page.waitForTimeout(800);
    await measure("one j", () => page.keyboard.press("j"));
    await measure("ten j", async () => {
      for (let i = 0; i < 10; i++) {
        await page.keyboard.press("j");
        await page.waitForTimeout(60);
      }
    });
    await measure("scroll 3,000 px", () =>
      page.evaluate(async () => {
        const el = document.querySelector(".list-scroll");
        for (let i = 0; i < 50; i++) {
          el.scrollTop += 60;
          await new Promise((r) => requestAnimationFrame(r));
        }
      }),
    );
    await measure("archive (e)", () => page.keyboard.press("e"), 800);
    await measure("open search (/)", () => page.keyboard.press("/"), 600);
    await measure("type 'invoice'", async () => {
      for (const c of "invoice") {
        await page.keyboard.type(c);
        await page.waitForTimeout(120);
      }
    }, 600);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(100);
    if (await page.locator(".search.panel").count()) await page.keyboard.press("Escape");
    await page.waitForTimeout(400);
    await measure("reply (r)", async () => {
      await page.keyboard.press("r");
      await page.waitForSelector(".ProseMirror");
    }, 800);
    await page.locator(".ProseMirror").first().click();
    await page.waitForTimeout(300);
    await measure("type 20 characters in the reply", async () => {
      for (const c of "thanks, see you then") {
        await page.keyboard.type(c);
        await page.waitForTimeout(40);
      }
    });
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
