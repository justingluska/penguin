//! Gmail REST v1 client. OWNER: gmail-sync agent.
//!
//! - Uses `format=full` (bodies for text parts, attachmentId for attachments —
//!   never download attachment bytes during sync).
//! - Converts payloads to `penguin_core::Message` in `convert.rs` (decode
//!   base64url, pick text/plain + text/html, collect attachments/inline cids,
//!   decode headers, fall back to `penguin_core::text::html_to_text`).
//! - Quota: Gmail's per-user "Total Query Cost" limit, 6,000 units/min per
//!   mailbox (override: `PENGUIN_GMAIL_UNITS_PER_MIN`; a raise to 60,000 is
//!   requested). Each account has its own budget by default
//!   (`PENGUIN_GMAIL_QUOTA_SCOPE=shared` pools them with fair turns). Real
//!   costs differ from the docs: messages.get(format=full) ≈60 units
//!   (12× its 5 documented units), list/history/labels/profile/modify ≈1.
//!   messages.get's cost is learned: re-derived from the last minute's
//!   traffic on a quota 403 when the window holds ≥20 gets, probed cheaper
//!   while clean. A throttle closes a pause gate (Retry-After, else 10 s,
//!   doubling to 60 s when it re-trips right after a pause). User actions
//!   skip the queue but their cost is debited. [`quota_stats`] reports it.
//! - Retries: exponential backoff with jitter on 5xx and transport errors;
//!   throttled calls retry through the shared gate instead of their own
//!   timers, so in-flight requests don't stampede. Non-idempotent sends only
//!   retry when Google provably rejected the request (429/403-rate, connect
//!   failure) so a message is never sent twice. Errors surface instead of
//!   silently dropping mail.
//!
//! Why parallel GETs rather than batch HTTP: batch requests are billed per
//! inner call, so batching can't raise the quota ceiling, and Google
//! throttles batches with per-part 429s that then need individual retry
//! bookkeeping. Over HTTP/2, a handful of concurrent GETs already saturates
//! the ~19 msgs/s quota, and each message retries independently.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::future::{join_all, try_join_all};
use penguin_core::{AttachmentMeta, Label, Message};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio::time::Instant;

use base64::Engine;

use crate::auth::AuthManager;
use crate::convert::{self, GmailMessage, B64URL, PART_ID_PREFIX};
use crate::{Error, Result};

const BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

#[derive(Debug, Clone)]
pub struct Profile {
    pub email: String,
    pub messages_total: u64,
    pub history_id: u64,
}

#[derive(Debug, Clone)]
pub struct IdPage {
    /// (message id, thread id), newest first.
    pub ids: Vec<(String, String)>,
    pub next_page_token: Option<String>,
    pub result_size_estimate: u64,
}

/// One page of history, collapsed to net effects. Label deltas are applied
/// in history order per message, so a message never appears in both
/// `labels_added` and `labels_removed` for the same label.
#[derive(Debug, Clone, Default)]
pub struct HistoryPage {
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    /// (message id, label ids added)
    pub labels_added: Vec<(String, Vec<String>)>,
    /// (message id, label ids removed)
    pub labels_removed: Vec<(String, Vec<String>)>,
    pub next_page_token: Option<String>,
    pub history_id: u64,
    /// message id → thread id for every message mentioned on this page.
    pub message_threads: HashMap<String, String>,
}

/// A message's current labels (from `format=minimal`), used to reconcile.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageLabels {
    pub id: String,
    pub thread_id: String,
    pub label_ids: Vec<String>,
}

// ---------- quota ----------

/// Gmail's binding limit here: quota metric "Total Query Cost", limit "Units
/// per minute per user" = 6,000 (Service Usage API; a 403
/// `rateLimitExceeded` names it when exceeded). The often-quoted 250
/// units/user/s is NOT what's enforced. Projects with a raised quota can
/// override via `PENGUIN_GMAIL_UNITS_PER_MIN`.
const DEFAULT_UNITS_PER_MIN: f64 = 6_000.0;
const UNITS_PER_MIN_ENV: &str = "PENGUIN_GMAIL_UNITS_PER_MIN";
/// The budget is per mailbox: Cloud Monitoring (2026-09-24) showed the
/// project spending ~31.8k units/min across five accounts, far above 6,000,
/// with each account throttled on its own. So by default each account gets
/// its own budget. `shared` puts every account on one bucket with fair
/// turns, for a project where that turns out to be how Google counts.
const QUOTA_SCOPE_ENV: &str = "PENGUIN_GMAIL_QUOTA_SCOPE";
/// Steady pacing as a fraction of the per-minute limit.
const TARGET_UTILIZATION: f64 = 0.95;
/// Bucket capacity as a fraction of the per-minute limit. Capacity + 60 s of
/// refill = 97% of the limit, so no rolling 60 s window can exceed it even
/// with request-arrival jitter.
const BURST_FRACTION: f64 = 0.02;

/// messages.get's real cost is NOT the documented 5 units. Cloud Monitoring
/// (2026-09-24): each HTTP messages.get(format=full) is 5 "default" units
/// and a constant 12× that in Total Query Cost, i.e. ~60 units, not
/// size-dependent; list/history/labels/profile/modify cost ≈1. At 6,000
/// units/min that's ~100 gets/min per account. The limiter still learns
/// the cost (quota increases, other formats); this is the starting estimate.
const INITIAL_GET_COST: f64 = 60.0;
const MIN_GET_COST: f64 = 3.0;
const MAX_GET_COST: f64 = 300.0;
/// On a quota 403 the estimate becomes what the last minute's traffic
/// implies (budget ÷ gets sent), plus this margin...
const LEARN_MARGIN: f64 = 0.05;
/// ...but only when the window holds enough gets to say anything. A 403
/// with few of our gets in the window means the budget was spent elsewhere
/// (another process, the previous minute of a restarted app): keep the
/// estimate and just pause.
const MIN_GETS_TO_LEARN: usize = 20;
/// Probing a cheaper estimate (a faster request rate) by `PROBE_STEP`: the
/// first probe waits `PROBE_AFTER` after a throttle (an estimate learned from
/// a full window is accurate, and an overshoot costs a pause); each clean
/// probe halves the wait down to `MIN_PROBE_INTERVAL`, so a genuinely cheaper
/// mailbox is found within minutes.
const PROBE_AFTER: Duration = Duration::from_secs(120);
const MIN_PROBE_INTERVAL: Duration = Duration::from_secs(20);
const PROBE_STEP: f64 = 0.97;
/// Trailing window for "what did we send", matching Google's per-minute limit.
const QUOTA_WINDOW: Duration = Duration::from_secs(60);

/// Background pause after a per-minute quota throttle without Retry-After:
/// `FIRST_PAUSE`, doubling (up to a full window) each time the limit trips
/// again within `STREAK_WINDOW` of the previous pause ending, i.e. when the
/// rolling minute evidently hadn't drained. A fixed short pause re-trips
/// every few seconds; a fixed long one wastes budget after an isolated
/// overshoot.
const FIRST_PAUSE: Duration = Duration::from_secs(10);
const MAX_PAUSE: Duration = Duration::from_secs(60);
const STREAK_WINDOW: Duration = Duration::from_secs(30);
/// An account more than this many get-costs ahead of the least-served
/// waiting account yields its turn.
const FAIR_SLACK_GETS: f64 = 2.0;
/// Concurrent in-flight background requests per account. Gmail also enforces
/// a per-user concurrency limit ("Too many concurrent requests for user").
const MAX_IN_FLIGHT: usize = 12;
/// Attempts for 5xx/transport failures (exponential backoff between them).
const MAX_ATTEMPTS: u32 = 8;
/// Throttled retries before giving up. They wait on the shared pause gate,
/// not their own timers, so this only bounds a quota that never recovers
/// (e.g. the daily project quota is gone).
const MAX_THROTTLED_RETRIES: u32 = 40;

/// `PENGUIN_GMAIL_UNITS_PER_MIN`, if set to a positive number (it wins over
/// the in-app setting).
fn env_units_per_min() -> Option<f64> {
    static UNITS: OnceLock<Option<f64>> = OnceLock::new();
    *UNITS.get_or_init(|| {
        match std::env::var(UNITS_PER_MIN_ENV)
            .ok()
            .map(|v| v.trim().parse::<f64>())
        {
            Some(Ok(v)) if v.is_finite() && v > 0.0 => Some(v),
            Some(_) => {
                tracing::warn!("{UNITS_PER_MIN_ENV} is not a positive number; ignoring it");
                None
            }
            None => None,
        }
    })
}

/// The in-app setting (`set_units_per_min`); DEFAULT_UNITS_PER_MIN until set.
fn setting_units_per_min() -> &'static Mutex<f64> {
    static SETTING: OnceLock<Mutex<f64>> = OnceLock::new();
    SETTING.get_or_init(|| Mutex::new(DEFAULT_UNITS_PER_MIN))
}

fn configured_units_per_min() -> f64 {
    env_units_per_min().unwrap_or_else(|| {
        *setting_units_per_min()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    })
}

/// Set the per-account Gmail budget (units/min) live, e.g. from Settings
/// after a quota raise. Every existing account's limiter is resized in
/// place (its learned messages.get cost is kept); new accounts use it too.
/// A no-op (logged) while `PENGUIN_GMAIL_UNITS_PER_MIN` is set, which wins.
pub fn set_units_per_min(units: u32) {
    if units == 0 {
        tracing::warn!("ignoring a Gmail quota setting of 0 units/min");
        return;
    }
    if let Some(env) = env_units_per_min() {
        tracing::info!(
            setting = units,
            env,
            "{UNITS_PER_MIN_ENV} is set and overrides the quota setting"
        );
        return;
    }
    *setting_units_per_min()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = units as f64;
    apply_units_per_min(units as f64);
}

fn apply_units_per_min(units: f64) {
    let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    let mut seen: Vec<*const Budget> = Vec::new();
    for q in reg.accounts.values() {
        let ptr = Arc::as_ptr(&q.budget);
        if !seen.contains(&ptr) {
            seen.push(ptr);
            q.budget.set_units_per_min(units);
        }
    }
    if let Some(shared) = &reg.shared {
        if !seen.contains(&Arc::as_ptr(shared)) {
            shared.set_units_per_min(units);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum QuotaScope {
    Shared,
    PerAccount,
}

fn configured_scope() -> QuotaScope {
    static SCOPE: OnceLock<QuotaScope> = OnceLock::new();
    *SCOPE.get_or_init(|| {
        match std::env::var(QUOTA_SCOPE_ENV)
            .ok()
            .as_deref()
            .map(str::trim)
        {
            None | Some("") | Some("per-account") => QuotaScope::PerAccount,
            Some("shared") => QuotaScope::Shared,
            Some(other) => {
                tracing::warn!(
                    "{QUOTA_SCOPE_ENV}={other} is not per-account|shared; using per-account"
                );
                QuotaScope::PerAccount
            }
        }
    })
}

/// What a call is charged against the budget.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Cost {
    /// messages.get / attachments.get: learned at runtime (see INITIAL_GET_COST).
    Get,
    /// Calls whose observed cost is stable.
    Fixed(f64),
}

/// Observed ≈1 unit (Cloud Monitoring): list, history, labels, profile, modify.
pub(crate) const CHEAP: Cost = Cost::Fixed(1.0);
/// Measured (probe, 2026-09-24; Total Query Cost is 12× the "default"
/// units for full/raw bodies and 4× for everything lighter):
/// messages.get format=metadata|minimal and attachments.get = 20,
/// history.list = 2, messages.list = 5. format=full|raw (`Cost::Get`) is
/// 60 and learned; a `fields=` mask doesn't change any of these.
pub(crate) const LIGHT_GET: Cost = Cost::Fixed(20.0);
const HISTORY_LIST: Cost = Cost::Fixed(2.0);
const MESSAGES_LIST: Cost = Cost::Fixed(5.0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    /// Sync traffic: waits for its fair turn, tokens and a concurrency permit.
    Background,
    /// User-initiated (modify, send, attachment): goes straight out and
    /// borrows from the bucket, so background sync slows to compensate.
    Interactive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Retry {
    /// Safe to repeat on any transient failure.
    Idempotent,
    /// Repeat only if Google certainly didn't act on it (429/403-rate,
    /// connect failure). Used for messages.send.
    OnlyIfRejected,
}

/// Per-account bookkeeping inside a budget.
#[derive(Default)]
struct Share {
    /// Background units granted (fair-queuing virtual time).
    served: f64,
    /// Background requests currently waiting for a turn.
    waiting: u32,
    /// Gets granted in the trailing window.
    gets: VecDeque<Instant>,
}

struct BudgetState {
    /// Budget paced against (units/min); changeable at runtime.
    units_per_min: f64,
    /// Units/s the bucket refills at.
    rate: f64,
    capacity: f64,
    /// May go negative: interactive calls borrow, background repays.
    tokens: f64,
    last: Instant,
    /// Shared gate: background (and throttled retries) wait until then.
    paused_until: Option<Instant>,
    /// Learned units per messages.get.
    get_cost: f64,
    /// Last throttle or probe step.
    last_change: Instant,
    /// Wait before the next probe step.
    probe_interval: Duration,
    /// Everything sent in the trailing `QUOTA_WINDOW`: (when, gets, other units).
    sent: VecDeque<(Instant, u32, f64)>,
    throttle_episodes: u64,
    /// Consecutive episodes (each within STREAK_WINDOW of the last pause's end).
    streak: u32,
    shares: HashMap<String, Share>,
}

impl BudgetState {
    fn trim(&mut self, now: Instant) {
        while self
            .sent
            .front()
            .is_some_and(|(t, _, _)| now.duration_since(*t) > QUOTA_WINDOW)
        {
            self.sent.pop_front();
        }
        for share in self.shares.values_mut() {
            while share
                .gets
                .front()
                .is_some_and(|t| now.duration_since(*t) > QUOTA_WINDOW)
            {
                share.gets.pop_front();
            }
        }
    }
    /// (gets, other units) in the trailing window.
    fn window(&self) -> (usize, f64) {
        self.sent.iter().fold((0, 0.0), |(g, u), (_, gets, units)| {
            (g + *gets as usize, u + units)
        })
    }
    /// Least `served` among accounts with waiting background requests.
    fn min_waiting_served(&self) -> Option<f64> {
        self.shares
            .values()
            .filter(|s| s.waiting > 0)
            .map(|s| s.served)
            .min_by(f64::total_cmp)
    }
}

/// One per-minute budget (one Cloud project's quota "user", or one account
/// in per-account mode): a token bucket whose capacity plus a minute of
/// refill stays under the limit, a learned messages.get cost, a shared
/// pause gate after throttles, and fair turns across accounts.
struct Budget {
    state: Mutex<BudgetState>,
}

/// (refill units/s, burst capacity) for a per-minute budget.
fn pacing(units_per_min: f64) -> (f64, f64) {
    (
        units_per_min * TARGET_UTILIZATION / 60.0,
        (units_per_min * BURST_FRACTION).max(5.0),
    )
}

impl Budget {
    fn new(units_per_min: f64) -> Self {
        let (rate, capacity) = pacing(units_per_min);
        let now = Instant::now();
        Budget {
            state: Mutex::new(BudgetState {
                units_per_min,
                rate,
                capacity,
                tokens: capacity,
                last: now,
                paused_until: None,
                get_cost: INITIAL_GET_COST,
                last_change: now,
                probe_interval: PROBE_AFTER,
                sent: VecDeque::new(),
                throttle_episodes: 0,
                streak: 0,
                shares: HashMap::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BudgetState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Resize the budget in place, keeping the learned cost and the window.
    fn set_units_per_min(&self, units_per_min: f64) {
        let (rate, capacity) = pacing(units_per_min);
        let mut st = self.lock();
        st.units_per_min = units_per_min;
        st.rate = rate;
        st.capacity = capacity;
        st.tokens = st.tokens.min(capacity);
    }

    /// Wait for permission to spend `cost` for `account`. Background
    /// requests take turns: an account that has been served more than the
    /// least-served waiting account yields, so every syncing account
    /// progresses at an equal share. Interactive calls go immediately and
    /// borrow, unless they're retrying a throttle, in which case they respect
    /// the pause gate too.
    async fn acquire(
        self: &Arc<Self>,
        account: &str,
        cost: Cost,
        priority: Priority,
        after_throttle: bool,
    ) {
        let _turn = (priority == Priority::Background).then(|| WaitTurn::register(self, account));
        loop {
            let wait = {
                let mut st = self.lock();
                let now = Instant::now();
                if now.duration_since(st.last_change) >= st.probe_interval
                    && st.get_cost > MIN_GET_COST
                {
                    st.get_cost = (st.get_cost * PROBE_STEP).max(MIN_GET_COST);
                    st.last_change = now;
                    st.probe_interval = (st.probe_interval / 2).max(MIN_PROBE_INTERVAL);
                }
                let units = match cost {
                    Cost::Get => st.get_cost,
                    Cost::Fixed(u) => u,
                };
                // The bucket must be able to hold one whole request, or a
                // learned cost above the burst size would wait forever.
                let cap = st.capacity.max(units);
                let dt = now.duration_since(st.last).as_secs_f64();
                st.tokens = (st.tokens + dt * st.rate).min(cap);
                st.last = now;
                let gated = priority == Priority::Background || after_throttle;
                let my_turn = priority == Priority::Interactive
                    || st.min_waiting_served().is_none_or(|min| {
                        st.shares.get(account).map_or(0.0, |s| s.served)
                            <= min + FAIR_SLACK_GETS * st.get_cost
                    });
                match st.paused_until {
                    Some(until) if until > now && gated => until - now,
                    _ if my_turn && (priority == Priority::Interactive || st.tokens >= units) => {
                        st.tokens -= units;
                        st.trim(now);
                        match cost {
                            Cost::Get => st.sent.push_back((now, 1, 0.0)),
                            Cost::Fixed(u) => st.sent.push_back((now, 0, u)),
                        }
                        let share = st.shares.entry(account.to_string()).or_default();
                        if priority == Priority::Background {
                            share.served += units;
                        }
                        if cost == Cost::Get {
                            share.gets.push_back(now);
                        }
                        return;
                    }
                    // Not our turn: the least-served account takes the next
                    // tokens; check back shortly.
                    _ if !my_turn => Duration::from_secs_f64((units / st.rate).max(0.005)),
                    _ => Duration::from_secs_f64((units - st.tokens) / st.rate),
                }
            };
            // The timer has millisecond resolution: a sub-ms sleep can return
            // without the clock moving, which would spin this loop.
            tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
        }
    }

    /// Google said the per-minute quota is spent. Close the shared gate until
    /// the window plausibly drains (Retry-After, else a pause that grows with
    /// consecutive episodes) and, once per episode, re-learn the messages.get
    /// cost if the window holds enough of our gets to say anything: Google
    /// counted ~the full budget, so each get cost (budget − other) ÷ gets.
    /// In-flight requests that also get 403 in the same episode only extend
    /// the gate.
    fn on_throttle(&self, retry_after: Option<Duration>) {
        let mut st = self.lock();
        let now = Instant::now();
        if st.paused_until.is_some_and(|p| p > now) {
            if let Some(ra) = retry_after {
                let until = now + ra;
                st.paused_until = st.paused_until.map(|p| p.max(until));
            }
            return;
        }
        st.trim(now);
        let (gets, other) = st.window();
        if gets >= MIN_GETS_TO_LEARN {
            let implied = (st.units_per_min - other).max(0.0) / gets as f64;
            if implied > st.get_cost {
                st.get_cost = (implied * (1.0 + LEARN_MARGIN)).clamp(MIN_GET_COST, MAX_GET_COST);
            }
        }
        st.streak = match st.paused_until {
            Some(end) if now.duration_since(end) < STREAK_WINDOW => st.streak + 1,
            _ => 0,
        };
        st.last_change = now;
        st.probe_interval = PROBE_AFTER;
        st.throttle_episodes += 1;
        st.tokens = st.tokens.min(0.0);
        let pause = retry_after
            .unwrap_or_else(|| (FIRST_PAUSE * 2u32.pow(st.streak.min(4))).min(MAX_PAUSE));
        st.paused_until = Some(now + pause);
        tracing::debug!(
            get_cost = st.get_cost,
            gets,
            other,
            ?pause,
            streak = st.streak,
            "gmail quota throttle"
        );
    }
}

/// Marks a background request as waiting for its turn; undone on drop (also
/// when the request future is cancelled). An account that starts waiting
/// after being idle joins at the current least-served level, so idle time
/// doesn't bank a burst that would starve the others.
struct WaitTurn {
    budget: Arc<Budget>,
    account: String,
}

impl WaitTurn {
    fn register(budget: &Arc<Budget>, account: &str) -> Self {
        let mut st = budget.lock();
        let floor = st.min_waiting_served();
        let share = st.shares.entry(account.to_string()).or_default();
        if share.waiting == 0 {
            if let Some(min) = floor {
                share.served = share.served.max(min);
            }
        }
        share.waiting += 1;
        WaitTurn {
            budget: budget.clone(),
            account: account.to_string(),
        }
    }
}

impl Drop for WaitTurn {
    fn drop(&mut self) {
        let mut st = self.budget.lock();
        if let Some(share) = st.shares.get_mut(&self.account) {
            share.waiting = share.waiting.saturating_sub(1);
        }
    }
}

/// Learned pacing state for an account's budget, for diagnostics.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaStats {
    /// "perAccount" (default: one budget per mailbox) or "shared".
    pub scope: String,
    /// Budget the limiter paces against (units/min).
    pub units_per_min: f64,
    /// Current learned cost of one messages.get (units).
    pub get_cost: f64,
    /// Sustainable messages.get rate for the whole budget (per second).
    pub gets_per_sec: f64,
    /// messages.get calls sent against the budget in the last minute.
    pub gets_last_min: u32,
    /// Estimated units spent against the budget in the last minute.
    pub units_last_min: f64,
    /// Quota throttle episodes since launch (whole budget).
    pub throttle_episodes: u64,
    /// This account's messages.get calls in the last minute.
    pub account_gets_last_min: u32,
    /// Accounts that used the budget in the last minute.
    pub active_accounts: u32,
    /// This account's fair share of `gets_per_sec`.
    pub account_gets_per_sec: f64,
}

/// An account's handle on its budget, plus its own concurrency cap.
pub(crate) struct Quota {
    account: String,
    budget: Arc<Budget>,
    in_flight: Semaphore,
}

struct Registry {
    shared: Option<Arc<Budget>>,
    accounts: HashMap<String, Arc<Quota>>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            shared: None,
            accounts: HashMap::new(),
        })
    })
}

/// Learned quota state for an account (None until it has made a call).
pub fn quota_stats(account_email: &str) -> Option<QuotaStats> {
    let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    reg.accounts
        .get(&account_email.to_ascii_lowercase())
        .map(|q| q.stats())
}

impl Quota {
    fn new(account: &str, budget: Arc<Budget>) -> Self {
        Quota {
            account: account.to_ascii_lowercase(),
            budget,
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
        }
    }

    /// One quota handle per account for the whole process; the budget is
    /// shared or per-account per `PENGUIN_GMAIL_QUOTA_SCOPE`.
    fn for_account(email: &str) -> Arc<Quota> {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        let key = email.to_ascii_lowercase();
        if let Some(q) = reg.accounts.get(&key) {
            return q.clone();
        }
        let budget = match configured_scope() {
            QuotaScope::Shared => reg
                .shared
                .get_or_insert_with(|| Arc::new(Budget::new(configured_units_per_min())))
                .clone(),
            QuotaScope::PerAccount => Arc::new(Budget::new(configured_units_per_min())),
        };
        let q = Arc::new(Quota::new(&key, budget));
        reg.accounts.insert(key, q.clone());
        q
    }

    async fn acquire(&self, cost: Cost, priority: Priority, after_throttle: bool) {
        self.budget
            .acquire(&self.account, cost, priority, after_throttle)
            .await
    }

    fn on_throttle(&self, retry_after: Option<Duration>) {
        self.budget.on_throttle(retry_after)
    }

    fn stats(&self) -> QuotaStats {
        let mut st = self.budget.lock();
        st.trim(Instant::now());
        let (gets, other) = st.window();
        let active = st
            .shares
            .values()
            .filter(|s| !s.gets.is_empty() || s.waiting > 0)
            .count()
            .max(1);
        let gets_per_sec = st.rate / st.get_cost;
        QuotaStats {
            scope: if configured_scope() == QuotaScope::Shared {
                "shared"
            } else {
                "perAccount"
            }
            .into(),
            units_per_min: st.units_per_min,
            get_cost: st.get_cost,
            gets_per_sec,
            gets_last_min: gets as u32,
            units_last_min: gets as f64 * st.get_cost + other,
            throttle_episodes: st.throttle_episodes,
            account_gets_last_min: st
                .shares
                .get(&self.account)
                .map_or(0, |s| s.gets.len() as u32),
            active_accounts: active as u32,
            account_gets_per_sec: gets_per_sec / active as f64,
        }
    }
}

fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let exp = Duration::from_millis(500u64.saturating_mul(1 << attempt.min(7)))
        .min(Duration::from_secs(64));
    let jitter = Duration::from_millis(fastrand::u64(0..1000));
    retry_after.map_or(exp, |ra| ra.max(Duration::from_millis(250))) + jitter
}

/// Guard against credential mix-ups (one account's requests carrying another
/// account's token would silently merge their quota buckets). Every request
/// checks that its token hasn't been seen for a different account; the first
/// time a debug build sees a token it also asks Google whose it is. Only a
/// hash of the token is kept, never the token.
struct TokenOwners {
    /// token hash → account it was used for
    seen: HashMap<u64, String>,
}

fn token_owners() -> &'static Mutex<TokenOwners> {
    static OWNERS: OnceLock<Mutex<TokenOwners>> = OnceLock::new();
    OWNERS.get_or_init(|| {
        Mutex::new(TokenOwners {
            seen: HashMap::new(),
        })
    })
}

fn token_hash(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

/// Record `token` as used by `account`. Returns Err(previous owner) if the
/// same token was already used for a different account, Ok(true) if it's new.
fn claim_token(token: &str, account: &str) -> std::result::Result<bool, String> {
    let mut owners = token_owners().lock().unwrap_or_else(|e| e.into_inner());
    // Bounded: access tokens rotate hourly; forget old ones occasionally.
    if owners.seen.len() > 1_000 {
        owners.seen.clear();
    }
    let key = token_hash(token);
    match owners.seen.get(&key) {
        Some(owner) if owner != account => Err(owner.clone()),
        Some(_) => Ok(false),
        None => {
            owners.seen.insert(key, account.to_string());
            Ok(true)
        }
    }
}

fn http() -> reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::user_agent())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(90))
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_keep_alive_interval(Duration::from_secs(30))
            .build()
            .expect("reqwest client")
    })
    .clone()
}

enum Outcome {
    Done(Vec<u8>),
    Retry(Error, Option<Duration>),
    /// Per-user quota spent: wait on the shared pause gate, then retry.
    Throttled,
    /// 401 on a cached token: invalidate it and retry once.
    RetryNow,
}

fn is_rate_limit_body(body: &str) -> bool {
    body.contains("ateLimitExceeded") || body.contains("Too many concurrent requests")
}

fn truncate(body: &str) -> String {
    let mut end = body.len().min(500);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_string()
}

// ---------- wire types ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileWire {
    email_address: String,
    #[serde(default)]
    messages_total: u64,
    history_id: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ListWire {
    messages: Vec<IdWire>,
    next_page_token: Option<String>,
    result_size_estimate: u64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct IdWire {
    pub(crate) id: String,
    pub(crate) thread_id: String,
    pub(crate) label_ids: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct HistoryWire {
    history: Vec<HistoryRecord>,
    next_page_token: Option<String>,
    history_id: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct HistoryRecord {
    messages_added: Vec<HistoryMsg>,
    messages_deleted: Vec<HistoryMsg>,
    labels_added: Vec<HistoryLabels>,
    labels_removed: Vec<HistoryLabels>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HistoryMsg {
    message: IdWire,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct HistoryLabels {
    message: IdWire,
    label_ids: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LabelsWire {
    labels: Vec<LabelWire>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LabelWire {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    color: Option<LabelColorWire>,
    messages_unread: Option<u32>,
    messages_total: Option<u64>,
    label_list_visibility: Option<String>,
}

fn label_from_wire(account: &str, l: LabelWire) -> Label {
    Label {
        account_id: account.to_string(),
        kind: match l.kind.as_deref() {
            Some("system") => "system".into(),
            _ => "user".into(),
        },
        id: l.id,
        name: l.name,
        color: l
            .color
            .and_then(|c| c.background_color)
            .filter(|c| !c.is_empty()),
        unread_count: l.messages_unread,
        hidden: l.label_list_visibility.as_deref() == Some("labelHide"),
    }
}

/// labels.patch body: only the given fields. `color: Some(None)` sends
/// `"color": null`, which clears it (Google's patch semantics: a field set
/// to null is deleted). `hidden: Some(false)` shows the label always
/// ("labelShow"), so a prior "labelShowIfUnread" isn't restored.
fn label_patch_body(
    name: Option<&str>,
    color: Option<Option<(&str, &str)>>,
    hidden: Option<bool>,
) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(name) = name {
        body.insert("name".into(), name.into());
    }
    match color {
        None => {}
        Some(None) => {
            body.insert("color".into(), serde_json::Value::Null);
        }
        Some(Some((bg, text))) => {
            body.insert(
                "color".into(),
                serde_json::json!({ "backgroundColor": bg, "textColor": text }),
            );
        }
    }
    if let Some(hidden) = hidden {
        let v = if hidden { "labelHide" } else { "labelShow" };
        body.insert("labelListVisibility".into(), v.into());
    }
    serde_json::Value::Object(body)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LabelColorWire {
    background_color: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AttachmentWire {
    data: String,
}

// ---------- client ----------

#[derive(Clone)]
pub struct GmailClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    auth: AuthManager,
    email: String,
    quota: Arc<Quota>,
}

impl GmailClient {
    pub fn new(auth: AuthManager, account_email: &str) -> Self {
        GmailClient {
            inner: Arc::new(Inner {
                http: http(),
                auth,
                email: account_email.to_string(),
                quota: Quota::for_account(account_email),
            }),
        }
    }

    /// Issue one API call with quota accounting and retries. Returns the
    /// body bytes of a 2xx response.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        json: Option<&serde_json::Value>,
        cost: Cost,
        priority: Priority,
        retry: Retry,
    ) -> Result<Vec<u8>> {
        let inner = &self.inner;
        let url = format!("{BASE}{path}");
        let mut attempt = 0u32;
        let mut retried_401 = false;
        let mut throttled = 0u32;
        loop {
            let permit = match priority {
                Priority::Background => Some(
                    inner
                        .quota
                        .in_flight
                        .acquire()
                        .await
                        .map_err(|e| Error::Other(e.to_string()))?,
                ),
                Priority::Interactive => None,
            };
            inner.quota.acquire(cost, priority, throttled > 0).await;
            let token = inner.auth.access_token(&inner.email).await?;
            self.check_token_subject(&token).await;

            let mut req = inner
                .http
                .request(method.clone(), &url)
                .bearer_auth(token)
                .query(query);
            if let Some(body) = json {
                req = req.json(body);
            }
            let outcome = match req.send().await {
                Err(e) => {
                    let not_sent = e.is_connect();
                    let err = Error::Network(e.without_url().to_string());
                    // A connect failure means nothing reached Google.
                    if retry == Retry::OnlyIfRejected && !not_sent {
                        return Err(err);
                    }
                    Outcome::Retry(err, None)
                }
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(Duration::from_secs);
                    match resp.bytes().await {
                        Ok(body) => {
                            self.classify(status, retry_after, &body, retry, &mut retried_401)?
                        }
                        Err(e) if retry == Retry::Idempotent => {
                            Outcome::Retry(Error::Network(e.without_url().to_string()), None)
                        }
                        // The server answered, so a send may have happened.
                        Err(e) => return Err(Error::Network(e.without_url().to_string())),
                    }
                }
            };
            drop(permit);
            match outcome {
                Outcome::Done(body) => return Ok(body),
                Outcome::RetryNow => {
                    // Drop the cached token so the retry refreshes; a revoked
                    // grant then surfaces as NeedsReauth right away.
                    inner.auth.invalidate_access_token(&inner.email).await;
                    continue;
                }
                Outcome::Throttled => {
                    // No private timer: the next acquire() waits on the
                    // account's shared gate, so retries don't stampede.
                    throttled += 1;
                    if throttled >= MAX_THROTTLED_RETRIES {
                        return Err(Error::RateLimited);
                    }
                }
                Outcome::Retry(err, retry_after) => {
                    attempt += 1;
                    if attempt >= MAX_ATTEMPTS {
                        return Err(err);
                    }
                    let wait = backoff(attempt, retry_after);
                    tracing::debug!(account = %inner.email, attempt, ?wait, error = %err, "gmail retry");
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }

    fn classify(
        &self,
        status: StatusCode,
        retry_after: Option<Duration>,
        body: &[u8],
        retry: Retry,
        retried_401: &mut bool,
    ) -> Result<Outcome> {
        if status.is_success() {
            return Ok(Outcome::Done(body.to_vec()));
        }
        let text = String::from_utf8_lossy(body);
        let http_err = || Error::Http {
            status: status.as_u16(),
            body: truncate(&text),
        };
        let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN && is_rate_limit_body(&text));
        if rate_limited {
            // Google's error body names which limit tripped (per-user rate,
            // concurrent requests, project quota); keep it for diagnosis.
            tracing::debug!(account = %self.inner.email, status = status.as_u16(), body = %truncate(&text), "gmail throttled");
            self.inner.quota.on_throttle(retry_after);
            Ok(Outcome::Throttled)
        } else if status == StatusCode::UNAUTHORIZED && !*retried_401 {
            // The cached access token was rejected (expired in flight or
            // revoked); retry once with a freshly refreshed token.
            *retried_401 = true;
            Ok(Outcome::RetryNow)
        } else if status.is_server_error() && retry == Retry::Idempotent {
            Ok(Outcome::Retry(http_err(), retry_after))
        } else {
            Err(http_err())
        }
    }

    async fn check_token_subject(&self, token: &str) {
        let account = &self.inner.email;
        match claim_token(token, account) {
            Err(other) => {
                tracing::error!(account = %account, other_account = %other, "credential mix-up: this access token was already used for another account");
                debug_assert!(false, "access token for {other} used for {account}");
            }
            Ok(true) if cfg!(debug_assertions) => {
                #[derive(Deserialize)]
                struct TokenInfo {
                    email: Option<String>,
                }
                let info = self
                    .inner
                    .http
                    .get("https://oauth2.googleapis.com/tokeninfo")
                    .query(&[("access_token", token)])
                    .send()
                    .await;
                let email = match info {
                    Ok(r) => r.json::<TokenInfo>().await.ok().and_then(|t| t.email),
                    Err(_) => None,
                };
                match email {
                    Some(e) if !e.eq_ignore_ascii_case(account) => {
                        tracing::error!(account = %account, token_subject = %e, "credential mix-up: token belongs to another account");
                        debug_assert!(false, "token for {e} used for {account}");
                    }
                    Some(_) => tracing::debug!(account = %account, "access token subject verified"),
                    None => {
                        tracing::debug!(account = %account, "could not verify access token subject")
                    }
                }
            }
            Ok(_) => {}
        }
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        cost: Cost,
        priority: Priority,
    ) -> Result<T> {
        let bytes = self
            .call(
                Method::GET,
                path,
                query,
                None,
                cost,
                priority,
                Retry::Idempotent,
            )
            .await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("gmail {path}: bad json: {e}")))
    }

    pub async fn profile(&self) -> Result<Profile> {
        let p: ProfileWire = self
            .get_json("/profile", &[], CHEAP, Priority::Background)
            .await?;
        Ok(Profile {
            email: p.email_address,
            messages_total: p.messages_total,
            history_id: p
                .history_id
                .parse()
                .map_err(|_| Error::Other("bad historyId".into()))?,
        })
    }

    /// messages.list, excluding nothing (includeSpamTrash=false) unless `q` says so.
    pub async fn list_message_ids(
        &self,
        page_token: Option<&str>,
        q: Option<&str>,
    ) -> Result<IdPage> {
        let mut query = vec![("maxResults", "500".to_string())];
        if let Some(t) = page_token {
            query.push(("pageToken", t.to_string()));
        }
        if let Some(q) = q {
            query.push(("q", q.to_string()));
        }
        let l: ListWire = self
            .get_json("/messages", &query, MESSAGES_LIST, Priority::Background)
            .await?;
        Ok(IdPage {
            ids: l
                .messages
                .into_iter()
                .map(|m| (m.id, m.thread_id))
                .collect(),
            next_page_token: l.next_page_token.filter(|t| !t.is_empty()),
            result_size_estimate: l.result_size_estimate,
        })
    }

    async fn get_raw_message(
        &self,
        id: &str,
        format: &str,
        priority: Priority,
    ) -> Result<Option<GmailMessage>> {
        let path = format!("/messages/{id}");
        // Bodies (full/raw) are the expensive, learned cost; everything
        // lighter is a measured flat 20.
        let cost = if matches!(format, "full" | "raw") {
            Cost::Get
        } else {
            LIGHT_GET
        };
        match self
            .get_json(&path, &[("format", format.to_string())], cost, priority)
            .await
        {
            Ok(m) => Ok(Some(m)),
            // Deleted between list and get.
            Err(Error::Http { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn get_one(&self, id: &str, priority: Priority) -> Result<Option<Message>> {
        let Some(gm) = self.get_raw_message(id, "full", priority).await? else {
            return Ok(None);
        };
        let mut extra = HashMap::new();
        for att_id in convert::out_of_line_bodies(&gm) {
            let bytes = self
                .fetch_attachment_bytes(&gm.id, &att_id, priority)
                .await?;
            extra.insert(att_id, bytes);
        }
        // Mail is hostile input: a converter bug on one message must park
        // that message, not take down the account's sync.
        let email = &self.inner.email;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            convert::to_message(email, &gm, &extra)
        })) {
            Ok(m) => Ok(Some(m)),
            Err(payload) => {
                let panic = crate::panic_message(&*payload);
                tracing::error!(account = %email, message_id = %gm.id, panic = %panic, "message conversion panicked");
                Err(Error::Other(format!(
                    "could not convert message {}: {panic}",
                    gm.id
                )))
            }
        }
    }

    /// Fetch full messages, concurrency-limited, preserving input order where found.
    /// Messages that no longer exist (404) are skipped; any other failure
    /// fails the whole call so the caller can retry without losing mail.
    pub async fn get_messages(&self, ids: &[String]) -> Result<Vec<Message>> {
        let fetched =
            try_join_all(ids.iter().map(|id| self.get_one(id, Priority::Background))).await?;
        Ok(fetched.into_iter().flatten().collect())
    }

    /// Fetch full messages, reporting each id's outcome separately so one
    /// bad message can't sink its siblings. `Ok(None)` = no longer exists.
    pub async fn get_messages_each(
        &self,
        ids: &[String],
    ) -> Vec<(String, Result<Option<Message>>)> {
        let results = join_all(ids.iter().map(|id| self.get_one(id, Priority::Background))).await;
        ids.iter().cloned().zip(results).collect()
    }

    /// Headers-only fetch (`format=metadata`, all headers, 20 units vs 60 for
    /// full), for indexing older mail cheaply: each result is a Message with
    /// headers, snippet and labels but empty bodies and no attachment list
    /// (see [`convert::to_message`]). Per-id outcomes like
    /// [`GmailClient::get_messages_each`]; `Ok(None)` = gone.
    pub async fn get_messages_metadata_each(
        &self,
        ids: &[String],
        interactive: bool,
    ) -> Vec<(String, Result<Option<Message>>)> {
        let priority = if interactive {
            Priority::Interactive
        } else {
            Priority::Background
        };
        let results = join_all(ids.iter().map(|id| async move {
            let Some(gm) = self.get_raw_message(id, "metadata", priority).await? else {
                return Ok(None);
            };
            let email = &self.inner.email;
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                convert::to_message(email, &gm, &HashMap::new())
            })) {
                Ok(m) => Ok(Some(m)),
                Err(payload) => {
                    let panic = crate::panic_message(&*payload);
                    tracing::error!(account = %email, message_id = %gm.id, panic = %panic, "metadata conversion panicked");
                    Err(Error::Other(format!("could not convert message {}: {panic}", gm.id)))
                }
            }
        }))
        .await;
        ids.iter().cloned().zip(results).collect()
    }

    /// Gmail-side search for the user right now (e.g. "also search Gmail"):
    /// one messages.list page for `q`, newest first, at interactive priority.
    pub async fn search_message_ids(&self, q: &str, max_results: u32) -> Result<IdPage> {
        let query = [
            ("q", q.to_string()),
            ("maxResults", max_results.clamp(1, 500).to_string()),
        ];
        let l: ListWire = self
            .get_json("/messages", &query, MESSAGES_LIST, Priority::Interactive)
            .await?;
        Ok(IdPage {
            ids: l
                .messages
                .into_iter()
                .map(|m| (m.id, m.thread_id))
                .collect(),
            next_page_token: l.next_page_token.filter(|t| !t.is_empty()),
            result_size_estimate: l.result_size_estimate,
        })
    }

    /// One full message for the user right now (e.g. opening a message whose
    /// body wasn't synced yet): skips the background queue.
    pub async fn get_message_interactive(&self, id: &str) -> Result<Option<Message>> {
        self.get_one(id, Priority::Interactive).await
    }

    /// Fetch one message in `format` (optionally with a `fields=` mask) and
    /// return the response size; used only by [`crate::probe`].
    pub async fn probe_fetch(&self, id: &str, format: &str, fields: Option<&str>) -> Result<usize> {
        let mut query = vec![("format", format.to_string())];
        if let Some(f) = fields {
            query.push(("fields", f.to_string()));
        }
        let path = format!("/messages/{id}");
        let bytes = self
            .call(
                Method::GET,
                &path,
                &query,
                None,
                Cost::Get,
                Priority::Background,
                Retry::Idempotent,
            )
            .await?;
        Ok(bytes.len())
    }

    /// Fetch one thread in `format` and return (response size, messages in
    /// it); used only by [`crate::probe`].
    pub async fn probe_fetch_thread(
        &self,
        thread_id: &str,
        format: &str,
    ) -> Result<(usize, usize)> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct ThreadWire {
            messages: Vec<serde::de::IgnoredAny>,
        }
        let path = format!("/threads/{thread_id}");
        let query = [("format", format.to_string())];
        let bytes = self
            .call(
                Method::GET,
                &path,
                &query,
                None,
                Cost::Get,
                Priority::Background,
                Retry::Idempotent,
            )
            .await?;
        let t: ThreadWire = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("thread: bad json: {e}")))?;
        Ok((bytes.len(), t.messages.len()))
    }

    /// Current label ids for each message (`format=minimal`), skipping
    /// messages that no longer exist. Used to reconcile after history expiry.
    pub async fn get_message_labels(&self, ids: &[String]) -> Result<Vec<MessageLabels>> {
        let fetched = try_join_all(ids.iter().map(|id| async move {
            let path = format!("/messages/{id}");
            match self
                .get_json::<IdWire>(
                    &path,
                    &[("format", "minimal".to_string())],
                    LIGHT_GET,
                    Priority::Background,
                )
                .await
            {
                Ok(m) => Ok(Some(MessageLabels {
                    id: m.id,
                    thread_id: m.thread_id,
                    label_ids: m.label_ids,
                })),
                Err(Error::Http { status: 404, .. }) => Ok(None),
                Err(e) => Err(e),
            }
        }))
        .await?;
        Ok(fetched.into_iter().flatten().collect())
    }

    /// Returns `Error::HistoryExpired` on 404.
    pub async fn list_history(
        &self,
        start_history_id: u64,
        page_token: Option<&str>,
    ) -> Result<HistoryPage> {
        let mut query = vec![
            ("startHistoryId", start_history_id.to_string()),
            ("maxResults", "500".to_string()),
        ];
        if let Some(t) = page_token {
            query.push(("pageToken", t.to_string()));
        }
        let wire: HistoryWire = match self
            .get_json("/history", &query, HISTORY_LIST, Priority::Background)
            .await
        {
            Err(Error::Http { status: 404, .. }) => return Err(Error::HistoryExpired),
            other => other?,
        };
        Ok(collapse_history(wire, start_history_id))
    }

    /// labels.get messagesTotal for one label (1 unit).
    pub async fn label_messages_total(&self, label_id: &str) -> Result<u64> {
        let l: LabelWire = self
            .get_json(
                &format!("/labels/{label_id}"),
                &[],
                CHEAP,
                Priority::Background,
            )
            .await?;
        Ok(l.messages_total.unwrap_or(0))
    }

    pub async fn list_labels(&self) -> Result<Vec<Label>> {
        let wire: LabelsWire = self
            .get_json("/labels", &[], CHEAP, Priority::Background)
            .await?;
        Ok(wire
            .labels
            .into_iter()
            .map(|l| label_from_wire(&self.inner.email, l))
            .collect())
    }

    /// labels.patch: rename, recolor (`Some(None)` removes the color; the
    /// pair must come from Gmail's palette, see `label_colors`), hide/show
    /// in the label list. Returns the label as Gmail now has it.
    pub async fn patch_label(
        &self,
        label_id: &str,
        name: Option<&str>,
        color: Option<Option<(&str, &str)>>,
        hidden: Option<bool>,
    ) -> Result<Label> {
        let body = label_patch_body(name, color, hidden);
        let path = format!("/labels/{label_id}");
        let bytes = self
            .call(
                Method::PATCH,
                &path,
                &[],
                Some(&body),
                CHEAP,
                Priority::Interactive,
                Retry::Idempotent,
            )
            .await?;
        let wire: LabelWire = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("gmail {path}: bad json: {e}")))?;
        Ok(label_from_wire(&self.inner.email, wire))
    }

    /// labels.create: a user label shown in the label list and on
    /// messages. Not retried after it may have landed (a retry would get a
    /// 409 for its own label); callers look the name up again on 409.
    pub async fn create_label(&self, name: &str) -> Result<Label> {
        let body = serde_json::json!({
            "name": name,
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
        });
        let bytes = self
            .call(
                Method::POST,
                "/labels",
                &[],
                Some(&body),
                CHEAP,
                Priority::Interactive,
                Retry::OnlyIfRejected,
            )
            .await?;
        let wire: LabelWire = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("gmail /labels: bad json: {e}")))?;
        Ok(label_from_wire(&self.inner.email, wire))
    }

    /// labels.delete: removes the label and strips it from every message.
    /// A 404 counts as done (already gone, or a retried delete that landed).
    pub async fn delete_label(&self, label_id: &str) -> Result<()> {
        let path = format!("/labels/{label_id}");
        match self
            .call(
                Method::DELETE,
                &path,
                &[],
                None,
                CHEAP,
                Priority::Interactive,
                Retry::Idempotent,
            )
            .await
        {
            Ok(_) | Err(Error::Http { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let body = serde_json::json!({ "addLabelIds": add, "removeLabelIds": remove });
        let path = format!("/threads/{thread_id}/modify");
        self.call(
            Method::POST,
            &path,
            &[],
            Some(&body),
            CHEAP,
            Priority::Interactive,
            Retry::Idempotent,
        )
        .await
        .map(drop)
    }

    pub async fn trash_thread(&self, thread_id: &str) -> Result<()> {
        let path = format!("/threads/{thread_id}/trash");
        // Unmeasured; charged conservatively.
        self.call(
            Method::POST,
            &path,
            &[],
            None,
            Cost::Fixed(5.0),
            Priority::Interactive,
            Retry::Idempotent,
        )
        .await
        .map(drop)
    }

    pub async fn untrash_thread(&self, thread_id: &str) -> Result<()> {
        let path = format!("/threads/{thread_id}/untrash");
        // Unmeasured; charged conservatively.
        self.call(
            Method::POST,
            &path,
            &[],
            None,
            Cost::Fixed(5.0),
            Priority::Interactive,
            Retry::Idempotent,
        )
        .await
        .map(drop)
    }

    /// Send a raw RFC 5322 message; `thread_id` keeps replies threaded.
    /// Gmail takes recipients from the To/Cc/Bcc headers and strips Bcc from
    /// what recipients receive.
    ///
    /// Once Google accepts the send, this never reports failure: if the
    /// follow-up fetch of the sent message fails, a skeletal `Message`
    /// (id/thread/labels) is returned and incremental sync fills it in.
    pub async fn send_raw(&self, rfc822: &[u8], thread_id: Option<&str>) -> Result<Message> {
        let mut body = serde_json::json!({ "raw": B64URL.encode(rfc822) });
        if let Some(t) = thread_id {
            body["threadId"] = serde_json::Value::String(t.to_string());
        }
        let bytes = self
            .call(
                Method::POST,
                "/messages/send",
                &[],
                Some(&body),
                Cost::Fixed(100.0),
                Priority::Interactive,
                Retry::OnlyIfRejected,
            )
            .await?;
        let sent: IdWire = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("send: bad json: {e}")))?;
        match self.get_one(&sent.id, Priority::Interactive).await {
            Ok(Some(m)) => Ok(m),
            other => {
                if let Err(e) = other {
                    tracing::warn!(account = %self.inner.email, error = %e, "sent, but fetching the sent message failed");
                }
                Ok(skeleton_message(&self.inner.email, sent))
            }
        }
    }

    async fn fetch_attachment_bytes(
        &self,
        message_id: &str,
        attachment_id: &str,
        priority: Priority,
    ) -> Result<Vec<u8>> {
        let path = format!("/messages/{message_id}/attachments/{attachment_id}");
        let a: AttachmentWire = self.get_json(&path, &[], LIGHT_GET, priority).await?;
        convert::b64url_decode(&a.data).ok_or_else(|| Error::Other("attachment: bad base64".into()))
    }

    /// Download attachment bytes. Handles parts Gmail inlined (`part:` ids)
    /// and attachment ids that went stale since the message was synced
    /// (Gmail may reissue them), by re-reading the message.
    pub async fn get_attachment(
        &self,
        message_id: &str,
        attachment: &AttachmentMeta,
    ) -> Result<Vec<u8>> {
        if let Some(part_id) = attachment.id.strip_prefix(PART_ID_PREFIX) {
            let gm = self
                .get_raw_message(message_id, "full", Priority::Interactive)
                .await?
                .ok_or_else(|| Error::Http {
                    status: 404,
                    body: "message not found".into(),
                })?;
            return convert::inline_part_bytes(&gm, part_id)
                .ok_or_else(|| Error::Other(format!("attachment part {part_id} has no data")));
        }
        match self
            .fetch_attachment_bytes(message_id, &attachment.id, Priority::Interactive)
            .await
        {
            Err(Error::Http {
                status: 400 | 404, ..
            }) => {
                let gm = self
                    .get_raw_message(message_id, "full", Priority::Interactive)
                    .await?
                    .ok_or_else(|| Error::Http {
                        status: 404,
                        body: "message not found".into(),
                    })?;
                let fresh =
                    convert::rematch_attachment_id(&gm, attachment).ok_or_else(|| Error::Http {
                        status: 404,
                        body: "attachment not found".into(),
                    })?;
                if let Some(part_id) = fresh.strip_prefix(PART_ID_PREFIX) {
                    return convert::inline_part_bytes(&gm, part_id).ok_or_else(|| {
                        Error::Other(format!("attachment part {part_id} has no data"))
                    });
                }
                self.fetch_attachment_bytes(message_id, &fresh, Priority::Interactive)
                    .await
            }
            other => other,
        }
    }
}

fn skeleton_message(account: &str, m: IdWire) -> Message {
    Message {
        account_id: account.to_string(),
        id: m.id,
        thread_id: m.thread_id,
        date: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
        from: penguin_core::Address {
            name: None,
            email: account.to_string(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: String::new(),
        snippet: String::new(),
        body_text: String::new(),
        body_html: None,
        label_ids: m.label_ids,
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        sender_authenticated: false,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
    }
}

/// Fold history records (oldest first) into net per-message effects.
fn collapse_history(wire: HistoryWire, fallback_history_id: u64) -> HistoryPage {
    let mut page = HistoryPage {
        next_page_token: wire.next_page_token.filter(|t| !t.is_empty()),
        history_id: wire
            .history_id
            .and_then(|h| h.parse().ok())
            .unwrap_or(fallback_history_id),
        ..Default::default()
    };
    let mut seen_added = std::collections::HashSet::new();
    let mut seen_deleted = std::collections::HashSet::new();
    // message id → (label → last op was "add"), in first-seen order.
    let mut label_ops: Vec<(String, Vec<(String, bool)>)> = Vec::new();
    let mut label_idx: HashMap<String, usize> = HashMap::new();
    let note = |page: &mut HistoryPage, m: &IdWire| {
        if !m.thread_id.is_empty() {
            page.message_threads
                .insert(m.id.clone(), m.thread_id.clone());
        }
    };
    for rec in wire.history {
        for a in rec.messages_added {
            note(&mut page, &a.message);
            if seen_added.insert(a.message.id.clone()) {
                page.added.push(a.message.id);
            }
        }
        for d in rec.messages_deleted {
            note(&mut page, &d.message);
            if seen_deleted.insert(d.message.id.clone()) {
                page.deleted.push(d.message.id);
            }
        }
        for (changes, is_add) in [(rec.labels_added, true), (rec.labels_removed, false)] {
            for c in changes {
                note(&mut page, &c.message);
                let idx = *label_idx.entry(c.message.id.clone()).or_insert_with(|| {
                    label_ops.push((c.message.id.clone(), Vec::new()));
                    label_ops.len() - 1
                });
                let ops = &mut label_ops[idx].1;
                for l in c.label_ids {
                    match ops.iter_mut().find(|(name, _)| *name == l) {
                        Some(op) => op.1 = is_add,
                        None => ops.push((l, is_add)),
                    }
                }
            }
        }
    }
    for (id, ops) in label_ops {
        let add: Vec<String> = ops.iter().filter(|o| o.1).map(|o| o.0.clone()).collect();
        let remove: Vec<String> = ops.iter().filter(|o| !o.1).map(|o| o.0.clone()).collect();
        if !add.is_empty() {
            page.labels_added.push((id.clone(), add));
        }
        if !remove.is_empty() {
            page.labels_removed.push((id, remove));
        }
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_patch_body_sends_only_given_fields() {
        assert_eq!(label_patch_body(None, None, None), serde_json::json!({}));
        assert_eq!(
            label_patch_body(
                Some("Clients/Active"),
                Some(Some(("#fb4c2f", "#ffffff"))),
                Some(true)
            ),
            serde_json::json!({
                "name": "Clients/Active",
                "color": { "backgroundColor": "#fb4c2f", "textColor": "#ffffff" },
                "labelListVisibility": "labelHide",
            })
        );
        let clear = label_patch_body(None, Some(None), Some(false));
        assert_eq!(
            clear,
            serde_json::json!({ "color": null, "labelListVisibility": "labelShow" })
        );
        assert!(clear.as_object().unwrap().contains_key("color"));
    }

    #[test]
    fn label_wire_reads_visibility() {
        let wire: LabelWire = serde_json::from_value(serde_json::json!({
            "id": "Label_7", "name": "Receipts", "type": "user",
            "labelListVisibility": "labelHide",
            "color": { "backgroundColor": "#16a766", "textColor": "#ffffff" }
        }))
        .unwrap();
        let l = label_from_wire("ada@x.example", wire);
        assert!(l.hidden);
        assert_eq!(l.kind, "user");
        assert_eq!(l.color.as_deref(), Some("#16a766"));
        let shown: LabelWire = serde_json::from_value(serde_json::json!({
            "id": "INBOX", "name": "INBOX", "type": "system", "labelListVisibility": "labelShowIfUnread"
        }))
        .unwrap();
        assert!(!label_from_wire("ada@x.example", shown).hidden);
    }

    #[test]
    fn history_collapses_label_ops_in_order() {
        let wire: HistoryWire = serde_json::from_value(serde_json::json!({
            "historyId": "900",
            "nextPageToken": "t2",
            "history": [
                { "id": "1", "messagesAdded": [{ "message": { "id": "m1", "threadId": "t1", "labelIds": ["INBOX"] } }] },
                { "id": "2", "labelsRemoved": [{ "message": { "id": "m2", "threadId": "t2" }, "labelIds": ["UNREAD"] }] },
                { "id": "3", "labelsAdded": [{ "message": { "id": "m2", "threadId": "t2" }, "labelIds": ["UNREAD", "STARRED"] }] },
                { "id": "4", "labelsRemoved": [{ "message": { "id": "m3", "threadId": "t3" }, "labelIds": ["INBOX"] }] },
                { "id": "5", "messagesDeleted": [{ "message": { "id": "m4", "threadId": "t4" } }] },
                { "id": "6", "messagesAdded": [{ "message": { "id": "m1", "threadId": "t1" } }] }
            ]
        }))
        .unwrap();
        let page = collapse_history(wire, 1);
        assert_eq!(page.history_id, 900);
        assert_eq!(page.next_page_token.as_deref(), Some("t2"));
        assert_eq!(page.added, vec!["m1"]);
        assert_eq!(page.deleted, vec!["m4"]);
        // Removed then re-added → net add only.
        assert_eq!(
            page.labels_added,
            vec![(
                "m2".to_string(),
                vec!["UNREAD".to_string(), "STARRED".to_string()]
            )]
        );
        assert_eq!(
            page.labels_removed,
            vec![("m3".to_string(), vec!["INBOX".to_string()])]
        );
        assert_eq!(
            page.message_threads.get("m4").map(String::as_str),
            Some("t4")
        );
    }

    #[test]
    fn empty_history_keeps_start_id() {
        let wire: HistoryWire = serde_json::from_value(serde_json::json!({})).unwrap();
        let page = collapse_history(wire, 42);
        assert_eq!(page.history_id, 42);
        assert!(page.added.is_empty() && page.next_page_token.is_none());
    }

    /// Stand-in for Gmail's per-minute quota: a rolling 60 s window of
    /// admitted cost, where each messages.get is charged its *true* cost
    /// (unknown to the client). Rejected calls cost nothing.
    struct SimServer {
        limit: f64,
        log: VecDeque<(Instant, f64)>,
    }

    impl SimServer {
        fn new(limit: f64) -> Arc<Mutex<SimServer>> {
            Arc::new(Mutex::new(SimServer {
                limit,
                log: VecDeque::new(),
            }))
        }
        fn admit(&mut self, cost: f64) -> bool {
            let now = Instant::now();
            while self
                .log
                .front()
                .is_some_and(|(t, _)| now.duration_since(*t) >= Duration::from_secs(60))
            {
                self.log.pop_front();
            }
            let used: f64 = self.log.iter().map(|(_, c)| c).sum();
            if used + cost > self.limit {
                return false;
            }
            self.log.push_back((now, cost));
            true
        }
    }

    #[derive(Default, Debug)]
    struct SimStats {
        /// (account index, when) of each successful get.
        ok_at: Vec<(usize, Duration)>,
        throttled_at: Vec<Duration>,
    }

    impl SimStats {
        fn ok(&self, account: Option<usize>, from: u64, to: u64) -> usize {
            self.ok_at
                .iter()
                .filter(|(a, t)| account.is_none_or(|x| x == *a) && in_range(*t, from, to))
                .count()
        }
        fn throttled(&self, from: u64, to: u64) -> usize {
            self.throttled_at
                .iter()
                .filter(|t| in_range(**t, from, to))
                .count()
        }
    }

    fn in_range(t: Duration, from: u64, to: u64) -> bool {
        t >= Duration::from_secs(from) && t < Duration::from_secs(to)
    }

    struct SimAccount {
        quota: Arc<Quota>,
        server: Arc<Mutex<SimServer>>,
        /// Start sending this many seconds in.
        start_after: u64,
    }

    /// Drive `MAX_IN_FLIGHT` workers per account issuing messages.get
    /// through the real Quota, with the retry discipline `call()` uses (on
    /// 403: report, go back through acquire). `true_cost` prices each call.
    async fn simulate(
        accounts: Vec<SimAccount>,
        true_cost: fn(&mut fastrand::Rng) -> f64,
        run_for: Duration,
    ) -> SimStats {
        let stats = Arc::new(Mutex::new(SimStats::default()));
        let start = Instant::now();
        let deadline = start + run_for;
        let mut workers = Vec::new();
        for (a, acct) in accounts.iter().enumerate() {
            for i in 0..MAX_IN_FLIGHT {
                let (quota, server, stats) =
                    (acct.quota.clone(), acct.server.clone(), stats.clone());
                let begin = start + Duration::from_secs(acct.start_after);
                workers.push(tokio::spawn(async move {
                    tokio::time::sleep_until(begin).await;
                    let mut rng = fastrand::Rng::with_seed((a * 100 + i) as u64);
                    // Varying latencies, like real messages.get calls.
                    let latency = Duration::from_millis(120 + 40 * i as u64);
                    let mut after_throttle = false;
                    while Instant::now() < deadline {
                        let _permit = quota.in_flight.acquire().await.unwrap();
                        quota
                            .acquire(Cost::Get, Priority::Background, after_throttle)
                            .await;
                        tokio::time::sleep(latency).await;
                        let ok = server.lock().unwrap().admit(true_cost(&mut rng));
                        let at = Instant::now() - start;
                        if ok {
                            stats.lock().unwrap().ok_at.push((a, at));
                            after_throttle = false;
                        } else {
                            stats.lock().unwrap().throttled_at.push(at);
                            quota.on_throttle(None);
                            after_throttle = true;
                        }
                    }
                }));
            }
        }
        for w in workers {
            w.await.unwrap();
        }
        Arc::try_unwrap(stats).unwrap().into_inner().unwrap()
    }

    /// `n` accounts drawing on one budget, against one server-side budget.
    fn shared_accounts(
        n: usize,
        configured: f64,
        server_limit: f64,
    ) -> (Arc<Budget>, Vec<SimAccount>) {
        let budget = Arc::new(Budget::new(configured));
        let server = SimServer::new(server_limit);
        let accounts = (0..n)
            .map(|i| SimAccount {
                quota: Arc::new(Quota::new(&format!("a{i}@x.example"), budget.clone())),
                server: server.clone(),
                start_after: 0,
            })
            .collect();
        (budget, accounts)
    }

    fn get_cost(b: &Budget) -> f64 {
        b.lock().get_cost
    }

    /// Each account with its own budget and its own server-side budget.
    fn per_account(
        n: usize,
        configured: f64,
        server_limit: f64,
    ) -> (Vec<Arc<Budget>>, Vec<SimAccount>) {
        let budgets: Vec<Arc<Budget>> = (0..n).map(|_| Arc::new(Budget::new(configured))).collect();
        let accounts = budgets
            .iter()
            .enumerate()
            .map(|(i, b)| SimAccount {
                quota: Arc::new(Quota::new(&format!("p{i}@x.example"), b.clone())),
                server: SimServer::new(server_limit),
                start_after: 0,
            })
            .collect();
        (budgets, accounts)
    }

    #[tokio::test(start_paused = true)]
    async fn five_accounts_each_run_near_their_own_ceiling() {
        // Live shape: 5 syncing mailboxes, 6,000/min each, ~60u per get.
        let (budgets, accounts) = per_account(5, 6_000.0, 6_000.0);
        let stats = simulate(accounts, |_| 60.0, Duration::from_secs(900)).await;
        for a in 0..5 {
            let per_min = stats.ok(Some(a), 300, 900) as f64 / 10.0;
            assert!(
                per_min >= 85.0,
                "account {a}: {per_min:.0} gets/min (ceiling 100)"
            );
        }
        let throttled = stats.throttled(300, 900);
        let sent = stats.ok(None, 300, 900) + throttled;
        println!(
            "per-account x5 @60u: {:.0} gets/min each (ceiling 100), {throttled}/{sent} throttled, learned {:.1}u",
            stats.ok(None, 300, 900) as f64 / 50.0,
            get_cost(&budgets[0])
        );
        assert!(
            (throttled as f64) < 0.01 * sent as f64,
            "{throttled} of {sent} throttled"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn shared_mode_splits_one_budget_fairly() {
        let (budget, accounts) = shared_accounts(5, 6_000.0, 6_000.0);
        let stats = simulate(accounts, |_| 60.0, Duration::from_secs(900)).await;
        let per_min = stats.ok(None, 300, 900) as f64 / 10.0;
        let throttled = stats.throttled(300, 900);
        let sent = stats.ok(None, 300, 900) + throttled;
        let shares: Vec<usize> = (0..5).map(|a| stats.ok(Some(a), 300, 900)).collect();
        println!(
            "shared x5 @60u: {per_min:.0} gets/min total (ceiling 100), {throttled}/{sent} throttled, shares {shares:?}, learned {:.1}u",
            get_cost(&budget)
        );
        assert!(per_min >= 85.0, "{per_min:.0} gets/min");
        assert!(
            (throttled as f64) < 0.02 * sent as f64,
            "{throttled} of {sent} throttled"
        );
        let mean = shares.iter().sum::<usize>() as f64 / 5.0;
        for s in &shares {
            assert!(
                (*s as f64 - mean).abs() <= 0.1 * mean,
                "unfair shares {shares:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shared_mode_late_joiner_gets_an_equal_share() {
        let budget = Arc::new(Budget::new(6_000.0));
        let server = SimServer::new(6_000.0);
        let accounts = vec![
            SimAccount {
                quota: Arc::new(Quota::new("early@x.example", budget.clone())),
                server: server.clone(),
                start_after: 0,
            },
            SimAccount {
                quota: Arc::new(Quota::new("late@x.example", budget.clone())),
                server,
                start_after: 180,
            },
        ];
        let stats = simulate(accounts, |_| 60.0, Duration::from_secs(600)).await;
        let (early, late) = (stats.ok(Some(0), 240, 600), stats.ok(Some(1), 240, 600));
        println!("after join: early {early}, late {late}");
        assert!(
            (early as f64 - late as f64).abs() <= 0.1 * early as f64,
            "early {early} vs late {late}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn learns_a_higher_real_cost() {
        let (budgets, accounts) = per_account(1, 6_000.0, 6_000.0);
        let stats = simulate(accounts, |_| 75.0, Duration::from_secs(900)).await;
        let per_min = stats.ok(None, 300, 900) as f64 / 10.0;
        let throttled = stats.throttled(300, 900);
        println!(
            "true 75u: {per_min:.0}/min (ceiling 80), {throttled} throttled, learned {:.1}u",
            get_cost(&budgets[0])
        );
        assert!(per_min >= 0.85 * 80.0, "{per_min:.0}");
        assert!((throttled as f64) < 0.02 * stats.ok(None, 300, 900) as f64);
    }

    #[tokio::test(start_paused = true)]
    async fn tracks_varying_costs() {
        let (budgets, accounts) = per_account(2, 6_000.0, 6_000.0);
        let stats = simulate(
            accounts,
            |r| 50.0 + r.f64() * 20.0,
            Duration::from_secs(900),
        )
        .await;
        let per_min = stats.ok(None, 300, 900) as f64 / 20.0;
        let throttled = stats.throttled(300, 900);
        println!(
            "variable 50–70u: {per_min:.0}/min each, {throttled} throttled, learned {:.1}u",
            get_cost(&budgets[0])
        );
        assert!(per_min >= 0.85 * 100.0, "{per_min:.0}");
        assert!((throttled as f64) < 0.02 * stats.ok(None, 300, 900) as f64);
    }

    #[tokio::test(start_paused = true)]
    async fn probes_up_when_gets_are_cheaper_than_assumed() {
        // E.g. a cheaper fetch format at ~12u: from the 60u start, find ~500/min.
        let (budgets, accounts) = per_account(1, 6_000.0, 6_000.0);
        let stats = simulate(accounts, |_| 12.0, Duration::from_secs(2_400)).await;
        let late = stats.ok(None, 1_800, 2_400) as f64 / 10.0;
        println!(
            "true 12u: late {late:.0}/min (ceiling 500), learned {:.1}u",
            get_cost(&budgets[0])
        );
        assert!(late >= 0.8 * 500.0, "{late:.0}");
    }

    #[tokio::test(start_paused = true)]
    async fn scales_up_after_a_quota_raise_via_config() {
        // PENGUIN_GMAIL_UNITS_PER_MIN=60000 once the raise is approved.
        let (_, accounts) = per_account(1, 60_000.0, 60_000.0);
        let stats = simulate(accounts, |_| 60.0, Duration::from_secs(600)).await;
        let per_min = stats.ok(None, 240, 600) as f64 / 6.0;
        println!("60k budget @60u: {per_min:.0}/min (ceiling 1000)");
        assert!(per_min >= 850.0, "{per_min:.0}");
    }

    #[tokio::test(start_paused = true)]
    async fn recovers_when_real_limit_is_lower_than_configured() {
        let (_, accounts) = per_account(1, 6_000.0, 3_000.0);
        let stats = simulate(accounts, |_| 60.0, Duration::from_secs(900)).await;
        let late = stats.ok(None, 300, 900) as f64 / 10.0;
        let throttled = stats.throttled(300, 900);
        println!("limit 3000 @60u: {late:.0}/min (ceiling 50), {throttled} throttled after 5 min");

        assert!(late >= 0.8 * 50.0, "{late:.0}");
        assert!((throttled as f64) < 0.03 * stats.ok(None, 300, 900) as f64);
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_without_our_traffic_pauses_but_keeps_the_estimate() {
        // Launch into a minute already spent elsewhere: 403 with no gets of ours.
        let budget = Arc::new(Budget::new(6_000.0));
        let q = Quota::new("a@x.example", budget.clone());
        for _ in 0..12 {
            q.on_throttle(None); // one in-flight burst = one episode
        }
        assert_eq!(budget.lock().throttle_episodes, 1);
        assert_eq!(get_cost(&budget), INITIAL_GET_COST);
        let t0 = Instant::now();
        q.acquire(Cost::Get, Priority::Background, false).await;
        assert!(t0.elapsed() >= FIRST_PAUSE);
        // Interactive calls don't wait for the gate (but throttled retries do).
        q.on_throttle(None);
        let t1 = Instant::now();
        q.acquire(CHEAP, Priority::Interactive, false).await;
        assert!(t1.elapsed() < Duration::from_millis(2));
        q.acquire(CHEAP, Priority::Interactive, true).await;
        assert!(
            t1.elapsed() >= FIRST_PAUSE * 2,
            "tripping again right after a pause pauses longer"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn consecutive_episodes_pause_longer_up_to_a_window_then_reset() {
        let budget = Budget::new(6_000.0);
        let mut pauses = Vec::new();
        for _ in 0..5 {
            budget.on_throttle(None);
            let until = budget.lock().paused_until.unwrap();
            pauses.push((until - Instant::now()).as_secs());
            tokio::time::advance(until - Instant::now()).await;
        }
        assert_eq!(pauses, vec![10, 20, 40, 60, 60]);
        // An isolated overshoot long after the last pause starts over.
        tokio::time::advance(STREAK_WINDOW).await;
        budget.on_throttle(None);
        assert_eq!(
            (budget.lock().paused_until.unwrap() - Instant::now()).as_secs(),
            10
        );
        // Retry-After wins when Google sends it.
        tokio::time::advance(Duration::from_secs(20)).await;
        budget.on_throttle(Some(Duration::from_secs(7)));
        assert_eq!(
            (budget.lock().paused_until.unwrap() - Instant::now()).as_secs(),
            7
        );
    }

    #[tokio::test(start_paused = true)]
    async fn learns_from_the_window_when_it_has_enough_gets() {
        let budget = Arc::new(Budget::new(6_000.0));
        let q = Quota::new("a@x.example", budget.clone());
        // 50 gets plus 100 cheap units in the last minute, and Google says
        // the 6,000 budget is spent: each get really cost (6,000 − 100) / 50.
        {
            let mut st = budget.lock();
            let now = Instant::now();
            for _ in 0..50 {
                st.sent.push_back((now, 1, 0.0));
                st.shares
                    .entry("a@x.example".into())
                    .or_default()
                    .gets
                    .push_back(now);
            }
            st.sent.push_back((now, 0, 100.0));
        }
        q.on_throttle(None);
        let expected = (6_000.0 - 100.0) / 50.0 * (1.0 + LEARN_MARGIN);
        assert!(
            (get_cost(&budget) - expected).abs() < 1e-9,
            "{}",
            get_cost(&budget)
        );
        let s = q.stats();
        assert_eq!(
            (
                s.gets_last_min,
                s.account_gets_last_min,
                s.active_accounts,
                s.throttle_episodes
            ),
            (50, 50, 1, 1)
        );
        assert!((s.gets_per_sec - budget.lock().rate / expected).abs() < 1e-9);
    }

    #[tokio::test(start_paused = true)]
    async fn a_learned_cost_above_the_burst_size_still_gets_through() {
        // Capacity is 2% of 6,000 = 120 units; a 200u get must not wait forever.
        let budget = Arc::new(Budget::new(6_000.0));
        budget.lock().get_cost = 200.0;
        budget.lock().tokens = 0.0;
        let q = Quota::new("a@x.example", budget.clone());
        let t0 = Instant::now();
        tokio::time::timeout(
            Duration::from_secs(10),
            q.acquire(Cost::Get, Priority::Background, false),
        )
        .await
        .expect("acquired");
        // 200 units at 95 u/s ≈ 2.1 s.
        assert!(t0.elapsed() >= Duration::from_secs(2) && t0.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn resizing_a_budget_keeps_the_learned_cost() {
        let b = Budget::new(6_000.0);
        b.lock().get_cost = 61.0;
        b.lock().tokens = 100.0;
        b.set_units_per_min(60_000.0);
        let st = b.lock();
        assert_eq!(st.units_per_min, 60_000.0);
        assert!((st.rate - 60_000.0 * TARGET_UTILIZATION / 60.0).abs() < 1e-9);
        assert_eq!(st.get_cost, 61.0);
        drop(st);
        b.set_units_per_min(600.0);
        let st = b.lock();
        assert!(
            st.tokens <= st.capacity,
            "tokens clamp to the smaller bucket"
        );
    }

    #[test]
    fn capacity_plus_a_minute_stays_under_the_limit() {
        for limit in [600.0, 6_000.0, 15_000.0] {
            let b = Budget::new(limit);
            let st = b.lock();
            assert!(
                st.capacity + st.rate * 60.0 <= limit * 0.98,
                "limit {limit}"
            );
        }
    }

    #[test]
    fn a_token_used_for_two_accounts_is_detected() {
        assert_eq!(claim_token("tok-a-unique-1", "a@x.example"), Ok(true));
        assert_eq!(claim_token("tok-a-unique-1", "a@x.example"), Ok(false));
        assert_eq!(claim_token("tok-b-unique-1", "b@x.example"), Ok(true));
        assert_eq!(
            claim_token("tok-a-unique-1", "b@x.example"),
            Err("a@x.example".to_string())
        );
    }

    #[test]
    fn backoff_grows_and_honors_retry_after() {
        assert!(backoff(1, None) >= Duration::from_secs(1));
        assert!(backoff(20, None) <= Duration::from_secs(65));
        assert!(backoff(1, Some(Duration::from_secs(30))) >= Duration::from_secs(30));
    }
}
