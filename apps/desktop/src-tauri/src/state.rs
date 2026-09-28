//! Long-lived app state: the store, the mail backends (Gmail's built once a
//! Google OAuth client exists; IMAP and Microsoft registered at startup),
//! and one sync task per account. Everything here is shared across commands
//! via `State<Arc<AppState>>`.
//!
//! Every mailbox call goes through the provider seam: `provider(account)`
//! returns the account's `MailProvider`, whichever backend serves it
//! (`providers::registry`). Only Google-only features (calendar, contact
//! photos, sign-in) use `services()` directly.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use penguin_core::{Account, AccountProvider, SemanticHandles, Store, SyncPhase, SyncStatus};
use penguin_gmail::auth::{AuthManager, IosClientConfig, OAuthClientConfig};
use penguin_gmail::provider::GmailBackend;
use penguin_gmail::sync::SyncEngine;
use penguin_provider::window::WindowPolicy;
use penguin_provider::{Backend, MailProvider, SyncHandle, SyncObserver};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::diagnostics::DiagState;
use crate::error::{CmdError, CmdResult};
use crate::inline_images::InlineImages;
use crate::ops::Paths;
use crate::providers::registry::Registry;
use crate::settings::{Settings, SettingsState, EVENT_SETTINGS_CHANGED};
use crate::views::{ActionFailedEvent, BodyFetchFailedEvent, MailChangedEvent};

pub const EVENT_SYNC_STATUS: &str = "penguin://sync-status";
pub const EVENT_MAIL_CHANGED: &str = "penguin://mail-changed";
/// An optimistic action was rejected by Gmail and has been rolled back.
pub const EVENT_ACTION_FAILED: &str = "penguin://action-failed";
/// Downloading an opened thread's headers-only messages failed.
pub const EVENT_BODY_FETCH_FAILED: &str = "penguin://body-fetch-failed";

/// Google's services, built once a Google OAuth client exists; rebuilt if it
/// is replaced. Gmail mail goes through `AppState::provider` like every
/// other provider; these are for Google-only features (sign-in, calendar,
/// contact photos, the account photo).
#[derive(Clone)]
pub struct Services {
    pub auth: AuthManager,
    pub engine: SyncEngine,
}

pub struct AppState {
    app: AppHandle,
    pub store: Store,
    pub paths: Paths,
    pub inline: InlineImages,
    services: RwLock<Option<Services>>,
    /// The backend of each provider kind and a client per account (each
    /// owns its connections).
    pub registry: Registry,
    /// Running sync tasks by account, with the kind that runs them.
    handles: Mutex<HashMap<String, (AccountProvider, SyncHandle)>>,
    /// Emits sync events for every backend.
    observer: Arc<dyn SyncObserver>,
    /// Status for accounts no engine is running (no credentials, no OAuth
    /// client yet, a provider this build lacks).
    local_status: Mutex<HashMap<String, SyncStatus>>,
    /// Files this session saved via save_attachment: the only paths
    /// open_path will hand to the OS.
    saved_paths: Mutex<HashSet<PathBuf>>,
    /// Serializes draft saves/sends/deletes (held across Gmail calls).
    pub drafts_lock: tokio::sync::Mutex<()>,
    /// Cancels the browser sign-in in flight (add_account / reconnect_account).
    /// Only one runs at a time; starting another cancels the previous one.
    sign_in_cancel: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// settings.json, loaded once at startup.
    pub settings: SettingsState,
    /// Session counters for the diagnostics screen.
    pub diag: Arc<DiagState>,
    /// Search by meaning: the embedding model and vector index, set by the
    /// semantic indexer once they exist (None = off; search is keyword-only).
    /// Read through `semantic()`; see docs/SEARCH-RANKING.md.
    semantic: RwLock<Option<SemanticHandles>>,
    /// The background indexer that downloads and loads the model, keeps
    /// semantic.db in step with the mail database, and fills `semantic`
    /// (src/semantic/, docs/SEMANTIC.md).
    pub semantic_indexer: Arc<crate::semantic::Semantic>,
    /// Bumped whenever local mail changes (sync, or the app itself: a body
    /// downloaded, a thread moved). The background scanners (fact
    /// extraction, the invitation scanner) sleep on it instead of polling.
    mail_changes: MailChanges,
}

/// A generation counter of local mail changes; see `AppState::mail_changes`.
pub type MailChanges = Arc<tokio::sync::watch::Sender<u64>>;

fn bump(changes: &MailChanges) {
    changes.send_modify(|g| *g = g.wrapping_add(1));
}

struct TauriObserver {
    app: AppHandle,
    diag: Arc<DiagState>,
    /// For queueing rule triggers (rules::enqueue).
    store: Store,
    /// Told about synced changes so new mail gets vectors quickly.
    semantic_indexer: Arc<crate::semantic::Semantic>,
    mail_changes: MailChanges,
}

impl SyncObserver for TauriObserver {
    fn status(&self, status: SyncStatus) {
        self.diag.sample(&status.account_id, status.indexed);
        emit(&self.app, EVENT_SYNC_STATUS, status);
    }
    fn mail_changed(&self, account_id: &str, thread_ids: Vec<String>) {
        self.semantic_indexer.mail_changed();
        bump(&self.mail_changes);
        emit(
            &self.app,
            EVENT_MAIL_CHANGED,
            MailChangedEvent {
                account_id: account_id.to_string(),
                thread_ids,
            },
        );
    }
    fn messages_added(&self, account_id: &str, message_ids: Vec<String>) {
        crate::notify::messages_added(&self.app, account_id, &message_ids);
        crate::rules::enqueue(
            &self.store,
            account_id,
            crate::rules::new_messages(message_ids),
        );
    }
    fn labels_added(&self, account_id: &str, changes: Vec<(String, Vec<String>)>) {
        crate::rules::enqueue(&self.store, account_id, crate::rules::labels_added(changes));
    }
}

fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!(event, error = %e, "failed to emit event");
    }
}

/// `blocking` for work nobody is waiting on (fact extraction, backfills):
/// it runs at utility QoS on macOS (see qos.rs).
pub async fn background<T, F>(f: F) -> CmdResult<T>
where
    F: FnOnce() -> CmdResult<T> + Send + 'static,
    T: Send + 'static,
{
    blocking(move || crate::qos::utility(f)).await
}

/// Run blocking work (Store calls, Keychain, rendering) off the async runtime.
pub async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    F: FnOnce() -> CmdResult<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CmdError::other(format!("background task failed: {e}")))?
}

impl AppState {
    pub fn init(app: &AppHandle) -> Result<Arc<AppState>, String> {
        let paths = Paths::resolve()?;
        paths
            .create_all()
            .map_err(|e| format!("creating app directories: {e}"))?;
        let store =
            Store::open(&paths.db_path()).map_err(|e| format!("opening the mail database: {e}"))?;
        let settings = SettingsState::load(&paths.config_dir);
        // Existing installs with accounts skip the Welcome setup; a new one
        // records that it hasn't had it yet (docs/ONBOARDING.md).
        let has_accounts = store.list_accounts().map(|a| !a.is_empty()).unwrap_or(false);
        if let Err(e) = settings.settle_welcome(has_accounts) {
            tracing::warn!(error = %e, "could not record the welcome setup state");
        }
        let diag = Arc::new(DiagState::default());
        let semantic =
            crate::semantic::Semantic::new(&paths.data_dir, settings.get().semantic_search);
        // No model download before the Welcome setup has asked.
        semantic.set_downloads_allowed(settings.get().welcome_completed);
        let mail_changes: MailChanges = Arc::new(tokio::sync::watch::Sender::new(0));
        let observer: Arc<dyn SyncObserver> = Arc::new(TauriObserver {
            app: app.clone(),
            diag: diag.clone(),
            store: store.clone(),
            semantic_indexer: semantic.clone(),
            mail_changes: mail_changes.clone(),
        });
        let state = Arc::new(AppState {
            app: app.clone(),
            settings,
            diag,
            semantic_indexer: semantic,
            store,
            inline: InlineImages::new(paths.clone()),
            paths,
            services: RwLock::new(None),
            registry: Registry::default(),
            handles: Mutex::new(HashMap::new()),
            observer,
            local_status: Mutex::new(HashMap::new()),
            saved_paths: Mutex::new(HashSet::new()),
            drafts_lock: tokio::sync::Mutex::new(()),
            sign_in_cancel: Mutex::new(None),
            semantic: RwLock::new(None),
            mail_changes,
        });
        match OAuthClientConfig::load(&state.paths.config_dir) {
            Ok(Some(config)) => state.install_services(config),
            Ok(None) => tracing::info!("no Google OAuth client configured yet"),
            // A malformed file is a setup problem, not a reason to refuse to
            // open the app: the setup screen will ask for the JSON again.
            Err(e) => tracing::warn!(error = %e, "ignoring unreadable Google OAuth client config"),
        }
        crate::providers::install_backends(&state);
        Ok(state)
    }

    /// (Re)build the auth manager and sync engine for a client config.
    /// Running Gmail sync tasks belong to the previous engine and are
    /// stopped; other providers keep running.
    pub fn install_services(&self, config: OAuthClientConfig) {
        self.stop_provider(AccountProvider::Gmail);
        let auth = AuthManager::new(config);
        // The optional iOS client enables the system sign-in sheet.
        auth.set_ios_client(self.ios_client());
        let engine = SyncEngine::new(self.store.clone(), auth.clone(), self.observer.clone());
        engine.set_window_policy(self.settings.get().window_policy());
        self.registry.set(Arc::new(GmailBackend::new(
            auth.clone(),
            engine.clone(),
            self.store.clone(),
        )));
        *self.services.write().unwrap() = Some(Services { auth, engine });
    }

    /// The saved iOS-type OAuth client (Settings / setup) that enables the
    /// system sign-in sheet, if any.
    pub fn ios_client(&self) -> Option<IosClientConfig> {
        match IosClientConfig::load(&self.paths.config_dir) {
            Ok(ios) => ios,
            Err(e) => {
                tracing::warn!(error = %e, "ignoring unreadable iOS OAuth client config");
                None
            }
        }
    }

    /// Register a non-Gmail backend (IMAP, Microsoft); see
    /// `providers::install_backends`. It gets the current window policy.
    pub fn register_backend(&self, backend: Arc<dyn Backend>) {
        backend.set_window_policy(self.settings.get().window_policy());
        self.registry.set(backend);
    }

    /// The observer every backend's sync engine reports through (Tauri
    /// events, diagnostics, rule triggers).
    pub fn sync_observer(&self) -> Arc<dyn SyncObserver> {
        self.observer.clone()
    }

    /// New Settings → Sync values for every backend.
    pub fn set_window_policy(&self, policy: WindowPolicy) {
        for backend in self.registry.all() {
            backend.set_window_policy(policy);
        }
    }

    /// Whether a Google OAuth client is configured (Google sign-in works).
    pub fn is_configured(&self) -> bool {
        self.services.read().unwrap().is_some()
    }

    /// The model and index hybrid search uses, if search by meaning is set
    /// up (cheap: two `Arc` clones).
    pub fn semantic(&self) -> Option<SemanticHandles> {
        self.semantic
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Install (or with None remove) the model and index. The indexer calls
    /// this when the model loads, and `set_semantic_progress` as it embeds.
    pub fn set_semantic(&self, handles: Option<SemanticHandles>) {
        *self.semantic.write().unwrap_or_else(|e| e.into_inner()) = handles;
    }

    /// Share of mail embedded (0–1). Below 1, search already uses what is
    /// embedded (newest first) and reports `semantic: "indexing"` with this
    /// progress.
    pub fn set_semantic_progress(&self, progress: f32) {
        if let Some(h) = self
            .semantic
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            h.progress = Some(progress);
        }
    }

    pub fn services(&self) -> CmdResult<Services> {
        self.services
            .read()
            .unwrap()
            .clone()
            .ok_or_else(CmdError::not_configured)
    }

    pub async fn accounts(&self) -> CmdResult<Vec<Account>> {
        let store = self.store.clone();
        blocking(move || Ok(store.list_accounts()?)).await
    }

    pub async fn account(&self, account_id: &str) -> CmdResult<Account> {
        self.accounts()
            .await?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| CmdError::not_found(format!("unknown account {account_id}")))
    }

    /// The account's mail provider (Gmail, IMAP or Microsoft): the one way
    /// the app reads from or writes to a mailbox. Fails with notConfigured
    /// for a Gmail account before a Google client exists, and "not
    /// implemented yet" for a provider this build lacks.
    pub async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>> {
        if let Some(c) = self.registry.cached(account_id) {
            return Ok(c);
        }
        let account = self.account(account_id).await?;
        self.registry.client(&account)
    }

    /// Fail unless every one of `account_ids` has a backend in this build
    /// (unknown accounts are skipped): checked before optimistic local
    /// changes that could never be pushed.
    pub async fn require_providers(
        &self,
        account_ids: impl IntoIterator<Item = &str>,
    ) -> CmdResult<()> {
        let accounts = self.accounts().await?;
        let mut seen = HashSet::new();
        for id in account_ids {
            if !seen.insert(id.to_string()) {
                continue;
            }
            if let Some(a) = accounts.iter().find(|a| a.id == id) {
                self.registry.backend(a.provider)?;
            }
        }
        Ok(())
    }

    /// Start sync for every stored account (app startup, new OAuth client).
    pub async fn start_all(self: &Arc<Self>) {
        let accounts = match self.accounts().await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(error = %e, "could not list accounts at startup");
                return;
            }
        };
        for account in accounts {
            self.start_account(&account).await;
        }
    }

    /// Ensure the sync task for one account is running (its backend returns
    /// the live task if there is one; a task that ended, e.g. on NeedsReauth,
    /// is replaced). Accounts without Keychain credentials are marked
    /// NeedsReauth instead.
    pub async fn start_account(self: &Arc<Self>, account: &Account) {
        let backend = match self.registry.backend(account.provider) {
            Ok(b) => b,
            Err(e) => {
                let message = match account.provider {
                    AccountProvider::Gmail => "Google OAuth client is not configured".to_string(),
                    _ => e.message,
                };
                self.set_local_status(&account.id, SyncPhase::Error, Some(message))
                    .await;
                return;
            }
        };
        let (b, a) = (backend.clone(), account.clone());
        let has_credentials = blocking(move || Ok(b.has_credentials(&a)))
            .await
            .unwrap_or(false);
        self.diag.set_keychain(&account.id, has_credentials);
        if !has_credentials {
            tracing::info!(account = %account.id, "no stored credentials; needs sign-in");
            self.set_local_status(
                &account.id,
                SyncPhase::NeedsReauth,
                Some("Sign in again to keep syncing".into()),
            )
            .await;
            return;
        }
        self.local_status.lock().unwrap().remove(&account.id);
        let handle = backend.start_sync(account);
        handle.poke();
        self.handles
            .lock()
            .unwrap()
            .insert(account.id.clone(), (account.provider, handle));
        tracing::info!(account = %account.id, provider = account.provider.as_str(), "sync running");
    }

    pub fn stop_account(&self, account_id: &str) {
        if let Some((_, h)) = self.handles.lock().unwrap().remove(account_id) {
            h.stop();
        }
    }

    pub fn forget_account(&self, account_id: &str) {
        self.stop_account(account_id);
        self.registry.forget(account_id);
        self.local_status.lock().unwrap().remove(account_id);
        self.diag.forget_account(account_id);
    }

    /// Stop every task of one provider kind (its backend is being replaced).
    pub fn stop_provider(&self, provider: AccountProvider) {
        self.handles.lock().unwrap().retain(|_, (kind, h)| {
            if *kind == provider {
                h.stop();
                false
            } else {
                true
            }
        });
    }

    pub fn poke_all(&self) {
        for (_, h) in self.handles.lock().unwrap().values() {
            h.poke();
        }
    }

    /// "Retry sync": clear the account's error, restart its task if it died,
    /// or wake it from error backoff. An account parked without a task (no
    /// credentials, no OAuth client) goes through start_account's checks again.
    pub async fn retry_account(self: &Arc<Self>, account: &Account) -> CmdResult<()> {
        let backend = self.registry.backend(account.provider)?;
        let has_task = self.handles.lock().unwrap().contains_key(&account.id);
        if !has_task {
            self.start_account(account).await;
            return Ok(());
        }
        self.local_status.lock().unwrap().remove(&account.id);
        let handle = backend.retry_sync(account);
        self.handles
            .lock()
            .unwrap()
            .insert(account.id.clone(), (account.provider, handle));
        tracing::info!(account = %account.id, "sync retried");
        Ok(())
    }

    pub fn poke(&self, account_id: &str) {
        if let Some((_, h)) = self.handles.lock().unwrap().get(account_id) {
            h.poke();
        }
    }

    async fn set_local_status(&self, account_id: &str, phase: SyncPhase, error: Option<String>) {
        let store = self.store.clone();
        let id = account_id.to_string();
        let indexed = blocking(move || Ok(store.count_messages(Some(&id))?))
            .await
            .unwrap_or(0);
        let status = SyncStatus {
            account_id: account_id.to_string(),
            phase,
            indexed,
            total_estimate: None,
            last_synced_at: None,
            error,
            rate_per_min: None,
            eta_secs: None,
            stage: None,
        };
        self.local_status
            .lock()
            .unwrap()
            .insert(account_id.to_string(), status.clone());
        self.emit_status(status);
    }

    /// Engine status for running accounts; the locally recorded status (or
    /// Idle) otherwise. `indexed` counts come from `counts` when the engine
    /// has not reported yet.
    pub fn statuses(&self, accounts: &[Account], counts: &HashMap<String, u64>) -> Vec<SyncStatus> {
        let handles = self.handles.lock().unwrap();
        let local = self.local_status.lock().unwrap();
        accounts
            .iter()
            .map(|a| {
                let from_engine = self
                    .registry
                    .get(a.provider)
                    .filter(|_| handles.contains_key(&a.id))
                    .and_then(|b| b.sync_status(&a.id));
                from_engine
                    .or_else(|| local.get(&a.id).cloned())
                    .unwrap_or_else(|| SyncStatus {
                        account_id: a.id.clone(),
                        phase: SyncPhase::Idle,
                        indexed: counts.get(&a.id).copied().unwrap_or(0),
                        total_estimate: None,
                        last_synced_at: None,
                        error: None,
                        rate_per_min: None,
                        eta_secs: None,
                        stage: None,
                    })
            })
            .collect()
    }

    /// Register a new sign-in; the receiver fires if it is cancelled (by
    /// cancel_sign_in or a newer sign-in).
    pub fn begin_sign_in(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Some(prev) = self.sign_in_cancel.lock().unwrap().replace(tx) {
            let _ = prev.send(());
        }
        rx
    }

    /// Returns whether a sign-in was pending.
    pub fn cancel_sign_in(&self) -> bool {
        match self.sign_in_cancel.lock().unwrap().take() {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    pub fn remember_saved_path(&self, path: PathBuf) {
        self.saved_paths.lock().unwrap().insert(path);
    }

    pub fn is_saved_path(&self, path: &std::path::Path) -> bool {
        self.saved_paths.lock().unwrap().contains(path)
    }

    pub fn emit_status(&self, status: SyncStatus) {
        emit(&self.app, EVENT_SYNC_STATUS, status);
    }

    /// Wait on this for local mail changes (a receiver subscribed now sees
    /// every change after this call, coalesced).
    pub fn mail_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.mail_changes.subscribe()
    }

    pub fn emit_mail_changed(&self, account_id: &str, thread_ids: Vec<String>) {
        bump(&self.mail_changes);
        emit(
            &self.app,
            EVENT_MAIL_CHANGED,
            MailChangedEvent {
                account_id: account_id.to_string(),
                thread_ids,
            },
        );
    }

    pub fn emit_action_failed(&self, message: String) {
        emit(
            &self.app,
            EVENT_ACTION_FAILED,
            ActionFailedEvent { message },
        );
    }

    pub fn emit_body_fetch_failed(&self, event: BodyFetchFailedEvent) {
        emit(&self.app, EVENT_BODY_FETCH_FAILED, event);
    }

    pub fn emit_settings_changed(&self, settings: Settings) {
        crate::app_menu::sync_settings(&self.app, &settings);
        emit(&self.app, EVENT_SETTINGS_CHANGED, settings);
    }
}
