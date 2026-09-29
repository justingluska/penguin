//! The per-account IMAP sync task (modeled on penguin-gmail's `sync.rs`).
//!
//! 1. Start: read the folder list, record every folder's UIDVALIDITY /
//!    UIDNEXT / HIGHESTMODSEQ (the incremental anchor) BEFORE any backfill,
//!    so mail arriving during a long backfill is picked up by incremental
//!    sync, not lost.
//! 2. Window fill, newest first per folder (inbox first): `UID SEARCH SINCE`
//!    the window start below the folder's fill mark, 100 messages per
//!    chunk; unknown messages in full, known ones just gain a location.
//!    Resumable (`fillBelow` per folder in the cursor). Round trips per
//!    chunk: one FETCH for ids, then one per body group (≤100 messages and
//!    ≤8 MiB, one store commit each); the folder is EXAMINEd once for its
//!    list and the selection reused after that.
//!    The Junk folder fills only back to the spam window
//!    (`spam_window_start_ms`: 30 days, or the window if shorter).
//! 3. Older mail per `OlderMail` (headers-only / full / none), same shape;
//!    never for Junk.
//! 4. Incremental, interleaved with 2–3: new UIDs at or above the anchor
//!    (`messages_added`), flag/label changes via CONDSTORE (CHANGEDSINCE)
//!    or a FLAGS scan, expunges via QRESYNC VANISHED or a UID diff when
//!    the folder's count doesn't add up, UIDVALIDITY resets by a rescan
//!    that re-maps locations (ids never change, nothing duplicates).
//!    Messages left with no copy anywhere are deleted only after every
//!    folder was looked at (a move between folders is not a deletion).
//! 5. Push: IDLE on INBOX (Gmail: All Mail) on a second connection pokes
//!    the loop; reconnects with backoff. Other folders are polled.
//! 6. NeedsReauth/Keychain stop the task; other errors back off 10 s → 5 min
//!    and then run a whole pass (poll included). Each failed attempt and the
//!    next progress are recorded on the status (penguin-core sync_health.rs),
//!    which decides when a failure is shown; connecting alone is not
//!    progress.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use penguin_core::{SyncCursor, SyncErrorKind, SyncPhase, SyncStage, SyncStatus};
use penguin_provider::heartbeat::Heartbeat;
use penguin_provider::window::{spam_window_start_ms, window_start_ms, OlderMail, WindowPolicy};
use penguin_provider::SyncObserver;
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::account::{blocking, Ctx, Depth, STALE_AFTER};
use crate::cursor::{FolderState, ImapCursor};
use crate::db;
use crate::folders::{Folder, FolderSet, Role};
use crate::proto::{seqset, Arg};
use crate::session::{search_date, IdleEvent, Session};
use crate::{Error, Result};

/// Messages per committed backfill chunk.
pub const FILL_CHUNK: usize = 100;
/// Headers-only messages per chunk in the older pass.
pub const OLDER_CHUNK: usize = 200;
/// New-mail UIDs per chunk.
pub const NEW_CHUNK: usize = 100;
/// The primary folder (INBOX / All Mail) is polled this often without IDLE.
pub const PRIMARY_POLL: Duration = Duration::from_secs(60);
/// …and this often with IDLE (a safety net).
pub const PRIMARY_POLL_IDLE: Duration = Duration::from_secs(5 * 60);
/// Every folder is polled this often.
pub const FULL_POLL: Duration = Duration::from_secs(5 * 60);
/// Without CONDSTORE, a folder's flags are rescanned at most this often
/// (the primary folder on every poll).
pub const FLAG_SCAN: Duration = Duration::from_secs(10 * 60);
/// IDLE is renewed after this (servers drop IDLE after ~30 min).
pub const IDLE_RENEW: Duration = Duration::from_secs(25 * 60);
const STATUS_TICK: Duration = Duration::from_secs(1);
/// The first error backoff (doubled per failure: 10 s, 20 s, 40 s …).
const ERROR_BACKOFF: Duration = Duration::from_secs(5);
const MAX_ERROR_BACKOFF: Duration = Duration::from_secs(5 * 60);
const RATE_WINDOW: Duration = Duration::from_secs(60);
const RATE_MIN_SPAN: Duration = Duration::from_secs(15);

/// STATUS answers: folder wire name → item → number.
type Statuses = HashMap<String, HashMap<String, u64>>;

/// What anchoring a folder needs, from STATUS or SELECT.
struct FolderStatus {
    exists: u64,
    uidvalidity: u32,
    uidnext: u32,
    highestmodseq: Option<u64>,
}

impl FolderStatus {
    fn from_status(st: &HashMap<String, u64>) -> Option<FolderStatus> {
        Some(FolderStatus {
            exists: *st.get("MESSAGES")?,
            uidvalidity: u32::try_from(*st.get("UIDVALIDITY")?).ok()?,
            uidnext: u32::try_from(*st.get("UIDNEXT")?).ok()?,
            // RFC 7162 §3.1.2.1: 0 = no persistent mod-sequences here.
            highestmodseq: st.get("HIGHESTMODSEQ").copied().filter(|m| *m > 0),
        })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// State shared between an account's task, its handle and the backend.
pub struct Shared {
    status: Mutex<SyncStatus>,
    poked: AtomicBool,
    wake: Notify,
    observer: Arc<dyn SyncObserver>,
    pub(crate) policy: Mutex<WindowPolicy>,
    /// IDLE is live on the primary folder (polls can relax).
    idling: AtomicBool,
    /// Progress heartbeat, ticking only while backfilling.
    heartbeat: Heartbeat,
}

impl Shared {
    pub fn new(
        account_id: &str,
        observer: Arc<dyn SyncObserver>,
        policy: WindowPolicy,
    ) -> Arc<Shared> {
        Arc::new(Shared {
            status: Mutex::new(SyncStatus::new(account_id, SyncPhase::Idle)),
            poked: AtomicBool::new(false),
            wake: Notify::new(),
            observer,
            policy: Mutex::new(policy),
            idling: AtomicBool::new(false),
            heartbeat: Heartbeat::default(),
        })
    }

    pub fn snapshot(&self) -> SyncStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut SyncStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        self.heartbeat.phase(snapshot.phase);
        self.observer.status(snapshot);
    }

    pub fn poke(&self) {
        self.poked.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub fn set_policy(&self, p: WindowPolicy) {
        *self.policy.lock().unwrap_or_else(|e| e.into_inner()) = p;
        self.poke();
    }

    fn policy(&self) -> WindowPolicy {
        *self.policy.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Messages/min over a trailing minute, smoothed.
struct Rate {
    since: Instant,
    samples: VecDeque<(Instant, u64)>,
    ema: Option<f64>,
}

impl Rate {
    fn new() -> Rate {
        Rate {
            since: Instant::now(),
            samples: VecDeque::new(),
            ema: None,
        }
    }
    fn record(&mut self, n: u64) -> Option<f64> {
        let now = Instant::now();
        self.samples.push_back((now, n));
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
        let raw = total as f64 / span.as_secs_f64() * 60.0;
        let ema = self.ema.map_or(raw, |e| e + 0.3 * (raw - e));
        self.ema = Some(ema);
        Some(ema)
    }
}

/// What one poll changed (reported together at the end of the pass).
#[derive(Default)]
struct Delta {
    threads: BTreeSet<String>,
    /// Copies disappeared: maybe deleted, maybe moved.
    removed: HashSet<String>,
    /// Flags/labels changed on stored messages.
    touched: HashSet<String>,
}

pub struct AccountSync {
    ctx: Arc<Ctx>,
    shared: Arc<Shared>,
    session: Option<Session>,
    /// When `session` was last handed out, and how long it may then sit
    /// before a NOOP check ([`STALE_AFTER`]; tests shorten it).
    session_used: Instant,
    pub(crate) stale_after: Duration,
    /// The first error backoff; it doubles per failure up to
    /// [`MAX_ERROR_BACKOFF`] (tests shorten it).
    pub(crate) backoff: Duration,
    cursor: SyncCursor,
    imap: ImapCursor,
    folders: Arc<FolderSet>,
    next_primary: Instant,
    next_full: Instant,
    failures: u32,
    fill_lists: HashMap<String, Vec<u32>>,
    older_lists: HashMap<String, Vec<u32>>,
    rate: Rate,
    total: Option<u64>,
}

impl AccountSync {
    pub fn new(ctx: Arc<Ctx>, shared: Arc<Shared>) -> AccountSync {
        AccountSync {
            ctx,
            shared,
            session: None,
            session_used: Instant::now(),
            stale_after: STALE_AFTER,
            backoff: ERROR_BACKOFF,
            cursor: SyncCursor::default(),
            imap: ImapCursor::default(),
            folders: Arc::new(FolderSet::default()),
            next_primary: Instant::now(),
            next_full: Instant::now(),
            failures: 0,
            fill_lists: HashMap::new(),
            older_lists: HashMap::new(),
            rate: Rate::new(),
            total: None,
        }
    }

    fn account(&self) -> String {
        self.ctx.cfg.account_id.clone()
    }

    async fn session(&mut self) -> Result<&mut Session> {
        let stale = self.session_used.elapsed() > self.stale_after;
        self.session_used = Instant::now();
        if let Some(s) = self.session.as_mut().filter(|s| s.is_open() && stale) {
            // Reaped while we waited between polls (Yahoo's BYE even
            // arrives uncompressed on a COMPRESS connection, so it reads
            // as garbage): a fresh connection, not a sync error.
            if let Err(e) = s.noop().await {
                tracing::debug!(account = %self.ctx.cfg.account_id, error = %e, "sync connection went stale; reopening");
                self.session = None;
            }
        }
        if self.session.as_ref().is_none_or(|s| !s.is_open()) {
            self.session = Some(self.ctx.open_session().await?);
        }
        Ok(self.session.as_mut().expect("opened"))
    }

    async fn save_cursor(&mut self) -> Result<()> {
        self.cursor.provider_state = self.imap.to_state();
        let (id, c) = (self.account(), self.cursor.clone());
        blocking(&self.ctx.store, move |s| s.set_sync_cursor(&id, &c)).await
    }

    fn notify(&self, threads: impl IntoIterator<Item = String>) {
        let t: Vec<String> = threads
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !t.is_empty() {
            self.shared
                .observer
                .mail_changed(&self.ctx.cfg.account_id, t);
        }
    }

    fn older_pending(&self) -> bool {
        let p = self.shared.policy();
        self.cursor.backfill_done
            && p.older != OlderMail::None
            && p.months != 0
            && self.folders.synced().iter().any(|f| {
                self.imap
                    .folders
                    .get(&f.raw)
                    .is_some_and(|st| !st.older_done)
            })
    }

    fn resting_phase(&self) -> SyncPhase {
        if self.cursor.backfill_done && !self.older_pending() {
            SyncPhase::Idle
        } else {
            SyncPhase::Backfilling
        }
    }

    fn stage(&self) -> Option<SyncStage> {
        if !self.cursor.backfill_done {
            Some(SyncStage::Window)
        } else if self.older_pending() {
            Some(SyncStage::Older)
        } else {
            None
        }
    }

    async fn count(&self) -> Result<u64> {
        let id = self.account();
        blocking(&self.ctx.store, move |s| s.count_messages(Some(&id))).await
    }

    /// Counts and phase after a step. `stored`: messages the step added.
    /// A step ends a failure streak when it is the work sync was doing:
    /// storing mail, or a poll once backfill is done. A poll that works
    /// while a failing backfill waits is not enough (the failure is the
    /// backfill's), and neither is init (logging in).
    async fn status_now(&mut self, stored: u64) -> Result<()> {
        self.status_after(stored, true).await
    }

    async fn status_after(&mut self, stored: u64, progress: bool) -> Result<()> {
        let indexed = self.count().await?;
        let phase = self.resting_phase();
        if !progress && self.shared.snapshot().failing() {
            self.shared.update(|s| s.indexed = indexed);
            return Ok(());
        }
        let stage = self.stage();
        let rate = if phase == SyncPhase::Backfilling && stored > 0 {
            self.rate.record(stored)
        } else {
            None
        };
        let total = self.total;
        // Mail was stored or checked: a failure streak is over.
        self.failures = 0;
        let now = now_ms();
        self.shared.update(|s| {
            s.record_progress(now);
            s.phase = phase;
            s.stage = stage;
            s.indexed = indexed;
            s.total_estimate = total.map(|t| t.max(indexed));
            s.error = None;
            s.last_synced_at = Some(now);
            if phase == SyncPhase::Backfilling {
                if rate.is_some() {
                    s.rate_per_min = rate;
                }
                s.eta_secs = match (s.rate_per_min, total) {
                    (Some(r), Some(t)) if r > 0.0 => {
                        Some((t.saturating_sub(indexed) as f64 / r * 60.0).round() as u64)
                    }
                    _ => None,
                };
            } else {
                s.rate_per_min = None;
                s.eta_secs = None;
            }
        });
        Ok(())
    }

    /// Load state, list folders, record anchors before any backfill.
    pub async fn init(&mut self) -> Result<()> {
        let id = self.account();
        self.cursor = blocking(&self.ctx.store, move |s| s.get_sync_cursor(&id)).await?;
        self.imap = ImapCursor::parse(&self.cursor.provider_state)?;
        let ctx = self.ctx.clone();
        let s = self.session().await?;
        let folders = ctx.folders(s, true).await?;
        self.folders = folders;
        let labels = self.folders.labels(&self.ctx.cfg.account_id);
        let id = self.account();
        blocking(&self.ctx.store, move |s| s.replace_labels(&id, &labels)).await?;
        self.reconcile_folders().await?;
        self.apply_window_policy();
        self.save_cursor().await?;
        // Logging in again is not progress: after a failure the status
        // keeps it until the pass that follows stores or checks mail.
        self.status_after(0, false).await
    }

    /// Anchor new folders, forget vanished ones.
    /// Forget folders that are gone, anchor new ones, and count the
    /// server's mail. Returns the folders' STATUS, which a poll reuses.
    async fn reconcile_folders(&mut self) -> Result<Statuses> {
        let synced: Vec<Folder> = self.folders.synced().into_iter().cloned().collect();
        let names: HashSet<&str> = synced.iter().map(|f| f.raw.as_str()).collect();
        let gone: Vec<String> = self
            .imap
            .folders
            .keys()
            .filter(|k| !names.contains(k.as_str()))
            .cloned()
            .collect();
        let mut removed = HashSet::new();
        for g in gone {
            let (account, folder) = (self.account(), g.clone());
            removed.extend(
                blocking(&self.ctx.store, move |s| {
                    db::clear_folder(s, &account, &folder)
                })
                .await?,
            );
            self.imap.folders.remove(&g);
        }
        let mut total = 0u64;
        let names: Vec<&str> = synced.iter().map(|f| f.raw.as_str()).collect();
        let statuses = self.statuses(&names).await?;
        for f in &synced {
            let sel = match statuses.get(&f.raw).and_then(FolderStatus::from_status) {
                Some(st) => st,
                None => {
                    let s = self.session().await?;
                    match s.select(&f.raw, true).await {
                        Ok(sel) => FolderStatus {
                            exists: sel.exists as u64,
                            uidvalidity: sel.uidvalidity,
                            uidnext: sel.uidnext,
                            highestmodseq: sel.highestmodseq,
                        },
                        Err(e) if e.is_not_found() => continue,
                        Err(e) => return Err(e),
                    }
                }
            };
            if !matches!(f.role, Role::Trash | Role::Junk) {
                total += sel.exists;
            }
            if !self.imap.folders.contains_key(&f.raw) {
                tracing::debug!(account = %self.ctx.cfg.account_id, exists = sel.exists, "anchoring folder");
                self.imap.folders.insert(
                    f.raw.clone(),
                    FolderState {
                        uidvalidity: sel.uidvalidity,
                        uidnext: sel.uidnext,
                        highestmodseq: sel.highestmodseq,
                        fill_below: Some(sel.uidnext),
                        older_below: None,
                        older_done: false,
                        last_flag_scan_ms: now_ms(),
                    },
                );
                if self.cursor.backfill_done {
                    // A folder that appeared later: fill it too.
                    self.cursor.backfill_done = false;
                }
            }
        }
        self.total = Some(total);
        if !removed.is_empty() {
            let removed: Vec<String> = removed.into_iter().collect();
            let threads = self.delete_orphans(&removed).await?;
            self.notify(threads);
        }
        Ok(statuses)
    }

    /// Follow Settings → Sync: start the first fill, or a band when the
    /// window grew, and reset the older pass when its mode changed.
    fn apply_window_policy(&mut self) {
        let policy = self.shared.policy();
        let w = window_start_ms(now_ms(), policy.months);
        let win = &mut self.cursor.window;
        match win.full_since_ms {
            None => {
                if win.fill_after_ms.is_none() {
                    win.fill_after_ms = Some(w);
                    win.fill_before_ms = None;
                } else if win.fill_after_ms != Some(w) {
                    let grew = w < win.fill_after_ms.unwrap_or(i64::MAX);
                    win.fill_after_ms = Some(w);
                    self.fill_lists.clear();
                    if grew {
                        for st in self.imap.folders.values_mut() {
                            st.fill_below = Some(st.uidnext);
                        }
                    }
                }
            }
            Some(s) if w < s && win.fill_after_ms.is_none() => {
                win.fill_after_ms = Some(w);
                win.fill_before_ms = Some(s);
                self.cursor.backfill_done = false;
                self.fill_lists.clear();
                for st in self.imap.folders.values_mut() {
                    st.fill_below = Some(st.uidnext);
                }
            }
            _ => {}
        }
        let mode = policy.older.as_str().to_string();
        if self.cursor.window.older_mode.as_deref() != Some(mode.as_str()) {
            self.cursor.window.older_mode = Some(mode);
            self.cursor.window.older_done = false;
            self.older_lists.clear();
            for st in self.imap.folders.values_mut() {
                st.older_done = false;
                st.older_below = None;
            }
        }
    }

    fn primary_interval(&self) -> Duration {
        if self.shared.idling.load(Ordering::SeqCst) {
            PRIMARY_POLL_IDLE
        } else {
            PRIMARY_POLL
        }
    }

    /// Which folders are due: all, the primary only, or none.
    fn due(&mut self) -> Option<bool> {
        let poked = self.shared.poked.swap(false, Ordering::SeqCst);
        let now = Instant::now();
        if now >= self.next_full {
            return Some(true);
        }
        if poked || now >= self.next_primary {
            return Some(false);
        }
        None
    }

    /// Incremental pass over the primary folder, or every folder.
    pub async fn poll(&mut self, all: bool) -> Result<()> {
        let now = Instant::now();
        let mut known = None;
        if all {
            let ctx = self.ctx.clone();
            let s = self.session().await?;
            let folders = ctx.folders(s, true).await?;
            if *folders != *self.folders {
                self.folders = folders;
                let labels = self.folders.labels(&self.ctx.cfg.account_id);
                let id = self.account();
                blocking(&self.ctx.store, move |s| s.replace_labels(&id, &labels)).await?;
            }
            known = Some(self.reconcile_folders().await?);
            self.next_full = now + FULL_POLL;
        }
        self.next_primary = now + self.primary_interval();
        let targets: Vec<Folder> = if all {
            self.folders.synced().into_iter().cloned().collect()
        } else {
            self.folders.primary().into_iter().cloned().collect()
        };
        let mut delta = Delta::default();
        let quiet = self.unchanged(&targets, known.as_ref()).await?;
        for f in targets.iter().filter(|f| !quiet.contains(&f.raw)) {
            self.poll_folder(f, &mut delta).await?;
        }
        if !all && !delta.removed.is_empty() {
            // Something left the inbox: look everywhere before calling it deleted.
            let rest: Vec<Folder> = self
                .folders
                .synced()
                .into_iter()
                .filter(|f| !targets.iter().any(|t| t.raw == f.raw))
                .cloned()
                .collect();
            let quiet = self.unchanged(&rest, None).await?;
            for f in rest.iter().filter(|f| !quiet.contains(&f.raw)) {
                self.poll_folder(f, &mut delta).await?;
            }
        }
        self.finish_poll(delta).await
    }

    async fn finish_poll(&mut self, delta: Delta) -> Result<()> {
        let mut threads = delta.threads;
        let removed: Vec<String> = delta.removed.iter().cloned().collect();
        threads.extend(self.delete_orphans(&removed).await?);
        let mut touched: Vec<String> = delta.touched.into_iter().chain(delta.removed).collect();
        touched.sort();
        touched.dedup();
        let (t, added) = self.ctx.refresh_labels(&self.folders, &touched).await?;
        threads.extend(t);
        self.notify(threads);
        if !added.is_empty() {
            self.shared
                .observer
                .labels_added(&self.ctx.cfg.account_id, added);
        }
        self.save_cursor().await?;
        let resting = self.resting_phase() != SyncPhase::Backfilling;
        self.status_after(0, resting).await
    }

    /// Delete stored messages that have no copy left; returns their threads.
    async fn delete_orphans(&self, candidates: &[String]) -> Result<Vec<String>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let (account, ids) = (self.account(), candidates.to_vec());
        blocking(&self.ctx.store, move |st| {
            let orphans = db::orphans(st, &account, &ids)?;
            if orphans.is_empty() {
                return Ok(Vec::new());
            }
            // Local drafts not yet on the server have no copy either; keep
            // drafts that still have a mapping.
            let mut doomed = Vec::new();
            let mut threads = Vec::new();
            for id in orphans {
                if let Some(m) = st.get_message(&account, &id)? {
                    threads.push(m.thread_id);
                    doomed.push(id);
                }
            }
            st.delete_messages(&account, &doomed)?;
            Ok(threads)
        })
        .await
    }

    /// STATUS of `names` in one round trip (see `Session::status_many`),
    /// with HIGHESTMODSEQ when the server has CONDSTORE.
    async fn statuses(&mut self, names: &[&str]) -> Result<Statuses> {
        let s = self.session().await?;
        let items = if s.caps.condstore() {
            "MESSAGES UIDNEXT UIDVALIDITY HIGHESTMODSEQ"
        } else {
            "MESSAGES UIDNEXT UIDVALIDITY"
        };
        s.status_many(names, items).await
    }

    /// Folders a poll can skip: STATUS (all of them in one round trip)
    /// shows the same UIDVALIDITY and UIDNEXT as last time, as many
    /// messages as are stored, and, with CONDSTORE, the same HIGHESTMODSEQ
    /// (RFC 7162 §3.1.2.1: unchanged, nothing's metadata changed). Without
    /// CONDSTORE flag changes can't be seen this way, so a folder is only
    /// skipped between its flag scans (the primary folder never). The
    /// selected folder is polled as before (STATUS isn't for it).
    async fn unchanged(
        &mut self,
        folders: &[Folder],
        known: Option<&Statuses>,
    ) -> Result<HashSet<String>> {
        let condstore = self.session().await?.caps.condstore();
        let primary = self.folders.primary().map(|p| p.raw.clone());
        let now = now_ms();
        let candidates: Vec<(Folder, FolderState)> = folders
            .iter()
            .filter_map(|f| {
                let st = self.imap.folders.get(&f.raw)?;
                let modseq = condstore && st.highestmodseq.is_some();
                let scan_due = primary.as_deref() == Some(f.raw.as_str())
                    || now - st.last_flag_scan_ms >= FLAG_SCAN.as_millis() as i64;
                (modseq || !scan_due).then(|| (f.clone(), st.clone()))
            })
            .collect();
        let mut quiet = HashSet::new();
        if candidates.is_empty() {
            return Ok(quiet);
        }
        let statuses = match known {
            Some(k) => k.clone(),
            None => {
                let names: Vec<&str> = candidates.iter().map(|(f, _)| f.raw.as_str()).collect();
                self.statuses(&names).await?
            }
        };
        let asked: Vec<(String, u32)> = candidates
            .iter()
            .filter(|(f, _)| statuses.contains_key(&f.raw))
            .map(|(f, st)| (f.raw.clone(), st.uidnext))
            .collect();
        let account = self.account();
        let stored: HashMap<String, u64> = blocking(&self.ctx.store, move |s| {
            asked
                .into_iter()
                .map(|(folder, uidnext)| {
                    let n = db::count_below(s, &account, &folder, uidnext)?;
                    Ok((folder, n))
                })
                .collect()
        })
        .await?;
        for (f, st) in &candidates {
            let (Some(got), Some(count)) = (statuses.get(&f.raw), stored.get(&f.raw)) else {
                continue;
            };
            let same = |k: &str, v: Option<u64>| v.is_some() && got.get(k).copied() == v;
            let modseq_ok = !(condstore && st.highestmodseq.is_some())
                || same("HIGHESTMODSEQ", st.highestmodseq);
            if same("UIDVALIDITY", Some(st.uidvalidity as u64))
                && same("UIDNEXT", Some(st.uidnext as u64))
                && same("MESSAGES", Some(*count))
                && modseq_ok
            {
                quiet.insert(f.raw.clone());
            }
        }
        Ok(quiet)
    }

    async fn poll_folder(&mut self, f: &Folder, delta: &mut Delta) -> Result<()> {
        let ctx = self.ctx.clone();
        let folders = self.folders.clone();
        let account = self.account();
        let sel = {
            let s = self.session().await?;
            match s.select(&f.raw, true).await {
                Ok(sel) => sel,
                Err(e) if e.is_not_found() => return Ok(()),
                Err(e) => return Err(e),
            }
        };
        let Some(mut st) = self.imap.folders.get(&f.raw).cloned() else {
            return Ok(());
        };
        let mut reset = false;
        if sel.uidvalidity != st.uidvalidity {
            tracing::warn!(account = %account, "UIDVALIDITY changed; re-mapping a folder");
            let (a, folder) = (account.clone(), f.raw.clone());
            delta
                .removed
                .extend(blocking(&ctx.store, move |s| db::clear_folder(s, &a, &folder)).await?);
            st = FolderState {
                uidvalidity: sel.uidvalidity,
                uidnext: 1,
                highestmodseq: None,
                fill_below: Some(sel.uidnext),
                older_below: None,
                older_done: false,
                last_flag_scan_ms: now_ms(),
            };
            self.fill_lists.remove(&f.raw);
            self.older_lists.remove(&f.raw);
            self.cursor.backfill_done = false;
            reset = true;
        }
        // New mail (or, after a reset, everything: re-mapped, not "new").
        if sel.uidnext > st.uidnext && sel.exists > 0 {
            let s = self.session.as_mut().expect("selected above");
            let mut uids: Vec<u32> = s
                .uid_search(vec![Arg::raw(format!("UID {}:*", st.uidnext))])
                .await?
                .into_iter()
                .filter(|u| *u >= st.uidnext)
                .collect();
            uids.sort_unstable();
            let caps = s.caps.clone();
            for chunk in uids.chunks(NEW_CHUNK) {
                let s = self.session.as_mut().expect("open");
                let (items, _) = s
                    .uid_fetch(&seqset::format(chunk), &Ctx::id_items(&caps), None)
                    .await?;
                let depth = if reset { Depth::KnownOnly } else { Depth::Full };
                let got = ctx
                    .ingest(s, &folders, f, sel.uidvalidity, items, depth)
                    .await?;
                delta.touched.extend(got.ids.iter().cloned());
                let mut threads = got.threads.clone();
                // Moves between folders re-use ids: only truly new mail counts.
                if !reset && !got.new_ids.is_empty() {
                    self.shared
                        .observer
                        .messages_added(&ctx.cfg.account_id, got.new_ids.clone());
                }
                st.uidnext = chunk.iter().max().map_or(st.uidnext, |m| m + 1);
                self.imap.folders.insert(f.raw.clone(), st.clone());
                self.notify(std::mem::take(&mut threads));
                self.save_cursor().await?;
            }
        }
        st.uidnext = st.uidnext.max(sel.uidnext);
        // Flags and expunges.
        let s = self.session.as_mut().expect("open");
        let caps = s.caps.clone();
        let known = {
            let (a, folder) = (account.clone(), f.raw.clone());
            blocking(&ctx.store, move |s| db::folder_locations(s, &a, &folder)).await?
        };
        let mut gone: Vec<u32> = Vec::new();
        let mut flag_changes = Vec::new();
        let condstore =
            caps.condstore() && st.highestmodseq.is_some() && sel.highestmodseq.is_some();
        let mut counted = false;
        let primary = folders.primary().is_some_and(|p| p.raw == f.raw);
        if condstore && !reset {
            if sel.highestmodseq != st.highestmodseq && !known.is_empty() {
                let range = format!("1:{}", st.uidnext.saturating_sub(1).max(1));
                let (items, vanished) = s
                    .uid_fetch(&range, &Ctx::flag_items(&caps), st.highestmodseq)
                    .await?;
                for i in &items {
                    collect_flag_change(i, &known, &f.raw, caps.gmail(), &mut flag_changes);
                }
                gone.extend(vanished.into_iter().filter(|u| known.contains_key(u)));
            }
            if s.qresync {
                counted = true;
            }
        } else if primary || now_ms() - st.last_flag_scan_ms >= FLAG_SCAN.as_millis() as i64 {
            if !known.is_empty() {
                let (items, _) = s
                    .uid_fetch(
                        &format!("1:{}", st.uidnext.saturating_sub(1).max(1)),
                        &Ctx::flag_items(&caps),
                        None,
                    )
                    .await?;
                let present: HashSet<u32> = items.iter().filter_map(|i| i.uid).collect();
                for i in &items {
                    collect_flag_change(i, &known, &f.raw, caps.gmail(), &mut flag_changes);
                }
                gone.extend(known.keys().filter(|u| !present.contains(u)).copied());
            }
            st.last_flag_scan_ms = now_ms();
            counted = true;
        }
        if !counted && !known.is_empty() {
            let (a, folder) = (account.clone(), f.raw.clone());
            let uidnext = st.uidnext;
            let stored = blocking(&ctx.store, move |s| {
                db::count_below(s, &a, &folder, uidnext)
            })
            .await?;
            if stored != sel.exists as u64 {
                let s = self.session.as_mut().expect("open");
                let all: HashSet<u32> = if sel.exists == 0 {
                    HashSet::new()
                } else {
                    s.uid_search(vec![Arg::raw("ALL")])
                        .await?
                        .into_iter()
                        .collect()
                };
                gone.extend(known.keys().filter(|u| !all.contains(u)).copied());
            }
        }
        if !flag_changes.is_empty() {
            delta
                .touched
                .extend(flag_changes.iter().filter_map(|(folder, uid, _, _)| {
                    (folder == &f.raw).then(|| known.get(uid).map(|l| l.message_id.clone()))?
                }));
            let (a, changes) = (account.clone(), flag_changes);
            blocking(&ctx.store, move |s| db::set_flags(s, &a, &changes)).await?;
        }
        if !gone.is_empty() {
            gone.sort_unstable();
            gone.dedup();
            let (a, folder) = (account.clone(), f.raw.clone());
            delta
                .removed
                .extend(blocking(&ctx.store, move |s| db::remove(s, &a, &folder, &gone)).await?);
        }
        st.highestmodseq = sel.highestmodseq;
        self.imap.folders.insert(f.raw.clone(), st);
        Ok(())
    }

    /// One chunk of the window fill. Returns false when nothing was left.
    pub async fn fill_step(&mut self) -> Result<bool> {
        // A fill reopened for a new folder or a UIDVALIDITY reset (no band
        // set) fills down to what's already complete.
        let after = self
            .cursor
            .window
            .fill_after_ms
            .or(self.cursor.window.full_since_ms)
            .unwrap_or(0);
        let before = self.cursor.window.fill_before_ms;
        let next = self
            .folders
            .synced()
            .into_iter()
            .find(|f| {
                self.imap
                    .folders
                    .get(&f.raw)
                    .is_some_and(|st| st.fill_below.is_some())
            })
            .cloned();
        let Some(f) = next else {
            // Every folder is filled: the window is complete.
            let full = self
                .cursor
                .window
                .full_since_ms
                .map_or(after, |s| s.min(after));
            self.cursor.window.full_since_ms = Some(full);
            self.cursor.window.fill_after_ms = None;
            self.cursor.window.fill_before_ms = None;
            self.cursor.window.fill_page_token = None;
            self.cursor.window.older_before_ms = Some(full);
            self.cursor.backfill_done = true;
            self.save_cursor().await?;
            self.status_now(0).await?;
            return Ok(false);
        };
        let mut st = self.imap.folders[&f.raw].clone();
        let below = st.fill_below.unwrap_or(0);
        let ctx = self.ctx.clone();
        let folders = self.folders.clone();
        // The folder's first chunk EXAMINEs it (fresh EXISTS for the list);
        // later chunks reuse the selection while it lasts: UIDVALIDITY can't
        // change under a selected mailbox (RFC 3501 §2.3.1.1), so a
        // re-EXAMINE per chunk was a round trip that told us nothing.
        let listed = self.fill_lists.contains_key(&f.raw);
        let sel = {
            let s = self.session().await?;
            let selected = if listed {
                s.ensure_selected(&f.raw, false).await
            } else {
                s.select(&f.raw, true).await
            };
            match selected {
                Ok(sel) => sel,
                Err(e) if e.is_not_found() => {
                    st.fill_below = None;
                    self.imap.folders.insert(f.raw.clone(), st);
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
        };
        if sel.uidvalidity != st.uidvalidity {
            // poll_folder handles the reset; don't fill with stale UIDs.
            let mut d = Delta::default();
            self.poll_folder(&f, &mut d).await?;
            self.finish_poll(d).await?;
            return Ok(true);
        }
        if !listed {
            // The Junk folder only as far back as the spam window.
            let after = if f.role == Role::Junk {
                after.max(spam_window_start_ms(now_ms(), self.shared.policy().months))
            } else {
                after
            };
            let list = if below <= 1 || sel.exists == 0 {
                Vec::new()
            } else {
                let mut criteria = vec![Arg::raw(format!("UID 1:{}", below - 1))];
                if after > 0 {
                    criteria.push(Arg::raw(format!("SINCE {}", search_date(after))));
                }
                if let Some(b) = before {
                    criteria.push(Arg::raw(format!("BEFORE {}", search_date(b))));
                }
                let s = self.session.as_mut().expect("open");
                let mut uids = s.uid_search(criteria).await?;
                uids.retain(|u| *u < below);
                uids.sort_unstable_by(|a, b| b.cmp(a));
                uids
            };
            self.fill_lists.insert(f.raw.clone(), list);
        }
        let list = self.fill_lists.get_mut(&f.raw).expect("inserted");
        let chunk: Vec<u32> = list.drain(..list.len().min(FILL_CHUNK)).collect();
        if chunk.is_empty() {
            st.fill_below = None;
            self.fill_lists.remove(&f.raw);
            self.imap.folders.insert(f.raw.clone(), st);
            self.save_cursor().await?;
            return Ok(true);
        }
        let s = self.session.as_mut().expect("open");
        let caps = s.caps.clone();
        let (items, _) = s
            .uid_fetch(&seqset::format(&chunk), &Ctx::fill_items(&caps), None)
            .await?;
        let got = ctx
            .ingest(s, &folders, &f, sel.uidvalidity, items, Depth::Full)
            .await?;
        st.fill_below = chunk.iter().min().copied();
        self.imap.folders.insert(f.raw.clone(), st);
        self.notify(got.threads);
        self.save_cursor().await?;
        self.status_now(got.new_ids.len() as u64).await?;
        Ok(true)
    }

    /// One chunk of the older-mail pass. Returns false when nothing was done.
    pub async fn older_step(&mut self) -> Result<bool> {
        let policy = self.shared.policy();
        let depth = match policy.older {
            OlderMail::None => return Ok(false),
            OlderMail::Headers => Depth::Headers,
            OlderMail::Full => Depth::Full,
        };
        let Some(since) = self.cursor.window.full_since_ms.filter(|s| *s > 0) else {
            return Ok(false);
        };
        let next = self
            .folders
            .synced()
            .into_iter()
            .find(|f| {
                self.imap
                    .folders
                    .get(&f.raw)
                    .is_some_and(|st| !st.older_done)
            })
            .cloned();
        let Some(f) = next else {
            if !self.cursor.window.older_done {
                self.cursor.window.older_done = true;
                self.cursor.window.older_before_ms = Some(since);
                self.save_cursor().await?;
                self.status_now(0).await?;
            }
            return Ok(false);
        };
        let mut st = self.imap.folders[&f.raw].clone();
        if f.role == Role::Junk {
            // Spam older than the window is never downloaded (the fill
            // took the spam window's worth).
            st.older_done = true;
            st.older_below = None;
            self.imap.folders.insert(f.raw.clone(), st);
            self.save_cursor().await?;
            return Ok(true);
        }
        let below = st.older_below.unwrap_or(st.uidnext);
        let ctx = self.ctx.clone();
        let folders = self.folders.clone();
        // As in fill_step: EXAMINE for the list, then reuse the selection.
        let listed = self.older_lists.contains_key(&f.raw);
        let sel = {
            let s = self.session().await?;
            let selected = if listed {
                s.ensure_selected(&f.raw, false).await
            } else {
                s.select(&f.raw, true).await
            };
            match selected {
                Ok(sel) => sel,
                Err(e) if e.is_not_found() => {
                    st.older_done = true;
                    self.imap.folders.insert(f.raw.clone(), st);
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
        };
        if sel.uidvalidity != st.uidvalidity {
            let mut d = Delta::default();
            self.poll_folder(&f, &mut d).await?;
            self.finish_poll(d).await?;
            return Ok(true);
        }
        if !listed {
            let list = if below <= 1 || sel.exists == 0 {
                Vec::new()
            } else {
                let s = self.session.as_mut().expect("open");
                let mut uids = s
                    .uid_search(vec![
                        Arg::raw(format!("UID 1:{}", below - 1)),
                        Arg::raw(format!("BEFORE {}", search_date(since))),
                    ])
                    .await?;
                uids.retain(|u| *u < below);
                uids.sort_unstable_by(|a, b| b.cmp(a));
                uids
            };
            self.older_lists.insert(f.raw.clone(), list);
        }
        let list = self.older_lists.get_mut(&f.raw).expect("inserted");
        let n = if depth == Depth::Full {
            FILL_CHUNK
        } else {
            OLDER_CHUNK
        };
        let chunk: Vec<u32> = list.drain(..list.len().min(n)).collect();
        if chunk.is_empty() {
            st.older_done = true;
            st.older_below = None;
            self.older_lists.remove(&f.raw);
            self.imap.folders.insert(f.raw.clone(), st);
            self.save_cursor().await?;
            return Ok(true);
        }
        let s = self.session.as_mut().expect("open");
        let caps = s.caps.clone();
        let items = if depth == Depth::Full {
            Ctx::fill_items(&caps)
        } else {
            Ctx::id_items(&caps)
        };
        let (items, _) = s.uid_fetch(&seqset::format(&chunk), &items, None).await?;
        let got = ctx
            .ingest(s, &folders, &f, sel.uidvalidity, items, depth)
            .await?;
        st.older_below = chunk.iter().min().copied();
        self.imap.folders.insert(f.raw.clone(), st);
        self.notify(got.threads);
        self.save_cursor().await?;
        self.status_now(got.new_ids.len() as u64).await?;
        Ok(true)
    }

    async fn run_inner(&mut self) -> Result<()> {
        self.init().await?;
        self.next_full = Instant::now() + FULL_POLL;
        self.next_primary = Instant::now() + self.primary_interval();
        loop {
            self.apply_window_policy();
            if let Some(all) = self.due() {
                self.poll(all).await?;
            }
            if !self.cursor.backfill_done {
                self.fill_step().await?;
                continue;
            }
            if self.older_pending() && self.older_step().await? {
                continue;
            }
            let wake = self.next_primary.min(self.next_full);
            tokio::select! {
                _ = tokio::time::sleep_until(wake) => {}
                _ = self.shared.wake.notified() => {}
            }
        }
    }

    pub async fn run(mut self) {
        loop {
            match self.run_inner().await {
                Ok(()) => return,
                Err(Error::NeedsReauth(msg)) => {
                    tracing::warn!(account = %self.ctx.cfg.account_id, "IMAP sync stopped: the server refused the password");
                    let now = now_ms();
                    self.shared
                        .update(|s| s.record_failure(SyncErrorKind::Auth, msg, now, None));
                    return;
                }
                Err(Error::Keychain(msg)) => {
                    tracing::warn!(account = %self.ctx.cfg.account_id, error = %msg, "IMAP sync stopped: keychain access failed");
                    let now = now_ms();
                    self.shared.update(|s| {
                        let msg = format!("Keychain access failed: {msg}");
                        s.record_failure(SyncErrorKind::Keychain, msg, now, None)
                    });
                    return;
                }
                Err(e) => {
                    self.failures += 1;
                    self.session = None;
                    let wait =
                        (self.backoff * (1u32 << self.failures.min(10))).min(MAX_ERROR_BACKOFF);
                    tracing::warn!(account = %self.ctx.cfg.account_id, error = %e, failures = self.failures, ?wait, "IMAP sync error; retrying");
                    let message = match &e {
                        Error::RateLimited => "The mail server is limiting downloads for this account right now; sync resumes automatically.".to_string(),
                        other => other.to_string(),
                    };
                    let (kind, now) = (e.sync_kind(), now_ms());
                    let retry_in = wait.as_millis() as i64;
                    self.shared
                        .update(|s| s.record_failure(kind, message, now, Some(retry_in)));
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.shared.wake.notified() => {}
                    }
                    // The next attempt is a whole pass, not just a login:
                    // only stored or checked mail ends the failure.
                    self.shared.poked.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

fn collect_flag_change(
    item: &crate::session::FetchItem,
    known: &HashMap<u32, db::Location>,
    folder: &str,
    gmail: bool,
    out: &mut Vec<(String, u32, db::Flags, Option<Vec<String>>)>,
) {
    let Some(uid) = item.uid else { return };
    let Some(loc) = known.get(&uid) else { return };
    let Some(flags) = item.flags.as_ref() else {
        return;
    };
    let f = db::Flags::from_imap(flags);
    let gm = gmail.then(|| crate::account::gm_label_ids(item));
    let gm_changed = gmail && item.gm_labels.is_some() && gm != loc.gm_labels;
    if f != loc.flags || gm_changed {
        out.push((
            folder.to_string(),
            uid,
            f,
            if gm_changed {
                gm
            } else {
                loc.gm_labels.clone()
            },
        ));
    }
}

/// IDLE on the primary folder; every change pokes the sync loop.
pub async fn idle_loop(ctx: Arc<Ctx>, shared: Arc<Shared>) {
    let mut failures = 0u32;
    loop {
        let result: Result<bool> = async {
            let mut s = ctx.open_session().await?;
            if !s.caps.has("IDLE") {
                return Ok(false);
            }
            let folders = ctx.folders(&mut s, false).await?;
            let Some(primary) = folders.primary().cloned() else {
                return Ok(false);
            };
            s.select(&primary.raw, true).await?;
            shared.idling.store(true, Ordering::SeqCst);
            loop {
                match s.idle(IDLE_RENEW).await? {
                    IdleEvent::Changed => {
                        failures = 0;
                        shared.poke();
                    }
                    IdleEvent::Timeout => {
                        s.noop().await?;
                    }
                }
            }
        }
        .await;
        shared.idling.store(false, Ordering::SeqCst);
        match result {
            Ok(false) => return std::future::pending().await,
            Ok(true) => {}
            Err(Error::NeedsReauth(_)) | Err(Error::Keychain(_)) => {
                return std::future::pending().await;
            }
            Err(e) => {
                failures += 1;
                let wait = Duration::from_secs(5u64 << failures.min(6)).min(MAX_ERROR_BACKOFF);
                tracing::debug!(account = %ctx.cfg.account_id, error = %e, ?wait, "IDLE connection lost; reconnecting");
                tokio::time::sleep(wait).await;
                shared.poke();
            }
        }
    }
}

/// A running account task.
#[derive(Clone)]
pub struct Handle {
    pub shared: Arc<Shared>,
    abort: Arc<AbortHandle>,
}

impl penguin_provider::SyncTask for Handle {
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

/// Spawn the account's task (sync loop + IDLE + status heartbeat).
pub fn spawn(ctx: Arc<Ctx>, shared: Arc<Shared>, idle: bool) -> Handle {
    let sync = AccountSync::new(ctx.clone(), shared.clone());
    let tick = shared.clone();
    let idle_shared = shared.clone();
    let account = ctx.cfg.account_id.clone();
    let task = tokio::spawn(async move {
        // Progress heartbeat while backfilling (no timer otherwise).
        let ticker = tick.heartbeat.run(STATUS_TICK, || {
            let s = tick.snapshot();
            if s.phase == SyncPhase::Backfilling {
                tick.observer.status(s);
            }
        });
        let idler = async {
            if idle {
                idle_loop(ctx, idle_shared).await
            } else {
                std::future::pending::<()>().await
            }
        };
        let run = AssertUnwindSafe(sync.run()).catch_unwind();
        tokio::select! {
            res = run => {
                if let Err(payload) = res {
                    let panic = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown".into());
                    tracing::error!(account = %account, panic = %panic, "IMAP sync task panicked");
                    let msg = format!("internal sync error ({panic}); retry sync to restart");
                    tick.update(|s| s.record_failure(SyncErrorKind::Internal, msg, now_ms(), None));
                }
            }
            _ = ticker => {}
            _ = idler => {}
        }
    });
    Handle {
        shared,
        abort: Arc::new(task.abort_handle()),
    }
}
