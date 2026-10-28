// Handoff with the iPhone, the pure half (Apple's Handoff, not the
// window-closing handoffs in app/handoffs.ts): what a window offers, and what
// an activity handed over from the iPhone opens. tests/handoff.test.ts runs it.
// The contract and the native side: src-tauri/src/handoff.rs.
import type { AccountProvider, HandoffActivity, HandoffOpen, ThreadRef } from "../../lib/types";

/** A conversation on screen (the reading pane, the full view, a conversation window). */
export interface ShownThread {
  accountId: string;
  threadId: string;
  subject: string;
}

/** An open composer: its Gmail draft once autosave has made one (null before). */
export interface ShownDraft {
  accountId: string;
  draftId: string | null;
  /** The conversation it answers; null for a new message. */
  threadId: string | null;
  subject: string;
}

export interface WindowShows {
  /** The window is on screen (not minimized, hidden or fully covered). */
  visible: boolean;
  /** An open composer covers whatever conversation is behind it. */
  draft: ShownDraft | null;
  thread: ShownThread | null;
  accounts: { id: string; provider: AccountProvider }[];
}

const NONE: HandoffActivity = { kind: "none" };

/** The iPhone app only knows Gmail accounts: an IMAP or Microsoft account's ids mean nothing there. */
function isGmail(accounts: WindowShows["accounts"], accountId: string): boolean {
  return accounts.some((a) => a.id === accountId && a.provider === "gmail");
}

/** What this window offers: its composer's saved draft, else its conversation, else nothing. */
export function handoffActivityOf(s: WindowShows): HandoffActivity {
  if (!s.visible) return NONE;
  if (s.draft) {
    // A message not saved to Gmail yet has nothing the iPhone could open.
    const d = s.draft;
    if (!d.draftId || !isGmail(s.accounts, d.accountId)) return NONE;
    return { kind: "draft", accountId: d.accountId, draftId: d.draftId, threadId: d.threadId, subject: d.subject.trim() || null };
  }
  const t = s.thread;
  if (!t || !isGmail(s.accounts, t.accountId)) return NONE;
  return { kind: "conversation", accountId: t.accountId, threadId: t.threadId, subject: t.subject.trim() || null };
}

export const HANDOFF_MESSAGES = {
  conversationUnreadable: "That conversation couldn't be opened on this Mac",
  draftUnreadable: "That draft couldn't be opened on this Mac",
  conversationNotHere: "That conversation isn't on this Mac",
  draftNotHere: "That draft isn't on this Mac",
  conversationNotYet: "That conversation isn't on this Mac yet",
  draftGone: "That draft isn't in Gmail anymore",
} as const;

export type HandoffPlan =
  | { kind: "thread"; ref: ThreadRef }
  | { kind: "draft"; accountId: string; draftId: string }
  | { kind: "toast"; message: string; detail?: string };

/** What an activity from the iPhone opens here, given the accounts signed in on this Mac. */
export function handoffPlan(open: HandoffOpen, accountIds: string[]): HandoffPlan {
  if (open.kind === "unreadable") {
    return {
      kind: "toast",
      message: open.activity === "draft" ? HANDOFF_MESSAGES.draftUnreadable : HANDOFF_MESSAGES.conversationUnreadable,
      detail: "Penguin on your iPhone may be newer than on this Mac.",
    };
  }
  const account = open.accountId.toLowerCase();
  const here = accountIds.find((id) => id.toLowerCase() === account);
  if (open.kind === "conversation") {
    if (!here) return { kind: "toast", message: HANDOFF_MESSAGES.conversationNotHere, detail: `${open.accountId} isn't signed in here.` };
    return { kind: "thread", ref: { accountId: here, threadId: open.threadId } };
  }
  if (!here) return { kind: "toast", message: HANDOFF_MESSAGES.draftNotHere, detail: `${open.accountId} isn't signed in here.` };
  return { kind: "draft", accountId: here, draftId: open.draftId };
}

/** What opening a handed-over activity needs (features/handoff/index.ts passes the real ones). */
export interface OpenDeps {
  accountIds: string[];
  /** The conversation from the local store (null: not synced here). */
  getThread: (ref: ThreadRef) => Promise<{ subject: string } | null>;
  /** The draft from Gmail (null: sent or deleted). Rejects with the command error. */
  getDraft: (accountId: string, draftId: string) => Promise<{ draftId: string; draft: { subject: string } } | null>;
  openThread: (ref: ThreadRef) => void;
  openDraftWindow: (accountId: string, draftId: string, title: string) => Promise<void>;
  syncNow: () => void;
  toast: (t: { message: string; detail?: string; error?: boolean }) => void;
}

/** Open what the iPhone handed over, or say why not. Never throws. */
export async function openHandedOver(open: HandoffOpen, deps: OpenDeps): Promise<void> {
  const plan = handoffPlan(open, deps.accountIds);
  try {
    if (plan.kind === "toast") {
      deps.toast({ message: plan.message, detail: plan.detail });
    } else if (plan.kind === "thread") {
      if (!(await deps.getThread(plan.ref))) {
        // Mail written on the iPhone a moment ago may not have synced here yet.
        deps.syncNow();
        deps.toast({ message: HANDOFF_MESSAGES.conversationNotYet, detail: "Penguin is checking for new mail. Try again in a moment." });
        return;
      }
      deps.openThread(plan.ref);
    } else {
      // The content travels through Gmail: the draft as the iPhone last saved it.
      const opened = await deps.getDraft(plan.accountId, plan.draftId).catch((e: unknown) => {
        if ((e as { code?: string } | null)?.code === "notFound") return null;
        throw e;
      });
      if (!opened) {
        deps.toast({ message: HANDOFF_MESSAGES.draftGone, detail: "It may have been sent or deleted." });
        return;
      }
      await deps.openDraftWindow(plan.accountId, opened.draftId, opened.draft.subject.trim() || "New message");
    }
  } catch (e) {
    const message = (e as { message?: string } | null)?.message ?? String(e);
    deps.toast({ message: "Couldn't open what your iPhone handed over", detail: message, error: true });
  }
}

/**
 * Sends a window's activity to the backend when it changes (the same one
 * twice is sent once); a failed send is retried with the next report.
 */
export function createPublisher(send: (a: HandoffActivity) => Promise<void>): (a: HandoffActivity) => void {
  let last = "";
  return (a) => {
    const key = JSON.stringify(a);
    if (key === last) return;
    last = key;
    send(a).catch(() => {
      if (last === key) last = "";
    });
  };
}

/**
 * What one window shows, reported by the component that shows it. The last
 * component to report owns the slot; clearing only clears one's own report
 * (a pane unmounting late doesn't wipe the one that replaced it).
 */
export class ShownSlot<T> {
  private owner: object | null = null;
  private value: T | null = null;
  private subs = new Set<() => void>();

  get(): T | null {
    return this.value;
  }

  set(owner: object, value: T | null) {
    if (value === null) {
      if (this.owner !== owner) return;
      this.owner = null;
    } else {
      this.owner = owner;
    }
    this.value = value;
    this.subs.forEach((f) => f());
  }

  subscribe(f: () => void): () => void {
    this.subs.add(f);
    return () => {
      this.subs.delete(f);
    };
  }
}
