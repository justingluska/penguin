// Sidebar theme contrast. Run: npm test.
// Parses src/styles/themes.css and checks every theme, dark and light, against
// WCAG AA (4.5:1): the four sidebar text levels on the sidebar background, the
// two strongest on the active and hover fills, the faintest (counts) on the
// active fill, the primary button's label, and the unread pill (styles/app.css
// .badge-unread, the theme's accent tint) on the sidebar and on the active row.
//
// Then the account color palette (lib/themes.ts ACCOUNT_COLORS, rendered by
// lib/accountColor.ts): its shape, that it keeps every color accounts and
// profiles already have, that the swatches stay apart, and that account-
// colored text passes AA on its tinted fill on every sidebar theme and app
// surface, dark and light.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  ACCENT_COLORS,
  ACCOUNT_COLORS,
  ACCOUNT_COLOR_COLUMNS,
  CORNER_STYLES,
  DARK_SHADES,
  SIDEBAR_THEMES,
  applyAccentAndCorners,
  applyCachedAccentAndCorners,
  applyCachedDarkShade,
  applyDarkShade,
} from "../src/lib/themes.ts";
import { ACCT_TINTS, accountShades, accountTone } from "../src/lib/accountColor.ts";

const css = readFileSync(new URL("../src/styles/themes.css", import.meta.url), "utf8");

type RGBA = [number, number, number, number];

function hex(h: string): RGBA {
  const s = h.replace("#", "");
  assert.ok(s.length === 6 || s.length === 8, `bad color ${h}`);
  const c = [0, 2, 4].map((i) => parseInt(s.slice(i, i + 2), 16));
  return [c[0], c[1], c[2], s.length === 8 ? parseInt(s.slice(6), 16) / 255 : 1];
}

function over(fg: RGBA, bg: RGBA): RGBA {
  const a = fg[3];
  return [0, 1, 2].map((i) => fg[i] * a + bg[i] * (1 - a)).concat(1) as RGBA;
}

/** The unread pill's rule in styles/app.css: text is th-accent-text mixed toward sb-text-1, on th-accent-soft. */
const badgeRule = (() => {
  const app = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");
  const rule = app.match(/\.nav-item \.count\.badge-unread \{([^}]*)\}/)?.[1] ?? "";
  const mix = rule.match(/color:\s*color-mix\(in srgb, var\(--th-accent-text\) (\d+)%, var\(--sb-text-1\)\)/);
  assert.ok(mix, "styles/app.css .badge-unread: expected color: color-mix(in srgb, var(--th-accent-text) N%, var(--sb-text-1))");
  assert.match(rule, /background:\s*var\(--th-accent-soft\);/, "styles/app.css .badge-unread: expected background: var(--th-accent-soft)");
  return { accentShare: Number(mix[1]) / 100 };
})();

function mixed(a: RGBA, b: RGBA, share: number): RGBA {
  return [0, 1, 2].map((i) => a[i] * share + b[i] * (1 - share)).concat(1) as RGBA;
}

function luminance(c: RGBA): number {
  const f = (v: number) => {
    const x = v / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]);
}

function contrast(a: RGBA, b: RGBA): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** Token blocks keyed by selector list, e.g. `[data-sidebar-theme="mint"]`. */
function blocks(): { selector: string; vars: Record<string, string> }[] {
  const out = [];
  for (const m of css.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    const vars: Record<string, string> = {};
    for (const d of m[2].matchAll(/--([\w-]+):\s*([^;]+);/g)) vars[d[1]] = d[2].trim();
    if (vars["sb-bg"]) out.push({ selector: m[1].replace(/\/\*[\s\S]*?\*\//g, "").trim(), vars });
  }
  return out;
}

const TOKENS = [
  "sb-bg", "sb-edge", "sb-text-1", "sb-text-2", "sb-text-3", "sb-text-4", "sb-fill", "sb-fill-hover",
  "sb-ring", "sb-hairline", "sb-primary", "sb-on-primary", "th-accent", "th-accent-soft", "th-accent-text", "th-accent-ink",
];

function themeBlock(id: string, mode: "dark" | "light", shade?: string) {
  const found = blocks().filter((b) => {
    const sels = b.selector.split(",").map((s) => s.trim());
    // Graphite's per-shade blocks are only looked up by shade.
    if (sels.some((s) => s.includes("data-dark-shade")) !== !!shade) return false;
    if (shade && !sels.some((s) => s.includes(`[data-dark-shade="${shade}"]`))) return false;
    const own = sels.some((s) => s.includes(`[data-sidebar-theme="${id}"]`));
    const light = sels.some((s) => s.startsWith(`[data-theme="light"]`));
    return own && light === (mode === "light");
  });
  assert.equal(found.length, 1, `${id}/${mode}: expected one token block, found ${found.length}`);
  return found[0].vars;
}

test("there are 22 themes, Graphite first, the same set in the UI, types.ts and settings.rs", () => {
  assert.equal(SIDEBAR_THEMES.length, 22);
  assert.equal(SIDEBAR_THEMES[0].id, "graphite");
  const ids = SIDEBAR_THEMES.map((t) => t.id).sort();
  assert.equal(new Set(ids).size, 22);
  const rs = readFileSync(new URL("../src-tauri/src/settings.rs", import.meta.url), "utf8").match(/pub enum SidebarTheme \{([\s\S]*?)\n\}/)![1];
  assert.match(rs, /#\[default\]\s*\n\s*Graphite,/);
  assert.deepEqual([...rs.matchAll(/^\s+([A-Z]\w*),/gm)].map((m) => m[1].toLowerCase()).sort(), ids);
  const ts = readFileSync(new URL("../src/lib/types.ts", import.meta.url), "utf8").match(/export type SidebarTheme =([^;]+);/)![1];
  assert.deepEqual([...ts.matchAll(/"(\w+)"/g)].map((m) => m[1]).sort(), ids);
});

function oklabOf(h: string): [number, number, number] {
  return oklab(hex(h.slice(0, 7)));
}

test("no two themes are near-duplicates: accent plus both sidebar backgrounds differ by at least 0.08 in OKLab", () => {
  // Summed over the light and dark accents' nearer pair and both sidebars. The
  // closest original pair (Mint and Sage) scores 0.094.
  const d = (a: string, b: string) => Math.hypot(...oklabOf(a).map((v, i) => v - oklabOf(b)[i]));
  const fails: string[] = [];
  for (let i = 0; i < SIDEBAR_THEMES.length; i++) {
    for (let j = i + 1; j < SIDEBAR_THEMES.length; j++) {
      const [a, b] = [SIDEBAR_THEMES[i].id, SIDEBAR_THEMES[j].id];
      const [ad, al, bd, bl] = [themeBlock(a, "dark"), themeBlock(a, "light"), themeBlock(b, "dark"), themeBlock(b, "light")];
      const score =
        Math.min(d(ad["th-accent"], bd["th-accent"]), d(al["th-accent"], bl["th-accent"])) + d(ad["sb-bg"], bd["sb-bg"]) + d(al["sb-bg"], bl["sb-bg"]);
      if (score < 0.08) fails.push(`${a} / ${b}: ${score.toFixed(3)}`);
    }
  }
  assert.deepEqual(fails, []);
});

for (const { id } of SIDEBAR_THEMES) {
  for (const mode of ["dark", "light"] as const) {
    test(`${id} (${mode}) defines every token as hex and passes WCAG AA`, () => sidebarPasses(id, mode, themeBlock(id, mode)));
  }
}

// Graphite follows the dark mode shade; every shade's Graphite passes the same checks.
for (const { id: shade } of DARK_SHADES.filter((s) => s.id !== "black")) {
  test(`graphite in the ${shade} shade defines every token as hex and passes WCAG AA`, () =>
    sidebarPasses("graphite", `dark ${shade}`, themeBlock("graphite", "dark", shade)));
}

function sidebarPasses(id: string, mode: string, v: Record<string, string>) {
  for (const t of TOKENS) assert.match(v[t] ?? "", /^#([0-9a-f]{6}|[0-9a-f]{8})$/i, `${id}/${mode}: --${t}`);
  const c = (k: string) => hex(v[k]);
  for (const k of ["sb-bg", "sb-text-1", "sb-text-2", "sb-text-3", "sb-text-4", "sb-primary", "sb-on-primary"])
    assert.equal(c(k)[3], 1, `${id}/${mode}: --${k} must be opaque`);
  const bg = c("sb-bg");
  const active = over(c("sb-fill"), bg);
  const hover = over(c("sb-fill-hover"), bg);
  const pill = mixed(c("th-accent-text"), c("sb-text-1"), badgeRule.accentShare);
  const checks: [string, RGBA, RGBA][] = [
    ["text-1 on bg", c("sb-text-1"), bg],
    ["text-2 on bg", c("sb-text-2"), bg],
    ["text-3 on bg", c("sb-text-3"), bg],
    ["text-4 on bg", c("sb-text-4"), bg],
    ["text-1 on active", c("sb-text-1"), active],
    ["text-2 on active", c("sb-text-2"), active],
    ["text-4 on active", c("sb-text-4"), active],
    ["text-1 on hover", c("sb-text-1"), hover],
    ["primary label", c("sb-on-primary"), c("sb-primary")],
    ["unread pill", pill, over(c("th-accent-soft"), bg)],
    ["unread pill on active", pill, over(c("th-accent-soft"), active)],
  ];
  // Text on a solid theme-accent fill ("today" in the calendar). Graphite's
  // accent is the default blue, whose light-mode ink stays the white it has
  // always been (3.3:1; see the Accent color test).
  if (!(id === "graphite" && mode === "light")) checks.push(["ink on accent", c("th-accent-ink"), c("th-accent")]);
  for (const [what, fg, back] of checks) {
    const r = contrast(fg, back);
    assert.ok(r >= 4.5, `${id}/${mode}: ${what} is ${r.toFixed(2)}:1, needs 4.5:1`);
  }
}

test("dark pastels stay deep: sidebar backgrounds are dark in dark mode and light in light mode", () => {
  for (const { id } of SIDEBAR_THEMES) {
    assert.ok(luminance(hex(themeBlock(id, "dark")["sb-bg"])) < 0.03, `${id}: dark sidebar too bright`);
    assert.ok(luminance(hex(themeBlock(id, "light")["sb-bg"])) > 0.7, `${id}: light sidebar too dark`);
  }
});

// ---------------------------------------------------------------------------
// Account colors
// ---------------------------------------------------------------------------
const src = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");

test("the palette is 40 distinct, named #RRGGBB colors, ten to a row", () => {
  assert.equal(ACCOUNT_COLOR_COLUMNS, 10);
  assert.equal(ACCOUNT_COLORS.length, 40);
  for (const c of ACCOUNT_COLORS) assert.match(c.hex, /^#[0-9A-F]{6}$/, c.name);
  assert.equal(new Set(ACCOUNT_COLORS.map((c) => c.hex)).size, 40, "duplicate hex");
  assert.equal(new Set(ACCOUNT_COLORS.map((c) => c.name)).size, 40, "duplicate name");
});

test("existing accounts and profiles keep a palette color", () => {
  const inPalette = (h: string) => ACCOUNT_COLORS.some((c) => c.hex.toLowerCase() === h.toLowerCase());
  const rust = src("../src-tauri/src/ops.rs").match(/ACCOUNT_PALETTE: &\[&str\] = &\[([^\]]*)\]/)?.[1] ?? "";
  const assigned = [...rust.matchAll(/"(#[0-9A-Fa-f]{6})"/g)].map((m) => m[1]);
  assert.ok(assigned.length >= 8, "ops.rs ACCOUNT_PALETTE not found");
  for (const h of assigned) assert.ok(inPalette(h), `ops.rs ACCOUNT_PALETTE ${h} is missing from ACCOUNT_COLORS`);
  const profiles = src("../src/app/profiles.ts").match(/PROFILE_COLORS = \[([^\]]*)\]/)?.[1] ?? "";
  const profileHexes = [...profiles.matchAll(/"(#[0-9A-Fa-f]{6})"/g)].map((m) => m[1]);
  assert.ok(profileHexes.length >= 8, "profiles.ts PROFILE_COLORS not found");
  for (const h of profileHexes) assert.ok(inPalette(h), `PROFILE_COLORS ${h} is missing from ACCOUNT_COLORS`);
});

function oklab(c: RGBA): [number, number, number] {
  const f = (v: number) => {
    const x = v / 255;
    return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  };
  const [r, g, b] = [f(c[0]), f(c[1]), f(c[2])];
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

test("swatches stay apart: every pair differs by at least 0.05 in OKLab (a few just-noticeable steps)", () => {
  for (let i = 0; i < ACCOUNT_COLORS.length; i++) {
    for (let j = i + 1; j < ACCOUNT_COLORS.length; j++) {
      const [a, b] = [oklab(hex(ACCOUNT_COLORS[i].hex)), oklab(hex(ACCOUNT_COLORS[j].hex))];
      const d = Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
      assert.ok(d >= 0.05, `${ACCOUNT_COLORS[i].name} and ${ACCOUNT_COLORS[j].name} are ${d.toFixed(3)} apart`);
    }
  }
});

/** The tint shares in themes.css .t-acct (dark) and [data-theme="light"] .t-acct. */
function cssTints(selector: string): number[] {
  const at = css.indexOf(selector + " {");
  assert.ok(at >= 0, `themes.css: ${selector} not found`);
  const body = css.slice(at, css.indexOf("}", at));
  return ["t3", "t4", "t5"].map((t) => {
    const m = body.match(new RegExp(`--${t}: color-mix\\(in srgb, var\\(--acct\\) (\\d+)%, transparent\\)`));
    assert.ok(m, `themes.css ${selector}: --${t} should be color-mix(in srgb, var(--acct) N%, transparent)`);
    return Number(m[1]) / 100;
  });
}

test("themes.css account tints match ACCT_TINTS", () => {
  assert.deepEqual(cssTints(".t-acct"), [...ACCT_TINTS.dark]);
  assert.deepEqual(cssTints('[data-theme="light"] .t-acct'), [...ACCT_TINTS.light]);
  assert.match(css, /\.t-acct \{[^}]*--t11: var\(--acct-on-dark\)/);
  assert.match(css, /\[data-theme="light"\] \.t-acct \{[^}]*--t11: var\(--acct-on-light\)/);
});

/** App surfaces (styles/penguin.css) where account-colored text sits outside the sidebar: canvas, panels, cards, a hovered row. */
/** A token from styles/penguin.css: the dark or light block, or a dark shade's block over the dark one. */
function penguinVar(mode: "dark" | "light", shade?: string) {
  const pcss = src("../src/styles/penguin.css");
  const block = (head: string) => {
    const start = pcss.indexOf(head);
    assert.ok(start >= 0, `penguin.css: ${head}`);
    return pcss.slice(start, pcss.indexOf("}", start));
  };
  const bodies = [block(mode === "dark" ? ':root,\n[data-theme="dark"] {' : '[data-theme="light"] {')];
  if (shade && shade !== "black") bodies.unshift(block(`[data-theme="dark"][data-dark-shade="${shade}"] {`));
  return (k: string): RGBA => {
    for (const body of bodies) {
      const m = body.match(new RegExp(`--${k}:\\s*(#[0-9a-f]{3,8})\\b`, "i"));
      if (m) return hex(m[1].length === 4 ? "#" + [...m[1].slice(1)].map((d) => d + d).join("") : m[1]);
    }
    assert.fail(`penguin.css ${mode}${shade ? ` ${shade}` : ""}: --${k}`);
  };
}

/** App surfaces (styles/penguin.css) where account-colored text sits outside the sidebar: canvas, panels, cards, a hovered row. */
function appSurfaces(mode: "dark" | "light", shade?: string): RGBA[] {
  const v = penguinVar(mode, shade);
  const background = v("background");
  return [background, v("bg-panel"), v("gray-1"), v("gray-2"), over(v(mode === "dark" ? "gray-a2" : "gray-a1"), background)];
}

for (const mode of ["dark", "light"] as const) {
  test(`account-colored text passes AA on its tint on every theme (${mode})`, () => {
    const [t3, t4, t5] = ACCT_TINTS[mode];
    const fails: string[] = [];
    for (const c of ACCOUNT_COLORS) {
      const s = accountShades(c.hex)!;
      const text = hex(mode === "dark" ? s.onDark : s.onLight);
      const color = hex(c.hex);
      const tint = (share: number, bg: RGBA): RGBA => over([color[0], color[1], color[2], share], bg);
      const check = (where: string, bg: RGBA) => {
        const r = contrast(text, bg);
        if (r < 4.5) fails.push(`${c.name} ${where}: ${r.toFixed(2)}:1`);
      };
      // Sidebar: tiles and monograms on --t3, on the row, the active row and a hovered row.
      for (const { id } of SIDEBAR_THEMES) {
        const v = themeBlock(id, mode);
        const bg = hex(v["sb-bg"]);
        for (const [what, surface] of [["", bg], [" active", over(hex(v["sb-fill"]), bg)], [" hover", over(hex(v["sb-fill-hover"]), bg)]] as const)
          check(`on ${id}${what}`, tint(t3, surface));
      }
      // Graphite's sidebar in each dark shade.
      if (mode === "dark")
        for (const { id: shade } of DARK_SHADES.filter((d) => d.id !== "black")) {
          const v = themeBlock("graphite", "dark", shade);
          const bg = hex(v["sb-bg"]);
          for (const surface of [bg, over(hex(v["sb-fill"]), bg), over(hex(v["sb-fill-hover"]), bg)]) check(`on graphite (${shade})`, tint(t3, surface));
        }
      // App: badges on --t3, calendar chips on --t4 and hovered on --t5 (in every dark shade).
      for (const shade of mode === "dark" ? DARK_SHADES.map((d) => d.id) : [undefined])
        appSurfaces(mode, shade).forEach((surface, i) => {
          for (const [n, share] of [[3, t3], [4, t4], [5, t5]] as const) check(`on app surface ${i}${shade ? ` (${shade})` : ""} t${n}`, tint(share, surface));
        });
    }
    assert.deepEqual(fails, []);
  });
}

test("account text keeps the picked color when it already passes, and its hue otherwise", () => {
  // A pale butter (a custom hex) reads on every dark surface, Dim's included, as is; light mode darkens it.
  const s = accountShades("#FFE9A8")!;
  assert.equal(s.mark, "#ffe9a8");
  assert.equal(s.onDark, "#ffe9a8");
  assert.notEqual(s.onLight, "#ffe9a8");
  // Honey is a hair too dark for Dim's lightest fills, so dark mode lifts it a little.
  assert.equal(accountShades("#FBCE5C")!.mark, "#fbce5c");
  assert.notEqual(accountShades("#FBCE5C")!.onDark, "#fbce5c");
  for (const c of ACCOUNT_COLORS) {
    const sh = accountShades(c.hex)!;
    for (const t of [sh.onDark, sh.onLight]) {
      const [, a0, b0] = oklab(hex(c.hex));
      const [, a1, b1] = oklab(hex(t));
      if (Math.hypot(a0, b0) < 0.04 || Math.hypot(a1, b1) < 0.04) continue; // near-neutral: hue is noise
      const dh = Math.abs(((Math.atan2(b1, a1) - Math.atan2(b0, a0) + 3 * Math.PI) % (2 * Math.PI)) - Math.PI);
      assert.ok(dh < 0.2, `${c.name}: text shade ${t} drifts ${dh.toFixed(2)} rad in hue`);
    }
  }
});

test("accountTone: a class per color, gray when unset or malformed", () => {
  assert.equal(accountTone("#4F7CFF"), "acct ac-4f7cff");
  assert.equal(accountTone(" #4f7cff "), "acct ac-4f7cff");
  assert.equal(accountTone(null), "gray");
  assert.equal(accountTone("blue"), "gray");
  assert.equal(accountTone("#12345"), "gray");
});

// ---------------------------------------------------------------------------
// Dark mode shades (styles/penguin.css 1c)
// ---------------------------------------------------------------------------
const SHADE_TOKENS = [
  ...Array.from({ length: 12 }, (_, i) => `gray-${i + 1}`),
  "gray-a1", "gray-a2", "gray-a3", "gray-a4", "gray-a5", "gray-a6", "background", "bg-panel", "shade-side", "logo-tile",
];

test("Black is the default and leads; the shades match between the design system and the app", () => {
  assert.equal(DARK_SHADES[0].id, "black");
  assert.match(src("../src/lib/settings.ts"), /\n\s+darkShade: "black",\n/);
  assert.match(src("../src/lib/mock/mail.ts"), /darkShade: \(devQuery\("darkShade"\)[^\n]*\?\? "black",/);
  const rs = src("../src-tauri/src/settings.rs").match(/pub enum DarkShade \{([\s\S]*?)\n\}/)![1];
  assert.match(rs, /#\[default\]\s*\n\s*Black,/);
  const variants = [...rs.matchAll(/^\s+([A-Z]\w*),/gm)].map((m) => m[1].toLowerCase());
  assert.deepEqual(variants, DARK_SHADES.map((d) => d.id));
  const ts = src("../src/lib/types.ts").match(/export type DarkShade = ([^;]+);/)![1];
  assert.deepEqual(ts.split("|").map((s) => s.trim().replace(/"/g, "")), DARK_SHADES.map((d) => d.id));
  // design/penguin.css is the source of visual truth; the app's copy carries the same shades.
  const section = (css: string) => css.slice(css.indexOf("1c. Dark shades"), css.indexOf("2. Constants"));
  assert.equal(section(src("../src/styles/penguin.css")), section(src("../../../design/penguin.css")));
});

for (const { id: shade } of DARK_SHADES) {
  test(`${shade}: every surface token is set and text keeps its contrast (4.5:1 body, 3:1 faint)`, () => {
    const v = penguinVar("dark", shade);
    for (const t of SHADE_TOKENS) v(t);
    const bg = v("background");
    const surfaces: [string, RGBA][] = [
      ["canvas", bg],
      ["panel", v("bg-panel")],
      ["subtle", v("gray-1")],
      ["elevated", v("gray-2")],
      ["hovered row", over(v("gray-a2"), bg)],
      ["selected row", over(v("gray-a3"), bg)],
    ];
    const fails: string[] = [];
    for (const [where, surface] of surfaces) {
      for (const [level, step, min] of [["emphasis", 12, 4.5], ["default", 11, 4.5], ["muted", 10, 4.5], ["faint", 9, 3]] as const) {
        const r = contrast(v(`gray-${step}`), surface);
        if (r < min) fails.push(`${level} on ${where}: ${r.toFixed(2)}:1`);
      }
    }
    assert.deepEqual(fails, []);
    // A dark theme: the canvas stays dark, panels lift off it, the Graphite sidebar sits below it.
    assert.ok(luminance(bg) < 0.03, `${shade}: canvas too bright`);
    if (shade !== "black") {
      assert.ok(luminance(v("bg-panel")) > luminance(bg), `${shade}: panels should sit above the canvas`);
      assert.ok(luminance(v("shade-side")) < luminance(bg), `${shade}: the sidebar should sit below the canvas`);
      assert.equal(themeBlock("graphite", "dark", shade)["sb-bg"], "#" + v("shade-side").slice(0, 3).map((x) => x.toString(16).padStart(2, "0")).join(""));
    }
  });
}

// ---------------------------------------------------------------------------
// Accent color and corners (styles/themes.css)
// ---------------------------------------------------------------------------
test("accent colors and corner styles: Blue and Rounded lead, the same set in the UI, types.ts and settings.rs", () => {
  const rs = src("../src-tauri/src/settings.rs");
  const variants = (name: string) => {
    const body = rs.match(new RegExp(`pub enum ${name} \\{([\\s\\S]*?)\\n\\}`))![1];
    return [...body.matchAll(/^\s+([A-Z]\w*),/gm)].map((m) => m[1].toLowerCase());
  };
  const tsUnion = (name: string) =>
    [...src("../src/lib/types.ts").match(new RegExp(`export type ${name} = ([^;]+);`))![1].matchAll(/"(\w+)"/g)].map((m) => m[1]);
  assert.deepEqual(variants("AccentColor"), ACCENT_COLORS.map((c) => c.id));
  assert.deepEqual(tsUnion("AccentColor"), ACCENT_COLORS.map((c) => c.id));
  assert.deepEqual(variants("CornerStyle"), CORNER_STYLES.map((c) => c.id));
  assert.deepEqual(tsUnion("CornerStyle"), CORNER_STYLES.map((c) => c.id));
  assert.equal(ACCENT_COLORS[0].id, "blue");
  assert.equal(CORNER_STYLES[0].id, "rounded");
  assert.match(src("../src/lib/settings.ts"), /\n\s+accentColor: "blue",\n\s+corners: "rounded",\n/);
  // Subtle and Square restate every radius, each no rounder than Rounded's.
  for (const style of ["subtle", "square"]) {
    const body = css.match(new RegExp(`:root\\[data-corners="${style}"\\] \\{([^}]*)\\}`))![1];
    for (const t of ["sm", "md", "lg", "xl", "2xl", "3xl", "row", "card"]) assert.match(body, new RegExp(`--radius-${t}: \\d+px;`), `${style}: --radius-${t}`);
  }
});

/** The accent tokens for one color and mode: Blue from penguin.css, the rest from themes.css. */
function accentTokens(id: string, mode: "dark" | "light"): Record<string, string> {
  if (id === "blue") {
    const v = penguinVar(mode);
    const h = (k: string) => "#" + v(k).slice(0, 3).map((x) => Math.round(x).toString(16).padStart(2, "0")).join("");
    return { accent: h("blue-a9"), "accent-text": h("blue-a11"), "accent-ink": mode === "dark" ? "#000000" : "#ffffff" };
  }
  const sel = mode === "dark" ? `:root[data-accent-color="${id}"] {` : `:root[data-theme="light"][data-accent-color="${id}"] {`;
  const at = css.indexOf(sel);
  assert.ok(at >= 0, `themes.css: ${sel}`);
  const body = css.slice(at, css.indexOf("}", at));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6,8});/gi)].map((m) => [m[1], m[2]]));
}

test("every accent color reads: its text at AA on every canvas and panel, its ink at AA on its fill", () => {
  const fails: string[] = [];
  for (const { id } of ACCENT_COLORS) {
    for (const mode of ["dark", "light"] as const) {
      const a = accentTokens(id, mode);
      for (const k of ["accent", "accent-text", "accent-ink"]) assert.match(a[k] ?? "", /^#[0-9a-f]{6}$/i, `${id}/${mode}: --${k}`);
      const shades = mode === "dark" ? DARK_SHADES.map((d) => d.id) : [undefined];
      for (const shade of shades) {
        const v = penguinVar(mode, shade);
        for (const [where, surface] of [["canvas", v("background")], ["panel", v("bg-panel")]] as const) {
          const r = contrast(hex(a["accent-text"]), surface);
          if (r < 4.5) fails.push(`${id}/${mode}${shade ? ` ${shade}` : ""}: text on ${where} ${r.toFixed(2)}:1`);
        }
      }
      // Blue's light ink is the white "today" has always had (3.3:1), kept so the default look doesn't change.
      if (!(id === "blue" && mode === "light")) {
        const r = contrast(hex(a["accent-ink"]), hex(a.accent));
        if (r < 4.5) fails.push(`${id}/${mode}: ink on accent ${r.toFixed(2)}:1`);
      }
    }
  }
  assert.deepEqual(fails, []);
});

test("the accent color and corners round-trip through <html> and the launch cache, unset at the defaults", () => {
  const g = globalThis as unknown as { document?: unknown; localStorage?: unknown };
  const dataset: Record<string, string> = {};
  const m = new Map<string, string>();
  g.document = { documentElement: { dataset } };
  g.localStorage = { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
  try {
    applyAccentAndCorners("teal", "square");
    assert.deepEqual([dataset.accentColor, dataset.corners], ["teal", "square"]);
    delete dataset.accentColor;
    delete dataset.corners;
    applyCachedAccentAndCorners();
    assert.deepEqual([dataset.accentColor, dataset.corners], ["teal", "square"]);
    applyAccentAndCorners("blue", "rounded");
    assert.deepEqual([dataset.accentColor, dataset.corners], [undefined, undefined]);
    applyAccentAndCorners("neon" as never, "blob" as never);
    assert.deepEqual([dataset.accentColor, dataset.corners], [undefined, undefined]);
  } finally {
    delete g.document;
    delete g.localStorage;
  }
});

test("the shade round-trips through <html data-dark-shade> and the launch cache", () => {
  const g = globalThis as unknown as { document?: unknown; localStorage?: unknown };
  const dataset: Record<string, string> = {};
  const m = new Map<string, string>();
  g.document = { documentElement: { dataset } };
  g.localStorage = { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
  try {
    for (const { id } of DARK_SHADES) {
      applyDarkShade(id);
      assert.equal(dataset.darkShade, id);
      dataset.darkShade = "";
      applyCachedDarkShade();
      assert.equal(dataset.darkShade, id);
    }
    applyDarkShade("oled" as never);
    assert.equal(dataset.darkShade, "black");
  } finally {
    delete g.document;
    delete g.localStorage;
  }
});
