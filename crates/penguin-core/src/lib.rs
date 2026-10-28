//! penguin-core: provider-agnostic domain types, local store, and search.
//! No networking and no Tauri here: the store and search stay independent of
//! the app shell.

pub mod ask;
pub mod cloud;
pub mod compose_prefs;
mod dates;
pub mod diagnostics;
pub mod hybrid;
mod lexicon;
pub mod notify;
#[cfg(test)]
mod hybrid_tests;
pub mod otp;
pub mod profiles;
pub mod query;
pub mod query_gmail;
pub mod receipts;
pub mod remote_images;
pub mod snippets;
pub mod splits;
mod search;
#[cfg(feature = "sql-profile")]
#[doc(hidden)]
pub mod sqlprof;
pub mod store;
#[cfg(test)]
mod store_tests;
pub mod structured;
pub mod summary;
pub mod sync_health;
pub mod text;
pub mod types;
pub mod unsubscribe;
pub mod when;
pub mod writing;
pub mod zero;

/// The SQLite binding the store uses, for provider-owned tables
/// ([`Store::provider_read`] / [`Store::provider_write`]) at the same version.
pub use rusqlite;
pub use hybrid::{HybridParams, SemanticHandles};
pub use store::Store;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid query: {0}")]
    InvalidQuery(String),
}

pub type Result<T> = std::result::Result<T, Error>;
