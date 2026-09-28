//! Sync window. OWNER: sync-window agent. A child module of `sync`, so it
//! extends `AccountSync` directly.
//!
//! Only the last `months` of mail is downloaded in full by default; older
//! mail is stored headers-only (or skipped, or downloaded in full later):
//!
//! - **Stage A, the fill** (`SyncStage::Window`): the existing newest-first
//!   backfill, but listing `after:<window start>` so only in-window ids come
//!   back. Messages stored headers-only are upgraded too. On completion
//!   `WindowCursor::full_since_ms` drops to the window start. A larger
//!   window later runs another fill for just the new band
//!   (`after:<new start> before:<full_since>`); a smaller one changes
//!   nothing (bodies are dropped only by an explicit "free up space").
//! - **Stage B, older mail** (`SyncStage::Older`): runs only once the fill
//!   is complete, listing `before:<full_since>`. `olderMail = headers` stores
//!   format=metadata (a third of a full get's quota cost) marked
//!   body-pending; `full` downloads them in full; `none` skips it. Each
//!   chunk yields to due history polls, and a policy change that needs a
//!   fill takes over at the next page.
//! - New mail from history.list is always fetched in full.
//!
//! Everything is resumable: `WindowCursor` is saved after every page, and
//! the fill reuses `GmailCursor::backfill_page_token` and
//! `SyncCursor::backfill_done`.
//!
//! Also here, for the app layer: lazy full fetch of body-pending messages
//! when a thread is opened ([`fetch_pending_bodies`]), server-side search
//! for mail outside the window ([`search_server`]) and the per-window
//! message estimates for Settings ([`window_estimate`]).

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Penguin queries in Gmail syntax live in penguin-core (the IMAP provider
/// uses them for X-GM-RAW too); re-exported where they always were.
pub use penguin_core::query_gmail::{range_query, to_gmail_query};
use penguin_core::{Message, SearchHit, Store, SyncStage};
use tokio::time::Instant;

use super::{AccountSync, FetchMode, GmailApi, FETCH_CHUNK};
use crate::api::IdPage;
use crate::{Error, Result};

// The policy types are provider-neutral (penguin-provider `window`).
use penguin_provider::window::DAY_MS;
pub use penguin_provider::window::{
    window_start_ms, OlderMail, WindowPolicy, WINDOW_MONTHS_DEFAULT, WINDOW_MONTH_CHOICES,
};
pub use penguin_provider::{ServerSearch, SERVER_SEARCH_FETCH_CAP};

/// A fill in progress is restarted only when the target moves by more than
/// this (the window start slides forward a day at a time).
const FILL_SLACK_MS: i64 = 2 * DAY_MS;

/// How the fill must change for `target` (the window start). Pure, so the
/// transitions are unit-tested without a store.
pub(crate) fn plan_fill(
    w: &mut penguin_core::WindowCursor,
    backfill_done: &mut bool,
    page_token: &mut Option<String>,
    target: i64,
) -> bool {
    let before = w.full_since_ms;
    let satisfied = before.is_some_and(|fs| fs <= target);
    if satisfied {
        if *backfill_done {
            return false;
        }
        // A fill that's no longer needed (the window shrank back).
        *backfill_done = true;
        *page_token = None;
        w.fill_after_ms = None;
        w.fill_before_ms = None;
        return true;
    }
    let in_progress = !*backfill_done && w.fill_after_ms.is_some();
    if in_progress
        && w.fill_before_ms == before
        && w.fill_after_ms
            .is_some_and(|a| (a - target).abs() <= FILL_SLACK_MS)
    {
        return false;
    }
    // Start (or restart) a fill of [target, before). The saved page token
    // belongs to another query, so the listing starts over; ids already
    // stored in full are skipped at the cost of list calls only.
    *backfill_done = false;
    *page_token = None;
    w.fill_after_ms = Some(target);
    w.fill_before_ms = before;
    true
}

impl<C: GmailApi> AccountSync<C> {
    pub(super) fn policy(&self) -> WindowPolicy {
        *self.shared.policy.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bring the cursor in line with the current policy (called at init and
    /// before every loop step). Persists and resets the rate when it changes.
    pub(super) async fn apply_window_policy(&mut self) -> Result<()> {
        let policy = self.policy();
        let w = &mut self.cursor.window;
        // A mailbox that finished the pre-window backfill has everything.
        let legacy_done =
            w.full_since_ms.is_none() && w.fill_after_ms.is_none() && self.cursor.backfill_done;
        if legacy_done {
            w.full_since_ms = Some(0);
        }
        let target = window_start_ms(super::now_ms(), policy.months);
        let changed = plan_fill(
            w,
            &mut self.cursor.backfill_done,
            &mut self.gmail.backfill_page_token,
            target,
        );
        if changed || legacy_done {
            if changed {
                tracing::info!(
                    account = %self.account_id,
                    after = ?self.cursor.window.fill_after_ms,
                    before = ?self.cursor.window.fill_before_ms,
                    done = self.cursor.backfill_done,
                    "sync window changed"
                );
                self.window_left = None;
                self.backfill_rate = super::RateEstimator::new(Instant::now());
            }
            self.save_cursor().await?;
        }
        Ok(())
    }

    /// messages.list `q` for the fill in progress.
    pub(super) fn fill_query(&self) -> Option<String> {
        let w = &self.cursor.window;
        range_query(w.fill_after_ms, w.fill_before_ms)
    }

    /// The fill's listing reached its end: everything since its lower bound
    /// is now full. Caller saves the cursor.
    pub(super) fn fill_completed(&mut self) {
        let w = &mut self.cursor.window;
        w.full_since_ms = Some(w.fill_after_ms.unwrap_or(0));
        w.fill_after_ms = None;
        w.fill_before_ms = None;
        self.window_left = None;
        self.backfill_rate = super::RateEstimator::new(Instant::now());
    }

    /// Whether stage B still has work under the current policy.
    pub(super) fn older_pending(&self) -> bool {
        let w = &self.cursor.window;
        let mode = self.policy().older;
        let Some(fs) = w.full_since_ms else {
            return false;
        };
        self.cursor.backfill_done
            && fs > 0
            && mode != OlderMail::None
            && !(w.older_done
                && w.older_before_ms == Some(fs)
                && w.older_mode.as_deref() == Some(mode.as_str()))
    }

    /// The stage to report with a Backfilling phase.
    pub(super) fn current_stage(&self) -> Option<SyncStage> {
        if !self.cursor.backfill_done {
            Some(SyncStage::Window)
        } else if self.older_pending() {
            Some(SyncStage::Older)
        } else {
            None
        }
    }

    /// Record the fill listing's size estimate and derive what's left.
    pub(super) async fn note_fill_estimate(&mut self, page: &IdPage) -> Result<()> {
        if self.window_left.is_some() {
            return Ok(());
        }
        let (acct, after, before) = (
            self.account_id.clone(),
            self.cursor.window.fill_after_ms.unwrap_or(0),
            self.cursor.window.fill_before_ms,
        );
        let have = self
            .db(move |s| s.count_full_between(&acct, after, before))
            .await?;
        self.window_left = Some(page.result_size_estimate.saturating_sub(have));
        Ok(())
    }

    /// ETA for the fill: what's left of the window at `rate`, falling back
    /// to the mailbox total when no estimate exists yet.
    pub(super) fn fill_eta(
        &mut self,
        added: u64,
        rate: Option<f64>,
        total: Option<u64>,
        indexed: u64,
    ) -> Option<u64> {
        match self.window_left.as_mut() {
            Some(left) => {
                *left = left.saturating_sub(added);
                super::eta_secs(Some(*left), 0, rate)
            }
            None => super::eta_secs(total, indexed, rate),
        }
    }

    /// One page of stage B. Returns false when there is nothing to do.
    pub(super) async fn older_page(&mut self) -> Result<bool> {
        if !self.older_pending() {
            return Ok(false);
        }
        let mode = self.policy().older;
        let fs = self.cursor.window.full_since_ms.unwrap_or(0);
        {
            let w = &mut self.cursor.window;
            if w.older_before_ms != Some(fs) || w.older_mode.as_deref() != Some(mode.as_str()) {
                w.older_before_ms = Some(fs);
                w.older_mode = Some(mode.as_str().into());
                w.older_page_token = None;
                w.older_done = false;
                self.backfill_rate = super::RateEstimator::new(Instant::now());
            }
        }
        self.shared.update(|s| {
            s.phase = penguin_core::SyncPhase::Backfilling;
            s.stage = Some(SyncStage::Older);
        });
        let token = self.cursor.window.older_page_token.clone();
        let q = range_query(None, Some(fs));
        let page = match self
            .client
            .list_message_ids(token.as_deref(), q.as_deref())
            .await
        {
            Err(Error::Http { status: 400, .. }) if token.is_some() => {
                tracing::warn!(account = %self.account_id, "older-mail page token rejected; restarting listing");
                self.cursor.window.older_page_token = None;
                self.save_cursor().await?;
                return Ok(true);
            }
            other => other?,
        };
        let parked: HashSet<String> = self.cursor.failed_message_ids.iter().cloned().collect();
        let listed: Vec<String> = page
            .ids
            .iter()
            .map(|(m, _)| m.clone())
            .filter(|m| !parked.contains(m))
            .collect();
        let todo = match mode {
            OlderMail::Full => self.needing_body(listed).await?,
            _ => self.unknown(listed).await?,
        };
        for chunk in todo.chunks(FETCH_CHUNK) {
            let added = match mode {
                OlderMail::Full => self.fetch_and_store(chunk, FetchMode::Fresh).await?.stored,
                _ => self.fetch_headers_and_store(chunk).await?,
            } as u64;
            let rate = self.backfill_rate.record(Instant::now(), added);
            self.shared.update(|s| {
                s.indexed += added;
                s.rate_per_min = rate;
                s.eta_secs = super::eta_secs(s.total_estimate, s.indexed, rate);
            });
            if self.poll_due() {
                self.poll_history().await?;
            }
            // A policy change that needs a fill (a bigger window) or turns
            // this stage off takes over from the next page.
            if self.policy().older != mode {
                break;
            }
        }
        let w = &mut self.cursor.window;
        if w.older_mode.as_deref() == Some(mode.as_str()) {
            w.older_done = page.next_page_token.is_none();
            w.older_page_token = page.next_page_token;
        }
        self.save_cursor().await?;
        if !self.older_pending() {
            tracing::info!(account = %self.account_id, mode = mode.as_str(), "older mail complete");
            self.db(|s| s.optimize()).await?;
            let indexed = self.count().await?;
            self.shared.update(|s| {
                s.phase = penguin_core::SyncPhase::Idle;
                s.stage = None;
                s.rate_per_min = None;
                s.eta_secs = None;
                s.indexed = indexed;
            });
        }
        Ok(true)
    }

    /// Ids from `ids` without a full body (unknown or headers-only).
    pub(super) async fn needing_body(&self, ids: Vec<String>) -> Result<Vec<String>> {
        let acct = self.account_id.clone();
        self.db(move |s| s.ids_needing_body(&acct, &ids)).await
    }

    /// Fetch headers for `ids` and store them body-pending. Per-message
    /// failures are parked (the retry loop later fetches them in full);
    /// account-level failures fail the page.
    async fn fetch_headers_and_store(&mut self, ids: &[String]) -> Result<usize> {
        let mut messages = Vec::new();
        let mut parked = Vec::new();
        let mut fatal = None;
        let mut resolved = 0usize;
        for (id, result) in self.client.get_headers_each(ids).await {
            match result {
                Ok(Some(m)) => {
                    resolved += 1;
                    messages.push(m);
                }
                Ok(None) => resolved += 1,
                Err(e) if super::is_message_specific(&e) => parked.push((id, e)),
                Err(e) => {
                    fatal.get_or_insert(e);
                }
            }
        }
        let threads: Vec<String> = messages.iter().map(|m| m.thread_id.clone()).collect();
        let stored = if messages.is_empty() {
            0
        } else {
            self.db(move |s| s.insert_header_messages(&messages))
                .await?
        };
        self.notify_changed(threads);
        if let Some(e) = fatal {
            return Err(e);
        }
        if !parked.is_empty() {
            if resolved == 0 && parked.len() > 1 {
                return Err(parked.pop().map(|(_, e)| e).expect("non-empty"));
            }
            tracing::warn!(account = %self.account_id, count = parked.len(), "parking messages whose headers failed to download");
            let ids: HashSet<&String> = parked.iter().map(|(id, _)| id).collect();
            self.cursor
                .failed_message_ids
                .retain(|id| !ids.contains(id));
            self.cursor
                .failed_message_ids
                .extend(parked.iter().map(|(id, _)| id.clone()));
            let note = self.health_note();
            self.shared.update(|s| s.error = note);
        }
        Ok(stored)
    }
}

// ---------------------------------------------------------------------------
// Interactive helpers for the app layer (lazy bodies, server search,
// estimates). Generic over `WindowApi` so tests use a fake.

/// The Gmail calls the interactive helpers need, all at interactive
/// priority (they skip the background queue and borrow from the budget).
pub trait WindowApi: Send + Sync {
    fn search_ids(
        &self,
        q: &str,
        max_results: u32,
    ) -> impl std::future::Future<Output = Result<IdPage>> + Send;
    fn headers_now(
        &self,
        ids: &[String],
    ) -> impl std::future::Future<Output = Vec<(String, Result<Option<Message>>)>> + Send;
    fn full_now(
        &self,
        ids: &[String],
    ) -> impl std::future::Future<Output = Vec<(String, Result<Option<Message>>)>> + Send;
}

impl WindowApi for crate::api::GmailClient {
    async fn search_ids(&self, q: &str, max_results: u32) -> Result<IdPage> {
        self.search_message_ids(q, max_results).await
    }
    async fn headers_now(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        self.get_messages_metadata_each(ids, true).await
    }
    async fn full_now(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        let results =
            futures_util::future::join_all(ids.iter().map(|id| self.get_message_interactive(id)))
                .await;
        ids.iter().cloned().zip(results).collect()
    }
}

fn in_flight() -> &'static Mutex<HashSet<(String, String)>> {
    static SET: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Download the full bodies of `ids` (headers-only messages of an opened
/// thread) and store them. Ids already being fetched by another call are
/// skipped. Returns the thread ids that changed.
pub async fn fetch_pending_bodies<A: WindowApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    ids: &[String],
) -> Result<Vec<String>> {
    let claimed: Vec<String> = {
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        ids.iter()
            .filter(|id| set.insert((account_id.to_string(), (*id).clone())))
            .cloned()
            .collect()
    };
    if claimed.is_empty() {
        return Ok(vec![]);
    }
    let result = async {
        // The caller's list may predate a download that finished since
        // (another get_thread, the window fill). Refetching would spend a
        // full get and re-store the message for nothing.
        let (store2, acct, ids) = (store.clone(), account_id.to_string(), claimed.clone());
        let claimed: Vec<String> = tokio::task::spawn_blocking(move || {
            let mut still = Vec::new();
            for id in ids {
                if store2.is_body_pending(&acct, &id)? == Some(true) {
                    still.push(id);
                }
            }
            Ok::<_, penguin_core::Error>(still)
        })
        .await
        .map_err(|e| Error::Other(format!("store task failed: {e}")))??;
        if claimed.is_empty() {
            return Ok(vec![]);
        }
        let mut messages = Vec::new();
        let mut last_err = None;
        for (id, r) in api.full_now(&claimed).await {
            match r {
                Ok(Some(m)) => messages.push(m),
                // Gone from Gmail: history will delete it locally.
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(account = %account_id, message = %id, error = %e, "could not download a message body");
                    last_err = Some(e);
                }
            }
        }
        let threads: Vec<String> = messages.iter().map(|m| m.thread_id.clone()).collect();
        if !messages.is_empty() {
            let store = store.clone();
            tokio::task::spawn_blocking(move || store.upsert_messages(&messages))
                .await
                .map_err(|e| Error::Other(format!("store task failed: {e}")))??;
        }
        match (threads.is_empty(), last_err) {
            (true, Some(e)) => Err(e),
            _ => Ok(threads),
        }
    }
    .await;
    let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
    for id in &claimed {
        set.remove(&(account_id.to_string(), id.clone()));
    }
    result
}

/// Messages listed per server search and account.
pub const SERVER_SEARCH_LIST: u32 = 100;

/// Search Gmail for `gmail_q` in one account: list up to
/// `SERVER_SEARCH_LIST` matches, store the unknown ones (up to
/// `fetch_cap`) headers-only, and return every listed message we now have
/// as a hit, newest first. Matches beyond the cap are left for a later search.
pub async fn search_server<A: WindowApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    gmail_q: &str,
    fetch_cap: usize,
) -> Result<ServerSearch> {
    let page = api.search_ids(gmail_q, SERVER_SEARCH_LIST).await?;
    let ids: Vec<String> = page.ids.iter().map(|(m, _)| m.clone()).collect();
    let (acct, probe) = (account_id.to_string(), ids.clone());
    let known: HashSet<String> = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || store.known_message_ids(&acct, &probe))
            .await
            .map_err(|e| Error::Other(format!("store task failed: {e}")))??
    };
    let unknown: Vec<String> = ids
        .iter()
        .filter(|id| !known.contains(*id))
        .take(fetch_cap)
        .cloned()
        .collect();
    let mut fetched = 0u32;
    if !unknown.is_empty() {
        let mut messages = Vec::new();
        for (id, r) in api.headers_now(&unknown).await {
            match r {
                Ok(Some(m)) => messages.push(m),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(account = %account_id, message = %id, error = %e, "server search: header fetch failed")
                }
            }
        }
        fetched = messages.len() as u32;
        let store2 = store.clone();
        tokio::task::spawn_blocking(move || store2.insert_header_messages(&messages))
            .await
            .map_err(|e| Error::Other(format!("store task failed: {e}")))??;
    }
    let (acct, want, store2) = (account_id.to_string(), ids, store.clone());
    let messages = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        for id in &want {
            if let Some(m) = store2.get_message(&acct, id)? {
                out.push(m);
            }
        }
        Ok::<_, penguin_core::Error>(out)
    })
    .await
    .map_err(|e| Error::Other(format!("store task failed: {e}")))??;
    // One hit per thread (newest message), like local search.
    let mut seen = HashSet::new();
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut sorted = messages;
    sorted.sort_by_key(|m| std::cmp::Reverse(m.date));
    for m in sorted {
        if !seen.insert(m.thread_id.clone()) {
            if let Some(h) = hits.iter_mut().find(|h| h.thread_id == m.thread_id) {
                h.match_count += 1;
            }
            continue;
        }
        hits.push(SearchHit {
            account_id: m.account_id.clone(),
            thread_id: m.thread_id.clone(),
            message_id: m.id.clone(),
            subject: m.subject.clone(),
            from: m.from.clone(),
            date: m.date,
            snippet_html: escape_html(&m.snippet),
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
        estimate: page.result_size_estimate,
    })
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Gmail's estimate of messages in the last `months` (0 = the mailbox),
/// cached per account and window for `ESTIMATE_TTL` (one list call, ~1 unit).
pub async fn window_estimate<A: WindowApi>(
    api: &A,
    account_id: &str,
    months: u32,
    now_ms: i64,
) -> Result<u64> {
    const ESTIMATE_TTL: Duration = Duration::from_secs(30 * 60);
    type Cache = Mutex<std::collections::HashMap<(String, u32), (u64, Instant)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (account_id.to_string(), months);
    if let Some((n, at)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        if at.elapsed() < ESTIMATE_TTL {
            return Ok(*n);
        }
    }
    let q = range_query(Some(window_start_ms(now_ms, months)), None).unwrap_or_default();
    let n = api.search_ids(&q, 1).await?.result_size_estimate;
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, (n, Instant::now()));
    Ok(n)
}

#[cfg(test)]
#[path = "sync_window_tests.rs"]
mod tests;
