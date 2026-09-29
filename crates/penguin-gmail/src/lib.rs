//! penguin-gmail: Google OAuth (installed-app loopback + PKCE), Keychain token
//! storage, Gmail REST client, the sync engine, Google Calendar, and the
//! Gmail implementation of the provider seam ([`provider`]).
//! Talks to Google directly from the device — there is no Penguin server.

pub mod api;
pub mod auth;
pub mod calendar;
pub mod convert;
pub mod details;
pub mod drafts;
pub mod ics;
pub mod imip;
pub mod label_colors;
pub mod probe;
pub mod provider;
pub mod sync;

// Provider-neutral since the provider seam; kept at their old paths.
pub use penguin_provider::{compose, outbox, snooze};

static APP_VERSION: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// The app's version for Google's user agent. The app sets it once at
/// startup, before any request; unset (tests, tools) reads "dev".
pub fn set_app_version(version: &'static str) {
    let _ = APP_VERSION.set(version);
}

/// `penguin/<version> (gzip)`: Google asks clients that want gzip to say so in the UA.
pub(crate) fn user_agent() -> String {
    format!("penguin/{} (gzip)", APP_VERSION.get().copied().unwrap_or("dev"))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("OAuth client not configured: {0}")]
    NotConfigured(String),
    #[error("account needs to sign in again: {0}")]
    NeedsReauth(String),
    #[error("oauth: {0}")]
    OAuth(String),
    /// The user closed the sign-in sheet or declined on Google's screen.
    #[error("sign-in cancelled")]
    SignInCancelled,
    #[error("keychain: {0}")]
    Keychain(String),
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    /// history.list returned 404 — startHistoryId too old; full resync required.
    #[error("gmail history expired")]
    HistoryExpired,
    #[error("rate limited")]
    RateLimited,
    #[error("network: {0}")]
    Network(String),
    #[error("store: {0}")]
    Store(#[from] penguin_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// What kind of problem this is when it stops a sync attempt (the same
    /// as the provider seam's `Error::sync_kind`).
    pub fn sync_kind(&self) -> penguin_core::SyncErrorKind {
        use penguin_core::SyncErrorKind as K;
        match self {
            Error::NeedsReauth(_) => K::Auth,
            Error::Keychain(_) => K::Keychain,
            Error::NotConfigured(_) => K::Config,
            Error::Network(_) => K::Network,
            Error::RateLimited | Error::Http { status: 429, .. } => K::RateLimited,
            Error::Http { status, .. } if *status >= 500 => K::Server,
            Error::Store(_) => K::Storage,
            _ => K::Other,
        }
    }
}

/// Gmail's errors as the provider seam's, variant for variant (same
/// messages), so the app's error codes and texts are what they were.
impl From<Error> for penguin_provider::Error {
    fn from(e: Error) -> Self {
        use penguin_provider::Error as P;
        match e {
            Error::NotConfigured(m) => P::NotConfigured(m),
            Error::NeedsReauth(m) => P::NeedsReauth(m),
            Error::OAuth(m) => P::OAuth(m),
            Error::SignInCancelled => P::SignInCancelled,
            Error::Keychain(m) => P::Keychain(m),
            Error::Http { status, body } => P::Http { status, body },
            // Internal to the sync engine; never reaches a provider caller.
            Error::HistoryExpired => P::Other(Error::HistoryExpired.to_string()),
            Error::RateLimited => P::RateLimited,
            Error::Network(m) => P::Network(m),
            Error::Store(e) => P::Store(e),
            Error::Other(m) => P::Other(m),
        }
    }
}

/// The shared modules (compose, outbox, snooze) report the provider seam's
/// errors; inside penguin-gmail they read as Gmail's.
impl From<penguin_provider::Error> for Error {
    fn from(e: penguin_provider::Error) -> Self {
        use penguin_provider::Error as P;
        match e {
            P::NotConfigured(m) => Error::NotConfigured(m),
            P::NeedsReauth(m) => Error::NeedsReauth(m),
            P::OAuth(m) => Error::OAuth(m),
            P::SignInCancelled => Error::SignInCancelled,
            P::Keychain(m) => Error::Keychain(m),
            P::Http { status, body } => Error::Http { status, body },
            P::RateLimited => Error::RateLimited,
            P::Network(m) => Error::Network(m),
            P::Store(e) => Error::Store(e),
            P::NotFound(m) | P::InvalidInput(m) | P::Unsupported(m) | P::Other(m) => {
                Error::Other(m)
            }
        }
    }
}

/// Text of a panic payload (`panic!("…")` yields `&str` or `String`).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn panic_message_reads_str_and_string_payloads() {
        let p = std::panic::catch_unwind(|| panic!("static text")).unwrap_err();
        assert_eq!(super::panic_message(&*p), "static text");
        let p = std::panic::catch_unwind(|| panic!("formatted {}", 42)).unwrap_err();
        assert_eq!(super::panic_message(&*p), "formatted 42");
        let p = std::panic::catch_unwind(|| std::panic::panic_any(7u8)).unwrap_err();
        assert_eq!(super::panic_message(&*p), "non-string panic payload");
    }
}
