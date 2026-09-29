//! The share-link storage settings (Settings → Share links).
//!
//! Everything but the secret is plain JSON in the app config dir
//! (`share-links.json`, never in settings.json). The secret access key is one
//! generic-password item in the macOS Keychain (service [`KEYCHAIN_SERVICE`],
//! account [`KEYCHAIN_ACCOUNT`]), behind the same `SecretBackend` seam the
//! IMAP and Microsoft vaults use, so tests swap in a memory backend. The
//! secret is never logged, never returned to the UI and never in an error.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use penguin_provider::credentials::{KeyringBackend, SecretBackend};
use serde::{Deserialize, Serialize};

use crate::error::{CmdError, CmdResult};

pub const KEYCHAIN_SERVICE: &str = "co.gluska.penguin.share-links";
pub const KEYCHAIN_ACCOUNT: &str = "secret-access-key.v1";
const FILE_NAME: &str = "share-links.json";
const FILE_VERSION: u32 = 1;

/// How long a link works. SigV4 presigned URLs can't outlive 7 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LinkLifetime {
    #[serde(rename = "1h")]
    Hour,
    #[default]
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
}

impl LinkLifetime {
    pub fn seconds(self) -> u64 {
        match self {
            LinkLifetime::Hour => 3600,
            LinkLifetime::Day => 24 * 3600,
            LinkLifetime::Week => 7 * 24 * 3600,
        }
    }
    pub fn hours(self) -> u64 {
        self.seconds() / 3600
    }
}

/// What `share-links.json` holds. `secret_saved` says the Keychain has the
/// secret, so the UI (and a menu) can tell "set up" without reading the
/// Keychain, which may show a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredConfig {
    pub version: u32,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub lifetime: LinkLifetime,
    pub delete_on_expiry: bool,
    pub allow_agents: bool,
    pub secret_saved: bool,
}

impl Default for StoredConfig {
    fn default() -> Self {
        StoredConfig {
            version: FILE_VERSION,
            endpoint: String::new(),
            bucket: String::new(),
            region: DEFAULT_REGION.into(),
            access_key_id: String::new(),
            lifetime: LinkLifetime::Day,
            delete_on_expiry: true,
            allow_agents: false,
            secret_saved: false,
        }
    }
}

pub const DEFAULT_REGION: &str = "auto";

impl StoredConfig {
    /// Every field is filled in and the secret was saved.
    pub fn configured(&self) -> bool {
        self.secret_saved
            && validate_target(
                &self.endpoint,
                &self.bucket,
                &self.region,
                &self.access_key_id,
            )
            .is_ok()
    }
    pub fn view(&self) -> ShareLinkConfig {
        ShareLinkConfig {
            configured: self.configured(),
            endpoint: self.endpoint.clone(),
            bucket: self.bucket.clone(),
            region: self.region.clone(),
            access_key_id: self.access_key_id.clone(),
            has_secret: self.secret_saved,
            lifetime: self.lifetime,
            delete_on_expiry: self.delete_on_expiry,
            allow_agents: self.allow_agents,
        }
    }
}

/// share_link_config_get / _set / _clear: the settings as the UI shows them.
/// No secret, only whether one is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareLinkConfig {
    pub configured: bool,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub has_secret: bool,
    pub lifetime: LinkLifetime,
    pub delete_on_expiry: bool,
    pub allow_agents: bool,
}

/// share_link_config_set / _test: the form. An empty or missing
/// `secretAccessKey` keeps (or tests with) the saved one.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkConfigInput {
    pub endpoint: String,
    pub bucket: String,
    #[serde(default)]
    pub region: String,
    pub access_key_id: String,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub lifetime: LinkLifetime,
    #[serde(default = "yes")]
    pub delete_on_expiry: bool,
    #[serde(default)]
    pub allow_agents: bool,
}

fn yes() -> bool {
    true
}

impl fmt::Debug for ShareLinkConfigInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShareLinkConfigInput")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "<redacted>"),
            )
            .field("lifetime", &self.lifetime)
            .field("delete_on_expiry", &self.delete_on_expiry)
            .field("allow_agents", &self.allow_agents)
            .finish()
    }
}

impl ShareLinkConfigInput {
    /// The typed secret, when one was typed (trimmed; blank means none).
    pub fn typed_secret(&self) -> Option<&str> {
        self.secret_access_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// Where requests go: a checked endpoint, bucket, region and key id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub endpoint: url::Url,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
}

impl Target {
    /// The endpoint as stored and compared (no trailing slash).
    pub fn endpoint_str(&self) -> String {
        self.endpoint.as_str().trim_end_matches('/').to_string()
    }
}

fn is_loopback(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// Check the storage fields; the error names the field and what's wrong.
pub fn validate_target(
    endpoint: &str,
    bucket: &str,
    region: &str,
    access_key_id: &str,
) -> Result<Target, String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(
            "Enter the storage's endpoint, like https://<account id>.r2.cloudflarestorage.com"
                .into(),
        );
    }
    let url = url::Url::parse(endpoint)
        .map_err(|_| "The endpoint isn't a web address. It looks like https://<account id>.r2.cloudflarestorage.com".to_string())?;
    let host = url.host().ok_or("The endpoint has no host name")?;
    match url.scheme() {
        "https" => {}
        // A local S3-compatible server (MinIO) for testing.
        "http" if is_loopback(&host) => {}
        "http" => return Err("The endpoint must start with https://".into()),
        _ => return Err("The endpoint must start with https://".into()),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Leave the key out of the endpoint; it has its own fields".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("The endpoint is only https:// and the host, with nothing after it".into());
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err(
            "The endpoint is only https:// and the host: put the bucket name in Bucket".into(),
        );
    }
    let bucket = bucket.trim();
    if !valid_bucket(bucket) {
        return Err(if bucket.is_empty() {
            "Enter the bucket's name".into()
        } else {
            "A bucket name is 3 to 63 lowercase letters, digits, dots and dashes".into()
        });
    }
    let region = region.trim();
    let region = if region.is_empty() {
        DEFAULT_REGION
    } else {
        region
    };
    if region.len() > 32
        || !region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("The region is a name like auto or us-east-1".into());
    }
    let key = access_key_id.trim();
    if key.is_empty() {
        return Err("Enter the access key ID".into());
    }
    if key.len() > 128 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("The access key ID has characters an access key ID never has".into());
    }
    Ok(Target {
        endpoint: url,
        bucket: bucket.into(),
        region: region.into(),
        access_key_id: key.into(),
    })
}

/// S3 bucket naming: 3-63 of a-z 0-9 . -, starting and ending with a letter
/// or digit, no "..". (R2 and MinIO use the same rules.)
pub fn valid_bucket(b: &str) -> bool {
    (3..=63).contains(&b.len())
        && b.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-')
        && b.as_bytes()[0].is_ascii_alphanumeric()
        && b.as_bytes()[b.len() - 1].is_ascii_alphanumeric()
        && !b.contains("..")
}

pub fn validate_secret(secret: &str) -> Result<(), String> {
    if secret.len() > 256 || !secret.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("The secret access key has characters a secret access key never has".into());
    }
    Ok(())
}

/// Read `share-links.json`; defaults when it's missing or unreadable (an
/// unreadable file is logged and treated as "not set up").
pub fn load(dir: &Path) -> StoredConfig {
    let path = dir.join(FILE_NAME);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<StoredConfig>(&bytes) {
            Ok(c) if c.version <= FILE_VERSION => c,
            Ok(_) => {
                tracing::warn!("share-links.json is from a newer Penguin; share links are off");
                StoredConfig::default()
            }
            Err(e) => {
                tracing::warn!(error = %e, "share-links.json is unreadable; share links are off");
                StoredConfig::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoredConfig::default(),
        Err(e) => {
            tracing::warn!(error = %e, "couldn't read share-links.json; share links are off");
            StoredConfig::default()
        }
    }
}

pub fn save(dir: &Path, config: &StoredConfig) -> std::io::Result<()> {
    write_atomic(&dir.join(FILE_NAME), &serde_json::to_vec_pretty(config)?)
}

/// Write through a temp file and rename, so a crash never leaves half a file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp: PathBuf = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The secret access key in the Keychain, read at most once per process
/// (then kept in memory). A failed read isn't cached: every share is a
/// user action, so asking again is what the user expects.
pub struct SecretStore {
    backend: Box<dyn SecretBackend>,
    cached: Mutex<Option<Option<String>>>,
}

impl SecretStore {
    pub fn new(backend: Box<dyn SecretBackend>) -> SecretStore {
        SecretStore {
            backend,
            cached: Mutex::new(None),
        }
    }
    pub fn keychain() -> SecretStore {
        SecretStore::new(Box::new(KeyringBackend::new(KEYCHAIN_SERVICE)))
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Option<String>>> {
        self.cached.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// Blocking (a Keychain read can show a prompt): call from the blocking pool.
    pub fn get(&self) -> CmdResult<Option<String>> {
        let mut cached = self.lock();
        if let Some(v) = cached.as_ref() {
            return Ok(v.clone());
        }
        let v = self.backend.get(KEYCHAIN_ACCOUNT).map_err(keychain_error)?;
        *cached = Some(v.clone());
        Ok(v)
    }
    pub fn set(&self, secret: &str) -> CmdResult<()> {
        let mut cached = self.lock();
        self.backend
            .set(KEYCHAIN_ACCOUNT, secret)
            .map_err(keychain_error)?;
        *cached = Some(Some(secret.to_string()));
        Ok(())
    }
    pub fn clear(&self) -> CmdResult<()> {
        let mut cached = self.lock();
        self.backend
            .delete(KEYCHAIN_ACCOUNT)
            .map_err(keychain_error)?;
        *cached = Some(None);
        Ok(())
    }
}

fn keychain_error(e: penguin_provider::Error) -> CmdError {
    tracing::warn!(error = %e, "share-link secret: Keychain error");
    CmdError::other(format!(
        "The Keychain refused the share-link secret ({e}). Open Settings → Share links and enter it again."
    ))
}
