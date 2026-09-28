//! Writing against a scripted engine: streaming, the permissive fallback,
//! refusals, re-trying with less context, cancellation, suggestions and the
//! request JSON. Fixtures: fictional people, `.example` domains.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use penguin_core::summary::SourceMessage;
use penguin_core::writing::{fit_context, instructions, WriteInput};
use penguin_core::{AiAvailability, WriteAction};
use serde_json::json;
use tokio::sync::watch;

use super::pipeline::{suggest, write, SuggestJob, WriteJob};
use super::{suggestion_prompts, write_prompts, Writer};
use crate::summary::engine::{Engine, EngineError, EngineEvent, ErrorKind, GenRequest, Generation};
use crate::summary::pipeline::Failure;

struct Scripted {
    scripts: Mutex<VecDeque<Vec<EngineEvent>>>,
    requests: Mutex<Vec<GenRequest>>,
    cancelled: Arc<AtomicBool>,
    open: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<EngineEvent>>>,
}

impl Scripted {
    fn new(scripts: Vec<Vec<EngineEvent>>) -> Self {
        Scripted {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
            cancelled: Arc::new(AtomicBool::new(false)),
            open: Mutex::new(Vec::new()),
        }
    }
    fn requests(&self) -> Vec<GenRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Engine for Scripted {
    fn availability(&self) -> AiAvailability {
        AiAvailability {
            available: true,
            reason: None,
            context_tokens: 4096,
        }
    }
    fn prewarm(&self, _: &str) {}
    fn start(&self, request: &GenRequest) -> Generation {
        self.requests.lock().unwrap().push(request.clone());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("more generations than scripted");
        for e in script {
            tx.send(e).unwrap();
        }
        self.open.lock().unwrap().push(tx);
        let flag = self.cancelled.clone();
        Generation {
            events: rx,
            cancel: Box::new(move || flag.store(true, Ordering::SeqCst)),
        }
    }
}

fn job(prompts: &[&str]) -> WriteJob {
    WriteJob {
        instructions: "instr".into(),
        prompts: prompts.iter().map(|p| p.to_string()).collect(),
        max_response_tokens: 600,
        temperature: 0.5,
        sign_off_name: None,
    }
}

fn err(code: ErrorKind) -> EngineEvent {
    EngineEvent::Error(EngineError::new(code))
}

fn never() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    std::mem::forget(tx);
    rx
}

async fn run_write(engine: &Scripted, j: &WriteJob) -> (Result<String, Failure>, Vec<String>) {
    let mut seen = Vec::new();
    let r = write(engine, j, never(), &mut |t| seen.push(t)).await;
    (r, seen)
}

#[tokio::test]
async fn a_write_streams_cleaned_snapshots_and_returns_the_text() {
    let engine = Scripted::new(vec![vec![
        EngineEvent::Snapshot(json!({"text": "Here's a shorter version:\nHi"})),
        EngineEvent::Snapshot(json!({"text": "Here's a shorter version:\nHi"})),
        EngineEvent::Snapshot(json!({"text": "Here's a shorter version:\nHi Maya, **yes**"})),
        EngineEvent::Done(json!({"text": "Here's a shorter version:\nHi Maya, **yes**.\n\n"})),
    ]]);
    let (r, seen) = run_write(&engine, &job(&["p"])).await;
    assert_eq!(r.unwrap(), "Hi Maya, yes.");
    assert_eq!(seen, vec!["Hi", "Hi Maya, yes"]);
    let req = &engine.requests()[0];
    assert!(!req.guided && !req.permissive && req.stream);
    assert_eq!(req.schema, None);
    assert_eq!(req.max_response_tokens, 600);
}

#[tokio::test]
async fn a_draft_loses_the_sign_off_the_model_added() {
    let engine = Scripted::new(vec![vec![
        EngineEvent::Snapshot(json!({"text": "Hi Maya,\n\nYes, Friday works.\n\nBest,"})),
        EngineEvent::Done(json!({"text": "Hi Maya,\n\nYes, Friday works.\n\nBest,\nSam"})),
    ]]);
    let j = WriteJob {
        sign_off_name: Some("Sam Okafor".into()),
        ..job(&["p"])
    };
    let (r, seen) = run_write(&engine, &j).await;
    assert_eq!(r.unwrap(), "Hi Maya,\n\nYes, Friday works.");
    assert_eq!(seen, vec!["Hi Maya,\n\nYes, Friday works."]);
}

#[tokio::test]
async fn a_guardrail_retries_once_permissively() {
    let engine = Scripted::new(vec![
        vec![err(ErrorKind::Guardrail)],
        vec![EngineEvent::Done(
            json!({"text": "The biopsy results are back; all clear."}),
        )],
    ]);
    let (r, _) = run_write(&engine, &job(&["p"])).await;
    assert_eq!(r.unwrap(), "The biopsy results are back; all clear.");
    let reqs = engine.requests();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[1].permissive && !reqs[1].guided);
    assert_eq!(reqs[1].prompt, reqs[0].prompt);
}

#[tokio::test]
async fn a_text_refusal_in_both_modes_is_refused() {
    let engine = Scripted::new(vec![
        vec![EngineEvent::Done(
            json!({"text": "I'm sorry, but I can't assist with that request."}),
        )],
        vec![EngineEvent::Done(json!({"text": "  "}))],
    ]);
    let (r, _) = run_write(&engine, &job(&["p"])).await;
    assert_eq!(r, Err(Failure::Refused));
    assert_eq!(engine.requests().len(), 2);
}

#[tokio::test]
async fn too_big_moves_to_the_prompt_with_less_context() {
    let engine = Scripted::new(vec![
        vec![err(ErrorKind::ContextExceeded)],
        vec![EngineEvent::Done(json!({"text": "Yes, Friday works."}))],
    ]);
    let (r, _) = run_write(&engine, &job(&["big", "small"])).await;
    assert_eq!(r.unwrap(), "Yes, Friday works.");
    let reqs = engine.requests();
    assert_eq!(reqs[0].prompt, "big");
    assert_eq!(reqs[1].prompt, "small");
    let engine = Scripted::new(vec![vec![err(ErrorKind::ContextExceeded)]]);
    assert_eq!(
        run_write(&engine, &job(&["only"])).await.0,
        Err(Failure::TooLong)
    );
}

#[tokio::test]
async fn errors_map_to_failures() {
    for (code, want) in [
        (ErrorKind::RateLimited, Failure::Busy),
        (ErrorKind::UnsupportedLanguage, Failure::UnsupportedLanguage),
        (ErrorKind::AssetsUnavailable, Failure::NotReady),
        (ErrorKind::Unavailable, Failure::Unavailable),
        (ErrorKind::Other, Failure::Other),
    ] {
        let engine = Scripted::new(vec![vec![err(code)]]);
        assert_eq!(
            run_write(&engine, &job(&["p"])).await.0,
            Err(want),
            "{code:?}"
        );
    }
}

#[tokio::test]
async fn cancelling_stops_the_engine() {
    let engine = Scripted::new(vec![vec![EngineEvent::Snapshot(json!({"text": "Hal"}))]]);
    let (tx, rx) = watch::channel(false);
    let j = job(&["p"]);
    let mut sink = |_: String| {};
    let fut = write(&engine, &j, rx, &mut sink);
    let cancel = async {
        tokio::task::yield_now().await;
        tx.send(true).unwrap();
    };
    let (r, _) = tokio::join!(fut, cancel);
    assert_eq!(r, Err(Failure::Cancelled));
    assert!(engine.cancelled.load(Ordering::SeqCst));
}

fn sjob() -> SuggestJob {
    SuggestJob {
        instructions: "instr".into(),
        prompts: vec!["p".into()],
    }
}

#[tokio::test]
async fn suggestions_are_guided_and_tidied() {
    let engine = Scripted::new(vec![vec![EngineEvent::Done(json!({"replies": [
        "Sounds good!", "sounds good", "Could we do Monday instead?", "What time?"
    ]}))]]);
    let r = suggest(&engine, &sjob(), never()).await.unwrap();
    assert_eq!(
        r,
        vec!["Sounds good!", "Could we do Monday instead?", "What time?"]
    );
    let req = &engine.requests()[0];
    assert!(req.guided && !req.stream && !req.permissive);
    assert_eq!(req.schema.as_deref(), Some("replies"));
}

#[tokio::test]
async fn suggestions_fall_back_to_numbered_lines() {
    let engine = Scripted::new(vec![
        vec![err(ErrorKind::Refusal)],
        vec![EngineEvent::Done(
            json!({"text": "1. Yes, that works.\n2. Not this week, sorry.\n3. Who else is coming?"}),
        )],
    ]);
    let r = suggest(&engine, &sjob(), never()).await.unwrap();
    assert_eq!(r.len(), 3);
    let reqs = engine.requests();
    assert!(reqs[1].permissive && !reqs[1].guided && reqs[1].schema.is_none());
    assert!(reqs[1].instructions.contains("1. first reply"));
    // An empty guided list also falls back; nothing usable either way is a refusal.
    let engine = Scripted::new(vec![
        vec![EngineEvent::Done(json!({"replies": []}))],
        vec![EngineEvent::Done(
            json!({"text": "I cannot help with that."}),
        )],
    ]);
    assert_eq!(
        suggest(&engine, &sjob(), never()).await,
        Err(Failure::Refused)
    );
}

#[test]
fn the_replies_request_json_matches_the_swift_bridge() {
    let r = GenRequest {
        instructions: "i".into(),
        prompt: "p".into(),
        guided: true,
        permissive: false,
        stream: false,
        max_response_tokens: 160,
        temperature: Some(0.6),
        schema: Some("replies".into()),
    };
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"instructions": "i", "prompt": "p", "guided": true, "permissive": false, "stream": false, "maxResponseTokens": 160, "temperature": 0.6, "schema": "replies"})
    );
}

fn msgs(n: u32, words: usize) -> Vec<SourceMessage> {
    (1..=n)
        .map(|i| SourceMessage {
            number: i,
            message_id: format!("m{i}"),
            header: format!("[#{i} · Thu 2026-09-10 09:00 · Maya Lin <maya@northwind.example>]"),
            text: "word ".repeat(words).trim().to_string(),
            preview_only: false,
        })
        .collect()
}

#[test]
fn draft_prompts_shrink_their_context() {
    let names = vec!["Maya Lin".to_string()];
    let input = WriteInput {
        action: Some(WriteAction::Draft),
        instruction: "say yes",
        text: "",
        subject: "Offsite",
        recipients: &names,
        my_name: "Sam",
    };
    let instr = instructions(WriteAction::Draft, "Thu 2026-09-10");
    let ctx = msgs(20, 150);
    let prompts = write_prompts(&input, &instr, &ctx, 4096, 600);
    assert_eq!(prompts.len(), 3);
    let count = |p: &str| p.matches("[#").count();
    assert!(count(&prompts[0]) > count(&prompts[1]));
    assert!(count(&prompts[1]) >= 1);
    // The last resort still has the message being answered.
    assert_eq!(count(&prompts[2]), 1);
    assert!(prompts[0].contains("[#20"));
    // A rewrite never carries the conversation.
    let rewrite = WriteInput {
        action: Some(WriteAction::Shorter),
        text: "a long text",
        ..input.clone()
    };
    let p = write_prompts(&rewrite, &instr, &ctx, 4096, 600);
    assert_eq!(p.len(), 1);
    assert!(!p[0].contains("[#"));
    // Nothing to fit: one prompt.
    assert_eq!(write_prompts(&input, &instr, &[], 4096, 600).len(), 1);
    // Context fits the default budget.
    assert!(fit_context(&ctx, 2200).len() < 20);
}

#[test]
fn suggestion_prompts_keep_the_newest_message() {
    let ctx = msgs(12, 200);
    let p = suggestion_prompts("Offsite", "instr", &ctx, 4096);
    assert!(!p.is_empty() && p.len() <= 2);
    assert!(p.iter().all(|x| x.contains("[#12")));
}

#[test]
fn suggestions_are_cached_per_thread_version() {
    let w = Writer::new();
    w.remember("a@x.example", "t1", "v1", &["Yes".into()]);
    assert_eq!(
        w.cached("a@x.example", "t1", "v1"),
        Some(vec!["Yes".to_string()])
    );
    assert_eq!(w.cached("a@x.example", "t1", "v2"), None);
    w.remember("a@x.example", "t1", "v2", &["No".into()]);
    assert_eq!(w.cached("a@x.example", "t1", "v1"), None);
    for i in 0..100 {
        w.remember("a@x.example", &format!("t{i}"), "v", &[]);
    }
    assert!(w.suggestions.lock().unwrap().len() <= super::SUGGESTION_CACHE);
}

#[test]
fn every_failure_reads_differently() {
    let all = [
        Failure::Unavailable,
        Failure::NotReady,
        Failure::Refused,
        Failure::UnsupportedLanguage,
        Failure::TooLong,
        Failure::Busy,
        Failure::Empty,
        Failure::Other,
    ];
    let texts: std::collections::HashSet<String> = all
        .iter()
        .map(|f| super::failure_error(*f).message)
        .collect();
    assert_eq!(texts.len(), all.len());
    assert_eq!(
        super::failure_error(Failure::Cancelled).code,
        crate::error::ErrorCode::Cancelled
    );
}
