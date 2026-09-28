// List style (Settings → General; lib/listStyle.ts, styles/list-styles.css).
// Run: npm test. Checks that Quiet is the default everywhere a default is
// written down, that the choice round-trips through <html data-list-style>
// and its launch cache, the row heights per style, that the CSS covers every
// style, and that Mail's white-on-accent selection passes AA on every theme.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  DEFAULT_LIST_STYLE,
  LINE_ROW_H,
  LIST_STYLES,
  STACKED_ROW_H,
  applyCachedListStyle,
  applyListStyle,
  listRowHeight,
} from "../src/lib/listStyle.ts";
import { searchSettings } from "../src/features/settings/catalog.ts";

const read = (p: string) => readFileSync(new URL(p, import.meta.url), "utf8");
const IDS = LIST_STYLES.map((s) => s.id);

test("Quiet is the default in the UI, the settings cache, the mock and the backend", () => {
  assert.equal(DEFAULT_LIST_STYLE, "quiet");
  assert.equal(IDS[0], "quiet", "Quiet leads the picker");
  assert.match(read("../src/lib/settings.ts"), /\n\s+listStyle: "quiet",\n/);
  assert.match(read("../src/lib/mock/mail.ts"), /listStyle: \(devQuery\("listStyle"\)[^\n]*\?\? "quiet",/);
  const rs = read("../src-tauri/src/settings.rs");
  const en = rs.match(/pub enum ListStyle \{([\s\S]*?)\n\}/)![1];
  assert.match(en, /#\[default\]\s*\n\s*Quiet,/);
  // The wire names are the same set on both sides (serde camelCase of the variants).
  const variants = [...en.matchAll(/^\s+([A-Z]\w*),/gm)].map((m) => m[1].charAt(0).toLowerCase() + m[1].slice(1));
  assert.deepEqual([...variants].sort(), [...IDS].sort());
  const ts = read("../src/lib/types.ts").match(/export type ListStyle = ([^;]+);/)![1];
  assert.deepEqual(ts.split("|").map((s) => s.trim().replace(/"/g, "")).sort(), [...IDS].sort());
});

class Storage {
  m = new Map<string, string>();
  getItem(k: string) {
    return this.m.get(k) ?? null;
  }
  setItem(k: string, v: string) {
    this.m.set(k, String(v));
  }
}

test("the style round-trips through <html data-list-style> and the launch cache", () => {
  const g = globalThis as unknown as { document?: unknown; localStorage?: unknown };
  const dataset: Record<string, string> = {};
  const storage = new Storage();
  g.document = { documentElement: { dataset } };
  g.localStorage = storage;
  try {
    // A fresh launch with nothing cached: Quiet.
    applyCachedListStyle();
    assert.equal(dataset.listStyle, "quiet");
    for (const id of IDS) {
      applyListStyle(id);
      assert.equal(dataset.listStyle, id);
      // The next launch puts it back before settings load.
      dataset.listStyle = "";
      applyCachedListStyle();
      assert.equal(dataset.listStyle, id);
    }
    // A name this build doesn't know (a newer build's, a hand edit) is Quiet.
    storage.setItem("penguin.listStyle", "dense");
    applyCachedListStyle();
    assert.equal(dataset.listStyle, "quiet");
    applyListStyle("sparkly" as never);
    assert.equal(dataset.listStyle, "quiet");
  } finally {
    delete g.document;
    delete g.localStorage;
  }
});

test("row heights: Quiet is Classic plus a hair, every style fits its lines", () => {
  // Classic is the original list, unchanged.
  assert.deepEqual(STACKED_ROW_H.classic, { compact: 72, comfortable: 80 });
  // Quiet: 4px more per row, no more.
  for (const d of ["compact", "comfortable"] as const) {
    assert.equal(STACKED_ROW_H.quiet[d] - STACKED_ROW_H.classic[d], 4, d);
    assert.equal(STACKED_ROW_H[IDS[0]][d], listRowHeight("quiet", d, true));
    for (const id of IDS) {
      const h = STACKED_ROW_H[id][d];
      assert.ok(Number.isInteger(h) && h >= STACKED_ROW_H.classic[d], `${id} ${d}: ${h}`);
      // Comfortable is roomier than compact in every style.
      assert.ok(STACKED_ROW_H[id].comfortable > STACKED_ROW_H[id].compact, id);
      // One-line rows don't depend on the style.
      assert.equal(listRowHeight(id, d, false), LINE_ROW_H[d]);
    }
  }
  // Cards inset 3px top and bottom; a code row (18 + 18 + 24 + 2 × 3 gaps = 66px) still fits with room.
  assert.ok(STACKED_ROW_H.cards.compact - 6 >= 66 + 8);
  // Mail's lines are 18 + 17 + 32 + 2 × 1 = 69px, plus padding.
  assert.ok(STACKED_ROW_H.mail.compact >= 69 + 12);
  // The one-line heights match the CSS the rows are drawn with.
  assert.match(read("../src/styles/penguin.css"), new RegExp(`--row-h: ${LINE_ROW_H.compact}px`));
  assert.match(read("../src/features/settings/settings.css"), new RegExp(`--row-h: ${LINE_ROW_H.comfortable}px`));
});

test("the stylesheet styles every style but Classic, and never with !important", () => {
  const css = read("../src/styles/list-styles.css");
  for (const id of IDS) {
    const has = css.includes(`[data-list-style="${id}"] .list`);
    if (id === "classic") assert.ok(!css.includes(`:root[data-list-style="classic"] .list`), "Classic is the base styling");
    else assert.ok(has, `${id} has rules`);
  }
  assert.ok(!/!important/.test(css));
  assert.match(read("../src/main.tsx"), /import "\.\/styles\/list-styles\.css";/);
});

// WCAG relative luminance and contrast.
const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const lum = ([r, g, b]: number[]) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
const contrast = (a: number[], b: number[]) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
};

test("Mail's selected row keeps white text at AA on every accent", () => {
  const css = read("../src/styles/list-styles.css");
  const share = Number(css.match(/--m-sel: color-mix\(in srgb, var\(--accent\) (\d+)%, #000\)/)![1]) / 100;
  // Blue by default, each Accent color, and with "Match accent to theme", each sidebar theme's own.
  const accents = new Set(["#0090ff", ...[...read("../src/styles/themes.css").matchAll(/--(?:th-)?accent: (#[0-9a-f]{6})/gi)].map((m) => m[1])]);
  assert.ok(accents.size > 40);
  for (const hex of accents) {
    const rgb = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) * share);
    const c = contrast(rgb, [255, 255, 255]);
    assert.ok(c >= 4.5, `${hex}: ${c.toFixed(2)}`);
  }
});

test("Settings search finds the picker", () => {
  const labels = (q: string) => searchSettings(q).flatMap((g) => g.hits.map((h) => `${g.page.id}:${h.entry?.label}`));
  for (const q of ["list style", "cards", "high contrast", "message list"]) assert.ok(labels(q).includes("general:List style"), q);
});
