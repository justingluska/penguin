//! One IMAP account behind the provider seam ([`MailProvider`]): reading on
//! demand, label deltas as flags and moves, drafts in the Drafts folder,
//! sending over SMTP with the Sent copy filed once, server search.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use penguin_core::query::ParsedQuery;
use penguin_core::{AccountProvider, Address, AttachmentMeta, Message, SearchHit, SentRef};
use penguin_provider::compose::{build_rfc822_draft_with_attachments, AttachmentBytes, Draft};
use penguin_provider::ids::{self, system};
use penguin_provider::{
    async_trait, DraftRef, MailProvider, MessageMetadata, OpenedDraft, ServerSearch,
};

use crate::account::{blocking, labels_for, Ctx, Depth, LARGE_MESSAGE};
use crate::db::{self, Location};
use crate::folders::{self, Folder, FolderSet, Role};
use crate::mime;
use crate::proto::{seqset, Arg};
use crate::search;
use crate::session::{FetchItem, Session};
use crate::smtp::{self, Smtp};
use crate::{Error, Result};

/// Matches listed per folder by a server search.
const SEARCH_LIST: usize = 100;

#[derive(Clone)]
pub struct ImapProvider {
    ctx: Arc<Ctx>,
    in_flight: Arc<Mutex<HashSet<String>>>,
}

/// A copy of an error for a second id (errors aren't Clone).
fn dup(e: &Error) -> Error {
    match e {
        Error::NeedsReauth(m) => Error::NeedsReauth(m.clone()),
        Error::Keychain(m) => Error::Keychain(m.clone()),
        Error::RateLimited => Error::RateLimited,
        Error::Network(m) => Error::Network(m.clone()),
        Error::NotFound(m) => Error::NotFound(m.clone()),
        Error::InvalidInput(m) => Error::InvalidInput(m.clone()),
        Error::Unsupported(m) => Error::Unsupported(m.clone()),
        other => Error::Other(other.to_string()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// What a label delta asks the server to do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// +\Seen (true) / −\Seen (false).
    pub seen: Option<bool>,
    pub flagged: Option<bool>,
    /// Where the thread's movable copies go, if anywhere.
    pub target: Option<Target>,
    /// Which copies move: those in folders with these roles, or with this
    /// user label.
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Role(Role),
    Label(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Role(Role),
    Label(String),
}

/// Translate canonical label deltas for a folder server
/// (docs/PROVIDERS-IMPL.md §4).
pub fn plan(add: &[String], remove: &[String]) -> Plan {
    let has = |list: &[String], l: &str| list.iter().any(|x| x == l);
    let mut p = Plan::default();
    if has(add, system::UNREAD) {
        p.seen = Some(false);
    } else if has(remove, system::UNREAD) {
        p.seen = Some(true);
    }
    if has(add, system::STARRED) {
        p.flagged = Some(true);
    } else if has(remove, system::STARRED) {
        p.flagged = Some(false);
    }
    let added_folder = add.iter().find(|l| l.starts_with(folders::FOLDER_PREFIX));
    let removed_folders: Vec<&String> = remove
        .iter()
        .filter(|l| l.starts_with(folders::FOLDER_PREFIX))
        .collect();
    let everywhere = vec![
        Source::Role(Role::Inbox),
        Source::Role(Role::Archive),
        Source::Role(Role::Other),
        Source::Role(Role::Junk),
        Source::Role(Role::Trash),
    ];
    if has(add, system::SPAM) {
        p.target = Some(Target::Role(Role::Junk));
        p.sources = vec![
            Source::Role(Role::Inbox),
            Source::Role(Role::Archive),
            Source::Role(Role::Other),
        ];
    } else if let Some(f) = added_folder {
        p.target = Some(Target::Label(f.clone()));
        p.sources = vec![
            Source::Role(Role::Inbox),
            Source::Role(Role::Archive),
            Source::Role(Role::Other),
            Source::Role(Role::Junk),
        ];
    } else if has(add, system::INBOX) {
        p.target = Some(Target::Role(Role::Inbox));
        p.sources = everywhere;
    } else if has(remove, system::SPAM) {
        p.target = Some(Target::Role(Role::Inbox));
        p.sources = vec![Source::Role(Role::Junk)];
    } else if has(remove, system::INBOX) {
        p.target = Some(Target::Role(Role::Archive));
        p.sources = vec![Source::Role(Role::Inbox)];
    } else if !removed_folders.is_empty() {
        p.target = Some(Target::Role(Role::Archive));
        p.sources = removed_folders
            .into_iter()
            .map(|l| Source::Label(l.clone()))
            .collect();
    }
    p
}

impl ImapProvider {
    pub fn new(ctx: Arc<Ctx>) -> ImapProvider {
        ImapProvider {
            ctx,
            in_flight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn ctx(&self) -> &Arc<Ctx> {
        &self.ctx
    }

    fn account(&self) -> String {
        self.ctx.cfg.account_id.clone()
    }

    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&penguin_core::Store) -> penguin_core::Result<T> + Send + 'static,
    ) -> Result<T> {
        blocking(&self.ctx.store, f).await
    }

    /// Read one message from the server (None: gone), with labels from
    /// every copy and the stored thread.
    async fn fetch_one(
        &self,
        s: &mut Session,
        folders: &FolderSet,
        id: &str,
    ) -> Result<Option<Message>> {
        let caps = s.caps.clone();
        let gmail = caps.gmail();
        let Some((loc, item)) = self
            .ctx
            .fetch_from_copies(s, folders, id, &Ctx::id_items(&caps))
            .await?
        else {
            return Ok(None);
        };
        let fresh = self
            .ctx
            .location(&loc.folder, loc.uidvalidity, &item, gmail)
            .unwrap_or_else(|| loc.clone());
        {
            let account = self.account();
            let change = vec![(
                fresh.folder.clone(),
                fresh.uid,
                fresh.flags,
                fresh.gm_labels.clone(),
            )];
            self.db(move |st| db::set_flags(st, &account, &change))
                .await?;
        }
        let mut message = if item.size.unwrap_or(0) <= LARGE_MESSAGE {
            let (got, _) = s
                .uid_fetch(&loc.uid.to_string(), "(UID BODY.PEEK[])", None)
                .await?;
            let Some(raw) = got.first().and_then(|g| g.section("BODY[]")) else {
                return Ok(None);
            };
            mime::to_message(raw, &self.meta(id, &item, gmail))
        } else {
            match self.ctx.fetch_large(s, &loc, &item, gmail).await? {
                Some(m) => m,
                None => return Ok(None),
            }
        };
        let (account, mid, folders2) = (self.account(), id.to_string(), folders.clone());
        let (stored_thread, locs) = self
            .db(move |st| {
                let t = st.get_message(&account, &mid)?.map(|m| m.thread_id);
                let l = db::locations(st, &account, std::slice::from_ref(&mid))?
                    .remove(&mid)
                    .unwrap_or_default();
                Ok((t, l))
            })
            .await?;
        message.label_ids = labels_for(&folders2, &locs);
        match stored_thread {
            Some(t) => message.thread_id = t,
            None if !message.thread_id.is_empty() => {}
            None => {
                let (account, input) = (
                    self.account(),
                    crate::threading::Input {
                        message_id: message.id.clone(),
                        own: message.message_id_header.clone(),
                        in_reply_to: message.in_reply_to.clone(),
                        references: message.references.clone(),
                        subject: message.subject.clone(),
                        date_ms: message.date,
                    },
                );
                let _w = self.ctx.write_lock.lock().await;
                message.thread_id = self
                    .db(move |st| crate::threading::assign(st, &account, &input).map(|(t, _)| t))
                    .await?;
            }
        }
        Ok(Some(message))
    }

    fn meta(&self, id: &str, item: &FetchItem, gmail: bool) -> mime::Meta {
        mime::Meta {
            account_id: self.account(),
            id: id.to_string(),
            thread_id: if gmail {
                item.gm_thrid
                    .map(|t| format!("gt:{t:x}"))
                    .unwrap_or_default()
            } else {
                String::new()
            },
            date_ms: item.internal_date,
            labels: Vec::new(),
            trusted_authserv: self.ctx.cfg.trusted_authserv(),
        }
    }

    /// The stored messages of a thread and all their copies.
    async fn thread_copies(&self, thread_id: &str) -> Result<(Vec<String>, Vec<Location>)> {
        let (account, t) = (self.account(), thread_id.to_string());
        let (ids, located) = self
            .db(move |st| {
                let Some(detail) = st.get_thread(&account, &t)? else {
                    return Ok(None);
                };
                let ids: Vec<String> = detail.messages.into_iter().map(|m| m.id).collect();
                let located = db::locations(st, &account, &ids)?;
                Ok(Some((ids, located)))
            })
            .await?
            .ok_or_else(|| Error::NotFound(format!("thread {thread_id} is not stored")))?;
        Ok((ids, located.into_values().flatten().collect()))
    }

    async fn store_flags(
        &self,
        s: &mut Session,
        locs: &[Location],
        flag: &str,
        on: bool,
    ) -> Result<()> {
        let mut by_folder: HashMap<&str, Vec<u32>> = HashMap::new();
        for l in locs {
            by_folder.entry(l.folder.as_str()).or_default().push(l.uid);
        }
        for (folder, uids) in by_folder {
            match s.ensure_selected(folder, true).await {
                Ok(_) => {}
                Err(e) if e.is_not_found() => continue,
                Err(e) => return Err(e),
            }
            let op = if on { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
            s.uid_store(&seqset::format(&uids), op, vec![Arg::raw(flag)])
                .await?;
        }
        let account = self.account();
        let changes: Vec<(String, u32, db::Flags, Option<Vec<String>>)> = locs
            .iter()
            .map(|l| {
                let mut f = l.flags;
                match flag {
                    "\\Seen" => f.seen = on,
                    "\\Flagged" => f.flagged = on,
                    _ => {}
                }
                (l.folder.clone(), l.uid, f, l.gm_labels.clone())
            })
            .collect();
        self.db(move |st| db::set_flags(st, &account, &changes))
            .await
    }

    /// Re-read FLAGS/X-GM-LABELS of copies in one folder into the table.
    async fn reread(&self, s: &mut Session, folder: &str, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let caps = s.caps.clone();
        let sel = s.ensure_selected(folder, false).await?;
        let (items, _) = s
            .uid_fetch(&seqset::format(uids), &Ctx::flag_items(&caps), None)
            .await?;
        let account = self.account();
        let folder = folder.to_string();
        let changes: Vec<(String, u32, db::Flags, Option<Vec<String>>)> = items
            .iter()
            .filter_map(|i| {
                Some((
                    folder.clone(),
                    i.uid?,
                    db::Flags::from_imap(i.flags.as_deref().unwrap_or(&[])),
                    caps.gmail().then(|| crate::account::gm_label_ids(i)),
                ))
            })
            .collect();
        let _ = sel;
        self.db(move |st| db::set_flags(st, &account, &changes))
            .await
    }

    /// Converge the stored labels of `ids` with the location table.
    async fn settle(&self, s: &mut Session, ids: &[String]) -> Result<()> {
        let folders = self.ctx.folders(s, false).await?;
        self.ctx.refresh_labels(&folders, ids).await?;
        Ok(())
    }

    async fn modify_generic(
        &self,
        s: &mut Session,
        folders: &FolderSet,
        locs: &[Location],
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let p = plan(add, remove);
        if let Some(on) = p.flagged {
            self.store_flags(s, locs, "\\Flagged", on).await?;
        }
        if let Some(seen) = p.seen {
            self.store_flags(s, locs, "\\Seen", seen).await?;
        }
        let Some(target) = &p.target else {
            return Ok(());
        };
        let dest: Folder = match target {
            Target::Role(Role::Inbox) => folders
                .by_role(Role::Inbox)
                .cloned()
                .ok_or_else(|| Error::Other("no INBOX".into()))?,
            Target::Role(Role::Archive) => self.ctx.folder_for(s, Role::Archive, "Archive").await?,
            Target::Role(Role::Junk) => self.ctx.folder_for(s, Role::Junk, "Junk").await?,
            Target::Role(Role::Trash) => self.ctx.folder_for(s, Role::Trash, "Trash").await?,
            Target::Role(r) => {
                return Err(Error::Other(format!("can't move to {r:?}")));
            }
            Target::Label(l) => folders.by_label(l).cloned().ok_or_else(|| {
                Error::InvalidInput("That folder doesn't exist on the server anymore.".into())
            })?,
        };
        let matches = |l: &Location| {
            let Some(f) = folders.by_raw(&l.folder) else {
                return false;
            };
            if f.raw == dest.raw {
                return false;
            }
            p.sources.iter().any(|src| match src {
                Source::Role(r) => f.role == *r,
                Source::Label(id) => folders.by_label(id).is_some_and(|x| x.raw == f.raw),
            })
        };
        let mut by_folder: HashMap<String, Vec<Location>> = HashMap::new();
        for l in locs.iter().filter(|l| matches(l)) {
            by_folder
                .entry(l.folder.clone())
                .or_default()
                .push(l.clone());
        }
        for (from, group) in by_folder {
            self.ctx.move_copies(s, &from, &group, &dest).await?;
        }
        Ok(())
    }

    async fn modify_gmail(
        &self,
        s: &mut Session,
        folders: &FolderSet,
        locs: &[Location],
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let p = plan(add, remove);
        if let Some(on) = p.flagged {
            self.store_flags(s, locs, "\\Flagged", on).await?;
        }
        if let Some(seen) = p.seen {
            self.store_flags(s, locs, "\\Seen", seen).await?;
        }
        let all = folders.by_role(Role::All).cloned();
        let has = |list: &[String], l: &str| list.iter().any(|x| x == l);
        // Out of Spam / Trash first (the inbox label can't be added there).
        if has(add, system::INBOX) || has(remove, system::SPAM) {
            let inbox = folders.by_role(Role::Inbox).cloned();
            if let Some(inbox) = inbox {
                let mut by_folder: HashMap<String, Vec<Location>> = HashMap::new();
                for l in locs {
                    if let Some(f) = folders.by_raw(&l.folder) {
                        if matches!(f.role, Role::Junk | Role::Trash) {
                            by_folder
                                .entry(l.folder.clone())
                                .or_default()
                                .push(l.clone());
                        }
                    }
                }
                for (from, group) in by_folder {
                    s.ensure_selected(&from, true).await?;
                    let uids: Vec<u32> = group.iter().map(|l| l.uid).collect();
                    s.uid_move(&seqset::format(&uids), &inbox.raw).await?;
                    let (account, f) = (self.account(), from.clone());
                    self.db(move |st| db::remove(st, &account, &f, &uids).map(|_| ()))
                        .await?;
                    if let Some(all) = &all {
                        for l in &group {
                            if let Some(found) = self.ctx.locate(s, all, &l.message_id).await? {
                                let account = self.account();
                                self.db(move |st| db::put(st, &account, &[found])).await?;
                            }
                        }
                    }
                }
            }
        }
        if has(add, system::SPAM) {
            let spam = self.ctx.folder_for(s, Role::Junk, "[Gmail]/Spam").await?;
            let in_all: Vec<Location> = locs
                .iter()
                .filter(|l| all.as_ref().is_some_and(|a| a.raw == l.folder))
                .cloned()
                .collect();
            if let Some(all) = &all {
                self.ctx.move_copies(s, &all.raw, &in_all, &spam).await?;
            }
            return Ok(());
        }
        let Some(all) = all else {
            return Ok(());
        };
        // Label changes on the All Mail copies (re-read: moves above may
        // have created them).
        let ids: Vec<String> = locs.iter().map(|l| l.message_id.clone()).collect();
        let account = self.account();
        let current: Vec<Location> = self
            .db(move |st| db::locations(st, &account, &ids))
            .await?
            .into_values()
            .flatten()
            .filter(|l| l.folder == all.raw)
            .collect();
        let uids: Vec<u32> = current.iter().map(|l| l.uid).collect();
        if uids.is_empty() {
            return Ok(());
        }
        let label_values = |list: &[String]| -> Vec<Arg> {
            list.iter()
                .filter(|l| {
                    !matches!(
                        l.as_str(),
                        system::UNREAD
                            | system::STARRED
                            | system::SPAM
                            | system::TRASH
                            | system::DRAFT
                    )
                })
                .filter_map(|l| folders::gmail_label_value(l))
                .map(|v| {
                    if v.starts_with('\\') {
                        Arg::raw(v)
                    } else {
                        Arg::string(v.as_bytes())
                    }
                })
                .collect()
        };
        let adds = label_values(add);
        let removes = label_values(remove);
        if adds.is_empty() && removes.is_empty() {
            return Ok(());
        }
        s.ensure_selected(&all.raw, true).await?;
        let set = seqset::format(&uids);
        if !adds.is_empty() {
            s.uid_store(&set, "+X-GM-LABELS.SILENT", adds).await?;
        }
        if !removes.is_empty() {
            s.uid_store(&set, "-X-GM-LABELS.SILENT", removes).await?;
        }
        self.reread(s, &all.raw, &uids).await
    }

    /// File the Sent copy of a message just sent (unless the server did),
    /// store it, and return its (id, thread).
    async fn file_sent(
        &self,
        raw: &[u8],
        thread_hint: Option<&str>,
    ) -> Result<Option<(String, String)>> {
        let env = mime::envelope(raw);
        let Some(mid) = env.message_id.clone() else {
            return Ok(None);
        };
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        if s.caps.gmail() {
            let Some(all) = folders.by_role(Role::All).cloned() else {
                return Ok(None);
            };
            for attempt in 0..4 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(750)).await;
                }
                let sel = s.select(&all.raw, false).await?;
                let uids = s
                    .uid_search(vec![
                        Arg::raw("X-GM-RAW"),
                        Arg::string(format!("rfc822msgid:{mid}").as_bytes()),
                    ])
                    .await?;
                if let Some(uid) = uids.into_iter().max() {
                    return self
                        .ctx
                        .store_uid(s, &all, sel.uidvalidity, uid, thread_hint)
                        .await;
                }
            }
            return Ok(None);
        }
        let sent = self.ctx.folder_for(s, Role::Sent, "Sent").await?;
        let tries = if self.ctx.cfg.server_saves_sent() {
            4
        } else {
            1
        };
        for attempt in 0..tries {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(750)).await;
            }
            let sel = s.select(&sent.raw, false).await?;
            let uids = s
                .uid_search(vec![
                    Arg::raw("HEADER"),
                    Arg::string("Message-ID"),
                    Arg::string(format!("<{mid}>").as_bytes()),
                ])
                .await?;
            if let Some(uid) = uids.into_iter().max() {
                tracing::debug!(account = %self.ctx.cfg.account_id, "the server filed the sent copy itself");
                return self
                    .ctx
                    .store_uid(s, &sent, sel.uidvalidity, uid, thread_hint)
                    .await;
            }
        }
        self.ctx
            .append_and_store(s, &sent, "\\Seen", raw, thread_hint)
            .await
    }

    /// Drop a draft's server copies and local message + mapping; returns
    /// its thread.
    async fn forget_draft(&self, draft_id: &str, server: bool) -> Result<Option<String>> {
        let (account, d) = (self.account(), draft_id.to_string());
        let found = self
            .db(move |st| {
                let Some(mid) = st.message_for_draft(&account, &d)? else {
                    return Ok(None);
                };
                let thread = st.get_message(&account, &mid)?.map(|m| m.thread_id);
                let locs = db::locations(st, &account, std::slice::from_ref(&mid))?
                    .remove(&mid)
                    .unwrap_or_default();
                Ok(Some((mid, thread, locs)))
            })
            .await?;
        let Some((mid, thread, locs)) = found else {
            let (account, d) = (self.account(), draft_id.to_string());
            self.db(move |st| st.remove_draft(&account, &d)).await?;
            return Ok(None);
        };
        if server && !locs.is_empty() {
            let mut g = self.ctx.conn().await?;
            self.ctx.destroy_copies(g.session(), &locs).await?;
        }
        let (account, d) = (self.account(), draft_id.to_string());
        self.db(move |st| {
            st.delete_messages(&account, std::slice::from_ref(&mid))?;
            db::forget_messages(st, &account, std::slice::from_ref(&mid))?;
            st.remove_draft(&account, &d)
        })
        .await?;
        Ok(thread)
    }
}

/// The ids a send returns when the stored copy couldn't be found: what
/// sync will compute for an exact copy, and the thread it will join.
fn fallback_sent_ref(raw: &[u8], thread_hint: Option<&str>) -> SentRef {
    let env = mime::envelope(raw);
    let message_id = ids::imap_fallback_message_id(
        env.message_id.as_deref(),
        env.date_raw.as_deref(),
        raw.len() as u64,
    );
    let thread_id = thread_hint.map(str::to_string).unwrap_or_else(|| {
        let root = env
            .references
            .first()
            .cloned()
            .or(env.in_reply_to.clone())
            .or(env.message_id.clone());
        match root {
            Some(r) => ids::imap_thread_id(&r),
            None => ids::imap_thread_id(&message_id),
        }
    });
    SentRef {
        message_id,
        thread_id,
    }
}

fn bare(id: &str) -> &str {
    id.trim().trim_start_matches('<').trim_end_matches('>')
}

#[async_trait]
impl MailProvider for ImapProvider {
    fn account_id(&self) -> &str {
        &self.ctx.cfg.account_id
    }

    fn provider(&self) -> AccountProvider {
        AccountProvider::Imap
    }

    async fn fetch_messages(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        let mut g = match self.ctx.conn().await {
            Ok(g) => g,
            Err(e) => return ids.iter().map(|id| (id.clone(), Err(dup(&e)))).collect(),
        };
        let s = g.session();
        let folders = match self.ctx.folders(s, false).await {
            Ok(f) => f,
            Err(e) => return ids.iter().map(|id| (id.clone(), Err(dup(&e)))).collect(),
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let r = self.fetch_one(s, &folders, id).await;
            out.push((id.clone(), r));
        }
        out
    }

    async fn fetch_pending_bodies(&self, ids: &[String]) -> Result<Vec<String>> {
        let (account, probe) = (self.account(), ids.to_vec());
        let pending: Vec<String> = self
            .db(move |st| {
                let mut out = Vec::new();
                for id in probe {
                    if st.is_body_pending(&account, &id)? == Some(true) {
                        out.push(id);
                    }
                }
                Ok(out)
            })
            .await?;
        let mine: Vec<String> = {
            let mut f = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
            pending
                .into_iter()
                .filter(|id| f.insert(id.clone()))
                .collect()
        };
        if mine.is_empty() {
            return Ok(Vec::new());
        }
        let result = async {
            let fetched = self.fetch_messages(&mine).await;
            let mut messages = Vec::new();
            let mut last_err = None;
            for (_, r) in fetched {
                match r {
                    Ok(Some(m)) => messages.push(m),
                    Ok(None) => {}
                    Err(e) => last_err = Some(e),
                }
            }
            if messages.is_empty() {
                if let Some(e) = last_err {
                    return Err(e);
                }
                return Ok(Vec::new());
            }
            let threads: BTreeSet<String> = messages.iter().map(|m| m.thread_id.clone()).collect();
            let _w = self.ctx.write_lock.lock().await;
            self.db(move |st| st.upsert_messages(&messages)).await?;
            Ok(threads.into_iter().collect())
        }
        .await;
        let mut f = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
        for id in &mine {
            f.remove(id);
        }
        result
    }

    async fn get_attachment(
        &self,
        message_id: &str,
        attachment: &AttachmentMeta,
    ) -> Result<Vec<u8>> {
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        let Some((loc, item)) = self
            .ctx
            .fetch_from_copies(s, &folders, message_id, "(UID BODYSTRUCTURE)")
            .await?
        else {
            return Err(Error::NotFound(
                "the message is no longer on the server".into(),
            ));
        };
        let part = item
            .bodystructure
            .as_ref()
            .and_then(mime::parse_bodystructure)
            .and_then(|bs| bs.find(&attachment.id).cloned());
        if let Some(part) = part.filter(|p| !p.mime.starts_with("multipart/")) {
            let spec = format!("(UID BODY.PEEK[{}])", part.path);
            let (got, _) = s.uid_fetch(&loc.uid.to_string(), &spec, None).await?;
            if let Some(bytes) = got
                .first()
                .and_then(|g| g.section(&format!("BODY[{}]", part.path)))
            {
                let decoded = if part.mime.starts_with("message/") {
                    bytes.to_vec()
                } else {
                    mime::decode_transfer(bytes, &part.encoding)
                };
                if !decoded.is_empty() {
                    return Ok(decoded);
                }
            }
        }
        // Structure didn't match: take it from the whole message.
        let (got, _) = s
            .uid_fetch(&loc.uid.to_string(), "(UID BODY.PEEK[])", None)
            .await?;
        got.first()
            .and_then(|g| g.section("BODY[]"))
            .and_then(|raw| mime::part_bytes(raw, &attachment.id))
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Error::NotFound("the attachment is no longer on the server".into()))
    }

    async fn get_message_metadata(&self, message_id: &str) -> Result<Option<MessageMetadata>> {
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        let Some((_, item)) = self
            .ctx
            .fetch_from_copies(
                s,
                &folders,
                message_id,
                "(UID RFC822.SIZE BODY.PEEK[HEADER])",
            )
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(MessageMetadata {
            headers: mime::header_fields(item.section("BODY[HEADER]").unwrap_or_default()),
            size_estimate: item.size,
        }))
    }

    async fn get_message_raw(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        Ok(self
            .ctx
            .fetch_from_copies(s, &folders, message_id, "(UID BODY.PEEK[])")
            .await?
            .and_then(|(_, i)| i.section("BODY[]").map(<[u8]>::to_vec)))
    }

    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let (ids, locs) = self.thread_copies(thread_id).await?;
        if locs.is_empty() {
            return Ok(());
        }
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        if s.caps.gmail() {
            self.modify_gmail(s, &folders, &locs, add, remove).await?;
        } else {
            self.modify_generic(s, &folders, &locs, add, remove).await?;
        }
        self.settle(s, &ids).await
    }

    /// A folder named `name` (on Gmail, a label): found by its shown name,
    /// else created (and subscribed) at the top level.
    async fn ensure_label(&self, name: &str) -> Result<penguin_core::Label> {
        let account = self.account();
        let find = |set: &FolderSet| {
            set.labels(&account)
                .into_iter()
                .find(|l| l.kind == "user" && l.name.eq_ignore_ascii_case(name))
        };
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        if let Some(l) = find(&*self.ctx.folders(s, true).await?) {
            return Ok(l);
        }
        s.create(&crate::proto::utf7::encode(name)).await?;
        find(&*self.ctx.folders(s, true).await?)
            .ok_or_else(|| Error::Other(format!("couldn't create the {name} folder")))
    }

    async fn trash_thread(&self, thread_id: &str) -> Result<()> {
        let (ids, locs) = self.thread_copies(thread_id).await?;
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        let trash = self.ctx.folder_for(s, Role::Trash, "Trash").await?;
        let mut by_folder: HashMap<String, Vec<Location>> = HashMap::new();
        for l in locs.into_iter().filter(|l| l.folder != trash.raw) {
            // Gmail: only the All Mail copy moves (Spam copies too).
            if folders.gmail
                && !folders
                    .by_raw(&l.folder)
                    .is_some_and(|f| matches!(f.role, Role::All | Role::Junk))
            {
                continue;
            }
            by_folder.entry(l.folder.clone()).or_default().push(l);
        }
        for (from, group) in by_folder {
            self.ctx.move_copies(s, &from, &group, &trash).await?;
        }
        self.settle(s, &ids).await
    }

    async fn untrash_thread(&self, thread_id: &str) -> Result<()> {
        let (ids, locs) = self.thread_copies(thread_id).await?;
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        let (Some(trash), Some(inbox)) = (
            folders.by_role(Role::Trash).cloned(),
            folders.by_role(Role::Inbox).cloned(),
        ) else {
            return Ok(());
        };
        let group: Vec<Location> = locs.into_iter().filter(|l| l.folder == trash.raw).collect();
        if folders.gmail {
            if !group.is_empty() {
                s.ensure_selected(&trash.raw, true).await?;
                let uids: Vec<u32> = group.iter().map(|l| l.uid).collect();
                s.uid_move(&seqset::format(&uids), &inbox.raw).await?;
                let (account, f) = (self.account(), trash.raw.clone());
                self.db(move |st| db::remove(st, &account, &f, &uids).map(|_| ()))
                    .await?;
                if let Some(all) = folders.by_role(Role::All).cloned() {
                    for l in &group {
                        if let Some(found) = self.ctx.locate(s, &all, &l.message_id).await? {
                            let account = self.account();
                            self.db(move |st| db::put(st, &account, &[found])).await?;
                        }
                    }
                }
            }
        } else {
            self.ctx.move_copies(s, &trash.raw, &group, &inbox).await?;
        }
        self.settle(s, &ids).await
    }

    async fn send_raw(&self, raw: &[u8], thread_id: Option<&str>) -> Result<SentRef> {
        let cred = self.ctx.credential().await?;
        let (from, rcpts, data) = smtp::envelope(raw)?;
        let password = cred.smtp_password.as_deref().unwrap_or(&cred.password);
        let mut conn = Smtp::connect(&self.ctx.cfg.smtp).await?;
        conn.login(&self.ctx.cfg.smtp.username, password).await?;
        conn.send(&from, &rcpts, &data).await?;
        conn.quit().await;
        tracing::info!(account = %self.ctx.cfg.account_id, recipients = rcpts.len(), "message sent over SMTP");
        // The message is out: filing the Sent copy must never fail the send.
        match self.file_sent(raw, thread_id).await {
            Ok(Some((message_id, thread_id))) => Ok(SentRef {
                message_id,
                thread_id,
            }),
            Ok(None) => Ok(fallback_sent_ref(raw, thread_id)),
            Err(e) => {
                tracing::warn!(account = %self.ctx.cfg.account_id, error = %e, "sent, but couldn't file the Sent copy");
                Ok(fallback_sent_ref(raw, thread_id))
            }
        }
    }

    async fn save_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: Option<&str>,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        let (in_reply_to, references) = self.reply_headers(draft).await?;
        let raw = build_rfc822_draft_with_attachments(
            draft,
            from,
            in_reply_to.as_deref(),
            &references,
            attachments,
        )?;
        let draft_id = draft_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("d:{:016x}{:08x}", fastrand::u64(..), fastrand::u32(..)));
        let (account, d) = (self.account(), draft_id.clone());
        let previous = self
            .db(move |st| st.message_for_draft(&account, &d))
            .await?;
        let (mid, thread) = {
            let mut g = self.ctx.conn().await?;
            let s = g.session();
            let drafts = self.ctx.folder_for(s, Role::Drafts, "Drafts").await?;
            let stored = self
                .ctx
                .append_and_store(
                    s,
                    &drafts,
                    "\\Draft \\Seen",
                    &raw,
                    draft.reply_to_thread_id.as_deref(),
                )
                .await?;
            let Some((mid, thread)) = stored else {
                return Err(Error::Other("the server didn't keep the draft".into()));
            };
            if let Some(prev) = previous.as_ref().filter(|p| **p != mid) {
                let (account, p) = (self.account(), prev.clone());
                let locs = self
                    .db(move |st| db::locations(st, &account, std::slice::from_ref(&p)))
                    .await?
                    .remove(prev)
                    .unwrap_or_default();
                if let Err(e) = self.ctx.destroy_copies(s, &locs).await {
                    tracing::warn!(account = %self.ctx.cfg.account_id, error = %e, "couldn't remove the previous draft version");
                }
            }
            (mid, thread)
        };
        let (account, d, m, prev) = (self.account(), draft_id.clone(), mid.clone(), previous);
        let stored = self
            .db(move |st| {
                if let Some(old) = prev.filter(|old| *old != m) {
                    st.delete_messages(&account, std::slice::from_ref(&old))?;
                    db::forget_messages(st, &account, &[old])?;
                }
                st.set_draft(&account, &d, &m)?;
                st.get_message(&account, &m)
            })
            .await?;
        Ok(DraftRef {
            draft_id,
            message_id: mid,
            thread_id: thread,
            // The stored copy's parts, in the composer's order (inline
            // images by Content-ID).
            attachments: stored
                .as_ref()
                .map(|m| penguin_provider::inline::saved_refs(&m.id, &m.attachments, attachments))
                .unwrap_or_default(),
        })
    }

    async fn send_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: &str,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        let (in_reply_to, references) = self.reply_headers(draft).await?;
        let raw = penguin_provider::compose::build_rfc822_with_attachments(
            draft,
            from,
            in_reply_to.as_deref(),
            &references,
            attachments,
        )?;
        let sent = self
            .send_raw(&raw, draft.reply_to_thread_id.as_deref())
            .await?;
        if let Err(e) = self.forget_draft(draft_id, true).await {
            tracing::warn!(account = %self.ctx.cfg.account_id, error = %e, "sent, but couldn't remove the draft");
        }
        Ok(DraftRef {
            draft_id: draft_id.to_string(),
            message_id: sent.message_id,
            thread_id: sent.thread_id,
            attachments: Vec::new(),
        })
    }

    async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef> {
        let (account, d) = (self.account(), draft_id.to_string());
        let found = self
            .db(move |st| {
                let Some(mid) = st.message_for_draft(&account, &d)? else {
                    return Ok(None);
                };
                let thread = st.get_message(&account, &mid)?.map(|m| m.thread_id);
                Ok(Some((mid, thread)))
            })
            .await?;
        let Some((mid, thread)) = found else {
            return Err(Error::NotFound(format!(
                "draft {draft_id} no longer exists"
            )));
        };
        let raw = {
            let mut g = self.ctx.conn().await?;
            let s = g.session();
            let folders = self.ctx.folders(s, false).await?;
            self.ctx
                .fetch_from_copies(s, &folders, &mid, "(UID BODY.PEEK[])")
                .await?
                .and_then(|(_, i)| i.section("BODY[]").map(<[u8]>::to_vec))
        };
        let Some(raw) = raw else {
            self.forget_draft(draft_id, false).await?;
            return Err(Error::NotFound(format!(
                "draft {draft_id} no longer exists"
            )));
        };
        // Unmap first: a retry after a crash mid-send finds nothing to send
        // (never a double send). Restored if the send fails.
        let (account, d) = (self.account(), draft_id.to_string());
        self.db(move |st| st.remove_draft(&account, &d)).await?;
        match self.send_raw(&raw, thread.as_deref()).await {
            Ok(sent) => {
                let (account, d, m) = (self.account(), draft_id.to_string(), mid.clone());
                self.db(move |st| st.set_draft(&account, &d, &m)).await?;
                if let Err(e) = self.forget_draft(draft_id, true).await {
                    tracing::warn!(account = %self.ctx.cfg.account_id, error = %e, "sent, but couldn't remove the draft");
                    let (account, d) = (self.account(), draft_id.to_string());
                    self.db(move |st| st.remove_draft(&account, &d)).await?;
                }
                Ok(sent)
            }
            Err(e) => {
                let (account, d, m) = (self.account(), draft_id.to_string(), mid);
                self.db(move |st| st.set_draft(&account, &d, &m)).await?;
                Err(e)
            }
        }
    }

    async fn delete_draft(&self, draft_id: &str) -> Result<Option<String>> {
        self.forget_draft(draft_id, true).await
    }

    async fn open_draft(
        &self,
        draft_id: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<Option<OpenedDraft>> {
        let account = self.account();
        let (draft_id, message_id) = match (draft_id, message_id) {
            (Some(d), _) => {
                let (a, dd) = (account.clone(), d.to_string());
                match self.db(move |st| st.message_for_draft(&a, &dd)).await? {
                    Some(m) => (d.to_string(), m),
                    None => return Ok(None),
                }
            }
            (None, Some(m)) => {
                let (a, mm) = (account.clone(), m.to_string());
                let mapped = self
                    .db(move |st| {
                        if let Some(d) = st.draft_for_message(&a, &mm)? {
                            return Ok(Some(d));
                        }
                        // A draft made elsewhere: give it a stable draft id.
                        let is_draft = st
                            .get_message(&a, &mm)?
                            .is_some_and(|m| m.label_ids.iter().any(|l| l == system::DRAFT));
                        if !is_draft {
                            return Ok(None);
                        }
                        let d = format!("d:{}", mm.replace(':', "-"));
                        st.set_draft(&a, &d, &mm)?;
                        Ok(Some(d))
                    })
                    .await?;
                match mapped {
                    Some(d) => (d, m.to_string()),
                    None => return Ok(None),
                }
            }
            (None, None) => {
                return Err(Error::Other(
                    "open_draft needs a draftId or a messageId".into(),
                ))
            }
        };
        let (a, m) = (account.clone(), message_id.clone());
        let mut message = self.db(move |st| st.get_message(&a, &m)).await?;
        if message.is_none() {
            if let Some((_, Ok(Some(m)))) = self
                .fetch_messages(std::slice::from_ref(&message_id))
                .await
                .pop()
            {
                let copy = m.clone();
                self.db(move |st| st.upsert_messages(&[copy])).await?;
                message = Some(m);
            }
        }
        let Some(message) = message else {
            return Ok(None);
        };
        let (a, t) = (account.clone(), message.thread_id.clone());
        let thread = self.db(move |st| st.get_thread(&a, &t)).await?;
        let others: Vec<&Message> = thread
            .as_ref()
            .map(|t| t.messages.iter().filter(|m| m.id != message.id).collect())
            .unwrap_or_default();
        let parent = message
            .in_reply_to
            .as_deref()
            .and_then(|irt| {
                others
                    .iter()
                    .find(|m| m.message_id_header.as_deref().map(bare) == Some(bare(irt)))
            })
            .map(|m| m.id.clone());
        let reply_to_thread_id =
            (parent.is_some() || !others.is_empty()).then(|| message.thread_id.clone());
        // The HTML through the editor's allowlist with the inline images it
        // shows; their parts come back as refs next to the files.
        let (body_html, attachments) = penguin_provider::inline::reopen(&message);
        Ok(Some(OpenedDraft {
            draft_id,
            message_id: message.id.clone(),
            thread_id: message.thread_id.clone(),
            draft: Draft {
                request_read_receipt: None,
                account_id: account,
                to: message.to.clone(),
                cc: message.cc.clone(),
                bcc: message.bcc.clone(),
                subject: message.subject.clone(),
                body_text: message.body_text.clone(),
                body_html,
                reply_to_thread_id,
                reply_to_message_id: parent,
                attachments,
            },
        }))
    }

    async fn server_search(&self, query: &ParsedQuery, fetch_cap: usize) -> Result<ServerSearch> {
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        let gmail = s.caps.gmail();
        // (folder, matching uids newest first)
        let mut matches: Vec<(Folder, u32, Vec<u32>)> = Vec::new();
        if gmail {
            let q = search::gmail_query(query);
            for f in folders.synced() {
                let wanted = f.role == Role::All
                    || (f.role == Role::Trash && (query.include_trash || q.contains("in:trash")))
                    || (f.role == Role::Junk && (query.include_spam || q.contains("in:spam")));
                if !wanted {
                    continue;
                }
                let sel = s.select(&f.raw, true).await?;
                let mut uids = s
                    .uid_search(vec![Arg::raw("X-GM-RAW"), Arg::string(q.as_bytes())])
                    .await?;
                uids.sort_unstable_by(|a, b| b.cmp(a));
                matches.push((f.clone(), sel.uidvalidity, uids));
            }
        } else {
            let (criteria, scope, _) = search::imap_criteria(query);
            for f in folders.synced() {
                let wanted = if !scope.folder_names.is_empty() {
                    scope.folder_names.iter().any(|n| {
                        n.eq_ignore_ascii_case(&f.display) || n.eq_ignore_ascii_case(&f.path)
                    })
                } else if !scope.roles.is_empty() {
                    scope.roles.contains(&f.role)
                } else {
                    match f.role {
                        Role::Trash => scope.include_trash,
                        Role::Junk => scope.include_spam,
                        Role::Drafts => false,
                        Role::Inbox => !scope.not_inbox,
                        _ => true,
                    }
                };
                if !wanted {
                    continue;
                }
                let sel = match s.select(&f.raw, true).await {
                    Ok(sel) => sel,
                    Err(e) if e.is_not_found() => continue,
                    Err(e) => return Err(e),
                };
                if sel.exists == 0 {
                    continue;
                }
                let mut uids = s.uid_search(criteria.clone()).await?;
                uids.sort_unstable_by(|a, b| b.cmp(a));
                matches.push((f.clone(), sel.uidvalidity, uids));
            }
        }
        let estimate: u64 = matches.iter().map(|(_, _, u)| u.len() as u64).sum();
        let caps = s.caps.clone();
        let mut all_ids: Vec<String> = Vec::new();
        let mut budget = fetch_cap;
        let mut fetched = 0u32;
        for (folder, uidvalidity, uids) in matches {
            let listed: Vec<u32> = uids.into_iter().take(SEARCH_LIST).collect();
            if listed.is_empty() {
                continue;
            }
            s.select(&folder.raw, true).await?;
            let (items, _) = s
                .uid_fetch(&seqset::format(&listed), &Ctx::id_items(&caps), None)
                .await?;
            let with_ids: Vec<(String, FetchItem)> = items
                .into_iter()
                .filter_map(|i| Some((crate::account::message_id_of(&i, gmail)?, i)))
                .collect();
            let (account, probe) = (
                self.account(),
                with_ids
                    .iter()
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>(),
            );
            let known = self
                .db(move |st| st.known_message_ids(&account, &probe))
                .await?;
            let mut keep = Vec::new();
            for (id, item) in with_ids {
                all_ids.push(id.clone());
                if known.contains(&id) {
                    keep.push(item);
                } else if budget > 0 {
                    budget -= 1;
                    keep.push(item);
                }
            }
            let got = self
                .ctx
                .ingest(s, &folders, &folder, uidvalidity, keep, Depth::Headers)
                .await?;
            fetched += got.new_ids.len() as u32;
        }
        let (account, want) = (self.account(), all_ids);
        let messages = self
            .db(move |st| {
                let mut out = Vec::new();
                let mut seen = HashSet::new();
                for id in &want {
                    if seen.insert(id.clone()) {
                        if let Some(m) = st.get_message(&account, id)? {
                            out.push(m);
                        }
                    }
                }
                Ok(out)
            })
            .await?;
        let mut sorted = messages;
        sorted.sort_by_key(|m| std::cmp::Reverse(m.date));
        let mut hits: Vec<SearchHit> = Vec::new();
        for m in sorted {
            if let Some(h) = hits.iter_mut().find(|h| h.thread_id == m.thread_id) {
                h.match_count += 1;
                continue;
            }
            let mut snippet_html = String::new();
            penguin_core::text::escape_html(&m.snippet, &mut snippet_html);
            hits.push(SearchHit {
                account_id: m.account_id.clone(),
                thread_id: m.thread_id.clone(),
                message_id: m.id.clone(),
                subject: m.subject.clone(),
                from: m.from.clone(),
                date: m.date,
                snippet_html,
                match_count: 1,
                has_attachments: m.attachments.iter().any(|a| !a.inline),
                unread: m.is_unread(),
                label_ids: m.label_ids,
                score: 0.0,
                matched_by: Vec::new(),
                passage: None,
            });
        }
        Ok(ServerSearch {
            hits,
            fetched,
            estimate,
        })
    }

    async fn window_estimate(&self, months: u32, now_ms: i64) -> Result<u64> {
        const TTL: Duration = Duration::from_secs(30 * 60);
        type Cache = Mutex<HashMap<(String, u32), (u64, Instant)>>;
        static CACHE: OnceLock<Cache> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let key = (self.account(), months);
        if let Some((n, at)) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            if at.elapsed() < TTL {
                return Ok(*n);
            }
        }
        let mut g = self.ctx.conn().await?;
        let s = g.session();
        let folders = self.ctx.folders(s, false).await?;
        let since = penguin_provider::window::window_start_ms(now_ms, months);
        let mut total = 0u64;
        for f in folders.synced() {
            if matches!(f.role, Role::Trash | Role::Junk) {
                continue;
            }
            if months == 0 {
                total += s
                    .status(&f.raw, "MESSAGES")
                    .await?
                    .get("MESSAGES")
                    .copied()
                    .unwrap_or(0);
            } else {
                s.select(&f.raw, true).await?;
                total += s
                    .uid_search(vec![Arg::raw(format!(
                        "SINCE {}",
                        crate::session::search_date(since)
                    ))])
                    .await?
                    .len() as u64;
            }
        }
        cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, (total, Instant::now()));
        Ok(total)
    }
}

impl ImapProvider {
    /// In-Reply-To and References for a reply to the stored parent.
    async fn reply_headers(&self, draft: &Draft) -> Result<(Option<String>, Vec<String>)> {
        let Some(parent_id) = draft.reply_to_message_id.clone() else {
            return Ok((None, Vec::new()));
        };
        let account = draft.account_id.clone();
        let parent = self
            .db(move |st| st.get_message(&account, &parent_id))
            .await?;
        Ok(parent
            .map(|p| (p.message_id_header, p.references))
            .unwrap_or_default())
    }

    /// Current time in ms (drafts' local mirror).
    pub fn now_ms() -> i64 {
        now_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn deltas_become_flags_and_moves() {
        let p = plan(&s(&["STARRED"]), &s(&["UNREAD"]));
        assert_eq!(p.flagged, Some(true));
        assert_eq!(p.seen, Some(true));
        assert_eq!(p.target, None);

        let archive = plan(&[], &s(&["INBOX"]));
        assert_eq!(archive.target, Some(Target::Role(Role::Archive)));
        assert_eq!(archive.sources, vec![Source::Role(Role::Inbox)]);

        let to_inbox = plan(&s(&["INBOX", "UNREAD"]), &[]);
        assert_eq!(to_inbox.target, Some(Target::Role(Role::Inbox)));
        assert_eq!(to_inbox.seen, Some(false));
        assert!(to_inbox.sources.contains(&Source::Role(Role::Trash)));
        assert!(!to_inbox.sources.contains(&Source::Role(Role::Sent)));

        // "Move to…" a folder: the thread's inbox/folder copies go there.
        let mv = plan(&s(&["f:Receipts"]), &s(&["INBOX", "f:Old"]));
        assert_eq!(mv.target, Some(Target::Label("f:Receipts".into())));
        assert!(mv.sources.contains(&Source::Role(Role::Inbox)));

        let unfile = plan(&[], &s(&["f:Receipts"]));
        assert_eq!(unfile.target, Some(Target::Role(Role::Archive)));
        assert_eq!(unfile.sources, vec![Source::Label("f:Receipts".into())]);

        let spam = plan(&s(&["SPAM"]), &s(&["INBOX"]));
        assert_eq!(spam.target, Some(Target::Role(Role::Junk)));
        let not_spam = plan(&[], &s(&["SPAM"]));
        assert_eq!(not_spam.target, Some(Target::Role(Role::Inbox)));
        assert_eq!(not_spam.sources, vec![Source::Role(Role::Junk)]);
    }

    #[test]
    fn sent_refs_without_a_stored_copy_are_valid_and_threaded() {
        let raw = b"From: a@mail.example\r\nTo: b@mail.example\r\nMessage-ID: <x1@mail.example>\r\nIn-Reply-To: <p@mail.example>\r\nReferences: <root@mail.example> <p@mail.example>\r\nDate: Mon, 1 Jan 2024 10:00:00 +0000\r\n\r\nhi\r\n";
        let r = fallback_sent_ref(raw, None);
        assert_eq!(ids::check_id(&r.message_id), None);
        assert_eq!(r.thread_id, ids::imap_thread_id("root@mail.example"));
        assert_eq!(fallback_sent_ref(raw, Some("t:abc")).thread_id, "t:abc");
    }
}
