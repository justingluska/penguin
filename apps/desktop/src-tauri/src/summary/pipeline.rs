//! Summarizing one thread with the engine: plan, map, reduce, retry.
//!
//! 1. Plan the messages into chunks for the model's window
//!    (`summary::plan`, estimated tokens).
//! 2. One chunk: generate the summary, streaming. Several: generate notes on
//!    each (map), then the summary from the notes (reduce), grouping the
//!    notes again when they don't fit one call.
//! 3. If the framework says a request is too big (measured on macOS 26.4+,
//!    or the model's own `exceededContextWindowSize`), plan again with a
//!    smaller budget: by the measured ratio when known, else by 40%.
//! 4. Every generation is guided (`ThreadDigest`) first. A guardrail
//!    violation or refusal retries it as plain text with
//!    `.permissiveContentTransformations`, Apple's mode for transforming
//!    content the default guardrails flag; prose that isn't the line format
//!    is taken as a refusal.
//!
//! Cancellation is checked while waiting on the model: the engine is told
//! to stop, and the summary fails with `Cancelled` at once.

use penguin_core::summary::{
    estimate_tokens, instructions, notes_prompt, pack_notes, parse_text_draft, plan, reduce_prompt,
    render_notes, resolve, summary_prompt, text_format_instructions, Budget, Chunk, Draft,
    SourceMessage, MAX_MAP_CHUNKS, RESPONSE_TOKENS,
};
use penguin_core::{AiSummary, SummaryProgress, SummaryStage};
use tokio::sync::watch;

use super::engine::{Engine, EngineError, EngineEvent, ErrorKind, GenRequest};

/// Everything a summary needs, read from the store up front.
#[derive(Debug, Clone)]
pub struct Job {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
    pub messages: Vec<SourceMessage>,
    pub version: String,
    /// `Fri 2026-09-12`: resolves "by Friday" (and is part of the
    /// instructions, so prewarm must use the same).
    pub today: String,
    pub context_tokens: usize,
    pub now_ms: i64,
}

/// Why a summary didn't happen, for the UI's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// No model on this Mac right now.
    Unavailable,
    /// The model is still being downloaded or set up.
    NotReady,
    /// Apple's guardrails or the model declined, in both modes.
    Refused,
    UnsupportedLanguage,
    /// Still too big after re-planning.
    TooLong,
    /// Rate limited, busy with another request, or timed out.
    Busy,
    /// Nothing to summarize (every message empty after cleaning).
    Empty,
    Cancelled,
    Other,
}

/// Temperature for every call: low, for faithful summaries, but not greedy,
/// so Regenerate can word things differently (greedy output is fixed for a
/// given model version: WWDC25 "Deep dive into the Foundation Models
/// framework").
const TEMPERATURE: f64 = 0.3;

/// Estimated tokens for a prompt's own wording (subject line, framing
/// sentence), on top of the chunk.
const PROMPT_WORDING_TOKENS: usize = 120;

/// Planning attempts after the model says a request didn't fit.
const MAX_ATTEMPTS: usize = 3;

enum Step {
    TooBig {
        tokens: Option<u64>,
        limit: Option<u64>,
    },
    Fail(Failure),
}

impl From<Failure> for Step {
    fn from(f: Failure) -> Self {
        Step::Fail(f)
    }
}

pub type Progress<'a> = &'a mut (dyn FnMut(SummaryProgress) + Send);

pub async fn summarize(
    engine: &dyn Engine,
    job: &Job,
    cancel: watch::Receiver<bool>,
    progress: Progress<'_>,
) -> Result<AiSummary, Failure> {
    if job.messages.is_empty() {
        return Err(Failure::Empty);
    }
    let instr = instructions(&job.today);
    // Budget for the larger of the two instruction sets (the text fallback
    // adds its format), so a fallback never overflows where guided fit.
    let fixed = estimate_tokens(&instr)
        + estimate_tokens(text_format_instructions())
        + PROMPT_WORDING_TOKENS;
    let mut budget = Budget::new(job.context_tokens, fixed);
    let mut ctx = Ctx {
        engine,
        job,
        instr,
        cancel,
        progress,
    };
    for _ in 0..MAX_ATTEMPTS {
        let plan = plan(&job.messages, budget.input_tokens, MAX_MAP_CHUNKS);
        match ctx.run(&plan, budget).await {
            Ok(draft) => {
                let (gist, points, asks) = resolve(&draft, &job.messages);
                if gist.is_empty() && points.is_empty() && asks.is_empty() {
                    return Err(Failure::Refused);
                }
                return Ok(AiSummary {
                    account_id: job.account_id.clone(),
                    thread_id: job.thread_id.clone(),
                    version: job.version.clone(),
                    gist,
                    points,
                    asks,
                    message_count: plan.included,
                    omitted: plan.omitted,
                    created_at: job.now_ms,
                    stale: false,
                });
            }
            Err(Step::Fail(f)) => return Err(f),
            Err(Step::TooBig { tokens, limit }) => {
                let percent = match (tokens, limit) {
                    // Scale to the measured overshoot, with 10% to spare.
                    (Some(t), Some(l)) if t > 0 => ((l * 90 / t) as usize).clamp(30, 90),
                    _ => 60,
                };
                budget = budget.shrunk(percent);
            }
        }
    }
    Err(Failure::TooLong)
}

struct Ctx<'a, 'p> {
    engine: &'a dyn Engine,
    job: &'a Job,
    instr: String,
    cancel: watch::Receiver<bool>,
    progress: Progress<'p>,
}

impl Ctx<'_, '_> {
    async fn run(
        &mut self,
        plan: &penguin_core::summary::Plan,
        budget: Budget,
    ) -> Result<Draft, Step> {
        let subject = self.job.subject.clone();
        if plan.is_single() {
            let prompt = summary_prompt(&subject, &plan.chunks[0]);
            return self.generate(prompt, Some((1, 1))).await;
        }
        let total_last = plan.chunks.last().map(|c| c.last).unwrap_or(0);
        let steps = plan.chunks.len() as u32 + 1;
        let mut notes: Vec<(u32, u32, String)> = Vec::new();
        for (i, chunk) in plan.chunks.iter().enumerate() {
            self.report(SummaryStage::Reading, i as u32 + 1, steps, None);
            let draft = self
                .generate(notes_prompt(&subject, chunk, total_last), None)
                .await?;
            notes.push((chunk.first, chunk.last, render_notes(chunk, &draft)));
        }
        loop {
            let texts: Vec<String> = notes.iter().map(|n| n.2.clone()).collect();
            let groups = pack_notes(&texts, budget.input_tokens);
            if groups.len() <= 1 {
                return self
                    .generate(
                        reduce_prompt(&subject, &texts, plan.omitted),
                        Some((steps, steps)),
                    )
                    .await;
            }
            if groups.len() >= notes.len() {
                // Every note needs a call of its own: grouping can't shrink them.
                return Err(Step::Fail(Failure::TooLong));
            }
            let mut next = Vec::new();
            for g in groups {
                let (first, last) = (notes[g[0]].0, notes[*g.last().unwrap()].1);
                let group: Vec<String> = g.iter().map(|&i| texts[i].clone()).collect();
                let draft = self
                    .generate(reduce_prompt(&subject, &group, 0), None)
                    .await?;
                let span = Chunk {
                    text: String::new(),
                    first,
                    last,
                    tokens: 0,
                };
                next.push((first, last, render_notes(&span, &draft)));
            }
            notes = next;
        }
    }

    fn report(&mut self, stage: SummaryStage, step: u32, steps: u32, partial: Option<&Draft>) {
        let partial = partial.map(|d| {
            let (gist, points, asks) = resolve(d, &self.job.messages);
            AiSummary {
                account_id: self.job.account_id.clone(),
                thread_id: self.job.thread_id.clone(),
                version: self.job.version.clone(),
                gist,
                points,
                asks,
                created_at: self.job.now_ms,
                ..AiSummary::default()
            }
        });
        // A snapshot with nothing readable yet (a source number, no text)
        // changes nothing on screen.
        if partial
            .as_ref()
            .is_some_and(|p| p.gist.is_empty() && p.points.is_empty() && p.asks.is_empty())
        {
            return;
        }
        (self.progress)(SummaryProgress {
            account_id: self.job.account_id.clone(),
            thread_id: self.job.thread_id.clone(),
            stage,
            step,
            steps,
            partial,
        });
    }

    /// One generation, guided first, then the plain-text fallback.
    /// `streaming` = (step, steps) to stream the partial summary as the
    /// Writing stage; None generates quietly (map and intermediate reduce).
    async fn generate(
        &mut self,
        prompt: String,
        streaming: Option<(u32, u32)>,
    ) -> Result<Draft, Step> {
        let guided = GenRequest {
            instructions: self.instr.clone(),
            prompt: prompt.clone(),
            guided: true,
            permissive: false,
            stream: streaming.is_some(),
            max_response_tokens: RESPONSE_TOKENS as u32,
            temperature: Some(TEMPERATURE),
            schema: None,
        };
        match self.run_one(&guided, streaming).await {
            Err(Step::Fail(Failure::Refused)) => {
                let text = GenRequest {
                    instructions: format!("{}\n\n{}", self.instr, text_format_instructions()),
                    guided: false,
                    permissive: true,
                    ..guided
                };
                self.run_one(&text, streaming).await
            }
            other => other,
        }
    }

    async fn run_one(
        &mut self,
        req: &GenRequest,
        streaming: Option<(u32, u32)>,
    ) -> Result<Draft, Step> {
        if *self.cancel.borrow() {
            return Err(Failure::Cancelled.into());
        }
        let mut generation = self.engine.start(req);
        loop {
            let event = tokio::select! {
                e = generation.events.recv() => e,
                _ = cancelled(&mut self.cancel) => {
                    (generation.cancel)();
                    return Err(Failure::Cancelled.into());
                }
            };
            match event {
                Some(EngineEvent::Snapshot(v)) => {
                    if let (Some((step, steps)), Some(d)) = (streaming, parse(req, &v)) {
                        self.report(SummaryStage::Writing, step, steps, Some(&d));
                    }
                }
                Some(EngineEvent::Done(v)) => {
                    // Guided output always parses (every field defaults); a
                    // plain-text answer without the format is a refusal.
                    return parse(req, &v).ok_or(Step::Fail(Failure::Refused));
                }
                Some(EngineEvent::Error(e)) => return Err(step_for(&e)),
                // The engine went away without a terminal event.
                None => return Err(Failure::Other.into()),
            }
        }
    }
}

/// Resolves once the summary is cancelled; never if the sender is gone.
async fn cancelled(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn parse(req: &GenRequest, v: &serde_json::Value) -> Option<Draft> {
    if req.guided {
        serde_json::from_value(v.clone()).ok()
    } else {
        parse_text_draft(v.get("text").and_then(|t| t.as_str()).unwrap_or(""))
    }
}

fn step_for(e: &EngineError) -> Step {
    match e.code {
        ErrorKind::ContextExceeded => Step::TooBig {
            tokens: e.tokens,
            limit: e.limit,
        },
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
