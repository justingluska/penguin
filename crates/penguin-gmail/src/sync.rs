//! Sync engine. OWNER: gmail-sync agent.
//!
//! Per account:
//! 1. First run: record `profile.history_id` into the cursor BEFORE backfill
//!    (so nothing that arrives during backfill is missed), sync labels.
//! 2. Backfill newest-first via messages.list pages, skipping known ids,
//!    persisting `backfill_page_token` after every page (resumable across
//!    restarts). Recent mail is usable within seconds.
//!
//! The history id and page token are Gmail's part of the cursor
//! ([`GmailCursor`], stored as `SyncCursor.provider_state` JSON); the rest
//! of `SyncCursor` (backfill done, parked ids, window) is generic.
//! 3. Incremental: poll history.list every ~30s (and immediately on
//!    `SyncHandle::poke`), apply adds/deletes/label changes, advance cursor.
//!    Incremental runs interleaved with backfill so new mail is never blocked
//!    behind a 250k-message backfill.
//! 4. `HistoryExpired` → re-record history id and reconcile recent pages; never
//!    wipe local data silently.
//! 5. `NeedsReauth` → phase NeedsReauth, stop that account, keep others going.
//! 6. A message that keeps failing on its own (non-404 HTTP error after
//!    `MESSAGE_ATTEMPTS` tries) is parked in `cursor.failed_message_ids` and
//!    the pass moves on. Parked ids are retried in bounded batches on
//!    incremental cycles and dropped on success or 404; while any remain,
//!    `SyncStatus.error` carries a note but the phase stays healthy.
//!
//! Throughput: backfill is bounded by each mailbox's 6,000 units/min
//! Gmail budget (see api.rs) and messages.get(format=full)'s real cost of
//! ~60 units: ≈95 messages/min ≈ 5.7k/hour per account, accounts in
//! parallel. A quota raise (PENGUIN_GMAIL_UNITS_PER_MIN) scales it linearly.
//! Newest mail lands first and is committed per 100-message chunk.
//!
//! Store calls are blocking and always go through `spawn_blocking`.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use penguin_core::{
    Label, Message, Store, SyncCursor, SyncErrorKind, SyncPhase, SyncStage, SyncStatus,
};
use penguin_provider::heartbeat::Heartbeat;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::api::{GmailClient, HistoryPage, IdPage, MessageLabels, Profile};
use crate::auth::AuthManager;
use crate::{Error, Result};

#[path = "sync_window.rs"]
mod window;
pub use window::{
    fetch_pending_bodies, range_query, search_server, to_gmail_query, window_estimate,
    window_start_ms, AccountState, AccountStates, BodyClaims, EstimateCache, OlderMail,
    ServerSearch, WindowApi, WindowPolicy, SERVER_SEARCH_FETCH_CAP, WINDOW_MONTHS_DEFAULT,
    WINDOW_MONTH_CHOICES,
};

impl penguin_provider::SyncTask for SyncHandle {
    fn poke(&self) {
        SyncHandle::poke(self);
    }
    fn stop(&self) {
        SyncHandle::stop(self);
    }
    fn is_running(&self) -> bool {
        SyncHandle::is_running(self)
    }
}

/// Callbacks into the app layer (Tauri emits these as events). Defined in
/// penguin-provider so every provider's engine reports the same way; for
/// Gmail, `messages_added` is mail stored from history.list and
/// `labels_added` is history.list labelsAdded.
pub use penguin_provider::SyncObserver;

/// Gmail's own resume position: `SyncCursor.provider_state` as JSON
/// (`{"historyId": 123, "backfillPageToken": "…"}`). Before providers
/// existed these were `sync_cursors` columns; the store migration moved
/// them here unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GmailCursor {
    /// History id to resume incremental sync from. Recorded BEFORE backfill starts.
    pub history_id: Option<u64>,
    /// messages.list page token for resumable newest-first backfill.
    pub backfill_page_token: Option<String>,
    /// The spam fill ran (see `AccountSync::spam_fill`). Missing = false,
    /// so an account synced before it existed gets its recent spam once.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub spam_filled: bool,
}

impl GmailCursor {
    /// Read `SyncCursor.provider_state` ("" = nothing recorded yet).
    pub fn parse(state: &str) -> Result<GmailCursor> {
        if state.trim().is_empty() {
            return Ok(GmailCursor::default());
        }
        serde_json::from_str(state)
            .map_err(|e| Error::Other(format!("gmail sync state is unreadable: {e}")))
    }

    /// The JSON stored in `SyncCursor.provider_state`.
    pub fn to_state(&self) -> String {
        if *self == GmailCursor::default() {
            return String::new();
        }
        serde_json::to_string(self).expect("plain struct serializes")
    }
}

/// Messages fetched and committed per store transaction.
const FETCH_CHUNK: usize = 100;
const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// Labels can be renamed/created without any history record, and
/// messagesTotal drifts; refresh both this often.
const LABEL_REFRESH: Duration = Duration::from_secs(10 * 60);
/// messages.list pages (500 ids each) re-checked after history expiry.
const RECONCILE_PAGES: usize = 2;
const STATUS_TICK: Duration = Duration::from_secs(1);
/// Backfill throughput is measured over this trailing window.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Don't report a rate from less history than this (too noisy).
const RATE_MIN_SPAN: Duration = Duration::from_secs(15);
/// EMA weight of each new window reading (one per stored chunk, ~every 5 s
/// at quota pace), so the ETA settles instead of jumping around.
const RATE_EMA_ALPHA: f64 = 0.3;
const MAX_ERROR_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Sync-level attempts for a message before it's parked in
/// `cursor.failed_message_ids` (each attempt already includes the client's
/// own backoff retries for 5xx).
const MESSAGE_ATTEMPTS: u32 = 3;
const MESSAGE_RETRY_BASE: Duration = Duration::from_secs(2);
/// Parked messages retried per incremental cycle.
const FAILED_RETRY_BATCH: usize = 50;
/// Minimum gap between retry passes over parked messages. A message that
/// keeps returning 5xx costs ~2 min of client backoff per attempt, so
/// retrying it every 30 s poll would stall everything else.
const FAILED_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The Gmail calls the sync engine needs. Implemented by [`GmailClient`];
/// tests substitute a fake.
pub trait GmailApi: Send + Sync + 'static {
    fn profile(&self) -> impl Future<Output = Result<Profile>> + Send;
    fn list_message_ids(
        &self,
        page_token: Option<&str>,
        q: Option<&str>,
    ) -> impl Future<Output = Result<IdPage>> + Send;
    /// Per-id outcome; `Ok(None)` = message no longer exists (404).
    fn get_messages_each(
        &self,
        ids: &[String],
    ) -> impl Future<Output = Vec<(String, Result<Option<Message>>)>> + Send;
    /// Headers-only messages (format=metadata) for older mail; same
    /// per-id outcomes. Defaults to full fetches.
    fn get_headers_each(
        &self,
        ids: &[String],
    ) -> impl Future<Output = Vec<(String, Result<Option<Message>>)>> + Send {
        self.get_messages_each(ids)
    }
    fn get_message_labels(
        &self,
        ids: &[String],
    ) -> impl Future<Output = Result<Vec<MessageLabels>>> + Send;
    fn list_history(
        &self,
        start_history_id: u64,
        page_token: Option<&str>,
    ) -> impl Future<Output = Result<HistoryPage>> + Send;
    fn list_labels(&self) -> impl Future<Output = Result<Vec<Label>>> + Send;
    fn label_messages_total(&self, label_id: &str) -> impl Future<Output = Result<u64>> + Send;
}

impl GmailApi for GmailClient {
    async fn profile(&self) -> Result<Profile> {
        GmailClient::profile(self).await
    }
    async fn list_message_ids(&self, page_token: Option<&str>, q: Option<&str>) -> Result<IdPage> {
        GmailClient::list_message_ids(self, page_token, q).await
    }
    async fn get_messages_each(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        GmailClient::get_messages_each(self, ids).await
    }
    async fn get_headers_each(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        GmailClient::get_messages_metadata_each(self, ids, false).await
    }
    async fn get_message_labels(&self, ids: &[String]) -> Result<Vec<MessageLabels>> {
        GmailClient::get_message_labels(self, ids).await
    }
    async fn list_history(
        &self,
        start_history_id: u64,
        page_token: Option<&str>,
    ) -> Result<HistoryPage> {
        GmailClient::list_history(self, start_history_id, page_token).await
    }
    async fn list_labels(&self) -> Result<Vec<Label>> {
        GmailClient::list_labels(self).await
    }
    async fn label_messages_total(&self, label_id: &str) -> Result<u64> {
        GmailClient::label_messages_total(self, label_id).await
    }
}

/// Errors that indict one message rather than the account or the network:
/// Gmail answered, but not with this message. Everything else (auth, rate
/// limits, transport, 401/403) stops the pass and is retried as a whole.
fn is_message_specific(e: &Error) -> bool {
    match e {
        Error::Http { status, .. } => !matches!(status, 401 | 403 | 429),
        Error::Other(_) => true,
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FetchMode {
    /// First sight of these ids: retry a few times, then park failures.
    /// If every id fails server-side, assume Gmail is having trouble (not
    /// the messages) and fail the pass instead of parking them all.
    Fresh,
    /// Parked ids: one attempt per pass; failures go to the back of the line.
    Retry,
}

#[derive(Default)]
struct Fetched {
    stored: usize,
    threads: Vec<String>,
    /// Ids of the messages stored.
    message_ids: Vec<String>,
    label_ids: HashSet<String>,
}

/// Backfill throughput for the status line: messages stored over a trailing
/// `RATE_WINDOW`, smoothed with an EMA. Reports nothing until
/// `RATE_MIN_SPAN` of measurement exists. Takes `now` explicitly so it can
/// be tested with a fake clock.
struct RateEstimator {
    since: Instant,
    samples: VecDeque<(Instant, u64)>,
    ema: Option<f64>,
}

impl RateEstimator {
    fn new(now: Instant) -> Self {
        RateEstimator {
            since: now,
            samples: VecDeque::new(),
            ema: None,
        }
    }

    /// Record `stored` messages at `now`; returns smoothed messages/min.
    fn record(&mut self, now: Instant, stored: u64) -> Option<f64> {
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
        let raw = total as f64 / span.as_secs_f64() * 60.0;
        let ema = self.ema.map_or(raw, |e| e + RATE_EMA_ALPHA * (raw - e));
        self.ema = Some(ema);
        Some(ema)
    }
}

/// Seconds left at `rate_per_min`, if both the total and a rate are known.
fn eta_secs(total: Option<u64>, indexed: u64, rate_per_min: Option<f64>) -> Option<u64> {
    let rate = rate_per_min.filter(|r| *r > 0.0)?;
    let remaining = total?.saturating_sub(indexed);
    Some((remaining as f64 / rate * 60.0).round() as u64)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// State shared between an account's task, its handle and the engine.
struct Shared {
    status: Mutex<SyncStatus>,
    poked: AtomicBool,
    wake: Notify,
    observer: Arc<dyn SyncObserver>,
    /// Sync-window settings (set by the engine; EVERYTHING when bare).
    policy: Mutex<WindowPolicy>,
    /// Progress heartbeat, ticking only while backfilling.
    heartbeat: Heartbeat,
}

impl Shared {
    fn new(account_id: &str, observer: Arc<dyn SyncObserver>) -> Arc<Self> {
        Arc::new(Shared {
            status: Mutex::new(SyncStatus::new(account_id, SyncPhase::Idle)),
            poked: AtomicBool::new(false),
            wake: Notify::new(),
            observer,
            policy: Mutex::new(WindowPolicy::EVERYTHING),
            heartbeat: Heartbeat::default(),
        })
    }

    fn snapshot(&self) -> SyncStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Mutate the status and emit it.
    fn update(&self, f: impl FnOnce(&mut SyncStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        self.heartbeat.phase(snapshot.phase);
        self.observer.status(snapshot);
    }

    fn poke(&self) {
        self.poked.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
}

/// One account's sync state machine. Generic over the Gmail client so the
/// logic is testable against a fake.
struct AccountSync<C> {
    account_id: String,
    client: C,
    store: Store,
    shared: Arc<Shared>,
    /// The generic part; `provider_state` is rewritten from `gmail` on save.
    cursor: SyncCursor,
    gmail: GmailCursor,
    labels: Vec<Label>,
    last_refresh: Option<Instant>,
    next_poll: Instant,
    failures: u32,
    last_failed_retry: Option<Instant>,
    backfill_rate: RateEstimator,
    failed_retry_interval: Duration,
    /// In-window messages still to download (fill ETA), once estimated.
    window_left: Option<u64>,
}

impl<C: GmailApi> AccountSync<C> {
    fn new(account_id: &str, client: C, store: Store, shared: Arc<Shared>) -> Self {
        AccountSync {
            account_id: account_id.to_string(),
            client,
            store,
            shared,
            cursor: SyncCursor::default(),
            gmail: GmailCursor::default(),
            labels: Vec::new(),
            last_refresh: None,
            next_poll: Instant::now(),
            failures: 0,
            last_failed_retry: None,
            backfill_rate: RateEstimator::new(Instant::now()),
            failed_retry_interval: FAILED_RETRY_INTERVAL,
            window_left: None,
        }
    }

    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> penguin_core::Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|e| Error::Other(format!("store task failed: {e}")))?
            .map_err(Error::from)
    }

    async fn save_cursor(&self) -> Result<()> {
        let (id, mut cursor) = (self.account_id.clone(), self.cursor.clone());
        cursor.provider_state = self.gmail.to_state();
        self.db(move |s| s.set_sync_cursor(&id, &cursor)).await
    }

    async fn count(&self) -> Result<u64> {
        let id = self.account_id.clone();
        self.db(move |s| s.count_messages(Some(&id))).await
    }

    fn resting_phase(&self) -> SyncPhase {
        if self.cursor.backfill_done && !self.older_pending() {
            SyncPhase::Idle
        } else {
            SyncPhase::Backfilling
        }
    }

    /// Non-fatal problem worth showing while the phase stays healthy.
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
            self.shared.observer.mail_changed(&self.account_id, threads);
        }
    }

    /// Load the cursor; on first run record the history id BEFORE any
    /// backfill so changes made during backfill are replayed afterwards.
    async fn init(&mut self) -> Result<()> {
        let id = self.account_id.clone();
        self.cursor = self.db(move |s| s.get_sync_cursor(&id)).await?;
        self.gmail = GmailCursor::parse(&self.cursor.provider_state)?;
        self.apply_window_policy().await?;
        let profile = self.client.profile().await?;
        if self.gmail.history_id.is_none() {
            self.gmail.history_id = Some(profile.history_id);
            self.save_cursor().await?;
        }
        self.refresh_labels().await?;
        let total = self.backfill_total(profile.messages_total).await?;
        let indexed = self.count().await?;
        let phase = self.resting_phase();
        let stage = self.current_stage();
        let note = self.health_note();
        self.shared.update(|s| {
            s.indexed = indexed;
            s.total_estimate = Some(total);
            // Reaching Gmail again is not progress: after a failure the
            // status keeps it until the pass that follows syncs something.
            if s.failing() {
                return;
            }
            s.phase = phase;
            s.stage = stage;
            s.error = note;
            if phase != SyncPhase::Backfilling {
                s.rate_per_min = None;
                s.eta_secs = None;
            }
        });
        self.next_poll = Instant::now() + POLL_INTERVAL;
        Ok(())
    }

    /// What backfill will actually store: the mailbox total minus SPAM and
    /// TRASH, which messages.list skips (2 extra units), so progress and ETA
    /// aim at a count they can reach.
    async fn backfill_total(&self, messages_total: u64) -> Result<u64> {
        let spam = self.client.label_messages_total("SPAM").await?;
        let trash = self.client.label_messages_total("TRASH").await?;
        Ok(messages_total.saturating_sub(spam + trash))
    }

    async fn refresh_labels(&mut self) -> Result<()> {
        let labels = self.client.list_labels().await?;
        if labels != self.labels {
            let (id, l) = (self.account_id.clone(), labels.clone());
            self.db(move |s| s.replace_labels(&id, &l)).await?;
            self.labels = labels;
        }
        self.last_refresh = Some(Instant::now());
        Ok(())
    }

    fn poll_due(&self) -> bool {
        self.shared.poked.swap(false, Ordering::SeqCst) || Instant::now() >= self.next_poll
    }

    /// Fetch `ids` and commit what arrived (one transaction per attempt).
    /// Messages that keep failing on their own are parked in
    /// `cursor.failed_message_ids` (persisted with the caller's next cursor
    /// save) so one bad message never stalls the pass.
    async fn fetch_and_store(&mut self, ids: &[String], mode: FetchMode) -> Result<Fetched> {
        let mut out = Fetched::default();
        let mut pending: Vec<String> = ids.to_vec();
        let mut last_error: Option<Error> = None;
        let mut any_resolved = false;
        let attempts = if mode == FetchMode::Fresh {
            MESSAGE_ATTEMPTS
        } else {
            1
        };
        for attempt in 1..=attempts {
            if pending.is_empty() {
                break;
            }
            if attempt > 1 {
                tokio::time::sleep(MESSAGE_RETRY_BASE * 2u32.pow(attempt - 2)).await;
            }
            let mut messages = Vec::new();
            let mut resolved = HashSet::new();
            let mut failed = Vec::new();
            let mut fatal = None;
            for (id, result) in self.client.get_messages_each(&pending).await {
                match result {
                    Ok(Some(m)) => {
                        resolved.insert(id);
                        messages.push(m);
                    }
                    // Gone from Gmail: nothing to fetch, ever.
                    Ok(None) => {
                        resolved.insert(id);
                    }
                    Err(e) if is_message_specific(&e) => {
                        failed.push(id);
                        last_error = Some(e);
                    }
                    Err(e) => {
                        failed.push(id);
                        fatal.get_or_insert(e);
                    }
                }
            }
            any_resolved |= !resolved.is_empty();
            self.store_fetched(messages, &mut out).await?;
            self.cursor
                .failed_message_ids
                .retain(|id| !resolved.contains(id));
            if let Some(e) = fatal {
                return Err(e);
            }
            pending = failed;
        }
        if pending.is_empty() {
            return Ok(out);
        }
        if mode == FetchMode::Fresh && !any_resolved && pending.len() > 1 {
            // Nothing in the batch worked: more likely Gmail than the messages.
            return Err(last_error.unwrap_or_else(|| Error::Other("message fetch failed".into())));
        }
        tracing::warn!(
            account = %self.account_id,
            count = pending.len(),
            error = %last_error.as_ref().map(ToString::to_string).unwrap_or_default(),
            "parking messages that failed to download; will retry"
        );
        // Park (or move to the back of the line, so retries rotate).
        let parked: HashSet<&String> = pending.iter().collect();
        self.cursor
            .failed_message_ids
            .retain(|id| !parked.contains(id));
        self.cursor
            .failed_message_ids
            .extend(pending.iter().cloned());
        let note = self.health_note();
        self.shared.update(|s| {
            if !s.failing() {
                s.error = note;
            }
        });
        Ok(out)
    }

    async fn store_fetched(&self, messages: Vec<Message>, out: &mut Fetched) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let threads: Vec<String> = messages.iter().map(|m| m.thread_id.clone()).collect();
        out.message_ids
            .extend(messages.iter().map(|m| m.id.clone()));
        out.label_ids
            .extend(messages.iter().flat_map(|m| m.label_ids.iter().cloned()));
        out.stored += messages.len();
        self.db(move |s| s.upsert_messages(&messages)).await?;
        self.notify_changed(threads.iter().cloned());
        out.threads.extend(threads);
        Ok(())
    }

    /// Retry a bounded batch of parked messages, at most every
    /// `failed_retry_interval`. Caller persists the cursor.
    async fn retry_failed(&mut self) -> Result<Vec<String>> {
        if self.cursor.failed_message_ids.is_empty()
            || self
                .last_failed_retry
                .is_some_and(|t| t.elapsed() < self.failed_retry_interval)
        {
            return Ok(vec![]);
        }
        self.last_failed_retry = Some(Instant::now());
        let batch: Vec<String> = self
            .cursor
            .failed_message_ids
            .iter()
            .take(FAILED_RETRY_BATCH)
            .cloned()
            .collect();
        let fetched = self.fetch_and_store(&batch, FetchMode::Retry).await?;
        if fetched.stored > 0 {
            tracing::info!(account = %self.account_id, recovered = fetched.stored, "parked messages downloaded");
        }
        Ok(fetched.threads)
    }

    /// Ids from `ids` not yet in the store, order preserved.
    async fn unknown(&self, ids: Vec<String>) -> Result<Vec<String>> {
        let (acct, probe) = (self.account_id.clone(), ids.clone());
        let known = self.db(move |s| s.known_message_ids(&acct, &probe)).await?;
        Ok(ids.into_iter().filter(|id| !known.contains(id)).collect())
    }

    /// One messages.list page of backfill (newest first), then persist the
    /// page token. Runs due history polls between chunks.
    async fn backfill_page(&mut self) -> Result<()> {
        let token = self.gmail.backfill_page_token.clone();
        let q = self.fill_query();
        let page = match self
            .client
            .list_message_ids(token.as_deref(), q.as_deref())
            .await
        {
            // Saved page tokens can go stale. Restart from the newest page;
            // known ids are skipped, so this costs only list calls.
            Err(Error::Http { status: 400, .. }) if token.is_some() => {
                tracing::warn!(account = %self.account_id, "backfill page token rejected; restarting listing");
                self.gmail.backfill_page_token = None;
                return self.save_cursor().await;
            }
            other => other?,
        };
        self.note_fill_estimate(&page).await?;
        // Parked ids are the retry loop's job; re-trying them here would
        // stall the page again.
        let parked: HashSet<String> = self.cursor.failed_message_ids.iter().cloned().collect();
        let listed: Vec<String> = page
            .ids
            .iter()
            .map(|(m, _)| m.clone())
            .filter(|m| !parked.contains(m))
            .collect();
        // Headers-only messages inside the window are upgraded too.
        let missing = self.needing_body(listed).await?;
        for chunk in missing.chunks(FETCH_CHUNK) {
            let fetched = self.fetch_and_store(chunk, FetchMode::Fresh).await?.stored as u64;
            let rate = self.backfill_rate.record(Instant::now(), fetched);
            // Upgrades replace a stored row, so recount instead of adding.
            let indexed = self.count().await?;
            let total = self.shared.snapshot().total_estimate;
            let eta = self.fill_eta(fetched, rate, total, indexed);
            let (now, note) = (now_ms(), self.health_note());
            self.shared.update(|s| {
                if s.failing() {
                    // Mail was stored: the failure is over.
                    s.record_progress(now);
                    s.phase = SyncPhase::Backfilling;
                    s.error = note;
                }
                s.indexed = indexed;
                s.stage = Some(SyncStage::Window);
                s.rate_per_min = rate;
                s.eta_secs = eta;
            });
            // The error backoff starts over after work that succeeded.
            self.failures = 0;
            if self.poll_due() {
                self.poll_history().await?;
            }
        }
        self.cursor.backfill_done = page.next_page_token.is_none();
        self.gmail.backfill_page_token = page.next_page_token;
        if self.cursor.backfill_done {
            self.fill_completed();
        }
        self.save_cursor().await?;
        if self.cursor.backfill_done {
            tracing::info!(account = %self.account_id, "backfill complete");
            // Merge the FTS segments a large backfill leaves behind.
            self.db(|s| s.optimize()).await?;
            let indexed = self.count().await?;
            let (phase, stage) = (self.resting_phase(), self.current_stage());
            let (now, note) = (now_ms(), self.health_note());
            self.shared.update(|s| {
                if s.failing() {
                    s.record_progress(now);
                    s.error = note;
                }
                s.phase = phase;
                s.stage = stage;
                s.rate_per_min = None;
                s.eta_secs = None;
                s.indexed = indexed;
            });
        }
        Ok(())
    }

    /// Replay history since the cursor. On expiry, re-anchor and reconcile.
    async fn poll_history(&mut self) -> Result<()> {
        let start = match self.gmail.history_id {
            Some(h) => h,
            None => return Err(Error::Other("history poll before init".into())),
        };
        if self.cursor.backfill_done {
            self.shared.update(|s| {
                // Not while a failure is open: this poll hasn't worked yet.
                if !s.failing() {
                    s.phase = SyncPhase::Incremental;
                }
            });
        }
        let mut token: Option<String> = None;
        let mut latest = start;
        let mut changed = HashSet::new();
        let mut labels_stale = false;
        loop {
            let page = match self.client.list_history(start, token.as_deref()).await {
                Err(Error::HistoryExpired) => {
                    self.recover_history_expired().await?;
                    return self.finish_poll(HashSet::new(), true).await;
                }
                other => other?,
            };
            labels_stale |= self.apply_history_page(&page, &mut changed).await?;
            latest = latest.max(page.history_id);
            token = page.next_page_token;
            if token.is_none() {
                break;
            }
        }
        changed.extend(self.retry_failed().await?);
        self.gmail.history_id = Some(latest);
        self.save_cursor().await?;
        self.finish_poll(changed, labels_stale).await
    }

    async fn finish_poll(&mut self, changed: HashSet<String>, labels_stale: bool) -> Result<()> {
        let refresh_due = self
            .last_refresh
            .is_none_or(|t| t.elapsed() >= LABEL_REFRESH);
        let mut total = None;
        if labels_stale || refresh_due {
            self.refresh_labels().await?;
            if refresh_due {
                let profile = self.client.profile().await?;
                total = Some(self.backfill_total(profile.messages_total).await?);
            }
        }
        self.notify_changed(changed);
        let indexed = self.count().await?;
        let phase = self.resting_phase();
        let stage = self.current_stage();
        let note = self.health_note();
        let now = now_ms();
        // The poll ends a failure only once backfill is done: while it
        // runs, the failure is the backfill's (its next page ends it).
        let progress = phase != SyncPhase::Backfilling;
        self.shared.update(|s| {
            s.indexed = indexed;
            s.last_synced_at = Some(now);
            if total.is_some() {
                s.total_estimate = total;
            }
            if !progress && s.failing() {
                return;
            }
            s.record_progress(now);
            s.phase = phase;
            s.stage = stage;
            s.error = note;
            if phase != SyncPhase::Backfilling {
                s.rate_per_min = None;
                s.eta_secs = None;
            }
        });
        self.next_poll = Instant::now() + POLL_INTERVAL;
        if progress {
            self.failures = 0;
        }
        Ok(())
    }

    /// Apply one history page. Returns true if it referenced labels we
    /// don't know (a label was created) so the label list needs a refresh.
    async fn apply_history_page(
        &mut self,
        page: &HistoryPage,
        changed: &mut HashSet<String>,
    ) -> Result<bool> {
        let deleted: HashSet<&String> = page.deleted.iter().collect();
        let added: HashSet<&String> = page.added.iter().collect();
        let known_labels: HashSet<String> = self.labels.iter().map(|l| l.id.clone()).collect();
        let mut labels_stale = false;

        // Label deltas, grouped so a bulk action is one store call. Messages
        // added on this page are skipped: they're re-fetched with current labels.
        let mut ops: HashMap<(Vec<String>, bool), Vec<String>> = HashMap::new();
        let mut labels_added: Vec<(String, Vec<String>)> = Vec::new();
        for (list, is_add) in [(&page.labels_added, true), (&page.labels_removed, false)] {
            for (msg, labels) in list {
                if deleted.contains(msg) || added.contains(msg) {
                    continue;
                }
                labels_stale |= labels.iter().any(|l| !known_labels.contains(l));
                if is_add {
                    labels_added.push((msg.clone(), labels.clone()));
                }
                ops.entry((labels.clone(), is_add))
                    .or_default()
                    .push(msg.clone());
            }
        }

        let acct = self.account_id.clone();
        let del: Vec<String> = page.deleted.clone();
        let threads = page.message_threads.clone();
        let touched: Vec<String> = ops
            .values()
            .flatten()
            .cloned()
            .chain(del.iter().cloned())
            .collect();
        let affected_threads = self
            .db(move |s| {
                // Thread ids for messages the page didn't attribute (and to
                // skip label changes on messages we haven't synced yet).
                let mut out = Vec::new();
                for id in &touched {
                    match threads.get(id) {
                        Some(t) => out.push(t.clone()),
                        None => {
                            if let Some(m) = s.get_message(&acct, id)? {
                                out.push(m.thread_id);
                            }
                        }
                    }
                }
                for ((labels, is_add), ids) in &ops {
                    let none: &[String] = &[];
                    let (add, remove) = if *is_add {
                        (labels.as_slice(), none)
                    } else {
                        (none, labels.as_slice())
                    };
                    s.modify_message_labels(&acct, ids, add, remove)?;
                }
                if !del.is_empty() {
                    s.delete_messages(&acct, &del)?;
                }
                Ok(out)
            })
            .await?;
        changed.extend(affected_threads);
        // Deleted for good: stop retrying them.
        self.cursor
            .failed_message_ids
            .retain(|id| !deleted.contains(&id));

        let to_fetch: Vec<String> = page
            .added
            .iter()
            .filter(|m| !deleted.contains(m))
            .cloned()
            .collect();
        for chunk in to_fetch.chunks(FETCH_CHUNK) {
            let fetched = self.fetch_and_store(chunk, FetchMode::Fresh).await?;
            changed.extend(fetched.threads);
            labels_stale |= fetched.label_ids.iter().any(|l| !known_labels.contains(l));
            if !fetched.message_ids.is_empty() {
                self.shared
                    .observer
                    .messages_added(&self.account_id, fetched.message_ids);
            }
        }
        if !labels_added.is_empty() {
            self.shared
                .observer
                .labels_added(&self.account_id, labels_added);
        }
        Ok(labels_stale)
    }

    /// history.list said our start id is too old. Re-anchor at the current
    /// history id FIRST (so later changes aren't lost), then re-check the
    /// newest pages: fetch messages we lack and fix labels on ones we have.
    /// Local data is never wiped; older drift is repaired as it's touched.
    async fn recover_history_expired(&mut self) -> Result<()> {
        tracing::warn!(account = %self.account_id, "gmail history expired; re-anchoring and reconciling recent mail");
        let profile = self.client.profile().await?;
        self.gmail.history_id = Some(profile.history_id);
        self.save_cursor().await?;

        let mut token: Option<String> = None;
        for _ in 0..RECONCILE_PAGES {
            // in:anywhere so messages trashed/spammed during the gap show up
            // with their new labels instead of looking unchanged.
            let page = self
                .client
                .list_message_ids(token.as_deref(), Some("in:anywhere"))
                .await?;
            let ids: Vec<String> = page.ids.iter().map(|(m, _)| m.clone()).collect();
            let missing = self.unknown(ids.clone()).await?;
            for chunk in missing.chunks(FETCH_CHUNK) {
                self.fetch_and_store(chunk, FetchMode::Fresh).await?;
            }
            let missing: HashSet<String> = missing.into_iter().collect();
            let known: Vec<String> = ids.into_iter().filter(|id| !missing.contains(id)).collect();
            for chunk in known.chunks(FETCH_CHUNK) {
                let current = self.client.get_message_labels(chunk).await?;
                let acct = self.account_id.clone();
                let threads = self
                    .db(move |s| {
                        let mut touched = Vec::new();
                        for m in current {
                            let Some(local) = s.get_message(&acct, &m.id)? else {
                                continue;
                            };
                            let add: Vec<String> = m
                                .label_ids
                                .iter()
                                .filter(|l| !local.label_ids.contains(l))
                                .cloned()
                                .collect();
                            let remove: Vec<String> = local
                                .label_ids
                                .iter()
                                .filter(|l| !m.label_ids.contains(l))
                                .cloned()
                                .collect();
                            if !add.is_empty() || !remove.is_empty() {
                                s.modify_message_labels(
                                    &acct,
                                    std::slice::from_ref(&m.id),
                                    &add,
                                    &remove,
                                )?;
                                touched.push(m.thread_id);
                            }
                        }
                        Ok(touched)
                    })
                    .await?;
                self.notify_changed(threads);
            }
            token = page.next_page_token;
            if token.is_none() {
                break;
            }
        }
        // Persist anything parked during the reconcile.
        self.save_cursor().await
    }

    /// Init, then alternate backfill pages with history polls until stopped.
    async fn run_inner(&mut self) -> Result<()> {
        self.init().await?;
        loop {
            self.apply_window_policy().await?;
            if self.poll_due() {
                self.poll_history().await?;
            }
            if !self.cursor.backfill_done {
                self.backfill_page().await?;
                continue;
            }
            if self.spam_fill().await? {
                continue;
            }
            if self.older_page().await? {
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep_until(self.next_poll) => {}
                _ = self.shared.wake.notified() => {}
            }
        }
    }

    /// Runs until stopped (aborted) or the account needs re-auth. Errors
    /// back off exponentially (a poke retries immediately).
    async fn run(mut self) {
        loop {
            match self.run_inner().await {
                Ok(()) => return,
                Err(Error::NeedsReauth(msg)) => {
                    tracing::warn!(account = %self.account_id, "sync stopped: account needs re-auth");
                    let now = now_ms();
                    self.shared
                        .update(|s| s.record_failure(SyncErrorKind::Auth, msg, now, None));
                    return;
                }
                // The Keychain refused (access denied/cancelled, locked).
                // Retrying on a timer would re-prompt the user over and over,
                // so stop until they act (reconnect, or SyncEngine::retry_account).
                Err(Error::Keychain(msg)) => {
                    tracing::warn!(account = %self.account_id, error = %msg, "sync stopped: keychain access failed");
                    let now = now_ms();
                    self.shared.update(|s| {
                        let msg = format!("Keychain access failed: {msg}");
                        s.record_failure(SyncErrorKind::Keychain, msg, now, None)
                    });
                    return;
                }
                Err(e) => {
                    self.failures += 1;
                    let wait =
                        Duration::from_secs(5u64 << self.failures.min(10)).min(MAX_ERROR_BACKOFF);
                    tracing::warn!(account = %self.account_id, error = %e, failures = self.failures, ?wait, "sync error; retrying");
                    let (kind, message, now) = (e.sync_kind(), e.to_string(), now_ms());
                    let retry_in = wait.as_millis() as i64;
                    self.shared
                        .update(|s| s.record_failure(kind, message, now, Some(retry_in)));
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.shared.wake.notified() => {}
                    }
                    // The next attempt polls too: only synced mail ends the failure.
                    self.shared.poked.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

// ---------- public engine ----------

#[derive(Clone)]
pub struct SyncEngine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    store: Store,
    auth: AuthManager,
    observer: Arc<dyn SyncObserver>,
    accounts: Mutex<HashMap<String, AccountEntry>>,
    policy: Mutex<WindowPolicy>,
}

struct AccountEntry {
    shared: Arc<Shared>,
    handle: Option<SyncHandle>,
}

/// Handle to a running per-account sync task.
#[derive(Clone)]
pub struct SyncHandle {
    shared: Arc<Shared>,
    abort: Arc<AbortHandle>,
}

impl SyncHandle {
    /// Trigger an immediate incremental sync (e.g. after a user action or window focus).
    pub fn poke(&self) {
        self.shared.poke();
    }
    /// Abort the account's task. An in-flight store transaction still
    /// completes (blocking tasks aren't cancelled), and the cursor is saved
    /// after each page, so the next start resumes cleanly.
    pub fn stop(&self) {
        self.abort.abort();
    }
    pub fn is_running(&self) -> bool {
        !self.abort.is_finished()
    }
}

impl SyncEngine {
    pub fn new(store: Store, auth: AuthManager, observer: Arc<dyn SyncObserver>) -> Self {
        SyncEngine {
            inner: Arc::new(EngineInner {
                store,
                auth,
                observer,
                accounts: Mutex::new(HashMap::new()),
                policy: Mutex::new(WindowPolicy::default()),
            }),
        }
    }

    fn shared_for(&self, account_id: &str) -> Arc<Shared> {
        let mut accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        accounts
            .entry(account_id.to_string())
            .or_insert_with(|| {
                let shared = Shared::new(account_id, self.inner.observer.clone());
                *shared.policy.lock().unwrap_or_else(|e| e.into_inner()) = self.window_policy();
                AccountEntry {
                    shared,
                    handle: None,
                }
            })
            .shared
            .clone()
    }

    pub fn window_policy(&self) -> WindowPolicy {
        *self.inner.policy.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Apply new sync-window settings to every account (live) and wake them:
    /// a larger window starts filling the new band right away.
    pub fn set_window_policy(&self, policy: WindowPolicy) {
        *self.inner.policy.lock().unwrap_or_else(|e| e.into_inner()) = policy;
        let accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for entry in accounts.values() {
            *entry
                .shared
                .policy
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = policy;
            entry.shared.poke();
        }
    }

    /// Spawn the background loop for one account on the current tokio runtime.
    /// Starting an account that's already running returns the existing handle.
    pub fn start_account(&self, account_id: &str) -> SyncHandle {
        let shared = self.shared_for(account_id);
        let mut accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = accounts.get_mut(account_id).expect("entry created above");
        if let Some(h) = entry.handle.as_ref().filter(|h| h.is_running()) {
            return h.clone();
        }

        let client = GmailClient::new(self.inner.auth.clone(), account_id);
        let sync = AccountSync::new(account_id, client, self.inner.store.clone(), shared.clone());
        let ticker_shared = shared.clone();
        let account = account_id.to_string();
        let task = tokio::spawn(async move {
            // Progress heartbeat: the UI gets a status at least once a
            // second while backfilling (no timer otherwise).
            let ticker = ticker_shared.heartbeat.run(STATUS_TICK, || {
                let s = ticker_shared.snapshot();
                if s.phase == SyncPhase::Backfilling {
                    ticker_shared.observer.status(s);
                }
            });
            // A panic in one account must never take down the others.
            let run = AssertUnwindSafe(sync.run()).catch_unwind();
            tokio::select! {
                res = run => {
                    if let Err(payload) = res {
                        let panic = crate::panic_message(&*payload);
                        tracing::error!(account = %account, panic = %panic, "sync task panicked");
                        let msg = format!("internal sync error ({panic}); restart sync to retry");
                        ticker_shared
                            .update(|s| s.record_failure(SyncErrorKind::Internal, msg, now_ms(), None));
                    }
                }
                _ = ticker => {}
            }
        });
        let handle = SyncHandle {
            shared,
            abort: Arc::new(task.abort_handle()),
        };
        entry.handle = Some(handle.clone());
        handle
    }

    /// User-initiated retry: clear the account's stale Error/NeedsReauth
    /// status (emitting it), then restart the task if it has stopped
    /// (panicked, needs-reauth, keychain) or wake a live one sleeping in its
    /// error backoff.
    pub fn retry_account(&self, account_id: &str) -> SyncHandle {
        let shared = self.shared_for(account_id);
        // The failure stays until this attempt syncs (or fails again), so
        // the UI can tell whether the retry worked.
        shared.update(|s| s.retry_now(now_ms()));
        let handle = self.start_account(account_id);
        handle.poke();
        handle
    }

    pub fn status(&self, account_id: &str) -> Option<SyncStatus> {
        let accounts = self
            .inner
            .accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        accounts.get(account_id).map(|e| e.shared.snapshot())
    }

    /// Run one full pass (backfill page(s) + incremental) — used by tests/CLI.
    /// Does one backfill page if backfill is unfinished, then one history poll.
    pub async fn sync_once(&self, account_id: &str) -> Result<()> {
        let shared = self.shared_for(account_id);
        let client = GmailClient::new(self.inner.auth.clone(), account_id);
        let mut sync = AccountSync::new(account_id, client, self.inner.store.clone(), shared);
        sync.init().await?;
        if !sync.cursor.backfill_done {
            sync.backfill_page().await?;
        }
        sync.poll_history().await
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_bench.rs"]
mod bench;
