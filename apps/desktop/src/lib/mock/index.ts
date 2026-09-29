// In-memory mock backend used when running outside Tauri (browser) or with
// VITE_MOCK=1. Handlers are split by owner:
//   ./mail.ts   — accounts, labels, threads, thread view, actions, sync (inbox/thread agent)
//   ./search.ts — search, send, oauth setup (search/command/compose agent)
//   ./window.ts — sync window: estimates, coverage, free up space, server search (sync-window agent)
//   ./avatars.ts — sender photos/logos (avatars agent)
//   ./compose.ts — rich-text composer helpers (richtext agent)
//   ./menus.ts — text services and label edits behind context menus (menus agent)
//   ./rules.ts — rules & automations (rules agent)
//   ./me.ts — Settings → You photo, native menu bar context (native agent)
//   ./calendar.ts — Google Calendar, plus the search's Calendar group (calendar agent)
//   ./invites.ts — invitations: row chips, cards, answering (calendar agent)
//   ./ask.ts — Ask your inbox answers (ask agent)
//   ./otp.ts — verification-code mail (otp agent)
//   ./unsubscribe.ts — mailing-list mail for the Unsubscribe button (unsubscribe agent)
//   ./snooze.ts — snooze records, wake timer, seeded snoozes (snooze agent)
//   ./providers.ts — Add account: provider detection, Microsoft client, connect_account (onboarding agent)
//   ./triage.ts — Reply Later label, Follow up list and dismissals, seeded examples
//   ./notifications.ts — new-mail notifications: permission, screen context
//   ./semanticIndex.ts — search by meaning: model download and indexing progress, semantic_status (semantic agent)
//   ./summaries.ts — thread summaries: a fake streaming "on-device model" (summaries agent)
//   ./smart.ts — smart views: seeded receipts, trips, parcels, bills, bookings, files; lists, headers, counts
//   ./writing.ts — Write with AI, suggested replies and snippet files: a fake streaming writer
//   ./split.ts — Split Inbox: the inbox filtered by a split's queries, and the tab counts
//   ./appWindows.ts — conversation and compose windows: window.open with the app's query, composer seeds
//   ./share.ts — share links: storage settings, the Test, fake uploads and links on share.example
import type { UnlistenFn } from "@tauri-apps/api/event";
import { mailHandlers, withAgentCancel } from "./mail";
import { searchHandlers } from "./search";
import { windowHandlers } from "./window";
import { avatarHandlers } from "./avatars";
import { composeHandlers } from "./compose";
import { menuHandlers } from "./menus";
import { ruleHandlers } from "./rules";
import { meHandlers } from "./me";
import { calendarHandlers, withEvents } from "./calendar";
import { askHandlers } from "./ask";
import "./otp"; // verification-code mail (features/otp)
import "./darkBodies"; // HTML mail for judging dark email bodies (features/message-body)
import { imageHandlers } from "./images"; // a conversation with pictures (features/image-viewer)
import { unsubscribeHandlers } from "./unsubscribe";
import { snoozeHandlers } from "./snooze";
import { providerHandlers } from "./providers";
import { triageHandlers, withReplyLaterClear } from "./triage";
import { notificationHandlers } from "./notifications";
import { inviteHandlers, withInviteCards, withInvites } from "./invites";
import { summaryHandlers } from "./summaries";
import { smartHandlers } from "./smart";
import { semanticIndexHandlers } from "./semanticIndex";
import { writingHandlers } from "./writing";
import { splitHandlers, withSplits } from "./split";
import { appWindowHandlers } from "./appWindows";
import { shareHandlers } from "./share";

export type MockHandler = (args: Record<string, any>) => unknown | Promise<unknown>;

const handlers: Record<string, MockHandler> = { ...mailHandlers, ...searchHandlers, ...windowHandlers, ...avatarHandlers, ...composeHandlers, ...menuHandlers, ...ruleHandlers, ...meHandlers, ...calendarHandlers, ...askHandlers, ...unsubscribeHandlers, ...snoozeHandlers, ...providerHandlers, ...triageHandlers, ...notificationHandlers, ...inviteHandlers, ...summaryHandlers, ...smartHandlers, ...semanticIndexHandlers, ...imageHandlers, ...writingHandlers, ...splitHandlers, ...appWindowHandlers, ...shareHandlers };
handlers.search = withEvents(handlers.search);
handlers.send_message = withReplyLaterClear(handlers.send_message);
handlers.cancel_scheduled_send = withAgentCancel(handlers.cancel_scheduled_send);
handlers.list_threads = withSplits(withInvites(handlers.list_threads));
Object.assign(handlers, splitHandlers(handlers.list_threads));
handlers.event_invite = withInviteCards(handlers.event_invite);
const listeners = new Map<string, Set<(p: unknown) => void>>();

export const mockBackend = {
  async invoke<T>(cmd: string, args: Record<string, any>): Promise<T> {
    const h = handlers[cmd];
    if (!h) throw new Error(`mock: no handler for command "${cmd}"`);
    if ((globalThis as { __penguinBench?: boolean }).__penguinBench) performance.mark(`penguin:ipc-send:${cmd}`, { detail: args });
    // simulate IPC hop
    await new Promise((r) => setTimeout(r, 4));
    const out = (await h(args)) as T;
    // scripts/bench-ui times keystroke → response → render with these marks.
    if ((globalThis as { __penguinBench?: boolean }).__penguinBench) performance.mark(`penguin:ipc:${cmd}`, { detail: args });
    return out;
  },
  async listen(event: string, cb: (p: unknown) => void): Promise<UnlistenFn> {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event)!.add(cb);
    return () => listeners.get(event)?.delete(cb);
  },
  emit(event: string, payload: unknown) {
    listeners.get(event)?.forEach((cb) => cb(payload));
    emitted.forEach((f) => f(event, payload));
  },
  /** Every event, also for the other windows (lib/mockBridge.ts hostMock). */
  onEmit(f: (event: string, payload: unknown) => void) {
    emitted.add(f);
  },
};
const emitted = new Set<(event: string, payload: unknown) => void>();

// scripts/bench-ui emits backend events through this (a mail-changed storm
// during a backfill), only on a page it has marked as a benchmark.
if ((globalThis as { __penguinBench?: boolean }).__penguinBench) (globalThis as { __penguinMock?: typeof mockBackend }).__penguinMock = mockBackend;
