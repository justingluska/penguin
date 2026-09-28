// Answering invitations from the row chip and the thread card: the click
// shows at once (optimistic), the backend sends it (respond_to_invite), and
// a failure rolls back with a toast. Both views read the same overrides, so
// answering in one updates the other. Kept here, additive to app/store.ts:
// the list refreshes itself from the mail-changed the backend emits.
// OWNER: calendar agent.
import { useSyncExternalStore } from "react";
import type { EventResponse, InviteAnswer, InviteCard } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { toast } from "../../components/Toast";
import { openSettings } from "../settings/state";
import { invalidateCalendar } from "./state";
import { answerWord, inviteKey, type PendingAnswer } from "./invite";

const overrides = new Map<string, PendingAnswer>();
const subs = new Set<() => void>();
let version = 0;

function emit() {
  version++;
  subs.forEach((f) => f());
}

/** Re-renders on any answer change; returns the override for `key`. */
export function useInviteOverride(key: string | null): PendingAnswer | null {
  useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => version,
  );
  return key ? (overrides.get(key) ?? null) : null;
}

// The first answer sent by email explains itself once.
const HINT_KEY = "penguin.invites.emailHintSeen";
function hintSeen(): boolean {
  try {
    return localStorage.getItem(HINT_KEY) === "1";
  } catch {
    return false;
  }
}
function markHintSeen() {
  try {
    localStorage.setItem(HINT_KEY, "1");
  } catch {
    // Storage unavailable: the hint may show again next session.
  }
}
export function emailHintSeen(): boolean {
  return hintSeen();
}

export interface AnswerRequest {
  accountId: string;
  threadId: string;
  messageId: string | null;
  uid: string | null;
  /** The response on screen before the click (for the optimistic override). */
  current: EventResponse | null;
  response: InviteAnswer;
  comment?: string | null;
  proposal?: { start: number; end: number } | null;
  /** Who gets it, for the toast ("Priya"). */
  organizer?: string | null;
}

/**
 * Send an answer. Shows it at once; on failure puts back what was there and
 * says why. Resolves to the fresh card, or null when it failed.
 */
export async function answerInvite(req: AnswerRequest): Promise<InviteCard | null> {
  const key = inviteKey(req.accountId, { uid: req.uid, messageId: req.messageId ?? undefined });
  const prior = overrides.get(key);
  overrides.set(key, { response: req.response, baseline: req.current, pending: true });
  emit();
  try {
    const card = await api.respondToInvite({
      accountId: req.accountId,
      threadId: req.threadId,
      messageId: req.messageId,
      response: req.response,
      comment: req.comment ?? null,
      proposal: req.proposal ?? null,
    });
    overrides.set(key, { response: req.response, baseline: req.current, pending: false });
    emit();
    invalidateCalendar();
    const via = card.sent?.via ?? "email";
    const to = req.organizer ? ` to ${req.organizer}` : "";
    const what = req.proposal ? "New time proposed" : `Answered ${answerWord(req.response)}`;
    const first = via === "email" && !hintSeen();
    toast({
      kind: "success",
      key: `invite-${key}`,
      message: via === "calendar" ? `${what} · Google Calendar updated` : via === "graph" ? `${what} · sent through Outlook` : `${what} · emailed${to}`,
      detail: first ? "Sent as a standard calendar reply any calendar understands." : undefined,
      action: first && card.rsvpAvailable ? { label: "Turn on RSVP", run: () => openSettings("calendar") } : undefined,
      duration: first ? 9000 : undefined,
    });
    if (via === "email") markHintSeen();
    return card;
  } catch (err) {
    if (prior) overrides.set(key, prior);
    else overrides.delete(key);
    emit();
    const e = asCommandError(err);
    toast({ kind: "error", message: `Couldn't send your answer: ${e.message}`, key: `invite-${key}` });
    return null;
  }
}
