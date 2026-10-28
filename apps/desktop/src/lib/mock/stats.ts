// Settings → Stats in the mock: a believable year of sending and receiving
// (weekday-heavy, quiet weekends, a two-week holiday), the same on every
// load. Fictional people and .example domains only.
import type { MailStats, StatsDay, StatsPerson } from "../types";
import type { MockHandler } from "./index";

const DAYS = 365;

/** Small seeded PRNG (mulberry32) so the grid is stable across reloads. */
function rng(seed: number) {
  return () => {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

const RECIPIENTS: StatsPerson[] = [
  { email: "marco@northwind.example", name: "Marco Bellini", count: 214 },
  { email: "priya@northwind.example", name: "Priya Raman", count: 171 },
  { email: "ops@harbor.example", name: "Harbor Ops", count: 96 },
  { email: "lena@fieldnotes.example", name: "Lena Okafor", count: 74 },
  { email: "sam@studio.example", name: "Sam Ortiz", count: 51 },
  { email: "jo@acme.example", name: null, count: 38 },
];
const SENDERS: StatsPerson[] = [
  { email: "priya@northwind.example", name: "Priya Raman", count: 402 },
  { email: "marco@northwind.example", name: "Marco Bellini", count: 355 },
  { email: "lena@fieldnotes.example", name: "Lena Okafor", count: 140 },
  { email: "billing@harbor.example", name: "Harbor Billing", count: 88 },
  { email: "sam@studio.example", name: "Sam Ortiz", count: 61 },
];

function mockStats(accountIds: string[] | null): MailStats {
  const scale = accountIds && accountIds.length === 1 ? 0.45 : 1;
  const rand = rng(accountIds?.join(",").length ?? 7);
  const today = new Date();
  today.setHours(12, 0, 0, 0);
  const days: StatsDay[] = [];
  for (let i = DAYS - 1; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(today.getDate() - i);
    const weekend = d.getDay() === 0 || d.getDay() === 6;
    const holiday = i > 120 && i < 135;
    const busy = weekend ? 0.25 : 1;
    const sent = holiday || rand() < (weekend ? 0.55 : 0.08) ? 0 : Math.round(rand() * rand() * 22 * busy * scale);
    const received = holiday ? Math.round(rand() * 20) : Math.round((20 + rand() * 60) * (weekend ? 0.4 : 1) * scale);
    days.push({ date: iso(d), sent, received });
  }
  const sentByHour = Array.from({ length: 7 }, (_, wd) =>
    Array.from({ length: 24 }, (_, h) => {
      const work = h >= 8 && h <= 18 ? 1 : h >= 20 && h <= 23 ? 0.35 : 0.04;
      const day = wd < 5 ? 1 : 0.2;
      return Math.round(work * day * (6 + rand() * 18) * scale);
    }),
  );
  let longest = 0;
  let run = 0;
  for (const d of days) {
    run = d.sent > 0 ? run + 1 : 0;
    longest = Math.max(longest, run);
  }
  const back = days[days.length - 1].sent > 0 ? days : days.slice(0, -1);
  let current = 0;
  for (let i = back.length - 1; i >= 0 && back[i].sent > 0; i--) current++;
  const sent = days.reduce((n, d) => n + d.sent, 0);
  const received = days.reduce((n, d) => n + d.received, 0);
  const replies = Math.round(sent * 0.62);
  const oldest = new Date(today);
  oldest.setFullYear(today.getFullYear() - 6);
  return {
    days,
    sent,
    received,
    activeDays: days.filter((d) => d.sent > 0).length,
    currentStreak: current,
    longestStreak: longest,
    sentByHour,
    medianReplyMinutes: 47,
    replies,
    repliesWithinHour: Math.round(replies * 0.54),
    receivedNewsletters: Math.round(received * 0.41),
    topRecipients: RECIPIENTS.map((p) => ({ ...p, count: Math.round(p.count * scale) })),
    topSenders: SENDERS.map((p) => ({ ...p, count: Math.round(p.count * scale) })),
    oldestMessage: oldest.getTime(),
  };
}

export const statsHandlers: Record<string, MockHandler> = {
  mail_stats: ({ accountIds }) => mockStats((accountIds as string[] | null | undefined) ?? null),
};
