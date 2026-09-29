//! Developer diagnostics: paths, sizes, counts and sync health for the
//! Settings → Diagnostics screen. Mirrored in `apps/desktop/src/lib/types.ts`
//! (Diagnostics, AccountDiagnostics, TableSizes, TableSize, RevealTarget).
//!
//! Only counts, sizes and paths leave here: no message content, subjects,
//! addresses other than the account's own, or tokens. The OAuth client id is
//! cut to its last 12 characters and the secret is never read out. Keychain
//! status comes from what this session already learned (start_account /
//! sign-in); diagnostics never touches the Keychain itself.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use penguin_core::diagnostics::TableSize;
use penguin_core::SyncPhase;
use serde::{Deserialize, Serialize};

/// Throughput window: rate = stored-count delta over (up to) this long.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Below this span a rate is noise; report none.
const MIN_RATE_SPAN: Duration = Duration::from_secs(5);
/// Samples closer together than this are merged (keeps the ring small).
const MIN_SAMPLE_GAP: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum KeychainStatus {
    Present,
    Missing,
    /// Not checked this session.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDiagnostics {
    pub account_id: String,
    pub email: String,
    pub phase: SyncPhase,
    pub messages_stored: u64,
    /// Of `messages_stored`, those with headers only (older than the sync
    /// window, body not downloaded).
    pub headers_only_messages: u64,
    pub gmail_total: Option<u64>,
    pub backfill_done: bool,
    pub history_id_present: bool,
    pub failed_message_ids: u64,
    pub last_synced_at: Option<i64>,
    pub msgs_per_minute: Option<f64>,
    pub keychain: KeychainStatus,
    /// Learned Gmail quota pacing (None until the account has made a call).
    pub quota: Option<penguin_gmail::api::QuotaStats>,
    pub inline_cache_bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub app_version: String,
    /// "macOS 26.0.1" (None where it can't be read).
    pub os_version: Option<String>,
    pub data_dir: String,
    pub config_dir: String,
    pub cache_dir: String,
    pub log_dir: Option<String>,
    pub db_path: String,
    pub db_bytes: u64,
    pub wal_bytes: u64,
    pub page_size: u64,
    pub page_count: u64,
    pub freelist_count: u64,
    pub total_messages: u64,
    pub total_threads: u64,
    pub inline_cache_bytes: u64,
    pub log_bytes: u64,
    pub oauth_client_id_tail: Option<String>,
    pub accounts: Vec<AccountDiagnostics>,
    pub trackers_removed_session: u64,
    /// PENGUIN_GMAIL_UNITS_PER_MIN, when set: it overrides the quota setting.
    pub gmail_units_env_override: Option<u64>,
    pub took_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableSizes {
    /// "dbstat" (measured page by page).
    pub method: String,
    pub tables: Vec<TableSize>,
    pub took_ms: f64,
}

/// Directories `reveal_path` will show; anything else is unrepresentable.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RevealTarget {
    Data,
    Config,
    Cache,
    Log,
}

/// The OS name and version for bug reports: "macOS 26.0.1". The web
/// view's user agent can't tell (WebKit freezes it at 10.15.7).
#[cfg(target_os = "macos")]
pub fn os_version() -> Option<String> {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: a NUL-terminated name, a buffer of `len` bytes that sysctl
    // fills and shortens `len` to, and no new value.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let text = std::str::from_utf8(&buf[..len.min(buf.len())]).ok()?;
    let version = text.trim_end_matches('\0').trim();
    (!version.is_empty()).then(|| format!("macOS {version}"))
}

#[cfg(not(target_os = "macos"))]
pub fn os_version() -> Option<String> {
    None
}

/// Session-lifetime counters feeding diagnostics.
#[derive(Default)]
pub struct DiagState {
    /// account → (when, messages stored) samples, oldest first.
    samples: Mutex<HashMap<String, VecDeque<(Instant, u64)>>>,
    /// account → whether the Keychain held a refresh token when last checked.
    keychain: Mutex<HashMap<String, bool>>,
    /// Trackers stripped per (account, message) rendered this session; keyed
    /// so re-rendering a thread doesn't count its pixels twice.
    trackers: Mutex<HashMap<(String, String), u32>>,
}

impl DiagState {
    /// Record a stored-message count for throughput. Called from every sync
    /// status event and every diagnostics read.
    pub fn sample(&self, account_id: &str, count: u64) {
        self.sample_at(account_id, count, Instant::now());
    }

    fn sample_at(&self, account_id: &str, count: u64, now: Instant) {
        let mut all = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        let ring = all.entry(account_id.to_string()).or_default();
        match ring.back_mut() {
            Some(last) if now.duration_since(last.0) < MIN_SAMPLE_GAP => *last = (now, count),
            _ => ring.push_back((now, count)),
        }
        // Keep one sample older than the window as the rate's baseline.
        while ring.len() > 2
            && ring
                .get(1)
                .is_some_and(|s| now.duration_since(s.0) >= RATE_WINDOW)
        {
            ring.pop_front();
        }
    }

    /// Messages stored per minute over the last ~minute, or None without
    /// enough history.
    pub fn rate_per_minute(&self, account_id: &str) -> Option<f64> {
        self.rate_at(account_id, Instant::now())
    }

    fn rate_at(&self, account_id: &str, now: Instant) -> Option<f64> {
        let all = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        let ring = all.get(account_id)?;
        let latest = *ring.back()?;
        // Oldest sample inside the window, else the baseline just before it.
        let base = ring
            .iter()
            .find(|s| now.duration_since(s.0) <= RATE_WINDOW)
            .copied()
            .or_else(|| ring.front().copied())?;
        let span = latest.0.duration_since(base.0);
        if span < MIN_RATE_SPAN {
            return None;
        }
        let delta = latest.1.saturating_sub(base.1) as f64;
        Some(delta * 60.0 / span.as_secs_f64())
    }

    pub fn set_keychain(&self, account_id: &str, present: bool) {
        self.keychain
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account_id.to_string(), present);
    }

    pub fn keychain(&self, account_id: &str) -> KeychainStatus {
        match self
            .keychain
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account_id)
        {
            Some(true) => KeychainStatus::Present,
            Some(false) => KeychainStatus::Missing,
            None => KeychainStatus::Unknown,
        }
    }

    pub fn forget_account(&self, account_id: &str) {
        self.samples
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account_id);
        self.keychain
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account_id);
        self.trackers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|(a, _), _| a != account_id);
    }

    pub fn record_trackers(&self, account_id: &str, message_id: &str, removed: u32) {
        if removed == 0 {
            return;
        }
        self.trackers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert((account_id.to_string(), message_id.to_string()), removed);
    }

    pub fn trackers_removed(&self) -> u64 {
        self.trackers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|&n| n as u64)
            .sum()
    }
}

pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Total bytes of regular files under `dir` (symlinks are not followed).
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = HashSet::new();
    while let Some(d) = stack.pop() {
        if !seen.insert(d.clone()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// `…-wal` next to the database file.
pub fn wal_path(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("-wal");
    PathBuf::from(s)
}

/// Last 12 characters of a client id (they identify the project without
/// being the whole id).
pub fn client_id_tail(client_id: &str) -> String {
    let chars: Vec<char> = client_id.chars().collect();
    chars[chars.len().saturating_sub(12)..].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throughput_from_count_deltas() {
        let d = DiagState::default();
        let t0 = Instant::now();
        assert_eq!(d.rate_at("a", t0), None);
        d.sample_at("a", 1000, t0);
        d.sample_at("a", 1010, t0 + Duration::from_secs(2));
        // Too short a span to trust.
        assert_eq!(d.rate_at("a", t0 + Duration::from_secs(2)), None);
        d.sample_at("a", 1500, t0 + Duration::from_secs(30));
        let r = d.rate_at("a", t0 + Duration::from_secs(30)).unwrap();
        assert!((r - 1000.0).abs() < 1e-6, "{r}");
        // Old samples fall out of the window; the rate follows recent activity.
        d.sample_at("a", 1500, t0 + Duration::from_secs(150));
        d.sample_at("a", 1500, t0 + Duration::from_secs(160));
        let r = d.rate_at("a", t0 + Duration::from_secs(160)).unwrap();
        assert_eq!(r, 0.0);
    }

    #[test]
    fn trackers_count_each_message_once() {
        let d = DiagState::default();
        d.record_trackers("a", "m1", 3);
        d.record_trackers("a", "m1", 3);
        d.record_trackers("b", "m2", 2);
        assert_eq!(d.trackers_removed(), 5);
        d.forget_account("a");
        assert_eq!(d.trackers_removed(), 2);
    }

    #[test]
    fn keychain_status_is_cache_only() {
        let d = DiagState::default();
        assert_eq!(d.keychain("a"), KeychainStatus::Unknown);
        d.set_keychain("a", true);
        assert_eq!(d.keychain("a"), KeychainStatus::Present);
        d.set_keychain("a", false);
        assert_eq!(d.keychain("a"), KeychainStatus::Missing);
    }

    #[test]
    fn client_id_is_truncated() {
        assert_eq!(
            client_id_tail("1234-abcdef.apps.googleusercontent.com"),
            "rcontent.com"
        );
        assert_eq!(client_id_tail("short"), "short");
    }

    #[test]
    fn sizes_and_reveal_targets() {
        let dir = std::env::temp_dir().join(format!("penguin-diag-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/x"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("a/b/y"), [0u8; 5]).unwrap();
        assert_eq!(dir_size(&dir), 15);
        assert_eq!(dir_size(&dir.join("missing")), 0);
        assert_eq!(
            wal_path(Path::new("/d/penguin.db")),
            PathBuf::from("/d/penguin.db-wal")
        );
        std::fs::remove_dir_all(dir).unwrap();
        let t: RevealTarget = serde_json::from_str(r#""log""#).unwrap();
        assert_eq!(t, RevealTarget::Log);
        assert!(serde_json::from_str::<RevealTarget>(r#""/etc""#).is_err());
    }
}
