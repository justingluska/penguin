//! The Microsoft sync engine and [`GraphBackend`] (docs/PROVIDERS-IMPL.md
//! §2.2–2.3, §5).
//!
//! Per account, one tokio task:
//! 1. Folders: the well-known folders, the folder tree and the categories
//!    become the label list (`folders.rs`), refreshed every 10 minutes and
//!    whenever an unknown folder or category shows up.
//! 2. Backfill = each synced folder's first delta round
//!    (`/me/mailFolders/{id}/messages/delta`), inbox first, newest first
//!    (`$orderby=receivedDateTime desc`; plain delta if Graph refuses it).
//!    Delta pages carry light fields only; messages inside the sync window
//!    are then fetched in full, older ones stored headers-only (or in full,
//!    or not at all, per `OlderMail`). An ordered page's bodies come from
//!    one folder listing of its date range (`list_full_range`), not a GET
//!    per message: Outlook limits requests per mailbox, not bytes. The page link is saved after every
//!    page, so a restart resumes mid-folder. A delta round's state is taken
//!    when it starts, so anything that changes during a long backfill comes
//!    in the next round: the anchor is recorded before the backfill by
//!    construction.
//! 3. Incremental: every 30 s (or when poked) each folder's `deltaLink`
//!    round runs: adds, flag/read/category changes, and removals. A move
//!    shows as a removal in one folder and an add in another; the location
//!    table (`locations.rs`) keeps that from deleting the message. New mail
//!    is reported through `messages_added` before the cursor that covers it
//!    is saved.
//! 4. `410 Gone` on a delta link: that folder starts a fresh round; at its
//!    end, stored messages the server no longer lists there are removed.
//!    Ids are immutable, so nothing is duplicated.
//! 5. A larger window (or older mail switched to full / headers) after the
//!    backfill runs a range pass over `/me/messages`, resumable through the
//!    generic `WindowCursor` fields.
//! 6. Messages that keep failing on their own are parked in
//!    `failed_message_ids` and retried every 5 minutes.
//! 7. NeedsReauth / Keychain errors stop the task; others back off 5 s → 5 min.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use penguin_core::{
    Account, AccountProvider, Message, Store, SyncCursor, SyncPhase, SyncStage, SyncStatus,
};
use penguin_provider::heartbeat::Heartbeat;
use penguin_provider::ids::system;
use penguin_provider::window::{window_start_ms, OlderMail, WindowPolicy};
use penguin_provider::{
    async_trait, Backend, Error, MailProvider, Result, SyncHandle, SyncObserver, SyncTask,
};
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::auth::Auth;
use crate::client::{enc, Fetched, GraphClient};
use crate::convert::{format_date, labels_for, parse_date_ms};
use crate::cursor::{FolderCursor, GraphCursor};
use crate::folders::{is_folder_label, Folder, FolderMap};
use crate::http::{graph_code, GraphApi, Transport};
use crate::locations;
use crate::provider::GraphProvider;
use crate::wire::{Page, WireMessage, SELECT_LIGHT};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
const LABEL_REFRESH: Duration = Duration::from_secs(10 * 60);
const STATUS_TICK: Duration = Duration::from_secs(1);
const MAX_ERROR_BACKOFF: Duration = Duration::from_secs(5 * 60);
const FAILED_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);
const FAILED_RETRY_BATCH: usize = 50;
/// Delta and listing page size (light fields only).
const PAGE_SIZE: u32 = 100;
/// Messages fetched in full per store transaction.
const FETCH_CHUNK: usize = 20;
/// A backfill page needing at least this many bodies lists them in full by
/// date range (one request) rather than one GET each…
const RANGE_MIN: usize = 8;
/// …when they are at least this share of the page's messages in that range
/// (so the listing doesn't mostly bring mail already stored).
const RANGE_DENSITY: f64 = 0.5;
/// Listing requests one range may take before the rest goes one by one.
const RANGE_MAX_PAGES: usize = 3;
/// Pages one folder may take per incremental poll (the rest next poll).
const MAX_POLL_PAGES: usize = 50;
/// "Every message" as a delta filter (it enables `$orderby`).
const ANCIENT: &str = "1900-01-01T00:00:00Z";
const RATE_WINDOW: Duration = Duration::from_secs(60);
const RATE_MIN_SPAN: Duration = Duration::from_secs(15);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// State shared between an account's task, its handle and the backend.
pub(crate) struct Shared {
    status: Mutex<SyncStatus>,
    poked: AtomicBool,
    wake: Notify,
    observer: Arc<dyn SyncObserver>,
    policy: Mutex<WindowPolicy>,
    /// Progress heartbeat, ticking only while backfilling.
    heartbeat: Heartbeat,
}

impl Shared {
    pub(crate) fn new(
        account_id: &str,
        observer: Arc<dyn SyncObserver>,
        policy: WindowPolicy,
    ) -> Arc<Self> {
        Arc::new(Shared {
            status: Mutex::new(SyncStatus {
                account_id: account_id.to_string(),
                phase: SyncPhase::Idle,
                indexed: 0,
                total_estimate: None,
                last_synced_at: None,
                error: None,
                rate_per_min: None,
                eta_secs: None,
                stage: None,
            }),
            poked: AtomicBool::new(false),
            wake: Notify::new(),
            observer,
            policy: Mutex::new(policy),
            heartbeat: Heartbeat::default(),
        })
    }

    fn snapshot(&self) -> SyncStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn update(&self, f: impl FnOnce(&mut SyncStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        self.heartbeat.phase(snapshot.phase);
        self.observer.status(snapshot);
    }

    pub(crate) fn poke(&self) {
        self.poked.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    fn policy(&self) -> WindowPolicy {
        *self.policy.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Stored messages per minute over the last minute (none before 15 s).
struct Rate {
    since: Instant,
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    fn new() -> Rate {
        Rate {
            since: Instant::now(),
            samples: VecDeque::new(),
        }
    }
    fn record(&mut self, stored: u64) -> Option<f64> {
        let now = Instant::now();
        self.samples.push_back((now, stored));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > RATE_WINDOW)
        {
            self.samples.pop_front();
        }
        let span = now.duration_since(self.since).min(RATE_WINDOW);
        if span < RATE_MIN_SPAN {
            return None;
        }
        let total: u64 = self.samples.iter().map(|(_, n)| n).sum();
        Some(total as f64 / span.as_secs_f64() * 60.0)
    }
}

/// Errors that indict one message rather than the account or the network.
fn is_message_specific(e: &Error) -> bool {
    match e {
        Error::Http { status, .. } => !matches!(status, 401 | 403 | 429),
        Error::Other(_) => true,
        _ => false,
    }
}

/// A delta link Graph no longer accepts: start the folder over.
fn is_stale_delta(e: &Error) -> bool {
    match e {
        Error::Http { status: 410, .. } => true,
        Error::Http { .. } => graph_code(e).is_some_and(|c| {
            let c = c.to_ascii_lowercase();
            c.contains("syncstate") || c.contains("resyncrequired")
        }),
        _ => false,
    }
}

/// The label changes that bring `local` to what the server says. Only
/// what the change reports is compared: a partial delta entry without
/// `isRead` leaves UNREAD alone.
pub(crate) fn label_diff(
    local: &[String],
    w: &WireMessage,
    desired: &[String],
) -> (Vec<String>, Vec<String>) {
    let managed = |l: &str| {
        is_folder_label(l)
            || (l == system::UNREAD && w.is_read.is_some())
            || (l == system::STARRED && w.flag.is_some())
            || (l.starts_with("c:") && w.categories.is_some())
    };
    let add = desired
        .iter()
        .filter(|l| !local.contains(l))
        .cloned()
        .collect();
    let remove = local
        .iter()
        .filter(|l| managed(l) && !desired.contains(l))
        .cloned()
        .collect();
    (add, remove)
}

/// One account's sync state machine.
pub(crate) struct AccountSync {
    client: Arc<GraphClient>,
    shared: Arc<Shared>,
    cursor: SyncCursor,
    graph: GraphCursor,
    next_poll: Instant,
    last_refresh: Option<Instant>,
    last_failed_retry: Option<Instant>,
    failures: u32,
    /// Folders re-listed after a stale delta link: ids seen so far this round.
    resync: HashMap<String, HashSet<String>>,
    /// Each folder's label as of the last refresh (to relabel on change).
    folder_labels: HashMap<String, Option<String>>,
    /// Categories seen on messages (kept in the label list).
    categories: BTreeSet<String>,
    rate: Rate,
}

#[derive(Default)]
struct PageResult {
    threads: Vec<String>,
    added: Vec<String>,
    labels_added: Vec<(String, Vec<String>)>,
    new_categories: bool,
    stored: u64,
}

impl AccountSync {
    pub(crate) fn new(client: Arc<GraphClient>, shared: Arc<Shared>) -> AccountSync {
        AccountSync {
            client,
            shared,
            cursor: SyncCursor::default(),
            graph: GraphCursor::default(),
            next_poll: Instant::now(),
            last_refresh: None,
            last_failed_retry: None,
            failures: 0,
            resync: HashMap::new(),
            folder_labels: HashMap::new(),
            categories: BTreeSet::new(),
            rate: Rate::new(),
        }
    }

    fn acct(&self) -> String {
        self.client.account_id().to_string()
    }

    async fn save_cursor(&mut self) -> Result<()> {
        self.cursor.provider_state = self.graph.to_state();
        let (id, cursor) = (self.acct(), self.cursor.clone());
        self.client
            .db(move |s| s.set_sync_cursor(&id, &cursor))
            .await
    }

    async fn count(&self) -> Result<u64> {
        let id = self.acct();
        self.client.db(move |s| s.count_messages(Some(&id))).await
    }

    /// Where full bodies start: the window, or everything when older mail
    /// is downloaded in full.
    fn full_start(&self) -> i64 {
        let p = self.shared.policy();
        if p.older == OlderMail::Full {
            0
        } else {
            window_start_ms(now_ms(), p.months)
        }
    }

    fn fill_pending(&self) -> bool {
        self.cursor.backfill_done
            && self
                .cursor
                .window
                .full_since_ms
                .is_some_and(|since| self.full_start() < since)
    }

    fn headers_pending(&self) -> bool {
        self.cursor.backfill_done
            && self.shared.policy().older == OlderMail::Headers
            && !matches!(
                self.cursor.window.older_mode.as_deref(),
                Some("headers" | "full")
            )
            && self.cursor.window.full_since_ms.is_some_and(|s| s > 0)
    }

    fn resting_phase(&self) -> SyncPhase {
        if self.cursor.backfill_done && !self.fill_pending() && !self.headers_pending() {
            SyncPhase::Idle
        } else {
            SyncPhase::Backfilling
        }
    }

    fn stage(&self) -> Option<SyncStage> {
        if !self.cursor.backfill_done {
            Some(SyncStage::Window)
        } else if self.fill_pending() || self.headers_pending() {
            Some(SyncStage::Older)
        } else {
            None
        }
    }

    fn health_note(&self) -> Option<String> {
        match self.cursor.failed_message_ids.len() {
            0 => None,
            1 => Some("1 message couldn't be downloaded yet; retrying".into()),
            n => Some(format!("{n} messages couldn't be downloaded yet; retrying")),
        }
    }

    fn notify_changed(&self, threads: impl IntoIterator<Item = String>) {
        let threads: Vec<String> = threads
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !threads.is_empty() {
            self.shared
                .observer
                .mail_changed(self.client.account_id(), threads);
        }
    }

    async fn publish_status(&mut self, stored: u64) -> Result<()> {
        let indexed = self.count().await?;
        let rate = if stored > 0 {
            self.rate.record(stored)
        } else {
            None
        };
        let (phase, stage, note) = (self.resting_phase(), self.stage(), self.health_note());
        self.shared.update(|s| {
            s.indexed = indexed;
            s.phase = phase;
            s.stage = stage;
            s.error = note;
            if phase == SyncPhase::Backfilling {
                if rate.is_some() {
                    s.rate_per_min = rate;
                    s.eta_secs = match (s.total_estimate, rate) {
                        (Some(total), Some(r)) if r > 0.0 => {
                            Some((total.saturating_sub(indexed) as f64 / r * 60.0).round() as u64)
                        }
                        _ => None,
                    };
                }
            } else {
                s.rate_per_min = None;
                s.eta_secs = None;
            }
        });
        Ok(())
    }

    pub(crate) async fn init(&mut self) -> Result<()> {
        let id = self.acct();
        self.cursor = self.client.db(move |s| s.get_sync_cursor(&id)).await?;
        self.graph = GraphCursor::parse(&self.cursor.provider_state)?;
        self.refresh_folders().await?;
        self.next_poll = Instant::now();
        self.publish_status(0).await
    }

    /// Re-read folders and categories: drop cursors of folders that are
    /// gone, remove local mail from them, relabel folders whose meaning
    /// changed (a folder deleted into Deleted Items becomes trash), save
    /// the label list.
    pub(crate) async fn refresh_folders(&mut self) -> Result<()> {
        let map = self.client.refresh_folders().await?;
        let acct = self.acct();
        if !map.truncated {
            let a = acct.clone();
            let held = self.client.db(move |s| locations::folders(s, &a)).await?;
            for fid in held.into_iter().filter(|f| !map.is_synced(f)) {
                let (a, f) = (acct.clone(), fid.clone());
                let ids = self
                    .client
                    .db(move |s| locations::in_folder(s, &a, &f))
                    .await?;
                tracing::info!(account = %acct, count = ids.len(), "folder no longer synced; removing its local mail");
                let threads = self.client.delete_local(ids).await?;
                self.notify_changed(threads);
            }
            self.graph.folders.retain(|id, _| map.is_synced(id));
        }
        let mut relabel = Vec::new();
        for f in map.synced() {
            let now = f.label();
            if let Some(before) = self.folder_labels.get(&f.id) {
                if *before != now {
                    relabel.push((f.id.clone(), before.clone(), now.clone()));
                }
            }
        }
        for (fid, before, now) in relabel {
            let (a, f) = (acct.clone(), fid.clone());
            let ids = self
                .client
                .db(move |s| locations::in_folder(s, &a, &f))
                .await?;
            let a = acct.clone();
            let threads = self
                .client
                .db(move |s| {
                    let mut threads = Vec::new();
                    for id in &ids {
                        if let Some(m) = s.get_message(&a, id)? {
                            threads.push(m.thread_id);
                        }
                    }
                    s.modify_message_labels(
                        &a,
                        &ids,
                        &now.into_iter().collect::<Vec<_>>(),
                        &before.into_iter().collect::<Vec<_>>(),
                    )?;
                    Ok(threads)
                })
                .await?;
            self.notify_changed(threads);
        }
        self.folder_labels = map.synced().map(|f| (f.id.clone(), f.label())).collect();
        self.save_labels(&map).await?;
        let total: u64 = map.synced().map(|f| f.total).sum();
        self.shared.update(|s| s.total_estimate = Some(total));
        self.last_refresh = Some(Instant::now());
        Ok(())
    }

    async fn save_labels(&mut self, map: &FolderMap) -> Result<()> {
        let acct = self.acct();
        let a = acct.clone();
        let stored: Vec<String> = self
            .client
            .db(move |s| {
                Ok(s.list_labels(Some(&a))?
                    .into_iter()
                    .filter_map(|l| crate::folders::category_of_label(&l.id))
                    .collect())
            })
            .await?;
        self.categories.extend(stored);
        let seen: Vec<String> = self.categories.iter().cloned().collect();
        let labels = map.labels(&acct, &seen);
        self.client
            .db(move |s| s.replace_labels(&acct, &labels))
            .await
    }

    fn poll_due(&self) -> bool {
        self.shared.poked.swap(false, Ordering::SeqCst) || Instant::now() >= self.next_poll
    }

    /// Fetch `ids` in full and store them; parks message-specific failures.
    /// Returns the ids stored.
    async fn fetch_and_store(&mut self, ids: &[String]) -> Result<(Vec<String>, Vec<String>)> {
        let mut stored = Vec::new();
        let mut threads = Vec::new();
        for chunk in ids.chunks(FETCH_CHUNK) {
            let mut ok: Vec<Fetched> = Vec::new();
            let mut resolved = HashSet::new();
            let mut failed = Vec::new();
            let mut fatal = None;
            let mut last = None;
            for (id, r) in self.client.fetch_full_each(chunk).await {
                match r {
                    Ok(Some(f)) => {
                        resolved.insert(id);
                        ok.push(f);
                    }
                    Ok(None) => {
                        resolved.insert(id);
                    }
                    Err(e) if is_message_specific(&e) => {
                        failed.push(id);
                        last = Some(e);
                    }
                    Err(e) => {
                        failed.push(id);
                        fatal.get_or_insert(e);
                    }
                }
            }
            for f in &ok {
                self.note_categories(&f.message);
            }
            stored.extend(ok.iter().map(|f| f.message.id.clone()));
            let t = self.client.store_full(ok).await?;
            threads.extend(t.iter().cloned());
            self.notify_changed(t);
            self.cursor
                .failed_message_ids
                .retain(|id| !resolved.contains(id));
            if let Some(e) = fatal {
                return Err(e);
            }
            if !failed.is_empty() {
                if resolved.is_empty() && failed.len() > 1 {
                    // Nothing worked: more likely Graph than the messages.
                    return Err(last.unwrap_or_else(|| Error::Other("message fetch failed".into())));
                }
                tracing::warn!(account = %self.client.account_id(), count = failed.len(), error = %last.as_ref().map(ToString::to_string).unwrap_or_default(), "parking messages that failed to download; will retry");
                let parked: HashSet<&String> = failed.iter().collect();
                self.cursor
                    .failed_message_ids
                    .retain(|id| !parked.contains(id));
                self.cursor
                    .failed_message_ids
                    .extend(failed.iter().cloned());
            }
        }
        Ok((stored, threads))
    }

    /// Like [`Self::fetch_and_store`] for a newest-first page of `folder`:
    /// the bodies `ids` need are listed by their date range in full (one
    /// request, [`GraphClient::list_full_range`]) instead of one GET each.
    /// Anything the listing didn't bring (moved since, a busy second) goes
    /// the old way; so does a page where the range would mostly re-list
    /// mail already stored, or a listing Graph refuses.
    async fn fetch_range_and_store(
        &mut self,
        folder: &Folder,
        page: &[WireMessage],
        ids: &[String],
    ) -> Result<(Vec<String>, Vec<String>)> {
        let want: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let dated: Vec<(bool, i64)> = page
            .iter()
            .filter(|w| w.parent_folder_id.as_deref().is_none_or(|p| p == folder.id))
            .filter_map(|w| {
                let ms = w.received_date_time.as_deref().and_then(parse_date_ms)?;
                Some((want.contains(w.id.as_str()), ms))
            })
            .collect();
        let wanted: Vec<i64> = dated.iter().filter(|d| d.0).map(|d| d.1).collect();
        let (Some(&from), Some(&to)) = (wanted.iter().min(), wanted.iter().max()) else {
            return self.fetch_and_store(ids).await;
        };
        let in_range = dated.iter().filter(|d| d.1 >= from && d.1 <= to).count();
        if wanted.len() < RANGE_MIN || (wanted.len() as f64) < in_range as f64 * RANGE_DENSITY {
            return self.fetch_and_store(ids).await;
        }
        let listed = match self
            .client
            .list_full_range(&folder.id, from, to, in_range + 10, RANGE_MAX_PAGES)
            .await
        {
            Ok(l) => l,
            Err(e) if is_message_specific(&e) => {
                tracing::debug!(account = %self.client.account_id(), error = %e, "range listing refused; fetching one by one");
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let mut ok: Vec<Fetched> = Vec::new();
        let mut got: HashSet<String> = HashSet::new();
        for w in listed.iter().filter(|w| want.contains(w.id.as_str())) {
            if got.contains(&w.id) {
                continue;
            }
            match self.client.convert(w, true).await {
                Ok(f) => {
                    got.insert(w.id.clone());
                    ok.push(f);
                }
                Err(e) => {
                    tracing::debug!(account = %self.client.account_id(), error = %e, "listed message didn't convert; fetching it alone")
                }
            }
        }
        for f in &ok {
            self.note_categories(&f.message);
        }
        let mut stored: Vec<String> = ok.iter().map(|f| f.message.id.clone()).collect();
        let mut threads = self.client.store_full(ok).await?;
        self.notify_changed(threads.iter().cloned());
        self.cursor
            .failed_message_ids
            .retain(|id| !got.contains(id));
        let rest: Vec<String> = ids
            .iter()
            .filter(|id| !got.contains(*id))
            .cloned()
            .collect();
        let (more, t) = self.fetch_and_store(&rest).await?;
        stored.extend(more);
        threads.extend(t);
        Ok((stored, threads))
    }

    fn note_categories(&mut self, m: &Message) -> bool {
        let mut new = false;
        for l in &m.label_ids {
            if let Some(c) = crate::folders::category_of_label(l) {
                new |= self.categories.insert(c);
            }
        }
        new
    }

    /// Look up messages a folder no longer lists: (still on the server,
    /// in another folder → their current state; gone → ids). A message now
    /// in a folder Penguin doesn't sync counts as gone.
    async fn resolve_removed(&self, ids: Vec<String>) -> Result<(Vec<WireMessage>, Vec<String>)> {
        let mut moved = Vec::new();
        let mut gone = Vec::new();
        let map = self.client.folders().await?;
        for id in ids {
            match self
                .client
                .api
                .get_json::<WireMessage>(
                    &format!("/me/messages/{}", enc(&id)),
                    &[("$select", SELECT_LIGHT.into())],
                    &[],
                    "message",
                )
                .await
            {
                Ok(w)
                    if w.parent_folder_id
                        .as_deref()
                        .is_some_and(|f| map.get(f).is_none() || map.is_synced(f)) =>
                {
                    moved.push(w)
                }
                Ok(_) => gone.push(id),
                Err(e) if crate::http::is_gone(&e) => gone.push(id),
                Err(e) => return Err(e),
            }
        }
        Ok((moved, gone))
    }

    /// Apply one delta page of `folder`.
    /// `ordered`: the page is a newest-first run of `folder` (a first
    /// delta round with `$orderby`), so what it needs can be listed by date.
    async fn apply_page(
        &mut self,
        folder: &Folder,
        items: Vec<WireMessage>,
        incremental: bool,
        ordered: bool,
    ) -> Result<PageResult> {
        let acct = self.acct();
        let mut out = PageResult::default();
        let (removed, present): (Vec<WireMessage>, Vec<WireMessage>) = items
            .into_iter()
            .filter(|w| !w.id.is_empty())
            .partition(|w| w.removed.is_some());

        // Removals: only messages still recorded here matter (otherwise the
        // other folder already reported the move). Those are looked up by
        // their immutable id: still there = moved (filed where it is now),
        // gone = deleted.
        let mut present = present;
        if !removed.is_empty() {
            let ids: Vec<String> = removed.into_iter().map(|w| w.id).collect();
            let (a, fid) = (acct.clone(), folder.id.clone());
            let here: Vec<String> = self
                .client
                .db(move |s| {
                    let locs = locations::get_many(s, &a, &ids)?;
                    let known = s.known_message_ids(&a, &ids)?;
                    Ok(ids
                        .into_iter()
                        .filter(|id| match locs.get(id) {
                            Some(f) => *f == fid,
                            None => known.contains(id),
                        })
                        .collect())
                })
                .await?;
            let (moved, gone) = self.resolve_removed(here).await?;
            let gone_set: HashSet<&String> = gone.iter().collect();
            self.cursor
                .failed_message_ids
                .retain(|id| !gone_set.contains(id));
            out.threads.extend(self.client.delete_local(gone).await?);
            present.extend(moved);
        }
        if present.is_empty() {
            return Ok(out);
        }
        if let Some(seen) = self.resync.get_mut(&folder.id) {
            seen.extend(present.iter().map(|w| w.id.clone()));
        }

        let ids: Vec<String> = present.iter().map(|w| w.id.clone()).collect();
        let a = acct.clone();
        let probe = ids.clone();
        let (locals, needing_body): (HashMap<String, Message>, HashSet<String>) = self
            .client
            .db(move |s| {
                let known = s.known_message_ids(&a, &probe)?;
                let mut locals = HashMap::new();
                for id in probe.iter().filter(|id| known.contains(*id)) {
                    if let Some(m) = s.get_message(&a, id)? {
                        locals.insert(id.clone(), m);
                    }
                }
                let needing: HashSet<String> =
                    s.ids_needing_body(&a, &probe)?.into_iter().collect();
                Ok((locals, needing))
            })
            .await?;
        let full_start = self.full_start();
        let older = self.shared.policy().older;
        let parked: HashSet<String> = self.cursor.failed_message_ids.iter().cloned().collect();
        let mut to_fetch: Vec<String> = Vec::new();
        let mut headers: Vec<Fetched> = Vec::new();
        let mut relabel: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
        let mut located: Vec<(String, String)> = Vec::new();
        for w in &present {
            let (folder_label, fid) = match w.parent_folder_id.as_deref() {
                Some(p) if p != folder.id => {
                    let (label, fid, synced) = self.client.place(w).await?;
                    if !synced {
                        continue;
                    }
                    (label, fid.unwrap_or_else(|| folder.id.clone()))
                }
                _ => (folder.label(), folder.id.clone()),
            };
            for c in w
                .categories
                .iter()
                .flatten()
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
            {
                out.new_categories |= self.categories.insert(c.to_string());
            }
            let date = w
                .received_date_time
                .as_deref()
                .and_then(parse_date_ms)
                .unwrap_or(0);
            match locals.get(&w.id) {
                Some(local) => {
                    let desired = labels_for(w, folder_label);
                    let (add, remove) = label_diff(&local.label_ids, w, &desired);
                    if !add.is_empty() || !remove.is_empty() {
                        if incremental && !add.is_empty() {
                            out.labels_added.push((w.id.clone(), add.clone()));
                        }
                        out.threads.push(local.thread_id.clone());
                        relabel.push((w.id.clone(), add, remove));
                    }
                    located.push((w.id.clone(), fid));
                    if needing_body.contains(&w.id) && date >= full_start && !parked.contains(&w.id)
                    {
                        to_fetch.push(w.id.clone());
                    }
                }
                None if parked.contains(&w.id) => {}
                None if date >= full_start || older == OlderMail::Full => {
                    to_fetch.push(w.id.clone())
                }
                None if older == OlderMail::Headers => {
                    match crate::convert::to_message(&acct, w, folder_label, false) {
                        Ok(message) => headers.push(Fetched {
                            message,
                            folder_id: Some(fid),
                        }),
                        Err(e) => {
                            tracing::warn!(account = %acct, error = %e, "skipped a message Graph listed")
                        }
                    }
                }
                None => {}
            }
        }
        let a = acct.clone();
        self.client
            .db(move |s| {
                for (id, add, remove) in &relabel {
                    s.modify_message_labels(&a, std::slice::from_ref(id), add, remove)?;
                }
                locations::set(s, &a, &located)
            })
            .await?;
        let header_ids: Vec<String> = headers.iter().map(|f| f.message.id.clone()).collect();
        let (inserted, t) = self.client.store_headers(headers).await?;
        out.stored += inserted as u64;
        out.threads.extend(t);
        let (stored, t) = if ordered {
            self.fetch_range_and_store(folder, &present, &to_fetch)
                .await?
        } else {
            self.fetch_and_store(&to_fetch).await?
        };
        out.stored += stored.len() as u64;
        out.threads.extend(t);
        if incremental {
            out.added
                .extend(stored.into_iter().filter(|id| !locals.contains_key(id)));
            out.added.extend(header_ids);
        }
        Ok(out)
    }

    /// The URL and query of a folder's next delta request.
    fn delta_request(
        folder: &Folder,
        fc: &FolderCursor,
    ) -> (String, Vec<(&'static str, String)>, bool) {
        match (&fc.next, &fc.delta_link) {
            (Some(next), d) => (next.clone(), vec![], d.is_some()),
            (None, Some(d)) => (d.clone(), vec![], true),
            (None, None) => {
                let mut q = vec![("$select", SELECT_LIGHT.to_string())];
                if !fc.plain {
                    q.push(("$filter", format!("receivedDateTime ge {ANCIENT}")));
                    q.push(("$orderby", "receivedDateTime desc".to_string()));
                }
                (
                    format!("/me/mailFolders/{}/messages/delta", enc(&folder.id)),
                    q,
                    false,
                )
            }
        }
    }

    /// One delta page of `folder`; returns true when its round finished.
    async fn delta_step(&mut self, folder: &Folder) -> Result<bool> {
        let fc = self
            .graph
            .folders
            .get(&folder.id)
            .cloned()
            .unwrap_or_default();
        let (url, query, incremental) = Self::delta_request(folder, &fc);
        let fresh = fc.next.is_none() && fc.delta_link.is_none();
        let page_size = format!("odata.maxpagesize={PAGE_SIZE}");
        let page: Page<WireMessage> = match self
            .client
            .api
            .get_json(&url, &query, &[page_size.as_str()], "delta page")
            .await
        {
            Ok(p) => p,
            Err(e) if is_stale_delta(&e) => {
                tracing::warn!(account = %self.client.account_id(), "delta link expired; re-listing the folder");
                self.graph.folders.insert(
                    folder.id.clone(),
                    FolderCursor {
                        plain: fc.plain,
                        ..FolderCursor::default()
                    },
                );
                self.resync.insert(folder.id.clone(), HashSet::new());
                self.save_cursor().await?;
                return Ok(false);
            }
            Err(Error::Http { status: 400, .. }) if fresh && !fc.plain => {
                tracing::info!(account = %self.client.account_id(), "ordered delta refused; using plain delta for this folder");
                self.graph.folders.insert(
                    folder.id.clone(),
                    FolderCursor {
                        plain: true,
                        ..FolderCursor::default()
                    },
                );
                self.save_cursor().await?;
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        let ordered = !incremental && !fc.plain;
        let result = self
            .apply_page(folder, page.value, incremental, ordered)
            .await?;
        self.notify_changed(result.threads.iter().cloned());
        if !result.added.is_empty() {
            self.shared
                .observer
                .messages_added(self.client.account_id(), result.added.clone());
        }
        if !result.labels_added.is_empty() {
            self.shared
                .observer
                .labels_added(self.client.account_id(), result.labels_added.clone());
        }
        if result.new_categories {
            let map = self.client.folders().await?;
            self.save_labels(&map).await?;
        }
        let entry = self.graph.folders.entry(folder.id.clone()).or_default();
        let finished = match (page.next_link, page.delta_link) {
            (Some(next), _) => {
                entry.next = Some(next);
                false
            }
            (None, Some(delta)) => {
                entry.next = None;
                entry.delta_link = Some(delta);
                true
            }
            (None, None) => {
                return Err(Error::Other(
                    "Graph returned a delta page without a next or delta link".into(),
                ))
            }
        };
        if finished {
            if let Some(seen) = self.resync.remove(&folder.id) {
                // Re-listed from scratch: what the server no longer lists here is gone.
                let (a, f) = (self.acct(), folder.id.clone());
                let here = self
                    .client
                    .db(move |s| locations::in_folder(s, &a, &f))
                    .await?;
                let stale: Vec<String> = here.into_iter().filter(|id| !seen.contains(id)).collect();
                if !stale.is_empty() {
                    // Present in the re-listing's folder no more: moved or deleted.
                    let stale: Vec<WireMessage> = stale
                        .into_iter()
                        .map(|id| WireMessage {
                            id,
                            removed: Some(serde_json::Value::Null),
                            ..WireMessage::default()
                        })
                        .collect();
                    let r = self.apply_page(folder, stale, false, false).await?;
                    self.notify_changed(r.threads);
                }
            }
        }
        self.save_cursor().await?;
        if !incremental {
            self.publish_status(result.stored).await?;
        }
        Ok(finished)
    }

    /// The first synced folder still in its first round.
    async fn next_initial_folder(&self) -> Result<Option<Folder>> {
        let map = self.client.folders().await?;
        let found = map
            .synced()
            .find(|f| {
                self.graph
                    .folders
                    .get(&f.id)
                    .is_none_or(|c| c.delta_link.is_none())
            })
            .cloned();
        Ok(found)
    }

    /// Incremental: every folder's delta round, parked retries, label refresh.
    async fn poll(&mut self) -> Result<()> {
        if self.cursor.backfill_done {
            self.shared.update(|s| s.phase = SyncPhase::Incremental);
        }
        let map = self.client.folders().await?;
        let folders: Vec<Folder> = map
            .synced()
            .filter(|f| {
                self.graph
                    .folders
                    .get(&f.id)
                    .is_some_and(|c| c.delta_link.is_some())
            })
            .cloned()
            .collect();
        for folder in folders {
            for _ in 0..MAX_POLL_PAGES {
                if self.delta_step(&folder).await? {
                    break;
                }
                if self
                    .graph
                    .folders
                    .get(&folder.id)
                    .is_none_or(|c| c.delta_link.is_none())
                {
                    // Reset (stale link): the backfill pass re-lists it.
                    break;
                }
            }
        }
        self.retry_failed().await?;
        if self
            .last_refresh
            .is_none_or(|t| t.elapsed() >= LABEL_REFRESH)
        {
            self.refresh_folders().await?;
        }
        self.save_cursor().await?;
        self.publish_status(0).await?;
        self.shared.update(|s| s.last_synced_at = Some(now_ms()));
        self.next_poll = Instant::now() + POLL_INTERVAL;
        self.failures = 0;
        Ok(())
    }

    async fn retry_failed(&mut self) -> Result<()> {
        if self.cursor.failed_message_ids.is_empty()
            || self
                .last_failed_retry
                .is_some_and(|t| t.elapsed() < FAILED_RETRY_INTERVAL)
        {
            return Ok(());
        }
        self.last_failed_retry = Some(Instant::now());
        let batch: Vec<String> = self
            .cursor
            .failed_message_ids
            .iter()
            .take(FAILED_RETRY_BATCH)
            .cloned()
            .collect();
        let (stored, _) = self.fetch_and_store(&batch).await?;
        if !stored.is_empty() {
            tracing::info!(account = %self.client.account_id(), recovered = stored.len(), "parked messages downloaded");
        }
        Ok(())
    }

    fn track_fill_start(&mut self) {
        if !self.cursor.backfill_done {
            let start = self.full_start();
            let w = &mut self.cursor.window;
            w.fill_after_ms = Some(w.fill_after_ms.map_or(start, |s| s.max(start)));
        }
    }

    async fn complete_backfill(&mut self) -> Result<()> {
        let since = self
            .cursor
            .window
            .fill_after_ms
            .unwrap_or_else(|| self.full_start());
        let older = self.shared.policy().older;
        let w = &mut self.cursor.window;
        w.full_since_ms = Some(since);
        w.fill_after_ms = None;
        w.fill_before_ms = None;
        w.fill_page_token = None;
        w.older_before_ms = Some(since);
        w.older_page_token = None;
        w.older_done = true;
        w.older_mode = Some(if since == 0 { "full" } else { older.as_str() }.to_string());
        self.cursor.backfill_done = true;
        self.save_cursor().await?;
        self.client.db(|s| s.optimize()).await?;
        tracing::info!(account = %self.client.account_id(), "backfill complete");
        self.publish_status(0).await
    }

    /// One page of the body-fill pass over `[full_start, full_since)`.
    async fn fill_page(&mut self) -> Result<()> {
        let start = self.full_start();
        let since = self.cursor.window.full_since_ms.unwrap_or(0);
        {
            let w = &mut self.cursor.window;
            if w.fill_after_ms != Some(start) || w.fill_before_ms != Some(since) {
                w.fill_after_ms = Some(start);
                w.fill_before_ms = Some(since);
                w.fill_page_token = None;
            }
        }
        let token = self.cursor.window.fill_page_token.clone();
        let page = self
            .client
            .list_messages(
                token.as_deref(),
                &[
                    (
                        "$filter",
                        format!(
                            "receivedDateTime ge {} and receivedDateTime lt {}",
                            format_date(start),
                            format_date(since)
                        ),
                    ),
                    ("$orderby", "receivedDateTime desc".into()),
                    ("$top", PAGE_SIZE.to_string()),
                ],
                &[],
            )
            .await?;
        let map = self.client.folders().await?;
        let parked: HashSet<String> = self.cursor.failed_message_ids.iter().cloned().collect();
        let ids: Vec<String> = page
            .value
            .iter()
            .filter(|w| w.removed.is_none() && !parked.contains(&w.id))
            .filter(|w| {
                w.parent_folder_id
                    .as_deref()
                    .is_some_and(|f| map.is_synced(f))
            })
            .map(|w| w.id.clone())
            .collect();
        let (a, probe) = (self.acct(), ids);
        let need = self
            .client
            .db(move |s| s.ids_needing_body(&a, &probe))
            .await?;
        let (stored, _) = self.fetch_and_store(&need).await?;
        match page.next_link {
            Some(next) => self.cursor.window.fill_page_token = Some(next),
            None => {
                let w = &mut self.cursor.window;
                w.full_since_ms = Some(start);
                w.older_before_ms = Some(start);
                w.fill_after_ms = None;
                w.fill_before_ms = None;
                w.fill_page_token = None;
                if start == 0 {
                    w.older_mode = Some("full".into());
                    w.older_done = true;
                }
            }
        }
        self.save_cursor().await?;
        self.publish_status(stored.len() as u64).await
    }

    /// One page of the headers pass over mail older than the window.
    async fn headers_page(&mut self) -> Result<()> {
        let since = self.cursor.window.full_since_ms.unwrap_or(0);
        if self.cursor.window.older_before_ms != Some(since) {
            self.cursor.window.older_before_ms = Some(since);
            self.cursor.window.older_page_token = None;
        }
        let token = self.cursor.window.older_page_token.clone();
        let page = self
            .client
            .list_messages(
                token.as_deref(),
                &[
                    (
                        "$filter",
                        format!("receivedDateTime lt {}", format_date(since)),
                    ),
                    ("$orderby", "receivedDateTime desc".into()),
                    ("$top", PAGE_SIZE.to_string()),
                ],
                &[],
            )
            .await?;
        let map = self.client.folders().await?;
        let listed: Vec<WireMessage> = page
            .value
            .into_iter()
            .filter(|w| w.removed.is_none() && !w.id.is_empty())
            .filter(|w| {
                w.parent_folder_id
                    .as_deref()
                    .is_some_and(|f| map.is_synced(f))
            })
            .collect();
        let (a, probe) = (
            self.acct(),
            listed.iter().map(|w| w.id.clone()).collect::<Vec<_>>(),
        );
        let known = self
            .client
            .db(move |s| s.known_message_ids(&a, &probe))
            .await?;
        let mut headers = Vec::new();
        for w in listed.iter().filter(|w| !known.contains(&w.id)) {
            match self.client.convert(w, false).await {
                Ok(f) => headers.push(f),
                Err(e) => {
                    tracing::warn!(account = %self.client.account_id(), error = %e, "skipped a message Graph listed")
                }
            }
        }
        let (inserted, threads) = self.client.store_headers(headers).await?;
        self.notify_changed(threads);
        match page.next_link {
            Some(next) => self.cursor.window.older_page_token = Some(next),
            None => {
                let w = &mut self.cursor.window;
                w.older_page_token = None;
                w.older_done = true;
                w.older_mode = Some("headers".into());
            }
        }
        self.save_cursor().await?;
        self.publish_status(inserted as u64).await
    }

    /// One unit of work; false when there's nothing to do until the next poll.
    pub(crate) async fn step(&mut self) -> Result<bool> {
        self.track_fill_start();
        if self.poll_due() {
            self.poll().await?;
            return Ok(true);
        }
        if let Some(folder) = self.next_initial_folder().await? {
            self.delta_step(&folder).await?;
            return Ok(true);
        }
        if !self.cursor.backfill_done {
            self.complete_backfill().await?;
            return Ok(true);
        }
        if self.fill_pending() {
            self.fill_page().await?;
            return Ok(true);
        }
        if self.headers_pending() {
            self.headers_page().await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn run_inner(&mut self) -> Result<()> {
        self.init().await?;
        loop {
            if !self.step().await? {
                tokio::select! {
                    _ = tokio::time::sleep_until(self.next_poll) => {}
                    _ = self.shared.wake.notified() => {}
                }
            }
        }
    }

    async fn run(mut self) {
        loop {
            match self.run_inner().await {
                Ok(()) => return,
                Err(Error::NeedsReauth(msg)) => {
                    tracing::warn!(account = %self.client.account_id(), "sync stopped: account needs to sign in again");
                    self.shared.update(|s| {
                        s.phase = SyncPhase::NeedsReauth;
                        s.error = Some(msg);
                    });
                    return;
                }
                Err(Error::Keychain(msg)) => {
                    tracing::warn!(account = %self.client.account_id(), error = %msg, "sync stopped: keychain access failed");
                    self.shared.update(|s| {
                        s.phase = SyncPhase::NeedsReauth;
                        s.error = Some(format!("Keychain access failed: {msg}"));
                    });
                    return;
                }
                Err(e) => {
                    self.failures += 1;
                    let wait =
                        Duration::from_secs(5u64 << self.failures.min(10)).min(MAX_ERROR_BACKOFF);
                    tracing::warn!(account = %self.client.account_id(), error = %e, ?wait, "sync error; retrying");
                    self.shared.update(|s| {
                        s.phase = SyncPhase::Error;
                        s.error = Some(e.to_string());
                    });
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.shared.wake.notified() => {}
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn cursor(&self) -> &SyncCursor {
        &self.cursor
    }

    #[cfg(test)]
    pub(crate) fn graph_cursor(&self) -> &GraphCursor {
        &self.graph
    }

    /// Tests: work until idle (polls included when poked).
    #[cfg(test)]
    pub(crate) async fn run_until_idle(&mut self) -> Result<()> {
        for _ in 0..10_000 {
            if !self.step().await? {
                return Ok(());
            }
        }
        panic!("sync never went idle");
    }

    #[cfg(test)]
    pub(crate) fn poke(&self) {
        self.shared.poke();
    }
}

// ---------- backend ----------

struct TaskHandle {
    shared: Arc<Shared>,
    abort: AbortHandle,
}

impl SyncTask for TaskHandle {
    fn poke(&self) {
        self.shared.poke();
    }
    fn stop(&self) {
        self.abort.abort();
    }
    fn is_running(&self) -> bool {
        !self.abort.is_finished()
    }
}

struct Entry {
    client: Arc<GraphClient>,
    shared: Arc<Shared>,
    task: Option<Arc<TaskHandle>>,
}

struct BackendInner {
    store: Store,
    observer: Arc<dyn SyncObserver>,
    auth: Arc<Auth>,
    transport: Arc<dyn Transport>,
    accounts: Mutex<HashMap<String, Entry>>,
    policy: Mutex<WindowPolicy>,
}

/// The Microsoft kind: per-account Graph clients and sync tasks.
#[derive(Clone)]
pub struct GraphBackend {
    inner: Arc<BackendInner>,
}

impl GraphBackend {
    /// The app's backend: the Keychain vault and the real network. Creates
    /// Penguin's Graph tables in the store if needed.
    pub fn new(store: Store, observer: Arc<dyn SyncObserver>) -> Result<GraphBackend> {
        let auth = crate::default_auth();
        let transport = auth.transport();
        GraphBackend::with_parts(store, observer, auth, transport)
    }

    pub fn with_parts(
        store: Store,
        observer: Arc<dyn SyncObserver>,
        auth: Arc<Auth>,
        transport: Arc<dyn Transport>,
    ) -> Result<GraphBackend> {
        locations::migrate(&store)?;
        Ok(GraphBackend {
            inner: Arc::new(BackendInner {
                store,
                observer,
                auth,
                transport,
                accounts: Mutex::new(HashMap::new()),
                policy: Mutex::new(WindowPolicy::default()),
            }),
        })
    }

    fn policy(&self) -> WindowPolicy {
        *self.inner.policy.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn entry(&self, account_id: &str) -> (Arc<GraphClient>, Arc<Shared>) {
        let mut accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let e = accounts
            .entry(account_id.to_string())
            .or_insert_with(|| Entry {
                client: Arc::new(GraphClient::new(
                    GraphApi::new(
                        account_id,
                        self.inner.transport.clone(),
                        self.inner.auth.clone(),
                    ),
                    self.inner.store.clone(),
                )),
                shared: Shared::new(account_id, self.inner.observer.clone(), self.policy()),
                task: None,
            });
        (e.client.clone(), e.shared.clone())
    }

    /// The account's sync state machine (tests drive it step by step).
    #[cfg(test)]
    pub(crate) fn account_sync(&self, account_id: &str) -> AccountSync {
        let (client, shared) = self.entry(account_id);
        AccountSync::new(client, shared)
    }
}

#[async_trait]
impl Backend for GraphBackend {
    fn provider(&self) -> AccountProvider {
        AccountProvider::Microsoft
    }

    fn client(&self, account: &Account) -> Result<Arc<dyn MailProvider>> {
        if account.provider != AccountProvider::Microsoft {
            return Err(Error::Other(format!(
                "{} is not a Microsoft account",
                account.id
            )));
        }
        let (client, _) = self.entry(&account.id);
        Ok(Arc::new(GraphProvider::new(client)))
    }

    fn has_credentials(&self, account: &Account) -> bool {
        self.inner.auth.has_credentials(&account.id)
    }

    async fn sign_out(&self, account: &Account) -> Result<()> {
        if let Some(e) = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&account.id)
        {
            if let Some(t) = e.task {
                t.stop();
            }
        }
        self.inner.auth.forget(&account.id).await
    }

    fn start_sync(&self, account: &Account) -> SyncHandle {
        let (client, shared) = self.entry(&account.id);
        let mut accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = accounts.get_mut(&account.id).expect("entry created above");
        if let Some(t) = entry.task.as_ref().filter(|t| t.is_running()) {
            return SyncHandle::new(t.clone());
        }
        let sync = AccountSync::new(client, shared.clone());
        let ticker_shared = shared.clone();
        let account_id = account.id.clone();
        let task = tokio::spawn(async move {
            // Progress heartbeat while backfilling (no timer otherwise).
            let ticker = ticker_shared.heartbeat.run(STATUS_TICK, || {
                let s = ticker_shared.snapshot();
                if s.phase == SyncPhase::Backfilling {
                    ticker_shared.observer.status(s);
                }
            });
            let run = AssertUnwindSafe(sync.run()).catch_unwind();
            tokio::select! {
                res = run => {
                    if let Err(payload) = res {
                        let panic = payload
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| payload.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".into());
                        tracing::error!(account = %account_id, panic = %panic, "Microsoft sync task panicked");
                        ticker_shared.update(|s| {
                            s.phase = SyncPhase::Error;
                            s.error = Some(format!("internal sync error ({panic}); restart sync to retry"));
                        });
                    }
                }
                _ = ticker => {}
            }
        });
        let handle = Arc::new(TaskHandle {
            shared,
            abort: task.abort_handle(),
        });
        entry.task = Some(handle.clone());
        SyncHandle::new(handle)
    }

    fn retry_sync(&self, account: &Account) -> SyncHandle {
        let (_, shared) = self.entry(&account.id);
        shared.update(|s| {
            s.phase = SyncPhase::Idle;
            s.error = None;
        });
        let handle = self.start_sync(account);
        handle.poke();
        handle
    }

    fn sync_status(&self, account_id: &str) -> Option<SyncStatus> {
        let accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_id)
            .filter(|e| e.task.is_some())
            .map(|e| e.shared.snapshot())
    }

    fn set_window_policy(&self, policy: WindowPolicy) {
        *self.inner.policy.lock().unwrap_or_else(|e| e.into_inner()) = policy;
        let accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for e in accounts.values() {
            *e.shared.policy.lock().unwrap_or_else(|p| p.into_inner()) = policy;
            e.shared.poke();
        }
    }
}
