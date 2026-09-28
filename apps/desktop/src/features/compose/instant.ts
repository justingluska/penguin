// Instant replies: your own one-liners (Settings → Compose), plus up to three
// replies Apple's on-device model suggests for the conversation when that's
// turned on (Settings → AI). They show in the reply composer while nothing is
// written yet (⌃1–⌃9 or a click puts one in; ⌘↵ then sends it, with the usual
// undo window), and in the thread's reply box, where a click opens the reply
// with it. Pure helpers here; the row is InstantReplies.tsx.
import type { InstantReplies } from "../../lib/types";

export interface InstantChoice {
  /** 1–9: ⌃n picks it. */
  n: number;
  text: string;
  /** Suggested by the on-device model (not one of yours). */
  ai: boolean;
}

/** The choices, numbered: yours first, then the model's (skipping ones you already have), nine at most. */
export function instantChoices(settings: InstantReplies, suggestions: string[] = []): InstantChoice[] {
  const out: InstantChoice[] = [];
  const seen = new Set<string>();
  const push = (text: string, ai: boolean) => {
    const t = text.trim();
    const k = t.toLowerCase().replace(/[.!?\s]+$/, "");
    if (!t || seen.has(k) || out.length >= 9) return;
    seen.add(k);
    out.push({ n: out.length + 1, text: t, ai });
  };
  if (settings.enabled) settings.replies.forEach((r) => push(r, false));
  if (settings.enabled && settings.aiSuggestions) suggestions.forEach((r) => push(r, true));
  return out;
}

/** Whether the model's suggestions should be asked for: on, a reply, and nothing written yet. */
export function wantsSuggestions(settings: InstantReplies, mode: string, writing: string, available: boolean): boolean {
  return settings.enabled && settings.aiSuggestions && available && (mode === "reply" || mode === "replyAll") && writing.trim() === "";
}

// ---------------------------------------------------------------------------
// What the next composer should do when it opens: start with an instant reply
// (the reply box, ⌘K) or open Write with AI (⌘K "Reply with AI").
// ---------------------------------------------------------------------------

export interface ComposeIntent {
  text?: string;
  ai?: boolean;
}

let intent: (ComposeIntent & { at: number; takenAt: number | null }) | null = null;

export function setComposeIntent(i: ComposeIntent) {
  intent = { ...i, at: Date.now(), takenAt: null };
}

/**
 * The intent set in the last few seconds, for the composer that opens next.
 * Taking it again within a second returns it again (React may run a
 * component's first render twice); after that it's gone.
 */
export function takeComposeIntent(now = Date.now()): ComposeIntent | null {
  const i = intent;
  if (!i || now - i.at > 5000 || (i.takenAt !== null && now - i.takenAt > 1000)) {
    intent = null;
    return null;
  }
  i.takenAt ??= now;
  return { text: i.text, ai: i.ai };
}
