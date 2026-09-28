//! Writing with the engine: one request, streamed as plain text, and
//! suggested replies, guided into a list.
//!
//! - Every write is plain text with the default guardrails first. A
//!   guardrail violation or a refusal (thrown, or an answer that is only the
//!   stock "I can't help with that") retries once with
//!   `.permissiveContentTransformations`, Apple's mode for transforming
//!   content (it only relaxes `String` output, which this is).
//! - `prompts` go from the most context to the least: when the framework
//!   says a request doesn't fit, the next one is tried; none left = too long.
//! - Suggestions are guided (`ReplyOptions`, schema "replies"), not
//!   streamed; the fallback is permissive plain text in numbered lines.
//!
//! Cancellation is checked while waiting on the model: the engine is told
//! to stop, and the write fails with `Cancelled` at once.

use penguin_core::writing::{
    clean_output, looks_like_refusal, parse_suggestions, strip_signoff, suggestion_text_format,
    tidy_suggestions, SUGGESTION_RESPONSE_TOKENS,
};
use tokio::sync::watch;

use crate::summary::engine::{
    Engine, EngineError, EngineEvent, ErrorKind, GenRequest, SCHEMA_REPLIES,
};
use crate::summary::pipeline::Failure;

/// One write: the instructions, the prompt at decreasing amounts of context
/// (at least one), and the answer's budget.
#[derive(Debug, Clone)]
pub struct WriteJob {
    pub instructions: String,
    pub prompts: Vec<String>,
    pub max_response_tokens: u32,
    pub temperature: f64,
    /// A draft: the writer's name, so a sign-off the model adds anyway is
    /// taken off (the composer keeps the user's signature below the text).
    pub sign_off_name: Option<String>,
}

/// Suggested replies: instructions and prompts as for a write.
#[derive(Debug, Clone)]
pub struct SuggestJob {
    pub instructions: String,
    pub prompts: Vec<String>,
}

/// Temperature for suggestions: some variety between the three.
const SUGGEST_TEMPERATURE: f64 = 0.6;

enum Step {
    TooBig,
    Fail(Failure),
}

impl From<Failure> for Step {
    fn from(f: Failure) -> Self {
        Step::Fail(f)
    }
}

pub type TextProgress<'a> = &'a mut (dyn FnMut(String) + Send);

/// Write (or rewrite) the text, reporting the cleaned text so far.
pub async fn write(
    engine: &dyn Engine,
    job: &WriteJob,
    mut cancel: watch::Receiver<bool>,
    progress: TextProgress<'_>,
) -> Result<String, Failure> {
    for prompt in &job.prompts {
        let req = GenRequest {
            instructions: job.instructions.clone(),
            prompt: prompt.clone(),
            guided: false,
            permissive: false,
            stream: true,
            max_response_tokens: job.max_response_tokens,
            temperature: Some(job.temperature),
            schema: None,
        };
        let signoff = job.sign_off_name.as_deref();
        let mut result = write_once(engine, &req, signoff, &mut cancel, progress).await;
        if matches!(result, Err(Step::Fail(Failure::Refused))) {
            let permissive = GenRequest {
                permissive: true,
                ..req
            };
            result = write_once(engine, &permissive, signoff, &mut cancel, progress).await;
        }
        match result {
            Ok(text) => return Ok(text),
            Err(Step::TooBig) => continue,
            Err(Step::Fail(f)) => return Err(f),
        }
    }
    Err(Failure::TooLong)
}

/// The model's text as the editor gets it.
fn finish(raw: &str, sign_off_name: Option<&str>) -> String {
    let text = clean_output(raw);
    match sign_off_name {
        Some(name) => strip_signoff(&text, name),
        None => text,
    }
}

async fn write_once(
    engine: &dyn Engine,
    req: &GenRequest,
    sign_off_name: Option<&str>,
    cancel: &mut watch::Receiver<bool>,
    progress: TextProgress<'_>,
) -> Result<String, Step> {
    let mut last = String::new();
    let done = run(engine, req, cancel, &mut |v| {
        let text = finish(text_of(v), sign_off_name);
        if !text.is_empty() && text != last {
            last = text.clone();
            progress(text);
        }
    })
    .await?;
    let text = finish(text_of(&done), sign_off_name);
    if text.is_empty() || looks_like_refusal(&text) {
        return Err(Failure::Refused.into());
    }
    Ok(text)
}

/// Up to three short replies to the newest message.
pub async fn suggest(
    engine: &dyn Engine,
    job: &SuggestJob,
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<String>, Failure> {
    for prompt in &job.prompts {
        let guided = GenRequest {
            instructions: job.instructions.clone(),
            prompt: prompt.clone(),
            guided: true,
            permissive: false,
            stream: false,
            max_response_tokens: SUGGESTION_RESPONSE_TOKENS,
            temperature: Some(SUGGEST_TEMPERATURE),
            schema: Some(SCHEMA_REPLIES.into()),
        };
        let mut result = run(engine, &guided, &mut cancel, &mut |_| {})
            .await
            .and_then(|v| non_empty(tidy_suggestions(replies_of(&v))));
        if matches!(result, Err(Step::Fail(Failure::Refused))) {
            let text = GenRequest {
                instructions: format!("{}\n\n{}", job.instructions, suggestion_text_format()),
                guided: false,
                permissive: true,
                schema: None,
                ..guided
            };
            result = run(engine, &text, &mut cancel, &mut |_| {})
                .await
                .and_then(|v| non_empty(parse_suggestions(text_of(&v))));
        }
        match result {
            Ok(r) => return Ok(r),
            Err(Step::TooBig) => continue,
            Err(Step::Fail(f)) => return Err(f),
        }
    }
    Err(Failure::TooLong)
}

fn non_empty(r: Vec<String>) -> Result<Vec<String>, Step> {
    if r.is_empty() {
        Err(Failure::Refused.into())
    } else {
        Ok(r)
    }
}

/// One generation: snapshots to `on_snapshot`, the terminal value back.
async fn run(
    engine: &dyn Engine,
    req: &GenRequest,
    cancel: &mut watch::Receiver<bool>,
    on_snapshot: &mut (dyn FnMut(&serde_json::Value) + Send),
) -> Result<serde_json::Value, Step> {
    if *cancel.borrow() {
        return Err(Failure::Cancelled.into());
    }
    let mut generation = engine.start(req);
    loop {
        let event = tokio::select! {
            e = generation.events.recv() => e,
            _ = cancelled(cancel) => {
                (generation.cancel)();
                return Err(Failure::Cancelled.into());
            }
        };
        match event {
            Some(EngineEvent::Snapshot(v)) => on_snapshot(&v),
            Some(EngineEvent::Done(v)) => return Ok(v),
            Some(EngineEvent::Error(e)) => return Err(step_for(&e)),
            None => return Err(Failure::Other.into()),
        }
    }
}

/// Resolves once cancelled; never if the sender is gone.
async fn cancelled(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn text_of(v: &serde_json::Value) -> &str {
    v.get("text").and_then(|t| t.as_str()).unwrap_or("")
}

fn replies_of(v: &serde_json::Value) -> Vec<String> {
    v.get("replies")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn step_for(e: &EngineError) -> Step {
    match e.code {
        ErrorKind::ContextExceeded => Step::TooBig,
        ErrorKind::Guardrail | ErrorKind::Refusal => Step::Fail(Failure::Refused),
        ErrorKind::UnsupportedLanguage => Step::Fail(Failure::UnsupportedLanguage),
        ErrorKind::AssetsUnavailable => Step::Fail(Failure::NotReady),
        ErrorKind::RateLimited | ErrorKind::ConcurrentRequests | ErrorKind::Timeout => {
            Step::Fail(Failure::Busy)
        }
        ErrorKind::Cancelled => Step::Fail(Failure::Cancelled),
        ErrorKind::Unavailable => Step::Fail(Failure::Unavailable),
        ErrorKind::Decoding | ErrorKind::Other => Step::Fail(Failure::Other),
    }
}
