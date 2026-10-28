//! The device side of the CloudKit transport (docs/CROSS-DEVICE-STATE.md,
//! "Moving records: CKSyncEngine"), shared by the Mac and the iPhone.
//!
//! Each device runs a small Swift shim around CKSyncEngine. The shim moves
//! records and reports what CloudKit says; it never looks inside a record.
//! Everything else is here, in [`CloudHost`]: merging what was fetched
//! (through the store's [`LocalStore`]), carrying it into the settings
//! file, choosing what to send, recording what was sent, starting over
//! when the iCloud account or the zone changes, the user's "Sync with
//! iCloud" switch and what Settings shows ([`CloudStatus`]).
//!
//! Records cross the shim as [`WireRecord`]s: the record name, the record
//! JSON (stored in CloudKit as one encrypted field), CloudKit's change tag
//! (its archived system fields, opaque here) and the server's modification
//! time.
//!
//! The settings file belongs to the app (the Mac's `Settings`, the
//! phone's `CoreSettings`), so it takes part through [`SettingsFile`]:
//! local writes are recorded with [`CloudHost::record_settings_change`]
//! before the file is written, and records that win a merge are written
//! into the file with [`SettingsFile::apply_synced`].
//!
//! What stays on each device and what syncs is per key
//! (`penguin_sync::settings::SYNCED_SETTINGS`): swipes, appearance and
//! what this machine runs stay local.

use std::sync::Mutex;

use penguin_sync::settings::{self, is_settings_kind};
use penguin_sync::{
    Applied, CloudAccount, CloudProblem, CloudStatus, LocalStore, Outgoing, Record, RemoteRecord,
    Report, ServerVersion,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result, Store};

/// A record as the server stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireRecord {
    /// The record's name (`penguin_sync::record_name`).
    pub name: String,
    /// The record, as JSON.
    pub payload: String,
    /// The server's change tag for this version; opaque.
    pub tag: String,
    /// When the server stored it (unix ms).
    pub modified_ms: i64,
}

/// A record to send, with the server version it is based on (None: the
/// server never had it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireOutgoing {
    pub name: String,
    pub payload: String,
    #[serde(default)]
    pub base_tag: Option<String>,
}

/// Why the server's copy is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetReason {
    /// Signed out of iCloud.
    SignedOut,
    /// Signed in to a different iCloud account.
    SwitchedAccounts,
    /// The zone was deleted (by another device starting over).
    ZoneDeleted,
    /// The zone's encryption keys were reset (iCloud Keychain reset): its
    /// records can't be read any more.
    ZoneReset,
    /// The user deleted Penguin's data in iCloud settings.
    Purged,
}

/// The app's settings file, as sync sees it.
pub trait SettingsFile: Send + Sync {
    /// The settings as JSON (the app's camelCase keys).
    fn current(&self) -> Value;
    /// A new install's settings as JSON.
    fn defaults(&self) -> Value;
    /// Write records that won a merge (settings, snippets, signatures)
    /// into the file, under the settings' write lock: usually
    /// [`apply_settings_records`] on the current settings as JSON with
    /// "this app can read it" as the check, then read back, normalized like
    /// any other write, saved without being recorded again, and the UI
    /// told. Nothing is written when nothing changed.
    fn apply_synced(&self, records: &[Record]);
}

/// Apply `records` to `settings` one at a time, keeping each only if
/// `readable` accepts the result: a value this build can't read (a newer
/// app's) is skipped instead of sinking the whole file. Returns whether
/// anything changed.
pub fn apply_settings_records(
    settings: &mut Value,
    records: &[Record],
    readable: impl Fn(&Value) -> bool,
) -> bool {
    let mut changed = false;
    for r in records {
        let mut next = settings.clone();
        if settings::apply_records(&mut next, std::slice::from_ref(r)) {
            if readable(&next) {
                *settings = next;
                changed = true;
            } else {
                tracing::warn!(
                    kind = r.kind.tag(),
                    "synced setting unreadable here; skipped"
                );
            }
        }
    }
    changed
}

/// What a build without a usable transport says, until the shim reports.
pub const NOT_BUILT: &str = "This build of Penguin has no iCloud container.";

/// See the module docs.
pub struct CloudHost {
    store: Store,
    settings: Box<dyn SettingsFile>,
    /// This app keeps only part of some settings (the iPhone).
    partial: bool,
    report: Mutex<Report>,
    /// None: the shim can run here; Some(reason): it can't.
    unavailable: Mutex<Option<String>>,
}

fn sync_err(e: penguin_sync::Error) -> Error {
    Error::Db(e.to_string())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl CloudHost {
    pub fn new(store: Store, settings: Box<dyn SettingsFile>, partial: bool) -> CloudHost {
        CloudHost {
            store,
            settings,
            partial,
            report: Mutex::default(),
            unavailable: Mutex::new(Some(NOT_BUILT.to_string())),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    // ---- the switch and the status ----

    /// Whether the shim can run on this device (None) or why not.
    pub fn set_available(&self, unavailable: Option<String>) -> Result<bool> {
        *lock(&self.unavailable) = unavailable;
        self.refresh()
    }

    /// "Sync with iCloud", on unless turned off.
    pub fn wanted(&self) -> Result<bool> {
        self.store.sync_wanted()
    }

    /// Turn sync on or off. Off keeps every record here (and on iCloud);
    /// on again sends what changed meanwhile.
    pub fn set_wanted(&self, on: bool) -> Result<bool> {
        self.store.set_sync_wanted(on)?;
        if on {
            let mut r = lock(&self.report);
            if r.problem == Some(CloudProblem::DataDeleted) {
                r.problem = None;
            }
        }
        self.refresh()
    }

    /// Whether the shim should run its engine: wanted and possible here.
    /// (Whether records move also needs an iCloud account, which only the
    /// running engine finds out.)
    pub fn should_run(&self) -> Result<bool> {
        Ok(lock(&self.unavailable).is_none() && self.wanted()?)
    }

    /// Records move (and snooze wakes wait for a fresh pull) only while
    /// the engine runs with an iCloud account; otherwise the device works
    /// alone, as before sync.
    fn refresh(&self) -> Result<bool> {
        let account = lock(&self.report).account;
        let active = self.should_run()? && account == CloudAccount::Available;
        if self.store.sync_enabled()? != active {
            self.store.set_sync_enabled(active)?;
        }
        Ok(active)
    }

    /// The iCloud account the engine sees. Returns whether records move.
    pub fn report_account(&self, account: CloudAccount) -> Result<bool> {
        lock(&self.report).account = account;
        self.refresh()
    }

    /// A problem to show (None clears it).
    pub fn report_problem(&self, problem: Option<CloudProblem>) {
        lock(&self.report).problem = problem;
    }

    pub fn status(&self) -> Result<CloudStatus> {
        let unavailable = lock(&self.unavailable).clone();
        let report = lock(&self.report).clone();
        let enabled = self.wanted()?;
        let (pull, send) = self.store.sync_times()?;
        let running = unavailable.is_none() && enabled;
        Ok(CloudStatus {
            available: unavailable.is_none(),
            unavailable_reason: unavailable,
            enabled,
            active: running && report.account == CloudAccount::Available,
            account: if running {
                report.account
            } else {
                CloudAccount::Unknown
            },
            last_sync_at: pull.max(send),
            pending: self.store.sync_pending_count()?,
            problem: report.problem,
        })
    }

    // ---- the engine ----

    pub fn engine_state(&self) -> Result<Option<Vec<u8>>> {
        self.store.sync_engine_state()
    }

    pub fn save_engine_state(&self, state: &[u8]) -> Result<()> {
        self.store.set_sync_engine_state(Some(state))
    }

    /// Fetched changes: merge each record (projecting the winners onto the
    /// app's tables and, for settings, the settings file) and forget
    /// records the server deleted.
    pub fn apply(&self, records: &[WireRecord], deleted: &[String], now_ms: i64) -> Result<()> {
        let mut settings_won = Vec::new();
        for w in records {
            if let Some((record, applied)) = self.apply_one(w, now_ms)? {
                if applied == Applied::Took && is_settings_kind(record.kind) {
                    settings_won.push(record);
                }
            }
        }
        for name in deleted {
            self.store.forget(name).map_err(sync_err)?;
        }
        if !settings_won.is_empty() {
            self.project_settings(&settings_won);
        }
        Ok(())
    }

    fn apply_one(&self, w: &WireRecord, now_ms: i64) -> Result<Option<(Record, Applied)>> {
        let record: Record = match serde_json::from_str(&w.payload) {
            Ok(r) => r,
            Err(e) => {
                // A newer app's format, or damage: leave it on the server.
                tracing::warn!(error = %e, "synced record unreadable; skipped");
                return Ok(None);
            }
        };
        if record.name() != w.name {
            tracing::warn!("synced record's name doesn't match its contents; skipped");
            return Ok(None);
        }
        let remote = RemoteRecord {
            record,
            version: ServerVersion {
                tag: w.tag.clone(),
                modified_ms: w.modified_ms,
            },
        };
        let applied = self.store.apply_remote(&remote, now_ms).map_err(sync_err)?;
        Ok(Some((remote.record, applied)))
    }

    /// A fetch that started at `started_ms` finished with nothing left:
    /// this device has seen every change made before then. The first time,
    /// its settings are carried over (what has no record yet).
    pub fn fetched(&self, started_ms: i64, now_ms: i64) -> Result<()> {
        self.store.pull_completed(started_ms).map_err(sync_err)?;
        if !self.store.sync_settings_seeded()? {
            let changes =
                settings::seed_changes(&self.settings.current(), &self.settings.defaults());
            let n = self.store.sync_seed_settings(&changes, now_ms)?;
            tracing::info!(settings = n, "settings carried over to sync");
        }
        Ok(())
    }

    /// Names of records waiting to be sent.
    pub fn pending_names(&self, limit: usize) -> Result<Vec<String>> {
        self.store.sync_pending_names(limit)
    }

    /// The records among `names` still waiting to be sent.
    pub fn outgoing(&self, names: &[String]) -> Result<Vec<WireOutgoing>> {
        self.store
            .sync_outgoing(names)?
            .into_iter()
            .map(|o| {
                Ok(WireOutgoing {
                    name: o.record.name(),
                    payload: serde_json::to_string(&o.record)
                        .map_err(|e| Error::Db(e.to_string()))?,
                    base_tag: o.base_tag,
                })
            })
            .collect()
    }

    /// Records the server stored, as it stored them.
    pub fn saved(&self, records: &[WireRecord], now_ms: i64) -> Result<()> {
        for w in records {
            let record: Record = match serde_json::from_str(&w.payload) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "saved record unreadable");
                    continue;
                }
            };
            let sent = Outgoing {
                record,
                base_tag: None,
            };
            let version = ServerVersion {
                tag: w.tag.clone(),
                modified_ms: w.modified_ms,
            };
            self.store.saved(&sent, &version).map_err(sync_err)?;
        }
        if !records.is_empty() {
            self.store.set_sync_last_send(now_ms)?;
        }
        Ok(())
    }

    /// A save lost to `server` (another device wrote first): merge it.
    /// Returns whether this device's version won and must be sent again.
    pub fn conflict(&self, server: &WireRecord, now_ms: i64) -> Result<bool> {
        self.apply(std::slice::from_ref(server), &[], now_ms)?;
        Ok(!self.store.sync_outgoing(&[server.name.clone()])?.is_empty())
    }

    /// The server lost these records: send them again as new.
    pub fn clear_base(&self, names: &[String]) -> Result<()> {
        self.store.sync_clear_base(names)
    }

    /// The server's copy is gone (see [`ResetReason`]): everything here is
    /// sent again. When the user deleted Penguin's data from iCloud, sync
    /// turns itself off instead of putting it straight back.
    pub fn reset(&self, reason: ResetReason) -> Result<()> {
        tracing::info!(?reason, "sync starts over");
        self.store.sync_reset_server()?;
        if reason == ResetReason::Purged {
            self.store.set_sync_wanted(false)?;
            lock(&self.report).problem = Some(CloudProblem::DataDeleted);
        }
        self.refresh().map(|_| ())
    }

    // ---- the settings file ----

    /// Record a local write of the settings file, before the file is
    /// written (from `old` to `new`, both as JSON).
    pub fn record_settings_change(&self, old: &Value, new: &Value, now_ms: i64) -> Result<()> {
        let changes = settings::changes(old, new);
        self.store
            .sync_record_settings(&changes, self.partial, now_ms)
    }

    /// Every local write of the settings file, before it is written:
    /// settings this device just stopped keeping to itself take the synced
    /// value again (written into `new`), then what changed is recorded.
    pub fn settings_write(&self, old: &Value, new: &mut Value, now_ms: i64) -> Result<()> {
        let unkept = settings::unkept(old, new);
        if !unkept.is_empty() {
            let records = self.store.sync_records(penguin_sync::Kind::Setting)?;
            for key in unkept {
                let synced = records
                    .iter()
                    .find(|r| r.key == key)
                    .and_then(|r| r.body.clone());
                if let (Some(v), Some(obj)) = (synced, new.as_object_mut()) {
                    obj.insert(key, v);
                }
            }
        }
        self.record_settings_change(old, new, now_ms)
    }

    /// Bring the settings file in line with the records (at launch: a
    /// record that won just before the app quit may not have reached the
    /// file).
    pub fn reconcile_settings(&self) -> Result<()> {
        let records = self.store.sync_settings_records()?;
        if !records.is_empty() {
            self.project_settings(&records);
        }
        Ok(())
    }

    fn project_settings(&self, records: &[Record]) {
        self.settings.apply_synced(records);
    }
}

#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
