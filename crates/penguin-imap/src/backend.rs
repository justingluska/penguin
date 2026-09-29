//! The IMAP kind behind the provider seam ([`Backend`]): one shared
//! context per account (settings, Keychain access, interactive connection,
//! folder cache), clients built on it, and the per-account sync tasks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use penguin_core::{Account, AccountProvider, Store, SyncErrorKind, SyncStatus};
use penguin_provider::credentials::{PasswordCredential, SecretVault, IMAP_SERVICE};
use penguin_provider::window::WindowPolicy;
use penguin_provider::{async_trait, Backend, MailProvider, SyncHandle, SyncObserver};

use crate::account::{Config, Ctx};
use crate::provider::ImapProvider;
use crate::sync::{self, Handle, Shared};
use crate::{db, Error, Result};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The process-wide IMAP Keychain vault (read at most once per process).
pub fn keychain_vault() -> Arc<SecretVault<PasswordCredential>> {
    static VAULT: OnceLock<Arc<SecretVault<PasswordCredential>>> = OnceLock::new();
    VAULT
        .get_or_init(|| Arc::new(SecretVault::keychain(IMAP_SERVICE)))
        .clone()
}

struct Entry {
    shared: Arc<Shared>,
    handle: Option<Handle>,
}

struct Inner {
    store: Store,
    observer: Arc<dyn SyncObserver>,
    vault: Arc<SecretVault<PasswordCredential>>,
    ctxs: Mutex<HashMap<String, Arc<Ctx>>>,
    tasks: Mutex<HashMap<String, Entry>>,
    policy: Mutex<WindowPolicy>,
    /// The provider tables couldn't be created (reported on use).
    schema_error: Option<String>,
    /// IDLE connections (tests turn them off).
    idle: bool,
}

#[derive(Clone)]
pub struct ImapBackend {
    inner: Arc<Inner>,
}

impl ImapBackend {
    /// The app's backend: the Keychain vault, IDLE on.
    pub fn new(store: Store, observer: Arc<dyn SyncObserver>) -> ImapBackend {
        ImapBackend::with_vault(store, observer, keychain_vault(), true)
    }

    /// With a given vault (tests use an in-memory one).
    pub fn with_vault(
        store: Store,
        observer: Arc<dyn SyncObserver>,
        vault: Arc<SecretVault<PasswordCredential>>,
        idle: bool,
    ) -> ImapBackend {
        let schema_error = db::migrate(&store).err().map(|e| {
            tracing::error!(error = %e, "IMAP provider tables couldn't be created");
            e.to_string()
        });
        ImapBackend {
            inner: Arc::new(Inner {
                store,
                observer,
                vault,
                ctxs: Mutex::new(HashMap::new()),
                tasks: Mutex::new(HashMap::new()),
                policy: Mutex::new(WindowPolicy::default()),
                schema_error,
                idle,
            }),
        }
    }

    pub fn vault(&self) -> &Arc<SecretVault<PasswordCredential>> {
        &self.inner.vault
    }

    /// The account's shared context (rebuilt when its settings change).
    pub fn ctx(&self, account: &Account) -> Result<Arc<Ctx>> {
        if let Some(e) = &self.inner.schema_error {
            return Err(Error::Other(format!("IMAP storage unavailable: {e}")));
        }
        let cfg = Config::from_account(account)?;
        let mut ctxs = self.inner.ctxs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = ctxs.get(&account.id) {
            if c.cfg == cfg {
                return Ok(c.clone());
            }
        }
        let ctx = Arc::new(Ctx::new(
            cfg,
            self.inner.store.clone(),
            self.inner.vault.clone(),
        ));
        ctxs.insert(account.id.clone(), ctx.clone());
        Ok(ctx)
    }

    fn shared_for(&self, account_id: &str) -> Arc<Shared> {
        let policy = *self.inner.policy.lock().unwrap_or_else(|p| p.into_inner());
        let mut tasks = self.inner.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks
            .entry(account_id.to_string())
            .or_insert_with(|| Entry {
                shared: Shared::new(account_id, self.inner.observer.clone(), policy),
                handle: None,
            })
            .shared
            .clone()
    }

    /// A handle that reports the error and does nothing (bad settings,
    /// unusable storage). Nothing retries it: shown at once.
    fn failed(&self, account_id: &str, e: &Error) -> SyncHandle {
        let shared = self.shared_for(account_id);
        let kind = match e.sync_kind() {
            SyncErrorKind::Other => SyncErrorKind::Config,
            k => k,
        };
        let message = e.to_string();
        shared.update(|s| s.record_failure(kind, message, now_ms(), None));
        SyncHandle::new(Arc::new(Dead(shared)))
    }
}

struct Dead(Arc<Shared>);

impl penguin_provider::SyncTask for Dead {
    fn poke(&self) {}
    fn stop(&self) {}
    fn is_running(&self) -> bool {
        let _ = &self.0;
        false
    }
}

#[async_trait]
impl Backend for ImapBackend {
    fn provider(&self) -> AccountProvider {
        AccountProvider::Imap
    }

    fn client(&self, account: &Account) -> Result<Arc<dyn MailProvider>> {
        if account.provider != AccountProvider::Imap {
            return Err(Error::Other(format!(
                "{} is not an IMAP account",
                account.id
            )));
        }
        Ok(Arc::new(ImapProvider::new(self.ctx(account)?)))
    }

    fn has_credentials(&self, account: &Account) -> bool {
        self.inner.vault.contains(&account.id)
    }

    async fn sign_out(&self, account: &Account) -> Result<()> {
        let ctx = self
            .inner
            .ctxs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&account.id);
        if let Some(ctx) = ctx {
            ctx.disconnect().await;
        }
        let (vault, id) = (self.inner.vault.clone(), account.id.clone());
        tokio::task::spawn_blocking(move || vault.delete(&id))
            .await
            .map_err(|e| Error::Other(format!("keychain task failed: {e}")))?
    }

    fn start_sync(&self, account: &Account) -> SyncHandle {
        let ctx = match self.ctx(account) {
            Ok(c) => c,
            Err(e) => return self.failed(&account.id, &e),
        };
        let shared = self.shared_for(&account.id);
        let mut tasks = self.inner.tasks.lock().unwrap_or_else(|p| p.into_inner());
        let entry = tasks.get_mut(&account.id).expect("created by shared_for");
        if let Some(h) = entry.handle.as_ref() {
            if penguin_provider::SyncTask::is_running(h) {
                return SyncHandle::new(Arc::new(h.clone()));
            }
        }
        let handle = sync::spawn(ctx, shared, self.inner.idle);
        entry.handle = Some(handle.clone());
        SyncHandle::new(Arc::new(handle))
    }

    fn retry_sync(&self, account: &Account) -> SyncHandle {
        let shared = self.shared_for(&account.id);
        // The failure stays until this attempt stores or checks mail (or
        // fails again), so the UI can tell whether the retry worked.
        shared.update(|s| s.retry_now(now_ms()));
        let h = self.start_sync(account);
        h.poke();
        h
    }

    fn sync_status(&self, account_id: &str) -> Option<SyncStatus> {
        self.inner
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account_id)
            .map(|e| e.shared.snapshot())
    }

    fn set_window_policy(&self, policy: WindowPolicy) {
        *self.inner.policy.lock().unwrap_or_else(|p| p.into_inner()) = policy;
        for e in self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
        {
            e.shared.set_policy(policy);
        }
    }
}
