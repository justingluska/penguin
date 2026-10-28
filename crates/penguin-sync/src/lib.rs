//! penguin-sync: the shared core of cross-device state (Mac ⇄ iPhone).
//!
//! Design and the reasons for it: docs/CROSS-DEVICE-STATE.md. In short,
//! each piece of synced state (a snooze, a Follow up dismissal, a setting,
//! a snippet, …) is one small [`Record`] stamped with hybrid logical clocks
//! ([`Hlc`]). Records merge by a total order ([`merge`]): the newest user
//! intent wins (`gen`), and within one intent the state only moves forward
//! (`phase`: snoozed → woken). That order is a join-semilattice, so every
//! device that has seen the same records agrees, whatever order they came
//! in, without a server deciding anything.
//!
//! - [`hlc`]: clocks, with skew bounds.
//! - [`record`]: the record model, kinds, bodies and the merge rule.
//! - [`engine`]: the [`Transport`] (where records are stored remotely) and
//!   [`LocalStore`] (where they are stored on the device) seams, and
//!   [`sync_once`], one pull-then-push pass for transports that are driven
//!   from Rust. CloudKit's CKSyncEngine drives itself; its Swift shim calls
//!   the same [`LocalStore`] methods from its delegate.
//! - [`memory`]: an in-memory server, transport and local store for tests.
//! - [`settings`]: which settings sync and how a settings JSON diff becomes
//!   records.
//! - [`outbox`]: send later and reminders across devices (exactly one
//!   device sends a scheduled message).
//! - [`status`]: what Settings shows about sync, the same on both devices.
//!
//! Pure Rust: no SQLite (penguin-core implements [`LocalStore`]), no
//! networking, no Apple frameworks, so the iPhone gets it through UniFFI
//! unchanged.

pub mod engine;
pub mod hlc;
pub mod memory;
pub mod outbox;
pub mod record;
pub mod settings;
pub mod status;

pub use engine::{
    sync_once, Applied, LocalStore, Outgoing, Pulled, PushOutcome, RemoteRecord, ServerVersion,
    SyncReport, Transport,
};
pub use hlc::{Clock, Hlc, MAX_SKEW_MS};
pub use record::{
    merge, record_name, split_thread_key, thread_key, DismissalBody, Kind, Record, SnoozeBody,
    SnoozeState, Winner,
};
pub use status::{CloudAccount, CloudProblem, CloudStatus, Report};

/// Errors from a transport or a local store. Kept as text: the two sides
/// have their own error types (CloudKit's, SQLite's) and the engine only
/// needs to stop the pass and report.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// The transport could not be reached (offline, signed out of iCloud,
    /// rate limited). The pass stops; nothing local is lost.
    #[error("sync transport unavailable: {0}")]
    Unavailable(String),
    /// The local store failed.
    #[error("sync store: {0}")]
    Store(String),
    /// A record that could not be read (a newer app's format, damage).
    #[error("sync record: {0}")]
    Record(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
