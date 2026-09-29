//! Shared domain types. These are THE contract between crates and the UI —
//! every struct serializes camelCase and is mirrored 1:1 in
//! `apps/desktop/src/lib/types.ts`. Change both together.

use serde::{Deserialize, Serialize};

/// Accounts are keyed by lowercased email address.
pub type AccountId = String;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: AccountId,
    pub email: String,
    pub display_name: Option<String>,
    /// Short user-set label ("Sam Work") shown wherever the UI names the
    /// account. Never used in mail headers (display_name is).
    #[serde(default)]
    pub nickname: Option<String>,
    /// Hex color used for the per-account dot in the UI.
    pub color: String,
    /// Unix ms.
    pub added_at: i64,
    /// Which backend the account syncs with. Rows stored before providers
    /// existed are Gmail.
    #[serde(default)]
    pub provider: AccountProvider,
    /// How to reach the provider (servers, client id). Never holds secrets:
    /// passwords and tokens live in the Keychain (docs/PROVIDERS-IMPL.md).
    #[serde(default)]
    pub provider_config: ProviderConfig,
    /// What the provider can do, so the UI hides what it can't. Derived from
    /// `provider` whenever accounts are read from the store
    /// ([`AccountProvider::capabilities`]); never stored.
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// The mail backend of an account. See docs/PROVIDERS-IMPL.md.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AccountProvider {
    /// Gmail REST API with the user's own Google OAuth client.
    #[default]
    Gmail,
    /// IMAP + SMTP (Yahoo, AOL, iCloud, Fastmail, Proton Bridge, any server).
    Imap,
    /// Microsoft Graph (Outlook.com, Hotmail, Microsoft 365).
    Microsoft,
}

impl AccountProvider {
    pub const ALL: [AccountProvider; 3] = [
        AccountProvider::Gmail,
        AccountProvider::Imap,
        AccountProvider::Microsoft,
    ];

    /// The stored and serialized name (`accounts.provider`).
    pub fn as_str(self) -> &'static str {
        match self {
            AccountProvider::Gmail => "gmail",
            AccountProvider::Imap => "imap",
            AccountProvider::Microsoft => "microsoft",
        }
    }

    pub fn parse(s: &str) -> Option<AccountProvider> {
        AccountProvider::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// The service named in user-facing errors ("Gmail no longer has this
    /// attachment"). Lowercase for IMAP ("the mail server"): capitalize it
    /// at the start of a sentence.
    pub fn service_name(self) -> &'static str {
        match self {
            AccountProvider::Gmail => "Gmail",
            AccountProvider::Imap => "the mail server",
            AccountProvider::Microsoft => "Outlook",
        }
    }

    /// What this kind of account supports: the one table the UI and the
    /// desktop layer consult. A provider may do less at runtime (e.g. an
    /// IMAP server without a Drafts folder) and reports that through errors.
    pub fn capabilities(self) -> Capabilities {
        let common = Capabilities {
            server_search: true,
            window_estimate: true,
            drafts: true,
            send_later: true,
            snooze: true,
            message_headers: true,
            raw_source: true,
            ..Capabilities::default()
        };
        match self {
            AccountProvider::Gmail => Capabilities {
                labels: true,
                label_edit: true,
                label_colors: true,
                inbox_categories: true,
                calendar: true,
                contact_photos: true,
                profile_photo: true,
                ..common
            },
            AccountProvider::Imap => Capabilities {
                folders: true,
                ..common
            },
            // Categories are the labels; folders are moves.
            AccountProvider::Microsoft => Capabilities {
                labels: true,
                folders: true,
                profile_photo: true,
                ..common
            },
        }
    }
}

/// What an account's provider can do. Mirrored in `types.ts`; the UI hides
/// actions whose flag is false. See docs/PROVIDERS-IMPL.md → Capabilities.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    /// Several labels per message, added and removed freely (Gmail labels,
    /// Microsoft categories).
    pub labels: bool,
    /// A message lives in one folder per copy; user folders show as labels
    /// and "adding" one is a move (IMAP, Microsoft).
    pub folders: bool,
    /// Rename, recolor, hide and delete user labels (`update_label`,
    /// `delete_label`).
    pub label_edit: bool,
    /// Label colors from Gmail's palette.
    pub label_colors: bool,
    /// IMPORTANT and CATEGORY_* labels exist (split-inbox tabs).
    pub inbox_categories: bool,
    /// "Also search <provider>" (`search_server`).
    pub server_search: bool,
    /// Per-window message counts for Settings → Sync (`sync_window_estimate`).
    pub window_estimate: bool,
    /// Server-side drafts (`save_draft`, `get_draft`, `delete_draft`).
    pub drafts: bool,
    /// Send later (needs drafts).
    pub send_later: bool,
    /// Snooze: a local record plus archive / back-to-inbox moves.
    pub snooze: bool,
    /// Google Calendar.
    pub calendar: bool,
    /// Google contact photos for senders.
    pub contact_photos: bool,
    /// The account's own profile photo (`account_photo`).
    pub profile_photo: bool,
    /// Every header of a message on demand (Message details).
    pub message_headers: bool,
    /// The raw RFC 822 source ("Show original").
    pub raw_source: bool,
}

/// How to reach an account's provider. Only the fields its provider uses
/// are set (Gmail: none). Stored as JSON in `accounts.provider_config`.
/// Never holds a password or token.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderConfig {
    /// IMAP: app password or account password.
    pub auth: Option<AuthMethod>,
    /// IMAP: incoming server.
    pub imap: Option<ServerSettings>,
    /// IMAP: outgoing (SMTP submission) server.
    pub smtp: Option<ServerSettings>,
    /// Microsoft: the Application (client) ID the refresh token belongs to.
    pub client_id: Option<String>,
    /// Microsoft: the directory (tenant) id from the token, once known.
    pub tenant_id: Option<String>,
    /// Who hosts the mailbox as detection saw it (a detection `ProviderKind`
    /// name such as "icloud" or "yahoo"), for server quirks.
    pub host: Option<String>,
}

/// How Penguin signs in to an account. Mirrored in `types.ts` (`AuthMethod`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthMethod {
    /// The user's own Google Cloud client (browser or sign-in sheet).
    GoogleOAuth,
    /// The user's own Microsoft Entra app registration (browser, PKCE).
    MicrosoftOAuth,
    /// A provider-issued app password over IMAP + SMTP.
    AppPassword,
    /// The account's own password (or whatever the server takes) over IMAP + SMTP.
    ImapPassword,
}

/// Transport security of one server. Mirrored in `types.ts` (`MailSecurity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MailSecurity {
    /// TLS from the first byte (IMAPS 993, SMTPS 465).
    Tls,
    /// Plain connection upgraded with STARTTLS (IMAP 143, submission 587).
    Starttls,
    /// No encryption. Only ever offered for a server on this Mac (Bridge).
    Plain,
}

/// One IMAP or SMTP server. Mirrored in `types.ts` (`ServerSettings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentMeta {
    /// Provider attachment id (Gmail `attachmentId`); fetch bytes lazily.
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    /// Content-ID for inline images (`cid:` references), without angle brackets.
    pub content_id: Option<String>,
    /// Drawn inside the body instead of listed as a file. Providers set it
    /// from the part's disposition; [`Message::settle_inline`] then keeps it
    /// only for parts the body can actually draw.
    pub inline: bool,
}

/// A normalized message as produced by a provider and persisted by the store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub account_id: AccountId,
    /// Provider message id (Gmail message id).
    pub id: String,
    pub thread_id: String,
    /// Unix ms (Gmail `internalDate`).
    pub date: i64,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: String,
    pub snippet: String,
    /// Best plain-text body (text/plain part, else html_to_text of the HTML part).
    pub body_text: String,
    /// Raw, UNSANITIZED html body. Never send to the UI without penguin-render.
    pub body_html: Option<String>,
    /// Gmail label ids, incl. system labels (INBOX, UNREAD, STARRED, IMPORTANT, SENT, DRAFT, TRASH, SPAM, CATEGORY_*).
    pub label_ids: Vec<String>,
    pub attachments: Vec<AttachmentMeta>,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub list_unsubscribe: Option<String>,
    /// `List-Unsubscribe-Post: List-Unsubscribe=One-Click` (RFC 8058) was
    /// present. None = unknown: stored before Penguin kept this header.
    #[serde(default)]
    pub list_unsubscribe_post: Option<bool>,
    /// Gmail's receiving MX verified the sender: DMARC pass, or a DKIM pass
    /// whose signing domain aligns with the From domain. Anything else
    /// (including mail with no Google Authentication-Results) is false.
    /// Gate sender-based trust (e.g. auto-loading images) on this, since
    /// From alone is spoofable.
    #[serde(default)]
    pub sender_authenticated: bool,
}

impl Message {
    pub fn is_unread(&self) -> bool {
        self.label_ids.iter().any(|l| l == "UNREAD")
    }
    pub fn is_starred(&self) -> bool {
        self.label_ids.iter().any(|l| l == "STARRED")
    }

    /// Keep `inline` only on parts the body draws: images with a Content-ID
    /// in a message whose HTML uses `cid:` (what the thread view renders).
    /// Every other part is listed as a file, whatever its disposition says.
    /// Apple Mail sends attached PDFs as `Content-Disposition: inline`, often
    /// with a Content-ID and an `<img src="cid:…">` placeholder; left inline,
    /// such a part is neither drawn nor listed, so it shows nowhere.
    /// Providers call this once they have the body.
    pub fn settle_inline(&mut self) {
        let html_uses_cid = self
            .body_html
            .as_deref()
            .is_some_and(|h| h.to_ascii_lowercase().contains("cid:"));
        for a in &mut self.attachments {
            a.inline = a.inline && html_uses_cid && a.is_body_image();
        }
    }
}

impl AttachmentMeta {
    /// An image the body can reference by Content-ID.
    pub fn is_body_image(&self) -> bool {
        self.content_id.is_some() && self.mime_type.to_ascii_lowercase().starts_with("image/")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub account_id: AccountId,
    pub id: String,
    pub name: String,
    /// "system" | "user"
    pub kind: String,
    pub color: Option<String>,
    pub unread_count: Option<u32>,
    /// Hidden from Gmail's label list (`labelListVisibility == "labelHide"`).
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub account_id: AccountId,
    pub thread_id: String,
    pub subject: String,
    /// Snippet of the latest message in the thread.
    pub snippet: String,
    /// Distinct senders, latest first.
    pub participants: Vec<Address>,
    pub message_count: u32,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
    /// Union of labels across the thread's messages.
    pub label_ids: Vec<String>,
    /// Unix ms of the latest message in the view.
    pub last_date: i64,
    /// Verification code or sign-in link in the thread's newest message.
    #[serde(default)]
    pub otp: Option<Otp>,
    /// Unix ms the thread is snoozed until (see [`Snooze`]); None when it
    /// isn't snoozed.
    #[serde(default)]
    pub snoozed_until: Option<i64>,
    /// Calendar invitation in the thread (its newest calendar part), for the
    /// row's event chip. Filled by `Store::attach_invites` from what the
    /// invite scanner parsed; None until then or when there is none.
    #[serde(default)]
    pub invite: Option<InviteChip>,
    /// Smart views only: the fact the row is about (merchant and amount,
    /// route and date, …). None everywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<SmartRow>,
}

/// A one-time code or magic sign-in link found in a message (see `otp.rs`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Otp {
    pub kind: OtpKind,
    /// The code to copy (digits keep leading zeros). None for `link`.
    pub code: Option<String>,
    /// Sender passed DMARC / aligned DKIM (`Message::sender_authenticated`).
    /// Unverified codes are shown muted and never auto-copied.
    pub verified: bool,
    /// Unix ms of the message it came from (codes go stale in minutes).
    pub date: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OtpKind {
    Code,
    Link,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub account_id: AccountId,
    pub thread_id: String,
    pub subject: String,
    pub label_ids: Vec<String>,
    /// Oldest first.
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "labelId", rename_all = "camelCase")]
pub enum MailboxView {
    Inbox,
    Starred,
    Sent,
    Drafts,
    /// Archived: not in INBOX, not TRASH/SPAM/DRAFT-only.
    Done,
    Trash,
    Spam,
    All,
    Label(String),
    /// Snoozed conversations, soonest wake first (not a Gmail label: see
    /// [`Snooze`]). One page holds every snooze in scope.
    Snoozed,
    /// Conversations you owe a reply: every account's [`REPLY_LATER_LABEL`]
    /// label view, merged, newest first (see `store_triage.rs`).
    ReplyLater,
    /// Conversations waiting on a reply: your message is the latest and was
    /// sent between [`FOLLOW_UP_LOOKBACK_DAYS`] and N days ago (see
    /// `Store::list_follow_ups`). Oldest first; one page holds them all.
    FollowUp,
    /// A smart view (`store_smart.rs`, [`SMART_VIEWS`]): "receipts",
    /// "travel", "packages", "bills", "reservations", "invites", "codes",
    /// "files" (or "files:pdf" | "files:images" | "files:docs" |
    /// "files:sheets"), "newsletters", "subscriptions", "people". Rows
    /// carry [`ThreadSummary::smart`]. JSON `{kind: "smart", labelId: id}`.
    Smart(String),
    /// A saved search pinned as a view: the query itself, in the search
    /// language. Newest match first, one page. JSON `{kind: "query",
    /// labelId: query}`.
    Query(String),
}

/// Every smart view id, in the default sidebar order.
pub const SMART_VIEWS: &[&str] = &[
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
];

/// The Files view's type filters ("files:pdf").
pub const SMART_FILE_KINDS: &[&str] = &["pdf", "images", "docs", "sheets"];

/// What a smart view row is about (the fact behind it), shown in place of
/// the snippet. Mirrored in `apps/desktop/src/lib/types.ts`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartRow {
    /// receipt | flight | stay | car | train | bus | parcel | bill |
    /// restaurant | event | reservation | subscription | file
    pub kind: String,
    /// Merchant, "SFO → LIS", hotel, carrier, biller, venue, file name.
    pub title: String,
    pub amount: Option<crate::structured::Money>,
    /// Local wall time or date the row is about ("2026-10-02T19:05",
    /// "2026-10-02"): departure, check-in, expected delivery, due date,
    /// start, the last charge.
    pub at: Option<String>,
    /// A second local date: check-out, drop-off, a subscription's next
    /// expected charge.
    pub end: Option<String>,
    /// Confirmation code, order number, tracking number, invoice number.
    pub reference: Option<String>,
    /// overdue | due | paid | unpaid | shipped | inTransit | outForDelivery |
    /// delivered | exception | cancelled | refunded | upcoming | past |
    /// active | stopped
    pub status: Option<String>,
    /// A secondary line: flight number, items, carrier, cadence, "+2 files".
    pub detail: Option<String>,
    /// The list's section this row belongs to ("Trip to Lisbon · Oct 2–9",
    /// "Overdue", "September 2026"); None = the usual day groups.
    pub group: Option<String>,
}

/// The slim header above a smart view's list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartViewInfo {
    pub view: String,
    pub stats: Vec<SmartStat>,
    /// A caveat ("Still reading 1,204 emails …").
    pub note: Option<String>,
}

/// One figure in a smart view's header: amounts (one per currency, never
/// converted) or a text value.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartStat {
    pub label: String,
    pub value: Option<String>,
    pub amounts: Vec<crate::structured::Money>,
    /// "warn" (overdue) | "good" | None.
    pub tone: Option<String>,
}

/// A smart view's sidebar count (what's active: upcoming trips, parcels on
/// the way, unpaid bills; unread conversations for plain lists).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SmartCount {
    pub view: String,
    pub count: u32,
}

/// The label (Gmail), folder (IMAP) or category (Microsoft) that holds each
/// account's Reply Later conversations. Created on first use.
pub const REPLY_LATER_LABEL: &str = "Reply Later";

/// Follow up: how long a sent message waits before it's listed (days).
pub const FOLLOW_UP_DAYS_DEFAULT: u32 = 3;
pub const FOLLOW_UP_DAYS_MIN: u32 = 1;
pub const FOLLOW_UP_DAYS_MAX: u32 = 14;
/// Follow up looks no further back than this (days): older silence is
/// history, not a nudge.
pub const FOLLOW_UP_LOOKBACK_DAYS: u32 = 60;

/// Per-account sizes of the Reply Later and Follow up views (sidebar counts).
/// Mirrored in `apps/desktop/src/lib/types.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TriageCount {
    pub account_id: AccountId,
    pub reply_later: u32,
    pub follow_up: u32,
}

/// Split-inbox tabs, only meaningful for `MailboxView::Inbox`.
/// - Newsletters: has List-Unsubscribe, or CATEGORY_PROMOTIONS/UPDATES/FORUMS/SOCIAL
/// - Important: IMPORTANT label and not a newsletter
/// - Other: everything else
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InboxTab {
    All,
    Important,
    Other,
    Newsletters,
}

/// Split Inbox: one tab of an inbox divided by search queries
/// (`store_split.rs`). A conversation belongs to the first split whose query
/// matches one of its inbox messages; the last tab ("Other") holds the rest.
/// Only meaningful for `MailboxView::Inbox`, where it replaces `tab`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SplitFilter {
    /// This split's query, in the search language; None = the rest of the
    /// inbox (every conversation no `exclude` query matches).
    pub include: Option<String>,
    /// The queries of the splits before this one: what they claim, this one
    /// doesn't show.
    pub exclude: Vec<String>,
}

/// How many conversations each split holds (`split_counts`): one entry per
/// split query in the order asked, then one for Other. `more` = counting
/// stopped at the cap, so the numbers are a floor.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SplitCounts {
    pub splits: Vec<SplitCount>,
    pub more: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SplitCount {
    pub total: u32,
    pub unread: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub view: MailboxView,
    pub tab: Option<InboxTab>,
    /// None = unified across all accounts.
    pub account_id: Option<AccountId>,
    /// A set of accounts (e.g. a profile). None = no restriction; intersected
    /// with `account_id` when both are set. See [`account_scope`].
    #[serde(default)]
    pub account_ids: Option<Vec<AccountId>>,
    pub limit: u32,
    /// Pagination cursor: only threads with last_date < before.
    pub before: Option<i64>,
    /// Only threads with unread mail in this view (the list's Unread filter).
    #[serde(default)]
    pub unread_only: bool,
    /// Split Inbox: with `MailboxView::Inbox`, list one split (`tab` is
    /// then ignored). None = the whole inbox.
    #[serde(default)]
    pub split: Option<SplitFilter>,
}

/// The accounts a request is limited to: None = every account, Some(empty) =
/// none (e.g. a profile and an account that don't overlap). `account_id` and
/// `account_ids` intersect; the result is deduplicated and keeps the order
/// of `account_ids`.
pub fn account_scope(
    account_id: Option<&str>,
    account_ids: Option<&[AccountId]>,
) -> Option<Vec<AccountId>> {
    match (account_id, account_ids) {
        (None, None) => None,
        (Some(a), None) => Some(vec![a.to_string()]),
        (a, Some(ids)) => {
            let mut out: Vec<AccountId> = Vec::with_capacity(ids.len());
            for id in ids {
                if a.is_none_or(|a| a == id) && !out.contains(id) {
                    out.push(id.clone());
                }
            }
            Some(out)
        }
    }
}

// ---------- search ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    pub account_id: Option<AccountId>,
    /// Limit to a set of accounts (a profile); intersected with `account_id`
    /// and with any `account:` operator in the query.
    #[serde(default)]
    pub account_ids: Option<Vec<AccountId>>,
    pub limit: u32,
}

/// One parsed piece of the query, rendered as a removable chip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchChip {
    /// "from" | "to" | "cc" | "subject" | "has" | "filename" | "label" | "in" | "is" | "before" | "after" | "date" | "account" | "text" | "exclude"
    /// ("date" = a `date:` range such as date:"last spring"; "error" = a `date:` value
    /// that couldn't be read, label "Couldn't read that date"; free words get no chip)
    pub kind: String,
    /// Human label, e.g. "From Mike Delgado", "Has PDF", "Before Mar 1, 2026".
    pub label: String,
    /// Exact source substring in the query, so removing a chip edits the text.
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub account_id: AccountId,
    pub thread_id: String,
    /// Best-matching message in the thread (open the thread AT this message).
    pub message_id: String,
    pub subject: String,
    pub from: Address,
    pub date: i64,
    /// Matching excerpt; matched terms wrapped in <mark>…</mark>, everything else HTML-escaped.
    pub snippet_html: String,
    /// Number of matching messages in this thread.
    pub match_count: u32,
    pub label_ids: Vec<String>,
    pub has_attachments: bool,
    pub unread: bool,
    pub score: f32,
    /// How the conversation matched: `"words"` (the typed words, full-text
    /// index) and/or `"meaning"` (search by meaning). Empty for filter-only
    /// queries and server results.
    #[serde(default)]
    pub matched_by: Vec<String>,
    /// The best-matching passage by meaning, plain text, at most 240
    /// characters. Only when `matched_by` has `"meaning"`.
    #[serde(default)]
    pub passage: Option<String>,
}

/// Whether search by meaning took part in a search.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SemanticStatus {
    /// The index covers the mail; results are hybrid where the query calls for it.
    Ready,
    /// The index is still being built (newest mail first); results already use
    /// the part that is embedded.
    Indexing,
    /// No model or index (not set up, or unavailable): keyword-only.
    #[default]
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentHit {
    pub account_id: AccountId,
    pub thread_id: String,
    pub message_id: String,
    pub attachment: AttachmentMeta,
    pub from: Address,
    pub date: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonHit {
    pub address: Address,
    pub message_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub chips: Vec<SearchChip>,
    pub hits: Vec<SearchHit>,
    pub attachments: Vec<AttachmentHit>,
    pub people: Vec<PersonHit>,
    pub took_ms: f64,
    /// Total messages in the local index (the "142,318 messages indexed locally" badge).
    pub indexed_messages: u64,
    /// The query's parsed date range (unix ms; after inclusive, before
    /// exclusive), so the UI can tell when it reaches past the sync window.
    #[serde(default)]
    pub after_ms: Option<i64>,
    #[serde(default)]
    pub before_ms: Option<i64>,
    /// The "Calendar" group: synced events matching the query (all of the
    /// results for `type:event`). Empty for mail-only filters.
    #[serde(default)]
    pub events: Vec<CalendarEvent>,
    /// Search by meaning: ready, still indexing, or off.
    #[serde(default)]
    pub semantic: SemanticStatus,
    /// Share of mail the meaning index covers (0–1), while `semantic` is
    /// "indexing"; None otherwise.
    #[serde(default)]
    pub semantic_progress: Option<f32>,
}

/// One message matching a rule condition (`Store::match_query`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueryMatch {
    pub account_id: AccountId,
    pub message_id: String,
    pub thread_id: String,
    pub date: i64,
    pub from: Address,
    pub subject: String,
}

// ---------- sync ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SyncPhase {
    Idle,
    Backfilling,
    Incremental,
    Error,
    NeedsReauth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub account_id: AccountId,
    pub phase: SyncPhase,
    /// Messages stored locally for this account.
    pub indexed: u64,
    /// Gmail's messagesTotal from the profile, if known.
    pub total_estimate: Option<u64>,
    pub last_synced_at: Option<i64>,
    pub error: Option<String>,
    /// Backfill throughput over roughly the last minute (messages/min), while backfilling.
    #[serde(default)]
    pub rate_per_min: Option<f64>,
    /// Estimated seconds until backfill completes at the current rate.
    #[serde(default)]
    pub eta_secs: Option<u64>,
    /// Which backfill stage is running while `phase` is Backfilling: full
    /// bodies inside the sync window, or older mail after it. None otherwise.
    #[serde(default)]
    pub stage: Option<SyncStage>,
    /// The failed attempts in a row since sync last made progress; None
    /// while healthy. Set by `record_failure`, closed by `record_progress`
    /// (sync_health.rs), which decides when a failure is worth an alert.
    #[serde(default)]
    pub failure: Option<SyncFailure>,
    /// The last failure streak that ended by itself (sync made progress
    /// again), so the UI can say it recovered. Cleared by the next failure.
    #[serde(default)]
    pub recovered: Option<SyncRecovery>,
}

/// What stopped a sync attempt, for how loudly the UI says so.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SyncErrorKind {
    /// Couldn't connect, timed out, or the connection dropped.
    Network,
    /// The server answered with an error of its own (HTTP 5xx, IMAP NO/BAD).
    Server,
    /// The server is limiting requests.
    RateLimited,
    /// The server refused the credentials: sign in again.
    Auth,
    /// The Keychain refused to hand over the saved credentials.
    Keychain,
    /// The local mail database failed.
    Storage,
    /// The sync task crashed (a panic).
    Internal,
    /// The account's settings or the app's client setup are missing or wrong.
    Config,
    Other,
}

/// A run of failed sync attempts with nothing synced in between.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncFailure {
    /// The latest attempt's kind (the message is `SyncStatus.error`).
    pub kind: SyncErrorKind,
    /// Failed attempts in a row (1 = the first).
    pub count: u32,
    /// When the first of them failed (ms).
    pub first_at: i64,
    /// When the latest failed (ms).
    pub last_at: i64,
    /// When sync tries again on its own (ms); None = it won't until someone
    /// acts (signed out, crashed, bad settings).
    pub next_retry_at: Option<i64>,
    /// Worth telling the user: it needs them, or it has lasted
    /// (`SYNC_ALERT_FAILURES` attempts or `SYNC_ALERT_AFTER_MS`). Until
    /// then the UI says "Retrying…" quietly.
    pub alert: bool,
}

/// A failure streak that ended because sync made progress again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecovery {
    /// When sync made progress again (ms).
    pub at: i64,
    /// The streak's first failure (ms).
    pub since: i64,
    /// How many attempts had failed.
    pub failures: u32,
    /// The last failure's kind.
    pub kind: SyncErrorKind,
    /// Whether the streak had been alerted (the user saw it).
    pub alerted: bool,
}

/// Backfill stages (see `WindowCursor`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SyncStage {
    /// Full messages inside the sync window, newest first.
    Window,
    /// Mail older than the window: headers only, or full bodies when
    /// `olderMail` is "full". Runs only after the window is complete.
    Older,
}

/// Persisted per-account sync position (`sync_cursors`). The generic part
/// (backfill done, parked ids, window coverage) is read by the app for
/// status, coverage and diagnostics; `provider_state` belongs to the
/// account's provider alone.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncCursor {
    /// The provider's own resume position, opaque to everyone else: JSON
    /// text by convention, "" = nothing recorded yet. Gmail:
    /// `{"historyId": 123, "backfillPageToken": "…"}` (penguin-gmail
    /// `sync::GmailCursor`); IMAP: per-folder UIDVALIDITY/UIDNEXT/MODSEQ;
    /// Microsoft: per-folder delta links. See docs/PROVIDERS-IMPL.md.
    #[serde(default)]
    pub provider_state: String,
    /// The window fill (first full download) has finished.
    pub backfill_done: bool,
    /// Message ids that failed to fetch repeatedly (non-404). Backfill skips
    /// past them so one bad message never stalls the mailbox; the engine
    /// retries these on every incremental cycle until they succeed or 404.
    #[serde(default)]
    pub failed_message_ids: Vec<String>,
    /// Sync-window progress (full bodies for recent mail, headers for older).
    #[serde(default)]
    pub window: WindowCursor,
}

/// Resumable state of the windowed backfill. All times are unix ms.
///
/// Stage A ("fill") downloads full messages listed by
/// `after:fill_after_ms [before:fill_before_ms]`, newest first. When it
/// completes, `full_since_ms` moves down to `fill_after_ms`. Growing the
/// window starts another fill for just the new band; shrinking it changes
/// nothing (bodies are only dropped by an explicit "free up space").
/// Stage B ("older") lists `before:older_before_ms` (= `full_since_ms`) and
/// stores headers only (or full messages, per `older_mode`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowCursor {
    /// Every message dated at or after this has its full body (apart from
    /// parked failures). None = the first fill hasn't finished; 0 = all mail.
    pub full_since_ms: Option<i64>,
    /// The fill in progress: lower bound, upper bound (None = now), page token.
    pub fill_after_ms: Option<i64>,
    pub fill_before_ms: Option<i64>,
    pub fill_page_token: Option<String>,
    /// The older-mail pass: its upper bound, page token, and whether it
    /// finished, for `older_mode` ("headers" | "full").
    pub older_before_ms: Option<i64>,
    pub older_page_token: Option<String>,
    pub older_done: bool,
    pub older_mode: Option<String>,
}

// ---------- outbox (send later, remind if no reply) ----------

/// A saved Gmail draft queued to be sent at `send_at`. The content lives in
/// the draft itself (edits before `send_at` are sent); this is only the
/// schedule. Mirrored in `apps/desktop/src/lib/types.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledSend {
    pub id: String,
    pub account_id: AccountId,
    pub draft_id: String,
    /// Unix ms. Moved forward when a send attempt fails and will be retried.
    pub send_at: i64,
    pub created_at: i64,
    /// After sending, remind (re-surface the thread) if nobody replies within
    /// this many ms.
    pub remind_after_ms: Option<i64>,
    /// Failed attempts so far.
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// "Remind me if no reply": at `remind_at`, if the thread has no message
/// from someone else newer than `sent_at`, it returns to the inbox as
/// unread. Mirrored in `apps/desktop/src/lib/types.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub id: String,
    pub account_id: AccountId,
    pub thread_id: String,
    /// The message being waited on, when known.
    pub sent_message_id: Option<String>,
    /// Unix ms; only replies after this count.
    pub sent_at: i64,
    /// Unix ms.
    pub remind_at: i64,
    pub created_at: i64,
}

/// A snoozed conversation. Gmail has no snooze API, so snoozing is an
/// archive (INBOX removed) plus this local record; at `wake_at` the app puts
/// the thread back in the inbox, unread, at the top. New mail in the thread
/// (anything that puts it back in the inbox) ends the snooze early.
/// Mirrored in `apps/desktop/src/lib/types.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Snooze {
    pub account_id: AccountId,
    pub thread_id: String,
    /// Unix ms it wakes.
    pub wake_at: i64,
    /// Unix ms it was snoozed.
    pub snoozed_at: i64,
}

/// Ids of a message just sent (`send_message`), e.g. to set a reminder on
/// its thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SentRef {
    pub message_id: String,
    pub thread_id: String,
}

// ---------- calendar ----------
// Google Calendar events synced per account (penguin-gmail/src/calendar.rs,
// store in store_calendar.rs). Mirrored in `apps/desktop/src/lib/types.ts`.

/// One calendar from the account's calendar list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarInfo {
    pub account_id: AccountId,
    /// Google calendar id (the primary calendar's is the account address).
    pub id: String,
    pub summary: String,
    /// `#rrggbb` from Google's calendar list, when set.
    pub color: Option<String>,
    /// Synced and shown. Starts as Google's own "selected" flag; toggled locally.
    pub selected: bool,
    pub primary: bool,
    /// owner | writer | reader | freeBusyReader
    pub access_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventAttendee {
    /// Lowercased.
    pub email: String,
    pub name: Option<String>,
    /// accepted | tentative | declined | needsAction
    pub response: String,
    pub organizer: bool,
    /// This attendee is the account itself.
    #[serde(rename = "self")]
    pub is_self: bool,
    pub optional: bool,
    /// A room or other resource, not a person.
    pub resource: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub account_id: AccountId,
    pub calendar_id: String,
    /// Google event id (an instance id like `abc_20260925T150000Z` for
    /// recurring events, which are expanded).
    pub id: String,
    pub ical_uid: Option<String>,
    /// confirmed | tentative
    pub status: String,
    pub summary: String,
    /// Plain text (Google sends limited HTML; it is reduced to text here and
    /// the UI only linkifies it).
    pub description: String,
    pub location: String,
    /// Unix ms. All-day events use local midnight of `start_date`.
    pub start: i64,
    /// Unix ms, exclusive.
    pub end: i64,
    pub all_day: bool,
    /// `YYYY-MM-DD` for all-day events (end exclusive, as Google sends it).
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub organizer: Option<Address>,
    pub attendees: Vec<EventAttendee>,
    /// The account's own response, when it is an attendee.
    pub my_response: Option<String>,
    /// Google Calendar web link (https only).
    pub html_link: Option<String>,
    /// Meet / Zoom / Teams / Webex join link (https only).
    pub conference_url: Option<String>,
    /// meet | zoom | teams | webex | other
    pub conference_kind: Option<String>,
    pub recurring_event_id: Option<String>,
    /// "Show as available" (transparency: transparent): never a conflict.
    pub free: bool,
    /// Unix ms of Google's last change.
    pub updated: i64,
}

/// One parsed calendar part of a message (`invites.data`): what the chip,
/// the card and "What changed" need. Descriptions are left out.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InviteSnapshot {
    /// request | cancel | reply | counter | publish | other
    pub method: String,
    pub uid: Option<String>,
    pub sequence: i64,
    /// RECURRENCE-ID as unix ms: this invite is about one instance.
    pub recurrence_id: Option<i64>,
    /// A recurring series (RRULE).
    pub recurring: bool,
    pub summary: String,
    pub location: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    /// DTSTART's TZID (the organizer's zone) when it isn't UTC.
    pub time_zone: Option<String>,
    pub organizer: Option<Address>,
    pub attendees: Vec<EventAttendee>,
}

impl InviteSnapshot {
    /// The account's own attendee row.
    pub fn me(&self) -> Option<&EventAttendee> {
        self.attendees.iter().find(|a| a.is_self)
    }

    /// Fields that differ from `previous` (an older version of the same
    /// event): time | location | title | guests, in that order.
    pub fn changes_since(&self, previous: &InviteSnapshot) -> Vec<String> {
        let mut out = Vec::new();
        if self.start != previous.start
            || self.end != previous.end
            || self.all_day != previous.all_day
        {
            out.push("time".to_string());
        }
        if self.location.trim() != previous.location.trim() {
            out.push("location".into());
        }
        if self.summary.trim() != previous.summary.trim() {
            out.push("title".into());
        }
        fn guests(s: &InviteSnapshot) -> Vec<&str> {
            let mut v: Vec<&str> = s
                .attendees
                .iter()
                .filter(|a| !a.resource)
                .map(|a| a.email.as_str())
                .collect();
            v.sort_unstable();
            v
        }
        if !previous.attendees.is_empty() && guests(self) != guests(previous) {
            out.push("guests".into());
        }
        out
    }
}

/// Your answer to an invitation as Penguin sent it (`invite_responses`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InviteResponse {
    /// accepted | tentative | declined
    pub response: String,
    /// How it went out: calendar (Google Calendar API) | graph (Microsoft
    /// Graph event action) | email (iMIP REPLY/COUNTER to the organizer).
    pub via: String,
    /// The SEQUENCE answered: a newer version of the invite asks again
    /// unless it carries your answer itself.
    pub sequence: i64,
    pub comment: Option<String>,
    /// A proposed new time (unix ms), when one was sent.
    pub proposed_start: Option<i64>,
    pub proposed_end: Option<i64>,
    /// Unix ms it was sent.
    pub at: i64,
}

/// The event chip in a list row (`ThreadSummary.invite`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InviteChip {
    /// The message whose calendar part this is.
    pub message_id: String,
    pub uid: Option<String>,
    /// request | cancel | reply | counter | publish | other
    pub method: String,
    /// A request that changes an earlier version (SEQUENCE > 0, or an older
    /// copy of it is stored).
    pub updated: bool,
    pub summary: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub recurring: bool,
    /// Your answer (accepted | tentative | declined | needsAction), newest
    /// of: what Penguin sent, the synced calendar event, the invite itself.
    /// None when you aren't on the guest list.
    pub response: Option<String>,
    /// Reply/counter mail: the guest who answered, with their answer.
    pub replier: Option<EventAttendee>,
    /// Yes / Maybe / No make sense here: a request you're a guest of (not
    /// the organizer), with a UID and organizer, that hasn't ended.
    pub can_respond: bool,
    /// First busy event it overlaps on your calendars, and how many.
    pub conflict: Option<String>,
    pub conflicts: u32,
}

/// "When did I last meet X": the person's latest past and soonest upcoming
/// event among synced calendars (local only).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonMeetings {
    pub email: String,
    pub last: Option<CalendarEvent>,
    pub next: Option<CalendarEvent>,
    /// Events with them in the synced window.
    pub count: u32,
}

// ---------------------------------------------------------------------------
// Thread summaries (on-device, docs/SUMMARIES.md)
// ---------------------------------------------------------------------------

/// A summary of one version of a thread. `points` and `asks` link to the
/// message they came from (`message_id` None = the model cited a message
/// that wasn't in the input, so the item links nowhere).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiSummary {
    pub account_id: AccountId,
    pub thread_id: String,
    /// `summary::version_key` of the messages it read.
    pub version: String,
    /// One or two sentences.
    pub gist: String,
    pub points: Vec<AiSummaryPoint>,
    /// Things someone asked of you, with any deadline.
    pub asks: Vec<AiSummaryAsk>,
    /// Messages read.
    pub message_count: u32,
    /// Earlier messages left out of a very long thread.
    pub omitted: u32,
    /// Unix ms.
    pub created_at: i64,
    /// The thread changed since (a cached summary of an older version).
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiSummaryPoint {
    pub text: String,
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiSummaryAsk {
    pub text: String,
    pub message_id: Option<String>,
    /// The deadline as the mail wrote it ("Friday", "by Oct 3").
    pub due: Option<String>,
}

/// Why summaries can't run on this Mac. The first three are Apple's
/// `SystemLanguageModel.Availability.UnavailableReason`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AiUnavailableReason {
    /// Not an Apple Intelligence Mac (Apple silicon is required).
    DeviceNotEligible,
    /// Apple Intelligence is off in System Settings.
    AppleIntelligenceNotEnabled,
    /// The model is still downloading or being set up.
    ModelNotReady,
    /// macOS older than 26.
    OsTooOld,
    /// Not a Mac (e.g. Linux): no on-device model here.
    UnsupportedPlatform,
    /// This build was made without the Foundation Models SDK.
    NotBuilt,
    /// A reason newer than this build knows.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiAvailability {
    pub available: bool,
    pub reason: Option<AiUnavailableReason>,
    /// The model's context window in tokens (0 when unavailable).
    pub context_tokens: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SummaryStage {
    /// Map steps: taking notes on part `step` of `steps`.
    Reading,
    /// The summary itself is streaming.
    Writing,
}

/// Payload of `penguin://summary-progress` while `summarize_thread` runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SummaryProgress {
    pub account_id: AccountId,
    pub thread_id: String,
    pub stage: SummaryStage,
    pub step: u32,
    pub steps: u32,
    /// The summary so far (Writing only).
    pub partial: Option<AiSummary>,
}

// ---------------------------------------------------------------------------
// Writing in the composer with the on-device model (penguin-core writing.rs,
// src-tauri src/writing/; mirrored at the end of src/lib/types.ts)
// ---------------------------------------------------------------------------

/// `draft` writes the message (or the reply) from the instruction; the
/// others rewrite `text`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum WriteAction {
    Draft,
    Shorter,
    Friendlier,
    Formal,
    Grammar,
    Custom,
}

/// `write_with_ai` argument.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WriteRequest {
    /// Chosen by the UI: 1–64 of [A-Za-z0-9_-].
    pub run_id: String,
    pub account_id: AccountId,
    /// The conversation replied to or forwarded; None for a new message.
    #[serde(default)]
    pub thread_id: Option<String>,
    pub action: WriteAction,
    /// What to write (`draft`) or how to change it (`custom`); ≤ 500 chars.
    #[serde(default)]
    pub instruction: String,
    /// The text to rewrite; for `draft`, what's written so far. ≤ 12,000 chars.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub subject: String,
    /// Recipients' display names (context only).
    #[serde(default)]
    pub recipients: Vec<String>,
    /// The writer's name, for a sign-off.
    #[serde(default)]
    pub my_name: String,
}

/// `write_with_ai` result: plain text, blank lines between paragraphs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    pub run_id: String,
    pub text: String,
}

/// Payload of `penguin://write-progress`: the text so far (a snapshot).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WriteProgress {
    pub run_id: String,
    pub text: String,
}

/// `suggest_replies` result: up to three short replies to the newest message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplySuggestions {
    pub account_id: AccountId,
    pub thread_id: String,
    /// `summary::version_key` of the messages it read.
    pub version: String,
    pub replies: Vec<String>,
}
