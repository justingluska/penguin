//! Sender avatars: contact photos and brand logos in place of monograms.
//!
//! Resolution order for an address (each step only when enabled in
//! Settings → Privacy → Sender photos):
//! 1. **Google contact photo**: from a daily bulk sync of the account's
//!    contacts and "Other contacts" (people.rs). Needs the contacts scopes,
//!    granted per account through "Connect contact photos".
//! 2. **BIMI logo**: `default._bimi.<domain>` → SVG, rasterized (image.rs).
//! 3. **Company icon**: apple-touch-icon / favicon of the sender's domain.
//! 4. **Gravatar** (off by default): by sha256 of the address.
//! 5. Otherwise the UI keeps its monogram.
//!
//! Brand imagery (2, 3) is only used for senders Gmail authenticated
//! (DMARC pass / aligned DKIM): the request's own flag when the UI knows it
//! (an open message), else whether the newest stored message from that
//! address was authenticated. Mailbox providers (gmail.com, …) never get one.
//!
//! Nothing here runs on the render path: `lookup` reads memory and the
//! cache index only and queues misses; a background worker resolves them
//! with bounded concurrency and emits `penguin://avatars-changed`. Images
//! reach the webview through the `avatar:` protocol (protocol.rs), never as
//! remote URLs, and every request is made from Rust (net.rs).

pub mod account;
pub mod cache;
pub mod commands;
/// Also used by provider detection (src/providers/) for MX lookups.
pub(crate) mod dns;
pub mod image;
pub mod net;
pub mod people;
pub mod protocol;
pub mod sources;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

use cache::{source_key, Cache, Lookup, SourceKind};
use net::Net;
use sources::Outcome;

pub const EVENT_AVATARS_CHANGED: &str = "penguin://avatars-changed";

/// Which sources may be used (Settings::avatar_prefs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    /// avatarPlacement isn't "off"; false = no lookups, no fetches.
    pub show: bool,
    pub contacts: bool,
    pub bimi: bool,
    pub favicons: bool,
    pub gravatar: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarRequest {
    pub email: String,
    /// The message's `senderAuthenticated` when the UI has it (thread
    /// view); null in lists, where the store decides.
    #[serde(default)]
    pub authenticated: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AvatarKind {
    /// A person: round.
    Photo,
    /// A brand: on a white rounded-square tile.
    Logo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarInfo {
    pub email: String,
    pub kind: Option<AvatarKind>,
    /// `avatar://localhost/<sha256>.png` (http://avatar.localhost/… on
    /// Windows); immutable, so the webview may cache it forever.
    pub url: Option<String>,
}

pub fn image_url(hash: &str) -> String {
    if cfg!(windows) {
        format!("http://{}.localhost/{hash}.png", protocol::SCHEME)
    } else {
        format!("{}://localhost/{hash}.png", protocol::SCHEME)
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Contact { hash: String, url: String },
    Bimi { domain: String },
    Icon { domain: String },
    Gravatar { email: String },
}

impl Source {
    fn kind(&self) -> SourceKind {
        match self {
            Source::Contact { .. } => SourceKind::Contact,
            Source::Bimi { .. } => SourceKind::Bimi,
            Source::Icon { .. } => SourceKind::Icon,
            Source::Gravatar { .. } => SourceKind::Gravatar,
        }
    }

    fn key(&self) -> String {
        match self {
            Source::Contact { hash, .. } => source_key(SourceKind::Contact, hash),
            Source::Bimi { domain } => source_key(SourceKind::Bimi, domain),
            Source::Icon { domain } => source_key(SourceKind::Icon, &sources::org_domain(domain)),
            Source::Gravatar { email } => {
                source_key(SourceKind::Gravatar, &sources::email_hash(email))
            }
        }
    }

    /// Contact records remember the photo URL; a new URL is a new photo.
    fn url(&self) -> Option<String> {
        match self {
            Source::Contact { url, .. } => Some(url.clone()),
            _ => None,
        }
    }
}

/// The Tauri-free core: decides, looks up and fetches. Tests drive it with
/// a fake [`Net`].
pub struct Resolver {
    pub cache: Cache,
    net: Arc<dyn Net>,
    /// sha256(address) → People API photo URL, merged over accounts.
    contacts: RwLock<HashMap<String, String>>,
    /// Single-flight per source key (two senders at one domain fetch once).
    key_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Resolver {
    pub fn new(cache: Cache, net: Arc<dyn Net>) -> Resolver {
        Resolver {
            cache,
            net,
            contacts: RwLock::new(HashMap::new()),
            key_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_contacts(&self, map: HashMap<String, String>) {
        *self.contacts.write().unwrap_or_else(|p| p.into_inner()) = map;
    }

    pub fn contacts_len(&self) -> usize {
        self.contacts
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    fn chain(&self, email: &str, prefs: &Prefs, authenticated: bool) -> Vec<Source> {
        let mut out = Vec::new();
        if !prefs.show {
            return out;
        }
        let email = email.trim().to_lowercase();
        if prefs.contacts {
            let hash = sources::email_hash(&email);
            let url = self
                .contacts
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .get(&hash)
                .cloned();
            if let Some(url) = url {
                out.push(Source::Contact { hash, url });
            }
        }
        if let Some(domain) = sources::domain_of(&email) {
            let brand_ok = authenticated && !sources::is_mailbox_provider(&domain);
            if brand_ok && prefs.bimi {
                out.push(Source::Bimi {
                    domain: domain.clone(),
                });
            }
            if brand_ok && prefs.favicons {
                out.push(Source::Icon { domain });
            }
        }
        if prefs.gravatar && email.contains('@') {
            out.push(Source::Gravatar { email });
        }
        out
    }

    fn state(&self, source: &Source, now: u64) -> Lookup {
        let key = source.key();
        if let Some(url) = source.url() {
            match self.cache.record(&key) {
                Some(r) if r.url.as_deref() == Some(url.as_str()) => {}
                _ => return Lookup::Missing,
            }
        }
        self.cache.lookup(&key, now)
    }

    /// Best image known right now, and whether a background resolve would
    /// improve or refresh it. Reads memory only.
    pub fn lookup(
        &self,
        email: &str,
        prefs: &Prefs,
        authenticated: bool,
        now: u64,
    ) -> (AvatarInfo, bool) {
        let mut info = AvatarInfo {
            email: email.to_string(),
            kind: None,
            url: None,
        };
        let mut needs_work = false;
        for source in self.chain(email, prefs, authenticated) {
            let (image, current) = match self.state(&source, now) {
                Lookup::Fresh(img) => (img, true),
                Lookup::Stale(img) => (img, false),
                Lookup::Missing => (None, false),
            };
            needs_work |= !current;
            if let Some(hash) = image {
                info.kind = Some(if source.kind().is_brand() {
                    AvatarKind::Logo
                } else {
                    AvatarKind::Photo
                });
                info.url = Some(image_url(&hash));
                break;
            }
        }
        (info, needs_work)
    }

    fn key_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.key_locks.lock().unwrap_or_else(|p| p.into_inner());
        locks.retain(|_, l| Arc::strong_count(l) > 1);
        locks.entry(key.to_string()).or_default().clone()
    }

    /// Walk the chain, fetching whatever is missing or stale until an image
    /// turns up. Returns whether the answer for this address changed.
    pub async fn resolve(&self, email: &str, prefs: &Prefs, authenticated: bool, now: u64) -> bool {
        let before = self.lookup(email, prefs, authenticated, now).0;
        for source in self.chain(email, prefs, authenticated) {
            let key = source.key();
            let lock = self.key_lock(&key);
            let _held = lock.lock().await;
            match self.state(&source, now) {
                Lookup::Fresh(Some(_)) => break,
                Lookup::Fresh(None) => continue,
                Lookup::Stale(_) | Lookup::Missing => {}
            }
            let kind = source.kind();
            let outcome = self.fetch(&source).await;
            let found = matches!(outcome, Outcome::Image(_));
            match outcome {
                Outcome::Image(png) => {
                    if let Err(e) =
                        self.cache
                            .put_image(&key, &png, now, kind.hit_ttl(), source.url())
                    {
                        tracing::warn!(error = %e, "could not write avatar image");
                    }
                }
                Outcome::Miss => self
                    .cache
                    .put_miss(&key, now, kind.miss_ttl(), source.url()),
                Outcome::Error(e) => {
                    tracing::debug!(source = ?kind, error = %e, "avatar source unavailable");
                    // Keep a stale image rather than dropping to a monogram.
                    if !matches!(self.cache.lookup(&key, now), Lookup::Stale(Some(_))) {
                        self.cache
                            .put_miss(&key, now, kind.error_ttl(), source.url());
                    }
                }
            }
            if found {
                break;
            }
        }
        self.lookup(email, prefs, authenticated, now).0 != before
    }

    async fn fetch(&self, source: &Source) -> Outcome {
        let net = self.net.as_ref();
        match source {
            Source::Contact { url, .. } => sources::contact_photo(net, url).await,
            Source::Bimi { domain } => sources::bimi(net, domain).await,
            Source::Icon { domain } => sources::company_icon(net, domain).await,
            Source::Gravatar { email } => sources::gravatar(net, email).await,
        }
    }
}

/// A queued background resolve.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Job {
    pub email: String,
    pub authenticated: Option<bool>,
}

/// Queue + sender-authentication evidence shared by the commands and the
/// worker (commands.rs).
pub struct Avatars {
    pub resolver: Resolver,
    /// address → (newest stored message authenticated, checked at).
    evidence: Mutex<HashMap<String, (bool, u64)>>,
    queue: Mutex<(VecDeque<Job>, HashSet<Job>)>,
    pub wake: tokio::sync::Notify,
    pub contacts_wake: tokio::sync::Notify,
    /// Per-account People API sync state, for Settings.
    pub contacts_state: Mutex<HashMap<String, commands::ContactsState>>,
}

/// Evidence is re-read from the store after this long.
const EVIDENCE_TTL: u64 = 6 * 3600;
/// Upper bound on queued jobs; beyond it lookups just don't queue (the
/// UI asks again when rows re-render).
const MAX_QUEUE: usize = 2000;

impl Avatars {
    pub fn new(resolver: Resolver) -> Avatars {
        Avatars {
            resolver,
            evidence: Mutex::new(HashMap::new()),
            queue: Mutex::new((VecDeque::new(), HashSet::new())),
            wake: tokio::sync::Notify::new(),
            contacts_wake: tokio::sync::Notify::new(),
            contacts_state: Mutex::new(HashMap::new()),
        }
    }

    /// (known, authenticated) for an address.
    fn evidence(&self, email: &str, now: u64) -> Option<bool> {
        self.evidence
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(email)
            .filter(|(_, at)| now < at + EVIDENCE_TTL)
            .map(|(a, _)| *a)
    }

    pub fn set_evidence(&self, found: HashMap<String, bool>, asked: &[String], now: u64) {
        let mut ev = self.evidence.lock().unwrap_or_else(|p| p.into_inner());
        for email in asked {
            ev.insert(
                email.clone(),
                (found.get(email).copied().unwrap_or(false), now),
            );
        }
    }

    pub fn needs_evidence(&self, email: &str, now: u64) -> bool {
        self.evidence(email, now).is_none()
    }

    /// Answer from memory; queue background work for anything unknown.
    pub fn lookup(&self, reqs: &[AvatarRequest], prefs: &Prefs, now: u64) -> Vec<AvatarInfo> {
        let mut out = Vec::with_capacity(reqs.len());
        let mut queued = false;
        for req in reqs {
            let email = req.email.trim().to_lowercase();
            let known = req.authenticated.or_else(|| self.evidence(&email, now));
            let (mut info, needs_work) =
                self.resolver
                    .lookup(&email, prefs, known.unwrap_or(false), now);
            info.email = req.email.clone();
            // Unknown evidence may unlock brand sources once checked.
            if needs_work || (known.is_none() && (prefs.bimi || prefs.favicons)) {
                queued |= self.enqueue(Job {
                    email,
                    authenticated: req.authenticated,
                });
            }
            out.push(info);
        }
        if queued {
            self.wake.notify_one();
        }
        out
    }

    fn enqueue(&self, job: Job) -> bool {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        if q.0.len() >= MAX_QUEUE || q.1.contains(&job) {
            return false;
        }
        q.1.insert(job.clone());
        q.0.push_back(job);
        true
    }

    /// Up to `n` queued jobs. They stay in the dedupe set until `done`.
    pub fn take(&self, n: usize) -> Vec<Job> {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        let k = n.min(q.0.len());
        q.0.drain(..k).collect()
    }

    pub fn done(&self, job: &Job) {
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .1
            .remove(job);
    }

    pub fn authenticated_for(&self, job: &Job, now: u64) -> bool {
        job.authenticated
            .or_else(|| self.evidence(&job.email, now))
            .unwrap_or(false)
    }

    pub fn clear(&self) -> std::io::Result<()> {
        self.evidence
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.resolver.set_contacts(HashMap::new());
        self.contacts_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.resolver.cache.clear()
    }
}

#[cfg(test)]
mod tests;
