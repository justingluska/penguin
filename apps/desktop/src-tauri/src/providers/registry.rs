//! Which backend serves an account: one [`Backend`] per `AccountProvider`,
//! plus a cache of per-account [`MailProvider`] clients. `AppState` owns
//! one; every mailbox call in the app goes `state.provider(account)` →
//! here. No Tauri, so dispatch is unit-tested with the fake backend.
//!
//! Gmail's backend is installed when a Google OAuth client is configured
//! (and replaced when it changes). The IMAP and Microsoft backends register
//! in `providers::install_backends` at startup.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use penguin_core::{Account, AccountProvider};
use penguin_provider::{Backend, MailProvider};

use crate::error::{CmdError, CmdResult};

/// A cached client and the kind of backend that built it.
type CachedClient = (AccountProvider, Arc<dyn MailProvider>);

#[derive(Default)]
pub struct Registry {
    backends: RwLock<HashMap<AccountProvider, Arc<dyn Backend>>>,
    /// account id → its client.
    clients: Mutex<HashMap<String, CachedClient>>,
}

/// The error for an account whose provider this build can't serve.
pub fn unavailable(provider: AccountProvider) -> CmdError {
    match provider {
        AccountProvider::Gmail => CmdError::not_configured(),
        other => penguin_provider::Error::not_implemented(other, "Syncing this account").into(),
    }
}

impl Registry {
    /// Install (or replace) the backend of its kind. Cached clients of that
    /// kind belong to the old backend and are dropped.
    pub fn set(&self, backend: Arc<dyn Backend>) {
        let kind = backend.provider();
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, (k, _)| *k != kind);
        self.backends
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(kind, backend);
    }

    pub fn get(&self, provider: AccountProvider) -> Option<Arc<dyn Backend>> {
        self.backends
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&provider)
            .cloned()
    }

    /// The backend for `provider`, or the error the user sees: Gmail
    /// without a Google client is `notConfigured`; a kind this build lacks
    /// is "not implemented yet".
    pub fn backend(&self, provider: AccountProvider) -> CmdResult<Arc<dyn Backend>> {
        self.get(provider).ok_or_else(|| unavailable(provider))
    }

    /// Every installed backend (to fan out settings such as the window).
    pub fn all(&self) -> Vec<Arc<dyn Backend>> {
        self.backends
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// The account's client, if one is cached.
    pub fn cached(&self, account_id: &str) -> Option<Arc<dyn MailProvider>> {
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account_id)
            .map(|(_, c)| c.clone())
    }

    /// The account's client, built by its provider's backend on first use.
    pub fn client(&self, account: &Account) -> CmdResult<Arc<dyn MailProvider>> {
        if let Some(c) = self.cached(&account.id) {
            return Ok(c);
        }
        let client = self.backend(account.provider)?.client(account)?;
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account.id.clone(), (account.provider, client.clone()));
        Ok(client)
    }

    /// Drop the account's cached client (removed account).
    pub fn forget(&self, account_id: &str) {
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account_id);
    }
}

#[cfg(test)]
mod tests {
    use penguin_core::Store;
    use penguin_provider::fake::{FakeBackend, RecordingObserver};
    use penguin_provider::window::WindowPolicy;

    use super::*;
    use crate::error::ErrorCode;

    fn account(id: &str, provider: AccountProvider) -> Account {
        Account {
            id: id.into(),
            email: id.into(),
            color: "#123456".into(),
            provider,
            ..Account::default()
        }
    }

    fn fake(provider: AccountProvider, store: &Store) -> Arc<FakeBackend> {
        Arc::new(FakeBackend::new(
            provider,
            store.clone(),
            Arc::new(RecordingObserver::default()),
        ))
    }

    #[test]
    fn accounts_reach_their_own_provider() {
        let store = Store::open_in_memory().unwrap();
        let reg = Registry::default();
        let gmail = fake(AccountProvider::Gmail, &store);
        let imap = fake(AccountProvider::Imap, &store);
        reg.set(gmail.clone());
        reg.set(imap.clone());
        let g = account("ada@gmail.example", AccountProvider::Gmail);
        let i = account("bea@icloud.example", AccountProvider::Imap);
        assert_eq!(reg.client(&g).unwrap().provider(), AccountProvider::Gmail);
        assert_eq!(reg.client(&i).unwrap().provider(), AccountProvider::Imap);
        assert_eq!(reg.client(&i).unwrap().account_id(), "bea@icloud.example");
        assert!(gmail.clients.lock().unwrap().contains_key(&g.id));
        assert!(!gmail.clients.lock().unwrap().contains_key(&i.id));
        assert!(imap.clients.lock().unwrap().contains_key(&i.id));
        // Cached: the backend builds a client once.
        let a = reg.client(&i).unwrap();
        let b = reg.client(&i).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn missing_backends_explain_themselves() {
        let reg = Registry::default();
        let e = reg
            .client(&account("ada@gmail.example", AccountProvider::Gmail))
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::NotConfigured);
        let e = reg
            .client(&account("bea@icloud.example", AccountProvider::Imap))
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::Other);
        assert_eq!(
            e.message,
            "Syncing this account is not implemented yet for IMAP"
        );
        let e = reg.backend(AccountProvider::Microsoft).err().unwrap();
        assert!(
            e.message.ends_with("not implemented yet for Microsoft"),
            "{}",
            e.message
        );
    }

    #[test]
    fn replacing_a_backend_drops_only_its_clients() {
        let store = Store::open_in_memory().unwrap();
        let reg = Registry::default();
        reg.set(fake(AccountProvider::Gmail, &store));
        reg.set(fake(AccountProvider::Imap, &store));
        let g = account("ada@gmail.example", AccountProvider::Gmail);
        let i = account("bea@icloud.example", AccountProvider::Imap);
        let old_g = reg.client(&g).unwrap();
        let old_i = reg.client(&i).unwrap();
        // A new Google client: new Gmail clients, IMAP untouched.
        reg.set(fake(AccountProvider::Gmail, &store));
        assert!(!Arc::ptr_eq(&old_g, &reg.client(&g).unwrap()));
        assert!(Arc::ptr_eq(&old_i, &reg.client(&i).unwrap()));
        reg.forget(&i.id);
        assert!(reg.cached(&i.id).is_none());
        assert_eq!(reg.all().len(), 2);
    }

    #[test]
    fn sync_goes_to_the_accounts_backend() {
        let store = Store::open_in_memory().unwrap();
        let reg = Registry::default();
        let imap = fake(AccountProvider::Imap, &store);
        reg.set(imap.clone());
        let i = account("bea@icloud.example", AccountProvider::Imap);
        let backend = reg.backend(i.provider).unwrap();
        let h = backend.start_sync(&i);
        h.poke();
        assert!(h.is_running());
        assert!(backend.sync_status(&i.id).is_some());
        for b in reg.all() {
            b.set_window_policy(WindowPolicy::EVERYTHING);
        }
        assert_eq!(*imap.policy.lock().unwrap(), Some(WindowPolicy::EVERYTHING));
        h.stop();
        assert!(!h.is_running());
    }
}
