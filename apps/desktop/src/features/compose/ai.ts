// Write with AI: the composer's side of the on-device writer
// (src-tauri/src/writing/, docs/COMPOSE-SPEED.md). Apple's Foundation Models
// on this Mac; nothing leaves it. The action hides when the Mac can't run it
// or it's turned off (Settings → AI), like thread summaries.
import { api, asCommandError, onWriteProgress } from "../../lib/api";
import { useSetting } from "../../lib/settings";
import type { WriteRequest } from "../../lib/types";
import { refreshAvailability, useAvailability } from "../summary/state";

export { WRITE_PRESETS, buildRequest, isEditInstruction, promptPlaceholder, type WritePreset } from "./writePrompt";

/** Write with AI can run: turned on, and Apple Intelligence is available on this Mac. */
export function useWriterReady(): boolean {
  const on = useSetting("writeWithAi");
  const availability = useAvailability();
  return on && availability?.available === true;
}

/** Apple Intelligence is available on this Mac (whatever the settings say). */
export function useAiAvailable(): boolean {
  return useAvailability()?.available === true;
}

/** Ask again (Apple Intelligence may have finished downloading since). */
export function checkWriter(): void {
  void refreshAvailability();
}

let seq = 0;
export function newRunId(): string {
  seq += 1;
  return `w${Date.now().toString(36)}${seq}`;
}

/**
 * One run: streams the text so far to `onText` and resolves with the final
 * text. Rejects with the backend's error ({code: "cancelled"} after cancel).
 */
export async function runWrite(request: WriteRequest, onText: (text: string) => void): Promise<string> {
  const unlisten = await onWriteProgress((p) => {
    if (p.runId === request.runId) onText(p.text);
  });
  try {
    const done = await api.writeWithAi(request);
    return done.text;
  } finally {
    unlisten();
  }
}

export function cancelWrite(runId: string): void {
  api.cancelWrite(runId).catch(() => {
    // Already finished: nothing to stop.
  });
}

/** What the bar says when a run fails (cancelled runs say nothing). */
export function writeErrorText(e: unknown): string | null {
  const err = asCommandError(e);
  if (err.code === "cancelled") return null;
  return err.message || "Couldn't write that on this Mac.";
}
