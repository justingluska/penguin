// OWNER: sync-window agent.
//
// Mock sync-window commands: per-window estimates, coverage, free up space
// and "Also search Gmail" over the inbox mock's threads. Numbers are shaped
// like a real mailbox (~2,400 messages a month) so the Settings slider reads
// sensibly; everything is fictional.
import type {
  AccountCoverage,
  FreeUpSpace,
  SearchHit,
  ServerSearchResponse,
  SyncCoverage,
  SyncWindowMonths,
  WindowEstimate,
} from "../types";
import { mockMail } from "./mail";
import type { MockHandler } from "./index";

const DAY = 86_400_000;
const MONTH = 30.436875 * DAY;
const PER_MONTH = [2_400, 900, 450];
const MAILBOX = [111_000, 38_000, 12_500];

function windowStart(months: number): number {
  if (months === 0) return 0;
  const start = Date.now() - months * MONTH;
  return Math.floor(start / DAY) * DAY;
}

async function settings() {
  const { mockBackend } = await import("./index");
  return mockBackend.invoke<import("../types").Settings>("get_settings", {});
}

function inWindow(i: number, months: number): number {
  const all = MAILBOX[i % MAILBOX.length];
  return months === 0 ? all : Math.min(all, PER_MONTH[i % PER_MONTH.length] * months);
}

function scoped(accountIds: string[] | null | undefined) {
  return mockMail.accounts().filter((a) => !accountIds || accountIds.includes(a.id));
}

export const windowHandlers: Record<string, MockHandler> = {
  sync_window_estimate: async ({ months, accountIds }): Promise<WindowEstimate[]> => {
    await new Promise((r) => setTimeout(r, 250));
    const m = months as SyncWindowMonths;
    return scoped(accountIds).map((a, i) => {
      const n = inWindow(i, m);
      const have = Math.min(n, inWindow(i, 6));
      const older = MAILBOX[i % MAILBOX.length] - n;
      return {
        accountId: a.id,
        months: m,
        inWindow: n,
        haveFull: have,
        // 6,000 units/min × 0.95 ÷ 60 per full get ≈ 95/min; headers 20 → 285/min.
        etaSecs: Math.round(((n - have) / 95) * 60),
        older,
        olderHeadersEtaSecs: Math.round((older / 285) * 60),
        olderFullEtaSecs: Math.round((older / 95) * 60),
        bytesPerMessage: 88_000,
        error: null,
      };
    });
  },
  sync_coverage: async ({ accountIds }): Promise<SyncCoverage> => {
    const s = await settings();
    const start = windowStart(s.syncWindowMonths);
    const accounts: AccountCoverage[] = scoped(accountIds).map((a, i) => ({
      accountId: a.id,
      fullSinceMs: start,
      windowComplete: true,
      olderComplete: i !== 0,
      full: inWindow(i, s.syncWindowMonths),
      headersOnly: s.olderMail === "none" ? 0 : MAILBOX[i % MAILBOX.length] - inWindow(i, s.syncWindowMonths),
    }));
    return { windowMonths: s.syncWindowMonths, olderMail: s.olderMail, windowStartMs: start, accounts };
  },
  free_up_space: async ({ dryRun, months }): Promise<FreeUpSpace> => {
    const s = await settings();
    const m = (months ?? s.syncWindowMonths) as number;
    if (m === 0) throw { code: "invalidInput", message: "The sync window is set to everything; choose a window first" };
    await new Promise((r) => setTimeout(r, dryRun ? 150 : 1200));
    const before = 2_140_000_000;
    // Full messages older than the window: whatever the mock accounts hold beyond it.
    const messages = mockMail.accounts().reduce((n, _a, i) => n + Math.max(0, inWindow(i, 6) - inWindow(i, m)), 0) + 1_200;
    const bodyBytes = messages * 47_000;
    return { messages, bodyBytes, bytesBefore: before, bytesAfter: dryRun ? before : before - Math.round(bodyBytes * 1.3) };
  },
  search_server: async ({ query, accountIds }): Promise<ServerSearchResponse> => {
    await new Promise((r) => setTimeout(r, 900));
    const s = await settings();
    const start = windowStart(s.syncWindowMonths);
    const words = String(query)
      .toLowerCase()
      .split(/\s+/)
      .filter((w) => w && !w.includes(":"));
    const accounts = scoped(accountIds);
    const ids = new Set(accounts.map((a) => a.id));
    const hits: SearchHit[] = mockMail
      .threads()
      .filter((t) => ids.has(t.accountId))
      .filter((t) => words.every((w) => `${t.subject} ${t.snippet}`.toLowerCase().includes(w)))
      .slice(0, 12)
      .map((t) => ({
        accountId: t.accountId,
        threadId: t.threadId,
        messageId: `${t.threadId}-srv`,
        subject: t.subject,
        from: t.participants[0] ?? { name: null, email: "someone@mail.example" },
        // Pretend these are older than the window.
        date: Math.min(t.lastDate, start > 0 ? start - 40 * DAY : t.lastDate),
        snippetHtml: t.snippet.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"),
        matchCount: 1,
        labelIds: t.labelIds,
        hasAttachments: t.hasAttachments,
        unread: t.unread,
        score: 0,
        matchedBy: [],
        passage: null,
      }));
    return {
      gmailQuery: words.join(" "),
      approximate: false,
      hits,
      accounts: accounts.map((a) => ({
        accountId: a.id,
        estimate: hits.filter((h) => h.accountId === a.id).length,
        fetched: hits.filter((h) => h.accountId === a.id).length,
        error: null,
      })),
    };
  },
};
