//! penguin-graph: Outlook.com, Hotmail, Live and Microsoft 365 through
//! Microsoft Graph, behind the provider seam (`penguin-provider`).
//!
//! - [`auth`]: sign-in with the user's own Entra app (public client, PKCE,
//!   `/common`, loopback redirect), refresh tokens in the Keychain, access
//!   tokens in memory; AADSTS errors keep their code.
//! - [`GraphProvider`] (`provider.rs`, `drafts.rs`): one account's
//!   `MailProvider`: messages, attachments, headers, MIME source, label
//!   deltas as Graph operations, drafts, sending, `$search`, estimates,
//!   the profile photo.
//! - [`GraphBackend`] (`sync.rs`): per-account sync tasks over per-folder
//!   delta with immutable ids, `conversationId` threads, the sync window.
//! - `folders.rs`: well-known folders → system labels, other folders →
//!   `f:<folder id>`, categories → `c:<name>`.
//! - [`http`]: the transport seam, throttling (429/503 + Retry-After) and
//!   the 4-requests-per-mailbox cap.
//!
//! Contract: docs/PROVIDERS-IMPL.md. Never logs tokens, bodies, subjects
//! or addresses (ids and codes only).

pub mod auth;
mod client;
mod convert;
pub mod cursor;
mod drafts;
pub mod folders;
pub mod http;
pub mod locations;
mod loopback;
mod meeting;
mod provider;
mod search;
mod sync;
mod wire;

#[cfg(test)]
mod fake_graph;
#[cfg(test)]
mod sync_bench;
#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};

use penguin_provider::credentials::{MicrosoftCredential, SecretVault, MICROSOFT_SERVICE};

pub use auth::{Auth, SignInUi, SignedIn, SCOPES};
pub use client::GraphClient;
pub use http::{Endpoints, ReqwestTransport, Transport};
pub use provider::GraphProvider;
pub use sync::GraphBackend;

/// The process's Microsoft tokens: the Keychain vault (read once per
/// process), the real network, Microsoft's `/common` endpoints. Shared by
/// sign-in and the backend so a fresh sign-in's token is used at once.
pub fn default_auth() -> Arc<Auth> {
    static AUTH: OnceLock<Arc<Auth>> = OnceLock::new();
    AUTH.get_or_init(|| {
        let vault: SecretVault<MicrosoftCredential> = SecretVault::keychain(MICROSOFT_SERVICE);
        Arc::new(Auth::new(
            Arc::new(ReqwestTransport::new()),
            Endpoints::microsoft(),
            Arc::new(vault),
        ))
    })
    .clone()
}

/// Browser sign-in for `account_id` (the typed address, lowercased) with
/// the user's Application (client) ID. Refuses another account; stores the
/// refresh token in the Keychain. Cancel by dropping the future (the
/// loopback listener closes).
pub async fn sign_in(
    client_id: &str,
    account_id: &str,
    ui: &SignInUi<'_>,
) -> penguin_provider::Result<SignedIn> {
    default_auth().sign_in(client_id, account_id, ui).await
}
