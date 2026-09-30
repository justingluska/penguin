// The ONLY way the UI talks to the Rust backend. Every function maps 1:1 to
// a #[tauri::command] in src-tauri/src/commands.rs (snake_case name, camelCase
// args). Outside Tauri (plain `vite` in a browser), with VITE_MOCK=1, or in demo
// mode, calls go to the in-memory mock backend so the UI can be built, reviewed
// and screenshotted alone.

import { invoke } from "@tauri-apps/api/core";
import { emit, emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isDemo } from "./demo";
import { isMainWindow, thisWindowLabel } from "./thisWindow";
import type { MockLike } from "./mockBridge";
import type {
  Address,
  SmartCount,
  SplitCounts,
  SmartViewInfo,
  LogTail,
  UpdateEvent,
  AskAnswer,
  AskQuery,
  AskScope,
  PersonSummary,
  MessageDetails,
  McpInfo,
  AgentActivity,
  AgentOrganized,
  AgentPendingSend,
  AgentSendQueued,
  AgentUndoStep,
  SemanticIndexStatus,
  CliLinkStatus,
  AttachmentPreview,
  ImageBytes,
  SaveAllItem,
  SavedImages,
  ShareLinkConfig,
  ShareLinkConfigInput,
  ShareRequest,
  ShareTestReport,
  SharedLink,
  DragFile,
  Account,
  AccountPatch,
  Draft,
  DraftRef,
  QuoteSource,
  OpenedDraft,
  Label,
  LabelPatch,
  ListQuery,
  MailChangedEvent,
  OAuthClientStatus,
  SignInLink,
  ConnectAccountRequest,
  DetectedProvider,
  MicrosoftClientStatus,
  SetupKind,
  SearchRequest,
  SearchResponse,
  SyncStatus,
  ThreadAction,
  ActionFailedEvent,
  BodyFetchFailedEvent,
  ThreadRef,
  ThreadSummary,
  ThreadView,
  MessageView,
  Diagnostics,
  RevealTarget,
  Settings,
  SettingsPatch,
  TableSizes,
  WindowEstimate,
  SyncCoverage,
  FreeUpSpace,
  ServerSearchResponse,
  SyncWindowMonths,
  ScheduledSend,
  Reminder,
  SentRef,
  ScheduledSentBatch,
  ReminderDue,
  Snooze,
  TriageCount,
  SnoozeWokeBatch,
  AvatarInfo,
  AvatarRequest,
  AvatarStatus,
  AvatarsChangedEvent,
  MenuContext,
  OpenedWindow,
  WindowRequest,
  MenuEvent,
  NotifyContext,
  NotificationOpen,
  NotificationPermission,
  CalendarChangedEvent,
  CalendarEvent,
  CalendarStatus,
  EventDetail,
  InviteAnswer,
  InviteCard,
  PersonMeetings,
  Rule,
  RuleInput,
  RulesOverview,
  RulePreview,
  RuleRunReport,
  RuleLogEntry,
  RuleFiredEvent,
  UnsubscribeMethod,
  UnsubscribeOffer,
  UnsubscribeOutcome,
  AiAvailability,
  AiSummary,
  SummaryProgress,
  WriteRequest,
  WriteResult,
  WriteProgress,
  ReplySuggestions,
  SnippetFile,
} from "./types";

/** spell_check (src-tauri/src/text_services.rs). */
export interface SpellCheck {
  misspelled: boolean;
  guesses: string[];
}

export const EVENTS = {
  syncStatus: "penguin://sync-status",
  mailChanged: "penguin://mail-changed",
  actionFailed: "penguin://action-failed",
  bodyFetchFailed: "penguin://body-fetch-failed",
  settingsChanged: "penguin://settings-changed",
  scheduledSent: "penguin://scheduled-sent",
  reminderDue: "penguin://reminder-due",
  agentSendQueued: "penguin://agent-send-queued",
  agentOrganized: "penguin://agent-organized",
  snoozeWoke: "penguin://snooze-woke",
  avatarsChanged: "penguin://avatars-changed",
  rulesChanged: "penguin://rules-changed",
  ruleFired: "penguin://rule-fired",
  calendarChanged: "penguin://calendar-changed",
  menu: "penguin://menu",
  update: "penguin://update",
  signInUrl: "penguin://sign-in-url",
  notificationOpen: "penguin://notification-open",
  summaryProgress: "penguin://summary-progress",
  imageOpen: "penguin://image-open",
  writeProgress: "penguin://write-progress",
} as const;

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
// Developer mock: dev builds outside Tauri, or VITE_MOCK=1 (`npm run dev:mock`).
// Constant false in a production build.
const devMock = (import.meta.env.DEV || import.meta.env.VITE_MOCK === "1") && (import.meta.env.VITE_MOCK === "1" || !inTauri);
/**
 * Demo mode (src/lib/demo.ts, docs/DEMO.md): the production app on the mock
 * backend, for screenshots. Every command and event below goes to the mock, so
 * the Rust side is never asked for mail, accounts or settings, and nothing real
 * is sent or changed. The only real calls left are window chrome: which native
 * menu items are enabled (setMenuContext), menu clicks (onMenu), and opening
 * conversation and compose windows (openWindow, takeWindowSeed), which run
 * on the same mock through the main window (lib/mockBridge.ts).
 */
export { isDemo };
/**
 * Commands and events go to the in-memory mock (src/lib/mock). The mock is a
 * lazy chunk (the dynamic import below): a production build carries it but
 * loads it only in demo mode.
 */
export const isMock = devMock || isDemo;
/** The sidebar's "Mock" badge: the developer mock only, never demo mode (clean screenshots). */
export const showMockBadge = devMock && !isDemo;

let mockModule: Promise<MockLike> | null = null;
/**
 * The mock lives in the main window; conversation and compose windows reach
 * it through the main window (lib/mockBridge.ts), so all of them share one
 * mailbox, as they share the one Rust backend.
 */
function mockBackend(): Promise<MockLike> {
  mockModule ??= isMainWindow
    ? Promise.all([import("./mock"), import("./mockBridge")]).then(([m, bridge]) => {
        bridge.hostMock(m.mockBackend);
        return m.mockBackend;
      })
    : import("./mockBridge").then((bridge) => bridge.remoteMock());
  return mockModule;
}

/** App events between windows (lib/windowBus.ts); nothing on the Rust side reads them. */
const BUS_EVENT = "penguin://window-bus";
let busChannel: BroadcastChannel | null = null;
const browserBus = () => (busChannel ??= typeof BroadcastChannel === "undefined" ? null : new BroadcastChannel(BUS_EVENT));

/**
 * Post a message to another window (`to` a label, or "*"). Window chrome, not
 * a backend call: real inside Tauri in demo mode too (the demo's windows
 * share its mock through it), a BroadcastChannel in a browser.
 */
export function busPost(to: string, msg: unknown): Promise<void> {
  if (inTauri) return to === "*" ? emit(BUS_EVENT, msg) : emitTo(to, BUS_EVENT, msg);
  browserBus()?.postMessage(msg);
  return Promise.resolve();
}

/** Messages to this window (and to every window) from the others. */
export function onBusMessage(cb: (msg: unknown) => void): void {
  if (inTauri) {
    void listenHere<unknown>(BUS_EVENT, (e) => cb(e.payload));
    return;
  }
  browserBus()?.addEventListener("message", (e) => cb((e as MessageEvent).data));
}

/**
 * Listen for a backend event meant for this window: `app.emit` reaches every
 * window, `emit_to(label)` only that one (menu items, the image viewer). A
 * listener with no target would hear events emitted to any window.
 */
function listenHere<T>(event: string, cb: (e: { payload: T }) => void): Promise<UnlistenFn> {
  return listen<T>(event, cb, { target: thisWindowLabel });
}

/** Mutating commands in flight, by name (callsSettled). */
const inflight = new Map<string, number>();
const settledWaiters = new Set<() => void>();

/**
 * Resolves once none of `cmds` is in flight, or after `timeoutMs`. A window
 * about to close waits for its last change to reach the backend (and to be
 * refused, if it is: app/windowShell.tsx).
 */
export function callsSettled(cmds: readonly string[], timeoutMs: number): Promise<void> {
  const busy = () => cmds.some((c) => (inflight.get(c) ?? 0) > 0);
  if (!busy()) return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => {
      settledWaiters.delete(check);
      clearTimeout(timer);
      resolve();
    };
    const check = () => !busy() && done();
    const timer = setTimeout(done, timeoutMs);
    settledWaiters.add(check);
  });
}

function track<T>(cmd: string, p: Promise<T>): Promise<T> {
  inflight.set(cmd, (inflight.get(cmd) ?? 0) + 1);
  const end = () => {
    const n = (inflight.get(cmd) ?? 1) - 1;
    if (n <= 0) inflight.delete(cmd);
    else inflight.set(cmd, n);
    // After the caller's own handlers (a failed removal puts the row back first).
    setTimeout(() => [...settledWaiters].forEach((f) => f()), 0);
  };
  p.then(end, end);
  return p;
}

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const p = track(cmd, isMock ? mockBackend().then((m) => m.invoke<T>(cmd, args ?? {})) : invoke<T>(cmd, args));
  // Every failed command lands in penguin.log (Settings → Developer → View log).
  p.catch((e) => {
    const err = asCommandError(e);
    if (!QUIET_COMMANDS.has(cmd) && !QUIET_CODES.has(err.code)) {
      logClientEvent({ level: "warn", source: "command", what: cmd, message: err.message, code: err.code });
    }
  });
  return p;
}

/** Commands whose rejection is an answer ("no photo", "nothing to unsubscribe"), not a failure. */
const QUIET_COMMANDS = new Set(["suggest_recipients", "me_photo", "avatar_lookup", "account_photo", "unsubscribe_check", "log_client_event"]);
/** The user backed out (closed the sign-in tab, cancelled a dialog). */
const QUIET_CODES = new Set(["cancelled"]);

export interface ClientLogEvent {
  level: "error" | "warn";
  /** command | toast | window */
  source: string;
  /** The command name, or where in the UI. */
  what: string;
  /** Error text only: never subjects, bodies or addresses typed by the user. */
  message: string;
  code?: string;
}

// A burst (a broken loop, a flood of identical failures) is logged once per second per message.
const recentlyLogged = new Map<string, number>();

/** Record a UI-side failure in penguin.log. Never throws. */
export function logClientEvent(e: ClientLogEvent): void {
  const key = `${e.source}|${e.what}|${e.message}`;
  const now = Date.now();
  if ((recentlyLogged.get(key) ?? 0) > now - 1000) return;
  recentlyLogged.set(key, now);
  if (recentlyLogged.size > 200) recentlyLogged.clear();
  if (isMock) {
    console.warn("penguin log:", e);
    return;
  }
  if (!inTauri) return;
  // Not through call(): a failure to log must not try to log itself.
  invoke<void>("log_client_event", { event: e }).catch((err) => console.warn("penguin: could not write to the log", err));
}

export const api = {
  // setup / accounts
  oauthClientStatus: () => call<OAuthClientStatus>("oauth_client_status"),
  /** Paste/choose the Google "Desktop app" client JSON. */
  setOauthClient: (json: string) => call<OAuthClientStatus>("set_oauth_client", { json }),
  /** iOS client (bare id, Google's .plist, or JSON) → system sign-in sheet. Bad input rejects invalidInput. */
  setIosOauthClient: (input: string) => call<OAuthClientStatus>("set_ios_oauth_client", { input }),
  clearIosOauthClient: () => call<OAuthClientStatus>("clear_ios_oauth_client"),
  listAccounts: () => call<Account[]>("list_accounts"),
  /**
   * Opens the system browser for Google sign-in; resolves when done. `loginHint` preselects the address typed in Add
   * account. With settings.calendar.connectOnSignIn the same consent asks for read-only calendar, and its sync starts in
   * the background when granted (watch penguin://calendar-changed). An unticked calendar box is not an error.
   */
  addAccount: (loginHint?: string | null) => call<Account>("add_account", { loginHint: loginHint ?? null }),
  /**
   * Who hosts an address and how to sign in (domain table → MX → autoconfig/ISPDB; only the domain leaves the Mac).
   * `choose`: a manual "Choose provider" pick, answered without the network. Bad addresses reject invalidInput.
   */
  detectProvider: (email: string, choose?: SetupKind | null) => call<DetectedProvider>("detect_provider", { email, choose: choose ?? null }),
  microsoftClientStatus: () => call<MicrosoftClientStatus>("microsoft_client_status"),
  /** Save (null removes) the Application (client) ID of the user's own Entra app registration; not a GUID → invalidInput. */
  setMicrosoftClient: (clientId: string | null) => call<MicrosoftClientStatus>("set_microsoft_client", { clientId }),
  /**
   * Sign in to a non-Google provider and start its sync (Microsoft: browser + PKCE with the saved client; app-password
   * and IMAP providers: verify the login, store the password in the Keychain). Implemented by the provider agents; until
   * then only reached where `DetectedProvider.available` is true (mock mode). Contract: ARCHITECTURE.md → Providers.
   */
  connectAccount: (request: ConnectAccountRequest) => call<Account>("connect_account", { request }),
  removeAccount: (accountId: string) => call<void>("remove_account", { accountId }),
  /**
   * Browser sign-in for an existing account (login hint preselected), then
   * sync restarts. Asks for calendar like addAccount, plus any calendar
   * access the account already had. Rejects with invalidInput if a different
   * Google account was chosen, or `cancelled` after cancelSignIn.
   */
  reconnectAccount: (accountId: string) => call<Account>("reconnect_account", { accountId }),
  /** Abandon the pending browser sign-in (add or reconnect). Resolves true if one was pending. */
  cancelSignIn: () => call<boolean>("cancel_sign_in"),
  /** The browser sign-in waiting for the user, or null (also sent as penguin://sign-in-url; see onSignInUrl). */
  signInLink: () => call<SignInLink | null>("sign_in_link"),
  /** "Browser didn't open? Open it again": rejects with the reason when there's no waiting sign-in or no browser opens. */
  reopenSignIn: () => call<void>("reopen_sign_in"),
  /** `accountIds` (a profile) limits the result to those accounts. */
  syncStatus: (accountIds: string[] | null = null) => call<SyncStatus[]>("sync_status", { accountIds }),
  syncNow: () => call<void>("sync_now"),
  /**
   * Fix a stuck account: restarts its sync task if it died, otherwise wakes it
   * from error backoff. The failure stays on its status until the retry syncs
   * (failure null, recovered set) or fails again (failure.count grows).
   */
  retrySyncAccount: (accountId: string) => call<void>("retry_account_sync", { accountId }),
  /** Set the nickname (null/blank clears) and/or #rrggbb color; resolves with the updated account. */
  updateAccount: (accountId: string, patch: AccountPatch) => call<Account>("update_account", { accountId, patch }),

  // reading
  /** accountId and accountIds (a profile) intersect; null = no restriction. */
  listLabels: (accountId: string | null, accountIds: string[] | null = null) =>
    call<Label[]>("list_labels", { accountId, accountIds }),
  /** Rename / recolor / hide a user label in Gmail, then locally; emits mail-changed (labels reload). */
  updateLabel: (accountId: string, labelId: string, patch: LabelPatch) =>
    call<Label>("update_label", { accountId, labelId, patch }),
  /** Delete a user label in Gmail and strip it from local mail; emits mail-changed. */
  deleteLabel: (accountId: string, labelId: string) => call<void>("delete_label", { accountId, labelId }),
  listThreads: (query: ListQuery) => call<ThreadSummary[]>("list_threads", { query }),
  getThread: (accountId: string, threadId: string) =>
    call<ThreadView | null>("get_thread", { accountId, threadId }),
  /** Re-render one message with remote images allowed. */
  loadRemoteImages: (accountId: string, messageId: string) =>
    call<MessageView>("load_remote_images", { accountId, messageId }),

  // acting (optimistic locally, then pushed to Gmail)
  modifyThreads: (targets: ThreadRef[], action: ThreadAction) =>
    call<void>("modify_threads", { targets, action }),
  /** With a draftId, sends via drafts.send so the saved draft disappears from Drafts. */
  sendMessage: (draft: Draft, draftId: string | null = null) =>
    call<SentRef>("send_message", { draft, draftId }),

  /**
   * The originals a reply or forward quotes, in order: sanitized HTML and
   * text (withBody) and attachments. Local unless a message is headers-only
   * (older than the sync window): then its body is downloaded first, and a
   * failure rejects (network / notFound) rather than returning nothing.
   */
  quoteSources: (accountId: string, messageIds: string[], withBody: boolean) =>
    call<QuoteSource[]>("quote_sources", { accountId, messageIds, withBody }),

  // drafts (Gmail drafts; mirrored locally with the DRAFT label)
  /** Create (draftId null) or update. Await a create before saving again, or a second draft is made. */
  saveDraft: (draft: Draft, draftId: string | null) => call<DraftRef>("save_draft", { draft, draftId }),
  deleteDraft: (accountId: string, draftId: string) => call<void>("delete_draft", { accountId, draftId }),
  /** Reopen by draftId or by the draft's messageId (Drafts view). Null if Gmail no longer has it. */
  getDraft: (accountId: string, ids: { draftId?: string | null; messageId?: string | null }) =>
    call<OpenedDraft | null>("get_draft", {
      accountId,
      draftId: ids.draftId ?? null,
      messageId: ids.messageId ?? null,
    }),

  // send later + "remind me if no reply" (local: they fire while Penguin runs)
  /**
   * Queue a SAVED draft to send at `sendAt` (unix ms). Re-scheduling a draft
   * replaces its schedule; sending or deleting the draft removes it. With
   * `remindAfterMs`, a reminder is armed when it actually sends.
   */
  scheduleSend: (req: { accountId: string; draftId: string; sendAt: number; remindAfterMs: number | null }) =>
    call<ScheduledSend>("schedule_send", req),
  /** The draft stays in Drafts. */
  cancelScheduledSend: (id: string) => call<void>("cancel_scheduled_send", { id }),
  /** Soonest first; null = every account. */
  listScheduledSends: (accountId: string | null = null) => call<ScheduledSend[]>("list_scheduled_sends", { accountId }),
  /** Re-surface the thread at `remindAt` unless someone else replies after it was sent. */
  setReminder: (req: { accountId: string; threadId: string; sentMessageId: string | null; remindAt: number }) =>
    call<Reminder>("set_reminder", req),
  cancelReminder: (id: string) => call<void>("cancel_reminder", { id }),
  listReminders: (accountId: string | null = null) => call<Reminder[]>("list_reminders", { accountId }),

  // snooze (local records + archive; woken by the local scheduler while Penguin runs)
  /** Archive `targets` and bring them back to the inbox, unread, at `until` (unix ms). Replaces a snooze. */
  snoozeThreads: (targets: ThreadRef[], until: number) => call<void>("snooze_threads", { targets, until }),
  /** End the snooze now; `toInbox` also moves them to the inbox (else only the record goes). */
  unsnoozeThreads: (targets: ThreadRef[], toInbox: boolean) => call<void>("unsnooze_threads", { targets, toInbox }),
  /** Active snoozes, soonest first; accountIds null = every account. */
  listSnoozes: (accountIds: string[] | null = null) => call<Snooze[]>("list_snoozes", { accountIds }),

  // Reply Later (a synced label per account) and Follow up (local)
  /** Mark (`on`) Reply Later: label, archive, mark read; the label is created on first use. `on: false` only removes the label. */
  replyLater: (targets: ThreadRef[], on: boolean) => call<void>("reply_later", { targets, on }),
  /** Sizes of Reply Later and Follow up per account; accountIds null = every account. */
  triageCounts: (accountIds: string[] | null = null) => call<TriageCount[]>("triage_counts", { accountIds }),
  /** Hide from Follow up until something newer is sent in the thread; `dismissed: false` undoes it. */
  dismissFollowUps: (targets: ThreadRef[], dismissed: boolean) => call<void>("dismiss_follow_ups", { targets, dismissed }),

  // Smart views (features/smart): the lists go through listThreads ({kind: "smart"} / {kind: "query"}).
  /** The header over a smart view: its key figures (local). */
  smartViewInfo: (view: string, accountIds: string[] | null = null) => call<SmartViewInfo>("smart_view_info", { view, accountIds }),
  /** Sidebar counts for these views ("query:<q>" for a pinned search); local. */
  smartCounts: (views: string[], accountIds: string[] | null = null) => call<SmartCount[]>("smart_counts", { views, accountIds }),
  /** Split Inbox: conversations (and unread) per split query in order, then Other; local. */
  splitCounts: (queries: string[], accountIds: string[] | null = null) => call<SplitCounts>("split_counts", { queries, accountIds }),

  /** Download one attachment to ~/Downloads (unique filename) and return the saved path. */
  /** Person card: one correspondent's mail at a glance (local, indexed). */
  personSummary: (email: string) => call<PersonSummary>("person_summary", { email }),
  /** To/Cc/Bcc suggestions from everyone in your mail (local, indexed): people you wrote to first. */
  suggestRecipients: (query: string, limit = 8) => call<Address[]>("suggest_recipients", { query, limit }),
  /** Ask your inbox: a deterministic, cited answer from the local index (no model, no network). */
  ask: (question: string, scope: AskScope | null = null) => call<AskAnswer>("ask", { question, scope }),
  /**
   * The same question read by Apple's on-device model (Settings → AI), when the grammar didn't answer it
   * exactly: the model only fills the query schema; the answer is computed from local mail. null = off,
   * unavailable, or the reading didn't check out (keep the first answer).
   */
  askUnderstand: (question: string, scope: AskScope | null = null) => call<AskAnswer | null>("ask_understand", { question, scope }),
  /** Answer an edited reading (the answer's chips). */
  askQuery: (question: string, query: AskQuery, scope: AskScope | null = null) => call<AskAnswer>("ask_query", { question, query, scope }),
  // Thread summaries: Apple's on-device model (src-tauri/src/summary/, docs/SUMMARIES.md).
  /** Can this Mac summarize, and if not, why (Apple Intelligence off, not eligible, model not ready…). */
  summaryAvailability: () => call<AiAvailability>("summary_availability"),
  /** The stored summary (local only); `stale` when the thread changed since. null = none, or summaries are off. */
  cachedSummary: (accountId: string, threadId: string) => call<AiSummary | null>("cached_summary", { accountId, threadId }),
  /** Make the summary (again). Streams penguin://summary-progress; rejects `cancelled` after cancelSummary. */
  summarizeThread: (accountId: string, threadId: string) => call<AiSummary>("summarize_thread", { accountId, threadId }),
  cancelSummary: (accountId: string, threadId: string) => call<boolean>("cancel_summary", { accountId, threadId }),
  /** The user is about to ask (pointer on Summarize): load the model ahead. */
  prewarmSummarizer: () => call<void>("prewarm_summarizer"),
  // Writing in the composer with the same on-device model (src-tauri/src/writing/, docs/COMPOSE-SPEED.md).
  /** Write or rewrite text. Streams penguin://write-progress; rejects `cancelled` after cancelWrite, invalidInput when off or unavailable. */
  writeWithAi: (request: WriteRequest) => call<WriteResult>("write_with_ai", { request }),
  cancelWrite: (runId: string) => call<boolean>("cancel_write", { runId }),
  /** The composer's AI bar opened: load the model ahead. */
  prewarmWriter: () => call<void>("prewarm_writer"),
  /** Up to three short replies to the conversation (instant replies); cached per thread version. */
  suggestReplies: (accountId: string, threadId: string) => call<ReplySuggestions>("suggest_replies", { accountId, threadId }),
  /** Keep a file with a snippet (content-addressed on this Mac, ≤ 25 MB). */
  saveSnippetFile: (filename: string, mimeType: string, dataBase64: string) =>
    call<SnippetFile>("save_snippet_file", { filename, mimeType, dataBase64 }),
  /** A snippet file's bytes (standard base64); notFound when it's gone. */
  readSnippetFile: (id: string) => call<string>("read_snippet_file", { id }),
  /** Message details modal: stored fields + Gmail's auth/TLS verdicts (format=metadata, ~20 units). */
  getMessageDetails: (accountId: string, messageId: string) =>
    call<MessageDetails>("get_message_details", { accountId, messageId }),
  /** "Show original": raw RFC 822 source as text (format=raw, fetched on demand). Render escaped only. */
  getMessageSource: (accountId: string, messageId: string) =>
    call<string>("get_message_source", { accountId, messageId }),
  /** In-app preview (image / PDF / text, else unsupported). Fetches once, cached for saveAttachment. */
  previewAttachment: (accountId: string, messageId: string, attachmentId: string) =>
    call<AttachmentPreview>("preview_attachment", { accountId, messageId, attachmentId }),
  /** The same preview for a composer file not on any message yet (its base64 bytes; Rust sniffs them). */
  previewOutgoingFile: (filename: string, mimeType: string, dataBase64: string) =>
    call<AttachmentPreview>("preview_outgoing_file", { filename, mimeType, dataBase64 }),
  saveAttachment: (accountId: string, messageId: string, attachmentId: string) =>
    call<string>("save_attachment", { accountId, messageId, attachmentId }),
  /** Open a saved file (path returned by saveAttachment) with the default app, or in Preview (macOS). */
  openPath: (path: string, inPreview = false) => call<void>("open_path", { path, inPreview }),
  /**
   * Image viewer: a remote picture already shown in a message body, fetched
   * again (hardened, see src-tauri/src/image_viewer.rs) for Copy. Sniffed.
   */
  fetchMessageImage: (url: string) => call<ImageBytes>("fetch_message_image", { url }),
  /** Image viewer: save a message-body picture (data: image or https) to Downloads; returns the path. */
  saveMessageImage: (url: string, filename: string) => call<string>("save_message_image", { url, filename }),
  /**
   * Save all images: every picture of one message (its attachments by id,
   * body pictures by source, each checked like the single saves) into a new
   * `<subject> images` folder in Downloads. The folder can be revealed.
   */
  saveMessageImages: (accountId: string, messageId: string, items: SaveAllItem[]) =>
    call<SavedImages>("save_message_images", { accountId, messageId, items }),
  // Drag out, Save As, Show in Finder (src-tauri/src/file_export.rs). macOS only; invalidInput elsewhere.
  /** A message-body picture (same sources and checks as saveMessageImage) written as a drag-out file. */
  prepareImageDrag: (url: string, filename: string) => call<DragFile>("prepare_image_drag", { url, filename }),
  /** An attachment written as a drag-out file under its own name. */
  prepareAttachmentDrag: (accountId: string, messageId: string, attachmentId: string) =>
    call<DragFile>("prepare_attachment_drag", { accountId, messageId, attachmentId }),
  /** Copy an attachment to the clipboard as a file (a file URL, as Finder copies), so a paste anywhere gives the file. */
  copyAttachmentFile: (accountId: string, messageId: string, attachmentId: string) =>
    call<DragFile>("copy_attachment_file", { accountId, messageId, attachmentId }),
  /** Start a native file drag of a prepared file (call from dragstart, button still down). */
  startFileDrag: (path: string) => call<void>("start_file_drag", { path }),
  /** Save As… panel for a message-body picture; the saved path, or null when cancelled. */
  saveImageAs: (url: string, filename: string) => call<string | null>("save_image_as", { url, filename }),
  /** Save As… panel for an attachment; the saved path, or null when cancelled. */
  saveAttachmentAs: (accountId: string, messageId: string, attachmentId: string) =>
    call<string | null>("save_attachment_as", { accountId, messageId, attachmentId }),
  /** Show in Finder: a path this session saved (saveAttachment, saveMessageImage, Save As). */
  revealSavedPath: (path: string) => call<void>("reveal_saved_path", { path }),

  // Share links (src-tauri/src/share/, docs/SHARE-LINKS.md). Config set,
  // clear and test are main-window only (Settings).
  shareLinkConfigGet: () => call<ShareLinkConfig>("share_link_config_get"),
  shareLinkConfigSet: (config: ShareLinkConfigInput) => call<ShareLinkConfig>("share_link_config_set", { config }),
  /** Forget the storage: settings back to defaults, the secret out of the Keychain. */
  shareLinkConfigClear: () => call<ShareLinkConfig>("share_link_config_clear"),
  /** Upload a small file, fetch it through a link, check the bucket is private, delete it. */
  shareLinkConfigTest: (config: ShareLinkConfigInput) => call<ShareTestReport>("share_link_config_test", { config }),
  /** Upload an attachment or body picture and presign a link (notConfigured until set up). */
  shareFile: (request: ShareRequest) => call<SharedLink>("share_file", { request }),
  /** Delete an upload now (only keys Penguin recorded). */
  shareDelete: (key: string) => call<void>("share_delete", { key }),

  // search
  search: (request: SearchRequest) => call<SearchResponse>("search", { request }),
  /**
   * "Also search Gmail": messages.list per account (cheap), up to 50 unknown
   * matches per account stored headers-only, every match returned. Emits
   * mail-changed for the threads it added. `accountIds` (a profile) narrows.
   */
  searchServer: (query: string, accountIds: string[] | null = null) =>
    call<ServerSearchResponse>("search_server", { query, accountIds }),

  // sync window
  /** Per-account message counts and ETAs for a window (one cached Gmail list call each). */
  syncWindowEstimate: (months: SyncWindowMonths, accountIds: string[] | null = null) =>
    call<WindowEstimate[]>("sync_window_estimate", { months, accountIds }),
  /** Full vs headers-only coverage per account; local only. */
  syncCoverage: (accountIds: string[] | null = null) => call<SyncCoverage>("sync_coverage", { accountIds }),
  /**
   * Drop stored bodies older than the window (headers stay searchable), then
   * compact the database. `dryRun` only counts. `months` previews/applies a
   * window other than the saved one. Rejects for "everything" (0).
   */
  freeUpSpace: (dryRun: boolean, accountIds: string[] | null = null, months: SyncWindowMonths | null = null) =>
    call<FreeUpSpace>("free_up_space", { dryRun, accountIds, months }),

  // misc
  openExternal: (url: string) => call<void>("open_external", { url }),
  /** The offer, after learning whether one-click works when `needsCheck` (one header fetch). */
  unsubscribeCheck: (accountId: string, messageId: string) =>
    call<UnsubscribeOffer | null>("unsubscribe_check", { accountId, messageId }),
  /** Act on the confirmed method (Rust re-derives the target; the UI never passes a URL). */
  unsubscribe: (accountId: string, messageId: string, method: UnsubscribeMethod) =>
    call<UnsubscribeOutcome>("unsubscribe", { accountId, messageId, method }),

  // text services (context menus: app/textMenu.ts; macOS, src-tauri/src/text_services.rs)
  /**
   * Menu Paste: AppKit's paste: action into the focused field (what ⌘V does).
   * The clipboard never reaches JS. False if nothing took the paste.
   */
  nativePaste: () => call<boolean>("native_paste"),
  /** Write plain text to the macOS clipboard (for native menu items, which aren't page clicks). */
  copyText: (text: string) => call<void>("copy_text", { text }),
  /** macOS spell checker verdict + guesses for one word. */
  spellCheck: (word: string) => call<SpellCheck>("spell_check", { word }),
  /** Add a word to the user's macOS dictionary. */
  learnSpelling: (word: string) => call<void>("learn_spelling", { word }),
  /** Look a word or short phrase up in Dictionary.app. */
  lookUp: (text: string) => call<void>("look_up", { text }),
  /** HTML entering the composer (paste) through penguin-render's strict editor allowlist. */
  sanitizeComposeHtml: (html: string) => call<string>("sanitize_compose_html", { html }),

  // settings + diagnostics
  getSettings: () => call<Settings>("get_settings"),
  /** Merge `patch` into the stored settings; resolves with the full result. */
  updateSettings: (patch: SettingsPatch) => call<Settings>("update_settings", { patch }),
  /** Agents (MCP server + CLI) status and copyable client config for Settings → Developer. Change the level via updateSettings({ mcp }). */
  mcpInfo: () => call<McpInfo>("mcp_info"),
  /** Let agents send, after the confirmation: `acknowledgement` is what the user typed ("I understand"). */
  enableAgentSend: (acknowledgement: string) => call<Settings>("enable_agent_send", { acknowledgement }),
  /** The newest agent calls from the audit log, newest first (counts and ids, never content). */
  agentActivity: (limit = 30) => call<AgentActivity[]>("agent_activity", { limit }),
  /** Sends agents queued that haven't gone yet, soonest first. Cancel with cancelScheduledSend. */
  agentPendingSends: () => call<AgentPendingSend[]>("agent_pending_sends"),
  /** Undo an agent's organizing (the toast after penguin://agent-organized): runs its undo steps in order, in the background. */
  agentUndo: (steps: AgentUndoStep[]) => call<void>("agent_undo", { steps }),
  /** Search by meaning: model download and indexing progress (local, instant). */
  semanticStatus: () => call<SemanticIndexStatus>("semantic_status"),
  /** Welcome setup: search by meaning confirmed on, so the one-time model download may start now (docs/ONBOARDING.md). */
  startModelDownload: () => call<void>("start_model_download"),
  /** Is ~/.local/bin/penguin linked to this app's CLI, and is ~/.local/bin on PATH? */
  cliInstallStatus: () => call<CliLinkStatus>("cli_install_status"),
  /** Symlink ~/.local/bin/penguin → the bundled CLI (creates ~/.local/bin; no admin). */
  installCli: () => call<CliLinkStatus>("install_cli"),
  /** Counts, sizes and paths only — no message content, subjects or tokens. */
  diagnostics: () => call<Diagnostics>("diagnostics"),
  /** Per-table/index sizes (dbstat). Slower than diagnostics on a big DB, so it's separate. */
  diagnosticsTableSizes: () => call<TableSizes>("diagnostics_table_sizes"),
  // sender avatars (src-tauri/src/avatars/)
  /**
   * Photos/logos for up to 500 senders, from local state only (never waits
   * on the network). Unknown senders are resolved in the background, then
   * penguin://avatars-changed names them.
   */
  avatarLookup: (requests: AvatarRequest[]) => call<AvatarInfo[]>("avatar_lookup", { requests }),
  /** Per-account contact-photo state and cache size (Settings → Privacy). */
  avatarStatus: () => call<AvatarStatus>("avatar_status"),
  /**
   * Browser sign-in asking this account for the read-only contacts scopes
   * (incremental), then a contacts sync. Turns senderPhotos.contacts on.
   * Rejects `cancelled` after cancelSignIn.
   */
  connectContactPhotos: (accountId: string) => call<AvatarStatus>("connect_contact_photos", { accountId }),
  /** Delete every cached photo, logo and miss (contacts re-sync if enabled). */
  clearAvatarCache: () => call<AvatarStatus>("clear_avatar_cache"),
  /**
   * The account's own Google profile photo as an `avatar:` URL, or null when
   * it only has Google's letter avatar. Cached for a day; may ask Google, so
   * only lib/accountPhotos.ts calls it (once per account per session).
   */
  accountPhoto: (accountId: string) => call<string | null>("account_photo", { accountId }),

  // Settings → You (src-tauri/src/me.rs)
  /** Store a cropped square photo (base64 PNG/JPEG/WebP): re-encoded as a 256 px PNG; resolves with the settings (me.photo set). */
  setMePhoto: (png: string) => call<Settings>("set_me_photo", { png }),
  /** This account's own Google profile photo (profile scope; no contacts access needed). notFound when it only has the letter avatar. */
  setMePhotoFromGoogle: (accountId: string) => call<Settings>("set_me_photo_from_google", { accountId }),
  clearMePhoto: () => call<Settings>("clear_me_photo"),
  /** The stored photo as a data:image/png URL, or null. */
  mePhoto: () => call<string | null>("me_photo"),
  /**
   * Native menu bar: which items are enabled (src-tauri/src/app_menu.rs).
   * The menu is window chrome, not mail data, so inside Tauri it is real even
   * in a mock build (a demo bundle gets a working menu bar).
   */
  /** The newest ~1 MB of penguin.log (Settings → Developer → View log). */
  readLog: () => call<LogTail>("read_log"),
  /** Restart into the update that's already installed (after penguin://update "ready"). */
  restartToUpdate: () => call<void>("restart_to_update"),
  /** Same as Penguin → Check for Updates…; progress arrives as penguin://update. */
  checkForUpdates: () => call<void>("check_for_updates"),
  setMenuContext: (context: MenuContext) =>
    inTauri ? invoke<void>("set_menu_context", { context }) : call<void>("set_menu_context", { context }),

  // Windows (src-tauri/src/windows.rs). Window chrome: real inside Tauri even
  // in demo mode (the new window runs on the demo mock too); the browser mock
  // opens a popup with the same query.
  /** Open a conversation or a composer in a window of its own (a conversation's second open focuses its window). */
  openWindow: (request: WindowRequest) =>
    inTauri ? invoke<OpenedWindow>("open_window", { request }) : call<OpenedWindow>("open_window", { request }),
  /** The editor state another window handed to this composer window, once (null: none). */
  takeWindowSeed: () =>
    inTauri ? invoke<unknown>("take_window_seed") : call<unknown>("take_window_seed", { label: thisWindowLabel }),

  // New-mail notifications (src-tauri/src/notify.rs).
  /** What's on screen, so new mail already visible doesn't notify. Sent when it changes. */
  setNotifyContext: (context: NotifyContext) => call<void>("set_notify_context", { context }),
  notificationPermission: () => call<NotificationPermission>("notification_permission"),
  /** Asked the first time notifications are turned on. */
  requestNotificationPermission: () => call<NotificationPermission>("request_notification_permission"),
  /** Shows "New mail will show up like this." */
  testNotification: () => call<void>("test_notification"),

  // Google Calendar (src-tauri/src/calendar/). Reads are local only.
  /** Per-account connection, calendars and sync state (Settings → Calendar). */
  calendarStatus: () => call<CalendarStatus>("calendar_status"),
  /**
   * Browser sign-in asking this account for calendar.readonly (plus
   * calendar.events when `rsvp`), then an immediate sync. Rejects `cancelled`
   * after cancelSignIn.
   */
  connectCalendar: (accountId: string, rsvp = false) => call<CalendarStatus>("connect_calendar", { accountId, rsvp }),
  /** Show/hide one calendar; hiding drops its events, showing syncs it. */
  setCalendarSelected: (accountId: string, calendarId: string, selected: boolean) =>
    call<CalendarStatus>("set_calendar_selected", { accountId, calendarId, selected }),
  /** Events overlapping [fromMs, toMs) (≤400 days), all-day first then by start. */
  listEvents: (fromMs: number, toMs: number, accountIds: string[] | null = null) =>
    call<CalendarEvent[]>("list_events", { fromMs, toMs, accountIds }),
  getEvent: (accountId: string, calendarId: string, eventId: string) =>
    call<EventDetail | null>("get_event", { accountId, calendarId, eventId }),
  /** The invite card for a thread, or null. May fetch the .ics part once (then cached). */
  eventInvite: (accountId: string, threadId: string) => call<InviteCard | null>("event_invite", { accountId, threadId }),
  /** Last and next event with this person. */
  personMeetings: (email: string) => call<PersonMeetings>("person_meetings", { email }),
  /** Needs the RSVP opt-in. Google notifies the organizer. */
  respondToEvent: (accountId: string, calendarId: string, eventId: string, response: "accepted" | "tentative" | "declined") =>
    call<CalendarEvent>("respond_to_event", { accountId, calendarId, eventId, response }),
  /**
   * Answer the invitation in a thread (explicit click only): through Google
   * Calendar, Microsoft Graph or an iMIP email to the organizer (card.route).
   * `proposal` makes it a "propose new time". Returns the card as it is now.
   */
  respondToInvite: (args: {
    accountId: string;
    threadId: string;
    messageId: string | null;
    response: InviteAnswer;
    comment?: string | null;
    proposal?: { start: number; end: number } | null;
  }) =>
    call<InviteCard>("respond_to_invite", {
      ...args,
      comment: args.comment ?? null,
      proposal: args.proposal ?? null,
    }),
  calendarSyncNow: () => call<void>("calendar_sync_now"),

  // rules & automations (src-tauri/src/rules/; Settings → Rules)
  listRules: () => call<RulesOverview>("list_rules"),
  /** Create (id null) or replace. Trash/forward actions need `confirmed: true`. */
  saveRule: (rule: RuleInput) => call<Rule>("save_rule", { rule }),
  /** Also forgets the rule's history. */
  deleteRule: (id: string) => call<void>("delete_rule", { id }),
  /** Turning a paused rule on clears its pause. */
  setRuleEnabled: (id: string, enabled: boolean) => call<Rule>("set_rule_enabled", { id, enabled }),
  /** Every rule id exactly once, in the new evaluation order. */
  reorderRules: (ids: string[]) => call<void>("reorder_rules", { ids }),
  /** Settings → Developer → "Allow hooks" (shell hooks and webhooks). */
  setAllowHooks: (allow: boolean) => call<RulesOverview>("set_allow_hooks", { allow }),
  /** Live match count + newest matches for a condition (local only). `ruleId` marks what that rule already did. */
  previewRule: (req: {
    condition: string;
    accountIds: string[] | null;
    profileId: string | null;
    ruleId: string | null;
    limit?: number;
  }) => call<RulePreview>("preview_rule", { limit: null, ...req }),
  /** "Run now" / "Also apply to existing mail": up to 1,000 matches the rule hasn't handled. */
  runRule: (id: string, dryRun: boolean) => call<RuleRunReport>("run_rule", { id, dryRun }),
  /** Newest first; ruleId null = every rule. */
  ruleHistory: (ruleId: string | null, limit = 100, beforeId: number | null = null) =>
    call<RuleLogEntry[]>("rule_history", { ruleId, limit, beforeId }),
  /** Reverse the label/archive/read/star/trash actions of these history rows. Resolves with rows undone. */
  undoRuleActions: (logIds: number[]) => call<number>("undo_rule_actions", { logIds }),

  /** Show one of Penguin's own directories in Finder. */
  revealPath: (target: RevealTarget) => call<void>("reveal_path", { target }),
  /** Merge search index segments (Store::optimize). */
  optimizeIndex: () => call<void>("optimize_index"),
};

/** A browser sign-in started, or Penguin couldn't open the browser for it (`error`). */
export function onSignInUrl(cb: (e: SignInLink) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.signInUrl, cb as (p: unknown) => void));
  return listenHere<SignInLink>(EVENTS.signInUrl, (e) => cb(e.payload));
}

export function onSyncStatus(cb: (s: SyncStatus) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.syncStatus, cb as (p: unknown) => void));
  return listenHere<SyncStatus>(EVENTS.syncStatus, (e) => cb(e.payload));
}

export function onMailChanged(cb: (e: MailChangedEvent) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.mailChanged, cb as (p: unknown) => void));
  return listenHere<MailChangedEvent>(EVENTS.mailChanged, (e) => cb(e.payload));
}

export function onActionFailed(cb: (e: ActionFailedEvent) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.actionFailed, cb as (p: unknown) => void));
  return listenHere<ActionFailedEvent>(EVENTS.actionFailed, (e) => cb(e.payload));
}

export function onBodyFetchFailed(cb: (e: BodyFetchFailedEvent) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.bodyFetchFailed, cb as (p: unknown) => void));
  return listenHere<BodyFetchFailedEvent>(EVENTS.bodyFetchFailed, (e) => cb(e.payload));
}

export function onSettingsChanged(cb: (s: Settings) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.settingsChanged, cb as (p: unknown) => void));
  return listenHere<Settings>(EVENTS.settingsChanged, (e) => cb(e.payload));
}

/**
 * A click on a picture in a message body, relayed by the native link guard
 * (payload `{ nonce, index }`, validated again by image-viewer/bridge.ts).
 * The payload is typed unknown on purpose: the receiver checks its schema.
 */
export function onImageOpen(cb: (payload: unknown) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.imageOpen, cb));
  return listenHere<unknown>(EVENTS.imageOpen, (e) => cb(e.payload));
}

/** Narrow an unknown rejection to CommandError. */
export function asCommandError(e: unknown): { code: string; message: string } {
  if (e && typeof e === "object" && "code" in e && "message" in e) return e as { code: string; message: string };
  return { code: "other", message: e instanceof Error ? e.message : String(e) };
}

export function onUpdate(cb: (e: UpdateEvent) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.update, cb as (p: unknown) => void));
  return listenHere<UpdateEvent>(EVENTS.update, (e) => cb(e.payload));
}

export function onAvatarsChanged(cb: (e: AvatarsChangedEvent) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.avatarsChanged, cb as (p: unknown) => void));
  return listenHere<AvatarsChangedEvent>(EVENTS.avatarsChanged, (e) => cb(e.payload));
}

export function onScheduledSent(cb: (e: ScheduledSentBatch) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.scheduledSent, cb as (p: unknown) => void));
  return listenHere<ScheduledSentBatch>(EVENTS.scheduledSent, (e) => cb(e.payload));
}

export function onReminderDue(cb: (e: ReminderDue) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.reminderDue, cb as (p: unknown) => void));
  return listenHere<ReminderDue>(EVENTS.reminderDue, (e) => cb(e.payload));
}

/** An agent's send is waiting in the outbox (src-tauri/src/agent_app.rs). */
export function onAgentSendQueued(cb: (e: AgentSendQueued) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.agentSendQueued, cb as (p: unknown) => void));
  return listenHere<AgentSendQueued>(EVENTS.agentSendQueued, (e) => cb(e.payload));
}

/** An agent organized mail (archive, labels, snooze, Trash…); the payload carries its undo. */
export function onAgentOrganized(cb: (e: AgentOrganized) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.agentOrganized, cb as (p: unknown) => void));
  return listenHere<AgentOrganized>(EVENTS.agentOrganized, (e) => cb(e.payload));
}

/** A new-mail notification was clicked (src-tauri/src/notify.rs). */
export function onNotificationOpen(cb: (e: NotificationOpen) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.notificationOpen, cb as (p: unknown) => void));
  return listenHere<NotificationOpen>(EVENTS.notificationOpen, (e) => cb(e.payload));
}

/** A native menu bar item was chosen (src-tauri/src/app_menu.rs); real inside Tauri even in a mock build. */
export function onSnoozeWoke(cb: (e: SnoozeWokeBatch) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.snoozeWoke, cb as (p: unknown) => void));
  return listenHere<SnoozeWokeBatch>(EVENTS.snoozeWoke, (e) => cb(e.payload));
}

export function onMenu(cb: (e: MenuEvent) => void): Promise<UnlistenFn> {
  if (!inTauri) return mockBackend().then((m) => m.listen(EVENTS.menu, cb as (p: unknown) => void));
  return listenHere<MenuEvent>(EVENTS.menu, (e) => cb(e.payload));
}

export function onCalendarChanged(cb: (e: CalendarChangedEvent) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.calendarChanged, cb as (p: unknown) => void));
  return listenHere<CalendarChangedEvent>(EVENTS.calendarChanged, (e) => cb(e.payload));
}

export function onRulesChanged(cb: (e: RulesOverview) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.rulesChanged, cb as (p: unknown) => void));
  return listenHere<RulesOverview>(EVENTS.rulesChanged, (e) => cb(e.payload));
}

/** A rule acted on (or, in dry-run, matched) mail. */
export function onRuleFired(cb: (e: RuleFiredEvent) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.ruleFired, cb as (p: unknown) => void));
  return listenHere<RuleFiredEvent>(EVENTS.ruleFired, (e) => cb(e.payload));
}

/** Text being written by write_with_ai, so far. */
export function onWriteProgress(cb: (e: WriteProgress) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.writeProgress, cb as (p: unknown) => void));
  return listenHere<WriteProgress>(EVENTS.writeProgress, (e) => cb(e.payload));
}

/** A summary in progress: notes on part of a long thread, or the summary so far. */
export function onSummaryProgress(cb: (e: SummaryProgress) => void): Promise<UnlistenFn> {
  if (isMock) return mockBackend().then((m) => m.listen(EVENTS.summaryProgress, cb as (p: unknown) => void));
  return listenHere<SummaryProgress>(EVENTS.summaryProgress, (e) => cb(e.payload));
}
