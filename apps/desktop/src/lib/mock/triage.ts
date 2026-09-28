// Mock Reply Later and Follow up (features/triage), mirroring the backend:
// Reply Later = the account's "Reply Later" label (created on first use) +
// archive + read; sending in the thread takes the label off. Follow up =
// threads whose latest message is yours, sent between 60 and N days ago, not
// only to machines or yourself, not snoozed or dismissed; oldest first.
// Seeds a few of each with fictional .example people.
import type { Address, ListQuery, MessageView, ThreadAction, ThreadRef, ThreadSummary, TriageCount } from "../types";
import { FOLLOW_UP_DAYS, FOLLOW_UP_LOOKBACK_DAYS, REPLY_LATER_LABEL } from "../types";
import type { MockHandler } from "./index";
import { mockBackend } from "./index";
import { ensureMockLabel, mailHandlers, mockMail, mockViews, replyLaterLabelId } from "./mail";
import { snoozeOf } from "./snoozeState";
import { isAutomatedAddress } from "./automated";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const k = (accountId: string, threadId: string) => `${accountId}:${threadId}`;

/** thread → date of the sent message that was dismissed. */
const dismissed = new Map<string, number>();

function settingsDays(): number {
  const s = mailHandlers.get_settings({}) as { followUpDays?: number };
  const d = Number(s.followUpDays);
  return Number.isFinite(d) ? Math.min(FOLLOW_UP_DAYS.max, Math.max(FOLLOW_UP_DAYS.min, Math.round(d))) : FOLLOW_UP_DAYS.def;
}

const ownEmails = () => new Set(mockMail.accounts().map((a) => a.email.toLowerCase()));

function inScope(accountId: string, one: string | null | undefined, set: string[] | null | undefined) {
  return (!one || one === accountId) && (!set || set.includes(accountId));
}

/** The awaited sent message's date, or null when the thread isn't waiting on anyone. */
function awaiting(s: ThreadSummary, own: Set<string>): number | null {
  const has = (l: string) => s.labelIds.includes(l);
  if (!has("SENT") || has("TRASH") || has("SPAM")) return null;
  const v = mockMail.thread(s.accountId, s.threadId);
  if (!v) return null;
  const live = v.messages.filter((m) => !m.labelIds.includes("DRAFT") && !m.labelIds.includes("TRASH"));
  const last = live[live.length - 1];
  if (!last || !own.has(last.from.email.toLowerCase())) return null;
  const to = [...last.to, ...last.cc, ...last.bcc].map((a) => a.email.toLowerCase());
  if (to.length && to.filter((e) => !own.has(e)).every(isAutomatedAddress)) return null;
  return last.date;
}

function followUps(q: { accountId?: string | null; accountIds?: string[] | null; unreadOnly?: boolean }, days: number): ThreadSummary[] {
  const now = Date.now();
  const hi = now - days * DAY;
  const lo = now - FOLLOW_UP_LOOKBACK_DAYS * DAY;
  const own = ownEmails();
  const out: ThreadSummary[] = [];
  for (const s of mockMail.threads()) {
    if (!inScope(s.accountId, q.accountId, q.accountIds)) continue;
    if (q.unreadOnly && !s.unread) continue;
    if (snoozeOf(s.accountId, s.threadId)) continue;
    const sent = awaiting(s, own);
    if (sent === null || sent >= hi || sent < lo) continue;
    if ((dismissed.get(k(s.accountId, s.threadId)) ?? -Infinity) >= sent) continue;
    out.push({ ...s, labelIds: [...s.labelIds], participants: [...s.participants], lastDate: sent, snoozedUntil: null });
  }
  return out.sort((a, b) => a.lastDate - b.lastDate);
}

mockViews.followUp = (q: ListQuery) => (q.before != null ? [] : followUps(q, settingsDays()));

function emitChanged(accountId: string, threadIds: string[]) {
  mockBackend.emit("penguin://mail-changed", { accountId, threadIds });
}

function replyLater(refs: ThreadRef[], on: boolean) {
  const by = new Map<string, ThreadRef[]>();
  for (const r of refs) by.set(r.accountId, [...(by.get(r.accountId) ?? []), r]);
  for (const [accountId, rs] of by) {
    const labelId = on ? ensureMockLabel(accountId, REPLY_LATER_LABEL) : replyLaterLabelId(accountId);
    if (!labelId) continue;
    const action: ThreadAction = on ? { kind: "replyLater", labelId } : { kind: "removeLabel", labelId };
    mailHandlers.modify_threads({ targets: rs, action });
  }
}

export const triageHandlers: Record<string, MockHandler> = {
  reply_later: ({ targets, on }) => replyLater(targets as ThreadRef[], !!on),
  triage_counts: ({ accountIds }): TriageCount[] => {
    const ids = mockMail.accounts().map((a) => a.id).filter((id) => !accountIds || (accountIds as string[]).includes(id));
    const days = settingsDays();
    const follow = followUps({ accountIds: ids }, days);
    return ids.map((accountId) => {
      const rl = replyLaterLabelId(accountId);
      const replyLater = rl
        ? mockMail.threads().filter((s) => s.accountId === accountId && s.labelIds.includes(rl) && !s.labelIds.includes("TRASH") && !s.labelIds.includes("SPAM")).length
        : 0;
      return { accountId, replyLater, followUp: follow.filter((s) => s.accountId === accountId).length };
    });
  },
  dismiss_follow_ups: ({ targets, dismissed: on }) => {
    const own = ownEmails();
    for (const r of targets as ThreadRef[]) {
      if (!on) {
        dismissed.delete(k(r.accountId, r.threadId));
        continue;
      }
      const s = mockMail.summary(r.accountId, r.threadId);
      const sent = s ? awaiting(s, own) : null;
      if (sent !== null) dismissed.set(k(r.accountId, r.threadId), sent);
    }
    const by = new Map<string, string[]>();
    for (const r of targets as ThreadRef[]) by.set(r.accountId, [...(by.get(r.accountId) ?? []), r.threadId]);
    for (const [accountId, threadIds] of by) emitChanged(accountId, threadIds);
  },
};

/** send_message: a reply takes its conversation out of Reply Later (like reply_later::after_send). */
export function withReplyLaterClear(send: MockHandler): MockHandler {
  return async (args) => {
    const out = (await send(args)) as { threadId?: string } | undefined;
    const draft = args.draft as { accountId: string; replyToThreadId?: string | null };
    const rl = replyLaterLabelId(draft.accountId);
    const threads = [...new Set([draft.replyToThreadId, out?.threadId].filter((t): t is string => !!t))];
    const hit = threads.filter((t) => rl && mockMail.summary(draft.accountId, t)?.labelIds.includes(rl));
    if (rl && hit.length) mailHandlers.modify_threads({ targets: hit.map((threadId) => ({ accountId: draft.accountId, threadId })), action: { kind: "removeLabel", labelId: rl } });
    return out;
  };
}

// ---------------------------------------------------------------------------
// Seeds
// ---------------------------------------------------------------------------
const ACCOUNTS: Record<string, string> = {
  "acc-northwind": "sam@northwind.example",
  "acc-harbor": "sam@harbor-labs.example",
  "acc-personal": "sam.okafor@gmail.example",
  "acc-okafor": "sam@okafor.example",
};
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function msg(accountId: string, threadId: string, i: number, from: Address, to: Address[], subject: string, body: string, date: number, labels: string[]): MessageView {
  return {
    accountId,
    id: `${threadId}-m${i}`,
    threadId,
    date,
    from,
    to,
    cc: [],
    bcc: [],
    replyTo: [],
    subject,
    snippet: body.slice(0, 140),
    bodyText: body,
    html:
      `<!doctype html><html><head><meta charset="utf-8"><style>:root{color-scheme:light dark}` +
      `body{margin:0;font:14px/22px Inter,system-ui,sans-serif;color:CanvasText}</style></head><body>` +
      body.split("\n\n").map((p) => `<p>${esc(p)}</p>`).join("") +
      `</body></html>`,
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: labels,
    attachments: [],
    unread: false,
    starred: false,
    senderAuthenticated: true,
    otp: null,
  };
}

interface Waiting {
  accountId: string;
  id: string;
  subject: string;
  them: Address;
  /** Their message first (a reply), else you started the thread. */
  theirs?: { body: string; daysAgo: number };
  mine: string;
  daysAgo: number;
}

const me = (accountId: string): Address => ({ name: "Sam Okafor", email: ACCOUNTS[accountId] });

const WAITING: Waiting[] = [
  {
    accountId: "acc-northwind",
    id: "t-fu-redlines",
    subject: "Contract redlines for the Q4 renewal",
    them: { name: "Priya Raman", email: "priya@lumen-legal.example" },
    theirs: { body: "Hi Sam, attached are our redlines on sections 4 and 9. Can you confirm the renewal term before Friday?", daysAgo: 7 },
    mine: "Thanks Priya. Sections 4 and 9 look fine on our side. One question on the renewal term: can we do 24 months with a 60-day notice window? Let me know and I'll send the countersigned copy.",
    daysAgo: 5,
  },
  {
    accountId: "acc-harbor",
    id: "t-fu-intro",
    subject: "Intro: Ana ↔ Marco (sensor calibration)",
    them: { name: "Marco Ferri", email: "marco@ferri-instruments.example" },
    mine: "Marco, meet Ana, who leads calibration at Harbor Labs. Ana, Marco built the drift model I mentioned. I think a 20-minute call would save you both a week. I'll let you take it from here.",
    daysAgo: 9,
  },
  {
    accountId: "acc-personal",
    id: "t-fu-cabin",
    subject: "Deposit for the lake cabin, Oct 17–19",
    them: { name: "Juniper Stays", email: "hosts@juniperstays.example" },
    theirs: { body: "Hi Sam! The cabin is free that weekend. We'll need a 30% deposit to hold it.", daysAgo: 6 },
    mine: "Great, we'd like to book it. I sent the deposit this morning. Could you confirm you received it and whether early check-in is possible?",
    daysAgo: 4,
  },
  {
    accountId: "acc-okafor",
    id: "t-fu-garden",
    subject: "Community garden plot application",
    them: { name: "Westside Garden Club", email: "plots@westside-garden.example" },
    mine: "Hello, I submitted an application for a plot for the spring season. Is there a waitlist, and is there anything else you need from me?",
    daysAgo: 12,
  },
  {
    accountId: "acc-northwind",
    id: "t-fu-invoice",
    subject: "Invoice 2291: payment status?",
    them: { name: "Dana Whitlock", email: "dana@brightforge.example" },
    mine: "Hi Dana, checking in on invoice 2291 from August. Could you let me know when it's scheduled for payment?",
    daysAgo: 2,
  },
  // Not listed: only a machine to wait on.
  {
    accountId: "acc-personal",
    id: "t-fu-noreply",
    subject: "Re: Your booking NLA-7Q2",
    them: { name: "Northline Air", email: "no-reply@northline.example" },
    mine: "Can I change my seat on this booking?",
    daysAgo: 6,
  },
];

function seedFollowUps() {
  const now = Date.now();
  for (const w of WAITING) {
    const msgs: MessageView[] = [];
    if (w.theirs) msgs.push(msg(w.accountId, w.id, 0, w.them, [me(w.accountId)], w.subject, w.theirs.body, now - w.theirs.daysAgo * DAY - 3 * HOUR, []));
    const subject = w.theirs ? `Re: ${w.subject}` : w.subject;
    msgs.push(msg(w.accountId, w.id, msgs.length, me(w.accountId), [w.them], subject, w.mine, now - w.daysAgo * DAY - 2 * HOUR, ["SENT"]));
    mockMail.addThread({ accountId: w.accountId, threadId: w.id, subject: w.subject, labelIds: ["SENT"], messages: msgs });
  }
}

/** A few read person threads from the inbox, already in Reply Later. */
function seedReplyLater() {
  const inbox = mailHandlers.list_threads({
    query: { view: { kind: "inbox" }, tab: null, accountId: null, limit: 80, before: null },
  }) as ThreadSummary[];
  const picks = inbox.filter((s) => !s.unread && s.messageCount > 1 && !s.labelIds.some((l) => l.startsWith("CATEGORY_"))).slice(3, 6);
  // Straight onto the summaries: modify_threads would emit before the mock backend exists.
  for (const p of picks) {
    const s = mockMail.summary(p.accountId, p.threadId);
    if (!s) continue;
    const id = ensureMockLabel(p.accountId, REPLY_LATER_LABEL);
    s.labelIds = [...s.labelIds.filter((l) => l !== "INBOX" && l !== "UNREAD"), id];
    s.unread = false;
  }
}

seedFollowUps();
seedReplyLater();
