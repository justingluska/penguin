// Write with AI: what the bar sends (pure; tests/composeSpeed.test.ts). The
// prompts themselves are built in Rust (penguin-core writing.rs); this only
// decides the action and trims the inputs.
import type { AiUnavailableReason, WriteAction, WriteRequest } from "../../lib/types";

/** Why Write with AI and suggested replies are hidden (Settings → AI says it; the composer just hides them). */
export function writerUnavailableText(reason: AiUnavailableReason | null): string {
  switch (reason) {
    case null:
      return "Available on this Mac.";
    case "deviceNotEligible":
      return "Needs a Mac that can run Apple Intelligence (Apple silicon, M1 or later).";
    case "appleIntelligenceNotEnabled":
      return "Turn on Apple Intelligence in System Settings → Apple Intelligence & Siri to use this.";
    case "modelNotReady":
      return "Apple Intelligence is still getting ready on this Mac (downloading its model). This works once it's done.";
    case "osTooOld":
      return "Needs macOS 26 or later with Apple Intelligence.";
    case "unsupportedPlatform":
      return "Runs on a Mac with Apple Intelligence.";
    case "notBuilt":
      return "This build of Penguin was made without Apple's Foundation Models SDK (Xcode 26 or later).";
    case "unknown":
      return "Apple Intelligence isn't available on this Mac right now.";
  }
}

export interface WritePreset {
  action: Exclude<WriteAction, "draft" | "custom">;
  label: string;
  /** 1–4 pick it while the bar is open and its field is empty. */
  key: string;
}

/** Gmail's Help me write refines with Formalize / Friendly / Shorten; Apple's Writing Tools add Proofread. */
export const WRITE_PRESETS: WritePreset[] = [
  { action: "shorter", label: "Shorter", key: "1" },
  { action: "friendlier", label: "Friendlier", key: "2" },
  { action: "formal", label: "More formal", key: "3" },
  { action: "grammar", label: "Fix spelling & grammar", key: "4" },
];

/** The placeholder of the bar's field for what it will work on. */
export function promptPlaceholder(scope: "selection" | "writing", hasText: boolean, reply: boolean): string {
  if (scope === "selection") return "Describe a change to the selection…";
  if (hasText) return "Describe a change, or what to add…";
  return reply ? "What should the reply say? e.g. yes to Thursday, ask for the agenda" : "What should it say?";
}

/**
 * "make it warmer", "shorten the second paragraph", "translate to Spanish":
 * a change to what's written, not a new message. Anything else with text
 * already written drafts with that text as the starting point.
 */
export function isEditInstruction(s: string): boolean {
  return /^(make|rewrite|rephrase|reword|shorten|lengthen|expand|simplify|tighten|soften|translate|turn|change|fix|polish|improve|edit|remove|cut|trim|add a|sound|less|more)\b/i.test(
    s.trim(),
  );
}

/** The request for a preset or a typed instruction. */
export function buildRequest(opts: {
  runId: string;
  accountId: string;
  threadId: string | null;
  preset: WritePreset["action"] | null;
  instruction: string;
  scope: "selection" | "writing";
  text: string;
  subject: string;
  recipients: string[];
  myName: string;
}): WriteRequest {
  const instruction = opts.instruction.trim().slice(0, 500);
  // A typed instruction changes a selection, or the whole message when it
  // reads like an edit; otherwise it writes the message.
  const edit = opts.scope === "selection" || (opts.text.trim() !== "" && isEditInstruction(instruction));
  const action: WriteAction = opts.preset ?? (edit ? "custom" : "draft");
  return {
    runId: opts.runId,
    accountId: opts.accountId,
    threadId: opts.threadId,
    action,
    instruction: opts.preset ? "" : instruction,
    text: opts.text.slice(0, 12_000),
    subject: opts.subject,
    recipients: opts.recipients.slice(0, 20),
    myName: opts.myName,
  };
}
