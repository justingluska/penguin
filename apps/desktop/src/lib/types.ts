// Mirror of crates/penguin-core/src/types.rs (+ app-layer view types from
// src-tauri). Keep in lockstep with the Rust side — this is the contract.

export type AccountId = string;

export interface Account {
  id: AccountId;
  email: string;
  /** The person's Google profile name (used in From headers). */
  displayName: string | null;
  /** Short user-set label ("Sam Work"), shown wherever accounts are named. Null = derived from the address. */
  nickname: string | null;
  /** #rrggbb; mapped to a theme-aware tone in the UI. */
  color: string;
  addedAt: number;
  /** Which backend the account syncs with (accounts from before providers are "gmail"). */
  provider: AccountProvider;
  /** How to reach the provider (servers, client id). Never a password or token. */
  providerConfig: ProviderConfig;
  /** What the provider can do; hide what it can't. Derived from `provider` by the backend. */
  capabilities: Capabilities;
}

/** The mail backend of an account (penguin-core AccountProvider). */
export type AccountProvider = "gmail" | "imap" | "microsoft";

/** Account.capabilities (penguin-core Capabilities; docs/PROVIDERS-IMPL.md → Capabilities). */
export interface Capabilities {
  /** Several labels per message, added and removed freely (Gmail labels, Microsoft categories). */
  labels: boolean;
  /** User labels are folders: a message lives in one, and "adding" one moves it (IMAP, Microsoft). */
  folders: boolean;
  /** Rename, recolor, hide and delete user labels. */
  labelEdit: boolean;
  /** Label colors from Gmail's palette. */
  labelColors: boolean;
  /** IMPORTANT and CATEGORY_* labels exist (split-inbox tabs). */
  inboxCategories: boolean;
  /** "Also search <provider>" (search_server). */
  serverSearch: boolean;
  /** Per-window message counts in Settings → Sync. */
  windowEstimate: boolean;
  /** Server drafts (save_draft, get_draft, delete_draft). */
  drafts: boolean;
  /** Send later (needs drafts). */
  sendLater: boolean;
  snooze: boolean;
  /** Google Calendar. */
  calendar: boolean;
  /** Google contact photos for senders. */
  contactPhotos: boolean;
  /** The account's own profile photo (account_photo). */
  profilePhoto: boolean;
  /** Every header on demand (Message details). */
  messageHeaders: boolean;
  /** Raw source ("Show original"). */
  rawSource: boolean;
}

/** Account.providerConfig: only the fields its provider uses are set (Gmail: none). */
export interface ProviderConfig {
  /** IMAP: app password or account password. */
  auth?: AuthMethod | null;
  imap?: ServerSettings | null;
  smtp?: ServerSettings | null;
  /** Microsoft: the Application (client) ID its token belongs to. */
  clientId?: string | null;
  tenantId?: string | null;
  /** Who hosts the mailbox as detection saw it (a ProviderKind, e.g. "icloud"). */
  host?: string | null;
}

/** update_account argument: omitted fields are unchanged; nickname null (or blank) clears it. */
export interface AccountPatch {
  nickname?: string | null;
  color?: string;
}

export interface Address {
  name: string | null;
  email: string;
}

export interface AttachmentMeta {
  id: string;
  filename: string;
  mimeType: string;
  size: number;
  contentId: string | null;
  inline: boolean;
}

export interface Label {
  accountId: AccountId;
  id: string;
  name: string;
  kind: "system" | "user";
  color: string | null;
  unreadCount: number | null;
  /** Hidden from Gmail's label list (labelListVisibility "labelHide"). */
  hidden: boolean;
}

/**
 * update_label argument: omitted fields are unchanged. `name` is trimmed
 * (non-empty, ≤225 chars, "/" nests); `color` is a background hex from
 * LABEL_COLORS (lib/labelColors.ts), null removes it; `hidden` hides the
 * label from Gmail's label list.
 */
export interface LabelPatch {
  name?: string;
  color?: string | null;
  hidden?: boolean;
}

export interface ThreadSummary {
  accountId: AccountId;
  threadId: string;
  subject: string;
  snippet: string;
  participants: Address[];
  messageCount: number;
  unread: boolean;
  starred: boolean;
  hasAttachments: boolean;
  labelIds: string[];
  lastDate: number;
  /** Verification code or sign-in link in the newest message (penguin-core otp.rs). */
  otp?: Otp | null;
  /** Unix ms the thread is snoozed until; null/absent when it isn't snoozed. */
  snoozedUntil?: number | null;
  /** Calendar invitation in the thread, for the row's event chip (Store::attach_invites). */
  invite?: InviteChip | null;
  /** Smart views only: what the row is about (penguin-core store_smart.rs). Absent elsewhere. */
  smart?: SmartRow | null;
}

/** A one-time code or magic sign-in link found in a message. */
export interface Otp {
  kind: "code" | "link";
  /** What to copy (leading zeros kept). Null for "link". */
  code: string | null;
  /** Sender passed DMARC / aligned DKIM. Unverified codes show muted, never auto-copy. */
  verified: boolean;
  /** Unix ms of the message (codes go stale in minutes). */
  date: number;
}

export type MailboxView =
  | { kind: "inbox" }
  | { kind: "starred" }
  | { kind: "sent" }
  | { kind: "drafts" }
  | { kind: "done" }
  | { kind: "trash" }
  | { kind: "spam" }
  | { kind: "all" }
  | { kind: "label"; labelId: string }
  /** Snoozed conversations, soonest wake first (local records, not a Gmail label). */
  | { kind: "snoozed" }
  /** Every account's "Reply Later" label (REPLY_LATER_LABEL), merged, newest first. */
  | { kind: "replyLater" }
  /**
   * Waiting on a reply: your message is the latest and was sent between 60 and
   * Settings.followUpDays days ago; oldest first, one page. `lastDate` is when it was sent.
   */
  | { kind: "followUp" }
  /**
   * A smart view (SMART_VIEWS, or "files:pdf" | "files:images" | "files:docs" |
   * "files:sheets"); rows carry `smart`. The id rides in `labelId`, as the backend's
   * MailboxView is adjacently tagged.
   */
  | { kind: "smart"; labelId: string }
  /** A saved search pinned as a view: `labelId` is the query itself. Newest match first, one page. */
  | { kind: "query"; labelId: string };

/** Every smart view id, in the default order (penguin-core SMART_VIEWS). */
export const SMART_VIEWS = [
  "receipts",
  "travel",
  "packages",
  "bills",
  "reservations",
  "invites",
  "codes",
  "files",
  "newsletters",
  "subscriptions",
  "people",
] as const;
export type SmartViewId = (typeof SMART_VIEWS)[number];
/** The Files view's type filters (SMART_FILE_KINDS). */
export const SMART_FILE_KINDS = ["pdf", "images", "docs", "sheets"] as const;
export type SmartFileKind = (typeof SMART_FILE_KINDS)[number];

/** What a smart view row is about, shown in place of the snippet (penguin-core SmartRow). */
export interface SmartRow {
  /** receipt | flight | stay | car | train | bus | parcel | bill | restaurant | event | reservation | subscription | file | invite | code | link */
  kind: string;
  /** Merchant, "SFO → LIS", hotel, carrier, biller, venue, file name. */
  title: string;
  amount: Money | null;
  /** Local wall time or date ("2026-10-02T19:05", "2026-10-02"). */
  at: string | null;
  /** Check-out, drop-off, a subscription's next expected charge. */
  end: string | null;
  /** Confirmation, order, tracking or invoice number. */
  reference: string | null;
  /** overdue | due | paid | unpaid | shipped | inTransit | outForDelivery | delivered | exception | cancelled | refunded | returned | upcoming | past | active | stopped */
  status: string | null;
  detail: string | null;
  /** The list section ("Trip to Lisbon · Oct 2–9", "Overdue", "September 2026"); null = day groups. */
  group: string | null;
}

/** The slim header over a smart view (smart_view_info). */
export interface SmartViewInfo {
  view: string;
  stats: SmartStat[];
  /** A caveat ("Still reading 1,204 emails …"). */
  note: string | null;
}

/** One figure: amounts per currency (never converted), or a text value. */
export interface SmartStat {
  label: string;
  value: string | null;
  amounts: Money[];
  tone: "warn" | "good" | null;
}

/** A smart view's sidebar count (smart_counts). */
export interface SmartCount {
  view: string;
  count: number;
}

/** The label (Gmail), folder (IMAP) or category (Microsoft) behind Reply Later. */
export const REPLY_LATER_LABEL = "Reply Later";
/** Follow up's wait in days: default and range (penguin-core FOLLOW_UP_DAYS_*). */
export const FOLLOW_UP_DAYS = { def: 3, min: 1, max: 14 } as const;
/** Follow up looks no further back than this many days (FOLLOW_UP_LOOKBACK_DAYS). */
export const FOLLOW_UP_LOOKBACK_DAYS = 60;

/** Sizes of the Reply Later and Follow up views for one account (sidebar counts). */
export interface TriageCount {
  accountId: AccountId;
  replyLater: number;
  followUp: number;
}

export type InboxTab = "all" | "important" | "other" | "newsletters";

/**
 * Split Inbox (penguin-core store_split.rs): one tab of the inbox. A
 * conversation belongs to the first split whose query matches one of its
 * inbox messages; Other (include null) holds the rest.
 */
export interface SplitFilter {
  /** This split's query; null = Other. */
  include: string | null;
  /** The queries of the splits before it. */
  exclude: string[];
}

export interface SplitCount {
  total: number;
  unread: number;
}

/** split_counts: one entry per split query asked, then Other. `more` = stopped at the cap (a floor). */
export interface SplitCounts {
  splits: SplitCount[];
  more: boolean;
}

/** One tab of the Split Inbox (Settings.inboxSplits). */
export interface InboxSplit {
  /** 1–64 of [A-Za-z0-9_-]. */
  id: string;
  /** ≤30 chars; blank = the query. */
  name: string;
  /** In the search language, ≤500 chars. */
  query: string;
  /** Leave the tab out while it has nothing in it. */
  hideWhenEmpty: boolean;
}

export interface ListQuery {
  view: MailboxView;
  tab: InboxTab | null;
  accountId: AccountId | null;
  /** A set of accounts (a profile); intersected with accountId. Omitted/null = no restriction. */
  accountIds?: AccountId[] | null;
  limit: number;
  before: number | null;
  /** Only threads with unread mail in this view (the list's Unread filter). Omitted = false. */
  unreadOnly?: boolean;
  /** Split Inbox: with the inbox view, list one split (`tab` is then ignored). Omitted/null = the whole inbox. */
  split?: SplitFilter | null;
}

/** App-layer message view: raw html is replaced by sanitized render output. */
export interface MessageView {
  accountId: AccountId;
  id: string;
  threadId: string;
  date: number;
  from: Address;
  to: Address[];
  cc: Address[];
  bcc: Address[];
  replyTo: Address[];
  subject: string;
  snippet: string;
  bodyText: string;
  /** Sanitized full HTML document for a sandboxed iframe srcdoc. */
  html: string;
  blockedRemoteImages: number;
  /** Distinct tracker URLs removed; can exceed the sum of removed `trackers[].count` when the list was capped. */
  trackersRemoved: number;
  /** Every tracker found (host + path, never the query string) with its `status`, at most 50 entries. */
  trackers: TrackerRemoved[];
  /**
   * Trackers found but not removed, because Settings → Privacy "Block tracking
   * pixels" is off: held back with the remote images, or loaded with them.
   * Always sent by the backend; optional only so hand-built mock views needn't set it.
   */
  trackersAllowed?: number;
  /** Links whose tracking parameters were removed ("Remove tracking from links" on). Optional like trackersAllowed. */
  linksCleaned?: number;
  /** Names of the parameters removed from links (never their values), e.g. ["mc_eid", "fbclid"]. */
  linkParams?: string[];
  /** Links that go through the sender's click-tracking redirect (can't be cleaned). */
  trackedLinks?: number;
  labelIds: string[];
  attachments: AttachmentMeta[];
  unread: boolean;
  starred: boolean;
  /**
   * The sender is on Settings → "always load images from", but the message
   * isn't authenticated as theirs, so images stayed blocked. Always sent by
   * the backend; optional only so hand-built mock views needn't set it.
   */
  trustedSenderUnverified?: boolean;
  /**
   * Only headers are stored (mail older than the sync window). get_thread
   * has started downloading the body; a mail-changed for this thread follows.
   * Optional only so hand-built mock views needn't set it.
   */
  bodyPending?: boolean;
  /**
   * Gmail authenticated the From address (DMARC pass / aligned DKIM). Brand
   * logos (avatars) only show on authenticated messages. Optional only so
   * hand-built mock views needn't set it.
   */
  senderAuthenticated?: boolean;
  /** Verification code / sign-in link in this message. */
  otp?: Otp | null;
  /** What the Unsubscribe button does; null/absent = no button. */
  unsubscribe?: UnsubscribeOffer | null;
  /** Read receipts that came back for this message you sent, oldest first. Optional like trackersAllowed. */
  readReceipts?: ReadReceipt[];
}

/** A read receipt (RFC 8098 MDN) for a message you sent; mirrors penguin_core::store::ReadReceipt. */
export interface ReadReceipt {
  /** Message-ID of the sent message, without <>. */
  originalMessageId: string;
  /** Who sent the receipt (lowercased), when it says. */
  recipient: string | null;
  /** "displayed" = read; also "deleted", "dispatched", "processed". */
  disposition: string;
  /** When the receipt arrived (unix ms). */
  date: number;
  /** The receipt email's own id. */
  messageId: string;
}

/**
 * Why an image was treated as a tracker (penguin_render::TrackerKind):
 *   knownTracker: matched Penguin's tracker list (host, host family or path)
 *   hiddenImage:  a tiny (3×3 px or smaller) or hidden image from any host
 */
export type TrackerKind = "knownTracker" | "hiddenImage";

/**
 * What happened to a tracker (penguin_render::TrackerStatus):
 *   removed: blocked ("Block tracking pixels" on, the default)
 *   held:    blocking off; waiting with the message's other remote images
 *   loaded:  blocking off and images were loaded, so it loaded too
 */
export type TrackerStatus = "removed" | "held" | "loaded";

/** One tracker found in a message (penguin_render::TrackerRemoved; the name predates `status`). */
export interface TrackerRemoved {
  /** Lowercased host, e.g. "pixel.mailerlite.example". */
  host: string;
  /** URL path without query or fragment, capped at 80 characters ("…"). */
  path: string;
  kind: TrackerKind;
  /** The company behind a known tracker, when the list names one. */
  company: string | null;
  /** The list entry that matched ("host awstrack.me", "path /wf/open"), or "hidden image". */
  rule: string;
  /** At least one removed URL had a query string (often your address or a per-recipient id). */
  hadQuery: boolean;
  /** Distinct URLs folded into this entry (same host, path and kind). */
  count: number;
  status: TrackerStatus;
}

/** Mirrors penguin_core::unsubscribe (Rust). */
export type UnsubscribeMethod = "oneClick" | "mailto" | "link";

export interface UnsubscribeMailto {
  /** One address; the email goes here from the receiving account. */
  to: string;
  subject: string;
  body: string;
}

export interface UnsubscribeRecord {
  /** Unix ms. */
  at: number;
  method: UnsubscribeMethod;
}

/**
 * The Unsubscribe button for one message. The UI never passes a URL back
 * (`unsubscribe` re-plans in Rust); a `link` offer carries `linkUrl` only
 * so the confirm can show where the browser will go.
 *   oneClick: RFC 8058 POST from the app (header + List-Unsubscribe-Post + verified sender)
 *   mailto:   send `mailto` from the receiving account (verified senders; else compose)
 *   link:     open the page in the browser (header https or a body link)
 */
export interface UnsubscribeOffer {
  method: UnsubscribeMethod;
  source: "header" | "body";
  /** Host of the link, or the mailto address's domain. */
  domain: string;
  mailto: UnsubscribeMailto | null;
  /** Gmail authenticated the sender (DMARC / aligned DKIM). */
  verified: boolean;
  /** Stored before Penguin kept List-Unsubscribe-Post: call unsubscribe_check before confirming. */
  needsCheck: boolean;
  /** Already unsubscribed from this sender on this account. */
  unsubscribed: UnsubscribeRecord | null;
  /** The page a `link` offer opens, for display in the confirm. Null for oneClick and mailto. */
  linkUrl: string | null;
}

/** unsubscribe result. */
export interface UnsubscribeOutcome {
  method: UnsubscribeMethod;
  domain: string;
  sender: string;
  sentTo: string | null;
  record: UnsubscribeRecord;
}

export interface ThreadView {
  accountId: AccountId;
  threadId: string;
  subject: string;
  labelIds: string[];
  /** Oldest first. */
  messages: MessageView[];
}

export interface SearchRequest {
  query: string;
  accountId: AccountId | null;
  /** Limit to a set of accounts (a profile); intersected with accountId and any `account:` operator. */
  accountIds?: AccountId[] | null;
  limit: number;
}

export interface SearchChip {
  kind: string;
  label: string;
  raw: string;
}

export interface SearchHit {
  accountId: AccountId;
  threadId: string;
  messageId: string;
  subject: string;
  from: Address;
  date: number;
  /** Escaped HTML with <mark> around matched terms. */
  snippetHtml: string;
  matchCount: number;
  labelIds: string[];
  hasAttachments: boolean;
  unread: boolean;
  score: number;
  /** How the conversation matched: the typed words (full-text index) and/or
   *  their meaning (search by meaning). Empty for filter-only queries and
   *  "Also search Gmail" results. */
  matchedBy: MatchedBy[];
  /** The best-matching passage by meaning, plain text (not HTML), ≤240
   *  characters; null unless matchedBy includes "meaning". */
  passage: string | null;
}

export type MatchedBy = "words" | "meaning";

/** Search by meaning: in use ("ready"), still building its index ("indexing":
 *  results already use the part embedded so far, newest mail first), or not set up ("off"). */
export type SemanticStatus = "ready" | "indexing" | "off";

export interface AttachmentHit {
  accountId: AccountId;
  threadId: string;
  messageId: string;
  attachment: AttachmentMeta;
  from: Address;
  date: number;
}

export interface PersonHit {
  address: Address;
  messageCount: number;
}

export interface SearchResponse {
  chips: SearchChip[];
  hits: SearchHit[];
  attachments: AttachmentHit[];
  people: PersonHit[];
  tookMs: number;
  indexedMessages: number;
  /** The query's parsed date range (unix ms, after inclusive / before exclusive); null = open. */
  afterMs?: number | null;
  beforeMs?: number | null;
  /** The "Calendar" group (all results for `type:event`); empty for mail-only filters. */
  events?: CalendarEvent[];
  /** Whether search by meaning took part (see docs/SEARCH-RANKING.md). */
  semantic: SemanticStatus;
  /** Share of mail the meaning index covers (0–1) while `semantic` is "indexing"; null otherwise. */
  semanticProgress: number | null;
}

export type SyncPhase = "idle" | "backfilling" | "incremental" | "error" | "needsReauth";

export interface SyncStatus {
  accountId: AccountId;
  phase: SyncPhase;
  indexed: number;
  totalEstimate: number | null;
  lastSyncedAt: number | null;
  error: string | null;
  /** Backfill throughput over ~the last minute (messages/min), while backfilling. */
  ratePerMin: number | null;
  /** Estimated seconds until backfill completes at the current rate. */
  etaSecs: number | null;
  /**
   * While backfilling: "window" = full messages inside the sync window;
   * "older" = mail older than the window (headers only, or full per olderMail).
   */
  stage?: SyncStage | null;
}

export type SyncStage = "window" | "older";

export type ThreadAction =
  | { kind: "archive" }
  | { kind: "moveToInbox" }
  | { kind: "trash" }
  | { kind: "untrash" }
  | { kind: "markRead" }
  | { kind: "markUnread" }
  | { kind: "star" }
  | { kind: "unstar" }
  | { kind: "addLabel"; labelId: string }
  | { kind: "removeLabel"; labelId: string }
  /** Add the account's Reply Later label, archive, mark read. The UI calls `api.replyLater`, which sends this per account. */
  | { kind: "replyLater"; labelId: string };

export interface ThreadRef {
  accountId: AccountId;
  threadId: string;
}

export interface Draft {
  accountId: AccountId;
  to: Address[];
  cc: Address[];
  bcc: Address[];
  subject: string;
  bodyText: string;
  bodyHtml: string | null;
  replyToThreadId: string | null;
  replyToMessageId: string | null;
  /** Files to send (optional on the wire; defaults to none). Gmail's limit is 25 MB total. */
  attachments?: OutgoingAttachment[];
  /**
   * Ask for a read receipt (RFC 8098 Disposition-Notification-To). Omitted/null:
   * send_message and save_draft use Settings → Privacy "Ask for read receipts".
   */
  requestReadReceipt?: boolean | null;
}

/**
 * An attachment on an outgoing message (mirror of compose.rs). `file` carries
 * the bytes (standard base64); `gmail` points at an attachment already on a
 * Gmail message (a reopened draft, a forward) and the backend fetches it.
 * `contentId` makes it an inline image: the HTML shows it as
 * `<img src="cid:…">` and it goes out as a multipart/related part (one the
 * HTML doesn't show goes as an ordinary file). See penguin-provider inline.rs.
 */
export type OutgoingAttachment =
  | { kind: "file"; filename: string; mimeType: string; dataBase64: string; contentId?: string }
  | {
      kind: "gmail";
      messageId: string;
      attachmentId: string;
      filename: string;
      mimeType: string;
      size: number;
      /** The account the message is stored in when it isn't the sending one (a forward from another account); absent = the sending account. */
      accountId?: string;
      contentId?: string;
    };

/** Gmail's ids for a saved draft (save_draft). The messageId changes on every save; draftId never does. */
export interface DraftRef {
  draftId: string;
  messageId: string;
  threadId: string;
  /**
   * The draft's attachments as refs to the NEW draft message, in the order
   * sent (use them for the next save). Omitted when there are none, or when
   * re-reading them failed (keep your list; the next save retries).
   */
  attachments?: OutgoingAttachment[];
}

/**
 * An original a reply or forward quotes (quote_sources; outgoing.rs
 * QuoteSource). Headers-only originals are downloaded before it returns.
 */
export interface QuoteSource {
  messageId: string;
  /** The HTML body through penguin-render's sanitize_quoted_html (formatting kept, nothing active or remote); null for text-only mail or without withBody. */
  html: string | null;
  /** The text body (from the HTML when there's no text part); null without withBody. */
  text: string | null;
  /** Every attachment, inline ones included. */
  attachments: AttachmentMeta[];
  /**
   * The inline images `html` still shows, as refs to the original's parts
   * under the fresh Content-IDs `html` now uses (accountId set). Attached
   * with the quote; empty without withBody.
   */
  inlineImages: OutgoingAttachment[];
}

/** A draft reopened for editing (get_draft). `draft.bodyHtml` is the stored HTML part through penguin-render's composer allowlist (sanitize_compose_html), or null. */
export interface OpenedDraft {
  draftId: string;
  messageId: string;
  threadId: string;
  draft: Draft;
}

export interface OAuthClientStatus {
  configured: boolean;
  /** Where the client JSON is read from (for the setup screen). */
  path: string;
  /** iOS OAuth client id that enables the macOS system sign-in sheet; null = browser sign-in. */
  iosClientId: string | null;
}

/**
 * The browser sign-in waiting for the user (sign_in_link, penguin://sign-in-url). `url` is Google's authorization
 * link (no secrets); `error` says why Penguin couldn't open the browser with it, null when it opened.
 */
export interface SignInLink {
  url: string;
  error: string | null;
}

// ---------------------------------------------------------------------------
// Providers and Add account (src-tauri/src/providers/mod.rs; change both).
// ---------------------------------------------------------------------------

/** Who hosts the mailbox. */
export type ProviderKind =
  | "gmail"
  | "googleWorkspace"
  | "outlookPersonal"
  | "microsoft365"
  | "yahoo"
  | "aol"
  | "icloud"
  | "fastmail"
  /** Any IMAP/SMTP server (autoconfig, a manual pick, or Proton Mail Bridge). */
  | "imapGeneric"
  | "unknown";

/** How Penguin signs in. */
export type AuthMethod = "googleOAuth" | "microsoftOAuth" | "appPassword" | "imapPassword";

/** Which instructions Add account shows; also a manual "Choose provider" pick. */
export type SetupKind = "google" | "microsoft" | "yahoo" | "aol" | "icloud" | "fastmail" | "proton" | "imap" | "unsupported";

export type DetectionSource = "domainTable" | "mx" | "autoconfig" | "ispdb" | "manual" | "unknown";

/** tls = TLS from the first byte (993/465); starttls = upgraded (143/587); plain = none (only for a server on this Mac). */
export type MailSecurity = "tls" | "starttls" | "plain";

export interface ServerSettings {
  host: string;
  port: number;
  security: MailSecurity;
  username: string;
}

/** detect_provider's answer for one address. */
export interface DetectedProvider {
  /** Trimmed, domain lowercased (IDN as punycode). */
  email: string;
  domain: string;
  kind: ProviderKind;
  displayName: string;
  auth: AuthMethod;
  setup: SetupKind;
  imap: ServerSettings | null;
  smtp: ServerSettings | null;
  source: DetectionSource;
  /** The domain's preferred MX host, when found. */
  mxHost: string | null;
  /** A mail security gateway (Proofpoint, Mimecast, …) hiding the real host. */
  gateway: string | null;
  /** DNS and the web were unreachable; the answer is a guess. */
  offline: boolean;
  /** This build can connect it now. False: the last step says "coming in the next update". */
  available: boolean;
}

/** connect_account's argument (src-tauri/src/providers/connect.rs; the Graph and IMAP providers implement it; see ARCHITECTURE.md → Providers). */
export interface ConnectAccountRequest {
  email: string;
  kind: ProviderKind;
  auth: AuthMethod;
  /** Microsoft: the Application (client) ID. */
  clientId?: string | null;
  /** App password or account password. Keychain only; never logged. */
  password?: string | null;
  imap?: ServerSettings | null;
  smtp?: ServerSettings | null;
}

export interface MicrosoftClientStatus {
  /** The saved Application (client) ID of the user's Entra app; null until added. */
  clientId: string | null;
  path: string;
}

/** Event payloads (see EVENTS in api.ts). */
export interface MailChangedEvent {
  accountId: AccountId;
  threadIds: string[];
}

/** person_summary result (mirror of penguin-core store_people.rs). */
export interface PersonSummary {
  /** Lowercased. */
  email: string;
  name: string | null;
  /** Other addresses stored under the same name. */
  otherAddresses: Address[];
  domain: string;
  /** Unix ms of the first / last message from or to them. */
  firstContact: number | null;
  lastContact: number | null;
  /** They sent you. */
  messagesFrom: number;
  /** You sent them (to/cc/bcc). */
  messagesTo: number;
  /** Your accounts with mail with them, most first: [0] is the one that usually talks to them. */
  accounts: { accountId: AccountId; count: number }[];
  /** ≤5, newest first. */
  recentThreads: { accountId: AccountId; threadId: string; subject: string; date: number }[];
  /** ≤5, newest first. */
  recentAttachments: AttachmentHit[];
}

// ---------- ask (mirror of penguin-core src/ask/mod.rs) ----------

/** ask() context: account set, and the previous answer's person for "he"/"they". */
export interface AskScope {
  accountIds?: AccountId[] | null;
  /** Emails of the person the previous answer was about (AskAnswer.person.emails). */
  person?: string[] | null;
  /** Extra names for account groups; the backend adds profile names itself. */
  aliases?: { name: string; accountIds: AccountId[] }[];
  /** Show verification codes in answers; the app's ask command sets it. */
  revealCodes?: boolean;
}

export type AskIntent =
  | "lastContact"
  | "firstContact"
  | "relationship"
  | "latestItem"
  | "count"
  | "spend"
  | "whoAbout"
  | "topSenders"
  | "whoIs"
  | "waitingOn"
  | "oweReplies"
  | "when"
  | "flight"
  | "stay"
  | "package"
  | "orders"
  | "bills"
  | "booking"
  | "code"
  | "contactInfo"
  | "subscriptions"
  | "said"
  | "didReply"
  | "find"
  | "passage"
  | "unknown"
  | "query";

// ---------- questions as queries (mirror of penguin-core src/ask/query.rs) ----------

export type QuerySubject = "flights" | "stays" | "orders" | "parcels" | "bills" | "bookings" | "spending" | "messages";
export type QueryOp = "count" | "sum" | "average" | "max" | "min" | "list" | "first" | "last" | "next" | "exists";
export type QueryMeasure = "items" | "money" | "nights" | "trips";
export type QueryGroup = "month" | "year" | "merchant" | "place" | "person";
export type QueryDirection = "from" | "to";
export type QueryField = "date" | "confirmation" | "flightNumber" | "tracking" | "orderNumber" | "amount" | "address";
export type QueryTense = "any" | "past" | "future";
/** grammar = Penguin's grammar; model = Apple Intelligence on this Mac, checked; edited = the chips. */
export type QuerySource = "grammar" | "model" | "edited";

/** A question as a query. Timeframes stay as written; the date grammar reads them. */
export interface AskQuery {
  subject: QuerySubject;
  op: QueryOp;
  measure: QueryMeasure;
  groupBy: QueryGroup | null;
  timeframe: string | null;
  /** Two or more timeframes, or names, compared. */
  compare: string[];
  place: string | null;
  merchant: string | null;
  person: string | null;
  direction: QueryDirection | null;
  field: QueryField | null;
  tense: QueryTense;
}

/** How the answer read the question (the chips above it). */
export interface AskUnderstood {
  query: AskQuery;
  source: QuerySource;
  /** "flights · August 2026 · count". */
  summary: string;
  /** "Aug 1 – Aug 31, 2026". */
  rangeLabel: string | null;
  compareLabels: string[];
}

/** One row of a grouped count or sum, or one side of a comparison. */
export interface AskGroup {
  label: string;
  count: number;
  totals: AskTotal[];
  nights: number | null;
  cites: AskCite[];
  /** Local midnight (unix ms) when the group is a period. */
  start: number | null;
  /** The answer to "which … the most" (or least). */
  best: boolean;
}

export type AskResultKind = "count" | "sum" | "average" | "list" | "groups" | "compare" | "exists" | "item";

/** The answer's value, machine-readable (the headline says the same). */
export interface AskResult {
  kind: AskResultKind;
  count: number | null;
  totals: AskTotal[];
  value: number | null;
  yes: boolean | null;
  winner: string | null;
  /** "2026-10-02". */
  date: string | null;
  text: string | null;
}

// ---------- extracted facts (mirror of penguin-core src/structured/mod.rs) ----------

export interface Money {
  value: number;
  /** ISO 4217. */
  currency: string;
}

/** Where a fact came from: schema.org markup (JSON-LD or microdata) or text patterns. */
export type FactSource = "jsonLd" | "microdata" | "pattern";

/** Local wall times are "2026-10-02T19:05" or a date "2026-10-02". */
export interface FlightFact {
  kind: "flight";
  airline: string | null;
  airlineCode: string | null;
  flightNumber: string | null;
  confirmation: string | null;
  passenger: string | null;
  departAirport: string | null;
  departName: string | null;
  arriveAirport: string | null;
  arriveName: string | null;
  departTime: string | null;
  arriveTime: string | null;
  /** "confirmed" | "cancelled" | "changed" */
  status: string | null;
  total: Money | null;
}

export interface LodgingFact {
  kind: "lodging";
  name: string | null;
  address: string | null;
  phone: string | null;
  checkin: string | null;
  checkout: string | null;
  confirmation: string | null;
  guest: string | null;
  status: string | null;
  total: Money | null;
}

export interface OrderFact {
  kind: "order";
  merchant: string | null;
  orderNumber: string | null;
  total: Money | null;
  totalSource: string | null;
  items: string[];
  /** processing | shipped | delivered | cancelled | returned | refunded */
  status: string | null;
}

export interface ShipmentFact {
  kind: "shipment";
  carrier: string | null;
  trackingNumber: string | null;
  /** The check digit was verified (or it came from markup). */
  verified: boolean;
  trackingUrl: string | null;
  /** shipped | inTransit | outForDelivery | delivered | exception */
  status: string | null;
  expected: string | null;
  merchant: string | null;
  orderNumber: string | null;
  items: string[];
}

export interface BillFact {
  kind: "bill";
  biller: string | null;
  invoiceNumber: string | null;
  amountDue: Money | null;
  amountSource: string | null;
  dueDate: string | null;
  /** due | paid | overdue */
  status: string | null;
}

export interface ReservationFact {
  kind: "reservation";
  /** event | restaurant | rentalCar | train | bus */
  category: string;
  name: string | null;
  start: string | null;
  end: string | null;
  venue: string | null;
  address: string | null;
  confirmation: string | null;
  partySize: number | null;
  status: string | null;
  total: Money | null;
}

export interface ContactFact {
  kind: "contact";
  phones: string[];
  addresses: string[];
}

export type Extracted = FlightFact | LodgingFact | OrderFact | ShipmentFact | BillFact | ReservationFact | ContactFact;

/** A structured fact shown as a card, with the email it came from. */
export interface AskCard {
  fact: Extracted;
  cite: AskCite;
  from: Address;
  subject: string;
  date: number;
  source: FactSource;
  /** Other emails about the same booking, order or parcel, newest first. */
  related: AskCite[];
  /** "In 6 days", "Delivered", "Due in 3 days", "Cancelled". */
  status: string | null;
}

/** A sentence quoted from an email as the answer. */
export interface AskPassage {
  text: string;
  /** Ranges in `text` (JS string offsets) of the question's words. */
  marks: [number, number][];
  cite: AskCite;
  from: Address;
  subject: string;
  date: number;
  sent: boolean;
  score: number;
}

export interface AskTotal {
  value: number;
  currency: string;
  count: number;
}

/** The math behind a sum; the emails added are the answer's items. */
export interface AskSum {
  totals: AskTotal[];
  basis: string;
  /** Emails repeating an order already counted. */
  duplicates: number;
  /** Emails with no amount. */
  skipped: number;
  /** Invoices still due, left out. */
  unpaid: number;
}

/** "none" = nothing found; the headline says so. */
export type AskConfidence = "high" | "medium" | "low" | "none";

/** A message an answer relies on: open the thread at this message. */
export interface AskCite {
  accountId: AccountId;
  threadId: string;
  messageId: string;
}

export interface AskFact {
  label: string;
  value: string;
  /** Unix ms when the fact is a date. */
  date: number | null;
  cite: AskCite | null;
}

export interface AskItem extends AskCite {
  subject: string;
  from: Address;
  date: number;
  /** Plain text. */
  snippet: string;
  /** Why it's cited ("Handoff", "Waiting 5 days · to Priya", a quoted sentence). */
  note: string | null;
  /** Spend answers: the amount this email added, and the line it came from. */
  amount: { value: number; currency: string; source: string } | null;
  /** You sent it. */
  sent: boolean;
}

export interface AskTimeline {
  unit: "month" | "year";
  buckets: { start: number; label: string; fromThem: number; fromMe: number }[];
  markers: { date: number; label: string; cite: AskCite | null }[];
}

export interface AskPerson {
  label: string;
  name: string | null;
  emails: string[];
  domain: string | null;
  company: boolean;
}

/** A clickable question: follow-up or "did you mean". */
export interface AskSuggestion {
  label: string;
  question: string;
}

export interface AskAnswer {
  intent: AskIntent;
  question: string;
  headline: string;
  detail: string | null;
  facts: AskFact[];
  timeline: AskTimeline | null;
  items: AskItem[];
  person: AskPerson | null;
  candidates: AskSuggestion[];
  confidence: AskConfidence;
  /** "How I got this": resolution and every query run. */
  steps: string[];
  /** A Penguin search showing the underlying mail. */
  searchQuery: string | null;
  followups: AskSuggestion[];
  /** Flights, stays, orders, parcels, bills, bookings, contact details. */
  cards: AskCard[];
  /** Quoted sentences answering a topic question, best first. */
  passages: AskPassage[];
  sum: AskSum | null;
  /** What the answer couldn't see yet ("Still reading 1,204 emails …"). */
  coverage: string | null;
  /** How the question was read as a query, when it was (the chips). */
  understood: AskUnderstood | null;
  /** Grouped counts or sums, or the sides of a comparison. */
  groups: AskGroup[];
  result: AskResult | null;
  tookMs: number;
}

/** get_message_details result (mirror of src-tauri/src/message_details.rs). */
export interface MessageDetails {
  accountId: AccountId;
  messageId: string;
  threadId: string;
  subject: string;
  from: Address;
  replyTo: Address[];
  to: Address[];
  cc: Address[];
  bcc: Address[];
  /** Unix ms (Gmail internalDate). */
  date: number;
  /** The Date header as the sender wrote it (their timezone). */
  dateHeader: string | null;
  messageIdHeader: string | null;
  inReplyTo: string | null;
  references: string[];
  labelIds: string[];
  attachments: AttachmentMeta[];
  /** Gmail's size estimate; null when headers couldn't be fetched. */
  size: number | null;
  /** DMARC pass or aligned DKIM pass, per Gmail's own Authentication-Results. */
  senderAuthenticated: boolean;
  auth: {
    spf: string | null;
    /** SPF envelope domain (Gmail's "mailed-by"). */
    mailedBy: string | null;
    dkim: string | null;
    /** Domains whose DKIM signature passed (Gmail's "signed-by"). */
    signedBy: string[];
    dmarc: string | null;
  };
  /** TLS on the hop into Google; tls null = unknown. */
  transport: { tls: boolean | null; detail: string | null };
  /** List-Unsubscribe targets, https and mailto only. */
  unsubscribe: string[];
  /** False when the header fetch failed (offline): auth, transport, size unknown. */
  headersFetched: boolean;
  headersError: string | null;
}

/** preview_attachment result (mirror of src-tauri/src/attachments.rs). */
export type AttachmentPreviewKind = "image" | "pdf" | "text" | "unsupported";
export interface AttachmentPreview {
  kind: AttachmentPreviewKind;
  /** For images the sniffed type; otherwise the attachment's own. */
  mimeType: string;
  filename: string;
  size: number;
  /** image / pdf: base64 data: URL. */
  dataUrl: string | null;
  /** text: UTF-8, at most 1 MB. Render escaped, never as markup. */
  text: string | null;
  /** text: the file is longer than `text`. */
  truncated: boolean;
  /** unsupported: "type" (no previewer), "tooLarge" (> 20 MB), "unreadable" (bytes don't match the type). */
  reason: "type" | "tooLarge" | "unreadable" | null;
}

/** fetch_message_image (src-tauri/src/image_viewer.rs ImageBytes): a sniffed image. */
export interface ImageBytes {
  mimeType: string;
  size: number;
  /** `data:image/…;base64,…` of the sniffed type. */
  dataUrl: string;
}

/** save_message_images (src-tauri/src/image_viewer.rs SaveAllItem): one picture of the message. */
export type SaveAllItem = { kind: "attachment"; attachmentId: string } | { kind: "body"; src: string; name: string };

/** save_message_images (src-tauri/src/image_viewer.rs SavedImages): the folder made in Downloads and how many were written. */
export interface SavedImages {
  folder: string;
  saved: number;
  total: number;
}

/** prepare_image_drag / prepare_attachment_drag (src-tauri/src/file_export.rs DragFile): a file written for a native drag. */
export interface DragFile {
  /** Absolute path under `<cache>/drag-out/<session>/`; the only kind start_file_drag accepts. */
  path: string;
  /** Its file name (sanitized; a picture's extension follows its sniffed type). */
  name: string;
}

/** Every rejected command rejects with this shape. */
export type CommandErrorCode =
  | "needsReauth"
  | "notConfigured"
  | "network"
  | "notFound"
  | "invalidInput"
  /** The user cancelled (cancel_sign_in during a browser sign-in). */
  | "cancelled"
  | "other";
export interface CommandError {
  code: CommandErrorCode;
  message: string;
}

/** Fired when an optimistic modify_threads was rejected by Gmail and rolled back. */
export interface ActionFailedEvent {
  message: string;
}

/**
 * Fired when opening a thread started downloading its headers-only messages
 * (mail outside the sync window) and the download failed. Reopening retries.
 */
export interface BodyFetchFailedEvent {
  accountId: string;
  messageIds: string[];
  /** Why, worded for the user. */
  message: string;
}

// ---------------------------------------------------------------------------
// Settings + diagnostics (mirror of src-tauri/src/settings.rs and
// src-tauri/src/diagnostics.rs).
// ---------------------------------------------------------------------------

export type ThemeSetting = "system" | "dark" | "light";
export type Density = "compact" | "comfortable";
/** How the conversation list's rows look (Settings → General → List style). Unknown names load as "quiet". */
export type ListStyle = "quiet" | "classic" | "contrast" | "cards" | "mail";
/** ask: blocked until "Load images"; always: loaded on open; never: no button. */
export type RemoteImages = "ask" | "always" | "never";
/** What a trackpad swipe on a list row does. */
export type SwipeAction = "toggleRead" | "star" | "archive" | "trash" | "none";
/** Sidebar tint (styles/themes.css). Unknown names load as "graphite". */
/** Composer font; mirrors ComposeFont in src-tauri/src/settings.rs. */
export type ComposeFont = "inter" | "geist" | "ibmPlexSans" | "sourceSerif4" | "literata" | "iaWriterQuattro" | "atkinsonHyperlegible";
export type SidebarTheme =
  | "graphite" | "midnight" | "arctic" | "lavender" | "mint" | "peach" | "sky" | "rose" | "butter" | "sage"
  | "slate" | "mauve" | "sand" | "mocha" | "crimson" | "amber" | "olive" | "forest" | "teal" | "ocean" | "indigo" | "plum";
/** The dark theme's neutral ramp (styles/penguin.css 1c). Unknown names load as "black". */
export type DarkShade = "black" | "charcoal" | "dim" | "navy";
/** The caret, focus ring, unread dot and selection color (styles/themes.css). Unknown names load as "blue". */
export type AccentColor = "blue" | "purple" | "pink" | "red" | "orange" | "yellow" | "green" | "teal" | "graphite";
/** How round panels, buttons and rows are (styles/themes.css). Unknown names load as "rounded". */
export type CornerStyle = "rounded" | "subtle" | "square";

/** Persisted in <config dir>/settings.json. Every field has a default. */
export interface Settings {
  theme: ThemeSetting;
  density: Density;
  listStyle: ListStyle;
  /** Split Inbox: `inboxSplits` as tabs above the inbox, plus Other. (Named for the old fixed tabs.) */
  inboxTabs: boolean;
  /** The splits in tab order; a conversation goes to the first that matches. Max 12. Starts with Important, Calendar, News. */
  inboxSplits: InboxSplit[];
  /** Get to zero: "Archive older than…" for the inbox or a split (⌘K, the list header). */
  getToZero: boolean;
  /** The inbox-zero screen celebrates; off shows a plain "No mail". */
  zeroCelebration: boolean;
  remoteImages: RemoteImages;
  /** Lowercased sender addresses whose remote images always load (when remoteImages is "ask"). */
  trustedImageSenders: string[];
  /** Remove tracking pixels even when remote images load. Default true. */
  blockTrackingPixels: boolean;
  /** Remove per-person tracking parameters (mc_eid, fbclid, …) from links in received mail. Default false. */
  stripLinkTracking: boolean;
  /** Ask for a read receipt on mail you send (the recipient's app decides). Default false. */
  requestReadReceipts: boolean;
  /** Account profiles, in order (the first nine get ⌃1–⌃9). Max 20. */
  profiles: Profile[];
  /**
   * Account ids left out of "All accounts" (the unified inbox, its counts and
   * the other unified views). They stay signed in and syncing, and show their
   * mail when picked on their own or through a profile that lists them.
   * Default []. Ids of removed accounts are pruned from what the UI sees.
   */
  hiddenFromAll: string[];
  /**
   * Account ids in the order the user dragged them into (sidebar, Settings →
   * Accounts). Every list of accounts follows it; accounts not listed come
   * after the listed ones in their natural order, so a new account goes last.
   * Default []. Lowercased and deduped; ids of removed accounts are pruned
   * from what the UI sees (src/app/accountOrder.ts).
   */
  accountOrder: string[];
  /** Swipe a list row right (default "toggleRead"), left ("archive"), far left ("trash"). */
  swipeRight: SwipeAction;
  swipeLeft: SwipeAction;
  swipeLeftLong: SwipeAction;
  /** Default "graphite". */
  sidebarTheme: SidebarTheme;
  /** Accent (caret, focus, unread) follows the sidebar theme. Default false. */
  matchAccent: boolean;
  /** The dark theme's shade; light mode ignores it. Default "black". */
  darkShade: DarkShade;
  /** Accent color; `matchAccent` overrides it. Default "blue". */
  accentColor: AccentColor;
  /** Corner style. Default "rounded". */
  corners: CornerStyle;
  /** Experimental: in the dark theme, HTML mail is shown dark (features/message-body/darkBody.ts). Default false. */
  darkEmailBodies: boolean;
  /** Start in Floe mode (single-column layout). Default false. */
  floeMode: boolean;
  /** Composer font (lib/composeFonts.ts). Default "inter". Unknown names load as "inter". */
  composeFont: ComposeFont;
  /** Composer font size in px, 12–20. Default 15. */
  composeFontSize: number;
  /** Sidebar text size in steps from the default, −2…+2 (0 = default). */
  sidebarTextSize: number;
  /** Follow up lists sent mail with no reply after this many days, 1–14. Default 3. */
  followUpDays: number;
  /** Composer snippets (";trigger" or the ⌘; picker), max 200. Defaults to two starters. */
  snippets: Snippet[];
  /** Undo-send window; 0 = off. Default 10. */
  undoSendSeconds: UndoSendSeconds;
  /** Hour (5–11, local) of the "Tomorrow morning" / "Monday morning" send-later presets. Default 8. */
  sendLaterHour: number;
  /** Instant replies: one-liners in the reply composer (⌃1–⌃9) and the reply box, plus optional on-device suggestions. */
  instantReplies: InstantReplies;
  /** Write with AI in the composer (Apple's on-device model). Default true; runs only when asked. */
  writeWithAi: boolean;
  /** Composer signatures (rich text), max 50. `html` is sanitized on write (sanitize_compose_html). */
  signatures: Signature[];
  /** Account id → the signature id it uses by default. Unknown signature ids are dropped on write. */
  signatureDefaults: Record<string, string>;
  /** When the account's default signature goes in by itself. Default all true. */
  signatureInsert: SignatureInsert;
  /** Put the "-- " separator line above the signature. Default false. */
  signatureSeparator: boolean;
  /** Replies and forwards send only from the account the mail came to. Default true. */
  lockReplyAccount: boolean;
  /**
   * Gmail API quota sync paces against, per account (units/min, 600–1,000,000).
   * Default 6,000; applied live. PENGUIN_GMAIL_UNITS_PER_MIN overrides it.
   */
  gmailUnitsPerMin: number;
  /** Months of mail downloaded in full: 1 | 3 | 6 (default) | 12 | 24 | 0 = everything. */
  syncWindowMonths: SyncWindowMonths;
  /** Mail older than the window: "headers" (default), "none" or "full". */
  olderMail: OlderMail;
  /** Local MCP server for AI tools (`penguin-cli mcp`). Default off. */
  mcp: McpSettings;
  /** Key hints ("E", "⌘K" caps, tooltip keys) across the UI. Default on; the "?" sheet always shows keys. */
  showShortcutHints: boolean;
  /** Shortcut coach: after a mouse action that has a key, show the key for a moment (app/ShortcutCoach.tsx). Default on. */
  shortcutCoach: boolean;
  /** The Unsubscribe button in the thread view (plus its menu item and ⌘U). Default on. */
  unsubscribeButton: boolean;
  /** Search by meaning (Settings → Search): the on-device embedding model and background indexing. Default on. */
  semanticSearch: boolean;
  /** Thread summaries with Apple's on-device model (Summarize, cached summaries). Default on. */
  summaries: boolean;
  /** Ask reads questions its grammar can't with Apple's on-device model (the answer is still computed from local mail). Default on. */
  askWithAi: boolean;
  /** Sender photos and logos; mirrors SenderPhotos in src-tauri/src/settings.rs. The patch replaces the whole object. */
  senderPhotos: SenderPhotos;
  /** Where sender photos appear: message list, message headers, both (default) or off (no lookups at all). Unknown values load as "both". */
  avatarPlacement: AvatarPlacement;
  /** "You" (Settings → You); mirrors Me in src-tauri/src/settings.rs. Only `photo` is used now, set by its own commands. */
  me: Me;
  /** Google Calendar; mirrors CalendarSettings in src-tauri/src/settings.rs. The patch replaces the whole object. */
  calendar: CalendarSettings;
  /** New-mail notifications; mirrors NotificationSettings in src-tauri/src/settings.rs. The patch replaces the whole object. */
  notifications: NotificationSettings;
  /**
   * Add account setups started but not finished, newest first (≤20, one per address). Shown in Settings → Accounts and on
   * the Add account start screen. Entries for accounts that exist are left out. The patch replaces the whole list.
   */
  pendingSetups: PendingSetup[];
  /** Smart views in the sidebar (Settings → Views). All off by default. */
  smartViews: SmartViewSettings;
  /**
   * The Welcome setup (features/welcome) was finished or skipped. False on a new install: it shows once after the first
   * account is added, and the search model doesn't download before it. Existing installs with accounts count as done.
   */
  welcomeCompleted: boolean;
}

/** Settings → General → Notifications (src-tauri/src/notify.rs). */
export interface NotificationSettings {
  /** "Notify me about new mail". Default false. */
  enabled: boolean;
  /**
   * Per-account switches (account id → on). An account not listed follows the
   * default: on, except accounts hidden from All Inboxes. Ids are lowercased,
   * ≤100, and pruned to signed-in accounts.
   */
  accounts: Record<string, boolean>;
  /** Only mail from people I've emailed before (from any account). Default false. */
  knownSendersOnly: boolean;
}

/** set_notify_context: what's on screen, so new mail already visible doesn't notify. */
export interface NotifyContext {
  /** Accounts whose inbox list is showing (empty when no inbox is). */
  inboxAccounts: string[];
  /** The open or previewed thread. */
  thread: ThreadRef | null;
}

/** penguin://notification-open: a new-mail notification was clicked. No threadId = that account's inbox (a grouped one). */
export interface NotificationOpen {
  accountId: string;
  threadId: string | null;
}

/** notification_permission / request_notification_permission. macOS builds always answer "granted". */
export type NotificationPermission = "granted" | "denied" | "prompt";

/** One unfinished Add account setup; mirrors PendingSetup in src-tauri/src/settings.rs. No secrets. */
/** Which smart views the sidebar shows, in order, and which show a count (settings.rs SmartViewSettings). */
export interface SmartViewSettings {
  /** Built-in ids (SMART_VIEWS) and pinned searches as "custom:<id>", in sidebar order. */
  shown: string[];
  /** Views whose sidebar row shows a count. */
  counts: string[];
  /** Saved searches pinned to the sidebar (always shown; removing unpins). */
  custom: CustomView[];
}

/** A saved search pinned as a sidebar view. */
export interface CustomView {
  /** 1–64 of [A-Za-z0-9_-]. */
  id: string;
  /** ≤40 chars; blank = the query. */
  name: string;
  /** The search, in the search language (≤500 chars). */
  query: string;
}

export interface PendingSetup {
  /** Lowercased. */
  email: string;
  kind: SetupKind;
  /** The flow step reached (a step id in features/onboarding/flow.tsx). */
  step: string;
  /** Unix ms. */
  startedAt: number;
  /** Why the last sign-in or connect attempt failed (≤300 chars). */
  lastError: string | null;
}

export interface CalendarSettings {
  /** Months of past events kept, 1–60. Default 24. */
  pastMonths: number;
  /** Months of upcoming events, 1–36. Default 12. */
  futureMonths: number;
  /** "Next up" chip in the status bar. Default true. */
  nextUp: boolean;
  /**
   * Adding or reconnecting an account also asks for read-only calendar
   * access in the same Google consent. Default true. Off for a Workspace
   * that allows Gmail but blocks Calendar (Google then refuses the sign-in).
   */
  connectOnSignIn: boolean;
}

/**
 * Your own identity across every account. Only `photo` is used: your name is
 * each account's Google profile name (lib/me.ts meName). The text fields are
 * legacy (Settings → You no longer edits them), kept so settings.json and the
 * backend stay in lockstep; nothing reads them.
 */
export interface Me {
  name: string;
  title: string;
  company: string;
  signOff: string;
  /** Id (sha256) of the stored photo; set only by setMePhoto / setMePhotoFromGoogle / clearMePhoto. */
  photo: string | null;
}

/** What the UI tells the native menu bar (set_menu_context) so items enable with context. */
export interface MenuContext {
  mail: boolean;
  blocked: boolean;
  selection: boolean;
  selectionUnread: boolean;
  selectionStarred: boolean;
  sidebarVisible: boolean;
  floe: boolean;
  /** The list's Unread filter is on (View → Show Only Unread). */
  unreadOnly: boolean;
  /** This window shows one conversation or one composer (open_window), not the mail shell. */
  detached: boolean;
}

/**
 * open_window (src-tauri/src/windows.rs WindowRequest): a conversation or a
 * composer in a window of its own. Ids ≤512 chars, no control characters.
 */
export type WindowRequest =
  | {
      kind: "thread";
      accountId: string;
      threadId: string;
      /** The subject, the window's title until the thread has loaded. */
      title?: string | null;
    }
  | {
      kind: "compose";
      /** A saved draft to open (both; a draft needs its account), or with no draft the account a new message is from. */
      accountId?: string | null;
      draftId?: string | null;
      /** The editor state handed over by another window (features/compose/handoff.ts ComposeSeed), taken once by the new window. ≤64 MB of JSON. */
      seed?: unknown;
      title?: string | null;
    };

/** open_window's answer. */
export interface OpenedWindow {
  label: string;
  /** The conversation already had a window; it was brought to the front. */
  existing: boolean;
}

/** Payload of `penguin://menu`: a menu bar item to run (a shortcut-registry id or a menu-only id). */
export interface MenuEvent {
  id: string;
}

export type AvatarPlacement = "list" | "message" | "both" | "off";

/** Which avatar sources may be used. Honored by the backend (src-tauri/src/avatars/). */
export interface SenderPhotos {
  /** Google contact photos; needs "Connect contact photos" per account. Default false. */
  contacts: boolean;
  /** BIMI brand logos for authenticated senders. Default true. */
  bimi: boolean;
  /** The sender domain's own icon. Default true. */
  favicons: boolean;
  /** Gravatar (tells Gravatar who emails you). Default false. */
  gravatar: boolean;
}

/** avatar_lookup item. `authenticated`: the message's senderAuthenticated when known (thread view), else null. */
export interface AvatarRequest {
  email: string;
  authenticated: boolean | null;
}

/** "photo" = a person (round); "logo" = a brand (white rounded tile). */
export type AvatarKind = "photo" | "logo";

export interface AvatarInfo {
  email: string;
  kind: AvatarKind | null;
  /** avatar://localhost/<sha256>.png (immutable); null = keep the monogram. */
  url: string | null;
}

/** penguin://avatars-changed: re-look-up these addresses (or everything with `all`). */
export interface AvatarsChangedEvent {
  emails: string[];
  all: boolean;
}

export interface AccountPhotos {
  accountId: AccountId;
  /** Both contacts scopes are granted for this account. */
  contactsGranted: boolean;
  /** Unix ms of the last contacts sync. */
  syncedAt: number | null;
  photos: number;
  error: string | null;
}

export interface AvatarStatus {
  accounts: AccountPhotos[];
  images: number;
  bytes: number;
}

export type SyncWindowMonths = 1 | 3 | 6 | 12 | 24 | 0;
export const SYNC_WINDOW_CHOICES: SyncWindowMonths[] = [1, 3, 6, 12, 24, 0];
export type OlderMail = "headers" | "none" | "full";

/** sync_window_estimate: one account's numbers for a window. */
export interface WindowEstimate {
  accountId: AccountId;
  months: SyncWindowMonths;
  /** Gmail's estimate of messages in the window (0 months = the mailbox). */
  inWindow: number;
  /** Of those, already stored with a full body. */
  haveFull: number;
  /** Time to download the rest in full at this account's quota. */
  etaSecs: number;
  /** Messages older than the window. */
  older: number;
  /** Time to store the older mail headers-only once the window is done. */
  olderHeadersEtaSecs: number;
  /** Time to download the older mail in full (olderMail "full"). */
  olderFullEtaSecs: number;
  /** Average bytes a message takes in the local database, for "uses about …" estimates. */
  bytesPerMessage: number;
  /** Set when Gmail couldn't be asked (e.g. the account needs sign-in). */
  error: string | null;
}

export interface AccountCoverage {
  accountId: AccountId;
  /** Every message since this (unix ms) has its full body; 0 = all mail; null = first download running. */
  fullSinceMs: number | null;
  windowComplete: boolean;
  /** Older mail is done for the current olderMail (true when "none"). */
  olderComplete: boolean;
  full: number;
  headersOnly: number;
}

/** sync_coverage: what's downloaded where (local reads only). */
export interface SyncCoverage {
  windowMonths: SyncWindowMonths;
  olderMail: OlderMail;
  /** Start of the current window (unix ms; 0 = everything). */
  windowStartMs: number;
  accounts: AccountCoverage[];
}

/** free_up_space result. */
export interface FreeUpSpace {
  /** Messages whose bodies were (or, for a dry run, would be) dropped. */
  messages: number;
  /** Stored (compressed) size of those bodies. */
  bodyBytes: number;
  bytesBefore: number;
  bytesAfter: number;
}

export interface ServerSearchAccount {
  accountId: AccountId;
  /** Gmail's estimate of all matches. */
  estimate: number;
  /** Messages newly stored (headers-only) by this search. */
  fetched: number;
  error: string | null;
}

/** search_server: "Also search Gmail". Hits are local now (headers-only if they were unknown). */
export interface ServerSearchResponse {
  gmailQuery: string;
  /** Parts of the query Gmail can't express were dropped. */
  approximate: boolean;
  /** Newest first across accounts. */
  hits: SearchHit[];
  accounts: ServerSearchAccount[];
}

/** `penguin-cli mcp`: read-only access to the local index for AI tools. */
export interface McpSettings {
  enabled: boolean;
}

/** Settings → Developer → "Install command-line tool" (cli_install_status / install_cli). */
export interface CliLinkStatus {
  /** ~/.local/bin/penguin */
  linkPath: string;
  /** The CLI inside this app; null when this build has none. */
  target: string | null;
  /** linkPath exists and points at target. */
  installed: boolean;
  /** Something else is at linkPath (another install, a stale link, a real file). */
  conflict: string | null;
  /** Whether ~/.local/bin is on the login shell's PATH; null if it couldn't be checked. */
  onPath: boolean | null;
  /** The exact line to run to add ~/.local/bin to PATH for the user's shell. */
  pathLine: string;
}

/** Settings → Developer → MCP (mcp_info). */
/** semantic_status: the search-by-meaning model download and indexing progress (Settings, debug info). Mirrors SemanticIndexStatus in src-tauri/src/semantic/mod.rs. Search results carry their own `semantic` / `semanticProgress`. */
export type SemanticIndexState = "off" | "downloading" | "loading" | "indexing" | "paused" | "ready" | "error";
export interface SemanticIndexStatus {
  state: SemanticIndexState;
  enabled: boolean;
  /** Messages with vectors. */
  indexed: number;
  /** Messages that should have them (everything but spam and drafts). */
  total: number;
  /** Passages (vectors) stored. */
  chunks: number;
  model: string;
  modelId: string;
  modelLicense: string;
  /** First-run model download, bytes. */
  downloadDone: number;
  downloadTotal: number;
  /** Why indexing waits ("Low Power Mode is on", "the Mac is running hot"). */
  pausedReason: string | null;
  error: string | null;
  /** RAM held by the vector scan structure (not the model). */
  indexRamBytes: number;
  /** Messages embedded per second, recent average; 0 when idle. */
  rate: number;
}

export interface McpInfo {
  enabled: boolean;
  /** penguin-cli next to the app binary; null when not built/bundled. */
  cliPath: string | null;
  /** `claude mcp add penguin -- <cli> mcp` */
  claudeCodeCommand: string;
  /** Pretty JSON to merge into claude_desktop_config.json. */
  claudeDesktopConfig: string;
  /** Where tool calls are logged (names, arguments, counts; never content). */
  auditLogPath: string | null;
}

export type UndoSendSeconds = 0 | 5 | 10 | 20 | 30;

/** Settings.instantReplies; mirrors InstantReplies in src-tauri/src/settings.rs. */
export interface InstantReplies {
  /** Offer the one-liners. Default true. */
  enabled: boolean;
  /** In order (⌃1…⌃9); ≤ 9, each ≤ 200 chars, trimmed and deduped on write. */
  replies: string[];
  /** Also suggest up to three replies with Apple's on-device model when a reply opens. Default false. */
  aiSuggestions: boolean;
}

/** A composer signature; mirrors Signature in src-tauri/src/settings.rs. */
export interface Signature {
  /** 1–64 of [A-Za-z0-9_-]. */
  id: string;
  /** ≤ 60 chars. */
  name: string;
  /** Rich text, ≤ 20,000 chars, through the composer allowlist. */
  html: string;
}

export interface SignatureInsert {
  newMessages: boolean;
  replies: boolean;
  forwards: boolean;
}

/**
 * A composer snippet. The body may use {first_name}, {last_name},
 * {full_name}, {company}, {sender_name}, {my_name}, {my_first_name}, {date}
 * and {cursor} (features/compose/snippets.ts). Invalid ids/triggers and
 * duplicate triggers are dropped by update_settings.
 */
export interface Snippet {
  /** 1–64 of [A-Za-z0-9_-]. */
  id: string;
  /** Typed after ";": 1–32 of [a-z0-9_-], unique. */
  trigger: string;
  /** ≤ 80 chars. */
  title: string;
  /** ≤ 10,000 chars. */
  body: string;
  uses: number;
  /** Subject it fills in when the message has none yet; "" = none. ≤ 200 chars. */
  subject: string;
  /** Addresses ("Name <a@b.example>" or bare) it adds to Cc / Bcc; ≤ 20 each. */
  cc: string[];
  bcc: string[];
  /** Files it attaches (save_snippet_file); ≤ 10. */
  attachments: SnippetFile[];
}

/**
 * A named group of accounts (a company, a role). Account ids that aren't
 * signed in are pruned by get_settings. An account may be in several profiles.
 */
export interface Profile {
  /** 1–64 of [A-Za-z0-9_-]. */
  id: string;
  name: string;
  /** #rrggbb from the account palette. */
  color: string;
  accountIds: AccountId[];
  emoji: string | null;
}

/** A partial update for update_settings; omitted fields are unchanged. */
export type SettingsPatch = Partial<Omit<Settings, "me">> & { me?: Partial<Omit<Me, "photo">> };

/** Payload of `penguin://settings-changed`: the full settings after a change. */
export type SettingsChangedEvent = Settings;

export interface AccountDiagnostics {
  accountId: AccountId;
  email: string;
  phase: SyncPhase;
  /** Messages stored locally (message_counts). */
  messagesStored: number;
  /** Of messagesStored, those with headers only (older than the sync window). */
  headersOnlyMessages: number;
  /** Gmail's messagesTotal from the profile, if the engine has reported it. */
  gmailTotal: number | null;
  backfillDone: boolean;
  historyIdPresent: boolean;
  failedMessageIds: number;
  lastSyncedAt: number | null;
  /** Messages stored per minute, measured over roughly the last minute. */
  msgsPerMinute: number | null;
  /** "present" | "missing" | "unknown" — from the in-memory cache only; never reads the Keychain. */
  keychain: "present" | "missing" | "unknown";
  /** Learned Gmail quota pacing (penguin_gmail::api::QuotaStats); null until the account has made a call. */
  quota: QuotaStats | null;
  inlineCacheBytes: number;
  error: string | null;
}

export interface QuotaStats {
  /** "perAccount": one budget per mailbox (default); "shared": one pooled budget. */
  scope: "shared" | "perAccount";
  /** Budget the limiter paces against (units/min). */
  unitsPerMin: number;
  /** Learned cost of one messages.get (units). */
  getCost: number;
  /** Sustainable messages.get rate for the whole budget. */
  getsPerSec: number;
  /** messages.get calls against the whole budget in the last minute. */
  getsLastMin: number;
  unitsLastMin: number;
  /** Quota throttle episodes since launch (whole budget). */
  throttleEpisodes: number;
  /** This account's messages.get calls in the last minute. */
  accountGetsLastMin: number;
  /** Accounts that used the budget in the last minute. */
  activeAccounts: number;
  /** This account's fair share of getsPerSec. */
  accountGetsPerSec: number;
}

export interface Diagnostics {
  appVersion: string;
  dataDir: string;
  configDir: string;
  cacheDir: string;
  logDir: string | null;
  dbPath: string;
  dbBytes: number;
  walBytes: number;
  pageSize: number;
  pageCount: number;
  freelistCount: number;
  totalMessages: number;
  totalThreads: number;
  inlineCacheBytes: number;
  logBytes: number;
  /** Last 12 characters of the OAuth client id; never the secret. */
  oauthClientIdTail: string | null;
  accounts: AccountDiagnostics[];
  trackersRemovedSession: number;
  /** PENGUIN_GMAIL_UNITS_PER_MIN when set; it overrides settings.gmailUnitsPerMin. */
  gmailUnitsEnvOverride: number | null;
  tookMs: number;
}

export interface TableSize {
  name: string;
  /** "table" | "index" | "fts" (FTS shadow tables are grouped under their virtual table). */
  kind: string;
  bytes: number;
}

export interface TableSizes {
  /** "dbstat" when measured page by page; "estimate" otherwise. */
  method: string;
  tables: TableSize[];
  tookMs: number;
}

/** Directories reveal_path may open in Finder. */
export type RevealTarget = "data" | "config" | "cache" | "log";

// ---------- outbox (send later, remind if no reply) ----------

/** A saved draft queued to send at `sendAt` (penguin_core::ScheduledSend). The content is whatever the draft holds when it fires. */
export interface ScheduledSend {
  id: string;
  accountId: string;
  draftId: string;
  /** Unix ms. Moves forward when a failed attempt will be retried. */
  sendAt: number;
  createdAt: number;
  /** After it's sent, re-surface the thread if nobody replies within this many ms. */
  remindAfterMs: number | null;
  /** Failed attempts so far. */
  attempts: number;
  lastError: string | null;
}

/** "Remind me if no reply" (penguin_core::Reminder). */
export interface Reminder {
  id: string;
  accountId: string;
  threadId: string;
  /** The sent message being waited on, when known. */
  sentMessageId: string | null;
  /** Unix ms; only replies after this count. */
  sentAt: number;
  /** Unix ms. */
  remindAt: number;
  createdAt: number;
}

/** Ids of a message just sent (send_message returns it). */
export interface SentRef {
  messageId: string;
  threadId: string;
}

/** Payload of `penguin://scheduled-sent`: one per scheduler pass that sent or gave up on something. */
export interface ScheduledSentBatch {
  sent: { id: string; accountId: string; draftId: string; messageId: string; threadId: string }[];
  /** Gave up: the draft is gone, or Gmail rejected it. Transient failures retry silently (see ScheduledSend.lastError). */
  failed: { id: string; accountId: string; draftId: string; message: string }[];
  /** How many of `sent` were already due when the app started. */
  missed: number;
}

/** Payload of `penguin://reminder-due`; the thread is back in the inbox, unread. */
export interface ReminderDue {
  accountId: string;
  threadId: string;
  subject: string;
}

/** A snoozed conversation (penguin_core::Snooze). */
export interface Snooze {
  accountId: string;
  threadId: string;
  /** Unix ms it wakes. */
  wakeAt: number;
  /** Unix ms it was snoozed. */
  snoozedAt: number;
}

/** penguin://snooze-woke: snoozed conversations back in the inbox (unread, at the top). */
export interface SnoozeWokeBatch {
  items: { accountId: string; threadId: string; subject: string }[];
  /** How many were due while Penguin was closed. */
  missed: number;
}

// ---------- calendar ----------
// Mirrors the calendar section of penguin-core types.rs and
// src-tauri/src/calendar/mod.rs.

export type EventResponse = "accepted" | "tentative" | "declined" | "needsAction";

export interface CalendarInfo {
  accountId: AccountId;
  /** Google calendar id (the primary calendar's is the account address). */
  id: string;
  summary: string;
  /** #rrggbb from Google, when set. */
  color: string | null;
  /** Synced and shown (local choice; starts as Google's own). */
  selected: boolean;
  primary: boolean;
  /** owner | writer | reader | freeBusyReader */
  accessRole: string;
}

export interface EventAttendee {
  /** Lowercased. */
  email: string;
  name: string | null;
  response: EventResponse;
  organizer: boolean;
  /** This attendee is the account itself. */
  self: boolean;
  optional: boolean;
  /** A room or other resource, not a person. */
  resource: boolean;
}

export interface CalendarEvent {
  accountId: AccountId;
  /** "" for an invite that isn't on any of your calendars. */
  calendarId: string;
  /** "" for an invite that isn't on any of your calendars. */
  id: string;
  icalUid: string | null;
  /** confirmed | tentative */
  status: string;
  summary: string;
  /** Plain text: render as text, linkify only. */
  description: string;
  location: string;
  /** Unix ms. All-day events start at local midnight of startDate. */
  start: number;
  /** Unix ms, exclusive. */
  end: number;
  allDay: boolean;
  /** YYYY-MM-DD for all-day events (end exclusive). */
  startDate: string | null;
  endDate: string | null;
  organizer: Address | null;
  attendees: EventAttendee[];
  /** Your response when you're a guest; null for your own events. */
  myResponse: EventResponse | null;
  /** Google Calendar web link (https only). */
  htmlLink: string | null;
  /** Meet / Zoom / Teams / Webex join link (https only). */
  conferenceUrl: string | null;
  conferenceKind: "meet" | "zoom" | "teams" | "webex" | "other" | null;
  recurringEventId: string | null;
  /** "Show as available": never a conflict. */
  free: boolean;
  updated: number;
}

/** "When did I last meet X": local only. */
export interface PersonMeetings {
  email: string;
  last: CalendarEvent | null;
  next: CalendarEvent | null;
  /** Events with them in the synced window. */
  count: number;
}

export interface CalendarAccountStatus {
  accountId: AccountId;
  /** calendar.readonly granted: events sync. */
  granted: boolean;
  /** calendar.events granted too: RSVP from invite cards. */
  rsvpGranted: boolean;
  syncing: boolean;
  /** Unix ms of the oldest successful sync among selected calendars. */
  syncedAt: number | null;
  events: number;
  error: string | null;
  calendars: CalendarInfo[];
}

export interface CalendarStatus {
  accounts: CalendarAccountStatus[];
}

export interface EventThread {
  accountId: AccountId;
  threadId: string;
  subject: string;
}

/** The event popover: the event, its calendar and its invitation thread. */
export interface EventDetail {
  event: CalendarEvent;
  calendar: CalendarInfo | null;
  thread: EventThread | null;
}

export type InviteMethod = "request" | "cancel" | "reply" | "publish" | "counter" | "other";
export type InviteAnswer = "accepted" | "tentative" | "declined";

/** One parsed calendar part (penguin-core InviteSnapshot). */
export interface InviteSnapshot {
  method: InviteMethod;
  uid: string | null;
  sequence: number;
  recurrenceId: number | null;
  recurring: boolean;
  summary: string;
  location: string;
  start: number;
  end: number;
  allDay: boolean;
  startDate: string | null;
  endDate: string | null;
  /** The organizer's time zone (DTSTART's TZID) when not UTC. */
  timeZone: string | null;
  organizer: Address | null;
  attendees: EventAttendee[];
}

/** An answer Penguin sent (penguin-core InviteResponse). */
export interface InviteResponse {
  response: InviteAnswer;
  /** calendar (Google Calendar API) | graph (Microsoft Graph) | email (iMIP to the organizer) */
  via: "calendar" | "graph" | "email";
  sequence: number;
  comment: string | null;
  proposedStart: number | null;
  proposedEnd: number | null;
  at: number;
}

/** The event chip in a list row (penguin-core InviteChip). */
export interface InviteChip {
  messageId: string;
  uid: string | null;
  method: InviteMethod;
  /** A request that changes an earlier version. */
  updated: boolean;
  summary: string;
  start: number;
  end: number;
  allDay: boolean;
  startDate: string | null;
  endDate: string | null;
  recurring: boolean;
  /** Your answer; null when you aren't a guest. */
  response: EventResponse | null;
  /** Reply/counter mail: the guest who answered. */
  replier: EventAttendee | null;
  /** Yes / Maybe / No make sense here. */
  canRespond: boolean;
  /** First busy event it overlaps, and how many. */
  conflict: string | null;
  conflicts: number;
}

/** The card at the top of a thread that carries an invitation. */
export interface InviteCard {
  accountId: AccountId;
  threadId: string;
  messageId: string;
  uid: string | null;
  method: InviteMethod;
  /** The synced copy when inCalendar, else the invite's own details. */
  event: CalendarEvent;
  inCalendar: boolean;
  /** The invite is for a whole recurring series (event = the next instance). */
  recurring: boolean;
  /** Busy events overlapping it on any of your calendars. */
  conflicts: CalendarEvent[];
  /** Yes / Maybe / No work (a request you're invited to, not over, with a route). */
  canRespond: boolean;
  /** How an answer goes out. */
  route: "calendar" | "graph" | "email" | "none";
  /** Email only because RSVP is off for this Google account (Settings → Calendar). */
  rsvpAvailable: boolean;
  /** "Propose new time" works (not for a whole series). */
  canPropose: boolean;
  /** Your answer (newest of sent / synced / the invite's own). */
  response: EventResponse | null;
  /** What Penguin last sent for it. */
  sent: InviteResponse | null;
  timeZone: string | null;
  updated: boolean;
  /** Fields that changed since `previous`: time | location | title | guests. */
  changes: ("time" | "location" | "title" | "guests")[];
  previous: InviteSnapshot | null;
  /** This account has calendar access (else offer "Connect calendar"). */
  calendarConnected: boolean;
}

/** Payload of `penguin://calendar-changed`. */
export interface CalendarChangedEvent {
  accountIds: string[];
}

// ---------- rules & automations (src-tauri/src/rules/; OWNER: rules agent) ----------

/** One message matching a rule condition (penguin-core QueryMatch). */
export interface QueryMatch {
  accountId: AccountId;
  messageId: string;
  threadId: string;
  date: number;
  from: Address;
  subject: string;
}

export type RuleTrigger =
  /** Mail arriving through incremental sync (never backfill). */
  | { kind: "newMessage" }
  /** A message gains `label` (a label name, or a system id like STARRED). */
  | { kind: "labelAdded"; label: string }
  /** Local time "HH:MM"; weekly needs `weekday` (0 = Sunday). */
  | { kind: "schedule"; every: "daily" | "weekly"; at: string; weekday?: number | null }
  /** Only "Run now". */
  | { kind: "manual" };

export type RuleAction =
  | { kind: "addLabel"; label: string }
  | { kind: "removeLabel"; label: string }
  | { kind: "archive" }
  | { kind: "markRead" }
  | { kind: "star" }
  /** `confirmed` must be true (the editor asks when it's added). */
  | { kind: "trash"; confirmed: boolean }
  /** Plain-text copy + attachments from the matching account; confirmed; rate-limited (20/h, 100/day). */
  | { kind: "forward"; to: string; confirmed: boolean }
  | { kind: "notify" }
  /** argv only (no shell); the match as ruleMatch v1 JSON on stdin. Needs "Allow hooks". */
  | { kind: "hook"; program: string; args: string[]; timeoutSecs: number }
  /** POST ruleMatch v1 JSON; `X-Penguin-Signature: t=<secs>,v1=<hex HMAC-SHA256 of "t.body">` when secret is set. */
  | { kind: "webhook"; url: string; secret: string | null };

export interface Rule {
  id: string;
  name: string;
  enabled: boolean;
  /** Match and log only. New rules start here. */
  dryRun: boolean;
  /** null = every account. */
  accountIds: string[] | null;
  /** Or a profile's accounts, resolved when the rule runs. */
  profileId: string | null;
  trigger: RuleTrigger;
  /** A Penguin search query, e.g. `from:@uber.com has:pdf`. */
  condition: string;
  actions: RuleAction[];
  /** Later rules skip messages this one matched. */
  stopProcessing: boolean;
  /** Hooks/webhooks get the message text (else headers only). */
  includeBody: boolean;
  createdAt: number;
  updatedAt: number;
  /** Set when the engine switched it off (fired too often); cleared on save / re-enable. */
  pausedReason: string | null;
}

/** save_rule input: `id` null creates. */
export type RuleInput = Omit<Rule, "id" | "createdAt" | "updatedAt" | "pausedReason"> & { id: string | null };

export interface RuleStats {
  ruleId: string;
  /** Messages acted on by live runs. */
  applied: number;
  dryRunMatches: number;
  lastRunAt: number | null;
  lastErrorAt: number | null;
  lastError: string | null;
}

export interface RulesOverview {
  allowHooks: boolean;
  /** In evaluation order. */
  rules: Rule[];
  stats: RuleStats[];
  auditLogPath: string | null;
}

export interface RuleOutcome {
  action: RuleAction["kind"] | "pause";
  ok: boolean;
  detail: string;
  /** Present when Undo can reverse it. */
  undo?: ThreadAction;
}

export interface RuleLogEntry {
  id: number;
  ts: number;
  ruleId: string;
  trigger: "newMessage" | "labelAdded" | "schedule" | "manual";
  dryRun: boolean;
  ok: boolean;
  accountId: string | null;
  threadId: string | null;
  messageId: string | null;
  subject: string | null;
  fromEmail: string | null;
  /** 1 per message row; 0 for a run's aggregate row (digest notification, batch hook). */
  matched: number;
  outcomes: RuleOutcome[];
  undone: boolean;
}

export interface RulePreviewItem {
  accountId: AccountId;
  threadId: string;
  messageId: string;
  subject: string;
  from: Address;
  date: number;
  /** The saved rule already acted on it (a run skips it). */
  alreadyApplied: boolean;
}

export interface RulePreview {
  /** Matching messages in scope (counted up to 100,000). */
  count: number;
  capped: boolean;
  /** Newest matches. */
  items: RulePreviewItem[];
}

export interface RuleRunReport {
  ruleId: string;
  dryRun: boolean;
  /** Matching messages, up to 1,000 per run. */
  matched: number;
  alreadyApplied: number;
  acted: number;
  failed: number;
  capped: boolean;
  logIds: number[];
}

/** Payload of `penguin://rule-fired`. */
export interface RuleFiredEvent {
  ruleId: string;
  ruleName: string;
  trigger: RuleLogEntry["trigger"];
  dryRun: boolean;
  count: number;
  failed: number;
  logIds: number[];
}

/** penguin://update; mirrors UpdateEvent in src-tauri/src/updater.rs. */
export interface UpdateEvent {
  /** "unconfigured": this build has no update channel (a source build). */
  state: "checking" | "downloading" | "ready" | "upToDate" | "error" | "unconfigured";
  /** From Penguin → Check for Updates… (automatic checks only send "ready"). */
  manual: boolean;
  current: string;
  version: string | null;
  notes: string | null;
  message: string | null;
}

/** read_log; mirrors LogTail in src-tauri/src/applog.rs. */
export interface LogTail {
  path: string | null;
  /** Oldest first. */
  lines: string[];
  /** Older lines exist beyond what was read. */
  truncated: boolean;
}

// ---------------------------------------------------------------------------
// Thread summaries (on-device; docs/SUMMARIES.md). Mirrors the AiSummary
// types at the end of penguin-core/src/types.rs.
// ---------------------------------------------------------------------------

/** A summary of one version of a thread; items link to their source message. */
export interface AiSummary {
  accountId: AccountId;
  threadId: string;
  /** Which version of the thread it read (penguin-core summary::version_key). */
  version: string;
  /** One or two sentences. */
  gist: string;
  points: AiSummaryPoint[];
  /** Things someone asked of you, with any deadline. */
  asks: AiSummaryAsk[];
  /** Messages read. */
  messageCount: number;
  /** Earlier messages left out of a very long thread. */
  omitted: number;
  /** Unix ms. */
  createdAt: number;
  /** The thread changed since (a cached summary of an older version). */
  stale: boolean;
}

export interface AiSummaryPoint {
  text: string;
  /** null: the model cited a message that wasn't in its input; links nowhere. */
  messageId: string | null;
}

export interface AiSummaryAsk {
  text: string;
  messageId: string | null;
  /** The deadline as the mail wrote it ("Friday", "by Oct 3"). */
  due: string | null;
}

/** Why summaries can't run here; the first three are Apple's UnavailableReason. */
export type AiUnavailableReason =
  | "deviceNotEligible"
  | "appleIntelligenceNotEnabled"
  | "modelNotReady"
  | "osTooOld"
  | "unsupportedPlatform"
  | "notBuilt"
  | "unknown";

/** summary_availability */
export interface AiAvailability {
  available: boolean;
  reason: AiUnavailableReason | null;
  /** The model's context window in tokens (0 when unavailable). */
  contextTokens: number;
}

/** Payload of `penguin://summary-progress` while summarize_thread runs. */
export interface SummaryProgress {
  accountId: AccountId;
  threadId: string;
  /** reading = taking notes on part `step` of a long thread; writing = the summary is streaming. */
  stage: "reading" | "writing";
  step: number;
  steps: number;
  /** The summary so far (writing only). */
  partial: AiSummary | null;
}

// ---------------------------------------------------------------------------
// Writing with Apple's on-device model in the composer (docs/COMPOSE-SPEED.md).
// Mirrors the Write* / ReplySuggestions types in penguin-core/src/types.rs.
// ---------------------------------------------------------------------------

/**
 * What to do: `draft` writes the message (or the reply) from `instruction`;
 * the others rewrite `text` (a selection, or the whole draft): shorter,
 * friendlier, more formal, fix spelling and grammar, or as `instruction` says.
 */
export type WriteAction = "draft" | "shorter" | "friendlier" | "formal" | "grammar" | "custom";

/** write_with_ai */
export interface WriteRequest {
  /** Chosen by the UI (1–64 of [A-Za-z0-9_-]); names the run for cancel_write and progress events. */
  runId: string;
  /** The conversation's account when `threadId` is set (its messages are read from there), else the sending account. */
  accountId: AccountId;
  /** The conversation replied to or forwarded (its messages are context for `draft`); null for a new message. */
  threadId: string | null;
  action: WriteAction;
  /** What to write (`draft`) or how to change it (`custom`); ≤ 500 chars. */
  instruction: string;
  /** The text to rewrite; for `draft`, what's written so far (may be empty). ≤ 12,000 chars. */
  text: string;
  /** Subject line of the message being written (context only). */
  subject: string;
  /** Recipients' display names (context only, e.g. to greet them). */
  recipients: string[];
  /** The writer's name: the text is written as them (context only; no sign-off, the signature follows it). */
  myName: string;
}

/** write_with_ai result: the finished text (plain text; blank lines separate paragraphs). */
export interface WriteResult {
  runId: string;
  text: string;
}

/** Payload of `penguin://write-progress`: the text so far (a snapshot, not a delta). */
export interface WriteProgress {
  runId: string;
  text: string;
}

/** suggest_replies result: up to three short replies to the conversation's latest message. */
export interface ReplySuggestions {
  accountId: AccountId;
  threadId: string;
  /** Which version of the thread they answer (penguin-core summary::version_key). */
  version: string;
  replies: string[];
}

/** A file kept with a snippet (save_snippet_file), stored content-addressed on this Mac. */
export interface SnippetFile {
  /** sha256 of the bytes, 64 lowercase hex. */
  id: string;
  filename: string;
  mimeType: string;
  size: number;
}
