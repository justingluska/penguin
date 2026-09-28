// Shortcut conflict resolution (src/lib/keyboard.ts pickShortcut).
import { test } from "node:test";
import assert from "node:assert/strict";
import { pickShortcut, type Shortcut } from "../src/lib/keyboard.ts";

const sc = (id: string, extra: Partial<Shortcut> = {}): Shortcut => ({ id, keys: "escape", label: id, run: () => {}, ...extra });
const yes = () => true;

test("context-specific beats global, later registration wins ties", () => {
  assert.equal(pickShortcut([sc("global"), sc("ctx", { when: yes })])?.id, "ctx");
  assert.equal(pickShortcut([sc("ctx", { when: yes }), sc("global")])?.id, "ctx");
  assert.equal(pickShortcut([sc("a", { when: yes }), sc("b", { when: yes })])?.id, "b");
  assert.equal(pickShortcut([]), undefined);
});

test("a fallback runs only when nothing else claims the key", () => {
  const leave = sc("floe.leave", { when: yes, fallback: true });
  // Registered last and context-specific, it still loses to any other live shortcut…
  assert.equal(pickShortcut([sc("nav.back", { when: yes }), leave])?.id, "nav.back");
  assert.equal(pickShortcut([sc("global"), leave])?.id, "global");
  // …and wins when it is alone.
  assert.equal(pickShortcut([leave])?.id, "floe.leave");
  // Two fallbacks resolve by the usual rules.
  assert.equal(pickShortcut([sc("f1", { fallback: true }), sc("f2", { fallback: true, when: yes })])?.id, "f2");
});
