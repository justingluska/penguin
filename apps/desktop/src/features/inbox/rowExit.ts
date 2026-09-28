// How rows about to leave the list should animate out (features/inbox/listMotion.ts).
// Whoever removes rows can leave a hint first; the list reads it when it sees
// the rows go. No hint = a plain fade (snooze, Reply Later, move, rules…).
import type { ThreadRef } from "../../lib/types";
import { currentSettings } from "../../lib/settings";
import type { ExitStyle } from "./listMotion";

const hints = new Map<string, { exit: ExitStyle; at: number }>();
/** A hint older than this is stale (its removal never reached the list). */
const HINT_MS = 1_500;

export const rowKey = (r: ThreadRef) => "t:" + r.accountId + "\u0000" + r.threadId;

/** Hint the exit for rows a thread action is about to remove. */
export function hintExit(refs: readonly ThreadRef[], exit: ExitStyle) {
  const at = performance.now();
  for (const r of refs) hints.set(rowKey(r), { exit, at });
  if (hints.size > 2_000) for (const [k, h] of hints) if (at - h.at > HINT_MS) hints.delete(k);
}

/**
 * Keyboard/button archive and trash slide toward the side their swipe lives
 * on (Settings → swipe actions), so E and a swipe look like the same thing;
 * an action with no swipe fades.
 */
export function hintActionExit(refs: readonly ThreadRef[], action: string) {
  const s = currentSettings();
  const swipe = action === "archive" ? "archive" : action === "trash" ? "trash" : null;
  if (!swipe) return;
  const exit: ExitStyle =
    s.swipeLeft === swipe || s.swipeLeftLong === swipe ? "left" : s.swipeRight === swipe ? "right" : "fade";
  hintExit(refs, exit);
}

/** The exit for a row key (list rows are "t:<account>\0<thread>", group headers "g:<label>"). */
export function exitOf(key: string): ExitStyle {
  const h = hints.get(key);
  // Not consumed: React may render twice (StrictMode) and must get the same answer.
  if (!h) return "fade";
  return performance.now() - h.at < HINT_MS ? h.exit : "fade";
}
