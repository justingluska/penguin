//! Thread summaries with Apple's on-device model (docs/SUMMARIES.md).
//!
//! - `summary_availability`: can this Mac summarize, and if not, why.
//! - `cached_summary`: the stored summary for the thread, marked `stale`
//!   when the thread changed since. Local only: one thread read plus one
//!   primary-key lookup; never waits on the model.
//! - `summarize_thread`: make (or remake) the summary. Streams
//!   `penguin://summary-progress` while the model writes, stores the result,
//!   and returns it. One summary runs at a time; asking again for the same
//!   thread cancels the one in progress.
//! - `cancel_summary`, `prewarm_summarizer` (the pointer is on Summarize).
//!
//! What goes into the model, and how long threads are split, is
//! `penguin_core::summary`; the orchestration is `pipeline.rs`; the model
//! itself is behind `engine::Engine` (`apple.rs` on macOS).
//!
//! Logging: ids, counts, timings and outcome codes only. Never a subject,
//! a body, a prompt or anything the model wrote.

#[cfg(target_os = "macos")]
pub mod apple;
pub mod engine;
pub mod pipeline;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use penguin_core::summary::{
    build_input, instructions, version_key, InputOptions, DEFAULT_CONTEXT_TOKENS,
};
use penguin_core::{AiAvailability, AiSummary, AiUnavailableReason, SummaryProgress};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::watch;

use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::state::{blocking, AppState};
use engine::Engine;
use pipeline::{Failure, Job};

pub const SUMMARY_PROGRESS_EVENT: &str = "penguin://summary-progress";

/// Prewarming again sooner than this does nothing (Apple keeps the model
/// loaded for a while; each call builds a session).
const PREWARM_EVERY: Duration = Duration::from_secs(60);

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type SummarizerRef<'a> = State<'a, Arc<Summarizer>>;

/// Managed state: the engine, and the summaries in progress.
pub struct Summarizer {
    /// Shared with the composer's writing (src/writing/).
    pub(crate) engine: Box<dyn Engine>,
    /// Cancel switches of the summaries in progress, by (account, thread),
    /// with a run number so a finished run removes only its own entry.
    running: Mutex<HashMap<(String, String), (u64, watch::Sender<bool>)>>,
    runs: std::sync::atomic::AtomicU64,
    /// One summary at a time: the model serves one request at a time
    /// (`concurrentRequests`), and a second long thread would only queue
    /// behind the first on the Neural Engine anyway.
    pub(crate) gate: tokio::sync::Mutex<()>,
    last_prewarm: Mutex<Option<Instant>>,
}

impl Summarizer {
    pub fn new(engine: Box<dyn Engine>) -> Arc<Self> {
        Arc::new(Summarizer {
            engine,
            running: Mutex::new(HashMap::new()),
            runs: std::sync::atomic::AtomicU64::new(1),
            gate: tokio::sync::Mutex::new(()),
            last_prewarm: Mutex::new(None),
        })
    }

    /// The system engine: Apple Foundation Models on macOS, none elsewhere.
    pub fn system() -> Arc<Self> {
        Self::new(engine::system())
    }
}

/// "Fri 2026-09-12" in local time. Part of the instructions, so prewarm and
/// the real request must agree on it.
pub(crate) fn today() -> String {
    chrono::Local::now().format("%a %Y-%m-%d").to_string()
}

fn now_ms() -> i64 {
    crate::ops::now_ms()
}

#[tauri::command]
pub async fn summary_availability(summarizer: SummarizerRef<'_>) -> CmdResult<AiAvailability> {
    let s = summarizer.inner().clone();
    blocking(move || Ok(s.engine.availability())).await
}

/// The thread's messages as the model would see them, and their version.
fn prepare(st: &AppState, account_id: &str, thread_id: &str) -> CmdResult<Option<(String, Job)>> {
    let Some(detail) = st.store.get_thread(account_id, thread_id)? else {
        return Ok(None);
    };
    let pending = st.store.pending_message_ids(account_id, thread_id)?;
    let mine = st
        .store
        .list_accounts()?
        .into_iter()
        .map(|a| a.email.to_lowercase())
        .collect();
    let offset = chrono::Local::now().offset().local_minus_utc() / 60;
    let messages = build_input(
        &detail,
        &pending,
        &InputOptions {
            my_addresses: mine,
            utc_offset_minutes: offset,
        },
    );
    let version = version_key(&messages);
    let job = Job {
        account_id: account_id.to_string(),
        thread_id: thread_id.to_string(),
        subject: detail.subject,
        messages,
        version: version.clone(),
        today: today(),
        context_tokens: DEFAULT_CONTEXT_TOKENS,
        now_ms: now_ms(),
    };
    Ok(Some((version, job)))
}

#[tauri::command]
pub async fn cached_summary(
    state: AppStateRef<'_>,
    account_id: String,
    thread_id: String,
) -> CmdResult<Option<AiSummary>> {
    if !state.settings.get().summaries {
        return Ok(None);
    }
    let st = state.inner().clone();
    blocking(move || {
        let Some((version, _)) = prepare(&st, &account_id, &thread_id)? else {
            return Ok(None);
        };
        Ok(st.store.get_summary(&account_id, &thread_id, &version)?)
    })
    .await
}

#[tauri::command]
pub async fn summarize_thread(
    app: AppHandle,
    state: AppStateRef<'_>,
    summarizer: SummarizerRef<'_>,
    account_id: String,
    thread_id: String,
) -> CmdResult<AiSummary> {
    if !state.settings.get().summaries {
        return Err(CmdError::invalid(
            "Summaries are turned off in Settings → AI.",
        ));
    }
    let s = summarizer.inner().clone();
    let availability = {
        let s = s.clone();
        blocking(move || Ok(s.engine.availability())).await?
    };
    if !availability.available {
        return Err(CmdError::invalid(unavailable_text(availability.reason)));
    }
    let st = state.inner().clone();
    let (acct, tid) = (account_id.clone(), thread_id.clone());
    let Some((_, mut job)) = blocking(move || prepare(&st, &acct, &tid)).await? else {
        return Err(CmdError::not_found(
            "This conversation is no longer available.",
        ));
    };
    job.context_tokens = availability.context_tokens.max(1) as usize;

    // Asking again for the same thread replaces the one in progress.
    let key = (account_id.clone(), thread_id.clone());
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let run = s.runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Some((_, old)) = s
        .running
        .lock()
        .unwrap()
        .insert(key.clone(), (run, cancel_tx))
    {
        let _ = old.send(true);
    }
    let started = Instant::now();
    let result = {
        let _turn = s.gate.lock().await;
        let app2 = app.clone();
        let mut progress = move |p: SummaryProgress| {
            let _ = app2.emit(SUMMARY_PROGRESS_EVENT, p);
        };
        pipeline::summarize(s.engine.as_ref(), &job, cancel_rx, &mut progress).await
    };
    {
        let mut running = s.running.lock().unwrap();
        if running.get(&key).is_some_and(|(r, _)| *r == run) {
            running.remove(&key);
        }
    }
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(summary) => {
            tracing::info!(
                account = %account_id,
                thread = %thread_id,
                messages = summary.message_count,
                omitted = summary.omitted,
                ms,
                "thread summarized"
            );
            let store = state.store.clone();
            let saved = summary.clone();
            blocking(move || Ok(store.put_summary(&saved)?)).await?;
            Ok(summary)
        }
        Err(f) => {
            tracing::info!(account = %account_id, thread = %thread_id, ms, outcome = ?f, "thread not summarized");
            Err(failure_error(f))
        }
    }
}

#[tauri::command]
pub fn cancel_summary(
    summarizer: SummarizerRef<'_>,
    account_id: String,
    thread_id: String,
) -> bool {
    let running = summarizer.running.lock().unwrap();
    match running.get(&(account_id, thread_id)) {
        Some((_, tx)) => tx.send(true).is_ok(),
        None => false,
    }
}

/// The user is about to ask (pointer on Summarize): load the model now.
#[tauri::command]
pub async fn prewarm_summarizer(
    state: AppStateRef<'_>,
    summarizer: SummarizerRef<'_>,
) -> CmdResult<()> {
    if !state.settings.get().summaries {
        return Ok(());
    }
    let s = summarizer.inner().clone();
    {
        let mut last = s.last_prewarm.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < PREWARM_EVERY) {
            return Ok(());
        }
        *last = Some(Instant::now());
    }
    blocking(move || {
        if s.engine.availability().available {
            s.engine.prewarm(&instructions(&today()));
        }
        Ok(())
    })
    .await
}

/// Settings and the thread view say this once; the Summarize action is
/// hidden while it applies.
pub fn unavailable_text(reason: Option<AiUnavailableReason>) -> &'static str {
    match reason {
        None => "Summaries are available.",
        Some(AiUnavailableReason::DeviceNotEligible) => {
            "This Mac can't run Apple Intelligence, which summaries use. It needs a Mac with Apple silicon (M1 or later)."
        }
        Some(AiUnavailableReason::AppleIntelligenceNotEnabled) => {
            "Turn on Apple Intelligence in System Settings → Apple Intelligence & Siri to summarize conversations."
        }
        Some(AiUnavailableReason::ModelNotReady) => {
            "Apple Intelligence is still getting ready on this Mac (downloading its model). Summaries will work once it's done."
        }
        Some(AiUnavailableReason::OsTooOld) => "Summaries need macOS 26 or later with Apple Intelligence.",
        Some(AiUnavailableReason::UnsupportedPlatform) => "Summaries run on a Mac with Apple Intelligence.",
        Some(AiUnavailableReason::NotBuilt) => {
            "This build of Penguin was made without Apple's Foundation Models SDK (Xcode 26 or later)."
        }
        Some(AiUnavailableReason::Unknown) => "Apple Intelligence isn't available on this Mac right now.",
    }
}

fn failure_error(f: Failure) -> CmdError {
    let text = match f {
        Failure::Cancelled => return CmdError::new(ErrorCode::Cancelled, "Summary cancelled"),
        Failure::Unavailable => "Apple Intelligence isn't available right now.",
        Failure::NotReady => {
            "Apple Intelligence is still getting ready on this Mac. Try again in a little while."
        }
        Failure::Refused => "Apple's on-device model declined to summarize this conversation.",
        Failure::UnsupportedLanguage => {
            "Apple Intelligence doesn't support this conversation's language yet."
        }
        Failure::TooLong => "This conversation is too long to summarize on this Mac.",
        Failure::Busy => "The on-device model is busy. Try again in a moment.",
        Failure::Empty => "There's no text in this conversation to summarize.",
        Failure::Other => "Couldn't summarize this conversation.",
    };
    CmdError::new(ErrorCode::Other, text)
}
