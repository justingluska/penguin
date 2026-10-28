//! penguin-onboard: adding a mail account, shared by the Mac app and the
//! iPhone app (docs/providers-and-onboarding.md §4).
//!
//! - [`Detector`] (detect.rs): which provider hosts an address. Built-in
//!   domain table → MX lookup → autoconfig (the domain's own, then
//!   Thunderbird's ISPDB) → unknown. Never sends the address anywhere, only
//!   the domain.
//! - [`ConnectAccountRequest`], [`validate`] and [`new_account`]
//!   (request.rs): what the Add account flow sends, checked without the
//!   network, and the account record a successful sign-in stores.
//! - [`normalize_client_id`]: the user's own Microsoft Entra app's
//!   Application (client) ID, as pasted.
//!
//! The wire shapes are the Mac's `types.ts` names (DetectedProvider,
//! ProviderKind, AuthMethod, SetupKind, DetectionSource, ServerSettings,
//! MailSecurity, ConnectAccountRequest): change both together. `AuthMethod`,
//! `ServerSettings` and `MailSecurity` live in penguin-core (they're also
//! part of `Account.providerConfig`).

pub mod autoconfig;
pub mod detect;
mod request;
pub mod table;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use detect::{parse_email, Address, Detector, ProviderNet, SystemNet};
pub use penguin_core::{AuthMethod, MailSecurity, ServerSettings};
pub use request::{new_account, normalize_client_id, validate, Checked, MAX_PASSWORD};

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

impl ProviderKind {
    /// The wire name ("icloud", "outlookPersonal"), which is also what
    /// `Account.provider_config.host` keeps.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Gmail => "gmail",
            ProviderKind::GoogleWorkspace => "googleWorkspace",
            ProviderKind::OutlookPersonal => "outlookPersonal",
            ProviderKind::Microsoft365 => "microsoft365",
            ProviderKind::Yahoo => "yahoo",
            ProviderKind::Aol => "aol",
            ProviderKind::Icloud => "icloud",
            ProviderKind::Fastmail => "fastmail",
            ProviderKind::ImapGeneric => "imapGeneric",
            ProviderKind::Unknown => "unknown",
        }
    }
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

/// The detector's answer for one address.
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
    /// This build can connect the account now (the app decides, see
    /// [`Detector::with_available`]). False = the flow shows "coming in the
    /// next update" at its last step.
    pub available: bool,
}

/// What the Add account flow sends to `connect_account`.
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

/// The account provider that signs in with `auth`.
pub fn provider_for(auth: AuthMethod) -> penguin_core::AccountProvider {
    use penguin_core::AccountProvider as P;
    match auth {
        AuthMethod::GoogleOAuth => P::Gmail,
        AuthMethod::MicrosoftOAuth => P::Microsoft,
        AuthMethod::AppPassword | AuthMethod::ImapPassword => P::Imap,
    }
}
