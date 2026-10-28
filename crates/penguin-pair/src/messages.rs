//! What travels inside the encrypted channel (JSON, camelCase).

use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{PairError, Result};

/// Limits on what the phone may claim, so a hostile hello can't fill the
/// Mac's screen or memory.
const MAX_TEXT: usize = 120;
const MAX_ACCOUNTS: usize = 500;

/// The phone introduces itself (inside the encrypted hello).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneHello {
    /// The name the phone goes by ("iPhone", or the user's name for it).
    pub device_name: String,
    /// "iPhone17,1".
    #[serde(default)]
    pub device_model: String,
    /// "iOS 26.0".
    #[serde(default)]
    pub system_version: String,
    #[serde(default)]
    pub app_version: String,
    /// The Google "iOS" OAuth client id the phone's build signs in with,
    /// None when it has none. Only tokens minted by this client can
    /// refresh on the phone.
    #[serde(default)]
    pub google_client_id: Option<String>,
    /// Lowercased addresses the phone already has a working token for:
    /// the Mac doesn't send these again.
    #[serde(default)]
    pub accounts: Vec<String>,
}

impl PhoneHello {
    /// Trim what the Mac will show; refuse an absurd account list.
    pub(crate) fn sanitized(mut self) -> Result<Self> {
        if self.accounts.len() > MAX_ACCOUNTS {
            return Err(PairError::Protocol("too many accounts".into()));
        }
        for text in [
            &mut self.device_name,
            &mut self.device_model,
            &mut self.system_version,
            &mut self.app_version,
        ] {
            *text = clean_text(text);
        }
        if self.device_name.is_empty() {
            self.device_name = "iPhone".into();
        }
        self.google_client_id = self
            .google_client_id
            .map(|c| c.trim().to_ascii_lowercase())
            .filter(|c| !c.is_empty() && c.len() <= 200);
        for a in &mut self.accounts {
            *a = a.trim().to_lowercase();
        }
        Ok(self)
    }
}

/// Printable text only, at most [`MAX_TEXT`] characters.
fn clean_text(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_TEXT)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// The Mac to the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MacMessage {
    /// The hello was accepted; the Mac is asking its user to confirm.
    #[serde(rename_all = "camelCase")]
    Waiting { mac_name: String },
    /// The user clicked Send.
    Transfer(Transfer),
    /// The user said no, or the Mac can't send anything.
    Declined { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub mac_name: String,
    /// The Mac's sign-in-sheet client (an iOS-type client), if it has one.
    #[serde(default)]
    pub google_client_id: Option<String>,
    /// `{"version": 3, "accounts": {email: credential}}`, the Keychain
    /// vault format both apps store (penguin-gmail `auth::keychain`), with
    /// only the tokens the phone can use.
    pub vault: SecretString,
    /// The Mac's "Desktop app" client, sent when `vault` holds tokens it
    /// minted (browser sign-ins): the phone needs it to refresh them.
    /// Protocol version 2; absent in a version 1 transfer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_client: Option<DesktopClient>,
    /// IMAP accounts' passwords (app passwords), as the IMAP Keychain vault
    /// stores them (penguin-provider `credentials`):
    /// `{"version": 1, "accounts": {account id: {password, smtpPassword?, savedAt}}}`.
    /// Protocol version 3; absent when no IMAP account moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imap_vault: Option<SecretString>,
    /// Microsoft accounts' refresh tokens, each with the Application
    /// (client) ID that minted it (a Microsoft token refreshes only with
    /// its own client), as the Microsoft Keychain vault stores them.
    /// Protocol version 3; absent when no Microsoft account moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microsoft_vault: Option<SecretString>,
    /// Every account on the Mac, in the Mac's order, with what happens to
    /// it on the phone (Gmail only before version 3).
    pub accounts: Vec<AccountMeta>,
}

/// Which provider serves an account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountKind {
    /// Gmail through Google's API (what versions 1 and 2 sent).
    #[default]
    Gmail,
    /// IMAP + SMTP with a password.
    Imap,
    /// Microsoft Graph (Outlook.com, Microsoft 365).
    Microsoft,
}

/// A Google "Desktop app" OAuth client, exactly as the Mac loaded it from
/// its client JSON. Google treats an installed app's secret as not
/// confidential, but it still travels only inside the encrypted transfer,
/// is never printed (`Debug` redacts it) and is wiped on drop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopClient {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<SecretString>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMeta {
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
    /// "#4F7CFF".
    pub color: String,
    /// Position in the Mac's account list, from 0.
    pub order: u32,
    pub status: AccountStatus,
    /// Protocol version 3; Gmail when absent.
    #[serde(default)]
    pub provider: AccountKind,
    /// IMAP and Microsoft: the account's server settings, exactly the
    /// Mac's `Account.providerConfig` (IMAP and SMTP servers and user
    /// names, the Microsoft client and tenant ids). Not secret: passwords
    /// and tokens travel only in the vaults. Protocol version 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AccountStatus {
    /// Its token is in `vault`.
    Token,
    /// The phone said it has this account already; nothing sent.
    AlreadyOnPhone,
    /// No usable token for the phone: sign in on the phone.
    SignIn { reason: SignInReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignInReason {
    /// Signed in through the browser with a "Desktop app" client the Mac
    /// can't send (version 1 never sent one; version 2 only when the Mac's
    /// client JSON is missing).
    DesktopClient,
    /// Signed in with an iOS client other than the phone's, or with a
    /// desktop client other than the one the Mac has now.
    OtherClient,
    /// The Mac has no token for it (signed out, or its Keychain is locked).
    NoToken,
}

impl Transfer {
    /// The provider settings the Mac sent for `email`: a JSON object of at
    /// most [`MAX_PROVIDER_CONFIG`] bytes, else None (a hostile Mac can't
    /// make the phone store a huge record).
    pub fn provider_config_of(&self, email: &str) -> Option<&serde_json::Value> {
        self.accounts
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(email))
            .and_then(|a| a.provider_config.as_ref())
            .filter(|v| v.is_object() && v.to_string().len() <= MAX_PROVIDER_CONFIG)
    }
}

/// The most of one account's provider settings the phone accepts (bytes of JSON).
pub const MAX_PROVIDER_CONFIG: usize = 4096;

/// The phone's answer to a [`Transfer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PhoneMessage {
    Done(PhoneDone),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneDone {
    /// Accounts whose tokens are now in the phone's Keychain.
    pub imported: Vec<String>,
    pub failed: Vec<FailedAccount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedAccount {
    pub email: String,
    pub reason: String,
}

/// A string holding secrets (the vault JSON): never printed, wiped on drop.
/// Serde copies may linger in freed memory; the long-lived copies don't.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted {} bytes>", self.0.len())
    }
}

pub(crate) fn to_json<T: Serialize>(value: &T) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    serde_json::to_vec(value)
        .map(zeroize::Zeroizing::new)
        .map_err(|e| PairError::Other(format!("can't encode a pairing message: {e}")))
}

pub(crate) fn from_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    // The error text names the field, never the value.
    serde_json::from_slice(bytes)
        .map_err(|e| PairError::Protocol(format!("malformed message at column {}", e.column())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_string_is_redacted() {
        let t = Transfer {
            mac_name: "Mac".into(),
            google_client_id: None,
            vault: SecretString::new(r#"{"refresh_token":"1//secret"}"#.into()),
            desktop_client: Some(DesktopClient {
                client_id: "1-desk.apps.googleusercontent.com".into(),
                client_secret: Some(SecretString::new("GOCSPX-fictional".into())),
            }),
            imap_vault: Some(SecretString::new(
                r#"{"version":1,"accounts":{"sam@icloud.example":{"password":"app-pass-1234"}}}"#.into(),
            )),
            microsoft_vault: Some(SecretString::new(
                r#"{"version":1,"accounts":{"bea@outlook.example":{"refreshToken":"M.C5-secret"}}}"#.into(),
            )),
            accounts: vec![],
        };
        let shown = format!("{t:?}");
        assert!(!shown.contains("1//secret"));
        assert!(!shown.contains("GOCSPX"));
        assert!(!shown.contains("app-pass-1234"));
        assert!(!shown.contains("M.C5-secret"));
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""vault":"{\"refresh_token\""#), "{json}");
        assert!(
            json.contains(r#""desktopClient":{"clientId":"1-desk.apps.googleusercontent.com","clientSecret":"GOCSPX-fictional"}"#),
            "{json}"
        );
    }

    #[test]
    fn a_transfer_without_a_desktop_client_still_reads() {
        // What a version 1 Mac sent: no desktopClient field.
        let t: Transfer = serde_json::from_str(
            r#"{"macName":"Mac","vault":"{}","accounts":[],"googleClientId":null}"#,
        )
        .unwrap();
        assert_eq!(t.desktop_client, None);
        assert_eq!(t.imap_vault, None);
        assert_eq!(t.microsoft_vault, None);
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("desktopClient"), "{json}");
        assert!(
            !json.contains("imapVault") && !json.contains("microsoftVault"),
            "{json}"
        );
    }

    #[test]
    fn accounts_without_a_provider_are_gmail() {
        // Versions 1 and 2 sent Gmail accounts only, with no provider.
        let t: Transfer = serde_json::from_str(
            r##"{"macName":"Mac","vault":"{}","accounts":[{"email":"ada@example.com","color":"#4F7CFF","order":0,"status":{"kind":"token"}}]}"##,
        )
        .unwrap();
        assert_eq!(t.accounts[0].provider, AccountKind::Gmail);
        assert_eq!(t.accounts[0].provider_config, None);
        let imap = AccountMeta {
            provider: AccountKind::Imap,
            provider_config: Some(serde_json::json!({"host": "icloud"})),
            ..t.accounts[0].clone()
        };
        let json = serde_json::to_value(&imap).unwrap();
        assert_eq!(json["provider"], "imap");
        assert_eq!(json["providerConfig"]["host"], "icloud");
        // Provider settings are read back only as a small object.
        let mut t = t;
        t.accounts = vec![imap.clone()];
        assert!(t.provider_config_of("ADA@example.com").is_some());
        t.accounts[0].provider_config = Some(serde_json::json!("not an object"));
        assert!(t.provider_config_of("ada@example.com").is_none());
        t.accounts[0].provider_config =
            Some(serde_json::json!({ "host": "x".repeat(MAX_PROVIDER_CONFIG) }));
        assert!(t.provider_config_of("ada@example.com").is_none());
    }

    #[test]
    fn hello_is_cleaned() {
        let hello = PhoneHello {
            device_name: format!("  Ada\u{202E}'s\n iPhone{}", "x".repeat(500)),
            device_model: "iPhone17,1".into(),
            system_version: "iOS 26.0".into(),
            app_version: String::new(),
            google_client_id: Some(" 1-ABC.apps.googleusercontent.com ".into()),
            accounts: vec![" Ada@Example.com ".into()],
        }
        .sanitized()
        .unwrap();
        assert!(hello.device_name.starts_with("Ada's iPhone"));
        assert_eq!(hello.device_name.chars().count(), MAX_TEXT);
        assert_eq!(
            hello.google_client_id.as_deref(),
            Some("1-abc.apps.googleusercontent.com")
        );
        assert_eq!(hello.accounts, ["ada@example.com"]);
        let blank = PhoneHello {
            device_name: " \u{7} ".into(),
            ..hello.clone()
        }
        .sanitized()
        .unwrap();
        assert_eq!(blank.device_name, "iPhone");
        let flood = PhoneHello {
            accounts: vec!["a@b.example".into(); MAX_ACCOUNTS + 1],
            ..hello
        };
        assert!(flood.sanitized().is_err());
    }

    #[test]
    fn statuses_serialize_readably() {
        let meta = AccountMeta {
            email: "ada@example.com".into(),
            display_name: None,
            nickname: None,
            color: "#4F7CFF".into(),
            order: 0,
            status: AccountStatus::SignIn {
                reason: SignInReason::DesktopClient,
            },
            provider: AccountKind::Gmail,
            provider_config: None,
        };
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(
            json["status"],
            serde_json::json!({"kind": "signIn", "reason": "desktopClient"})
        );
    }
}
