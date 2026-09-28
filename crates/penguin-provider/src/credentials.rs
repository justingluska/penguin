//! Secrets for the IMAP and Microsoft providers, in the macOS Keychain.
//!
//! Same layout as Gmail's tokens (penguin-gmail `auth/keychain.rs`): ONE
//! generic-password item per provider — service [`IMAP_SERVICE`] or
//! [`MICROSOFT_SERVICE`], account [`VAULT_ACCOUNT`] — whose secret is
//! versioned JSON `{"version": 1, "accounts": {<account id>: <credential>}}`.
//! One item means one Keychain access prompt per binary identity, not one
//! per account. [`SecretVault`] reads the item at most once per process,
//! remembers a failed read (no prompt storms; only a write retries) and
//! never overwrites an item it couldn't read.
//!
//! Credentials are keyed by account id (the lowercased address). Their
//! `Debug` redacts every secret; never log them or put them in errors.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Keychain service of the IMAP vault (app and account passwords).
pub const IMAP_SERVICE: &str = "co.gluska.penguin.imap";
/// Keychain service of the Microsoft vault (Graph refresh tokens).
pub const MICROSOFT_SERVICE: &str = "co.gluska.penguin.microsoft";
/// Keychain account name of the single item in each vault.
pub const VAULT_ACCOUNT: &str = "credentials.v1";
/// The vault format this build writes and the newest it reads.
pub const VAULT_VERSION: u32 = 1;

/// An IMAP account's secrets. The server settings themselves are in
/// `Account.provider_config` (not secret).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordCredential {
    /// The app password (Yahoo, AOL, iCloud, Fastmail) or account password,
    /// for IMAP and, unless `smtp_password` is set, SMTP.
    pub password: String,
    /// A different SMTP password, for the rare server that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp_password: Option<String>,
    /// Unix seconds it was stored (last successful login check).
    #[serde(default)]
    pub saved_at: u64,
}

impl fmt::Debug for PasswordCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordCredential")
            .field("password", &"<redacted>")
            .field(
                "smtp_password",
                &self.smtp_password.as_ref().map(|_| "<redacted>"),
            )
            .field("saved_at", &self.saved_at)
            .finish()
    }
}

/// A Microsoft account's tokens (public client, PKCE, `/common`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftCredential {
    pub refresh_token: String,
    /// The Application (client) ID that issued it: Microsoft refreshes a
    /// token only with its own client, so a new client means signing in again.
    pub client_id: String,
    /// Scopes Microsoft reported as granted.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The token's `tid` (tenant) and `oid` (user) claims, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    /// Unix seconds the refresh token was obtained (rotated tokens update it).
    #[serde(default)]
    pub obtained_at: u64,
}

impl fmt::Debug for MicrosoftCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MicrosoftCredential")
            .field("refresh_token", &"<redacted>")
            .field("client_id", &self.client_id)
            .field("scopes", &self.scopes)
            .field("tenant_id", &self.tenant_id)
            .field("object_id", &self.object_id)
            .field("obtained_at", &self.obtained_at)
            .finish()
    }
}

/// Raw secret items of one Keychain service, by Keychain account name.
/// Implementations may block (a Keychain read can show a prompt); call from
/// the blocking pool.
pub trait SecretBackend: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    /// Deleting a missing item is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

/// The macOS Keychain (login keychain) through `keyring`.
pub struct KeyringBackend {
    service: &'static str,
}

impl KeyringBackend {
    pub fn new(service: &'static str) -> KeyringBackend {
        KeyringBackend { service }
    }
    fn entry(&self, account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(self.service, account).map_err(keychain_err)
    }
}

impl SecretBackend for KeyringBackend {
    fn get(&self, account: &str) -> Result<Option<String>> {
        match self.entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_err(e)),
        }
    }
    fn set(&self, account: &str, secret: &str) -> Result<()> {
        self.entry(account)?
            .set_password(secret)
            .map_err(keychain_err)
    }
    fn delete(&self, account: &str) -> Result<()> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_err(e)),
        }
    }
}

fn keychain_err(e: keyring::Error) -> Error {
    // BadEncoding carries the raw secret bytes; never format them.
    match e {
        keyring::Error::BadEncoding(_) => {
            Error::Keychain("stored credential is not valid UTF-8".into())
        }
        other => Error::Keychain(other.to_string()),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(bound = "T: Serialize + DeserializeOwned")]
struct VaultFile<T> {
    version: u32,
    #[serde(default = "BTreeMap::new")]
    accounts: BTreeMap<String, T>,
}

enum Loaded<T> {
    No,
    /// The item couldn't be read; reported again without re-reading.
    Failed(String),
    Yes(BTreeMap<String, T>),
}

/// In-memory, write-through cache over one vault item. Create one per
/// provider per process (e.g. in a `OnceLock`) so every client shares it.
pub struct SecretVault<T> {
    backend: Box<dyn SecretBackend>,
    /// Held across backend calls, which makes the first load single-flight.
    state: Mutex<Loaded<T>>,
}

impl<T: Clone + Serialize + DeserializeOwned> SecretVault<T> {
    pub fn new(backend: Box<dyn SecretBackend>) -> SecretVault<T> {
        SecretVault {
            backend,
            state: Mutex::new(Loaded::No),
        }
    }

    /// The Keychain vault of `service` ([`IMAP_SERVICE`], [`MICROSOFT_SERVICE`]).
    pub fn keychain(service: &'static str) -> SecretVault<T> {
        SecretVault::new(Box::new(KeyringBackend::new(service)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Loaded<T>> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn loaded<'a>(&self, state: &'a mut Loaded<T>) -> Result<&'a mut BTreeMap<String, T>> {
        if let Loaded::Failed(msg) = state {
            return Err(Error::Keychain(msg.clone()));
        }
        if matches!(state, Loaded::No) {
            *state = match self.read() {
                Ok(map) => Loaded::Yes(map),
                Err(e) => {
                    tracing::warn!(error = %e, "credential vault unreadable; not retrying until the next sign-in");
                    Loaded::Failed(e.to_string())
                }
            };
        }
        match state {
            Loaded::Yes(map) => Ok(map),
            Loaded::Failed(msg) => Err(Error::Keychain(msg.clone())),
            Loaded::No => unreachable!("loaded above"),
        }
    }

    fn read(&self) -> Result<BTreeMap<String, T>> {
        let Some(secret) = self.backend.get(VAULT_ACCOUNT)? else {
            return Ok(BTreeMap::new());
        };
        let file: VaultFile<T> = serde_json::from_str(&secret)
            .map_err(|_| Error::Keychain("stored credential vault is unreadable".into()))?;
        if file.version > VAULT_VERSION {
            return Err(Error::Keychain(format!(
                "credential vault version {} is newer than this build supports",
                file.version
            )));
        }
        Ok(file.accounts)
    }

    fn write(&self, accounts: &BTreeMap<String, T>) -> Result<()> {
        let file = VaultFile {
            version: VAULT_VERSION,
            accounts: accounts.clone(),
        };
        let secret = serde_json::to_string(&file).map_err(|e| Error::Other(e.to_string()))?;
        self.backend.set(VAULT_ACCOUNT, &secret)
    }

    /// The account's credential. The first call per process reads the item.
    pub fn load(&self, account_id: &str) -> Result<Option<T>> {
        let mut state = self.lock();
        Ok(self.loaded(&mut state)?.get(account_id).cloned())
    }

    /// Whether the account has a credential (false when the vault can't be read).
    pub fn contains(&self, account_id: &str) -> bool {
        self.load(account_id).is_ok_and(|c| c.is_some())
    }

    /// Store or replace the account's credential. A write is a user action
    /// (sign-in), so an earlier failed read is retried once here; a vault
    /// that still can't be read is never overwritten (that would drop the
    /// other accounts' secrets).
    pub fn save(&self, account_id: &str, credential: &T) -> Result<()> {
        let mut state = self.lock();
        if matches!(*state, Loaded::Failed(_)) {
            *state = Loaded::No;
        }
        let map = self.loaded(&mut state)?;
        let mut next = map.clone();
        next.insert(account_id.to_string(), credential.clone());
        self.write(&next)?;
        *map = next;
        Ok(())
    }

    /// Remove the account's credential (missing is fine).
    pub fn delete(&self, account_id: &str) -> Result<()> {
        let mut state = self.lock();
        if matches!(*state, Loaded::Failed(_)) {
            *state = Loaded::No;
        }
        let map = self.loaded(&mut state)?;
        if map.contains_key(account_id) {
            let mut next = map.clone();
            next.remove(account_id);
            self.write(&next)?;
            *map = next;
        }
        Ok(())
    }
}

/// An in-memory [`SecretBackend`] for tests, counting reads.
#[derive(Default)]
pub struct MemorySecrets {
    pub items: Mutex<BTreeMap<String, String>>,
    pub reads: Mutex<u32>,
    pub fail_reads: Mutex<bool>,
}

impl SecretBackend for std::sync::Arc<MemorySecrets> {
    fn get(&self, account: &str) -> Result<Option<String>> {
        *self.reads.lock().unwrap() += 1;
        if *self.fail_reads.lock().unwrap() {
            return Err(Error::Keychain("user canceled".into()));
        }
        Ok(self.items.lock().unwrap().get(account).cloned())
    }
    fn set(&self, account: &str, secret: &str) -> Result<()> {
        self.items
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<()> {
        self.items.lock().unwrap().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn pw(p: &str) -> PasswordCredential {
        PasswordCredential {
            password: p.into(),
            smtp_password: None,
            saved_at: 7,
        }
    }

    #[test]
    fn item_shape_is_pinned() {
        let mem = Arc::new(MemorySecrets::default());
        let vault: SecretVault<PasswordCredential> = SecretVault::new(Box::new(mem.clone()));
        vault.save("ada@mail.example", &pw("app-pass")).unwrap();
        let item = mem.items.lock().unwrap()[VAULT_ACCOUNT].clone();
        assert_eq!(
            item,
            r#"{"version":1,"accounts":{"ada@mail.example":{"password":"app-pass","savedAt":7}}}"#
        );
        let ms = MicrosoftCredential {
            refresh_token: "rt".into(),
            client_id: "1b2c3d4e-0000-1111-2222-333344445555".into(),
            scopes: vec!["Mail.ReadWrite".into()],
            tenant_id: Some("tid".into()),
            object_id: None,
            obtained_at: 9,
        };
        assert_eq!(
            serde_json::to_string(&ms).unwrap(),
            r#"{"refreshToken":"rt","clientId":"1b2c3d4e-0000-1111-2222-333344445555","scopes":["Mail.ReadWrite"],"tenantId":"tid","obtainedAt":9}"#
        );
    }

    #[test]
    fn debug_never_shows_secrets() {
        let mut c = pw("hunter2");
        c.smtp_password = Some("smtp-secret".into());
        let s = format!("{c:?}");
        assert!(!s.contains("hunter2") && !s.contains("smtp-secret"), "{s}");
        let ms = MicrosoftCredential {
            refresh_token: "0.AAAA-secret".into(),
            client_id: "c".into(),
            scopes: vec![],
            tenant_id: None,
            object_id: None,
            obtained_at: 0,
        };
        assert!(!format!("{ms:?}").contains("AAAA-secret"));
    }

    #[test]
    fn reads_once_and_keeps_every_account() {
        let mem = Arc::new(MemorySecrets::default());
        let vault: SecretVault<PasswordCredential> = SecretVault::new(Box::new(mem.clone()));
        assert_eq!(vault.load("a@x.example").unwrap(), None);
        vault.save("a@x.example", &pw("1")).unwrap();
        vault.save("b@x.example", &pw("2")).unwrap();
        assert!(vault.contains("a@x.example"));
        assert_eq!(vault.load("b@x.example").unwrap().unwrap().password, "2");
        vault.delete("a@x.example").unwrap();
        vault.delete("nobody@x.example").unwrap();
        assert!(!vault.contains("a@x.example"));
        assert_eq!(*mem.reads.lock().unwrap(), 1);
        // A new process reads what was written.
        let again: SecretVault<PasswordCredential> = SecretVault::new(Box::new(mem.clone()));
        assert_eq!(again.load("b@x.example").unwrap().unwrap().password, "2");
    }

    #[test]
    fn a_failed_read_is_remembered_and_never_overwritten() {
        let mem = Arc::new(MemorySecrets::default());
        mem.items.lock().unwrap().insert(
            VAULT_ACCOUNT.into(),
            r#"{"version":1,"accounts":{"b@x.example":{"password":"2"}}}"#.into(),
        );
        *mem.fail_reads.lock().unwrap() = true;
        let vault: SecretVault<PasswordCredential> = SecretVault::new(Box::new(mem.clone()));
        assert!(vault.load("b@x.example").is_err());
        assert!(vault.load("b@x.example").is_err());
        assert_eq!(*mem.reads.lock().unwrap(), 1, "no prompt storm");
        // A save retries the read once, and refuses to clobber on failure.
        assert!(vault.save("a@x.example", &pw("1")).is_err());
        assert_eq!(*mem.reads.lock().unwrap(), 2);
        assert!(mem.items.lock().unwrap()[VAULT_ACCOUNT].contains("b@x.example"));
        *mem.fail_reads.lock().unwrap() = false;
        vault.save("a@x.example", &pw("1")).unwrap();
        assert!(vault.contains("b@x.example") && vault.contains("a@x.example"));
    }

    #[test]
    fn newer_vaults_are_refused() {
        let mem = Arc::new(MemorySecrets::default());
        mem.items.lock().unwrap().insert(
            VAULT_ACCOUNT.into(),
            r#"{"version":2,"accounts":{}}"#.into(),
        );
        let vault: SecretVault<PasswordCredential> = SecretVault::new(Box::new(mem));
        let e = vault.load("a@x.example").unwrap_err().to_string();
        assert!(e.contains("newer"), "{e}");
    }
}
