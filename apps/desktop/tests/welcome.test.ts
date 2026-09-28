// Welcome setup rules (src/features/welcome/model.ts): when it opens by
// itself, which screens it has, and when the search model may download.
// The backend half (the flag's migration, the download gate) is tested in
// src-tauri/src/settings.rs and src-tauri/src/semantic/mod.rs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { shouldAutoOpen, startsDownload, welcomeScreens } from "../src/features/welcome/model.ts";
import { semanticLine } from "../src/features/settings/semanticLine.ts";
import type { SemanticIndexStatus } from "../src/lib/types.ts";

test("opens by itself only on a new install with an account, on the desktop", () => {
  assert.equal(shouldAutoOpen({ welcomeCompleted: false }, 1), true);
  assert.equal(shouldAutoOpen({ welcomeCompleted: true }, 1), false, "finished, skipped, or an existing install");
  assert.equal(shouldAutoOpen({ welcomeCompleted: false }, 0), false, "not before the first account");
});

test("the download question comes first; Apple Intelligence only when available", () => {
  const ids = (a: boolean | null) => welcomeScreens(a).map((s) => s.id);
  assert.deepEqual(ids(true), ["search", "ai", "mail", "inbox", "look", "keys"]);
  assert.deepEqual(ids(false), ["search", "mail", "inbox", "look", "keys"]);
  assert.deepEqual(ids(null), ["search", "mail", "inbox", "look", "keys"], "still checking: no screen that may vanish");
  assert.ok(ids(true).length >= 4 && ids(true).length <= 6);
});

test("the model download starts only after the search screen, and only if kept on", () => {
  assert.equal(startsDownload("search", true), true);
  assert.equal(startsDownload("search", false), false);
  for (const s of ["ai", "mail", "inbox", "look", "keys"] as const) assert.equal(startsDownload(s, true), false);
});

test("Settings says why the model is waiting on a new install", () => {
  const s: SemanticIndexStatus = {
    state: "paused",
    enabled: true,
    indexed: 0,
    total: 1200,
    chunks: 0,
    model: "EmbeddingGemma 300M (4-bit)",
    modelId: "x",
    modelLicense: "Gemma Terms of Use",
    downloadDone: 0,
    downloadTotal: 217_000_000,
    pausedReason: "you finish setting up Penguin",
    error: null,
    indexRamBytes: 0,
    rate: 0,
  };
  assert.match(semanticLine(s), /^Paused while you finish setting up Penguin/);
});
