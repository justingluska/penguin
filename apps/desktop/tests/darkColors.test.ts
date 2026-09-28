// Dark email bodies: the color mapping (src/features/message-body/darkColors.ts).
// Run: npm test.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  DARK_CANVAS,
  WHITE,
  adjustBorder,
  adjustText,
  contrast,
  darkBackground,
  darkGradient,
  formatColor,
  fromOklch,
  isDark,
  parseColor,
  textTarget,
  toOklch,
  type RGBA,
} from "../src/features/message-body/darkColors.ts";

function hex(h: string, a = 1): RGBA {
  const s = h.replace("#", "");
  return { r: parseInt(s.slice(0, 2), 16), g: parseInt(s.slice(2, 4), 16), b: parseInt(s.slice(4, 6), 16), a };
}

function hueDelta(a: RGBA, b: RGBA): number {
  const d = Math.abs(toOklch(a).h - toOklch(b).h) % 360;
  return d > 180 ? 360 - d : d;
}

test("parses computed-style color serializations", () => {
  assert.deepEqual(parseColor("rgb(255, 128, 0)"), { r: 255, g: 128, b: 0, a: 1 });
  assert.deepEqual(parseColor("rgba(0, 0, 0, 0.5)"), { r: 0, g: 0, b: 0, a: 0.5 });
  assert.deepEqual(parseColor("rgb(10 20 30 / 25%)"), { r: 10, g: 20, b: 30, a: 0.25 });
  assert.deepEqual(parseColor("transparent"), { r: 0, g: 0, b: 0, a: 0 });
  const srgb = parseColor("color(srgb 1 0.5 0)")!;
  assert.equal(Math.round(srgb.g), 128);
  const ok = parseColor("oklch(0.628 0.2577 29.23)")!;
  assert.ok(ok.r > 250 && ok.g < 10 && ok.b < 10, formatColor(ok));
  for (const bad of ["", "red", "lab(50 20 30)", "rgb(1, 2)", "rgb(a, b, c)", "color(display-p3 1 0 0)", "url(x)"]) {
    assert.equal(parseColor(bad), null, bad);
  }
  assert.equal(formatColor({ r: 1.4, g: 2.6, b: 3, a: 1 }), "rgb(1, 3, 3)");
  assert.equal(formatColor({ r: 1, g: 2, b: 3, a: 0.5 }), "rgba(1, 2, 3, 0.5)");
});

test("OKLCH round trip", () => {
  for (const h of ["#0b63ce", "#e11d48", "#fef3c7", "#1c1c1e", "#777777"]) {
    const c = hex(h);
    const back = fromOklch(toOklch(c));
    assert.ok(Math.abs(back.r - c.r) < 1 && Math.abs(back.g - c.g) < 1 && Math.abs(back.b - c.b) < 1, h);
  }
});

test("light backgrounds turn dark, keeping their hue", () => {
  const white = darkBackground(WHITE)!;
  assert.ok(contrast(white, DARK_CANVAS) < 1.05, `white → ${formatColor(white)}`);
  // Light grays and tints become slightly raised dark surfaces.
  for (const h of ["#f4f4f5", "#eeeeee", "#e8f0fe", "#fef3c7", "#dcfce7", "#cccccc"]) {
    const d = darkBackground(hex(h))!;
    assert.ok(d, h);
    assert.ok(isDark(d), `${h} → ${formatColor(d)}`);
    assert.ok(contrast(d, WHITE) >= 7, `${h} → ${formatColor(d)} leaves room for text`);
    if (toOklch(hex(h)).c > 0.02) assert.ok(hueDelta(d, hex(h)) < 12, `${h} keeps its hue → ${formatColor(d)}`);
  }
  // Lighter in, darker out.
  assert.ok(toOklch(darkBackground(hex("#ffffff"))!).l < toOklch(darkBackground(hex("#e5e5e5"))!).l);
  // Alpha is kept.
  assert.equal(darkBackground(hex("#ffffff", 0.5))!.a, 0.5);
});

test("dark and vivid backgrounds keep their color", () => {
  for (const h of ["#1e3a8a", "#0b63ce", "#e11d48", "#7c3aed", "#15803d", "#1c1c1e", "#000000"]) {
    assert.equal(darkBackground(hex(h)), null, h);
  }
  assert.equal(darkBackground(hex("#ffffff", 0)), null);
  // A near-black neutral fill that sat on a light page (a black button) is
  // lifted off the canvas; vivid dark fills aren't.
  const lifted = darkBackground(hex("#000000"), true)!;
  assert.ok(contrast(lifted, DARK_CANVAS) > 1.3, formatColor(lifted));
  assert.ok(contrast(lifted, WHITE) > 7, formatColor(lifted));
  assert.equal(darkBackground(hex("#1e3a8a"), true), null);
});

test("text on a darkened background flips to the light side, readable", () => {
  const canvas = darkBackground(WHITE)!;
  for (const h of ["#000000", "#1b1c1f", "#333333", "#555555", "#6b7280", "#999999", "#0b63ce", "#b91c1c", "#15803d"]) {
    const fg = hex(h);
    // null: it already reads on the new background.
    const next = adjustText(fg, WHITE, canvas) ?? fg;
    const c = contrast(next, canvas);
    assert.ok(c >= 4.5 - 0.05, `${h} → ${formatColor(next)} (${c.toFixed(2)}:1)`);
    if (toOklch(fg).c > 0.05) assert.ok(hueDelta(next, fg) < 15, `${h} keeps its hue → ${formatColor(next)}`);
  }
  // The hierarchy survives: black headings stay brighter than gray body text.
  const heading = adjustText(hex("#000000"), WHITE, canvas)!;
  const body = adjustText(hex("#555555"), WHITE, canvas)!;
  assert.ok(contrast(heading, canvas) > contrast(body, canvas) + 1);
  // …and pure black doesn't become glaring pure white.
  assert.ok(contrast(heading, canvas) < 16);
});

test("text keeps its color where the background didn't change", () => {
  const blue = hex("#0b63ce");
  assert.equal(adjustText(WHITE, blue, blue), null);
  assert.equal(adjustText(hex("#fde68a"), blue, blue), null);
  // …or where it already reads on the new background.
  assert.equal(adjustText(hex("#ffffff"), hex("#1e3a8a"), DARK_CANVAS), null);
});

test("hidden text stays hidden", () => {
  const canvas = darkBackground(WHITE)!;
  const next = adjustText(hex("#ffffff"), WHITE, canvas)!;
  assert.ok(contrast(next, canvas) < 1.05, formatColor(next));
  const faint = adjustText(hex("#fafafa"), WHITE, canvas)!;
  assert.ok(contrast(faint, canvas) < 1.05, formatColor(faint));
});

test("borders keep a hairline's contrast, capped", () => {
  const canvas = darkBackground(WHITE)!;
  const hair = adjustBorder(hex("#e5e7eb"), WHITE, canvas)!;
  const c = contrast(hair, canvas);
  assert.ok(c > 1.1 && c < 1.6, `${formatColor(hair)} ${c.toFixed(2)}`);
  const rule = adjustBorder(hex("#000000"), WHITE, canvas)!;
  assert.ok(contrast(rule, canvas) <= 3.05, formatColor(rule));
  assert.ok(toOklch(rule).l > toOklch(canvas).l);
});

test("text targets are monotonic and floored at AA", () => {
  assert.equal(textTarget(1.5), 4.5);
  assert.equal(textTarget(4.5), 4.5);
  let prev = 0;
  for (let r = 4.5; r <= 21; r += 0.5) {
    const t = textTarget(r);
    assert.ok(t >= prev && t <= r, `${r} → ${t}`);
    prev = t;
  }
});

test("all-light gradients darken stop by stop; others are kept", () => {
  const g = darkGradient("linear-gradient(135deg, rgb(224, 242, 254) 0%, rgb(252, 231, 243) 100%)")!;
  assert.ok(g, "light gradient");
  assert.match(g.css, /^linear-gradient\(135deg, rgb\(\d+, \d+, \d+\) 0%, rgb\(\d+, \d+, \d+\) 100%\)$/);
  assert.ok(isDark(g.after) && !isDark(g.before));
  for (const keep of [
    "linear-gradient(rgb(91, 63, 214), rgb(31, 122, 224))",
    "linear-gradient(rgb(255, 255, 255), rgba(0, 0, 0, 0))",
    'url("data:image/png;base64,AAAA"), linear-gradient(rgb(255, 255, 255), rgb(250, 250, 250))',
    "none",
  ]) {
    assert.equal(darkGradient(keep), null, keep);
  }
});
