//! penguin-provider: the seam between the app and mail backends.
//!
//! - [`MailProvider`] (one account) and [`Backend`] (one provider kind, runs
//!   the sync tasks): the interface Gmail, IMAP and Microsoft Graph
//!   implement. The app dispatches every mailbox call through them by
//!   `Account.provider`.
//! - Shared, provider-neutral pieces: outgoing MIME ([`compose`], inline
//!   images in [`inline`]), send
//!   later and reminders ([`outbox`]), snooze wakes ([`snooze`]), the sync
//!   window policy ([`window`]), the backfill heartbeat ([`heartbeat`]), id
//!   and label rules ([`ids`]), Keychain vaults for passwords and Microsoft
//!   tokens ([`credentials`]), message snippets ([`text`]).
//! - Tests: an in-memory provider ([`fake`], feature `fake`) and the
//!   conformance suite every provider must pass ([`conformance`]).
//!
//! No Tauri here; networking belongs to the provider crates. The contract
//! is docs/PROVIDERS-IMPL.md.

pub mod compose;
#[cfg(any(test, feature = "fake"))]
pub mod conformance;
pub mod credentials;
mod error;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
pub mod heartbeat;
pub mod ids;
pub mod inline;
pub mod outbox;
mod provider;
pub mod quote;
pub mod snooze;
pub mod text;
pub mod window;

pub use error::{provider_label, Error, Result};
pub use provider::{
    Backend, DraftRef, InvitationAnswer, LabelColor, LabelUpdate, MailProvider, MessageMetadata,
    OpenedDraft, ServerSearch, SyncHandle, SyncObserver, SyncTask, SERVER_SEARCH_FETCH_CAP,
};

/// Re-exported so implementations name the same macro version.
pub use async_trait::async_trait;
