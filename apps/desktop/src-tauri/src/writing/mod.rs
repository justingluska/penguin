//! Writing in the composer with Apple's on-device model (docs/COMPOSE-SPEED.md):
//! the same Foundation Models engine and one-at-a-time gate as thread
//! summaries (src/summary/).
//!
//! - `write_with_ai`: draft a message or a reply from a short request, or
//!   rewrite text (shorter, friendlier, more formal, fixed grammar, or as
//!   asked). Streams `penguin://write-progress {runId, text}` and returns the
//!   finished text; the composer shows it as a suggestion to accept or discard.
//! - `cancel_write`, `prewarm_writer` (the AI bar opened).
//! - `suggest_replies`: up to three short replies to the newest message
//!   (instant replies), cached per thread version in memory.
//!
//! What the model is told and shown is `penguin_core::writing`; the calls
//! are `pipeline.rs`. Logging: ids, action, sizes, timings and outcome codes
//! only. Never the request, the text, the mail or anything the model wrote.

pub mod pipeline;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use penguin_core::summary::{
    build_input, estimate_tokens, version_key, InputOptions, SourceMessage,
};
use penguin_core::writing::{
    check_request, context_budget, fit_context, instructions, response_tokens,
    suggestion_instructions, suggestion_prompt, suggestion_text_format, temperature, write_prompt,
    WriteInput, SUGGESTION_RESPONSE_TOKENS,
};
use penguin_core::{
    AiUnavailableReason, ReplySuggestions, WriteAction, WriteProgress, WriteRequest, WriteResult,
};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::watch;

use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::state::{blocking, AppState};
use crate::summary::pipeline::Failure;
use crate::summary::{today, Summarizer};
use pipeline::{SuggestJob, WriteJob};

pub const WRITE_PROGRESS_EVENT: &str = "penguin://write-progress";

/// Prewarming again sooner than this does nothing.
const PREWARM_EVERY: Duration = Duration::from_secs(60);
/// Suggestion sets kept in memory (per account, thread and version).
const SUGGESTION_CACHE: usize = 64;
/// Shares of the context budget tried in turn when a request doesn't fit;
/// the last still holds (the end of) the message being answered.
const CONTEXT_SHARES: [usize; 3] = [100, 50, 20];

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type SummarizerRef<'a> = State<'a, Arc<Summarizer>>;
type WriterRef<'a> = State<'a, Arc<Writer>>;

/// Managed state: writes in progress and suggestions made.
pub struct Writer {
    running: Mutex<HashMap<String, (u64, watch::Sender<bool>)>>,
    runs: std::sync::atomic::AtomicU64,
    last_prewarm: Mutex<Option<Instant>>,
    suggestions: Mutex<VecDeque<(String, String, String, Vec<String>)>>,
}

impl Writer {
    pub fn new() -> Arc<Self> {
        Arc::new(Writer {
            running: Mutex::new(HashMap::new()),
            runs: std::sync::atomic::AtomicU64::new(1),
            last_prewarm: Mutex::new(None),
            suggestions: Mutex::new(VecDeque::new()),
        })
    }

    fn cached(&self, account: &str, thread: &str, version: &str) -> Option<Vec<String>> {
        let cache = self.suggestions.lock().unwrap();
        cache
            .iter()
            .find(|(a, t, v, _)| a == account && t == thread && v == version)
            .map(|e| e.3.clone())
    }

    fn remember(&self, account: &str, thread: &str, version: &str, replies: &[String]) {
        let mut cache = self.suggestions.lock().unwrap();
        cache.retain(|(a, t, _, _)| !(a == account && t == thread));
        cache.push_back((
            account.into(),
            thread.into(),
            version.into(),
            replies.to_vec(),
        ));
        while cache.len() > SUGGESTION_CACHE {
            cache.pop_front();
        }
    }
}

/// The conversation as the model reads it (authored text, oldest first),
/// and its subject; None when the thread isn't in the store.
fn thread_context(
    st: &AppState,
    account_id: &str,
    thread_id: &str,
) -> CmdResult<Option<(String, Vec<SourceMessage>)>> {
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
    Ok(Some((detail.subject, messages)))
}

/// The prompts for a request at decreasing context (just one for a
/// rewrite, or a draft without a conversation).
fn write_prompts(
    input: &WriteInput<'_>,
    instr: &str,
    context: &[SourceMessage],
    context_tokens: usize,
    response: u32,
) -> Vec<String> {
    let bare = write_prompt(input, &[]);
    if context.is_empty() || input.action != Some(WriteAction::Draft) {
        return vec![bare];
    }
    let fixed = estimate_tokens(instr) + estimate_tokens(&bare);
    let mut out: Vec<String> = Vec::new();
    for share in CONTEXT_SHARES {
        let budget = context_budget(context_tokens, fixed, response as usize, share);
        let p = write_prompt(input, &fit_context(context, budget));
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn suggestion_prompts(
    subject: &str,
    instr: &str,
    context: &[SourceMessage],
    context_tokens: usize,
) -> Vec<String> {
    let fixed = estimate_tokens(instr)
        + estimate_tokens(suggestion_text_format())
        + estimate_tokens(&suggestion_prompt(subject, &[]));
    let mut out: Vec<String> = Vec::new();
    for share in [100, 50] {
        let budget = context_budget(
            context_tokens,
            fixed,
            SUGGESTION_RESPONSE_TOKENS as usize,
            share,
        );
        let ctx = fit_context(context, budget);
        if ctx.is_empty() {
            continue;
        }
        let p = suggestion_prompt(subject, &ctx);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

#[tauri::command]
pub async fn write_with_ai(
    app: AppHandle,
    state: AppStateRef<'_>,
    summarizer: SummarizerRef<'_>,
    writer: WriterRef<'_>,
    request: WriteRequest,
) -> CmdResult<WriteResult> {
    if !state.settings.get().write_with_ai {
        return Err(CmdError::invalid(
            "Write with AI is turned off in Settings → AI.",
        ));
    }
    if let Some(why) = check_request(
        request.action,
        &request.run_id,
        &request.instruction,
        &request.text,
        request.thread_id.is_some(),
    ) {
        return Err(CmdError::invalid(why));
    }
    let s = summarizer.inner().clone();
    let availability = {
        let s = s.clone();
        blocking(move || Ok(s.engine.availability())).await?
    };
    if !availability.available {
        return Err(CmdError::invalid(unavailable_text(availability.reason)));
    }

    // The conversation, for a draft that answers one.
    let context = match (&request.thread_id, request.action) {
        (Some(tid), WriteAction::Draft) => {
            let st = state.inner().clone();
            let (acct, tid) = (request.account_id.clone(), tid.clone());
            blocking(move || thread_context(&st, &acct, &tid))
                .await?
                .map(|(_, m)| m)
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let instr = instructions(request.action, &today());
    let response = response_tokens(request.action, &request.text);
    let input = WriteInput {
        action: Some(request.action),
        instruction: &request.instruction,
        text: &request.text,
        subject: &request.subject,
        recipients: &request.recipients,
        my_name: &request.my_name,
    };
    let job = WriteJob {
        prompts: write_prompts(
            &input,
            &instr,
            &context,
            availability.context_tokens.max(1) as usize,
            response,
        ),
        instructions: instr,
        max_response_tokens: response,
        temperature: temperature(request.action),
        sign_off_name: (request.action == WriteAction::Draft).then(|| request.my_name.clone()),
    };

    // A second request with the same run id replaces the first.
    let run_id = request.run_id.clone();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let run = writer
        .runs
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Some((_, old)) = writer
        .running
        .lock()
        .unwrap()
        .insert(run_id.clone(), (run, cancel_tx))
    {
        let _ = old.send(true);
    }
    let started = Instant::now();
    let result = {
        let _turn = s.gate.lock().await;
        let app2 = app.clone();
        let id = run_id.clone();
        let mut progress = move |text: String| {
            let _ = app2.emit(
                WRITE_PROGRESS_EVENT,
                WriteProgress {
                    run_id: id.clone(),
                    text,
                },
            );
        };
        pipeline::write(s.engine.as_ref(), &job, cancel_rx, &mut progress).await
    };
    {
        let mut running = writer.running.lock().unwrap();
        if running.get(&run_id).is_some_and(|(r, _)| *r == run) {
            running.remove(&run_id);
        }
    }
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(text) => {
            tracing::info!(
                run = %run_id,
                action = ?request.action,
                context = context.len(),
                in_chars = request.text.chars().count(),
                out_chars = text.chars().count(),
                ms,
                "wrote with the on-device model"
            );
            Ok(WriteResult { run_id, text })
        }
        Err(f) => {
            tracing::info!(run = %run_id, action = ?request.action, ms, outcome = ?f, "nothing written");
            Err(failure_error(f))
        }
    }
}

#[tauri::command]
pub fn cancel_write(writer: WriterRef<'_>, run_id: String) -> bool {
    let running = writer.running.lock().unwrap();
    match running.get(&run_id) {
        Some((_, tx)) => tx.send(true).is_ok(),
        None => false,
    }
}

/// The AI bar opened: load the model now (at most once a minute).
#[tauri::command]
pub async fn prewarm_writer(
    state: AppStateRef<'_>,
    summarizer: SummarizerRef<'_>,
    writer: WriterRef<'_>,
) -> CmdResult<()> {
    if !state.settings.get().write_with_ai {
        return Ok(());
    }
    {
        let mut last = writer.last_prewarm.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < PREWARM_EVERY) {
            return Ok(());
        }
        *last = Some(Instant::now());
    }
    let s = summarizer.inner().clone();
    blocking(move || {
        if s.engine.availability().available {
            s.engine
                .prewarm(&instructions(WriteAction::Draft, &today()));
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn suggest_replies(
    state: AppStateRef<'_>,
    summarizer: SummarizerRef<'_>,
    writer: WriterRef<'_>,
    account_id: String,
    thread_id: String,
) -> CmdResult<ReplySuggestions> {
    let settings = state.settings.get();
    if !(settings.instant_replies.enabled && settings.instant_replies.ai_suggestions) {
        return Err(CmdError::invalid(
            "Suggested replies are turned off in Settings → Compose.",
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
    let Some((subject, messages)) = blocking(move || thread_context(&st, &acct, &tid)).await?
    else {
        return Err(CmdError::not_found(
            "This conversation is no longer available.",
        ));
    };
    let version = version_key(&messages);
    let out = |replies: Vec<String>| ReplySuggestions {
        account_id: account_id.clone(),
        thread_id: thread_id.clone(),
        version: version.clone(),
        replies,
    };
    if messages.is_empty() {
        return Ok(out(Vec::new()));
    }
    if let Some(replies) = writer.cached(&account_id, &thread_id, &version) {
        return Ok(out(replies));
    }
    let instr = suggestion_instructions(&today());
    let job = SuggestJob {
        prompts: suggestion_prompts(
            &subject,
            &instr,
            &messages,
            availability.context_tokens.max(1) as usize,
        ),
        instructions: instr,
    };
    if job.prompts.is_empty() {
        return Ok(out(Vec::new()));
    }
    let started = Instant::now();
    // Nothing cancels a suggestion; the sender just has to outlive it.
    let (_keep, cancel_rx) = watch::channel(false);
    let result = {
        let _turn = s.gate.lock().await;
        pipeline::suggest(s.engine.as_ref(), &job, cancel_rx).await
    };
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(replies) => {
            tracing::info!(account = %account_id, thread = %thread_id, count = replies.len(), ms, "replies suggested");
            writer.remember(&account_id, &thread_id, &version, &replies);
            Ok(out(replies))
        }
        Err(f) => {
            tracing::info!(account = %account_id, thread = %thread_id, ms, outcome = ?f, "no replies suggested");
            Err(failure_error(f))
        }
    }
}

/// Why writing help can't run here (the composer hides it; Settings says this).
pub fn unavailable_text(reason: Option<AiUnavailableReason>) -> &'static str {
    match reason {
        None => "Writing help is available.",
        Some(AiUnavailableReason::DeviceNotEligible) => {
            "This Mac can't run Apple Intelligence, which writing help uses. It needs a Mac with Apple silicon (M1 or later)."
        }
        Some(AiUnavailableReason::AppleIntelligenceNotEnabled) => {
            "Turn on Apple Intelligence in System Settings → Apple Intelligence & Siri to write with AI."
        }
        Some(AiUnavailableReason::ModelNotReady) => {
            "Apple Intelligence is still getting ready on this Mac (downloading its model). Writing help will work once it's done."
        }
        Some(AiUnavailableReason::OsTooOld) => {
            "Writing help needs macOS 26 or later with Apple Intelligence."
        }
        Some(AiUnavailableReason::UnsupportedPlatform) => {
            "Writing help runs on a Mac with Apple Intelligence."
        }
        Some(AiUnavailableReason::NotBuilt) => {
            "This build of Penguin was made without Apple's Foundation Models SDK (Xcode 26 or later)."
        }
        Some(AiUnavailableReason::Unknown) => {
            "Apple Intelligence isn't available on this Mac right now."
        }
    }
}

pub(crate) fn failure_error(f: Failure) -> CmdError {
    let text = match f {
        Failure::Cancelled => return CmdError::new(ErrorCode::Cancelled, "Writing cancelled"),
        Failure::Unavailable => "Apple Intelligence isn't available right now.",
        Failure::NotReady => {
            "Apple Intelligence is still getting ready on this Mac. Try again in a little while."
        }
        Failure::Refused => {
            "Apple's on-device model declined to write this. Try different wording."
        }
        Failure::UnsupportedLanguage => "Apple Intelligence doesn't support this language yet.",
        Failure::TooLong => "That's too long for the on-device model. Select a shorter part.",
        Failure::Busy => "The on-device model is busy. Try again in a moment.",
        Failure::Empty => "There's nothing to write from.",
        Failure::Other => "Couldn't write that.",
    };
    CmdError::new(ErrorCode::Other, text)
}
