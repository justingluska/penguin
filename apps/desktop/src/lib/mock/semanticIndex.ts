// Search by meaning: semantic_status (model download and indexing progress)
// for the mock backend. OWNER: semantic.
// It follows the search stand-in's mode (./semantic.ts: ?semantic=ready |
// indexing | off), so Settings and search results agree, and the
// "semanticSearch" setting. Extra dev presets for screenshots:
//   ?semanticIndex=download | paused | error
import type { SemanticIndexState, SemanticIndexStatus, Settings } from "../types";
import type { MockHandler } from "./index";
import { mailHandlers } from "./mail";
import { semanticState } from "./semantic";

const TOTAL = 48_210;
const DOWNLOAD = 217_568_394;
const startedAt = Date.now();

function preset(): string | null {
  return typeof location === "undefined" ? null : new URLSearchParams(location.search).get("semanticIndex");
}

function base(state: SemanticIndexState, enabled: boolean): SemanticIndexStatus {
  return {
    state,
    enabled,
    indexed: 0,
    total: TOTAL,
    chunks: 0,
    model: "EmbeddingGemma 300M (4-bit)",
    modelId: "embeddinggemma-300m-q4@5090578d9565:256d:t256:c1",
    modelLicense: "Gemma Terms of Use",
    downloadDone: DOWNLOAD,
    downloadTotal: DOWNLOAD,
    pausedReason: null,
    error: null,
    indexRamBytes: 0,
    rate: 0,
  };
}

function withCount(s: SemanticIndexStatus, indexed: number): SemanticIndexStatus {
  const n = Math.min(TOTAL, Math.round(indexed));
  return { ...s, indexed: n, chunks: Math.round(n * 1.9), indexRamBytes: Math.round(n * 1.9 * 148) };
}

/** When the Welcome setup allowed the download (start_model_download or finishing it); null while it's held back. */
let downloadAllowedAt: number | null = null;

export async function mockSemanticIndexStatus(): Promise<SemanticIndexStatus> {
  const settings = (await mailHandlers.get_settings({})) as Settings;
  const search = semanticState();
  if (!settings.semanticSearch || search.semantic === "off") return base("off", false);
  const s = base("indexing", true);
  // ?welcome=1: a new install. No model until the setup asks, then a 30 s download (src-tauri/src/semantic).
  if (new URLSearchParams(typeof location === "undefined" ? "" : location.search).get("welcome") === "1") {
    if (downloadAllowedAt === null && settings.welcomeCompleted) downloadAllowedAt = Date.now();
    if (downloadAllowedAt === null) return { ...s, state: "paused", downloadDone: 0, pausedReason: "you finish setting up Penguin" };
    const done = Math.min(DOWNLOAD, ((Date.now() - downloadAllowedAt) / 30_000) * DOWNLOAD);
    if (done < DOWNLOAD) return { ...s, state: "downloading", downloadDone: done };
  }
  switch (preset()) {
    case "download": {
      const done = Math.min(DOWNLOAD, ((Date.now() - startedAt) / 20_000) * DOWNLOAD);
      return { ...s, state: done < DOWNLOAD ? "downloading" : "loading", downloadDone: done };
    }
    case "paused":
      return { ...withCount(s, TOTAL * 0.42), state: "paused", pausedReason: "Low Power Mode is on" };
    case "error":
      return { ...s, state: "error", error: "couldn't download the search model: network unreachable" };
  }
  if (search.semantic === "ready") return withCount({ ...s, state: "ready" }, TOTAL);
  return { ...withCount(s, TOTAL * (search.semanticProgress ?? 0)), rate: 12 };
}

export const semanticIndexHandlers: Record<string, MockHandler> = {
  semantic_status: () => mockSemanticIndexStatus(),
  start_model_download: () => {
    downloadAllowedAt ??= Date.now();
  },
};
