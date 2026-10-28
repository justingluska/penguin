//! The two seams and one sync pass.
//!
//! - [`Transport`]: where records live remotely. Conditional writes
//!   (compare-and-swap on the server's change tag) and a change feed with
//!   an opaque token. CloudKit's private database has both natively
//!   (`recordChangeTag` + `serverRecordChanged`, zone change tokens); the
//!   in-memory server in [`crate::memory`] mimics it for tests.
//! - [`LocalStore`]: where records live on the device and how they turn
//!   into the app's own tables (penguin-core's `Store`, or
//!   [`crate::memory::MemoryStore`] in tests). It merges every remote
//!   record with [`crate::merge`] in the same transaction that updates the
//!   app's tables.
//!
//! [`sync_once`] is pull then push, for transports driven from Rust. With
//! CloudKit the Swift shim's CKSyncEngine delegate calls the same
//! [`LocalStore`] methods instead: fetched changes → `apply_remote`,
//! "next batch to send" → `pending`, sent → `saved` / `apply_remote` on a
//! `serverRecordChanged` conflict (with the server's record), fetch done →
//! `pull_completed`.

use crate::record::Record;
use crate::Result;

/// The server's version of a stored record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerVersion {
    /// Opaque change tag (CloudKit `recordChangeTag`); the base for the
    /// next conditional write.
    pub tag: String,
    /// When the server stored it (unix ms; CloudKit `modificationDate`).
    /// Bounds clock skew (see [`crate::Hlc::clamped`]).
    pub modified_ms: i64,
}

/// A record as the server has it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRecord {
    pub record: Record,
    pub version: ServerVersion,
}

/// A local change to send: the record, and the server tag it was based on
/// (None: the server has never had it, create only).
#[derive(Debug, Clone, PartialEq)]
pub struct Outgoing {
    pub record: Record,
    pub base_tag: Option<String>,
}

/// What one conditional write did.
#[derive(Debug, Clone, PartialEq)]
pub enum PushOutcome {
    Saved(ServerVersion),
    /// The server has a different version (another device wrote first).
    /// It comes back so the local store can merge and try again.
    Conflict(RemoteRecord),
}

/// One page of the change feed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pulled {
    pub records: Vec<RemoteRecord>,
    /// Record names removed on the server (old tombstones collected).
    pub deleted: Vec<String>,
    /// Where the next pull continues.
    pub token: String,
    /// More pages are waiting.
    pub more: bool,
}

/// What applying one remote record did locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The remote version won and the app's tables now reflect it.
    Took,
    /// The local version won; it stays queued to send.
    Kept,
    /// Both sides had the same version.
    Same,
}

/// Where records are stored remotely.
pub trait Transport {
    /// Changes since `token` (None: everything).
    fn pull(&mut self, token: Option<&str>) -> Result<Pulled>;
    /// Conditional writes, one outcome per record in order.
    fn push(&mut self, batch: &[Outgoing]) -> Result<Vec<PushOutcome>>;
}

/// Where records are stored on the device.
pub trait LocalStore {
    /// The change-feed token saved by the last complete pull.
    fn sync_token(&self) -> Result<Option<String>>;
    fn set_sync_token(&self, token: &str) -> Result<()>;
    /// Merge one remote record and, when it wins, apply it to the app's
    /// tables, in one transaction. Also advances the device clock past it.
    fn apply_remote(&self, remote: &RemoteRecord, now_ms: i64) -> Result<Applied>;
    /// A record name the server no longer has: forget it unless it has
    /// unsent local changes.
    fn forget(&self, name: &str) -> Result<()>;
    /// Local changes not yet on the server, oldest first, at most `limit`.
    fn pending(&self, limit: usize) -> Result<Vec<Outgoing>>;
    /// `sent` is stored on the server as `version`. If the local record
    /// changed since `sent` was read it stays queued (with the new base).
    fn saved(&self, sent: &Outgoing, version: &ServerVersion) -> Result<()>;
    /// A pull finished at `at_ms` with nothing left to fetch: this device
    /// has seen every change made before then. Gates snooze wakes.
    fn pull_completed(&self, at_ms: i64) -> Result<()>;
}

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub pulled: usize,
    pub took: usize,
    pub pushed: usize,
    pub conflicts: usize,
}

/// Records per push.
pub const PUSH_BATCH: usize = 200;
/// Push rounds per pass; a conflict re-queues the merged record for the
/// next round, so this bounds a pass when another device keeps writing.
const PUSH_ROUNDS: usize = 4;

/// One sync pass: pull every change, then push local changes, merging any
/// conflicts and pushing again (bounded). `now` gives the wall clock.
pub fn sync_once<S: LocalStore + ?Sized, T: Transport + ?Sized>(
    store: &S,
    transport: &mut T,
    now: &dyn Fn() -> i64,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    loop {
        let token = store.sync_token()?;
        let page = transport.pull(token.as_deref())?;
        for r in &page.records {
            report.pulled += 1;
            if store.apply_remote(r, now())? == Applied::Took {
                report.took += 1;
            }
        }
        for name in &page.deleted {
            store.forget(name)?;
        }
        store.set_sync_token(&page.token)?;
        if !page.more {
            break;
        }
    }
    store.pull_completed(now())?;

    for _ in 0..PUSH_ROUNDS {
        let batch = store.pending(PUSH_BATCH)?;
        if batch.is_empty() {
            break;
        }
        let outcomes = transport.push(&batch)?;
        if outcomes.len() != batch.len() {
            return Err(crate::Error::Unavailable(format!(
                "push answered {} of {} records",
                outcomes.len(),
                batch.len()
            )));
        }
        let mut conflicted = false;
        for (sent, outcome) in batch.iter().zip(outcomes) {
            match outcome {
                PushOutcome::Saved(v) => {
                    report.pushed += 1;
                    store.saved(sent, &v)?;
                }
                PushOutcome::Conflict(server) => {
                    report.conflicts += 1;
                    conflicted = true;
                    store.apply_remote(&server, now())?;
                }
            }
        }
        if !conflicted {
            break;
        }
    }
    Ok(report)
}
