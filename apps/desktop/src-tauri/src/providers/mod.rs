//! Mail providers as the Add account flow sees them: which provider hosts an
//! address, how Penguin signs in to it, and the provider-agnostic contract the
//! Microsoft (Graph) and IMAP providers implement.
//!
//! - `detect_provider(email, choose)`: detection, shared with the iPhone
//!   (penguin-onboard's `Detector`). Built-in domain table → MX lookup →
//!   autoconfig (the domain's own, then Thunderbird's ISPDB) → unknown.
//!   Never sends the address anywhere, only the domain.
//! - `microsoft_client_status` / `set_microsoft_client`: the user's own
//!   Microsoft Entra app registration (ms_client.rs), like the Google client.
//! - `connect_account` (connect.rs): signs in a Microsoft or IMAP account
//!   from a `ConnectAccountRequest` (checked by penguin-onboard), dispatched
//!   by auth method.
//! - `registry`: which backend serves an account (every mailbox call goes
//!   through it), and `install_backends`, where each provider registers.
//!
//! Mirrored in `apps/desktop/src/lib/types.ts` (DetectedProvider,
//! ProviderKind, AuthMethod, SetupKind, DetectionSource, ServerSettings,
//! MailSecurity, ConnectAccountRequest, MicrosoftClientStatus): change both
//! together. The types live in penguin-onboard; `AuthMethod`,
//! `ServerSettings` and `MailSecurity` in penguin-core (they're also part of
//! `Account.providerConfig`).

pub mod connect;
pub mod ms_client;
pub mod registry;

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::error::{CmdError, CmdResult};
use crate::state::{blocking, AppState};

pub use connect::connect_account;
pub use penguin_onboard::{
    provider_for, AuthMethod, ConnectAccountRequest, DetectedProvider, DetectionSource, Detector,
    MailSecurity, ProviderKind, ProviderNet, ServerSettings, SetupKind, SystemNet,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftClientStatus {
    /// The saved Application (client) ID; null until the user adds one.
    pub client_id: Option<String>,
    /// Where it is saved (for the setup screen).
    pub path: String,
}

/// Whether this build can sign in with `auth`. Each provider flips its own
/// method on when it lands (Google is the only one today).
pub fn connectable(auth: AuthMethod) -> bool {
    matches!(
        auth,
        AuthMethod::GoogleOAuth
            | AuthMethod::MicrosoftOAuth
            | AuthMethod::AppPassword
            | AuthMethod::ImapPassword
    )
}

/// Register the non-Gmail backends at startup (Gmail's is built when a
/// Google client is configured: `AppState::install_services`). Each
/// provider adds one line here, e.g.
/// `state.register_backend(Arc::new(penguin_imap::ImapBackend::new(state.store.clone(), state.sync_observer())));`
/// Until then their accounts show "not implemented yet" and never sync.
pub fn install_backends(state: &Arc<AppState>) {
    state.register_backend(Arc::new(penguin_imap::ImapBackend::new(
        state.store.clone(),
        state.sync_observer(),
    )));
    match penguin_graph::GraphBackend::new(state.store.clone(), state.sync_observer()) {
        Ok(b) => state.register_backend(Arc::new(b)),
        Err(e) => {
            tracing::error!(error = %e, "Microsoft backend unavailable: its store tables couldn't be created")
        }
    }
}

/// Managed state: the detector with its per-session domain cache.
pub struct Providers {
    pub detector: Detector,
}

pub fn init() -> Arc<Providers> {
    Arc::new(Providers {
        detector: Detector::new(Arc::new(SystemNet::new()))
            .with_available(|auth, _| connectable(auth)),
    })
}

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Who hosts `email` and how to sign in. With `choose` (a manual pick in
/// "Choose provider"), that provider's settings for the address, no network.
#[tauri::command]
pub async fn detect_provider(
    providers: State<'_, Arc<Providers>>,
    email: String,
    choose: Option<SetupKind>,
) -> CmdResult<DetectedProvider> {
    let found = providers
        .detector
        .detect(&email, choose)
        .await
        .map_err(CmdError::invalid)?;
    // Kind and source only: the address and domain stay out of the log.
    tracing::info!(kind = ?found.kind, source = ?found.source, offline = found.offline, "provider detected");
    Ok(found)
}

fn ms_status(state: &AppState) -> MicrosoftClientStatus {
    MicrosoftClientStatus {
        client_id: ms_client::load(&state.paths.config_dir),
        path: ms_client::path(&state.paths.config_dir)
            .display()
            .to_string(),
    }
}

#[tauri::command]
pub async fn microsoft_client_status(state: AppStateRef<'_>) -> CmdResult<MicrosoftClientStatus> {
    Ok(ms_status(&state))
}

/// Save (or with null, remove) the Application (client) ID of the user's own
/// Entra app registration. Accepts the bare GUID or a pasted line containing
/// it; anything else is invalidInput.
#[tauri::command]
pub async fn set_microsoft_client(
    state: AppStateRef<'_>,
    client_id: Option<String>,
) -> CmdResult<MicrosoftClientStatus> {
    let id = match client_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(penguin_onboard::normalize_client_id(raw).map_err(CmdError::invalid)?),
    };
    let dir = state.paths.config_dir.clone();
    let saved = id.clone();
    blocking(move || {
        ms_client::save(&dir, saved.as_deref())
            .map_err(|e| CmdError::other(format!("Couldn't save the Microsoft client: {e}")))
    })
    .await?;
    tracing::info!(configured = id.is_some(), "Microsoft client ID updated");
    Ok(ms_status(&state))
}
