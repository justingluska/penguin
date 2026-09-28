// macOS (and the release build) uses a case-insensitive file system: two
// paths that differ only in letter case are one file there, so an import of
// one silently resolves to the other. Linux doesn't notice, so check here.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { join, relative } from "node:path";

const root = join(import.meta.dirname, "..");
const SKIP = new Set(["node_modules", "dist", "target", ".git", "gen"]);

function walk(dir: string, out: string[]) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (SKIP.has(e.name)) continue;
    const p = join(dir, e.name);
    out.push(relative(root, p));
    if (e.isDirectory()) walk(p, out);
  }
}

test("no two files differ only in letter case", () => {
  const paths: string[] = [];
  for (const d of ["src", "tests", "scripts", "src-tauri/src"]) walk(join(root, d), paths);
  const seen = new Map<string, string>();
  const clashes: string[] = [];
  for (const p of paths) {
    const k = p.toLowerCase();
    const other = seen.get(k);
    if (other && other !== p) clashes.push(`${other} ↔ ${p}`);
    else seen.set(k, p);
  }
  assert.deepEqual(clashes, []);
});
