//! One IMAP account's shared context: its settings, credentials, the
//! interactive connection, the folder list, and the operations both the
//! provider (user actions) and the sync engine use to turn server state into
//! stored messages (ids, labels, threads, locations).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use penguin_core::{Account, AttachmentMeta, Message, ServerSettings, Store};
use penguin_provider::credentials::{PasswordCredential, SecretVault};
use penguin_provider::ids::{self, system};

use crate::db::{self, Flags, Location};
use crate::folders::{self, Folder, FolderSet, Role};
use crate::mime::{self, Meta};
use crate::proto::{seqset, Arg};
use crate::session::{Caps, FetchItem, Session};
use crate::threading;
use crate::{Error, Result};

/// Messages larger than this are fetched as structure + text parts, never
/// whole (attachments are downloaded on demand).
pub const LARGE_MESSAGE: u64 = 2 * 1024 * 1024;
/// Sync fetches messages above this size the same way: what makes mail this
/// big is almost always attachments, which sync doesn't download (Gmail's
/// API path never does either). Below it, one whole-message FETCH is
/// cheaper than the extra round trip.
pub const STRUCTURE_ABOVE: u64 = 128 * 1024;
/// Each body text part of a [`LARGE_MESSAGE`] is fetched up to this many
/// bytes (smaller mail's text comes whole, as a whole download would).
pub const TEXT_PART_CAP: u64 = 1024 * 1024;
/// At most this many messages per full-message FETCH (and store commit)…
pub const FULL_CHUNK: usize = 100;
/// …and at most this many bytes (RFC822.SIZE), so a group of big messages
/// is split while ordinary mail (tens of KB) goes in one round trip.
pub const FULL_FETCH_BYTES: u64 = 8 * 1024 * 1024;
/// The folder list is re-read this often.
pub const FOLDER_TTL: Duration = Duration::from_secs(10 * 60);
/// A connection idle this long is checked with NOOP before reuse: servers
/// log idle sessions out (Yahoo after 5 minutes, earlier than RFC 3501's
/// 30), and a reaped one should reopen quietly, not fail the next command.
pub const STALE_AFTER: Duration = Duration::from_secs(60);

/// Server settings for one account (from `Account.provider_config`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub account_id: String,
    pub email: String,
    pub imap: ServerSettings,
    pub smtp: ServerSettings,
    /// Detection's provider kind name ("gmail", "icloud", "yahoo", …).
    pub host: Option<String>,
}

impl Config {
    pub fn from_account(account: &Account) -> Result<Config> {
        let pc = &account.provider_config;
        let (Some(imap), Some(smtp)) = (pc.imap.clone(), pc.smtp.clone()) else {
            return Err(Error::Other(format!(
                "{} has no IMAP/SMTP server settings",
                account.id
            )));
        };
        Ok(Config {
            account_id: account.id.clone(),
            email: account.email.clone(),
            imap,
            smtp,
            host: pc.host.clone(),
        })
    }

    /// Gmail's servers (quick setup). The X-GM-EXT-1 capability decides
    /// the sync model; this only picks hints and the Sent-copy policy.
    pub fn is_gmail_host(&self) -> bool {
        self.host.as_deref() == Some("gmail")
            || self.imap.host.eq_ignore_ascii_case("imap.gmail.com")
            || self.imap.host.eq_ignore_ascii_case("imap.googlemail.com")
    }

    /// Servers that file a copy of mail sent through their SMTP in Sent
    /// themselves (so appending one would duplicate it).
    pub fn server_saves_sent(&self) -> bool {
        self.is_gmail_host()
            || matches!(self.host.as_deref(), Some("icloud"))
            || self.smtp.host.eq_ignore_ascii_case("smtp.mail.me.com")
    }

    /// authserv-id of the account's receiving server, when Penguin knows it.
    pub fn trusted_authserv(&self) -> Option<String> {
        self.is_gmail_host().then(|| "mx.google.com".to_string())
    }
}

struct Cached {
    folders: Option<(Arc<FolderSet>, Instant)>,
    caps: Option<Caps>,
}

struct Conn {
    session: Option<Session>,
    last_used: Instant,
}

/// Shared by the account's provider client and its sync task.
pub struct Ctx {
    pub cfg: Config,
    pub store: Store,
    vault: Arc<SecretVault<PasswordCredential>>,
    conn: tokio::sync::Mutex<Conn>,
    cached: Mutex<Cached>,
    /// Serializes "assign a thread, then store" so two writers can't split
    /// one conversation.
    pub write_lock: tokio::sync::Mutex<()>,
}

/// The interactive connection, held for one operation.
pub struct ConnGuard<'a> {
    guard: tokio::sync::MutexGuard<'a, Conn>,
}

impl ConnGuard<'_> {
    pub fn session(&mut self) -> &mut Session {
        self.guard.session.as_mut().expect("opened by Ctx::conn")
    }
}

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        self.guard.last_used = Instant::now();
        if self.guard.session.as_ref().is_some_and(|s| !s.is_open()) {
            self.guard.session = None;
        }
    }
}

/// Run blocking store work off the async runtime.
pub async fn blocking<T: Send + 'static>(
    store: &Store,
    f: impl FnOnce(&Store) -> penguin_core::Result<T> + Send + 'static,
) -> Result<T> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| Error::Other(format!("store task failed: {e}")))?
        .map_err(Error::from)
}

/// What an ingest stored.
#[derive(Debug, Default)]
pub struct Ingested {
    /// Messages that weren't stored before (new to Penguin).
    pub new_ids: Vec<String>,
    /// Every message id the batch covered (known or new).
    pub ids: Vec<String>,
    pub threads: BTreeSet<String>,
}

/// How to store unknown messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Full,
    Headers,
    /// Record locations of known messages only; store nothing new.
    KnownOnly,
}

impl Ctx {
    pub fn new(cfg: Config, store: Store, vault: Arc<SecretVault<PasswordCredential>>) -> Ctx {
        Ctx {
            cfg,
            store,
            vault,
            conn: tokio::sync::Mutex::new(Conn {
                session: None,
                last_used: Instant::now(),
            }),
            cached: Mutex::new(Cached {
                folders: None,
                caps: None,
            }),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.cfg.account_id
    }

    /// The saved password (NeedsReauth when there is none).
    pub async fn credential(&self) -> Result<PasswordCredential> {
        let vault = self.vault.clone();
        let id = self.cfg.account_id.clone();
        tokio::task::spawn_blocking(move || vault.load(&id))
            .await
            .map_err(|e| Error::Other(format!("keychain task failed: {e}")))??
            .ok_or_else(|| Error::NeedsReauth("no saved password for this account".into()))
    }

    /// A new logged-in session (extensions enabled).
    pub async fn open_session(&self) -> Result<Session> {
        let cred = self.credential().await?;
        let mut s = Session::connect(&self.cfg.imap).await?;
        s.login(&self.cfg.imap.username, &cred.password).await?;
        s.compress().await?;
        s.enable_extensions().await?;
        self.cached.lock().unwrap_or_else(|p| p.into_inner()).caps = Some(s.caps.clone());
        Ok(s)
    }

    /// The interactive connection, (re)opened as needed.
    pub async fn conn(&self) -> Result<ConnGuard<'_>> {
        let mut guard = self.conn.lock().await;
        let stale = guard.last_used.elapsed() > STALE_AFTER;
        if let Some(s) = guard.session.as_mut() {
            if s.is_open() && stale && s.noop().await.is_err() {
                guard.session = None;
            }
        }
        if guard.session.as_ref().is_none_or(|s| !s.is_open()) {
            guard.session = Some(self.open_session().await?);
        }
        Ok(ConnGuard { guard })
    }

    /// Drop the interactive connection (after a password change).
    pub async fn disconnect(&self) {
        let mut guard = self.conn.lock().await;
        if let Some(s) = guard.session.take() {
            s.logout().await;
        }
    }

    pub fn caps(&self) -> Option<Caps> {
        self.cached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .caps
            .clone()
    }

    /// Gmail mode: the server has X-GM-EXT-1.
    pub fn gmail(&self, s: &Session) -> bool {
        s.caps.gmail()
    }

    /// The folder list, re-read after [`FOLDER_TTL`] or when `refresh`.
    pub async fn folders(&self, s: &mut Session, refresh: bool) -> Result<Arc<FolderSet>> {
        if !refresh {
            let c = self.cached.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((f, at)) = &c.folders {
                if at.elapsed() < FOLDER_TTL && f.gmail == s.caps.gmail() {
                    return Ok(f.clone());
                }
            }
        }
        let list = s.list().await?;
        let set = Arc::new(FolderSet::from_list(&list, s.caps.gmail()));
        self.cached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .folders = Some((set.clone(), Instant::now()));
        Ok(set)
    }

    /// The folder with `role`, creating `name` when the server has none.
    pub async fn folder_for(&self, s: &mut Session, role: Role, name: &str) -> Result<Folder> {
        let set = self.folders(s, false).await?;
        let found = match role {
            Role::Archive => set.archive().cloned(),
            r => set.by_role(r).cloned(),
        };
        if let Some(f) = found {
            return Ok(f);
        }
        let raw = crate::proto::utf7::encode(name);
        if set.by_raw(&raw).is_none() {
            s.create(&raw).await?;
        }
        let set = self.folders(s, true).await?;
        set.by_raw(&raw)
            .cloned()
            .ok_or_else(|| Error::Other(format!("couldn't create the {name} folder")))
    }

    // ----- identity -----

    /// The FETCH items that identify messages (and their flags).
    pub fn id_items(caps: &Caps) -> String {
        let mut s = String::from(
            "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID DATE)]",
        );
        if caps.has("OBJECTID") {
            s.push_str(" EMAILID");
        }
        if caps.gmail() {
            s.push_str(" X-GM-MSGID X-GM-THRID X-GM-LABELS");
        }
        s.push(')');
        s
    }

    /// The id items plus BODYSTRUCTURE, for backfill chunks that download
    /// in full: big messages' text parts can then be fetched without an
    /// extra round trip for their structure.
    pub fn fill_items(caps: &Caps) -> String {
        let mut s = Ctx::id_items(caps);
        s.pop();
        s.push_str(" BODYSTRUCTURE)");
        s
    }

    /// FLAGS (and Gmail labels) only.
    pub fn flag_items(caps: &Caps) -> String {
        if caps.gmail() {
            "(UID FLAGS X-GM-LABELS)".into()
        } else {
            "(UID FLAGS)".into()
        }
    }

    /// Location of a FETCH item in `folder` (needs an id).
    pub fn location(
        &self,
        folder: &str,
        uidvalidity: u32,
        item: &FetchItem,
        gmail: bool,
    ) -> Option<Location> {
        let uid = item.uid?;
        Some(Location {
            folder: folder.to_string(),
            uid,
            uidvalidity,
            message_id: message_id_of(item, gmail)?,
            flags: Flags::from_imap(item.flags.as_deref().unwrap_or(&[])),
            gm_labels: gmail.then(|| gm_label_ids(item)),
        })
    }

    // ----- labels -----

    /// Recompute stored labels of `ids` from their locations; returns the
    /// changed threads and (id, added labels).
    pub async fn refresh_labels(
        &self,
        folders: &FolderSet,
        ids: &[String],
    ) -> Result<(BTreeSet<String>, Vec<(String, Vec<String>)>)> {
        if ids.is_empty() {
            return Ok((BTreeSet::new(), Vec::new()));
        }
        let account = self.cfg.account_id.clone();
        let ids = ids.to_vec();
        let folders = folders.clone();
        blocking(&self.store, move |store| {
            let located = db::locations(store, &account, &ids)?;
            let mut threads = BTreeSet::new();
            let mut added_out = Vec::new();
            for id in &ids {
                let Some(locs) = located.get(id) else {
                    continue;
                };
                let Some(m) = store.get_message(&account, id)? else {
                    continue;
                };
                let want = labels_for(&folders, locs);
                let have: HashSet<&String> = m.label_ids.iter().collect();
                let want_set: HashSet<&String> = want.iter().collect();
                let add: Vec<String> = want.iter().filter(|l| !have.contains(l)).cloned().collect();
                let remove: Vec<String> = m
                    .label_ids
                    .iter()
                    .filter(|l| !want_set.contains(l))
                    .cloned()
                    .collect();
                if add.is_empty() && remove.is_empty() {
                    continue;
                }
                store.modify_message_labels(&account, std::slice::from_ref(id), &add, &remove)?;
                threads.insert(m.thread_id.clone());
                if !add.is_empty() {
                    added_out.push((id.clone(), add));
                }
            }
            Ok((threads, added_out))
        })
        .await
    }

    // ----- storing -----

    /// Store what a batch of id FETCH items (from `folder`) points at:
    /// record every copy, download unknown messages at `depth` (and, with
    /// Full, headers-only ones too), refresh labels of known ones.
    pub async fn ingest(
        &self,
        s: &mut Session,
        folders: &FolderSet,
        folder: &Folder,
        uidvalidity: u32,
        items: Vec<FetchItem>,
        depth: Depth,
    ) -> Result<Ingested> {
        let gmail = s.caps.gmail();
        let mut out = Ingested::default();
        let mut locs: Vec<(Location, FetchItem)> = Vec::new();
        for item in items {
            match self.location(&folder.raw, uidvalidity, &item, gmail) {
                Some(l) => locs.push((l, item)),
                None => tracing::debug!(uid = ?item.uid, "IMAP message without an id; skipped"),
            }
        }
        if locs.is_empty() {
            return Ok(out);
        }
        let ids: Vec<String> = locs.iter().map(|(l, _)| l.message_id.clone()).collect();
        let account = self.cfg.account_id.clone();
        let probe = ids.clone();
        let (known, need_body): (HashSet<String>, HashSet<String>) =
            blocking(&self.store, move |st| {
                let known = st.known_message_ids(&account, &probe)?;
                let need = st.ids_needing_body(&account, &probe)?;
                Ok((known, need.into_iter().collect()))
            })
            .await?;
        // Known messages: record the copy now.
        let known_locs: Vec<Location> = locs
            .iter()
            .filter(|(l, _)| known.contains(&l.message_id))
            .map(|(l, _)| l.clone())
            .collect();
        {
            let (account, kl) = (self.cfg.account_id.clone(), known_locs.clone());
            blocking(&self.store, move |st| db::put(st, &account, &kl)).await?;
        }
        let mut fresh: Vec<(Location, FetchItem)> = Vec::new();
        let mut upgrade: Vec<(Location, FetchItem)> = Vec::new();
        let mut seen = HashSet::new();
        for (l, item) in locs {
            if !seen.insert(l.message_id.clone()) {
                continue;
            }
            if !known.contains(&l.message_id) {
                if depth != Depth::KnownOnly {
                    fresh.push((l, item));
                }
            } else if depth == Depth::Full && need_body.contains(&l.message_id) {
                upgrade.push((l, item));
            }
        }
        out.ids = ids;
        let known_ids: Vec<String> = known_locs.iter().map(|l| l.message_id.clone()).collect();
        let (threads, _) = self.refresh_labels(folders, &known_ids).await?;
        out.threads.extend(threads);
        match depth {
            Depth::Full => {
                let mut all = fresh;
                let fresh_ids: HashSet<String> =
                    all.iter().map(|(l, _)| l.message_id.clone()).collect();
                all.extend(upgrade);
                for chunk in full_groups(&all) {
                    let stored = self.store_full(s, folders, chunk).await?;
                    for (id, thread_changes) in stored {
                        out.threads.extend(thread_changes);
                        if fresh_ids.contains(&id) {
                            out.new_ids.push(id);
                        }
                    }
                }
            }
            Depth::Headers => {
                for chunk in fresh.chunks(100) {
                    let stored = self.store_headers(s, folders, chunk).await?;
                    for (id, thread_changes) in stored {
                        out.threads.extend(thread_changes);
                        out.new_ids.push(id);
                    }
                }
            }
            Depth::KnownOnly => {}
        }
        Ok(out)
    }

    /// Full download and store; returns (id, changed threads) per stored
    /// message. Mail up to [`STRUCTURE_ABOVE`] comes whole; bigger mail as
    /// header + capped text parts, without its attachments (grouped by
    /// shape: mail comes in a handful, `1`, `1 2`, `1.1 1.2`…). The
    /// structures come with the id FETCH when the caller asked for them
    /// (backfill), else in one FETCH here; then every body request is sent
    /// in one pipelined round trip.
    async fn store_full(
        &self,
        s: &mut Session,
        folders: &FolderSet,
        batch: &[(Location, FetchItem)],
    ) -> Result<Vec<(String, Vec<String>)>> {
        struct Shaped<'a> {
            entry: &'a (Location, FetchItem),
            parts: Vec<(String, bool, Option<String>, String)>,
            attachments: Vec<AttachmentMeta>,
        }
        let gmail = s.caps.gmail();
        let (mut whole, big): (Vec<_>, Vec<_>) = batch
            .iter()
            .partition(|(_, i)| i.size.unwrap_or(0) <= STRUCTURE_ABOVE);
        let missing: Vec<u32> = big
            .iter()
            .filter(|(_, i)| i.bodystructure.is_none())
            .map(|(l, _)| l.uid)
            .collect();
        let mut fetched_structures: HashMap<u32, FetchItem> = HashMap::new();
        if !missing.is_empty() {
            let (items, _) = s
                .uid_fetch(&seqset::format(&missing), "(UID BODYSTRUCTURE)", None)
                .await?;
            fetched_structures = items
                .into_iter()
                .filter_map(|i| Some((i.uid?, i)))
                .collect();
        }
        let mut shaped: Vec<Shaped> = Vec::new();
        let mut shapes: BTreeMap<(Vec<String>, bool), Vec<usize>> = BTreeMap::new();
        for entry in big {
            let (l, id_item) = entry;
            let structure = match &id_item.bodystructure {
                Some(v) => Some(v),
                None => match fetched_structures.get(&l.uid) {
                    Some(item) => item.bodystructure.as_ref(),
                    // Gone since it was listed: nothing to store.
                    None => continue,
                },
            };
            let Some(bs) = structure.and_then(mime::parse_bodystructure) else {
                // No usable structure: whole, unless it's too big for that.
                if id_item.size.unwrap_or(0) <= LARGE_MESSAGE {
                    whole.push(entry);
                }
                continue;
            };
            let parts = bs.text_parts();
            // Past LARGE_MESSAGE each text part is capped, as on open.
            let capped = id_item.size.unwrap_or(0) > LARGE_MESSAGE;
            shapes
                .entry((parts.iter().map(|p| p.0.clone()).collect(), capped))
                .or_default()
                .push(shaped.len());
            shaped.push(Shaped {
                entry,
                parts,
                attachments: bs.attachments(),
            });
        }
        let mut requests: Vec<(String, String)> = Vec::new();
        if !whole.is_empty() {
            let uids: Vec<u32> = whole.iter().map(|(l, _)| l.uid).collect();
            requests.push((seqset::format(&uids), "(UID BODY.PEEK[])".into()));
        }
        for ((paths, capped), members) in &shapes {
            let mut items = String::from("(UID BODY.PEEK[HEADER]");
            for p in paths {
                if *capped {
                    items.push_str(&format!(" BODY.PEEK[{p}]<0.{TEXT_PART_CAP}>"));
                } else {
                    items.push_str(&format!(" BODY.PEEK[{p}]"));
                }
            }
            items.push(')');
            let uids: Vec<u32> = members.iter().map(|&k| shaped[k].entry.0.uid).collect();
            requests.push((seqset::format(&uids), items));
        }
        let by_uid: HashMap<u32, FetchItem> = s
            .uid_fetch_pipelined(&requests)
            .await?
            .into_iter()
            .filter_map(|i| Some((i.uid?, i)))
            .collect();
        let mut messages: Vec<(Location, FetchItem, Message)> = Vec::new();
        for (l, id_item) in whole {
            let Some(raw) = by_uid.get(&l.uid).and_then(|i| i.section("BODY[]")) else {
                continue;
            };
            let meta = self.meta(l, id_item, gmail);
            messages.push((l.clone(), id_item.clone(), mime::to_message(raw, &meta)));
        }
        for m in shaped {
            let (l, id_item) = m.entry;
            let Some(item) = by_uid.get(&l.uid) else {
                continue;
            };
            let Some(header) = item.section("BODY[HEADER]") else {
                continue;
            };
            let (mut text, mut html) = (Vec::new(), Vec::new());
            for (path, is_html, charset, encoding) in &m.parts {
                let Some(bytes) = item.section(&format!("BODY[{path}]")) else {
                    continue;
                };
                let decoded = mime::decode_transfer(bytes, encoding);
                let t = mime::decode_text(&decoded, charset.as_deref());
                if *is_html {
                    html.push(t);
                } else {
                    text.push(t);
                }
            }
            let meta = self.meta(l, id_item, gmail);
            let message = mime::from_parts(header, text, html, m.attachments, &meta);
            messages.push((l.clone(), id_item.clone(), message));
        }
        self.commit(folders, messages, false).await
    }

    async fn store_headers(
        &self,
        s: &mut Session,
        folders: &FolderSet,
        batch: &[(Location, FetchItem)],
    ) -> Result<Vec<(String, Vec<String>)>> {
        let gmail = s.caps.gmail();
        let uids: Vec<u32> = batch.iter().map(|(l, _)| l.uid).collect();
        let (items, _) = s
            .uid_fetch(
                &seqset::format(&uids),
                "(UID BODY.PEEK[HEADER] BODYSTRUCTURE)",
                None,
            )
            .await?;
        let by_uid: HashMap<u32, FetchItem> = items
            .into_iter()
            .filter_map(|i| Some((i.uid?, i)))
            .collect();
        let mut messages = Vec::new();
        for (l, id_item) in batch {
            let Some(item) = by_uid.get(&l.uid) else {
                continue;
            };
            let Some(header) = item.section("BODY[HEADER]") else {
                continue;
            };
            let meta = self.meta(l, id_item, gmail);
            let mut m = mime::headers_to_message(header, &meta);
            if let Some(bs) = item
                .bodystructure
                .as_ref()
                .and_then(mime::parse_bodystructure)
            {
                m.attachments = bs.attachments();
            }
            messages.push((l.clone(), id_item.clone(), m));
        }
        self.commit(folders, messages, true).await
    }

    /// Structure + header + capped text parts of one large message.
    pub async fn fetch_large(
        &self,
        s: &mut Session,
        l: &Location,
        id_item: &FetchItem,
        gmail: bool,
    ) -> Result<Option<Message>> {
        let (items, _) = s
            .uid_fetch(
                &l.uid.to_string(),
                "(UID BODYSTRUCTURE BODY.PEEK[HEADER])",
                None,
            )
            .await?;
        let Some(item) = items.into_iter().next() else {
            return Ok(None);
        };
        let Some(header) = item.section("BODY[HEADER]").map(<[u8]>::to_vec) else {
            return Ok(None);
        };
        let structure = item
            .bodystructure
            .as_ref()
            .and_then(mime::parse_bodystructure);
        let mut text = Vec::new();
        let mut html = Vec::new();
        let mut attachments = Vec::new();
        if let Some(bs) = &structure {
            attachments = bs.attachments();
            for (path, is_html, charset, encoding) in bs.text_parts() {
                let spec = format!("(UID BODY.PEEK[{path}]<0.{TEXT_PART_CAP}>)");
                let (parts, _) = s.uid_fetch(&l.uid.to_string(), &spec, None).await?;
                let Some(bytes) = parts
                    .first()
                    .and_then(|p| p.section(&format!("BODY[{path}]")))
                else {
                    continue;
                };
                let decoded = mime::decode_transfer(bytes, &encoding);
                let t = mime::decode_text(&decoded, charset.as_deref());
                if is_html {
                    html.push(t);
                } else {
                    text.push(t);
                }
            }
        }
        let meta = self.meta(l, id_item, gmail);
        Ok(Some(mime::from_parts(
            &header,
            text,
            html,
            attachments,
            &meta,
        )))
    }

    fn meta(&self, l: &Location, id_item: &FetchItem, gmail: bool) -> Meta {
        Meta {
            account_id: self.cfg.account_id.clone(),
            id: l.message_id.clone(),
            thread_id: if gmail {
                id_item
                    .gm_thrid
                    .map(|t| format!("gt:{t:x}"))
                    .unwrap_or_default()
            } else {
                String::new()
            },
            date_ms: id_item.internal_date,
            labels: Vec::new(),
            trusted_authserv: self.cfg.trusted_authserv(),
        }
    }

    /// Thread, label and store converted messages (plus their copies).
    async fn commit(
        &self,
        folders: &FolderSet,
        messages: Vec<(Location, FetchItem, Message)>,
        headers_only: bool,
    ) -> Result<Vec<(String, Vec<String>)>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let _w = self.write_lock.lock().await;
        let account = self.cfg.account_id.clone();
        let folders = folders.clone();
        blocking(&self.store, move |store| {
            let mut out = Vec::new();
            let locs: Vec<Location> = messages.iter().map(|(l, _, _)| l.clone()).collect();
            db::put(store, &account, &locs)?;
            let ids: Vec<String> = locs.iter().map(|l| l.message_id.clone()).collect();
            let located = db::locations(store, &account, &ids)?;
            let mut batch = Vec::with_capacity(messages.len());
            for (l, _, mut m) in messages {
                let mut changed = Vec::new();
                if let Some(existing) = store.get_message(&account, &m.id)? {
                    m.thread_id = existing.thread_id;
                } else if m.thread_id.is_empty() {
                    let (t, merged) = threading::assign(
                        store,
                        &account,
                        &threading::Input {
                            message_id: m.id.clone(),
                            own: m.message_id_header.clone(),
                            in_reply_to: m.in_reply_to.clone(),
                            references: m.references.clone(),
                            subject: m.subject.clone(),
                            date_ms: m.date,
                        },
                    )?;
                    m.thread_id = t;
                    changed.extend(merged);
                }
                m.label_ids = labels_for(
                    &folders,
                    located
                        .get(&m.id)
                        .map(Vec::as_slice)
                        .unwrap_or(std::slice::from_ref(&l)),
                );
                changed.push(m.thread_id.clone());
                out.push((m.id.clone(), changed));
                batch.push(m);
            }
            if headers_only {
                store.insert_header_messages(&batch)?;
            } else {
                store.upsert_messages(&batch)?;
            }
            Ok(out)
        })
        .await
    }

    // ----- finding messages on the server -----

    /// The best copy to read a message from (Gmail: All Mail; else INBOX
    /// first), with the others as fallbacks.
    pub async fn copies(&self, folders: &FolderSet, id: &str) -> Result<Vec<Location>> {
        let (account, want) = (self.cfg.account_id.clone(), vec![id.to_string()]);
        let mut locs = blocking(&self.store, move |st| db::locations(st, &account, &want))
            .await?
            .remove(id)
            .unwrap_or_default();
        let rank = |l: &Location| match folders.by_raw(&l.folder).map(|f| f.role) {
            Some(Role::All) => 0,
            Some(Role::Inbox) => 1,
            Some(Role::Other) | Some(Role::Archive) => 2,
            Some(Role::Sent) | Some(Role::Drafts) => 3,
            _ => 4,
        };
        locs.sort_by_key(rank);
        Ok(locs)
    }

    /// FETCH `items` for a message from its first copy that still exists
    /// (stale copies are forgotten). None when no copy is left.
    pub async fn fetch_from_copies(
        &self,
        s: &mut Session,
        folders: &FolderSet,
        id: &str,
        items: &str,
    ) -> Result<Option<(Location, FetchItem)>> {
        for loc in self.copies(folders, id).await? {
            let sel = match s.ensure_selected(&loc.folder, false).await {
                Ok(sel) => sel,
                Err(e) if e.is_not_found() => continue,
                Err(e) => return Err(e),
            };
            if sel.uidvalidity != loc.uidvalidity {
                continue;
            }
            let (got, _) = s.uid_fetch(&loc.uid.to_string(), items, None).await?;
            if let Some(item) = got.into_iter().find(|i| i.uid == Some(loc.uid)) {
                return Ok(Some((loc, item)));
            }
            let (account, folder, uid) = (self.cfg.account_id.clone(), loc.folder.clone(), loc.uid);
            blocking(&self.store, move |st| {
                db::remove(st, &account, &folder, &[uid])
            })
            .await?;
        }
        Ok(None)
    }

    /// Find a stored message in `folder` after a move without COPYUID.
    pub async fn locate(
        &self,
        s: &mut Session,
        folder: &Folder,
        id: &str,
    ) -> Result<Option<Location>> {
        let sel = s.select(&folder.raw, false).await?;
        let criteria: Vec<Arg> = if let Some(hex) = id.strip_prefix("gm:") {
            let n =
                u64::from_str_radix(hex, 16).map_err(|_| Error::Other("bad Gmail id".into()))?;
            vec![Arg::raw(format!("X-GM-MSGID {n}"))]
        } else if let Some(emailid) = id.strip_prefix("e:") {
            vec![Arg::raw("EMAILID"), Arg::string(emailid.as_bytes())]
        } else {
            let (account, mid) = (self.cfg.account_id.clone(), id.to_string());
            let header = blocking(&self.store, move |st| st.get_message(&account, &mid))
                .await?
                .and_then(|m| m.message_id_header);
            let Some(h) = header else { return Ok(None) };
            vec![
                Arg::raw("HEADER"),
                Arg::string("Message-ID"),
                Arg::string(format!("<{h}>").as_bytes()),
            ]
        };
        let uids = s.uid_search(criteria).await?;
        if uids.is_empty() {
            return Ok(None);
        }
        let caps = s.caps.clone();
        let (items, _) = s
            .uid_fetch(&seqset::format(&uids), &Ctx::id_items(&caps), None)
            .await?;
        let gmail = caps.gmail();
        Ok(items
            .iter()
            .filter_map(|i| self.location(&folder.raw, sel.uidvalidity, i, gmail))
            .find(|l| l.message_id == id))
    }

    /// Move copies (all in `from`) to `to`, keeping the location table
    /// right: COPYUID when the server gives it, else a search in `to`.
    pub async fn move_copies(
        &self,
        s: &mut Session,
        from: &str,
        locs: &[Location],
        to: &Folder,
    ) -> Result<()> {
        if locs.is_empty() || from == to.raw {
            return Ok(());
        }
        s.ensure_selected(from, true).await?;
        let uids: Vec<u32> = locs.iter().map(|l| l.uid).collect();
        let copied = s.uid_move(&seqset::format(&uids), &to.raw).await?;
        let account = self.cfg.account_id.clone();
        let (f, u) = (from.to_string(), uids.clone());
        blocking(&self.store, move |st| {
            db::remove(st, &account, &f, &u).map(|_| ())
        })
        .await?;
        let mut placed: Vec<Location> = Vec::new();
        let mut missing: Vec<&Location> = Vec::new();
        match copied {
            Some(c) => {
                for l in locs {
                    match c.pairs.iter().find(|(src, _)| *src == l.uid) {
                        Some((_, dst)) => placed.push(Location {
                            folder: to.raw.clone(),
                            uid: *dst,
                            uidvalidity: c.uidvalidity,
                            ..l.clone()
                        }),
                        None => missing.push(l),
                    }
                }
            }
            None => missing.extend(locs.iter()),
        }
        for l in missing {
            if let Some(found) = self.locate(s, to, &l.message_id).await? {
                placed.push(Location {
                    flags: l.flags,
                    gm_labels: l.gm_labels.clone(),
                    ..found
                });
            }
        }
        let account = self.cfg.account_id.clone();
        blocking(&self.store, move |st| db::put(st, &account, &placed)).await
    }

    /// Permanently remove copies (drafts being replaced). Gmail moves them
    /// to Trash first: expunging from Drafts would only archive them.
    pub async fn destroy_copies(&self, s: &mut Session, locs: &[Location]) -> Result<()> {
        let folders = self.folders(s, false).await?;
        let mut by_folder: HashMap<String, Vec<Location>> = HashMap::new();
        for l in locs {
            by_folder
                .entry(l.folder.clone())
                .or_default()
                .push(l.clone());
        }
        for (folder, locs) in by_folder {
            let mut folder = folder;
            let mut locs = locs;
            if s.caps.gmail() {
                if let Some(trash) = folders.by_role(Role::Trash).cloned() {
                    if trash.raw != folder {
                        self.move_copies(s, &folder, &locs, &trash).await?;
                        let ids: Vec<String> = locs.iter().map(|l| l.message_id.clone()).collect();
                        let account = self.cfg.account_id.clone();
                        let located =
                            blocking(&self.store, move |st| db::locations(st, &account, &ids))
                                .await?;
                        locs = located
                            .into_values()
                            .flatten()
                            .filter(|l| l.folder == trash.raw)
                            .collect();
                        folder = trash.raw.clone();
                    }
                }
            }
            if locs.is_empty() {
                continue;
            }
            match s.ensure_selected(&folder, true).await {
                Ok(_) => {}
                Err(e) if e.is_not_found() => continue,
                Err(e) => return Err(e),
            }
            let uids: Vec<u32> = locs.iter().map(|l| l.uid).collect();
            let set = seqset::format(&uids);
            s.uid_store(&set, "+FLAGS.SILENT", vec![Arg::raw("\\Deleted")])
                .await?;
            s.expunge_uids(&set).await?;
            let account = self.cfg.account_id.clone();
            blocking(&self.store, move |st| {
                db::remove(st, &account, &folder, &uids).map(|_| ())
            })
            .await?;
        }
        Ok(())
    }

    /// Append `raw` to `folder` and store it like sync would (id, thread,
    /// labels, location). Returns the stored message's id and thread.
    pub async fn append_and_store(
        &self,
        s: &mut Session,
        folder: &Folder,
        flags: &str,
        raw: &[u8],
        thread_hint: Option<&str>,
    ) -> Result<Option<(String, String)>> {
        let appended = s.append(&folder.raw, flags, raw).await?;
        let sel = s.select(&folder.raw, false).await?;
        let uids = match appended {
            Some((uv, uid)) if uv == sel.uidvalidity => vec![uid],
            _ => {
                // No UIDPLUS: find it by its Message-ID.
                let env = mime::envelope(raw);
                match env.message_id {
                    Some(mid) => {
                        s.uid_search(vec![
                            Arg::raw("HEADER"),
                            Arg::string("Message-ID"),
                            Arg::string(format!("<{mid}>").as_bytes()),
                        ])
                        .await?
                    }
                    None => Vec::new(),
                }
            }
        };
        let Some(uid) = uids.into_iter().max() else {
            return Ok(None);
        };
        if s.caps.gmail() && folder.role != Role::All {
            // Gmail: a label folder's copy is the All Mail message; store
            // it from there (labels come with it).
            let (items, _) = s
                .uid_fetch(&uid.to_string(), "(UID X-GM-MSGID)", None)
                .await?;
            let folders = self.folders(s, false).await?;
            if let (Some(m), Some(all)) = (
                items.first().and_then(|i| i.gm_msgid),
                folders.by_role(Role::All).cloned(),
            ) {
                let sel = s.select(&all.raw, false).await?;
                let found = s
                    .uid_search(vec![Arg::raw(format!("X-GM-MSGID {m}"))])
                    .await?;
                if let Some(uid) = found.into_iter().max() {
                    return self
                        .store_uid(s, &all, sel.uidvalidity, uid, thread_hint)
                        .await;
                }
            }
            return Ok(None);
        }
        self.store_uid(s, folder, sel.uidvalidity, uid, thread_hint)
            .await
    }

    /// Store the message at `uid` of the selected `folder` (full), with an
    /// optional thread to put it in. Returns (id, thread).
    pub async fn store_uid(
        &self,
        s: &mut Session,
        folder: &Folder,
        uidvalidity: u32,
        uid: u32,
        thread_hint: Option<&str>,
    ) -> Result<Option<(String, String)>> {
        let caps = s.caps.clone();
        let (items, _) = s
            .uid_fetch(&uid.to_string(), &Ctx::id_items(&caps), None)
            .await?;
        let Some(item) = items.into_iter().next() else {
            return Ok(None);
        };
        let Some(loc) = self.location(&folder.raw, uidvalidity, &item, caps.gmail()) else {
            return Ok(None);
        };
        let id = loc.message_id.clone();
        if let (Some(t), false) = (thread_hint, caps.gmail()) {
            // A reply composed in a known thread belongs there even when
            // its headers can't say so yet.
            let (account, t) = (self.cfg.account_id.clone(), t.to_string());
            let raw_headers = item.section("BODY[HEADER.FIELDS]").map(<[u8]>::to_vec);
            blocking(&self.store, move |st| {
                if let Some(h) = raw_headers.and_then(|h| mime::envelope(&h).message_id) {
                    db::set_threads(st, &account, &[ids::normalize_message_id(&h)], &t, &[])?;
                }
                Ok(())
            })
            .await?;
        }
        let folders = self.folders(s, false).await?;
        self.ingest(s, &folders, folder, uidvalidity, vec![item], Depth::Full)
            .await?;
        let (account, mid) = (self.cfg.account_id.clone(), id.clone());
        let thread = blocking(&self.store, move |st| st.get_message(&account, &mid))
            .await?
            .map(|m| m.thread_id);
        Ok(thread.map(|t| (id, t)))
    }
}

/// Penguin's id for a FETCH item: Gmail's X-GM-MSGID, the server's
/// EMAILID, else a hash of Message-ID, Date and size.
/// Split messages to download in full into FETCH groups: at most
/// [`FULL_CHUNK`] messages and [`FULL_FETCH_BYTES`] bytes of download
/// each ([`download_bytes`]; a message over the limit goes alone). Each
/// group is one round trip and one store commit.
pub fn full_groups(all: &[(Location, FetchItem)]) -> Vec<&[(Location, FetchItem)]> {
    let mut groups = Vec::new();
    let (mut start, mut bytes) = (0usize, 0u64);
    for (i, (_, item)) in all.iter().enumerate() {
        let size = download_bytes(item);
        if i > start && (i - start >= FULL_CHUNK || bytes + size > FULL_FETCH_BYTES) {
            groups.push(&all[start..i]);
            (start, bytes) = (i, 0);
        }
        bytes += size;
    }
    if start < all.len() {
        groups.push(&all[start..]);
    }
    groups
}

/// What [`Ctx::store_full`] downloads for a message: all of it up to
/// [`STRUCTURE_ABOVE`], else its header and text parts (from the
/// BODYSTRUCTURE when the id FETCH brought it; without one, as much as
/// could come).
pub fn download_bytes(item: &FetchItem) -> u64 {
    /// Allowance for the header block of a structured download.
    const HEADER: u64 = 16 * 1024;
    let size = item.size.unwrap_or(0);
    if size <= STRUCTURE_ABOVE {
        return size;
    }
    let cap = if size > LARGE_MESSAGE {
        TEXT_PART_CAP
    } else {
        u64::MAX
    };
    match item
        .bodystructure
        .as_ref()
        .and_then(mime::parse_bodystructure)
    {
        Some(bs) => {
            (bs.text_sizes().into_iter().map(|n| n.min(cap)).sum::<u64>() + HEADER).min(size)
        }
        None => size.min(LARGE_MESSAGE),
    }
}

pub fn message_id_of(item: &FetchItem, gmail: bool) -> Option<String> {
    if gmail {
        if let Some(m) = item.gm_msgid {
            return Some(format!("gm:{m:x}"));
        }
    }
    if let Some(e) = &item.emailid {
        let id = ids::imap_message_id_from_emailid(e);
        if ids::check_id(&id).is_none() {
            return Some(id);
        }
    }
    let size = item.size?;
    let header = item.section("BODY[HEADER.FIELDS]").unwrap_or_default();
    let env = mime::envelope(header);
    Some(ids::imap_fallback_message_id(
        env.message_id.as_deref(),
        env.date_raw.as_deref(),
        size,
    ))
}

/// Gmail labels of a FETCH item as Penguin label ids.
pub fn gm_label_ids(item: &FetchItem) -> Vec<String> {
    let mut out: Vec<String> = item
        .gm_labels
        .as_deref()
        .map(crate::session::gm_label_names)
        .unwrap_or_default()
        .iter()
        .filter_map(|n| folders::gmail_label_id(n))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A message's labels from all its copies (docs/PROVIDERS-IMPL.md §4).
pub fn labels_for(folders: &FolderSet, locs: &[Location]) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    let mut unread = false;
    let mut starred = false;
    for l in locs {
        let role = folders.by_raw(&l.folder).map(|f| f.role);
        if folders.gmail {
            if let Some(g) = &l.gm_labels {
                set.extend(g.iter().filter(|x| x.as_str() != system::UNREAD).cloned());
            }
        }
        if let Some(label) = folders
            .by_raw(&l.folder)
            .and_then(|f| f.label(folders.gmail))
        {
            set.insert(label);
        }
        if l.flags.flagged {
            starred = true;
        }
        let outgoing = matches!(role, Some(Role::Sent) | Some(Role::Drafts))
            || l.gm_labels
                .as_ref()
                .is_some_and(|g| g.iter().any(|x| x == system::SENT || x == system::DRAFT));
        if !l.flags.seen && !outgoing {
            unread = true;
        }
    }
    if starred {
        set.insert(system::STARRED.into());
    } else {
        set.remove(system::STARRED);
    }
    if unread {
        set.insert(system::UNREAD.into());
    }
    if set.contains(system::TRASH) || set.contains(system::SPAM) {
        set.remove(system::INBOX);
    }
    if set.contains(system::TRASH) {
        set.remove(system::SPAM);
    }
    if set.contains(system::SENT) {
        set.remove(system::DRAFT);
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ListEntry;

    fn folders(gmail: bool) -> FolderSet {
        let e = |raw: &str, attr: &[&str]| ListEntry {
            raw: raw.into(),
            attrs: attr.iter().map(|s| s.to_string()).collect(),
            delimiter: Some("/".into()),
        };
        if gmail {
            FolderSet::from_list(
                &[
                    e("INBOX", &[]),
                    e("[Gmail]/All Mail", &["\\All"]),
                    e("[Gmail]/Trash", &["\\Trash"]),
                    e("[Gmail]/Spam", &["\\Junk"]),
                ],
                true,
            )
        } else {
            FolderSet::from_list(
                &[
                    e("INBOX", &[]),
                    e("Sent", &["\\Sent"]),
                    e("Drafts", &["\\Drafts"]),
                    e("Trash", &["\\Trash"]),
                    e("Archive", &["\\Archive"]),
                    e("Receipts", &[]),
                ],
                false,
            )
        }
    }

    fn loc(folder: &str, seen: bool, flagged: bool) -> Location {
        Location {
            folder: folder.into(),
            uid: 1,
            uidvalidity: 1,
            message_id: "h:1".into(),
            flags: Flags {
                seen,
                flagged,
                draft: false,
            },
            gm_labels: None,
        }
    }

    #[test]
    fn folder_copies_become_labels() {
        let f = folders(false);
        assert_eq!(
            labels_for(&f, &[loc("INBOX", false, false)]),
            vec!["INBOX", "UNREAD"]
        );
        assert_eq!(
            labels_for(&f, &[loc("Archive", true, true)]),
            vec!["STARRED"]
        );
        assert_eq!(
            labels_for(&f, &[loc("Receipts", true, false)]),
            vec!["f:Receipts"]
        );
        // Mail to yourself: Sent and Inbox; the Sent copy never makes it unread.
        assert_eq!(
            labels_for(&f, &[loc("INBOX", true, false), loc("Sent", false, false)]),
            vec!["INBOX", "SENT"]
        );
        // A copy in Trash wins over the inbox.
        assert_eq!(
            labels_for(&f, &[loc("INBOX", true, false), loc("Trash", true, false)]),
            vec!["TRASH"]
        );
        assert!(labels_for(&f, &[]).is_empty());
    }

    #[test]
    fn gmail_copies_use_their_labels() {
        let f = folders(true);
        let mut l = loc("[Gmail]/All Mail", false, true);
        l.gm_labels = Some(vec![
            "INBOX".into(),
            "IMPORTANT".into(),
            "f:Receipts".into(),
        ]);
        assert_eq!(
            labels_for(&f, &[l]),
            vec!["IMPORTANT", "INBOX", "STARRED", "UNREAD", "f:Receipts"]
        );
        let mut sent = loc("[Gmail]/All Mail", false, false);
        sent.gm_labels = Some(vec!["SENT".into()]);
        assert_eq!(labels_for(&f, &[sent]), vec!["SENT"]);
        assert_eq!(
            labels_for(&f, &[loc("[Gmail]/Trash", true, false)]),
            vec!["TRASH"]
        );
    }

    #[test]
    fn ids_prefer_server_ids_and_fall_back_to_a_stable_hash() {
        let mut item = FetchItem {
            uid: Some(3),
            size: Some(120),
            sections: vec![(
                "BODY[HEADER.FIELDS]".into(),
                Some(
                    b"Message-ID: <a@mail.example>\r\nDate: Mon, 1 Jan 2024 10:00:00 +0000\r\n\r\n"
                        .to_vec(),
                ),
            )],
            ..Default::default()
        };
        let hashed = message_id_of(&item, false).unwrap();
        assert_eq!(
            hashed,
            ids::imap_fallback_message_id(
                Some("a@mail.example"),
                Some("Mon, 1 Jan 2024 10:00:00 +0000"),
                120
            )
        );
        item.emailid = Some("M6d99ac32".into());
        assert_eq!(message_id_of(&item, false).unwrap(), "e:M6d99ac32");
        item.gm_msgid = Some(0x1b2c);
        assert_eq!(message_id_of(&item, true).unwrap(), "gm:1b2c");
        assert_eq!(message_id_of(&item, false).unwrap(), "e:M6d99ac32");
        for id in ["gm:1b2c", "e:M6d99ac32", hashed.as_str()] {
            assert_eq!(ids::check_id(id), None);
        }
    }
}
