//! Self-update: Penguin → Check for Updates…, plus a quiet check shortly
//! after launch and every few hours.
//!
//! Source builds have no update channel: tauri.conf.json carries no
//! `plugins.updater`, so the plugin isn't registered, nothing checks in the
//! background, and Check for Updates… says updates aren't set up for this
//! build. A release build gets its channel at build time:
//! `.github/workflows/release.yml` passes `plugins.updater.endpoints` and
//! `plugins.updater.pubkey` through `tauri build --config`, signs and
//! notarizes the app, signs the `.app.tar.gz` with the matching updater key
//! and publishes it with `latest.json`. The plugin refuses a bundle whose
//! signature doesn't verify.
//!
//! Flow: check → download and install in the background (on macOS this
//! swaps the .app bundle on disk; the running process is untouched) →
//! `penguin://update {state: "ready"}` → the UI's "Restart" calls
//! `restart_to_update`. Nothing restarts without the user. The UI gets no
//! updater plugin API: only this event and that one command.
//!
//! Debug builds never check on their own (a dev build is not what's
//! installed); the menu item still works there, for testing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;

use crate::error::{CmdError, CmdResult};

/// Does this build's config name an update channel (at least one endpoint
/// and a public key)? Without one the updater plugin can't even be
/// registered: its config requires `pubkey`.
pub fn configured(plugins: &tauri::utils::config::PluginConfig) -> bool {
    let Some(u) = plugins.0.get("updater") else {
        return false;
    };
    let has_key = u
        .get("pubkey")
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.trim().is_empty());
    let has_endpoint = u
        .get("endpoints")
        .and_then(|e| e.as_array())
        .is_some_and(|e| !e.is_empty());
    has_key && has_endpoint
}

/// Payload: [`UpdateEvent`].
pub const EVENT_UPDATE: &str = "penguin://update";

const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEvent {
    /// checking | downloading | ready | upToDate | error | unconfigured
    pub state: &'static str,
    /// From the menu: say so even when there's nothing new, or it failed.
    pub manual: bool,
    pub current: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub message: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    /// This build has an update channel ([`configured`]).
    configured: bool,
    /// A check or download is running; a second one waits for it.
    busy: AtomicBool,
    /// A new version is installed on disk and needs a restart.
    ready: AtomicBool,
}

pub fn init<R: Runtime>(app: &AppHandle<R>, configured: bool) -> Arc<Updates> {
    let updates = Arc::new(Updates {
        configured,
        ..Updates::default()
    });
    app.manage(updates.clone());
    if !configured {
        tracing::info!("no update channel in this build; update checks are off");
    }
    if configured && !cfg!(debug_assertions) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_CHECK).await;
            loop {
                check(&app, false).await;
                tokio::time::sleep(EVERY).await;
            }
        });
    }
    updates
}

impl UpdateEvent {
    fn new(state: &'static str, manual: bool, current: &str) -> Self {
        UpdateEvent {
            state,
            manual,
            current: current.to_string(),
            version: None,
            notes: None,
            message: None,
        }
    }
}

fn event<R: Runtime>(app: &AppHandle<R>, e: UpdateEvent) {
    if let Err(err) = app.emit_to("main", EVENT_UPDATE, e) {
        tracing::warn!(error = %err, "could not send update status to the UI");
    }
}

/// Check, and when there's something new, download and install it. Quiet
/// (automatic) checks only speak up when an update is ready.
pub async fn check<R: Runtime>(app: &AppHandle<R>, manual: bool) {
    let Some(updates) = app.try_state::<Arc<Updates>>().map(|s| s.inner().clone()) else {
        return;
    };
    let current = app.package_info().version.to_string();
    let base = |state: &'static str| UpdateEvent::new(state, manual, &current);
    if !updates.configured {
        if manual {
            event(app, base("unconfigured"));
        }
        return;
    }
    if updates.ready.load(Ordering::SeqCst) {
        // Already installed; only the restart is left. Say so again.
        if manual {
            event(app, base("ready"));
        }
        return;
    }
    if updates.busy.swap(true, Ordering::SeqCst) {
        if manual {
            event(app, base("checking"));
        }
        return;
    }
    if manual {
        event(app, base("checking"));
    }
    let result = run(app, manual, &current).await;
    updates.busy.store(false, Ordering::SeqCst);
    match result {
        Ok(Some(ready)) => {
            updates.ready.store(true, Ordering::SeqCst);
            event(app, ready);
        }
        Ok(None) => {
            if manual {
                event(app, base("upToDate"));
            }
        }
        Err(message) => {
            tracing::warn!(error = %message, manual, "update check failed");
            if manual {
                event(
                    app,
                    UpdateEvent {
                        message: Some(message),
                        ..base("error")
                    },
                );
            }
        }
    }
}

async fn run<R: Runtime>(
    app: &AppHandle<R>,
    manual: bool,
    current: &str,
) -> Result<Option<UpdateEvent>, String> {
    let base = |state: &'static str| UpdateEvent::new(state, manual, current);
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    tracing::info!(version = %update.version, "update found; downloading");
    if manual {
        event(
            app,
            UpdateEvent {
                version: Some(update.version.clone()),
                ..base("downloading")
            },
        );
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(version = %update.version, "update installed; waiting for restart");
    Ok(Some(UpdateEvent {
        version: Some(update.version.clone()),
        notes: update.body.clone().filter(|b| !b.trim().is_empty()),
        ..base("ready")
    }))
}

/// Settings → What's new → Check for updates: same as the menu item; the
/// answer arrives as `penguin://update` events.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> CmdResult<()> {
    check(&app, true).await;
    Ok(())
}

/// The "Restart" on the update-ready toast.
#[tauri::command]
pub fn restart_to_update(app: AppHandle) -> CmdResult<()> {
    let ready = app
        .try_state::<Arc<Updates>>()
        .is_some_and(|u| u.ready.load(Ordering::SeqCst));
    if !ready {
        return Err(CmdError::invalid("no update is waiting to be installed"));
    }
    tracing::info!("restarting into the installed update");
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::configured;

    fn config(plugins: serde_json::Value) -> tauri::utils::config::PluginConfig {
        serde_json::from_value(plugins).unwrap()
    }

    #[test]
    fn an_update_channel_needs_an_endpoint_and_a_key() {
        assert!(!configured(&config(serde_json::json!({}))));
        assert!(!configured(&config(
            serde_json::json!({"updater": {"endpoints": ["https://u.example/latest.json"]}})
        )));
        assert!(!configured(&config(
            serde_json::json!({"updater": {"pubkey": "k", "endpoints": []}})
        )));
        assert!(!configured(&config(
            serde_json::json!({"updater": {"pubkey": " ", "endpoints": ["https://u.example/latest.json"]}})
        )));
        assert!(configured(&config(
            serde_json::json!({"updater": {"pubkey": "k", "endpoints": ["https://u.example/latest.json"]}})
        )));
    }

    #[test]
    fn the_source_tree_ships_without_one() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert!(
            conf.pointer("/plugins/updater").is_none(),
            "the update channel is injected by the release workflow, not committed"
        );
    }
}
