//! What Penguin has uploaded, and deleting it when its link expires.
//!
//! `share-uploads.json` in the app data dir lists each upload: its key, the
//! endpoint and bucket it went to, when it was made and when its link
//! expires, plus the retry state of a failed delete. No file names beyond
//! the key's, no message ids, no URLs.
//!
//! A background task ([`super::spawn_cleanup`]) wakes every few minutes and
//! deletes what has expired (when "Delete uploads when their links expire" is
//! on), with a growing wait after each failure. It never blocks a command:
//! it takes the list's lock only to read or update the list, never across a
//! request. Records whose deletes keep failing, or that belong to storage
//! that is no longer set up, are forgotten 30 days after they expired (an
//! R2 lifecycle rule is the backstop: docs/SHARE-LINKS.md).

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::config::write_atomic;

const FILE_NAME: &str = "share-uploads.json";
const FILE_VERSION: u32 = 1;
const MINUTE_MS: i64 = 60_000;
/// First retry after a failed delete; doubles each time up to [`MAX_BACKOFF_MS`].
const FIRST_BACKOFF_MS: i64 = 5 * MINUTE_MS;
const MAX_BACKOFF_MS: i64 = 24 * 60 * MINUTE_MS;
/// Records are forgotten this long after their link expired, deleted or not.
pub const FORGET_AFTER_MS: i64 = 30 * 24 * 60 * MINUTE_MS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub key: String,
    /// Endpoint (no trailing slash) and bucket it went to: a delete only runs
    /// against the same storage.
    pub endpoint: String,
    pub bucket: String,
    /// Unix ms.
    pub created_at: i64,
    pub expires_at: i64,
    /// Failed deletes so far, and when to try the next one (Unix ms).
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub next_try_at: i64,
}

#[derive(Serialize, Deserialize)]
struct UploadsFile {
    version: u32,
    #[serde(default)]
    uploads: Vec<Upload>,
}

/// The wait before the next delete after `attempts` failures.
pub fn backoff_ms(attempts: u32) -> i64 {
    let doublings = attempts.saturating_sub(1).min(20);
    (FIRST_BACKOFF_MS << doublings).min(MAX_BACKOFF_MS)
}

/// Uploads to delete now from the storage at `endpoint`/`bucket`: expired,
/// and past their retry time.
pub fn due(list: &[Upload], now: i64, endpoint: &str, bucket: &str) -> Vec<Upload> {
    list.iter()
        .filter(|u| {
            u.expires_at <= now
                && u.next_try_at <= now
                && u.endpoint == endpoint
                && u.bucket == bucket
        })
        .cloned()
        .collect()
}

/// Records old enough to forget.
pub fn stale(u: &Upload, now: i64) -> bool {
    now.saturating_sub(u.expires_at) > FORGET_AFTER_MS
}

/// The upload list, kept in memory and written through to disk.
pub struct Uploads {
    path: Option<PathBuf>,
    list: Mutex<Vec<Upload>>,
}

impl Uploads {
    /// Load `share-uploads.json` from `dir` (empty when missing; an
    /// unreadable file is logged and set aside, never overwritten blind).
    pub fn load(dir: &std::path::Path) -> Uploads {
        let path = dir.join(FILE_NAME);
        let list = match std::fs::read(&path) {
            Ok(bytes) => {
                match serde_json::from_slice::<UploadsFile>(&bytes) {
                    Ok(f) if f.version <= FILE_VERSION => f.uploads,
                    Ok(_) | Err(_) => {
                        tracing::warn!("share-uploads.json is unreadable; keeping it as share-uploads.json.bad");
                        let _ = std::fs::rename(&path, path.with_extension("json.bad"));
                        Vec::new()
                    }
                }
            }
            Err(_) => Vec::new(),
        };
        Uploads {
            path: Some(path),
            list: Mutex::new(list),
        }
    }

    /// In memory only (tests).
    pub fn in_memory(list: Vec<Upload>) -> Uploads {
        Uploads {
            path: None,
            list: Mutex::new(list),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Upload>> {
        self.list.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn persist(&self, list: &[Upload]) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = UploadsFile {
            version: FILE_VERSION,
            uploads: list.to_vec(),
        };
        write_atomic(path, &serde_json::to_vec_pretty(&file)?)
    }

    pub fn snapshot(&self) -> Vec<Upload> {
        self.lock().clone()
    }

    pub fn get(&self, key: &str) -> Option<Upload> {
        self.lock().iter().find(|u| u.key == key).cloned()
    }

    /// Record an upload before its bytes go out, so a crash mid-upload still
    /// gets cleaned up.
    pub fn add(&self, upload: Upload) -> std::io::Result<()> {
        let mut list = self.lock();
        list.retain(|u| u.key != upload.key);
        list.push(upload);
        self.persist(&list)
    }

    pub fn remove(&self, key: &str) -> std::io::Result<()> {
        let mut list = self.lock();
        let before = list.len();
        list.retain(|u| u.key != key);
        if list.len() == before {
            return Ok(());
        }
        self.persist(&list)
    }

    /// A delete failed: try again after the backoff.
    pub fn failed(&self, key: &str, now: i64) -> std::io::Result<()> {
        let mut list = self.lock();
        let Some(u) = list.iter_mut().find(|u| u.key == key) else {
            return Ok(());
        };
        u.attempts = u.attempts.saturating_add(1);
        u.next_try_at = now + backoff_ms(u.attempts);
        self.persist(&list)
    }

    /// Forget stale records; returns how many.
    pub fn prune(&self, now: i64) -> std::io::Result<usize> {
        let mut list = self.lock();
        let before = list.len();
        list.retain(|u| !stale(u, now));
        let gone = before - list.len();
        if gone > 0 {
            self.persist(&list)?;
        }
        Ok(gone)
    }
}

/// What one sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sweep {
    pub deleted: usize,
    pub failed: usize,
    pub forgotten: usize,
}

/// Delete every due upload with `delete` (one at a time; each outcome is
/// written back before the next), then forget stale records. `target` is
/// the storage currently set up (endpoint, bucket), or None when deleting is
/// off or nothing is set up: then only the forgetting happens.
pub async fn sweep<F, Fut, E>(
    uploads: &Uploads,
    now: i64,
    target: Option<(&str, &str)>,
    mut delete: F,
) -> Sweep
where
    F: FnMut(Upload) -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Debug,
{
    let mut out = Sweep::default();
    if let Some((endpoint, bucket)) = target {
        for u in due(&uploads.snapshot(), now, endpoint, bucket) {
            let key = u.key.clone();
            match delete(u).await {
                Ok(()) => {
                    out.deleted += 1;
                    if let Err(e) = uploads.remove(&key) {
                        tracing::warn!(error = %e, "share links: couldn't update share-uploads.json");
                    }
                }
                Err(e) => {
                    out.failed += 1;
                    tracing::warn!(error = ?e, "share links: expired upload not deleted yet; will retry");
                    if let Err(e) = uploads.failed(&key, now) {
                        tracing::warn!(error = %e, "share links: couldn't update share-uploads.json");
                    }
                }
            }
        }
    }
    match uploads.prune(now) {
        Ok(n) => {
            out.forgotten = n;
            if n > 0 {
                tracing::warn!(count = n, "share links: forgot uploads 30 days past expiry (a bucket lifecycle rule removes any left)");
            }
        }
        Err(e) => tracing::warn!(error = %e, "share links: couldn't update share-uploads.json"),
    }
    out
}
