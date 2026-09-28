//! Mail providers as the Add account flow sees them: which provider hosts an
//! address, how Penguin signs in to it, and the provider-agnostic contract the
//! Microsoft (Graph) and IMAP providers implement.
//!
//! - `detect_provider(email, choose)`: detection (detect.rs). Built-in domain
//!   table → MX lookup → autoconfig (the domain's own, then Thunderbird's
//!   ISPDB) → unknown. Never sends the address anywhere, only the domain.
//! - `microsoft_client_status` / `set_microsoft_client`: the user's own
//!   Microsoft Entra app registration (ms_client.rs), like the Google client.
//! - `connect_account` (connect.rs): signs in a Microsoft or IMAP account
//!   from a `ConnectAccountRequest`, dispatched by auth method. Phase 0
//!   answers "not implemented yet"; the provider agents fill it in.
//! - `registry`: which backend serves an account (every mailbox call goes
//!   through it), and `install_backends`, where each provider registers.
//!
//! Mirrored in `apps/desktop/src/lib/types.ts` (DetectedProvider,
//! ProviderKind, AuthMethod, SetupKind, DetectionSource, ServerSettings,
//! MailSecurity, ConnectAccountRequest, MicrosoftClientStatus): change both
//! together. `AuthMethod`, `ServerSettings` and `MailSecurity` live in
//! penguin-core (they're also part of `Account.providerConfig`).

pub mod autoconfig;
pub mod connect;
pub mod detect;
pub mod ms_client;
pub mod registry;
pub mod table;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{CmdError, CmdResult};
use crate::state::{blocking, AppState};

pub use connect::connect_account;
pub use detect::{Detector, ProviderNet, SystemNet};
pub use penguin_core::{AuthMethod, MailSecurity, ServerSettings};

/// Who hosts the mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    Gmail,
    GoogleWorkspace,
    OutlookPersonal,
    Microsoft365,
    Yahoo,
    Aol,
    Icloud,
    Fastmail,
    /// Any IMAP/SMTP server (from autoconfig, a manual choice, or Proton
    /// Mail Bridge).
    ImapGeneric,
    Unknown,
}

/// Which instructions the Add account flow shows. Also what a manual
/// "Choose provider" pick sends back as `choose`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupKind {
    Google,
    Microsoft,
    Yahoo,
    Aol,
    Icloud,
    Fastmail,
    /// Proton Mail through Proton Mail Bridge (IMAP on this Mac).
    Proton,
    /// Server settings form.
    Imap,
    /// "We don't support <domain> automatically yet."
    Unsupported,
}

/// Where the answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectionSource {
    /// Built-in table of provider domains (no network).
    DomainTable,
    /// The domain's MX records.
    Mx,
    /// The domain's own autoconfig file (autoconfig.<domain> or .well-known).
    Autoconfig,
    /// Thunderbird's ISPDB (autoconfig.thunderbird.net), for the domain or
    /// its mail server's domain.
    Ispdb,
    /// The user picked the provider.
    Manual,
    /// Nothing matched.
    Unknown,
}

/// `detect_provider`'s answer for one address.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedProvider {
    /// The address, trimmed, with the domain lowercased (IDN as punycode).
    pub email: String,
    pub domain: String,
    pub kind: ProviderKind,
    /// "Google Workspace", "iCloud Mail", the ISPDB's name for the host, …
    pub display_name: String,
    pub auth: AuthMethod,
    pub setup: SetupKind,
    /// Known or detected IMAP server (usernames already filled in).
    pub imap: Option<ServerSettings>,
    pub smtp: Option<ServerSettings>,
    pub source: DetectionSource,
    /// The domain's preferred MX host, when one was looked up and found.
    pub mx_host: Option<String>,
    /// A mail security gateway (Proofpoint, Mimecast, …) in front of the
    /// real provider, which hides who hosts the mailbox.
    pub gateway: Option<String>,
    /// Detection couldn't reach DNS or the web; the answer is a guess.
    pub offline: bool,
    /// This build can connect the account now (`connect_account` or, for
    /// Google, `add_account`). False = the flow shows "coming in the next
    /// update" at its last step.
    pub available: bool,
}

/// What the Add account flow sends to `connect_account` (connect.rs).
/// Deserialized here so the shape the UI sends is pinned by a test.
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectAccountRequest {
    pub email: String,
    pub kind: ProviderKind,
    pub auth: AuthMethod,
    /// Microsoft: the Application (client) ID (also saved by set_microsoft_client).
    #[serde(default)]
    pub client_id: Option<String>,
    /// App password or account password. Goes to the Keychain, never to logs.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub imap: Option<ServerSettings>,
    #[serde(default)]
    pub smtp: Option<ServerSettings>,
}

impl std::fmt::Debug for ConnectAccountRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectAccountRequest")
            .field("kind", &self.kind)
            .field("auth", &self.auth)
            .field("client_id", &self.client_id)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("imap", &self.imap)
            .field("smtp", &self.smtp)
            .finish_non_exhaustive()
    }
}

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

/// The account provider that signs in with `auth`.
pub fn provider_for(auth: AuthMethod) -> penguin_core::AccountProvider {
    use penguin_core::AccountProvider as P;
    match auth {
        AuthMethod::GoogleOAuth => P::Gmail,
        AuthMethod::MicrosoftOAuth => P::Microsoft,
        AuthMethod::AppPassword | AuthMethod::ImapPassword => P::Imap,
    }
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
        detector: Detector::new(Arc::new(SystemNet::new())),
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
        Some(raw) => Some(ms_client::normalize_client_id(raw).map_err(CmdError::invalid)?),
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
