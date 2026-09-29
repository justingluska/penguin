//! The one error type every provider returns. The app maps it to the
//! command error codes (`src-tauri/src/error.rs`):
//!
//! | variant | code | meaning |
//! |---|---|---|
//! | `NotConfigured` | notConfigured | the app lacks the provider's client setup |
//! | `NeedsReauth` | needsReauth | credentials missing, revoked or rejected: sign in again (sync stops) |
//! | `Keychain` | other (sync: needsReauth) | the Keychain refused; never retried on a timer |
//! | `SignInCancelled` | cancelled | the user closed the sign-in |
//! | `Network`, `RateLimited` | network | retryable; sync backs off |
//! | `Http { status ≥ 500 or 429 }` | network | retryable |
//! | `Http { 404 }`, `NotFound` | notFound | the object is gone on the server |
//! | `InvalidInput` | invalidInput | the server refused what the user asked (name taken, bad password) |
//! | `Unsupported` | other | this provider can't do that (see `Capabilities`) |
//! | `Store` | (the store's) | local database |
//! | everything else | other | |
//!
//! Gmail's own `penguin_gmail::Error` converts into this one variant for
//! variant (same messages), so Gmail behaves exactly as before.

use penguin_core::{AccountProvider, SyncErrorKind};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("OAuth client not configured: {0}")]
    NotConfigured(String),
    #[error("account needs to sign in again: {0}")]
    NeedsReauth(String),
    #[error("oauth: {0}")]
    OAuth(String),
    /// The user closed the sign-in or declined consent.
    #[error("sign-in cancelled")]
    SignInCancelled,
    #[error("keychain: {0}")]
    Keychain(String),
    /// An HTTP API answered with an error status (Gmail, Graph). `body` is
    /// the server's text; never user content.
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error("rate limited")]
    RateLimited,
    #[error("network: {0}")]
    Network(String),
    /// The object doesn't exist on the server (any protocol).
    #[error("{0}")]
    NotFound(String),
    /// The server refused the request as given (user-facing text).
    #[error("{0}")]
    InvalidInput(String),
    /// This provider can't do it (the capability is off).
    #[error("{0}")]
    Unsupported(String),
    #[error("store: {0}")]
    Store(#[from] penguin_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// The server no longer has the object: HTTP 404 or [`Error::NotFound`].
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::NotFound(_) | Error::Http { status: 404, .. })
    }

    /// Worth retrying later without anyone acting.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Http { status, .. } => *status == 429 || *status >= 500,
            Error::Network(_) | Error::RateLimited => true,
            _ => false,
        }
    }

    /// What kind of problem this is when it stops a sync attempt
    /// (`SyncStatus::record_failure`).
    pub fn sync_kind(&self) -> SyncErrorKind {
        match self {
            Error::NeedsReauth(_) => SyncErrorKind::Auth,
            Error::Keychain(_) => SyncErrorKind::Keychain,
            Error::NotConfigured(_) => SyncErrorKind::Config,
            Error::Network(_) => SyncErrorKind::Network,
            Error::RateLimited | Error::Http { status: 429, .. } => SyncErrorKind::RateLimited,
            Error::Http { status, .. } if *status >= 500 => SyncErrorKind::Server,
            Error::Store(_) => SyncErrorKind::Storage,
            _ => SyncErrorKind::Other,
        }
    }

    /// "<what> isn't available for <provider> accounts".
    pub fn unsupported(provider: AccountProvider, what: &str) -> Error {
        Error::Unsupported(format!(
            "{what} isn't available for {} accounts",
            provider_label(provider)
        ))
    }

    /// For the phase-0 stubs the IMAP and Microsoft providers replace:
    /// "Connecting an account is not implemented yet for IMAP".
    pub fn not_implemented(provider: AccountProvider, what: &str) -> Error {
        Error::Unsupported(format!(
            "{what} is not implemented yet for {}",
            provider_label(provider)
        ))
    }
}

/// "Gmail", "IMAP", "Microsoft": the account type as the UI names it.
pub fn provider_label(provider: AccountProvider) -> &'static str {
    match provider {
        AccountProvider::Gmail => "Gmail",
        AccountProvider::Imap => "IMAP",
        AccountProvider::Microsoft => "Microsoft",
    }
}
