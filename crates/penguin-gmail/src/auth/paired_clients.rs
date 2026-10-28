//! "Desktop app" clients this device received from a Mac (Pair iPhone,
//! docs/PAIRING.md). A refresh token only refreshes with the client that
//! minted it, so a browser sign-in from the Mac works on the phone only
//! with the Mac's client id and secret. They're kept in their own Keychain
//! item (service [`KEYCHAIN_SERVICE`], account [`PAIRED_CLIENTS_ACCOUNT`]),
//! never in a file, as `{"version": 1, "clients": {client_id: secret}}`.
//!
//! Only the phone writes this item (on import); the Mac never reads it
//! unless one of its own tokens names a desktop client other than its
//! configured one, and then it finds nothing.
//!
//! Google documents an installed app's client secret as not confidential;
//! it's still treated like a token here: never logged, never in `Debug`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::keychain::SecretBackend;
use super::OAuthClientConfig;
use crate::{Error, Result};

/// Keychain account name of the item holding the paired desktop clients.
pub const PAIRED_CLIENTS_ACCOUNT: &str = "desktop-clients.v1";
const VERSION: u32 = 1;

#[derive(Default, Serialize, Deserialize)]
struct ClientsFile {
    version: u32,
    /// client id → client secret (None for a client without one).
    #[serde(default)]
    clients: BTreeMap<String, Option<String>>,
}

impl Drop for ClientsFile {
    fn drop(&mut self) {
        for secret in self.clients.values_mut().flatten() {
            zeroize::Zeroize::zeroize(secret);
        }
    }
}

/// Write-through cache over the Keychain item, read at most once per
/// process (a failed read is retried on the next use).
pub(crate) struct PairedClients {
    backend: Box<dyn SecretBackend>,
    loaded: Mutex<Option<BTreeMap<String, OAuthClientConfig>>>,
}

/// The process-wide instance every `AuthManager` shares (tests use their
/// own in-memory ones).
#[cfg(not(test))]
pub(crate) fn shared_paired_clients() -> Arc<PairedClients> {
    use super::keychain::KeyringBackend;
    use std::sync::OnceLock;
    static CLIENTS: OnceLock<Arc<PairedClients>> = OnceLock::new();
    CLIENTS
        .get_or_init(|| Arc::new(PairedClients::new(Box::new(KeyringBackend))))
        .clone()
}

impl PairedClients {
    pub fn new(backend: Box<dyn SecretBackend>) -> Self {
        Self {
            backend,
            loaded: Mutex::new(None),
        }
    }

    fn with_loaded<T>(
        &self,
        f: impl FnOnce(&mut BTreeMap<String, OAuthClientConfig>) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(self.read()?);
        }
        f(guard.as_mut().expect("loaded above"))
    }

    fn read(&self) -> Result<BTreeMap<String, OAuthClientConfig>> {
        let Some(secret) = self
            .backend
            .get(PAIRED_CLIENTS_ACCOUNT)?
            .map(Zeroizing::new)
        else {
            return Ok(BTreeMap::new());
        };
        let file: ClientsFile = serde_json::from_str(&secret)
            .map_err(|_| Error::Keychain("stored desktop clients are unreadable".into()))?;
        if file.version > VERSION {
            return Err(Error::Keychain(format!(
                "stored desktop clients version {} is newer than this build supports",
                file.version
            )));
        }
        Ok(file
            .clients
            .iter()
            .map(|(id, secret)| {
                (
                    id.clone(),
                    OAuthClientConfig {
                        client_id: id.clone(),
                        client_secret: secret.clone(),
                    },
                )
            })
            .collect())
    }

    /// The stored client with this id, if any.
    pub fn get(&self, client_id: &str) -> Result<Option<OAuthClientConfig>> {
        self.with_loaded(|clients| Ok(clients.get(client_id).cloned()))
    }

    /// Store a client (replacing one with the same id). Writes only when
    /// something changed; never overwrites an item it couldn't read.
    pub fn add(&self, client: &OAuthClientConfig) -> Result<()> {
        self.with_loaded(|clients| {
            let same = clients
                .get(&client.client_id)
                .is_some_and(|c| c.client_secret == client.client_secret);
            if same {
                return Ok(());
            }
            let mut next = clients.clone();
            next.insert(client.client_id.clone(), client.clone());
            let file = ClientsFile {
                version: VERSION,
                clients: next
                    .values()
                    .map(|c| (c.client_id.clone(), c.client_secret.clone()))
                    .collect(),
            };
            let json = Zeroizing::new(
                serde_json::to_string(&file).map_err(|e| Error::Other(e.to_string()))?,
            );
            self.backend.set(PAIRED_CLIENTS_ACCOUNT, &json)?;
            *clients = next;
            tracing::info!(
                clients = clients.len(),
                "stored a desktop client from pairing"
            );
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::keychain::memory::MemoryBackend;
    use super::*;

    fn client(id: &str, secret: &str) -> OAuthClientConfig {
        OAuthClientConfig {
            client_id: id.into(),
            client_secret: Some(secret.into()),
        }
    }

    #[test]
    fn stores_reads_back_and_reads_the_item_once() {
        let backend = Arc::new(MemoryBackend::default());
        let clients = PairedClients::new(Box::new(backend.clone()));
        assert!(clients
            .get("1-desk.apps.googleusercontent.com")
            .unwrap()
            .is_none());
        clients
            .add(&client("1-desk.apps.googleusercontent.com", "GOCSPX-a"))
            .unwrap();
        // Same again: no second write.
        clients
            .add(&client("1-desk.apps.googleusercontent.com", "GOCSPX-a"))
            .unwrap();
        assert_eq!(backend.sets.lock().unwrap().len(), 1);
        assert_eq!(backend.gets_of(PAIRED_CLIENTS_ACCOUNT), 1);

        // A fresh process reads what was stored.
        let again = PairedClients::new(Box::new(backend.clone()));
        let got = again
            .get("1-desk.apps.googleusercontent.com")
            .unwrap()
            .unwrap();
        assert_eq!(got.client_secret.as_deref(), Some("GOCSPX-a"));
        let stored: serde_json::Value =
            serde_json::from_str(&backend.items.lock().unwrap()[PAIRED_CLIENTS_ACCOUNT]).unwrap();
        assert_eq!(stored["version"], 1);
        assert_eq!(
            stored["clients"]["1-desk.apps.googleusercontent.com"],
            "GOCSPX-a"
        );
    }

    #[test]
    fn an_unreadable_item_is_never_overwritten() {
        let backend = Arc::new(MemoryBackend::default());
        backend
            .items
            .lock()
            .unwrap()
            .insert(PAIRED_CLIENTS_ACCOUNT.into(), r#"{"version":9}"#.into());
        let clients = PairedClients::new(Box::new(backend.clone()));
        assert!(clients.get("x").is_err());
        assert!(clients.add(&client("x", "y")).is_err());
        assert!(backend.sets.lock().unwrap().is_empty());
    }
}
