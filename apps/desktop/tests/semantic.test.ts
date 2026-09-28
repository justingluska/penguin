// Settings → Search → "Search by meaning": the progress line for every state.
import { test } from "node:test";
import assert from "node:assert/strict";
import { semanticLine } from "../src/features/settings/semanticLine.ts";
import type { SemanticIndexStatus } from "../src/lib/types.ts";

const base: SemanticIndexStatus = {
  state: "indexing",
  enabled: true,
  indexed: 12_000,
  total: 48_000,
  chunks: 23_000,
  model: "EmbeddingGemma 300M (4-bit)",
  modelId: "embeddinggemma-300m-q4@5090578d9565:256d:t256:c1",
  modelLicense: "Gemma Terms of Use",
  downloadDone: 0,
  downloadTotal: 217_568_394,
  pausedReason: null,
  error: null,
  indexRamBytes: 1_000_000,
  rate: 40,
};

test("progress line per state", () => {
  assert.equal(semanticLine(null), " ");
  assert.equal(semanticLine({ ...base, state: "off", enabled: false }), "Off. Search finds the words you type.");
  assert.match(semanticLine(base), /^Indexing, 25%: 12,000 of 48,000 messages\. Newest mail first/);
  assert.match(semanticLine({ ...base, state: "downloading", downloadDone: 108_000_000 }), /^Downloading the search model, .+ of .+ \(one time\)\.$/);
  assert.equal(semanticLine({ ...base, state: "loading" }), "Getting ready…");
  assert.equal(
    semanticLine({ ...base, state: "paused", pausedReason: "Low Power Mode is on" }),
    "Paused while Low Power Mode is on: 12,000 of 48,000 messages.",
  );
  assert.equal(semanticLine({ ...base, state: "ready", indexed: 48_000 }), "Ready: 48,000 of 48,000 messages.");
  assert.match(semanticLine({ ...base, state: "error", error: "network unreachable" }), /network unreachable\. Penguin will try again\.$/);
});

test("percent never exceeds 100 and survives an empty mailbox", () => {
  assert.match(semanticLine({ ...base, indexed: 10, total: 0 }), /^Indexing, 0%/);
  assert.match(semanticLine({ ...base, indexed: 60_000 }), /^Indexing, 100%/);
});
