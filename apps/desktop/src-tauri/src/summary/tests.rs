//! The summary pipeline against a scripted engine: streaming, map-reduce,
//! re-planning after a context overflow, the guardrail fallback, refusals
//! and cancellation. Fixtures: fictional people, `.example` domains.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use penguin_core::summary::SourceMessage;
use penguin_core::{AiAvailability, SummaryProgress, SummaryStage};
use serde_json::json;
use tokio::sync::watch;

use super::engine::{Engine, EngineError, EngineEvent, ErrorKind, GenRequest, Generation};
use super::pipeline::{summarize, Failure, Job};

/// Each `start` plays the next script, then `rest` once they run out. A
/// script without a terminal event leaves the generation hanging (for
/// cancellation).
struct Scripted {
    scripts: Mutex<VecDeque<Vec<EngineEvent>>>,
    rest: Option<Vec<EngineEvent>>,
    requests: Mutex<Vec<GenRequest>>,
    cancelled: Arc<AtomicBool>,
    // Keeps hanging generations' senders alive.
    open: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<EngineEvent>>>,
}

impl Scripted {
    fn new(scripts: Vec<Vec<EngineEvent>>) -> Self {
        Scripted {
            scripts: Mutex::new(scripts.into()),
            rest: None,
            requests: Mutex::new(Vec::new()),
            cancelled: Arc::new(AtomicBool::new(false)),
            open: Mutex::new(Vec::new()),
        }
    }
    fn then(mut self, rest: Vec<EngineEvent>) -> Self {
        self.rest = Some(rest);
        self
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
            .or_else(|| self.rest.clone())
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

fn source(n: u32, words: usize) -> SourceMessage {
    SourceMessage {
        number: n,
        message_id: format!("msg-{n}"),
        header: format!("[#{n} · Thu 2026-09-10 09:00 · Maya Lin <maya@northwind.example>]"),
        text: (0..words)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" "),
        preview_only: false,
    }
}

fn job(messages: Vec<SourceMessage>, context_tokens: usize) -> Job {
    Job {
        account_id: "sam@penguin.example".into(),
        thread_id: "t1".into(),
        subject: "Offsite venue".into(),
        messages,
        version: "v1".into(),
        today: "Thu 2026-09-10".into(),
        context_tokens,
        now_ms: 1_789_000_000_000,
    }
}

fn digest(gist: &str, source: i64) -> serde_json::Value {
    json!({
        "points": [{"source": source, "text": format!("point from #{source}")}],
        "asks": [{"source": source, "text": "Confirm the venue", "due": "Friday"}],
        "gist": gist,
    })
}

fn err(code: ErrorKind) -> EngineEvent {
    EngineEvent::Error(EngineError::new(code))
}

async fn run(
    engine: &Scripted,
    job: &Job,
) -> (
    Result<penguin_core::AiSummary, Failure>,
    Vec<SummaryProgress>,
) {
    let (_tx, rx) = watch::channel(false);
    let mut seen = Vec::new();
    let mut progress = |p: SummaryProgress| seen.push(p);
    let r = summarize(engine, job, rx, &mut progress).await;
    (r, seen)
}

#[tokio::test]
async fn a_short_thread_streams_one_guided_summary() {
    let engine = Scripted::new(vec![vec![
        EngineEvent::Snapshot(json!({"points": [{"source": 2}]})),
        EngineEvent::Snapshot(json!({"points": [{"source": 2, "text": "Lake house"}]})),
        EngineEvent::Done(digest("Maya asked Sam to confirm the venue.", 2)),
    ]]);
    let (r, progress) = run(&engine, &job(vec![source(1, 20), source(2, 20)], 4096)).await;
    let s = r.unwrap();
    assert_eq!(s.gist, "Maya asked Sam to confirm the venue.");
    assert_eq!(s.points[0].message_id.as_deref(), Some("msg-2"));
    assert_eq!(s.asks[0].due.as_deref(), Some("Friday"));
    assert_eq!(
        (s.message_count, s.omitted, s.version.as_str()),
        (2, 0, "v1")
    );

    let reqs = engine.requests();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].guided && reqs[0].stream && !reqs[0].permissive);
    assert!(reqs[0]
        .prompt
        .starts_with("Summarize this email conversation."));
    assert!(reqs[0].instructions.contains("Today is Thu 2026-09-10"));
    // The partial with no text yet is dropped; the one with text streams.
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].stage, SummaryStage::Writing);
    assert_eq!(
        progress[0].partial.as_ref().unwrap().points[0].text,
        "Lake house"
    );
}

#[tokio::test]
async fn long_threads_take_notes_then_combine_them() {
    // ~1,800 estimated tokens per message: one per chunk in a 4,096 window.
    let msgs: Vec<_> = (1..=3).map(|n| source(n, 1300)).collect();
    let engine = Scripted::new(vec![
        vec![EngineEvent::Done(digest("Part one.", 1))],
        vec![EngineEvent::Done(digest("Part two.", 2))],
        vec![EngineEvent::Done(digest("Part three.", 3))],
        vec![
            EngineEvent::Snapshot(json!({"points": [{"source": 3, "text": "Final"}]})),
            EngineEvent::Done(digest("The whole thread.", 3)),
        ],
    ]);
    let (r, progress) = run(&engine, &job(msgs, 4096)).await;
    assert_eq!(r.unwrap().gist, "The whole thread.");
    let reqs = engine.requests();
    assert_eq!(reqs.len(), 4);
    for (i, map) in reqs[..3].iter().enumerate() {
        assert!(!map.stream, "map steps don't stream");
        assert!(map.prompt.starts_with(&format!(
            "This is part of a longer email conversation (messages #{n}–#{n} of 3).",
            n = i + 1
        )));
    }
    let reduce = &reqs[3];
    assert!(reduce.stream);
    assert!(reduce
        .prompt
        .contains("Notes on messages #2–#2:\nOverview: Part two.\n- point from #2 (#2)"));
    let stages: Vec<_> = progress
        .iter()
        .map(|p| (p.stage, p.step, p.steps))
        .collect();
    assert_eq!(
        stages,
        vec![
            (SummaryStage::Reading, 1, 4),
            (SummaryStage::Reading, 2, 4),
            (SummaryStage::Reading, 3, 4),
            (SummaryStage::Writing, 4, 4),
        ]
    );
}

#[tokio::test]
async fn a_request_that_does_not_fit_is_planned_again_smaller() {
    let msgs = vec![source(1, 1000), source(2, 1000)];
    // The first plan's first call is measured at 6,000 of 4,096 tokens;
    // every call of the smaller plan succeeds.
    let engine = Scripted::new(vec![vec![EngineEvent::Error(EngineError {
        code: ErrorKind::ContextExceeded,
        tokens: Some(6000),
        limit: Some(4096),
    })]])
    .then(vec![EngineEvent::Done(digest("Done smaller.", 2))]);
    let (r, _) = run(&engine, &job(msgs, 4096)).await;
    assert_eq!(r.unwrap().gist, "Done smaller.");
    let reqs = engine.requests();
    // The retry split the messages into smaller parts: message 1 no longer
    // fits one call.
    assert!(reqs[0].prompt.contains("(messages #1–#1 of 2)"));
    assert!(!reqs[0].prompt.contains("part 1 of"));
    assert!(
        reqs[1].prompt.contains("part 1 of"),
        "{}",
        &reqs[1].prompt[..200]
    );
    assert!(reqs[1].prompt.len() < reqs[0].prompt.len());
}

#[tokio::test]
async fn overflow_on_every_plan_gives_up() {
    let engine = Scripted::new(
        (0..10)
            .map(|_| vec![err(ErrorKind::ContextExceeded)])
            .collect(),
    );
    let (r, _) = run(&engine, &job(vec![source(1, 30)], 4096)).await;
    assert_eq!(r.unwrap_err(), Failure::TooLong);
    assert_eq!(engine.requests().len(), 3, "three plans, then stop");
}

#[tokio::test]
async fn a_guardrail_falls_back_to_permissive_plain_text() {
    let engine = Scripted::new(vec![
        vec![err(ErrorKind::Guardrail)],
        vec![
            EngineEvent::Snapshot(json!({"text": "POINT #1: The invoice is overdue"})),
            EngineEvent::Done(json!({
                "text": "POINT #1: The invoice is overdue.\nASK #1 (due Monday): Pay the invoice\nGIST: A supplier says an invoice is overdue."
            })),
        ],
    ]);
    let (r, progress) = run(&engine, &job(vec![source(1, 20)], 4096)).await;
    let s = r.unwrap();
    assert_eq!(s.gist, "A supplier says an invoice is overdue.");
    assert_eq!(s.asks[0].due.as_deref(), Some("Monday"));
    assert_eq!(s.asks[0].message_id.as_deref(), Some("msg-1"));
    let reqs = engine.requests();
    assert!(reqs[1].permissive && !reqs[1].guided);
    assert!(reqs[1]
        .instructions
        .ends_with(penguin_core::summary::text_format_instructions()));
    assert_eq!(
        progress.last().unwrap().partial.as_ref().unwrap().points[0].text,
        "The invoice is overdue"
    );
}

#[tokio::test]
async fn a_refusal_in_both_modes_is_reported_as_declined() {
    let engine = Scripted::new(vec![
        vec![err(ErrorKind::Refusal)],
        vec![EngineEvent::Done(
            json!({"text": "Sorry, I can't help with that."}),
        )],
    ]);
    let (r, _) = run(&engine, &job(vec![source(1, 20)], 4096)).await;
    assert_eq!(r.unwrap_err(), Failure::Refused);
}

#[tokio::test]
async fn an_empty_answer_is_not_a_summary() {
    let engine = Scripted::new(vec![vec![EngineEvent::Done(
        json!({"points": [], "asks": [], "gist": " "}),
    )]]);
    let (r, _) = run(&engine, &job(vec![source(1, 20)], 4096)).await;
    assert_eq!(r.unwrap_err(), Failure::Refused);
}

#[tokio::test]
async fn framework_errors_map_to_what_the_user_sees() {
    for (kind, failure) in [
        (ErrorKind::UnsupportedLanguage, Failure::UnsupportedLanguage),
        (ErrorKind::AssetsUnavailable, Failure::NotReady),
        (ErrorKind::RateLimited, Failure::Busy),
        (ErrorKind::ConcurrentRequests, Failure::Busy),
        (ErrorKind::Unavailable, Failure::Unavailable),
        (ErrorKind::Decoding, Failure::Other),
    ] {
        let engine = Scripted::new(vec![vec![err(kind)]]);
        let (r, _) = run(&engine, &job(vec![source(1, 20)], 4096)).await;
        assert_eq!(r.unwrap_err(), failure, "{kind:?}");
    }
}

#[tokio::test]
async fn nothing_to_summarize_calls_no_model() {
    let engine = Scripted::new(vec![]);
    let (r, _) = run(&engine, &job(vec![], 4096)).await;
    assert_eq!(r.unwrap_err(), Failure::Empty);
    assert!(engine.requests().is_empty());
}

#[tokio::test]
async fn cancelling_stops_the_engine_and_returns_at_once() {
    // A generation that never finishes.
    let engine = Scripted::new(vec![vec![EngineEvent::Snapshot(json!({"gist": "Half"}))]]);
    let (tx, rx) = watch::channel(false);
    let j = job(vec![source(1, 20)], 4096);
    let mut progress = |_: SummaryProgress| {};
    let run = summarize(&engine, &j, rx, &mut progress);
    let cancel = async {
        tokio::task::yield_now().await;
        tx.send(true).unwrap();
    };
    let (r, _) = tokio::join!(run, cancel);
    assert_eq!(r.unwrap_err(), Failure::Cancelled);
    assert!(
        engine.cancelled.load(Ordering::SeqCst),
        "the engine was told to stop"
    );
}

#[test]
fn engine_error_codes_round_trip() {
    for code in [
        "contextExceeded",
        "guardrail",
        "refusal",
        "unsupportedLanguage",
        "assetsUnavailable",
        "rateLimited",
        "concurrentRequests",
        "decoding",
        "timeout",
        "cancelled",
        "unavailable",
    ] {
        assert_ne!(ErrorKind::from_code(code), ErrorKind::Other, "{code}");
    }
    assert_eq!(ErrorKind::from_code("somethingNew"), ErrorKind::Other);
}

#[test]
fn the_request_json_matches_the_swift_bridge() {
    let r = GenRequest {
        instructions: "i".into(),
        prompt: "p".into(),
        guided: true,
        permissive: false,
        stream: true,
        max_response_tokens: 700,
        temperature: Some(0.3),
        schema: None,
    };
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"instructions": "i", "prompt": "p", "guided": true, "permissive": false, "stream": true, "maxResponseTokens": 700, "temperature": 0.3})
    );
}

#[test]
fn every_unavailable_reason_has_its_own_explanation() {
    use penguin_core::AiUnavailableReason as R;
    let texts: std::collections::HashSet<_> = [
        R::DeviceNotEligible,
        R::AppleIntelligenceNotEnabled,
        R::ModelNotReady,
        R::OsTooOld,
        R::UnsupportedPlatform,
        R::NotBuilt,
        R::Unknown,
    ]
    .into_iter()
    .map(|r| super::unavailable_text(Some(r)))
    .collect();
    assert_eq!(texts.len(), 7);
}
