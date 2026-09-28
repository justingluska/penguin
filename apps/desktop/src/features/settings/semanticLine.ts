// The quiet progress line under Settings → Search → "Search by meaning".
// Pure (tests/semantic.test.ts).
import type { SemanticIndexStatus } from "../../lib/types";
import { bytes, num } from "../../lib/format.ts";

export function semanticLine(s: SemanticIndexStatus | null): string {
  if (!s) return " ";
  const pct = (a: number, b: number) => (b > 0 ? Math.min(100, Math.floor((a / b) * 100)) : 0);
  const count = `${num(s.indexed)} of ${num(s.total)} messages`;
  switch (s.state) {
    case "off":
      return "Off. Search finds the words you type.";
    case "downloading":
      return `Downloading the search model, ${bytes(s.downloadDone)} of ${bytes(s.downloadTotal)} (one time).`;
    case "loading":
      return "Getting ready…";
    case "indexing":
      return `Indexing, ${pct(s.indexed, s.total)}%: ${count}. Newest mail first; search already uses what's done.`;
    case "paused":
      return `Paused while ${s.pausedReason ?? "the Mac is busy"}: ${count}.`;
    case "ready":
      return `Ready: ${count}.`;
    case "error":
      return `Not working right now: ${s.error ?? "unknown error"}. Penguin will try again.`;
  }
}
