//! The provider seam: what the app asks of a mail backend, per account
//! ([`MailProvider`]) and per kind ([`Backend`]). Gmail implements both by
//! wrapping penguin-gmail (`penguin_gmail::provider`); IMAP and Microsoft
//! Graph implement them in their own crates. The contract (identity, label
//! and folder mapping, cursors, errors, tests) is docs/PROVIDERS-IMPL.md.

use std::sync::Arc;

use async_trait::async_trait;
use penguin_core::query::ParsedQuery;
use penguin_core::{
    Account, AccountProvider, Address, AttachmentMeta, Capabilities, Label, Message, SearchHit,
    SentRef, SyncStatus,
};
use serde::{Deserialize, Serialize};

use crate::compose::{AttachmentBytes, Draft, OutgoingAttachment};
use crate::window::WindowPolicy;
use crate::{Error, Result};

/// Unknown messages a server search stores (headers-only) per account.
pub const SERVER_SEARCH_FETCH_CAP: usize = 50;

/// Every header of one message, in server order (topmost first), plus the
/// server's size for the whole message when it reports one ("Message
/// details").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageMetadata {
    pub headers: Vec<(String, String)>,
    pub size_estimate: Option<u64>,
}

/// An answer to the calendar invitation a message carries
/// ([`MailProvider::respond_to_invitation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationAnswer {
    /// accepted | tentative | declined
    pub response: String,
    /// A note to the organizer.
    pub comment: Option<String>,
    /// A proposed new time (unix ms start, end).
    pub proposal: Option<(i64, i64)>,
}

/// A label color: Gmail's palette pair (background, text), both `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelColor {
    pub background: String,
    pub text: String,
}

/// A validated change to one user label; `None` fields are unchanged,
/// `color: Some(None)` removes the color.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelUpdate {
    pub name: Option<String>,
    pub color: Option<Option<LabelColor>>,
    pub hidden: Option<bool>,
}

/// One account's server-search outcome.
#[derive(Debug, Clone, Default)]
pub struct ServerSearch {
    /// One hit per thread (newest matching message), newest first. Every
    /// hit's message is in the store when this returns.
    pub hits: Vec<SearchHit>,
    /// Matches that weren't stored and were fetched (headers-only).
    pub fetched: u32,
    /// The server's estimate of all matches.
    pub estimate: u64,
}

/// The server's identifiers for one draft (or, after a send, the sent
/// message). Mirrored in `types.ts` (`DraftRef`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftRef {
    pub draft_id: String,
    pub message_id: String,
    pub thread_id: String,
    /// After a save: the draft's attachments as refs to the NEW draft
    /// message (Gmail mints new message and attachment ids on every save),
    /// in the order they were sent. Empty when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<OutgoingAttachment>,
}

/// A stored draft reopened for editing. Mirrored in `types.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedDraft {
    pub draft_id: String,
    pub message_id: String,
    pub thread_id: String,
    pub draft: Draft,
}

/// One account's mail backend: everything the app does to a mailbox apart
/// from the background sync loop ([`Backend::start_sync`]). One instance
/// per account, cached by the app; implementations hold their own client,
/// credentials access and a `Store` clone.
///
/// Rules every implementation follows (details in docs/PROVIDERS-IMPL.md):
/// - Ids are the ones the provider stored (`Message.id`, `thread_id`,
///   `AttachmentMeta.id`, label ids): opaque, stable, per account.
/// - Labels are the canonical system names (INBOX, SENT, DRAFT, TRASH,
///   SPAM, STARRED, UNREAD, IMPORTANT) or the provider's user label/folder
///   ids. `modify_thread` translates them (IMAP: flags and moves).
/// - Interactive calls: they run because the user is waiting; don't queue
///   them behind the sync.
/// - Never log bodies, subjects, addresses or secrets; ids and codes only.
#[async_trait]
pub trait MailProvider: Send + Sync {
    fn account_id(&self) -> &str;
    fn provider(&self) -> AccountProvider;
    /// What the account supports. Defaults to the kind's table.
    fn capabilities(&self) -> Capabilities {
        self.provider().capabilities()
    }

    // ----- reading on demand -----

    /// Full messages by id, one outcome per id in input order; `Ok(None)` =
    /// the server no longer has it.
    async fn fetch_messages(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)>;

    /// Download and store the full bodies of headers-only messages (an
    /// opened thread's `bodyPending` messages). Ids already being fetched
    /// or no longer body-pending are skipped. Returns the threads that
    /// changed. Errors only when nothing could be stored.
    async fn fetch_pending_bodies(&self, ids: &[String]) -> Result<Vec<String>>;

    /// One attachment's bytes (the caller caches them).
    async fn get_attachment(
        &self,
        message_id: &str,
        attachment: &AttachmentMeta,
    ) -> Result<Vec<u8>>;

    /// Every header of a message; `None` when the server no longer has it.
    async fn get_message_metadata(&self, message_id: &str) -> Result<Option<MessageMetadata>>;

    /// The raw RFC 822 source; `None` when the server no longer has it.
    async fn get_message_raw(&self, message_id: &str) -> Result<Option<Vec<u8>>>;

    // ----- changing mail (the optimistic modify path, rules, snooze) -----

    /// Add and remove labels on every message of a thread. The app has
    /// already applied the change locally; an error reverts it there.
    async fn modify_thread(&self, thread_id: &str, add: &[String], remove: &[String])
        -> Result<()>;

    /// Move a thread to the trash.
    async fn trash_thread(&self, thread_id: &str) -> Result<()>;

    /// Take a thread out of the trash and put it in the inbox.
    async fn untrash_thread(&self, thread_id: &str) -> Result<()>;

    /// Rename / recolor / hide a user label; returns it as the server now
    /// has it (`account_id` may be left empty; the app fills it in).
    /// `Capabilities::label_edit`.
    async fn update_label(&self, label_id: &str, _update: &LabelUpdate) -> Result<Label> {
        let _ = label_id;
        Err(Error::unsupported(self.provider(), "Editing labels"))
    }

    /// Delete a user label (a missing label counts as deleted).
    /// `Capabilities::label_edit`.
    async fn delete_label(&self, label_id: &str) -> Result<()> {
        let _ = label_id;
        Err(Error::unsupported(self.provider(), "Deleting labels"))
    }

    /// The user label (Gmail), folder (IMAP) or category (Microsoft) named
    /// `name`, created when the account has none; returned as the provider
    /// has it (`account_id` may be left empty; the app fills it in and
    /// stores it). Reply Later uses it. Interactive. Every provider
    /// implements it (labels or folders).
    async fn ensure_label(&self, name: &str) -> Result<Label> {
        let _ = name;
        Err(Error::unsupported(self.provider(), "Creating labels"))
    }

    // ----- sending -----

    /// Send a complete RFC 822 message (built with [`crate::compose`]).
    /// `thread_id`: the conversation it replies in, when known. The sent
    /// copy reaches the store through sync (the app pokes it).
    async fn send_raw(&self, raw: &[u8], thread_id: Option<&str>) -> Result<SentRef>;

    /// Create (`draft_id` None) or update a server draft and mirror it
    /// locally (label DRAFT) so the Drafts view shows it at once. A draft
    /// gone from the server is recreated. `attachments` are
    /// `draft.attachments` resolved to bytes, in order.
    async fn save_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: Option<&str>,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef>;

    /// Send a saved draft with its final content, removing the draft
    /// (server and local copy). Returns the sent message's ids.
    async fn send_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: &str,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef>;

    /// Send a saved draft exactly as stored (send later). A draft that no
    /// longer exists is a not-found error.
    async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef>;

    /// Delete a draft on the server (missing = done) and locally. Returns
    /// the thread it was in, when it was stored locally.
    async fn delete_draft(&self, draft_id: &str) -> Result<Option<String>>;

    /// Reopen a draft by draft id or by its message id; `None` when the
    /// server has no such draft.
    async fn open_draft(
        &self,
        draft_id: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<Option<OpenedDraft>>;

    // ----- optional -----

    /// Search the server for `query`, store up to `fetch_cap` unknown
    /// matches headers-only and return the matches as hits.
    /// `Capabilities::server_search`.
    async fn server_search(&self, query: &ParsedQuery, fetch_cap: usize) -> Result<ServerSearch> {
        let _ = (query, fetch_cap);
        Err(Error::unsupported(self.provider(), "Server search"))
    }

    /// How many messages the server holds from the start of a `months`
    /// window (0 = the whole mailbox). `Capabilities::window_estimate`.
    async fn window_estimate(&self, months: u32, now_ms: i64) -> Result<u64> {
        let _ = (months, now_ms);
        Err(Error::unsupported(
            self.provider(),
            "Mailbox size estimates",
        ))
    }

    /// The account's own profile photo bytes (any raster format), `None`
    /// when it has none. `Capabilities::profile_photo`. (Gmail's is fetched
    /// by the app through the People API; see `avatars/account.rs`.)
    async fn profile_photo(&self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    /// Answer the calendar invitation `message_id` carries through the
    /// provider's own calendar, which tells the organizer (Microsoft:
    /// Graph event accept / tentativelyAccept / decline, with
    /// `proposedNewTime`). Needs calendar access the account may not have
    /// granted: then `Unsupported` or an HTTP 403, and the app answers by
    /// email (iMIP) instead. Default: `Unsupported` (Gmail answers through
    /// Google Calendar in the app; IMAP has no calendar).
    async fn respond_to_invitation(
        &self,
        _message_id: &str,
        _answer: &InvitationAnswer,
    ) -> Result<()> {
        Err(Error::unsupported(self.provider(), "Answering invitations"))
    }
}

/// Callbacks from a sync engine into the app (Tauri emits these as events).
/// Moved from penguin-gmail; every provider's engine reports through it.
pub trait SyncObserver: Send + Sync + 'static {
    fn status(&self, status: SyncStatus);
    /// Threads whose local state changed (new mail, label changes, deletions).
    fn mail_changed(&self, account_id: &str, thread_ids: Vec<String>);
    /// Messages that arrived through incremental sync — new mail, not
    /// backfill or reconcile. Called after they're committed and before the
    /// cursor that covers them is saved, so after a crash they're reported
    /// again. Keep it quick (the rules engine queues).
    fn messages_added(&self, _account_id: &str, _message_ids: Vec<String>) {}
    /// Labels added to messages already stored: (message id, added label
    /// ids). Same timing guarantee as `messages_added`.
    fn labels_added(&self, _account_id: &str, _changes: Vec<(String, Vec<String>)>) {}
}

/// A running per-account sync task, as its engine exposes it.
pub trait SyncTask: Send + Sync {
    /// Sync now (after a user action, on window focus).
    fn poke(&self);
    /// Stop the task. Must leave the cursor resumable.
    fn stop(&self);
    fn is_running(&self) -> bool;
}

/// Handle to a running per-account sync task (cheap to clone).
#[derive(Clone)]
pub struct SyncHandle(Arc<dyn SyncTask>);

impl SyncHandle {
    pub fn new(task: Arc<dyn SyncTask>) -> SyncHandle {
        SyncHandle(task)
    }
    pub fn poke(&self) {
        self.0.poke();
    }
    pub fn stop(&self) {
        self.0.stop();
    }
    pub fn is_running(&self) -> bool {
        self.0.is_running()
    }
}

/// One provider kind: builds per-account clients and runs the sync tasks.
/// The app holds one per [`AccountProvider`] (docs/PROVIDERS-IMPL.md →
/// Registering a backend).
#[async_trait]
pub trait Backend: Send + Sync {
    fn provider(&self) -> AccountProvider;

    /// The account's API client. Cheap; the app caches one per account and
    /// drops it when the account is removed or the backend replaced.
    fn client(&self, account: &Account) -> Result<Arc<dyn MailProvider>>;

    /// The Keychain holds what the account needs to sync. May block on the
    /// Keychain (the app calls it on the blocking pool); never prompts more
    /// than once per process.
    fn has_credentials(&self, account: &Account) -> bool;

    /// Forget the account's credentials (and revoke tokens where the
    /// provider can). Best effort: the app removes local data regardless.
    async fn sign_out(&self, account: &Account) -> Result<()>;

    /// Start (or return the running) background sync for the account:
    /// backfill per the window policy, then incremental changes, reporting
    /// through the backend's [`SyncObserver`]. A task that ended (needs
    /// re-auth, panicked) is replaced.
    fn start_sync(&self, account: &Account) -> SyncHandle;

    /// "Retry sync": clear the account's error status (emitting it), then
    /// restart its task if it stopped or wake it from error backoff.
    fn retry_sync(&self, account: &Account) -> SyncHandle;

    /// Last status of the account's task; `None` if it never ran here.
    fn sync_status(&self, account_id: &str) -> Option<SyncStatus>;

    /// New Settings → Sync values, applied live to every running account.
    fn set_window_policy(&self, policy: WindowPolicy);
}
