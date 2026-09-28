// OWNER: richtext agent. Which signature an account uses and when it goes in
// by itself. No editor imports: the Composer (main bundle) uses these.
import type { Settings, Signature } from "../../../lib/types";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward";

/** The account's default signature, if it has one. */
export function defaultSignature(s: Pick<Settings, "signatures" | "signatureDefaults">, accountId: string): Signature | null {
  const id = s.signatureDefaults[accountId.toLowerCase()] ?? s.signatureDefaults[accountId];
  return (id && s.signatures.find((x) => x.id === id)) || null;
}

/** Whether a fresh message in this mode gets the signature by itself. */
export function autoInserts(s: Pick<Settings, "signatureInsert">, mode: ComposeMode): boolean {
  const i = s.signatureInsert;
  return mode === "new" ? i.newMessages : mode === "forward" ? i.forwards : i.replies;
}

/**
 * The account a reply or forward is held to, with lockReplyAccount on: the
 * one the mail came to. New messages are never held. Null = From is free.
 */
export function lockedAccount(
  s: Pick<Settings, "lockReplyAccount">,
  ctx: { mode: ComposeMode; thread?: { accountId: string } | null },
): string | null {
  return s.lockReplyAccount && ctx.mode !== "new" && ctx.thread ? ctx.thread.accountId : null;
}

/**
 * Make `sigId` the default for exactly `accountIds`: set it on those, and
 * clear it from any other account that had it. Keys are lowercased, as the
 * backend stores them.
 */
export function assignDefaults(defaults: Settings["signatureDefaults"], sigId: string, accountIds: string[]): Record<string, string> {
  const want = new Set(accountIds.map((a) => a.toLowerCase()));
  const next = Object.fromEntries(Object.entries(defaults).filter(([a, id]) => id !== sigId || want.has(a)));
  want.forEach((a) => (next[a] = sigId));
  return next;
}
