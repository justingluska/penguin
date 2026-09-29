// Demo mode's flag, localStorage stand-in and switch (src/lib/demoFlag.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { DEMO_KEY, MemoryStorage, readDemoFlag, shadowLocalStorage, switchDemoMode, writeDemoFlag, type SwitchDeps } from "../src/lib/demoFlag.ts";

test("the flag is off unless stored as \"1\", and unreadable storage means off", () => {
  const s = new MemoryStorage();
  assert.equal(readDemoFlag(s), false);
  s.setItem(DEMO_KEY, "true");
  assert.equal(readDemoFlag(s), false);
  s.setItem(DEMO_KEY, "1");
  assert.equal(readDemoFlag(s), true);
  assert.equal(readDemoFlag(null), false);
  const throwing = { getItem: () => { throw new Error("SecurityError"); } };
  assert.equal(readDemoFlag(throwing), false);
});

test("writing the flag sets and removes the key, and fails loudly without storage", () => {
  const s = new MemoryStorage();
  writeDemoFlag(s, true);
  assert.equal(s.getItem(DEMO_KEY), "1");
  writeDemoFlag(s, false);
  assert.equal(s.getItem(DEMO_KEY), null);
  assert.equal(s.length, 0);
  assert.throws(() => writeDemoFlag(null, true));
});

test("MemoryStorage behaves like Storage", () => {
  const s = new MemoryStorage();
  s.setItem("a", "1");
  s.setItem("b", 2 as unknown as string);
  assert.equal(s.length, 2);
  assert.equal(s.getItem("b"), "2");
  assert.equal(s.key(0), "a");
  assert.equal(s.key(5), null);
  s.removeItem("a");
  assert.equal(s.getItem("a"), null);
  s.clear();
  assert.equal(s.length, 0);
});

test("shadowing localStorage hides the real one and keeps it untouched", () => {
  const real = new MemoryStorage();
  real.setItem("penguin.search.recent", JSON.stringify(["from:boss"]));
  real.setItem(DEMO_KEY, "1");
  const win: { localStorage: Storage } = { localStorage: real };
  assert.equal(shadowLocalStorage(win), true);
  assert.notEqual(win.localStorage, real);
  assert.equal(win.localStorage.getItem("penguin.search.recent"), null);
  win.localStorage.setItem("penguin.search.recent", JSON.stringify(["demo query"]));
  assert.equal(real.getItem("penguin.search.recent"), JSON.stringify(["from:boss"]));
});

test("shadowing reports failure on a frozen global", () => {
  const win = Object.freeze({ localStorage: new MemoryStorage() });
  assert.equal(shadowLocalStorage(win), false);
});

function deps(over: Partial<SwitchDeps> = {}) {
  const storage = new MemoryStorage();
  const calls: string[] = [];
  const d: SwitchDeps = {
    storage,
    sendPending: () => false,
    flushDrafts: async () => {
      calls.push("flush");
    },
    reload: () => calls.push("reload"),
    ...over,
  };
  return { d, storage, calls };
}

test("entering demo mode saves drafts, then sets the flag, then reloads", async () => {
  const { d, storage, calls } = deps({
    reload: () => {
      calls.push(`reload:${storage.getItem(DEMO_KEY)}`);
    },
  });
  assert.equal(await switchDemoMode(true, false, d), "reloading");
  assert.deepEqual(calls, ["flush", "reload:1"]);
});

test("leaving demo mode clears the flag and reloads", async () => {
  const { d, storage, calls } = deps();
  storage.setItem(DEMO_KEY, "1");
  assert.equal(await switchDemoMode(false, true, d), "reloading");
  assert.equal(readDemoFlag(storage), false);
  assert.deepEqual(calls, ["flush", "reload"]);
});

test("asking for the current mode does nothing", async () => {
  const { d, calls, storage } = deps();
  assert.equal(await switchDemoMode(false, false, d), "unchanged");
  assert.equal(await switchDemoMode(true, true, d), "unchanged");
  assert.deepEqual(calls, []);
  assert.equal(storage.length, 0);
});

test("a send counting down blocks the switch: nothing is saved or reloaded", async () => {
  const { d, calls, storage } = deps({ sendPending: () => true });
  assert.equal(await switchDemoMode(true, false, d), "send-pending");
  assert.deepEqual(calls, []);
  assert.equal(readDemoFlag(storage), false);
});

test("storage that refuses the flag rejects without reloading", async () => {
  const { d, calls } = deps({ storage: null });
  await assert.rejects(switchDemoMode(true, false, d));
  assert.deepEqual(calls, ["flush"]);
});

// Demo mode's promise (no real backend calls) holds only while every command
// goes through call() and every event checks isMock first. These guard it.
const src = new URL("../src/", import.meta.url).pathname;
const apiSource = readFileSync(join(src, "lib/api.ts"), "utf8");

test("api.ts: raw invoke() only in call(), the log writer and window chrome (menu context, opening windows)", () => {
  const raw = apiSource.split("\n").filter((l) => /(?<![.\w])invoke</.test(l));
  assert.equal(raw.length, 5, raw.join("\n"));
  assert.ok(raw[0].includes("isMock ?"), "call() routes on isMock");
  assert.ok(raw[1].includes('"log_client_event"'));
  // Window chrome: no mail, accounts or settings; a demo's windows run on the mock too.
  for (const [line, cmd] of [
    [raw[2], "set_menu_context"],
    [raw[3], "open_window"],
    [raw[4], "take_window_seed"],
  ]) {
    assert.ok(line.includes(`"${cmd}"`) && line.includes("inTauri ?"), line);
  }
  const log = apiSource.slice(apiSource.indexOf("export function logClientEvent"));
  assert.ok(log.indexOf("if (isMock)") < log.indexOf('invoke<void>("log_client_event"'), "the log writer returns early in mock/demo mode");
});

test("api.ts: every event listener checks isMock before the real listen() (the native menu excepted)", () => {
  const lines = apiSource.split("\n");
  let checked = 0;
  lines.forEach((l, i) => {
    if (!/return listenHere</.test(l) || l.includes("EVENTS.menu")) return;
    assert.match(lines[i - 1], /if \(isMock\) return mockBackend\(\)/, `line ${i + 1}: ${l.trim()}`);
    checked++;
  });
  assert.ok(checked > 10);
  // The real listen() is called in two places only: listenHere (each window
  // hears the backend's events for it) and the window bus, whose events come
  // from Penguin's other windows, never from the backend.
  const real = lines.filter((l) => /(?<![.\w])listen</.test(l));
  assert.equal(real.length, 1, real.join("\n"));
  assert.ok(real[0].includes("target: thisWindowLabel"));
  const bus = apiSource.slice(apiSource.indexOf("export function onBusMessage"));
  assert.ok(bus.indexOf("listenHere<unknown>(BUS_EVENT") > 0 && bus.indexOf("listenHere<unknown>(BUS_EVENT") < bus.indexOf("\n}"));
});

test("no module outside api.ts calls the Tauri command or event API", () => {
  const files: string[] = [];
  const walk = (d: string) => {
    for (const f of readdirSync(d)) {
      const p = join(d, f);
      if (statSync(p).isDirectory()) walk(p);
      else if (/\.tsx?$/.test(f)) files.push(p);
    }
  };
  walk(src);
  const offenders = files.filter(
    (f) =>
      !f.endsWith("lib/api.ts") &&
      !f.includes("/lib/mock/") &&
      /@tauri-apps\/api\/(core|event)["']/.test(readFileSync(f, "utf8").replace(/import type[^;]+;/g, "")),
  );
  assert.deepEqual(
    offenders.map((f) => f.slice(src.length)),
    [],
  );
});

test("main.tsx loads lib/demo first, before anything can read localStorage", () => {
  const main = readFileSync(join(src, "main.tsx"), "utf8");
  const firstImport = main.split("\n").find((l) => l.startsWith("import "));
  assert.equal(firstImport, 'import "./lib/demo";');
});
