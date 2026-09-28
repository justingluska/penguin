//! Tauri side of the avatar service: commands, the resolve worker, and the
//! daily contacts sync. Managed as `State<Arc<Avatars>>` next to AppState.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use penguin_gmail::auth::{CONTACTS_OTHER_READONLY_SCOPE, CONTACTS_READONLY_SCOPE};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::cache::Cache;
use super::net::HttpNet;
use super::people::{self, ContactPhotos, SyncError};
use super::{now_secs, AvatarInfo, AvatarRequest, Avatars, Resolver, EVENT_AVATARS_CHANGED};
use crate::error::{CmdError, CmdResult};
use crate::state::{blocking, AppState};

/// Network resolves in flight at once.
const CONCURRENCY: usize = 4;
/// Jobs taken per worker round (one store query covers them all).
const BATCH: usize = 64;
/// Contact photo maps are refreshed after this long.
const CONTACTS_TTL: u64 = 24 * 3600;
const MAX_LOOKUP: usize = 500;

pub const CONTACT_SCOPES: [&str; 2] = [CONTACTS_OTHER_READONLY_SCOPE, CONTACTS_READONLY_SCOPE];

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactsState {
    /// Unix ms of the last successful sync.
    pub synced_at: Option<i64>,
    pub photos: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPhotos {
    pub account_id: String,
    /// Both contacts scopes are granted for this account.
    pub contacts_granted: bool,
    pub synced_at: Option<i64>,
    pub photos: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarStatus {
    pub accounts: Vec<AccountPhotos>,
    /// Cached images and their size on disk.
    pub images: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarsChanged {
    /// Addresses whose answer changed; empty with `all`.
    pub emails: Vec<String>,
    /// Everything may have changed (cache cleared, contacts synced).
    pub all: bool,
}

pub fn init(state: &AppState) -> Arc<Avatars> {
    let cache = Cache::new(state.paths.cache_dir.join("avatars"));
    Arc::new(Avatars::new(Resolver::new(cache, Arc::new(HttpNet::new()))))
}

fn contacts_dir(state: &AppState) -> std::path::PathBuf {
    state.paths.cache_dir.join("avatars").join("contacts")
}

fn emit(app: &AppHandle, payload: AvatarsChanged) {
    if let Err(e) = app.emit(EVENT_AVATARS_CHANGED, payload) {
        tracing::warn!(error = %e, "failed to emit avatars-changed");
    }
}

/// Start the resolve worker and the contacts sync loop.
pub fn spawn(app: AppHandle, state: Arc<AppState>, avatars: Arc<Avatars>) {
    {
        let (app, state, avatars) = (app.clone(), state.clone(), avatars.clone());
        tauri::async_runtime::spawn(async move {
            // Load maps from the last sync and tidy the image dir first.
            let (st, av) = (state.clone(), avatars.clone());
            let _ = blocking(move || {
                load_contacts(&st, &av);
                if let Err(e) = av.resolver.cache.gc() {
                    tracing::debug!(error = %e, "avatar cache gc failed");
                }
                Ok(())
            })
            .await;
            worker(app, state, avatars).await;
        });
    }
    tauri::async_runtime::spawn(contacts_loop(app, state, avatars));
}

async fn worker(app: AppHandle, state: Arc<AppState>, avatars: Arc<Avatars>) {
    let limit = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    loop {
        let jobs = avatars.take(BATCH);
        if jobs.is_empty() {
            avatars.wake.notified().await;
            continue;
        }
        let now = now_secs();
        let prefs = state.settings.get().avatar_prefs();

        // One store scan for every address whose authentication is unknown.
        let unknown: Vec<String> = jobs
            .iter()
            .filter(|j| j.authenticated.is_none() && avatars.needs_evidence(&j.email, now))
            .map(|j| j.email.clone())
            .collect();
        if !unknown.is_empty() {
            let store = state.store.clone();
            let asked = unknown.clone();
            match blocking(move || Ok(store.latest_sender_authenticated(&asked)?)).await {
                Ok(found) => avatars.set_evidence(found, &unknown, now),
                Err(e) => tracing::warn!(error = %e, "sender authentication lookup failed"),
            }
        }

        let mut set = tokio::task::JoinSet::new();
        for job in jobs {
            let (avatars, limit) = (avatars.clone(), limit.clone());
            set.spawn(async move {
                let _permit = limit.acquire_owned().await;
                let authed = avatars.authenticated_for(&job, now);
                let changed = avatars
                    .resolver
                    .resolve(&job.email, &prefs, authed, now)
                    .await;
                avatars.done(&job);
                changed.then_some(job.email)
            });
        }
        let mut changed = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok(Some(email)) = res {
                changed.push(email);
            }
        }
        let av = avatars.clone();
        let _ = blocking(move || {
            if let Err(e) = av.resolver.cache.flush() {
                tracing::warn!(error = %e, "could not write the avatar index");
            }
            Ok(())
        })
        .await;
        if !changed.is_empty() {
            changed.sort();
            changed.dedup();
            emit(
                &app,
                AvatarsChanged {
                    emails: changed,
                    all: false,
                },
            );
        }
    }
}

/// Merge every signed-in account's stored map into the resolver. Blocking.
fn load_contacts(state: &AppState, avatars: &Avatars) {
    let accounts = state.store.list_accounts().unwrap_or_default();
    let dir = contacts_dir(state);
    let mut merged = HashMap::new();
    let mut status = avatars
        .contacts_state
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for a in &accounts {
        if let Some(file) = people::load(&people::file_for(&dir, &a.id)) {
            let entry = status.entry(a.id.clone()).or_default();
            entry.synced_at = Some(file.synced_at as i64 * 1000);
            entry.photos = file.photos.len();
            merged.extend(file.photos);
        }
    }
    // Files of removed accounts go.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let keep: Vec<_> = accounts
            .iter()
            .map(|a| people::file_for(&dir, &a.id))
            .collect();
        for e in entries.flatten() {
            if !keep.contains(&e.path()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    status.retain(|id, _| accounts.iter().any(|a| &a.id == id));
    drop(status);
    avatars.resolver.set_contacts(merged);
}

fn has_contact_scopes(granted: &[String]) -> bool {
    CONTACT_SCOPES
        .iter()
        .all(|s| granted.iter().any(|g| g == s))
}

/// Sync accounts whose map is older than a day (or `force`d ones).
async fn sync_contacts(
    app: &AppHandle,
    state: &Arc<AppState>,
    avatars: &Arc<Avatars>,
    force: Option<&str>,
) {
    if !state.settings.get().sender_photos.contacts {
        return;
    }
    let Ok(services) = state.services() else {
        return;
    };
    let Ok(accounts) = state.accounts().await else {
        return;
    };
    let dir = contacts_dir(state);
    let now = now_secs();
    let mut any = false;
    // Google contacts: Google accounts only.
    for account in accounts
        .into_iter()
        .filter(|a| a.capabilities.contact_photos)
    {
        let auth = services.auth.clone();
        let email = account.email.clone();
        let granted = blocking(move || Ok(auth.granted_scopes(&email)))
            .await
            .unwrap_or_default();
        if !has_contact_scopes(&granted) {
            continue;
        }
        let path = people::file_for(&dir, &account.id);
        let p = path.clone();
        let existing = blocking(move || Ok(people::load(&p))).await.unwrap_or(None);
        let due = force == Some(account.id.as_str())
            || existing
                .as_ref()
                .is_none_or(|f| now >= f.synced_at + CONTACTS_TTL);
        if !due {
            continue;
        }
        let token = match services.auth.access_token(&account.email).await {
            Ok(t) => t,
            Err(e) => {
                set_contacts_error(avatars, &account.id, e.to_string());
                continue;
            }
        };
        let net = HttpNet::new();
        match people::sync(&net, &token).await {
            Ok(photos) => {
                let file = ContactPhotos {
                    synced_at: now,
                    photos,
                };
                let count = file.photos.len();
                let p = path.clone();
                if let Err(e) = blocking(move || {
                    people::save(&p, &file).map_err(|e| CmdError::other(e.to_string()))
                })
                .await
                {
                    tracing::warn!(error = %e, "could not save contact photos");
                }
                let mut st = avatars
                    .contacts_state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                st.insert(
                    account.id.clone(),
                    ContactsState {
                        synced_at: Some(now as i64 * 1000),
                        photos: count,
                        error: None,
                    },
                );
                tracing::info!(account = %account.id, photos = count, "contact photos synced");
                any = true;
            }
            Err(SyncError::NotGranted) => set_contacts_error(
                avatars,
                &account.id,
                "Google didn't allow contact access; connect contact photos again".into(),
            ),
            Err(SyncError::ApiDisabled) => set_contacts_error(
                avatars,
                &account.id,
                "The People API is off in your Google Cloud project. Enable it, then try again"
                    .into(),
            ),
            Err(SyncError::Failed(e)) => set_contacts_error(avatars, &account.id, e),
        }
    }
    if any {
        let (st, av) = (state.clone(), avatars.clone());
        let _ = blocking(move || {
            load_contacts(&st, &av);
            Ok(())
        })
        .await;
        emit(
            app,
            AvatarsChanged {
                emails: vec![],
                all: true,
            },
        );
    }
}

fn set_contacts_error(avatars: &Avatars, account_id: &str, error: String) {
    tracing::warn!(account = %account_id, error = %error, "contact photo sync failed");
    avatars
        .contacts_state
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(account_id.to_string())
        .or_default()
        .error = Some(error);
}

async fn contacts_loop(app: AppHandle, state: Arc<AppState>, avatars: Arc<Avatars>) {
    // Let startup sync go first.
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        sync_contacts(&app, &state, &avatars, None).await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3600)) => {}
            _ = avatars.contacts_wake.notified() => {}
        }
    }
}

// ---------- commands ----------

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type AvatarsRef<'a> = State<'a, Arc<Avatars>>;

/// Avatars for the given senders, from memory only (never waits on the
/// network). Unknown senders are resolved in the background and announced
/// with `penguin://avatars-changed`.
#[tauri::command]
pub async fn avatar_lookup(
    state: AppStateRef<'_>,
    avatars: AvatarsRef<'_>,
    requests: Vec<AvatarRequest>,
) -> CmdResult<Vec<AvatarInfo>> {
    if requests.len() > MAX_LOOKUP {
        return Err(CmdError::invalid(format!(
            "at most {MAX_LOOKUP} avatars per call"
        )));
    }
    let prefs = state.settings.get().avatar_prefs();
    let av = avatars.inner().clone();
    // The first call may read the index file; keep it off the async threads.
    blocking(move || Ok(av.lookup(&requests, &prefs, now_secs()))).await
}

async fn status(state: &Arc<AppState>, avatars: &Arc<Avatars>) -> CmdResult<AvatarStatus> {
    let accounts = state.accounts().await?;
    let services = state.services().ok();
    let av = avatars.clone();
    blocking(move || {
        let (_, images, bytes) = av.resolver.cache.stats();
        let st = av.contacts_state.lock().unwrap_or_else(|p| p.into_inner());
        let accounts = accounts
            .iter()
            .map(|a| {
                let s = st.get(&a.id).cloned().unwrap_or_default();
                AccountPhotos {
                    account_id: a.id.clone(),
                    contacts_granted: services
                        .as_ref()
                        .is_some_and(|sv| has_contact_scopes(&sv.auth.granted_scopes(&a.email))),
                    synced_at: s.synced_at,
                    photos: s.photos,
                    error: s.error,
                }
            })
            .collect();
        Ok(AvatarStatus {
            accounts,
            images,
            bytes,
        })
    })
    .await
}

#[tauri::command]
pub async fn avatar_status(
    state: AppStateRef<'_>,
    avatars: AvatarsRef<'_>,
) -> CmdResult<AvatarStatus> {
    let status = status(state.inner(), avatars.inner()).await?;
    // Contact photos just switched on (Settings shows this status): sync
    // connected accounts now rather than at the next hourly check.
    if state.settings.get().sender_photos.contacts
        && status
            .accounts
            .iter()
            .any(|a| a.contacts_granted && a.synced_at.is_none() && a.error.is_none())
    {
        avatars.contacts_wake.notify_one();
    }
    Ok(status)
}

/// "Connect contact photos": a browser sign-in for this account asking for
/// the two read-only contacts scopes on top of what it already granted
/// (incremental authorization), then an immediate contacts sync.
#[tauri::command]
pub async fn connect_contact_photos(
    app: AppHandle,
    state: AppStateRef<'_>,
    avatars: AvatarsRef<'_>,
    account_id: String,
) -> CmdResult<AvatarStatus> {
    let account = state.account(&account_id).await?;
    if !account.capabilities.contact_photos {
        return Err(CmdError::invalid(
            "Contact photos are available for Google accounts only",
        ));
    }
    let services = state.services()?;
    let signed =
        crate::sign_in::interactive_sign_in(&app, &state, Some(&account.email), &CONTACT_SCOPES)
            .await?;
    if !signed.email.eq_ignore_ascii_case(&account.email) {
        return Err(CmdError::invalid(format!(
            "You signed in as {}; choose {} to connect its contact photos",
            signed.email, account.email
        )));
    }
    let auth = services.auth.clone();
    let email = account.email.clone();
    let granted = blocking(move || Ok(auth.granted_scopes(&email))).await?;
    if !has_contact_scopes(&granted) {
        return Err(CmdError::invalid(
            "Contact access wasn't granted. Try again and leave both contacts boxes ticked on Google's screen",
        ));
    }
    tracing::info!(account = %account.id, "contact photos connected");
    // Connecting is an explicit opt-in: make sure the source is on.
    if !state.settings.get().sender_photos.contacts {
        let st = state.inner().clone();
        let saved = blocking(move || {
            let mut photos = st.settings.get().sender_photos;
            photos.contacts = true;
            Ok(st.settings.update(crate::settings::SettingsPatch {
                sender_photos: Some(photos),
                ..Default::default()
            })?)
        })
        .await?;
        state.emit_settings_changed(saved);
    }
    sync_contacts(&app, state.inner(), avatars.inner(), Some(&account.id)).await;
    status(state.inner(), avatars.inner()).await
}

/// Settings → Privacy → "Clear photo cache": every cached image, miss and
/// contacts map. Contacts are re-synced right away when enabled.
#[tauri::command]
pub async fn clear_avatar_cache(
    app: AppHandle,
    state: AppStateRef<'_>,
    avatars: AvatarsRef<'_>,
) -> CmdResult<AvatarStatus> {
    let av = avatars.inner().clone();
    blocking(move || {
        av.clear()
            .map_err(|e| CmdError::other(format!("could not clear the photo cache: {e}")))
    })
    .await?;
    tracing::info!("avatar cache cleared");
    emit(
        &app,
        AvatarsChanged {
            emails: vec![],
            all: true,
        },
    );
    avatars.contacts_wake.notify_one();
    status(state.inner(), avatars.inner()).await
}
