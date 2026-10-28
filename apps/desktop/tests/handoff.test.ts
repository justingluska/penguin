// Handoff with the iPhone, the UI half (features/handoff/rules.ts): what a
// window offers, and what an activity handed over from the iPhone opens.
// The native half and the contract: src-tauri/src/handoff.rs.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createPublisher,
  handoffActivityOf,
  handoffPlan,
  HANDOFF_MESSAGES,
  openHandedOver,
  ShownSlot,
  type OpenDeps,
  type ShownDraft,
  type ShownThread,
  type WindowShows,
} from "../src/features/handoff/rules.ts";
import type { HandoffActivity, HandoffOpen, ThreadRef } from "../src/lib/types.ts";

const GMAIL = "sam@northwind.example";
const IMAP = "sam@harbor.example";
const accounts = [
  { id: GMAIL, provider: "gmail" as const },
  { id: IMAP, provider: "imap" as const },
];
const thread: ShownThread = { accountId: GMAIL, threadId: "18c2f0a1", subject: "Lunch on Friday" };
const draft = (over: Partial<ShownDraft> = {}): ShownDraft => ({ accountId: GMAIL, draftId: "r-1", threadId: null, subject: "Plans", ...over });
const shows = (over: Partial<WindowShows> = {}): WindowShows => ({ visible: true, draft: null, thread: null, accounts, ...over });

// ---------- what a window offers ----------

test("the conversation on screen is offered with its ids and subject", () => {
  assert.deepEqual(handoffActivityOf(shows({ thread })), {
    kind: "conversation",
    accountId: GMAIL,
    threadId: "18c2f0a1",
    subject: "Lunch on Friday",
  });
  assert.deepEqual(handoffActivityOf(shows({ thread: { ...thread, subject: "  " } })), {
    kind: "conversation",
    accountId: GMAIL,
    threadId: "18c2f0a1",
    subject: null,
  });
});

test("nothing is offered with nothing on screen or the window hidden", () => {
  assert.deepEqual(handoffActivityOf(shows()), { kind: "none" });
  assert.deepEqual(handoffActivityOf(shows({ thread, visible: false })), { kind: "none" });
  assert.deepEqual(handoffActivityOf(shows({ draft: draft(), visible: false })), { kind: "none" });
});

test("an open composer covers the conversation behind it, and offers its draft once saved", () => {
  assert.deepEqual(handoffActivityOf(shows({ thread, draft: draft({ threadId: "18c2f0a1", subject: "Re: Lunch on Friday" }) })), {
    kind: "draft",
    accountId: GMAIL,
    draftId: "r-1",
    threadId: "18c2f0a1",
    subject: "Re: Lunch on Friday",
  });
  // Not saved to Gmail yet: nothing (not the conversation behind it).
  assert.deepEqual(handoffActivityOf(shows({ thread, draft: draft({ draftId: null }) })), { kind: "none" });
  // A new message without a subject.
  assert.deepEqual(handoffActivityOf(shows({ draft: draft({ subject: "" }) })), {
    kind: "draft",
    accountId: GMAIL,
    draftId: "r-1",
    threadId: null,
    subject: null,
  });
});

test("only Gmail accounts are offered: the iPhone can't open the others", () => {
  assert.deepEqual(handoffActivityOf(shows({ thread: { ...thread, accountId: IMAP } })), { kind: "none" });
  assert.deepEqual(handoffActivityOf(shows({ draft: draft({ accountId: IMAP }) })), { kind: "none" });
  // An account that's gone (signed out meanwhile).
  assert.deepEqual(handoffActivityOf(shows({ thread: { ...thread, accountId: "old@gone.example" } })), { kind: "none" });
});

test("only what changed is sent, and a failed send is tried again", async () => {
  const sent: HandoffActivity[] = [];
  let fail = false;
  const publish = createPublisher((a) => {
    sent.push(a);
    return fail ? Promise.reject(new Error("ipc")) : Promise.resolve();
  });
  const conv: HandoffActivity = { kind: "conversation", accountId: GMAIL, threadId: "t", subject: null };
  publish(conv);
  publish({ ...conv });
  assert.equal(sent.length, 1);
  publish({ kind: "none" });
  assert.equal(sent.length, 2);
  fail = true;
  publish(conv);
  await new Promise((r) => setTimeout(r, 0));
  fail = false;
  publish(conv);
  assert.equal(sent.length, 4, "the failed one goes again");
});

test("a pane's report is cleared only by its own owner", () => {
  const slot = new ShownSlot<ShownThread>();
  let changes = 0;
  slot.subscribe(() => changes++);
  const a = {};
  const b = {};
  slot.set(a, thread);
  slot.set(b, { ...thread, threadId: "t2" });
  // The first pane unmounts after the second has reported: the second stays.
  slot.set(a, null);
  assert.equal(slot.get()?.threadId, "t2");
  slot.set(b, null);
  assert.equal(slot.get(), null);
  assert.equal(changes, 3);
});

// ---------- what an activity from the iPhone opens ----------

test("a conversation or a draft of a signed-in account opens", () => {
  const conv: HandoffOpen = { kind: "conversation", accountId: "Sam@Northwind.example", threadId: "t1" };
  assert.deepEqual(handoffPlan(conv, [GMAIL]), { kind: "thread", ref: { accountId: GMAIL, threadId: "t1" } });
  const d: HandoffOpen = { kind: "draft", accountId: GMAIL, draftId: "r-9", threadId: "t1" };
  assert.deepEqual(handoffPlan(d, [GMAIL]), { kind: "draft", accountId: GMAIL, draftId: "r-9" });
});

test("an account not on this Mac, or an activity this Mac can't read, says so", () => {
  const conv = handoffPlan({ kind: "conversation", accountId: "kai@elsewhere.example", threadId: "t" }, [GMAIL]);
  assert.equal(conv.kind === "toast" && conv.message, HANDOFF_MESSAGES.conversationNotHere);
  const d = handoffPlan({ kind: "draft", accountId: "kai@elsewhere.example", draftId: "r", threadId: null }, [GMAIL]);
  assert.equal(d.kind === "toast" && d.message, HANDOFF_MESSAGES.draftNotHere);
  const u = handoffPlan({ kind: "unreadable", activity: "draft" }, [GMAIL]);
  assert.equal(u.kind === "toast" && u.message, HANDOFF_MESSAGES.draftUnreadable);
  const uc = handoffPlan({ kind: "unreadable", activity: "conversation" }, [GMAIL]);
  assert.equal(uc.kind === "toast" && uc.message, HANDOFF_MESSAGES.conversationUnreadable);
});

function deps(over: Partial<OpenDeps> = {}) {
  const log: string[] = [];
  const d: OpenDeps = {
    accountIds: [GMAIL],
    getThread: async () => ({ subject: "Lunch" }),
    getDraft: async (_a, draftId) => ({ draftId, draft: { subject: "Plans" } }),
    openThread: (ref: ThreadRef) => log.push(`thread ${ref.accountId}/${ref.threadId}`),
    openDraftWindow: async (accountId, draftId, title) => {
      log.push(`draft ${accountId}/${draftId} "${title}"`);
    },
    syncNow: () => log.push("sync"),
    toast: (t) => log.push(`toast ${t.message}`),
    ...over,
  };
  return { d, log };
}

test("a conversation opens in the main window; one not synced yet syncs and says so", async () => {
  const ok = deps();
  await openHandedOver({ kind: "conversation", accountId: GMAIL, threadId: "t1" }, ok.d);
  assert.deepEqual(ok.log, [`thread ${GMAIL}/t1`]);
  const missing = deps({ getThread: async () => null });
  await openHandedOver({ kind: "conversation", accountId: GMAIL, threadId: "t1" }, missing.d);
  assert.deepEqual(missing.log, ["sync", `toast ${HANDOFF_MESSAGES.conversationNotYet}`]);
});

test("a draft opens in a compose window, as Gmail has it now", async () => {
  const ok = deps();
  await openHandedOver({ kind: "draft", accountId: GMAIL, draftId: "r-9", threadId: null }, ok.d);
  assert.deepEqual(ok.log, [`draft ${GMAIL}/r-9 "Plans"`]);
  const untitled = deps({ getDraft: async (_a, draftId) => ({ draftId, draft: { subject: " " } }) });
  await openHandedOver({ kind: "draft", accountId: GMAIL, draftId: "r-9", threadId: null }, untitled.d);
  assert.deepEqual(untitled.log, [`draft ${GMAIL}/r-9 "New message"`]);
});

test("a draft sent or deleted meanwhile says so; other failures are reported, never thrown", async () => {
  for (const getDraft of [async () => null, async () => Promise.reject({ code: "notFound", message: "gone" })]) {
    const gone = deps({ getDraft });
    await openHandedOver({ kind: "draft", accountId: GMAIL, draftId: "r-9", threadId: null }, gone.d);
    assert.deepEqual(gone.log, [`toast ${HANDOFF_MESSAGES.draftGone}`]);
  }
  const offline = deps({ getDraft: async () => Promise.reject({ code: "network", message: "offline" }) });
  await openHandedOver({ kind: "draft", accountId: GMAIL, draftId: "r-9", threadId: null }, offline.d);
  assert.deepEqual(offline.log, ["toast Couldn't open what your iPhone handed over"]);
  const notHere = deps();
  await openHandedOver({ kind: "conversation", accountId: "kai@elsewhere.example", threadId: "t" }, notHere.d);
  assert.deepEqual(notHere.log, [`toast ${HANDOFF_MESSAGES.conversationNotHere}`]);
});
