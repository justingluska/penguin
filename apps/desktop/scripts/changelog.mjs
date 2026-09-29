// Writes src/generated/changelog.json for Settings → What's new, from git.
// Runs before every production build (package.json "build").
//
// Releases come from releases.json next to the updates (published by
// .github/workflows/release.yml): [{ version, commit, date }], oldest first.
// Each release lists the first-parent commits on main since the one before
// it, as user-facing lines: merge summaries and topic commits, minus
// internal scopes (docs, ci, …), with the "scope: " prefix dropped.
// whats-new.json can replace a commit's line with plain words, or hide it
// (null), keyed by a commit hash prefix; the next build shows the result for
// every release, old ones included.
//
//   PENGUIN_VERSION   this build's version (CI: 0.1.<run>); default: tauri.conf.json's
//   RELEASES_URL      default: releases.json next to the first updater endpoint in
//                     tauri.conf.json (plugins.updater.endpoints), so a fork that
//                     points the updater at its own host gets its own history.
//                     Source builds name no endpoint, so they list no releases;
//                     the release workflow sets RELEASES_URL.
//   --releases-out F  also write releases.json with this build appended (CI uploads it)
//   --notes-out F     also write this release's lines as text (latest.json notes)
//
// No network or no git history just means a shorter changelog, never a failed build.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const OUT = join(here, "../src/generated/changelog.json");
const CONF = JSON.parse(readFileSync(join(here, "../src-tauri/tauri.conf.json"), "utf8"));
const RELEASES_URL = process.env.RELEASES_URL ?? releasesUrlFor(CONF.plugins?.updater?.endpoints?.[0]);

/** "https://host/latest.json" → "https://host/releases.json"; no endpoint → null (no published releases). */
export function releasesUrlFor(endpoint) {
  if (typeof endpoint !== "string" || !endpoint) return null;
  return new URL("releases.json", endpoint).href;
}
/** Commits shown for the first release (the history before it is the whole project). */
const FIRST_RELEASE_MAX = 25;
const INTERNAL = new Set(["docs", "ci", "box", "design", "scripts", "mock", "research", "icons", "chore", "test", "tests", "release", "publish"]);
/** Commit hash prefix → the line to show instead, or null to hide it. */
const OVERRIDES = loadOverrides(join(here, "../whats-new.json"));

export function loadOverrides(path) {
  try {
    const commits = JSON.parse(readFileSync(path, "utf8")).commits ?? {};
    return Object.entries(commits).filter(([k]) => /^[0-9a-f]{7,40}$/.test(k));
  } catch {
    return [];
  }
}

/** The override for a full commit hash: undefined = none, null = hide, string = the line. */
export function overrideFor(hash, overrides = OVERRIDES) {
  const hit = overrides.find(([prefix]) => hash.startsWith(prefix));
  return hit ? hit[1] : undefined;
}

const arg = (name) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
};

function git(...args) {
  return execFileSync("git", args, { cwd: here, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
}

/** "Merge: Floe mode exit…" → "Floe mode exit…"; "compose: lock replies…" → "Lock replies…"; internal → null. */
export function userLine(subject) {
  let s = subject.trim();
  if (/^Merge (branch|pull request|remote)/.test(s)) return null;
  s = s.replace(/^Merge:\s*/, "");
  const m = /^([a-z][\w/ -]{0,30}):\s+(.+)$/.exec(s);
  if (m) {
    // "box-cargo", "ci/updater": a scope's first word decides (box-cargo is box tooling).
    const scopes = m[1].split(/[\s/]+/).map((x) => x.split("-")[0]);
    if (scopes.every((x) => INTERNAL.has(x))) return null;
    s = m[2];
  }
  return s ? s[0].toUpperCase() + s.slice(1) : null;
}

/** First-parent commits in (from, to], newest first, as { hash, text }. */
function changes(from, to, max) {
  let out;
  try {
    const range = from ? `${from}..${to}` : to;
    const args = ["log", "--first-parent", "--format=%H%x09%s", range];
    if (max) args.splice(1, 0, `-${max}`);
    out = git(...args);
  } catch {
    return [];
  }
  return out
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      const [full, subject] = l.split("\t");
      const o = overrideFor(full);
      return { hash: full.slice(0, 7), text: o === undefined ? userLine(subject ?? "") : o };
    })
    .filter((c) => c.text);
}

async function fetchReleases() {
  if (!RELEASES_URL) return [];
  try {
    const r = await fetch(RELEASES_URL, { signal: AbortSignal.timeout(8000) });
    if (!r.ok) return [];
    const j = await r.json();
    return Array.isArray(j) ? j.filter((x) => x && typeof x.version === "string" && typeof x.commit === "string") : [];
  } catch {
    return [];
  }
}

async function main() {
  const conf = CONF;
  // A local build (install-local.sh) isn't a release: it's listed as "local build".
  const version = process.env.PENGUIN_VERSION || conf.version;
  const entryVersion = process.env.PENGUIN_VERSION || "local build";
  let head = null;
  try {
    head = git("rev-parse", "HEAD");
  } catch {
    // Not a git checkout (a source tarball): no history to show.
  }

  const published = await fetchReleases();
  // This build becomes a release when CI publishes it; a local build is "this build".
  const all = head && !published.some((r) => head.startsWith(r.commit) || r.commit.startsWith(head))
    ? [...published, { version: entryVersion, commit: head, date: new Date().toISOString(), current: true }]
    : published.map((r) => ({ ...r, current: head ? head.startsWith(r.commit) || r.commit.startsWith(head) : false }));

  const releases = all.map((r, i) => ({
    version: r.version,
    date: r.date,
    current: !!r.current,
    changes: changes(i > 0 ? all[i - 1].commit : null, r.commit, i > 0 ? 0 : FIRST_RELEASE_MAX),
  }));

  mkdirSync(dirname(OUT), { recursive: true });
  writeFileSync(OUT, JSON.stringify({ version, releases: releases.reverse() }, null, 2) + "\n");

  const relOut = arg("--releases-out");
  if (relOut && head) {
    const list = published.some((r) => r.commit === head) ? published : [...published, { version, commit: head, date: new Date().toISOString() }];
    writeFileSync(relOut, JSON.stringify(list, null, 2) + "\n");
  }
  const notesOut = arg("--notes-out");
  if (notesOut) {
    const mine = releases.find((r) => r.current);
    writeFileSync(notesOut, (mine?.changes ?? []).map((c) => `• ${c.text}`).join("\n"));
  }
  console.log(`changelog: ${releases.length} release(s), ${releases[0]?.changes.length ?? 0} change(s) in ${version}`);
}

// Imported by tests for userLine(); run directly by the build.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) await main();
