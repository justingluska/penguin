//! On-disk avatar cache: `<cache>/avatars/`.
//!
//! - `img/<sha256 of the PNG>.png`: images we encoded (see image.rs),
//!   content-addressed, so a URL never changes meaning and the webview may
//!   cache it forever.
//! - `index.json`: one record per *source key* (a hash of "kind:identity",
//!   e.g. a contact's address or a sender's domain; no addresses in clear),
//!   holding the image hash or a miss, when it was checked and for how long
//!   it holds. Misses are cached too, so a sender without a logo costs one
//!   lookup per TTL, not one per render.
//!
//! The index lives in memory and is written back atomically after changes.
//! Everything here is disposable: the OS may wipe the cache dir.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const INDEX_FILE: &str = "index.json";
const IMG_DIR: &str = "img";
const INDEX_VERSION: u32 = 1;

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

/// Where an image came from; decides TTLs and how the UI frames it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Contact,
    Bimi,
    Icon,
    Gravatar,
    /// A signed-in account's own Google profile photo (account.rs).
    Account,
}

impl SourceKind {
    fn tag(self) -> &'static str {
        match self {
            SourceKind::Contact => "contact",
            SourceKind::Bimi => "bimi",
            SourceKind::Icon => "icon",
            SourceKind::Gravatar => "gravatar",
            SourceKind::Account => "account",
        }
    }

    /// How long a found image is trusted before we look again.
    pub fn hit_ttl(self) -> u64 {
        match self {
            SourceKind::Contact => 7 * DAY,
            SourceKind::Bimi | SourceKind::Icon => 30 * DAY,
            SourceKind::Gravatar => 14 * DAY,
            // Your own photo: re-checked about once a day, never per render.
            SourceKind::Account => DAY,
        }
    }

    /// How long a definite "there is none" holds.
    pub fn miss_ttl(self) -> u64 {
        match self {
            SourceKind::Contact | SourceKind::Account => DAY,
            SourceKind::Bimi | SourceKind::Icon | SourceKind::Gravatar => 7 * DAY,
        }
    }

    /// A failure that says nothing about the sender (offline, timeout, 5xx).
    pub fn error_ttl(self) -> u64 {
        HOUR
    }

    /// Brand imagery: shown on a white tile, and only for authenticated senders.
    pub fn is_brand(self) -> bool {
        matches!(self, SourceKind::Bimi | SourceKind::Icon)
    }
}

/// `<kind>-<first 32 hex of sha256("kind:identity")>`.
pub fn source_key(kind: SourceKind, identity: &str) -> String {
    let digest = Sha256::digest(format!("{}:{}", kind.tag(), identity).as_bytes());
    format!("{}-{}", kind.tag(), &hex(&digest)[..32])
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A valid image name: exactly 64 lowercase hex digits. The protocol handler
/// serves nothing else, so a request can't name any other file.
pub fn valid_image_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    /// Hash of the PNG in `img/`, or None for a cached miss.
    pub image: Option<String>,
    /// Unix seconds when this was decided.
    pub at: u64,
    /// Seconds the record holds.
    pub ttl: u64,
    /// For contact photos: the People API URL it was fetched from, so a
    /// changed photo is refetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Decided and within its TTL. `None` = cached miss.
    Fresh(Option<String>),
    /// Past its TTL: usable, but should be refreshed.
    Stale(Option<String>),
    Missing,
}

#[derive(Default, Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    sources: HashMap<String, SourceRecord>,
}

pub struct Cache {
    root: PathBuf,
    index: Mutex<Option<IndexState>>,
}

struct IndexState {
    file: IndexFile,
    dirty: bool,
}

impl Cache {
    pub fn new(root: PathBuf) -> Cache {
        Cache {
            root,
            index: Mutex::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn with_index<T>(&self, f: impl FnOnce(&mut IndexState) -> T) -> T {
        let mut guard = self.index.lock().unwrap_or_else(|p| p.into_inner());
        let state = guard.get_or_insert_with(|| IndexState {
            file: self.read_index(),
            dirty: false,
        });
        f(state)
    }

    fn read_index(&self) -> IndexFile {
        let path = self.root.join(INDEX_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<IndexFile>(&bytes) {
                Ok(f) if f.version == INDEX_VERSION => f,
                Ok(_) => IndexFile::default(),
                Err(e) => {
                    tracing::warn!(error = %e, "avatar index unreadable; starting empty");
                    IndexFile::default()
                }
            },
            Err(_) => IndexFile::default(),
        }
    }

    pub fn lookup(&self, key: &str, now: u64) -> Lookup {
        self.with_index(|s| match s.file.sources.get(key) {
            None => Lookup::Missing,
            Some(r) if now < r.at.saturating_add(r.ttl) => Lookup::Fresh(r.image.clone()),
            Some(r) => Lookup::Stale(r.image.clone()),
        })
    }

    pub fn record(&self, key: &str) -> Option<SourceRecord> {
        self.with_index(|s| s.file.sources.get(key).cloned())
    }

    /// Store a PNG we encoded and point `key` at it. Returns the image hash.
    pub fn put_image(
        &self,
        key: &str,
        png: &[u8],
        now: u64,
        ttl: u64,
        url: Option<String>,
    ) -> std::io::Result<String> {
        let hash = hex(&Sha256::digest(png));
        let path = self.root.join(IMG_DIR).join(format!("{hash}.png"));
        if !path.exists() {
            write_atomic(&path, png)?;
        }
        self.set(
            key,
            SourceRecord {
                image: Some(hash.clone()),
                at: now,
                ttl,
                url,
            },
        );
        Ok(hash)
    }

    pub fn put_miss(&self, key: &str, now: u64, ttl: u64, url: Option<String>) {
        self.set(
            key,
            SourceRecord {
                image: None,
                at: now,
                ttl,
                url,
            },
        );
    }

    fn set(&self, key: &str, record: SourceRecord) {
        self.with_index(|s| {
            s.file.sources.insert(key.to_string(), record);
            s.dirty = true;
        });
    }

    /// Path of a cached image, only for a well-formed hash that exists.
    pub fn image_path(&self, hash: &str) -> Option<PathBuf> {
        if !valid_image_hash(hash) {
            return None;
        }
        let path = self.root.join(IMG_DIR).join(format!("{hash}.png"));
        path.is_file().then_some(path)
    }

    /// Write the index if it changed. Blocking.
    pub fn flush(&self) -> std::io::Result<()> {
        let bytes = {
            let mut guard = self.index.lock().unwrap_or_else(|p| p.into_inner());
            let Some(state) = guard.as_mut().filter(|s| s.dirty) else {
                return Ok(());
            };
            state.file.version = INDEX_VERSION;
            state.dirty = false;
            serde_json::to_vec(&state.file).map_err(std::io::Error::other)?
        };
        write_atomic(&self.root.join(INDEX_FILE), &bytes)
    }

    /// Delete images no record points at (replaced photos, cleared sources).
    pub fn gc(&self) -> std::io::Result<usize> {
        let live: std::collections::HashSet<String> = self.with_index(|s| {
            s.file
                .sources
                .values()
                .filter_map(|r| r.image.clone())
                .collect()
        });
        let dir = self.root.join(IMG_DIR);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".png").unwrap_or(&name);
            if !live.contains(stem) {
                let _ = std::fs::remove_file(entry.path());
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// "Clear photo cache": forget every record and delete every file.
    pub fn clear(&self) -> std::io::Result<()> {
        let mut guard = self.index.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(IndexState {
            file: IndexFile::default(),
            dirty: false,
        });
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// (records, image files, bytes on disk).
    pub fn stats(&self) -> (usize, usize, u64) {
        let records = self.with_index(|s| s.file.sources.len());
        let (mut files, mut bytes) = (0, 0);
        if let Ok(entries) = std::fs::read_dir(self.root.join(IMG_DIR)) {
            for e in entries.flatten() {
                if let Ok(m) = e.metadata() {
                    files += 1;
                    bytes += m.len();
                }
            }
        }
        (records, files, bytes)
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("{}.part", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("penguin-avatars-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn keys_hide_identities_and_differ_by_kind() {
        let a = source_key(SourceKind::Bimi, "shop.example");
        let b = source_key(SourceKind::Icon, "shop.example");
        assert!(a.starts_with("bimi-") && b.starts_with("icon-"));
        assert_ne!(a[5..], b[5..]);
        assert!(!a.contains("shop"));
        assert_eq!(a, source_key(SourceKind::Bimi, "shop.example"));
    }

    #[test]
    fn hit_miss_and_ttl() {
        let dir = temp("ttl");
        let cache = Cache::new(dir.clone());
        let key = source_key(SourceKind::Icon, "shop.example");
        assert_eq!(cache.lookup(&key, 1000), Lookup::Missing);

        let hash = cache
            .put_image(&key, b"\x89PNGfake", 1000, 100, None)
            .unwrap();
        assert!(valid_image_hash(&hash));
        assert_eq!(cache.lookup(&key, 1099), Lookup::Fresh(Some(hash.clone())));
        assert_eq!(cache.lookup(&key, 1100), Lookup::Stale(Some(hash.clone())));
        assert_eq!(
            std::fs::read(cache.image_path(&hash).unwrap()).unwrap(),
            b"\x89PNGfake"
        );

        // Negative caching: a miss is Fresh(None) until its TTL runs out.
        let miss = source_key(SourceKind::Bimi, "nologo.example");
        cache.put_miss(&miss, 1000, 50, None);
        assert_eq!(cache.lookup(&miss, 1049), Lookup::Fresh(None));
        assert_eq!(cache.lookup(&miss, 1050), Lookup::Stale(None));

        // Survives a restart once flushed.
        cache.flush().unwrap();
        let reopened = Cache::new(dir.clone());
        assert_eq!(
            reopened.lookup(&key, 1099),
            Lookup::Fresh(Some(hash.clone()))
        );
        assert_eq!(reopened.lookup(&miss, 1049), Lookup::Fresh(None));

        // Replacing the record orphans the old image; gc removes it.
        reopened
            .put_image(&key, b"\x89PNGnew", 2000, 100, None)
            .unwrap();
        assert_eq!(reopened.gc().unwrap(), 1);
        assert!(reopened.image_path(&hash).is_none());

        reopened.clear().unwrap();
        assert_eq!(reopened.lookup(&key, 2001), Lookup::Missing);
        assert!(!dir.exists());
    }

    #[test]
    fn image_paths_only_for_hashes() {
        let cache = Cache::new(temp("paths"));
        for bad in [
            "../index",
            "..%2Findex",
            "/etc/passwd",
            &"a".repeat(63),
            &"A".repeat(64),
            &format!("{}/..", "a".repeat(61)),
        ] {
            assert!(cache.image_path(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn corrupt_index_starts_empty() {
        let dir = temp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(INDEX_FILE), b"{nope").unwrap();
        let cache = Cache::new(dir.clone());
        assert_eq!(cache.lookup("icon-x", 0), Lookup::Missing);
        let _ = std::fs::remove_dir_all(dir);
    }
}
