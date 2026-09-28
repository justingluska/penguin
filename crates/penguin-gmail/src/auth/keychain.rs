//! Refresh-token storage.
//!
//! All accounts share ONE generic-password item in the macOS login Keychain:
//! service [`KEYCHAIN_SERVICE`], account [`VAULT_ACCOUNT`], secret = versioned
//! JSON `{"version": 3, "accounts": {email: StoredCredential}}`, where each entry
//! records the OAuth client its refresh token belongs to. One item means
//! one Keychain access prompt per binary identity instead of one per account.
//!
//! The item is read at most once per process: [`Vault`] keeps the decoded map
//! in memory (a process-wide instance backs every `AuthManager`, see
//! [`shared_keychain`]) and writes through on sign-in, token rotation and
//! sign-out. Failed reads are remembered too, so a denied prompt doesn't turn
//! into a prompt storm; only a write (a user action) retries the read.
//!
//! Migration from v1 (one item per account, account name = email) is lazy and
//! happens once per account: when an email is missing from the map, its v1
//! item is read once, folded into the map, written back, and deleted.
//!
//! Compatibility: an older build ignores fields it doesn't know and would
//! drop them when it rewrites the item. So any new field in a record needs a
//! version bump, which [`Vault`] enforces by refusing newer versions.
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const KEYCHAIN_SERVICE: &str = "co.gluska.penguin.google";
/// Keychain account name of the single item holding every account's token.
pub const VAULT_ACCOUNT: &str = "tokens.v2";

const CREDENTIAL_VERSION: u32 = 1;
/// v3: each credential records the OAuth client that minted it.
const VAULT_VERSION: u32 = 3;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredCredential {
    #[serde(default = "default_version")]
    pub version: u32,
    pub refresh_token: String,
    /// Scopes Google reported as granted at sign-in.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Unix seconds.
    #[serde(default)]
    pub obtained_at: u64,
    /// The client the refresh token is bound to (Google only refreshes it
    /// with that client). Entries from before v3 are desktop-client tokens.
    #[serde(default)]
    pub client: TokenClient,
}

/// Which OAuth client minted a refresh token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum TokenClient {
    /// The "Desktop app" client (loopback sign-in). `client_id` is recorded
    /// for new sign-ins; `None` for tokens migrated from older vaults.
    Desktop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
    /// The "iOS" client (system sign-in sheet). Refreshing needs only the id.
    Ios { client_id: String },
}

impl Default for TokenClient {
    fn default() -> Self {
        Self::Desktop { client_id: None }
    }
}

fn default_version() -> u32 {
    CREDENTIAL_VERSION
}

impl StoredCredential {
    pub fn new(refresh_token: String, scopes: Vec<String>, obtained_at: u64) -> Self {
        Self {
            version: CREDENTIAL_VERSION,
            refresh_token,
            scopes,
            obtained_at,
            client: TokenClient::default(),
        }
    }

    pub fn with_client(mut self, client: TokenClient) -> Self {
        self.client = client;
        self
    }
}

impl fmt::Debug for StoredCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredCredential")
            .field("version", &self.version)
            .field("refresh_token", &"<redacted>")
            .field("scopes", &self.scopes)
            .field("obtained_at", &self.obtained_at)
            .field("client", &self.client)
            .finish()
    }
}

/// Where refresh tokens live, keyed by lowercased email. Implementations may
/// block (a Keychain read can show a system prompt), so async callers go
/// through `spawn_blocking`.
pub(crate) trait CredentialStore: Send + Sync {
    fn load(&self, account: &str) -> Result<Option<StoredCredential>>;
    fn save(&self, account: &str, credential: &StoredCredential) -> Result<()>;
    /// Deleting a missing entry is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

/// Raw secret items under [`KEYCHAIN_SERVICE`], by Keychain account name.
pub(crate) trait SecretBackend: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    /// Deleting a missing item is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

pub(crate) struct KeyringBackend;

impl KeyringBackend {
    fn entry(account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(keychain_err)
    }
}

impl SecretBackend for KeyringBackend {
    fn get(&self, account: &str) -> Result<Option<String>> {
        match Self::entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_err(e)),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(keychain_err)
    }

    fn delete(&self, account: &str) -> Result<()> {
        match Self::entry(account)?.delete_credential() {
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

/// The process-wide Keychain vault shared by every `AuthManager`, so
/// recreating the manager (e.g. after the OAuth client changes) never causes
/// another Keychain read.
pub(crate) fn shared_keychain() -> Arc<Vault> {
    static VAULT: OnceLock<Arc<Vault>> = OnceLock::new();
    VAULT
        .get_or_init(|| Arc::new(Vault::new(Box::new(KeyringBackend))))
        .clone()
}

#[derive(Default, Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    #[serde(default)]
    accounts: BTreeMap<String, StoredCredential>,
}

enum Loaded {
    No,
    /// The vault item couldn't be read; reported again without re-reading.
    Failed(String),
    Yes,
}

struct VaultState {
    loaded: Loaded,
    accounts: BTreeMap<String, StoredCredential>,
    /// Emails whose v1 item has already been looked up this process.
    legacy_checked: HashSet<String>,
}

/// In-memory, write-through cache over the single Keychain item.
pub(crate) struct Vault {
    backend: Box<dyn SecretBackend>,
    /// Held across backend calls, which makes the first load single-flight.
    state: Mutex<VaultState>,
}

impl Vault {
    pub fn new(backend: Box<dyn SecretBackend>) -> Self {
        Self {
            backend,
            state: Mutex::new(VaultState {
                loaded: Loaded::No,
                accounts: BTreeMap::new(),
                legacy_checked: HashSet::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VaultState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Read the vault item unless it was already read (or failed) this process.
    fn ensure_loaded(&self, state: &mut VaultState) -> Result<()> {
        match &state.loaded {
            Loaded::Yes => return Ok(()),
            Loaded::Failed(msg) => return Err(Error::Keychain(msg.clone())),
            Loaded::No => {}
        }
        match self.read_vault() {
            Ok(accounts) => {
                tracing::debug!(accounts = accounts.len(), "keychain vault loaded");
                state.accounts = accounts;
                state.loaded = Loaded::Yes;
                Ok(())
            }
            Err(e) => {
                tracing::warn!(error = %e, "keychain vault unreadable; not retrying until the next sign-in");
                state.loaded = Loaded::Failed(e.to_string());
                Err(e)
            }
        }
    }

    fn read_vault(&self) -> Result<BTreeMap<String, StoredCredential>> {
        let Some(secret) = self.backend.get(VAULT_ACCOUNT)? else {
            return Ok(BTreeMap::new());
        };
        let file: VaultFile = serde_json::from_str(&secret)
            .map_err(|_| Error::Keychain("stored token vault is unreadable".into()))?;
        if file.version > VAULT_VERSION {
            return Err(Error::Keychain(format!(
                "token vault version {} is newer than this build supports",
                file.version
            )));
        }
        if file.version < VAULT_VERSION {
            // v2 → v3: entries without a `client` deserialize as desktop-client
            // tokens (the only client before v3). Persist the upgrade now; if
            // that fails the next write persists it anyway.
            match self.write_vault(&file.accounts) {
                Ok(()) => tracing::info!(
                    from = file.version,
                    to = VAULT_VERSION,
                    "upgraded keychain token vault"
                ),
                Err(e) => tracing::warn!(error = %e, "could not persist token vault upgrade"),
            }
        }
        Ok(file.accounts)
    }

    fn write_vault(&self, accounts: &BTreeMap<String, StoredCredential>) -> Result<()> {
        let file = VaultFile {
            version: VAULT_VERSION,
            accounts: accounts.clone(),
        };
        let secret = serde_json::to_string(&file).map_err(|e| Error::Other(e.to_string()))?;
        self.backend.set(VAULT_ACCOUNT, &secret)
    }

    /// Fold an account's v1 item into the vault, once per process per email.
    fn migrate_legacy(&self, state: &mut VaultState, account: &str) -> Result<()> {
        if account == VAULT_ACCOUNT || !state.legacy_checked.insert(account.to_owned()) {
            return Ok(());
        }
        let Some(secret) = self.backend.get(account)? else {
            return Ok(());
        };
        let credential: StoredCredential = serde_json::from_str(&secret).map_err(|_| {
            Error::Keychain(format!("stored credential for {account} is unreadable"))
        })?;
        let mut accounts = state.accounts.clone();
        accounts.insert(account.to_owned(), credential);
        self.write_vault(&accounts)?;
        state.accounts = accounts;
        // Already safe in the vault; a leftover v1 item is never read again.
        if let Err(e) = self.backend.delete(account) {
            tracing::warn!(account, error = %e, "could not delete migrated keychain item");
        }
        tracing::info!(
            account,
            "migrated keychain credential into the shared vault"
        );
        Ok(())
    }
}

impl CredentialStore for Vault {
    fn load(&self, account: &str) -> Result<Option<StoredCredential>> {
        let mut state = self.lock();
        self.ensure_loaded(&mut state)?;
        if !state.accounts.contains_key(account) {
            self.migrate_legacy(&mut state, account)?;
        }
        Ok(state.accounts.get(account).cloned())
    }

    fn save(&self, account: &str, credential: &StoredCredential) -> Result<()> {
        let mut state = self.lock();
        if matches!(state.loaded, Loaded::Failed(_)) {
            // A write is a user action (sign-in), so one fresh read is fine.
            state.loaded = Loaded::No;
        }
        // Never overwrite a vault we couldn't read: that would drop the
        // other accounts' tokens.
        self.ensure_loaded(&mut state)?;
        let mut accounts = state.accounts.clone();
        accounts.insert(account.to_owned(), credential.clone());
        self.write_vault(&accounts)?;
        state.accounts = accounts;
        // A fresh sign-in supersedes any v1 item; don't migrate it later.
        if state.legacy_checked.insert(account.to_owned()) {
            if let Err(e) = self.backend.delete(account) {
                tracing::warn!(account, error = %e, "could not delete old keychain item");
            }
        }
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        let mut state = self.lock();
        if matches!(state.loaded, Loaded::Failed(_)) {
            state.loaded = Loaded::No;
        }
        self.ensure_loaded(&mut state)?;
        if state.accounts.contains_key(account) {
            let mut accounts = state.accounts.clone();
            accounts.remove(account);
            self.write_vault(&accounts)?;
            state.accounts = accounts;
        }
        // Sign-out must also remove an unmigrated v1 item.
        state.legacy_checked.insert(account.to_owned());
        self.backend.delete(account)
    }
}

#[cfg(test)]
pub(crate) mod memory {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub(crate) struct MemoryStore(pub Mutex<HashMap<String, StoredCredential>>);

    impl CredentialStore for MemoryStore {
        fn load(&self, account: &str) -> Result<Option<StoredCredential>> {
            Ok(self.0.lock().unwrap().get(account).cloned())
        }
        fn save(&self, account: &str, credential: &StoredCredential) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(account.to_owned(), credential.clone());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<()> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    /// Fake Keychain that counts calls per item.
    #[derive(Default)]
    pub(crate) struct MemoryBackend {
        pub items: Mutex<HashMap<String, String>>,
        pub gets: Mutex<Vec<String>>,
        pub sets: Mutex<Vec<String>>,
        pub fail_reads: Mutex<bool>,
    }

    impl MemoryBackend {
        pub fn gets_of(&self, account: &str) -> usize {
            self.gets
                .lock()
                .unwrap()
                .iter()
                .filter(|a| *a == account)
                .count()
        }
    }

    impl SecretBackend for Arc<MemoryBackend> {
        fn get(&self, account: &str) -> Result<Option<String>> {
            self.gets.lock().unwrap().push(account.to_owned());
            if *self.fail_reads.lock().unwrap() {
                return Err(Error::Keychain("user denied access".into()));
            }
            Ok(self.items.lock().unwrap().get(account).cloned())
        }
        fn set(&self, account: &str, secret: &str) -> Result<()> {
            self.sets.lock().unwrap().push(account.to_owned());
            self.items
                .lock()
                .unwrap()
                .insert(account.to_owned(), secret.to_owned());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<()> {
            self.items.lock().unwrap().remove(account);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::memory::MemoryBackend;
    use super::*;

    fn cred(rt: &str) -> StoredCredential {
        StoredCredential::new(rt.into(), vec!["openid".into()], 7)
    }

    fn vault() -> (Arc<MemoryBackend>, Vault) {
        let backend = Arc::new(MemoryBackend::default());
        (backend.clone(), Vault::new(Box::new(backend)))
    }

    fn vault_json(backend: &MemoryBackend) -> serde_json::Value {
        serde_json::from_str(&backend.items.lock().unwrap()[VAULT_ACCOUNT]).unwrap()
    }

    #[test]
    fn credential_roundtrips_and_tolerates_missing_fields() {
        let cred = StoredCredential::new("1//rt".into(), vec!["openid".into()], 1_700_000_000);
        let json = serde_json::to_string(&cred).unwrap();
        assert_eq!(
            serde_json::from_str::<StoredCredential>(&json).unwrap(),
            cred
        );

        let minimal: StoredCredential = serde_json::from_str(r#"{"refresh_token":"x"}"#).unwrap();
        assert_eq!(minimal.version, CREDENTIAL_VERSION);
        assert!(minimal.scopes.is_empty());
    }

    #[test]
    fn debug_redacts_refresh_token() {
        let cred = StoredCredential::new("1//very-secret".into(), vec![], 0);
        assert!(!format!("{cred:?}").contains("very-secret"));
    }

    #[test]
    fn vault_item_is_read_once_per_process() {
        let (backend, vault) = vault();
        let file = serde_json::json!({
            "version": 2,
            "accounts": {
                "ada@example.com": { "version": 1, "refresh_token": "rt-a", "scopes": [], "obtained_at": 1 },
                "grace@example.com": { "version": 1, "refresh_token": "rt-g", "scopes": [], "obtained_at": 2 },
            }
        });
        backend.set(VAULT_ACCOUNT, &file.to_string()).unwrap();

        for _ in 0..5 {
            assert_eq!(
                vault
                    .load("ada@example.com")
                    .unwrap()
                    .unwrap()
                    .refresh_token,
                "rt-a"
            );
            assert_eq!(
                vault
                    .load("grace@example.com")
                    .unwrap()
                    .unwrap()
                    .refresh_token,
                "rt-g"
            );
        }
        assert_eq!(backend.gets_of(VAULT_ACCOUNT), 1);
        // Present accounts never touch their v1 items.
        assert_eq!(backend.gets.lock().unwrap().len(), 1);

        // Unknown accounts: one v1 lookup each, then answered from memory.
        for _ in 0..5 {
            assert!(vault.load("nobody@example.com").unwrap().is_none());
        }
        assert_eq!(backend.gets_of("nobody@example.com"), 1);
        assert_eq!(backend.gets.lock().unwrap().len(), 2);
    }

    #[test]
    fn writes_go_to_the_single_item() {
        let (backend, vault) = vault();
        vault.save("ada@example.com", &cred("rt-a")).unwrap();
        vault.save("grace@example.com", &cred("rt-g")).unwrap();
        vault.save("ada@example.com", &cred("rt-a2")).unwrap();

        let items = backend.items.lock().unwrap().clone();
        assert_eq!(items.keys().collect::<Vec<_>>(), [VAULT_ACCOUNT]);
        let json = vault_json(&backend);
        assert_eq!(json["version"], 3);
        assert_eq!(
            json["accounts"]["ada@example.com"]["refresh_token"],
            "rt-a2"
        );
        assert_eq!(
            json["accounts"]["grace@example.com"]["refresh_token"],
            "rt-g"
        );

        vault.delete("ada@example.com").unwrap();
        let json = vault_json(&backend);
        assert!(json["accounts"].get("ada@example.com").is_none());
        assert!(vault.load("ada@example.com").unwrap().is_none());

        // A new process sees the same state with one read.
        let fresh = Vault::new(Box::new(backend.clone()));
        let before = backend.gets_of(VAULT_ACCOUNT);
        assert_eq!(
            fresh
                .load("grace@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-g"
        );
        assert_eq!(backend.gets_of(VAULT_ACCOUNT), before + 1);
    }

    #[test]
    fn upgrades_v2_vault_to_v3_with_desktop_clients() {
        let (backend, vault) = vault();
        let v2 = serde_json::json!({
            "version": 2,
            "accounts": {
                "ada@example.com": { "version": 1, "refresh_token": "rt-a", "scopes": ["openid"], "obtained_at": 1 },
            }
        });
        backend.set(VAULT_ACCOUNT, &v2.to_string()).unwrap();

        let cred = vault.load("ada@example.com").unwrap().unwrap();
        assert_eq!(cred.refresh_token, "rt-a");
        assert_eq!(cred.client, TokenClient::Desktop { client_id: None });

        let json = vault_json(&backend);
        assert_eq!(json["version"], 3);
        assert_eq!(
            json["accounts"]["ada@example.com"]["client"],
            serde_json::json!({"type": "desktop"})
        );
        assert_eq!(json["accounts"]["ada@example.com"]["refresh_token"], "rt-a");
        // Still one read.
        assert_eq!(backend.gets_of(VAULT_ACCOUNT), 1);
    }

    #[test]
    fn stores_client_per_token() {
        let (backend, vault) = vault();
        vault
            .save(
                "ada@example.com",
                &cred("rt-a").with_client(TokenClient::Ios {
                    client_id: "ios.apps.googleusercontent.com".into(),
                }),
            )
            .unwrap();
        vault
            .save(
                "grace@example.com",
                &cred("rt-g").with_client(TokenClient::Desktop {
                    client_id: Some("desk.apps.googleusercontent.com".into()),
                }),
            )
            .unwrap();
        let json = vault_json(&backend);
        assert_eq!(
            json["accounts"]["ada@example.com"]["client"],
            serde_json::json!({"type": "ios", "client_id": "ios.apps.googleusercontent.com"})
        );
        assert_eq!(
            json["accounts"]["grace@example.com"]["client"],
            serde_json::json!({"type": "desktop", "client_id": "desk.apps.googleusercontent.com"})
        );
        let next = Vault::new(Box::new(backend.clone()));
        assert_eq!(
            next.load("ada@example.com").unwrap().unwrap().client,
            TokenClient::Ios {
                client_id: "ios.apps.googleusercontent.com".into()
            }
        );
    }

    #[test]
    fn newer_vault_is_refused() {
        let (backend, vault) = vault();
        backend
            .set(VAULT_ACCOUNT, r#"{"version":4,"accounts":{}}"#)
            .unwrap();
        assert!(vault.load("ada@example.com").is_err());
    }

    #[test]
    fn migrates_v1_items_once() {
        let (backend, vault) = vault();
        for (email, rt) in [("ada@example.com", "rt-a"), ("grace@example.com", "rt-g")] {
            backend
                .set(email, &serde_json::to_string(&cred(rt)).unwrap())
                .unwrap();
        }

        assert_eq!(
            vault
                .load("ada@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-a"
        );
        assert_eq!(
            vault
                .load("grace@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-g"
        );
        assert_eq!(
            vault
                .load("ada@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-a"
        );
        assert_eq!(backend.gets_of("ada@example.com"), 1);
        assert_eq!(backend.gets_of("grace@example.com"), 1);

        // Old items are gone; the vault holds both.
        let items = backend.items.lock().unwrap().clone();
        assert_eq!(items.keys().collect::<Vec<_>>(), [VAULT_ACCOUNT]);
        let json = vault_json(&backend);
        assert_eq!(json["accounts"]["ada@example.com"]["refresh_token"], "rt-a");
        assert_eq!(
            json["accounts"]["grace@example.com"]["refresh_token"],
            "rt-g"
        );

        // Next process: only the vault item is read.
        backend.gets.lock().unwrap().clear();
        let next = Vault::new(Box::new(backend.clone()));
        assert!(next.load("ada@example.com").unwrap().is_some());
        assert!(next.load("grace@example.com").unwrap().is_some());
        assert_eq!(*backend.gets.lock().unwrap(), [VAULT_ACCOUNT]);
    }

    #[test]
    fn delete_removes_unmigrated_v1_item() {
        let (backend, vault) = vault();
        backend
            .set(
                "ada@example.com",
                &serde_json::to_string(&cred("rt-a")).unwrap(),
            )
            .unwrap();
        vault.delete("ada@example.com").unwrap();
        assert!(!backend
            .items
            .lock()
            .unwrap()
            .contains_key("ada@example.com"));
        assert!(vault.load("ada@example.com").unwrap().is_none());
    }

    #[test]
    fn failed_read_is_not_retried_until_a_write() {
        let (backend, vault) = vault();
        backend
            .set(VAULT_ACCOUNT, r#"{"version":2,"accounts":{}}"#)
            .unwrap();
        *backend.fail_reads.lock().unwrap() = true;
        for _ in 0..5 {
            assert!(matches!(
                vault.load("ada@example.com"),
                Err(Error::Keychain(_))
            ));
        }
        assert_eq!(backend.gets_of(VAULT_ACCOUNT), 1);

        // Still unreadable: the save must refuse rather than clobber.
        assert!(vault.save("ada@example.com", &cred("rt")).is_err());
        assert_eq!(backend.gets_of(VAULT_ACCOUNT), 2);
        assert_eq!(vault_json(&backend)["accounts"], serde_json::json!({}));

        // Access granted now: the next write re-reads and succeeds.
        *backend.fail_reads.lock().unwrap() = false;
        vault.save("ada@example.com", &cred("rt")).unwrap();
        assert!(vault.load("ada@example.com").unwrap().is_some());
        assert_eq!(backend.gets_of(VAULT_ACCOUNT), 3);
    }

    #[test]
    fn corrupt_vault_is_an_error_not_an_empty_map() {
        let (backend, vault) = vault();
        backend.set(VAULT_ACCOUNT, "not json").unwrap();
        assert!(vault.load("ada@example.com").is_err());
        assert!(vault.save("ada@example.com", &cred("rt")).is_err());
        assert_eq!(backend.items.lock().unwrap()[VAULT_ACCOUNT], "not json");
    }
}
