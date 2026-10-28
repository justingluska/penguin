//! Cross-device state on the Mac: Sync with iCloud
//! (docs/CROSS-DEVICE-STATE.md).
//!
//! The Swift shim (swift/PenguinAI/Sources/PenguinAI/CloudSync.swift and
//! CloudSyncBridge.swift, built and linked by build.rs) runs CKSyncEngine
//! and calls back into [`host_callback`] for everything that touches the
//! database or the settings file; this module answers through penguin-core's
//! [`CloudHost`], the same host the iPhone uses. Here also:
//! - the settings hook: every local settings write keeps other devices'
//!   accounts (`keep_other_devices_accounts`), takes the synced value of a
//!   setting no longer kept to this Mac ("Only on this Mac", `keepHere`)
//!   and is recorded before the file is written; synced records that win
//!   are written back into settings.json and the UI hears
//!   `penguin://settings-changed`;
//! - the store's pending listener, which hands new records to the engine;
//! - Settings → General → Sync with iCloud (`cloud_status`,
//!   `set_cloud_sync`, `cloud_sync_now`) and `penguin://cloud-status`.
//!
//! The engine only starts in a build signed with an iCloud container (the
//! release build: entitlements and a Developer ID provisioning profile from
//! release.yml) on macOS 14 or later. Everywhere else (`tauri dev`,
//! self-builds, Linux) sync reports itself unavailable and the app works
//! alone, as before.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{Arc, OnceLock, Weak};

use base64::Engine as _;
use penguin_core::cloud::{CloudHost, ResetReason, SettingsFile, WireRecord, NOT_BUILT};
use penguin_core::Store;
use penguin_sync::{CloudAccount, CloudProblem, CloudStatus};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{CmdError, CmdResult};
use crate::ops::now_ms;
use crate::settings::Settings;
use crate::state::{blocking, AppState};

/// Payload: [`CloudStatus`].
pub const EVENT_CLOUD_STATUS: &str = "penguin://cloud-status";

pub struct Cloud {
    host: CloudHost,
    app: AppHandle,
}

/// The one instance, for the C callback (Swift holds no Rust pointer).
static CLOUD: OnceLock<Arc<Cloud>> = OnceLock::new();

/// The settings file as sync sees it.
struct MacSettings {
    app: AppHandle,
}

impl MacSettings {
    fn state(&self) -> Option<Arc<AppState>> {
        self.app
            .try_state::<Arc<AppState>>()
            .map(|s| s.inner().clone())
    }
}

impl SettingsFile for MacSettings {
    fn current(&self) -> Value {
        self.state()
            .and_then(|s| serde_json::to_value(s.settings.get()).ok())
            .unwrap_or(Value::Null)
    }

    fn defaults(&self) -> Value {
        serde_json::to_value(Settings::default()).unwrap_or(Value::Null)
    }

    fn apply_synced(&self, records: &[penguin_sync::Record]) {
        let Some(state) = self.state() else { return };
        match state.settings.apply_synced(records) {
            Ok(Some(saved)) => {
                tracing::info!(
                    records = records.len(),
                    "settings changed on another device"
                );
                tauri::async_runtime::spawn(async move {
                    match crate::commands::settings_view(&state, saved).await {
                        Ok(view) => state.emit_settings_changed(view),
                        Err(e) => tracing::warn!(error = %e, "synced settings not shown"),
                    }
                });
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "could not write synced settings"),
        }
    }
}

/// Every local settings write (SettingsState's hook): keep the accounts
/// this Mac doesn't have, take the synced value of a setting no longer
/// kept to this Mac, then record what changed.
fn on_settings_write(store: &Store, cloud: &Weak<Cloud>, old: &Settings, new: &mut Settings) {
    let (Ok(o), Ok(mut n)) = (serde_json::to_value(old), serde_json::to_value(&*new)) else {
        return;
    };
    match store.list_accounts() {
        Ok(accounts) => {
            let known: HashSet<String> = accounts.into_iter().map(|a| a.id).collect();
            penguin_sync::settings::keep_other_devices_accounts(&o, &mut n, &|a| known.contains(a));
        }
        Err(e) => tracing::warn!(error = %e, "accounts unreadable; settings write not checked"),
    }
    if let Some(cloud) = cloud.upgrade() {
        if let Err(e) = cloud.host.settings_write(&o, &mut n, now_ms()) {
            tracing::warn!(error = %e, "settings change not recorded for sync");
        }
    }
    match serde_json::from_value::<Settings>(n) {
        Ok(kept) => *new = kept,
        Err(e) => tracing::warn!(error = %e, "could not keep synced settings in the write"),
    }
}

/// Set up sync at launch (after the app state is managed): availability,
/// the settings hook, the settings file brought in line with the records,
/// the pending listener, then the engine if it should run.
pub fn start(app: &AppHandle, state: &Arc<AppState>) -> Arc<Cloud> {
    let cloud = Arc::new(Cloud {
        host: CloudHost::new(
            state.store.clone(),
            Box::new(MacSettings { app: app.clone() }),
            false,
        ),
        app: app.clone(),
    });
    let cloud = CLOUD.get_or_init(|| cloud).clone();
    let unavailable = bridge::unavailable();
    match &unavailable {
        None => tracing::info!("sync with iCloud: available"),
        Some(why) => tracing::info!(reason = %why, "sync with iCloud: unavailable"),
    }
    if let Err(e) = cloud.host.set_available(unavailable) {
        tracing::warn!(error = %e, "sync state unreadable");
    }
    let weak = Arc::downgrade(&cloud);
    let store = state.store.clone();
    state.settings.set_hook(Box::new(move |old, new| {
        on_settings_write(&store, &weak, old, new)
    }));
    if let Err(e) = cloud.host.reconcile_settings() {
        tracing::warn!(error = %e, "synced settings not reconciled");
    }
    state
        .store
        .set_sync_listener(Some(Arc::new(bridge::pending)));
    cloud.apply_switch();
    cloud
}

impl Cloud {
    /// Start or stop the engine to match the switch and the build.
    fn apply_switch(&self) {
        match self.host.should_run() {
            Ok(true) => bridge::start(),
            Ok(false) => bridge::stop(),
            Err(e) => tracing::warn!(error = %e, "sync switch unreadable"),
        }
    }

    pub fn status(&self) -> CmdResult<CloudStatus> {
        Ok(self.host.status()?)
    }

    fn emit_status(&self) {
        match self.host.status() {
            Ok(s) => {
                if let Err(e) = self.app.emit(EVENT_CLOUD_STATUS, s) {
                    tracing::warn!(error = %e, "failed to emit cloud status");
                }
            }
            Err(e) => tracing::warn!(error = %e, "sync status unreadable"),
        }
    }

    /// One operation from the Swift engine (see CloudSyncBridge.swift's
    /// `CloudOp`); the JSON answer, if it has one.
    fn handle(&self, op: i32, body: &[u8]) -> penguin_core::Result<Option<Value>> {
        let h = &self.host;
        let now = now_ms();
        let out = match op {
            op::ENGINE_STATE => {
                let state = h
                    .engine_state()?
                    .map(|b| base64::engine::general_purpose::STANDARD.encode(b));
                Some(json!({ "state": state }))
            }
            op::SAVE_ENGINE_STATE => {
                let r: StateBody = parse(body)?;
                if let Some(b64) = r.state {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| penguin_core::Error::Db(e.to_string()))?;
                    h.save_engine_state(&bytes)?;
                }
                None
            }
            op::APPLY => {
                let r: ApplyBody = parse(body)?;
                h.apply(&r.records, &r.deleted, now)?;
                None
            }
            op::FETCHED => {
                let r: FetchedBody = parse(body)?;
                h.fetched(r.started_ms, now)?;
                // A snooze wake waiting for this fetch can go now.
                crate::outbox::schedule_changed();
                None
            }
            op::PENDING_NAMES => {
                let r: LimitBody = parse(body)?;
                Some(json!({ "names": h.pending_names(r.limit)? }))
            }
            op::OUTGOING => {
                let r: NamesBody = parse(body)?;
                Some(json!({ "records": h.outgoing(&r.names)? }))
            }
            op::SAVED => {
                let r: RecordsBody = parse(body)?;
                h.saved(&r.records, now)?;
                // A claim to send a scheduled message may be confirmed now.
                crate::outbox::schedule_changed();
                None
            }
            op::CONFLICT => {
                let r: WireRecord = parse(body)?;
                Some(json!({ "value": h.conflict(&r, now)? }))
            }
            op::CLEAR_BASE => {
                let r: NamesBody = parse(body)?;
                h.clear_base(&r.names)?;
                None
            }
            op::RESET => {
                let r: ReasonBody = parse(body)?;
                h.reset(r.reason)?;
                None
            }
            op::REPORT_ACCOUNT => {
                let r: AccountBody = parse(body)?;
                Some(json!({ "value": h.report_account(r.account)? }))
            }
            op::REPORT_PROBLEM => {
                let r: ProblemBody = parse(body)?;
                h.report_problem(r.problem());
                None
            }
            op::SHOULD_RUN => Some(json!({ "value": h.should_run()? })),
            op::STATUS_CHANGED => {
                self.emit_status();
                None
            }
            other => {
                tracing::warn!(op = other, "unknown sync operation");
                None
            }
        };
        Ok(out)
    }
}

/// Operation numbers (CloudSyncBridge.swift `CloudOp`).
mod op {
    pub const ENGINE_STATE: i32 = 1;
    pub const SAVE_ENGINE_STATE: i32 = 2;
    pub const APPLY: i32 = 3;
    pub const FETCHED: i32 = 4;
    pub const PENDING_NAMES: i32 = 5;
    pub const OUTGOING: i32 = 6;
    pub const SAVED: i32 = 7;
    pub const CONFLICT: i32 = 8;
    pub const CLEAR_BASE: i32 = 9;
    pub const RESET: i32 = 10;
    pub const REPORT_ACCOUNT: i32 = 11;
    pub const REPORT_PROBLEM: i32 = 12;
    pub const SHOULD_RUN: i32 = 13;
    pub const STATUS_CHANGED: i32 = 14;
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> penguin_core::Result<T> {
    serde_json::from_slice(body).map_err(|e| penguin_core::Error::Db(format!("sync bridge: {e}")))
}

#[derive(Deserialize)]
struct StateBody {
    state: Option<String>,
}

#[derive(Deserialize)]
struct ApplyBody {
    records: Vec<WireRecord>,
    #[serde(default)]
    deleted: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchedBody {
    started_ms: i64,
}

#[derive(Deserialize)]
struct LimitBody {
    limit: usize,
}

#[derive(Deserialize)]
struct NamesBody {
    names: Vec<String>,
}

#[derive(Deserialize)]
struct RecordsBody {
    records: Vec<WireRecord>,
}

#[derive(Deserialize)]
struct ReasonBody {
    reason: ResetReason,
}

#[derive(Deserialize)]
struct AccountBody {
    account: CloudAccount,
}

#[derive(Deserialize)]
struct ProblemBody {
    kind: Option<String>,
    #[serde(default)]
    code: i64,
    #[serde(default)]
    detail: String,
}

impl ProblemBody {
    fn problem(self) -> Option<CloudProblem> {
        match self.kind.as_deref()? {
            "quotaExceeded" => Some(CloudProblem::QuotaExceeded),
            "notSetUp" => Some(CloudProblem::NotSetUp {
                detail: self.detail,
            }),
            _ => Some(CloudProblem::Other {
                code: self.code,
                detail: self.detail,
            }),
        }
    }
}

/// The C callback the Swift engine calls (CloudSyncBridge.swift
/// `PenguinCloudHostCallback`). Answers by calling `reply` before
/// returning. Never unwinds into Swift.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
extern "C" fn host_callback(
    _ctx: *mut c_void,
    op: i32,
    data: *const u8,
    len: isize,
    reply_ctx: *mut c_void,
    reply: bridge::Reply,
) {
    let body: &[u8] = if data.is_null() || len <= 0 {
        b"{}"
    } else {
        // SAFETY: Swift passes `len` readable bytes, valid for this call.
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let Some(cloud) = CLOUD.get() else { return };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cloud.handle(op, body)));
    let answer = match result {
        Ok(Ok(answer)) => answer,
        Ok(Err(e)) => {
            tracing::warn!(op, error = %e, "sync operation failed");
            None
        }
        Err(_) => {
            tracing::error!(op, "sync operation panicked");
            None
        }
    };
    if let Some(v) = answer {
        if let Ok(bytes) = serde_json::to_vec(&v) {
            // SAFETY: `reply` copies the bytes before it returns.
            reply(reply_ctx, bytes.as_ptr(), bytes.len() as isize);
        }
    }
}

/// Coming back to the app: fetch what other devices changed.
pub fn on_focus() {
    if CLOUD.get().is_some() {
        bridge::fetch();
    }
}

/// A snooze is due and its wake waits for a fresh fetch.
pub fn fetch_for_wake() {
    bridge::fetch();
}

// ---------- commands ----------

#[tauri::command]
pub async fn cloud_status(cloud: State<'_, Arc<Cloud>>) -> CmdResult<CloudStatus> {
    let c = cloud.inner().clone();
    blocking(move || c.status()).await
}

/// Settings → Sync with iCloud. Off keeps every record where it is; on
/// again sends what changed meanwhile.
#[tauri::command]
pub async fn set_cloud_sync(cloud: State<'_, Arc<Cloud>>, enabled: bool) -> CmdResult<CloudStatus> {
    let c = cloud.inner().clone();
    let status = blocking(move || {
        c.host.set_wanted(enabled)?;
        c.apply_switch();
        c.status()
    })
    .await?;
    tracing::info!(enabled, "sync with iCloud switched");
    cloud.emit_status();
    Ok(status)
}

/// Settings → Sync now: fetch and send at once.
#[tauri::command]
pub async fn cloud_sync_now(cloud: State<'_, Arc<Cloud>>) -> CmdResult<CloudStatus> {
    let c = cloud.inner().clone();
    let status = blocking(move || c.status()).await?;
    if !status.available {
        return Err(CmdError::invalid(
            status
                .unavailable_reason
                .unwrap_or_else(|| NOT_BUILT.to_string()),
        ));
    }
    bridge::check_account();
    bridge::fetch();
    bridge::send();
    Ok(status)
}

// ---------- the Swift bridge ----------

#[cfg(target_os = "macos")]
mod bridge {
    use std::ffi::c_void;

    pub type Reply = extern "C" fn(ctx: *mut c_void, data: *const u8, len: isize);
    type Host = extern "C" fn(
        ctx: *mut c_void,
        op: i32,
        data: *const u8,
        len: isize,
        reply_ctx: *mut c_void,
        reply: Reply,
    );

    extern "C" {
        fn penguin_cloud_availability() -> i32;
        fn penguin_cloud_start(ctx: *mut c_void, host: Host);
        fn penguin_cloud_stop();
        fn penguin_cloud_pending();
        fn penguin_cloud_fetch();
        fn penguin_cloud_send();
        fn penguin_cloud_check_account();
    }

    /// None when sync can run on this Mac; otherwise why not.
    pub fn unavailable() -> Option<String> {
        // SAFETY: no arguments; reads the OS version and this app's
        // entitlements.
        match unsafe { penguin_cloud_availability() } {
            0 => None,
            1 => Some("Sync with iCloud needs macOS 14 or later.".into()),
            _ => Some(super::NOT_BUILT.into()),
        }
    }

    pub fn start() {
        // SAFETY: the callback is a plain function; Swift keeps it for the
        // engine's lifetime and passes the (unused) null context back.
        unsafe { penguin_cloud_start(std::ptr::null_mut(), super::host_callback) }
    }

    pub fn stop() {
        // SAFETY: no arguments; a no-op when not running.
        unsafe { penguin_cloud_stop() }
    }

    pub fn pending() {
        // SAFETY: no arguments; Swift hops to its own queue at once.
        unsafe { penguin_cloud_pending() }
    }

    pub fn fetch() {
        // SAFETY: as above.
        unsafe { penguin_cloud_fetch() }
    }

    pub fn send() {
        // SAFETY: as above.
        unsafe { penguin_cloud_send() }
    }

    pub fn check_account() {
        // SAFETY: as above.
        unsafe { penguin_cloud_check_account() }
    }
}

/// Not a Mac: no CloudKit, sync is never available.
#[cfg(not(target_os = "macos"))]
mod bridge {
    use std::ffi::c_void;

    pub type Reply = extern "C" fn(ctx: *mut c_void, data: *const u8, len: isize);

    pub fn unavailable() -> Option<String> {
        Some(super::NOT_BUILT.into())
    }
    pub fn start() {}
    pub fn stop() {}
    pub fn pending() {}
    pub fn fetch() {}
    pub fn send() {}
    pub fn check_account() {}
}
