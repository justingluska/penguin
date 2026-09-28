// The "?" sheet lists every key (app/keyCatalog.ts): registry shortcuts with
// their hidden aliases and menu accelerators, menu-only keys, overlay keys.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { MENU_KEYS, OVERLAY_KEYS, menuAccelToKeys, menuKeysFromSource, sheetGroups } from "../src/app/keyCatalog.ts";
import type { Shortcut } from "../src/lib/keyboard.ts";

const run = () => {};
const sc = (id: string, keys: string, label: string, group: string, hidden = false): Shortcut => ({ id, keys, label, group, hidden, run });

const REGISTRY: Shortcut[] = [
  sc("nav.down", "j", "Next conversation", "Navigate"),
  sc("nav.down.arrow", "arrowdown", "Next conversation", "Navigate", true),
  sc("nav.first", "shift+k", "First conversation", "Navigate", true),
  sc("compose.reply", "r", "Reply", "Compose"),
  sc("triage.done", "e", "Mark done (archive)", "Triage"),
  sc("triage.move", "v", "Move to…", "Triage"),
  sc("triage.move.l", "l", "Move to…", "Triage", true),
  sc("person.close", "escape", "Close person card", "People", true),
  sc("floe.toggle", "mod+shift+f", "Floe mode (single column)", "App"),
  sc("floe.toggle.backslash", "\\", "Floe mode (single column)", "App", true),
];

const find = (groups: ReturnType<typeof sheetGroups>, label: string) =>
  groups.flatMap((g) => g.rows.map((r) => ({ ...r, group: g.name }))).find((r) => r.label === label);

test("hidden aliases join their action's row; unique hidden actions get a row; lone Esc closers don't", () => {
  const g = sheetGroups(REGISTRY, { mac: false });
  assert.deepEqual(find(g, "Next conversation")?.keys, ["j", "arrowdown"]);
  assert.deepEqual(find(g, "Move to…")?.keys, ["v", "l"]);
  assert.deepEqual(find(g, "First conversation")?.keys, ["shift+k"]);
  assert.deepEqual(find(g, "Floe mode (single column)")?.keys, ["mod+shift+f", "\\"]);
  assert.equal(find(g, "Close person card"), undefined);
});

test("on the Mac, menu accelerators are alternates and menu-only keys get rows", () => {
  const mac = sheetGroups(REGISTRY, { mac: true });
  assert.deepEqual(find(mac, "Reply")?.keys, ["r", "mod+r"]);
  assert.deepEqual(find(mac, "Mark done (archive)")?.keys, ["e", "ctrl+mod+a"]);
  // Floe's menu key is the same as its registry key: listed once.
  assert.deepEqual(find(mac, "Floe mode (single column)")?.keys, ["mod+shift+f", "\\"]);
  assert.equal(find(mac, "Zoom in")?.group, "Window");
  const other = sheetGroups(REGISTRY, { mac: false });
  assert.deepEqual(find(other, "Reply")?.keys, ["r"]);
  assert.equal(find(other, "Zoom in"), undefined);
});

test("overlay keys are listed, groups come in order, and the filter matches labels and keys", () => {
  const g = sheetGroups(REGISTRY, { mac: true });
  const names = g.map((x) => x.name);
  assert.ok(names.indexOf("Navigate") < names.indexOf("Triage"));
  assert.ok(names.indexOf("Compose") < names.indexOf("Compose window"));
  assert.deepEqual(find(g, "Send")?.keys, ["mod+enter"]);
  assert.ok(OVERLAY_KEYS.every((o) => o.rows.length > 0));
  const filtered = sheetGroups(REGISTRY, { mac: true, q: "reply" });
  assert.deepEqual(filtered.flatMap((x) => x.rows.map((r) => r.label)), ["Reply"]);
  assert.ok(sheetGroups(REGISTRY, { mac: false, q: "shift+k" }).some((x) => x.rows.some((r) => r.label === "First conversation")));
});

test("menu accelerators convert to registry syntax", () => {
  assert.equal(menuAccelToKeys("CmdOrCtrl+Shift+R"), "mod+shift+r");
  assert.equal(menuAccelToKeys("Ctrl+CmdOrCtrl+A"), "ctrl+mod+a");
  assert.equal(menuAccelToKeys("CmdOrCtrl+Backspace"), "mod+backspace");
  assert.equal(menuAccelToKeys("CmdOrCtrl+,"), "mod+,");
});

test("MENU_KEYS matches the menu bar in app_menu.rs", () => {
  const rust = readFileSync(new URL("../src-tauri/src/app_menu.rs", import.meta.url), "utf8");
  const fromSource = menuKeysFromSource(rust);
  assert.ok(Object.keys(fromSource).length >= 20, "parsed the menu specs");
  assert.deepEqual(MENU_KEYS, fromSource);
});
