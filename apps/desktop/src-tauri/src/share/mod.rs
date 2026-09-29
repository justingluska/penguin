//! Share links (docs/SHARE-LINKS.md): upload an attachment or a picture to
//! the user's own S3-compatible storage (Cloudflare R2, S3, MinIO, …) and
//! hand back a presigned GET link that expires.
//!
//! - Off until the user sets up storage in Settings → Share links. Nothing is
//!   uploaded except on an explicit "Copy Share Link" (or, when the user
//!   allows it, an agent's `create_share_link` request through
//!   [`share_attachment`]; see [`agent_gate`]).
//! - The bucket stays private: a link is a SigV4 presigned GET (1 hour, 24
//!   hours or 7 days, S3's maximum), and the key has 128 random bits
//!   (keys.rs). Whoever holds the link can download the file until it
//!   expires; nobody else can find or list it.
//! - One PUT per file, up to [`MAX_SHARE_BYTES`] (100 MB). Mail attachments
//!   stay far below it (Gmail caps a message at 25 MB), and every upload is
//!   already whole in memory (attachment cache, picture bytes), so multipart
//!   would add requests and failure modes for no file Penguin has.
//! - Every upload is recorded (uploads.rs); a background task deletes it
//!   once its link expired, when "Delete uploads when their links expire" is
//!   on (the default).
//! - Presigned URLs are bearer credentials and the secret is a credential:
//!   neither is ever logged, nor put in an error message.

pub mod config;
pub mod keys;
pub mod s3;
pub mod uploads;

#[cfg(test)]
pub(crate) mod stub;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use penguin_core::Store;
use penguin_provider::{async_trait, MailProvider};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops::Paths;
use crate::state::{blocking, AppState};
use config::{SecretStore, ShareLinkConfig, ShareLinkConfigInput, StoredConfig, Target};
use s3::{S3Error, Signer};
use uploads::{Upload, Uploads};

/// The largest file one share uploads (a single PUT; see the module docs).
pub const MAX_SHARE_BYTES: u64 = 100 * 1024 * 1024;
/// The first cleanup runs this long after launch, then every [`CLEANUP_EVERY`].
const CLEANUP_FIRST: Duration = Duration::from_secs(60);
const CLEANUP_EVERY: Duration = Duration::from_secs(10 * 60);

/// Who asked for a share. Agents (the CLI and MCP server, through the app)
/// are refused unless the user turned on "Let agents (CLI and MCP) create
/// share links": text inside an email can try to talk an agent into
/// publishing things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    User,
    Agent,
}

/// What to share. Attachments by id (bytes come from the attachment cache,
/// else the provider, exactly as Save does); a picture in a message body by
/// its source, the same `data:` image or public https URL Save takes (see
/// image_viewer.rs). Agents may only share attachments.
#[derive(Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ShareRequest {
    Attachment {
        account_id: String,
        message_id: String,
        attachment_id: String,
    },
    Picture {
        account_id: String,
        message_id: String,
        src: String,
        name: String,
    },
}

impl fmt::Debug for ShareRequest {
    // Never the picture's source: a data: URL is the picture, an https one
    // may carry a per-recipient token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShareRequest::Attachment {
                account_id,
                message_id,
                attachment_id,
            } => f
                .debug_struct("Attachment")
                .field("account_id", account_id)
                .field("message_id", message_id)
                .field("attachment_id", attachment_id)
                .finish(),
            ShareRequest::Picture {
                account_id,
                message_id,
                ..
            } => f
                .debug_struct("Picture")
                .field("account_id", account_id)
                .field("message_id", message_id)
                .finish_non_exhaustive(),
        }
    }
}

/// A share that worked. `url` is the link (a bearer credential: never log
/// it); `key` names the object for `share_delete`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedLink {
    pub url: String,
    /// Unix ms when the link stops working.
    pub expires_at: i64,
    pub key: String,
    /// The file's name (what a download is called).
    pub name: String,
    pub size: u64,
}

impl fmt::Debug for SharedLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedLink")
            .field("url", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// One step of the Settings test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestStep {
    /// upload | link | private | delete
    pub step: &'static str,
    /// ok | failed | warning | skipped
    pub status: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareTestReport {
    /// Upload, link and delete all worked.
    pub ok: bool,
    pub steps: Vec<TestStep>,
}

/// Share-link state: the settings, the Keychain secret, the upload records
/// and the HTTP client. Managed by Tauri next to AppState.
pub struct Share {
    config_dir: PathBuf,
    config: Mutex<StoredConfig>,
    secret: SecretStore,
    pub(crate) uploads: Uploads,
    http: reqwest::Client,
}

/// Ready to upload: the settings, where to, and a signer with the secret.
pub struct Prepared {
    pub config: StoredConfig,
    pub target: Target,
    pub signer: Signer,
}

impl Share {
    pub fn new(config_dir: PathBuf, data_dir: &std::path::Path, secret: SecretStore) -> Share {
        Share {
            config: Mutex::new(config::load(&config_dir)),
            config_dir,
            secret,
            uploads: Uploads::load(data_dir),
            http: s3::client(),
        }
    }

    /// The app's instance: files in the app config and data dirs, the
    /// secret in the macOS Keychain.
    pub fn init(state: &AppState) -> Arc<Share> {
        Arc::new(Share::new(
            state.paths.config_dir.clone(),
            &state.paths.data_dir,
            SecretStore::keychain(),
        ))
    }

    pub fn config(&self) -> StoredConfig {
        self.config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn store_config(&self, next: StoredConfig) -> CmdResult<()> {
        config::save(&self.config_dir, &next)
            .map_err(|e| CmdError::other(format!("Couldn't save the share-link settings: {e}")))?;
        *self.config.lock().unwrap_or_else(|p| p.into_inner()) = next;
        Ok(())
    }

    /// Save the form. The secret goes to the Keychain when one was typed;
    /// otherwise the saved one stays (and there must be one).
    pub fn set_config(&self, input: &ShareLinkConfigInput) -> CmdResult<ShareLinkConfig> {
        let target = config::validate_target(
            &input.endpoint,
            &input.bucket,
            &input.region,
            &input.access_key_id,
        )
        .map_err(CmdError::invalid)?;
        let current = self.config();
        match input.typed_secret() {
            Some(secret) => {
                config::validate_secret(secret).map_err(CmdError::invalid)?;
                self.secret.set(secret)?;
            }
            None if current.secret_saved => {}
            None => return Err(CmdError::invalid("Enter the secret access key")),
        }
        let next = StoredConfig {
            endpoint: target.endpoint_str(),
            bucket: target.bucket,
            region: target.region,
            access_key_id: target.access_key_id,
            lifetime: input.lifetime,
            delete_on_expiry: input.delete_on_expiry,
            allow_agents: input.allow_agents,
            secret_saved: true,
            ..current
        };
        self.store_config(next.clone())?;
        tracing::info!(
            lifetime_hours = next.lifetime.hours(),
            delete_on_expiry = next.delete_on_expiry,
            allow_agents = next.allow_agents,
            "share links: settings saved"
        );
        Ok(next.view())
    }

    /// Forget the storage: settings back to defaults, secret out of the
    /// Keychain. Upload records stay (they are forgotten 30 days after they
    /// expire; the lifecycle rule removes the files).
    pub fn clear_config(&self) -> CmdResult<ShareLinkConfig> {
        self.secret.clear()?;
        let next = StoredConfig::default();
        self.store_config(next.clone())?;
        tracing::info!("share links: storage removed");
        Ok(next.view())
    }

    /// Check the caller, the settings and the secret (Keychain: blocking).
    pub fn prepare(&self, caller: Caller) -> CmdResult<Prepared> {
        let config = self.config();
        check_caller(caller, &config)?;
        if !config.configured() {
            return Err(CmdError::new(
                ErrorCode::NotConfigured,
                "Share links aren't set up. Add your storage in Settings → Share links.",
            ));
        }
        let target = config::validate_target(
            &config.endpoint,
            &config.bucket,
            &config.region,
            &config.access_key_id,
        )
        .map_err(CmdError::invalid)?;
        let secret = self.secret.get()?.ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotConfigured,
                "The share-link secret is missing from the Keychain. Enter it again in Settings → Share links.",
            )
        })?;
        let signer = Signer::new(&target, &secret).map_err(CmdError::invalid)?;
        Ok(Prepared {
            config,
            target,
            signer,
        })
    }

    /// Upload `bytes` as a new object and return its link. The upload is
    /// recorded first (so a crash mid-upload is still cleaned up) and the
    /// record dropped again if the upload fails.
    pub async fn upload(
        &self,
        prepared: &Prepared,
        name: &str,
        content_type: &str,
        bytes: Vec<u8>,
        now: SystemTime,
    ) -> CmdResult<SharedLink> {
        let size = bytes.len() as u64;
        if size > MAX_SHARE_BYTES {
            return Err(too_large(size));
        }
        let key = keys::object_key(name)
            .ok_or_else(|| CmdError::other("This Mac couldn't make a random name for the file"))?;
        let content_type = keys::content_type(content_type);
        let disposition = keys::content_disposition(name, &content_type);
        let lifetime = prepared.config.lifetime;
        let created_at = unix_ms(now);
        let expires_at = created_at + (lifetime.seconds() * 1000) as i64;
        let record = Upload {
            key: key.clone(),
            endpoint: prepared.target.endpoint_str(),
            bucket: prepared.target.bucket.clone(),
            created_at,
            expires_at,
            attempts: 0,
            next_try_at: 0,
        };
        let uploads = &self.uploads;
        if let Err(e) = uploads.add(record) {
            return Err(CmdError::other(format!("Couldn't record the upload: {e}")));
        }
        if let Err(e) = s3::put(
            &self.http,
            &prepared.signer,
            &key,
            bytes,
            &content_type,
            &disposition,
        )
        .await
        {
            if let Err(io) = uploads.remove(&key) {
                tracing::warn!(error = %io, "share links: couldn't update share-uploads.json");
            }
            return Err(s3_error(&e, &prepared.target.bucket));
        }
        let url = prepared
            .signer
            .get_url(&key, Duration::from_secs(lifetime.seconds()), now);
        tracing::info!(size, lifetime_hours = lifetime.hours(), kind = %content_type, "share link created");
        Ok(SharedLink {
            url: url.into(),
            expires_at,
            key,
            name: crate::ops::sanitize_filename(name),
            size,
        })
    }

    /// Delete one of Penguin's uploads now (the toast's "Delete now").
    pub async fn delete(self: &Arc<Share>, key: &str) -> CmdResult<()> {
        if !keys::is_share_key(key) {
            return Err(CmdError::invalid("That isn't a file Penguin shared"));
        }
        let record = self.uploads.get(key).ok_or_else(|| {
            CmdError::not_found("Penguin has no record of this upload (it may already be deleted)")
        })?;
        let prepared = self.prepare_blocking(Caller::User).await?;
        if record.endpoint != prepared.target.endpoint_str()
            || record.bucket != prepared.target.bucket
        {
            return Err(CmdError::invalid(
                "This file is in storage that's no longer set up in Penguin. Delete it in the storage's dashboard.",
            ));
        }
        s3::delete(&self.http, &prepared.signer, key)
            .await
            .map_err(|e| s3_error(&e, &prepared.target.bucket))?;
        self.uploads.remove(key).map_err(|e| {
            CmdError::other(format!("Deleted, but couldn't update the upload list: {e}"))
        })?;
        tracing::info!("share link deleted");
        Ok(())
    }

    /// [`Share::prepare`] on the blocking pool (the first Keychain read may
    /// wait on a prompt).
    pub async fn prepare_blocking(self: &Arc<Share>, caller: Caller) -> CmdResult<Prepared> {
        let share = self.clone();
        blocking(move || share.prepare(caller)).await
    }

    /// One cleanup pass (see uploads.rs). Reads the Keychain only when an
    /// upload is due for deletion.
    pub async fn sweep_once(self: &Arc<Share>, now: SystemTime) -> uploads::Sweep {
        let now_ms = unix_ms(now);
        let config = self.config();
        let target = (config.delete_on_expiry && config.configured())
            .then(|| {
                config::validate_target(
                    &config.endpoint,
                    &config.bucket,
                    &config.region,
                    &config.access_key_id,
                )
                .ok()
            })
            .flatten();
        let signer = match &target {
            Some(t)
                if !uploads::due(
                    &self.uploads.snapshot(),
                    now_ms,
                    &t.endpoint_str(),
                    &t.bucket,
                )
                .is_empty() =>
            {
                let share = self.clone();
                match blocking(move || share.secret.get()).await {
                    Ok(Some(secret)) => Signer::new(t, &secret).ok(),
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!(error = %e.message, "share links: cleanup can't read the secret; will retry");
                        None
                    }
                }
            }
            _ => None,
        };
        match (&target, &signer) {
            (Some(t), Some(signer)) => {
                let endpoint = t.endpoint_str();
                uploads::sweep(
                    &self.uploads,
                    now_ms,
                    Some((&endpoint, &t.bucket)),
                    |u: Upload| async move { s3::delete(&self.http, signer, &u.key).await },
                )
                .await
            }
            _ => {
                uploads::sweep(&self.uploads, now_ms, None, |_: Upload| async {
                    Ok::<(), S3Error>(())
                })
                .await
            }
        }
    }

    /// The Settings test: upload a small file, fetch it through a presigned
    /// link, check the bucket refuses it without one, delete it. Each step
    /// is reported in words; a failed step skips the ones that need it.
    pub async fn test(&self, target: &Target, secret: &str) -> ShareTestReport {
        let signer = match Signer::new(target, secret) {
            Ok(s) => s,
            Err(message) => {
                return ShareTestReport {
                    ok: false,
                    steps: vec![step("upload", "failed", message)],
                }
            }
        };
        run_test(&self.http, &signer, &target.bucket).await
    }
}

/// Agents need the user's say-so ("Let agents (CLI and MCP) create share links").
pub fn check_caller(caller: Caller, config: &StoredConfig) -> CmdResult<()> {
    match caller {
        Caller::User => Ok(()),
        Caller::Agent => agent_gate(config),
    }
}

/// Whether agents may create share links now: storage is set up and the
/// user turned on "Let agents (CLI and MCP) create share links". Reads
/// share-links.json only (never the Keychain), so the MCP server can hide
/// `create_share_link` without going near the secret.
pub fn agents_allowed(config: &StoredConfig) -> bool {
    agent_gate(config).is_ok()
}

/// The share-link half of an agent's `create_share_link` (the agent level
/// is the other half, agent/permission.rs): a permission error that names
/// what the user has to turn on, and where.
pub fn agent_gate(config: &StoredConfig) -> CmdResult<()> {
    if !config.configured() {
        return Err(CmdError::denied(
            "Share links aren't set up, so agents can't create them. The user can add storage in \
             Penguin → Settings → Share links, then turn on \"Let agents (CLI and MCP) create share \
             links\" there.",
        ));
    }
    if !config.allow_agents {
        return Err(CmdError::denied(
            "Share links for agents are off. The user can turn on \"Let agents (CLI and MCP) create \
             share links\" in Penguin → Settings → Share links.",
        ));
    }
    Ok(())
}

fn step(step: &'static str, status: &'static str, message: impl Into<String>) -> TestStep {
    TestStep {
        step,
        status,
        message: message.into(),
    }
}

const TEST_BODY: &[u8] =
    b"Penguin share-link test. It is deleted right after the test; safe to delete.\n";

pub(crate) async fn run_test(
    http: &reqwest::Client,
    signer: &Signer,
    bucket: &str,
) -> ShareTestReport {
    let mut steps = Vec::new();
    let Some(token) = keys::token() else {
        return ShareTestReport {
            ok: false,
            steps: vec![step(
                "upload",
                "failed",
                "This Mac couldn't make a random file name",
            )],
        };
    };
    let key = format!("{}{token}.txt", keys::TEST_PREFIX);
    let disposition = keys::content_disposition("penguin-test.txt", "text/plain");
    if let Err(e) = s3::put(
        http,
        signer,
        &key,
        TEST_BODY.to_vec(),
        "text/plain",
        &disposition,
    )
    .await
    {
        steps.push(step(
            "upload",
            "failed",
            format!("Upload failed. {}", e.message(bucket)),
        ));
        for s in ["link", "private", "delete"] {
            steps.push(step(s, "skipped", "Skipped: nothing was uploaded"));
        }
        return ShareTestReport { ok: false, steps };
    }
    steps.push(step(
        "upload",
        "ok",
        format!("Uploaded a small test file to {bucket}"),
    ));

    let link = signer.get_url(&key, Duration::from_secs(120), SystemTime::now());
    let link_ok = match s3::get(http, signer.host(), link, TEST_BODY.len() + 1).await {
        Ok(body) if body == TEST_BODY => {
            steps.push(step("link", "ok", "Downloaded it through a share link"));
            true
        }
        Ok(_) => {
            steps.push(step(
                "link",
                "failed",
                "The share link returned something other than the test file",
            ));
            false
        }
        Err(e) => {
            steps.push(step(
                "link",
                "failed",
                format!("The share link didn't work. {}", e.message(bucket)),
            ));
            false
        }
    };

    match signer.unsigned_url(&key) {
        Ok(url) => match s3::status(http, signer.host(), url).await {
            Ok(status) if (200..300).contains(&status) => steps.push(step(
                "private",
                "warning",
                "Anyone can download files from this bucket without a link. Make the bucket private (for R2: turn off public access and the r2.dev URL).",
            )),
            Ok(_) => steps.push(step("private", "ok", "The bucket is private: files open only through a link")),
            Err(e) => steps.push(step("private", "skipped", format!("Couldn't check. {}", e.message(bucket)))),
        },
        Err(_) => steps.push(step("private", "skipped", "Couldn't check")),
    }

    let delete_ok = match s3::delete(http, signer, &key).await {
        Ok(()) => {
            steps.push(step("delete", "ok", "Deleted the test file"));
            true
        }
        Err(e) => {
            steps.push(step(
                "delete",
                "failed",
                format!(
                    "Couldn't delete the test file ({key}), so Penguin can't delete expired uploads either. {}",
                    e.message(bucket)
                ),
            ));
            false
        }
    };
    ShareTestReport {
        ok: link_ok && delete_ok,
        steps,
    }
}

fn too_large(size: u64) -> CmdError {
    CmdError::invalid(format!(
        "This file is {} MB; share links take files up to {} MB",
        size.div_ceil(1024 * 1024),
        MAX_SHARE_BYTES / (1024 * 1024)
    ))
}

fn s3_error(e: &S3Error, bucket: &str) -> CmdError {
    let code = match e {
        S3Error::Unreachable { .. } => ErrorCode::Network,
        S3Error::Status { status, .. } if *status >= 500 || *status == 429 => ErrorCode::Network,
        _ => ErrorCode::Other,
    };
    CmdError::new(code, e.message(bucket))
}

fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Where a share's bytes come from: the index (which attachment), the
/// attachment cache and the account's provider. The app's [`AppState`] for
/// Copy Share Link; the agent service's host for agents (agent/sharing.rs).
#[async_trait]
pub trait ShareSource: Send + Sync {
    fn store(&self) -> &Store;
    fn paths(&self) -> &Paths;
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>>;
}

#[async_trait]
impl ShareSource for AppState {
    fn store(&self) -> &Store {
        &self.store
    }
    fn paths(&self) -> &Paths {
        &self.paths
    }
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>> {
        AppState::provider(self, account_id).await
    }
}

/// A file's name, declared type and bytes for `request`.
async fn request_bytes<S: ShareSource + ?Sized>(
    source: &S,
    request: &ShareRequest,
) -> CmdResult<(String, String, Vec<u8>)> {
    match request {
        ShareRequest::Attachment {
            account_id,
            message_id,
            attachment_id,
        } => {
            let store = source.store().clone();
            let (acct, mid, aid) = (
                account_id.clone(),
                message_id.clone(),
                attachment_id.clone(),
            );
            // Refuse an oversized file before downloading it.
            let meta =
                blocking(move || crate::attachments::find(&store, &acct, &mid, &aid)).await?;
            if meta.size > MAX_SHARE_BYTES {
                return Err(too_large(meta.size));
            }
            // The attachment cache, else the provider (then cached), as Save does.
            let provider = source.provider(account_id).await?;
            let bytes = crate::attachments::bytes(
                source.paths(),
                provider.as_ref(),
                account_id,
                message_id,
                &meta,
            )
            .await?;
            Ok((meta.filename, meta.mime_type, bytes))
        }
        ShareRequest::Picture { src, name, .. } => {
            let (bytes, mime) = crate::image_viewer::image_bytes(src).await?;
            Ok((
                crate::image_viewer::file_name(name, mime),
                mime.to_string(),
                bytes,
            ))
        }
    }
}

/// Share an attachment or a body picture: the one entry point for the UI's
/// "Copy Share Link" (`share_file`, [`Caller::User`]) and for agents
/// ([`Caller::Agent`]: `create_share_link` from the MCP server or
/// `penguin-cli share-link`, answered by the app over the agent socket,
/// agent/sharing.rs). Agents are refused unless storage is set up and the
/// user allowed them ([`agent_gate`]), and may name only attachments
/// (embedded cid: pictures included, by account, message and attachment
/// id), never a picture by URL.
pub async fn share_attachment<S: ShareSource + ?Sized>(
    source: &S,
    share: &Arc<Share>,
    caller: Caller,
    request: ShareRequest,
) -> CmdResult<SharedLink> {
    if caller == Caller::Agent && matches!(request, ShareRequest::Picture { .. }) {
        return Err(CmdError::denied(
            "Agents can share a message's attachments and embedded (cid:) pictures, never a picture by its web address",
        ));
    }
    // Settings, the caller gate and the secret before any bytes are fetched.
    let prepared = share.prepare_blocking(caller).await?;
    let (name, content_type, bytes) = request_bytes(source, &request).await?;
    share
        .upload(&prepared, &name, &content_type, bytes, SystemTime::now())
        .await
}

/// Start the expiry cleanup (uploads.rs): first pass a minute after launch,
/// then every ten minutes. Never blocks a command.
pub fn spawn_cleanup(share: Arc<Share>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CLEANUP_FIRST).await;
        loop {
            let swept = share.sweep_once(SystemTime::now()).await;
            if swept != uploads::Sweep::default() {
                tracing::info!(
                    deleted = swept.deleted,
                    failed = swept.failed,
                    forgotten = swept.forgotten,
                    "share links: cleanup"
                );
            }
            tokio::time::sleep(CLEANUP_EVERY).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

type ShareRef<'a> = State<'a, Arc<Share>>;

#[tauri::command]
pub fn share_link_config_get(share: ShareRef<'_>) -> CmdResult<ShareLinkConfig> {
    Ok(share.config().view())
}

#[tauri::command]
pub async fn share_link_config_set(
    share: ShareRef<'_>,
    config: ShareLinkConfigInput,
) -> CmdResult<ShareLinkConfig> {
    let share = share.inner().clone();
    blocking(move || share.set_config(&config)).await
}

#[tauri::command]
pub async fn share_link_config_clear(share: ShareRef<'_>) -> CmdResult<ShareLinkConfig> {
    let share = share.inner().clone();
    blocking(move || share.clear_config()).await
}

#[tauri::command]
pub async fn share_link_config_test(
    share: ShareRef<'_>,
    config: ShareLinkConfigInput,
) -> CmdResult<ShareTestReport> {
    let target = config::validate_target(
        &config.endpoint,
        &config.bucket,
        &config.region,
        &config.access_key_id,
    )
    .map_err(CmdError::invalid)?;
    let secret = match config.typed_secret() {
        Some(s) => {
            config::validate_secret(s).map_err(CmdError::invalid)?;
            s.to_string()
        }
        None => {
            let share = share.inner().clone();
            blocking(move || share.secret.get())
                .await?
                .ok_or_else(|| CmdError::invalid("Enter the secret access key"))?
        }
    };
    let report = share.test(&target, &secret).await;
    tracing::info!(ok = report.ok, "share links: storage tested");
    Ok(report)
}

#[tauri::command]
pub async fn share_file(
    state: State<'_, Arc<AppState>>,
    share: ShareRef<'_>,
    request: ShareRequest,
) -> CmdResult<SharedLink> {
    share_attachment(state.inner().as_ref(), share.inner(), Caller::User, request).await
}

#[tauri::command]
pub async fn share_delete(share: ShareRef<'_>, key: String) -> CmdResult<()> {
    share.inner().delete(&key).await
}
