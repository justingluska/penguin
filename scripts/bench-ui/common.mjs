// Shared by the bench-ui tools: build the mock bundle, serve it, summarize samples.
import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, statSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";

// ----- stats -----
export function stats(v) {
  if (!v.length) return { n: 0 };
  const s = [...v].sort((a, b) => a - b);
  const pct = (p) => s[Math.round((s.length - 1) * p)];
  const r = (x) => Math.round(x * 1000) / 1000;
  return { n: s.length, p50: r(pct(0.5)), p95: r(pct(0.95)), p99: r(pct(0.99)), max: r(s[s.length - 1]), mean: r(s.reduce((a, b) => a + b, 0) / s.length) };
}

// ----- build + serve -----
export function build(desktop) {
  const vite = join(desktop, "node_modules/.bin/vite");
  if (!existsSync(vite)) throw new Error("apps/desktop/node_modules is missing: run `npm ci` in apps/desktop first.");
  const out = mkdtempSync(join(tmpdir(), "penguin-bench-ui-"));
  const r = spawnSync(vite, ["build", "--outDir", out, "--emptyOutDir", "--logLevel", "warn"], {
    cwd: desktop,
    env: { ...process.env, VITE_MOCK: "1" },
    stdio: ["ignore", "inherit", "inherit"],
  });
  if (r.status !== 0) throw new Error("vite build failed");
  return out;
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".woff2": "font/woff2", ".woff": "font/woff", ".json": "application/json", ".ico": "image/x-icon" };
export function serve(dir, port = 0) {
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    let p = join(dir, decodeURIComponent(url.pathname));
    if (!p.startsWith(dir) || !existsSync(p) || statSync(p).isDirectory()) p = join(dir, "index.html");
    res.writeHead(200, { "content-type": TYPES[extname(p)] ?? "application/octet-stream", "cache-control": "no-store" });
    res.end(readFileSync(p));
  });
  return new Promise((ok) => server.listen(port, "127.0.0.1", () => ok(server)));
}

