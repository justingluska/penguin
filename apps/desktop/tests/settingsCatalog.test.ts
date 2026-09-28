// Settings search (src/features/settings/catalog.ts): matching, grouping by
// page, and that every labeled setting row in the source is in the index.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import {
  SETTINGS_INDEX,
  SETTINGS_PAGES,
  flattenResults,
  labelMatches,
  normalize,
  searchSettings,
} from "../src/features/settings/catalog.ts";

const labels = (q: string) => searchSettings(q).flatMap((g) => g.hits.map((h) => `${g.page.id}:${h.entry?.label}`));
const pages = (q: string) => searchSettings(q).map((g) => g.page.id);

test("an empty or blank query finds nothing", () => {
  assert.deepEqual(searchSettings(""), []);
  assert.deepEqual(searchSettings("   "), []);
});

test("the queries people actually type land on the right rows", () => {
  const cases: [string, string][] = [
    ["photo", "you:Your photo"],
    ["dark", "general:Theme"],
    ["signature", "signatures:Add to new messages"],
    ["undo", "compose:Undo send"],
    ["quota", "diagnostics:Gmail quota (units per minute per account)"],
    ["notifications", "general:Notify me about new mail"],
    ["reply later", "general:Follow up after"],
    ["demo", "diagnostics:Demo mode"],
    ["unsubscribe", "privacy:Show unsubscribe button"],
    ["colour", "accounts:Account colors"],
    ["rsvp", "calendar:Answer invitations from Penguin"],
    ["mcp", "diagnostics:Enable MCP server"],
  ];
  for (const [q, want] of cases) assert.ok(labels(q).includes(want), `${q} → ${want}; got ${labels(q).join(", ")}`);
});

const first = (q: string) => {
  const [g] = searchSettings(q);
  return `${g.page.id}:${g.hits[0]?.entry?.label}`;
};

test("the most likely row comes first", () => {
  assert.equal(first("photo"), "you:Your photo");
  assert.equal(first("dark"), "general:Theme");
  assert.equal(first("undo"), "compose:Undo send");
  assert.equal(first("demo"), "diagnostics:Demo mode");
  assert.equal(first("notifications"), "general:Notify me about new mail");
});

test("the best match's page comes first", () => {
  assert.equal(pages("undo send")[0], "compose");
  assert.equal(pages("theme")[0], "general");
  assert.equal(pages("sync")[0], "sync");
});

test("every word must match, as a word prefix, in any field", () => {
  assert.ok(labels("remote img").length === 0);
  assert.ok(labels("remote ima").includes("privacy:Remote images"));
  // "hem" is inside "theme" but starts no word.
  assert.ok(!labels("hem").includes("general:Theme"));
  // Words can come from different fields: the label and a keyword.
  assert.ok(labels("theme night").includes("general:Theme"));
});

test("case, accents and punctuation don't matter", () => {
  assert.deepEqual(labels("UNDO"), labels("undo"));
  assert.deepEqual(labels("thème"), labels("theme"));
  assert.ok(labels("\"--\" separator").includes('signatures:"-- " separator'));
  assert.ok(labels("ive emailed").includes("general:Only people I've emailed before"));
});

test("a page's name finds the page without listing every row on it", () => {
  const [g] = searchSettings("calendar").filter((x) => x.page.id === "calendar");
  assert.ok(g.pageMatched);
  // Rows are listed only when the query hits them, not just the page name.
  assert.ok(!g.hits.some((h) => h.entry?.label === "Keep past events"));
  assert.ok(g.hits.some((h) => h.entry?.label === "Google Calendar"));
});

test("the page name can narrow a query", () => {
  assert.ok(labels("privacy images").includes("privacy:Remote images"));
  assert.ok(labels("sidebar general").every((l) => l.startsWith("general:")));
});

test("flattened results put each page before its rows", () => {
  const flat = flattenResults(searchSettings("photo"));
  assert.equal(flat[0].entry, null);
  for (let i = 1; i < flat.length; i++) {
    if (flat[i].entry) assert.equal(flat[i].page.id, flat[i - 1].page.id);
  }
});

test("labelMatches finds a rendered row by the start of its text", () => {
  const e = SETTINGS_INDEX.find((x) => x.label === "Sign-in method")!;
  assert.ok(labelMatches(e, "Sign-in method: Google iOS client"));
  assert.ok(!labelMatches(e, "Sign in again"));
  const dark = SETTINGS_INDEX.find((x) => x.label === "Dark email bodies")!;
  assert.ok(labelMatches(dark, "Dark email bodiesExperimental"));
});

test("the index is well formed", () => {
  const ids = new Set(SETTINGS_PAGES.map((p) => p.id));
  assert.equal(ids.size, SETTINGS_PAGES.length);
  const seen = new Set<string>();
  for (const e of SETTINGS_INDEX) {
    assert.ok(ids.has(e.page), `${e.label}: unknown page ${e.page}`);
    const key = `${e.page}:${normalize(e.label)}`;
    assert.ok(!seen.has(key), `duplicate ${key}`);
    seen.add(key);
  }
});

// Every row with literal `.setting-label` text shown in Settings must be in
// SETTINGS_INDEX, so new settings are searchable (see catalog.ts to add one).
const NOT_IN_SETTINGS = new Set(["RuleEditor.tsx"]); // its own dialog, not a Settings page

function tsxFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? tsxFiles(p) : p.endsWith(".tsx") ? [p] : [];
  });
}

test("every labeled setting in the source is searchable", () => {
  const root = new URL("../src/", import.meta.url).pathname;
  const missing: string[] = [];
  for (const file of tsxFiles(root)) {
    if (NOT_IN_SETTINGS.has(file.split("/").pop()!)) continue;
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(/className="(?:setting-label|st-sub)[^"]*"[^>]*>([^<{]*)(<\/|<|\{)/g)) {
      const text = m[1].replace(/\s+/g, " ").trim();
      if (!text) continue; // dynamic ({label}): add those to the index by hand
      // Text that goes on in a nested element or expression ("Command-line
      // tool: <span>penguin</span>") may be the start of the entry's label.
      const continues = m[2] !== "</";
      const found = SETTINGS_INDEX.some((e) => labelMatches(e, text) || (continues && normalize(e.label).startsWith(normalize(text))));
      if (!found) missing.push(`${file.slice(root.length)}: "${text}"`);
    }
  }
  assert.deepEqual(missing, [], "Add these to SETTINGS_INDEX in src/features/settings/catalog.ts");
});
