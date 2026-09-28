// Mock snooze (snooze agent): snooze_threads / unsnooze_threads /
// list_snoozes, a wake timer standing in for the app's scheduler, and a few
// seeded snoozes so the Snoozed view has something to show. Mirrors the
// backend: snooze = record + archive; wake = INBOX + UNREAD, on top.
import type { Snooze, ThreadRef } from "../types";
import type { MockHandler } from "./index";
import { mockBackend } from "./index";
import { mailHandlers, mockMail } from "./mail";
import { dropSnooze, setSnooze, snoozes } from "./snoozeState";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const MAX_AHEAD = 366 * DAY;

function emit(event: string, payload: unknown) {
  mockBackend.emit(event, payload);
}

function changed(refs: ThreadRef[]) {
  const by = new Map<string, string[]>();
  for (const r of refs) by.set(r.accountId, [...(by.get(r.accountId) ?? []), r.threadId]);
  for (const [accountId, threadIds] of by) emit("penguin://mail-changed", { accountId, threadIds });
}

function wakeDue() {
  const now = Date.now();
  const items: { accountId: string; threadId: string; subject: string }[] = [];
  for (const z of [...snoozes.values()]) {
    if (z.wakeAt > now) continue;
    dropSnooze(z.accountId, z.threadId);
    const s = mockMail.summary(z.accountId, z.threadId);
    if (!s || s.labelIds.includes("TRASH")) continue;
    s.labelIds = [...new Set([...s.labelIds, "INBOX", "UNREAD"])];
    s.unread = true;
    s.lastDate = now; // the bump: back on top of the inbox
    items.push({ accountId: z.accountId, threadId: z.threadId, subject: s.subject });
  }
  if (items.length === 0) return;
  changed(items);
  emit("penguin://snooze-woke", { items, missed: 0 });
}
setInterval(wakeDue, 5_000);

function at(dayOffset: number, hour: number): number {
  const d = new Date();
  d.setDate(d.getDate() + dayOffset);
  d.setHours(hour, 0, 0, 0);
  return d.getTime();
}

/** A few snoozed conversations: later today, tomorrow morning, next Monday. */
function seed() {
  const inbox = mailHandlers.list_threads({
    query: { view: { kind: "inbox" }, tab: null, accountId: null, limit: 60, before: null },
  }) as { accountId: string; threadId: string }[];
  const picks = [inbox[9], inbox[17], inbox[26]].filter(Boolean);
  const toMonday = ((8 - new Date().getDay()) % 7) || 7;
  const later = Math.max(Date.now() + 2 * HOUR, at(0, 18));
  const wakes = [later < at(1, 0) ? later : at(1, 12), at(1, 8), at(toMonday, 8)];
  picks.forEach((r, i) => {
    const s = mockMail.summary(r.accountId, r.threadId);
    if (!s) return;
    s.labelIds = s.labelIds.filter((l) => l !== "INBOX");
    setSnooze({ accountId: r.accountId, threadId: r.threadId, wakeAt: wakes[i], snoozedAt: Date.now() - (i + 1) * DAY });
  });
}
seed();

export const snoozeHandlers: Record<string, MockHandler> = {
  snooze_threads: ({ targets, until }) => {
    const now = Date.now();
    if (typeof until !== "number" || until <= now) throw { code: "invalidInput", message: "Pick a time in the future" };
    if (until > now + MAX_AHEAD) throw { code: "invalidInput", message: "Pick a time within a year" };
    const refs = targets as ThreadRef[];
    for (const r of refs) setSnooze({ accountId: r.accountId, threadId: r.threadId, wakeAt: until, snoozedAt: now });
    mailHandlers.modify_threads({ targets: refs, action: { kind: "archive" } });
  },
  unsnooze_threads: ({ targets, toInbox }) => {
    const refs = targets as ThreadRef[];
    for (const r of refs) dropSnooze(r.accountId, r.threadId);
    if (toInbox) mailHandlers.modify_threads({ targets: refs, action: { kind: "moveToInbox" } });
    else changed(refs);
  },
  list_snoozes: ({ accountIds }): Snooze[] =>
    [...snoozes.values()]
      .filter((z) => !accountIds || (accountIds as string[]).includes(z.accountId))
      .sort((a, b) => a.wakeAt - b.wakeAt),
};
