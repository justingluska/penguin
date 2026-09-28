//! The model seam: one generation at a time, streamed as events. The Apple
//! Foundation Models engine (`apple.rs`, macOS) implements it; elsewhere
//! [`Unavailable`] does; tests use a scripted fake (`tests.rs`).

use penguin_core::{AiAvailability, AiUnavailableReason};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;

/// One generation. Serialized as the Swift bridge's `Request`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GenRequest {
    pub instructions: String,
    pub prompt: String,
    /// Guided generation into the `ThreadDigest` schema; false = plain text
    /// in `summary::text_format_instructions` lines.
    pub guided: bool,
    /// `.permissiveContentTransformations` guardrails (plain text only;
    /// Apple's docs: for other output they act like the default).
    pub permissive: bool,
    /// Report snapshots while generating.
    pub stream: bool,
    pub max_response_tokens: u32,
    pub temperature: Option<f64>,
    /// Which `@Generable` type a guided generation fills: None (or
    /// "digest") = `ThreadDigest`, "replies" = `ReplyOptions` (instant
    /// replies, `{"replies": [...]}`). Ignored for plain text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
}

/// `GenRequest::schema` for suggested replies.
pub const SCHEMA_REPLIES: &str = "replies";
/// `GenRequest::schema` for an Ask question read as a query (`MailQuery`).
pub const SCHEMA_QUERY: &str = "query";

/// What the engine reports. Snapshots and done carry the bridge's JSON: a
/// `summary::Draft` for guided generations, `{"text": …}` for plain text.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    Snapshot(serde_json::Value),
    Done(serde_json::Value),
    Error(EngineError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineError {
    pub code: ErrorKind,
    /// For `ContextExceeded` when the framework measured it (macOS 26.4+):
    /// tokens the request needed, and the window.
    pub tokens: Option<u64>,
    pub limit: Option<u64>,
}

impl EngineError {
    pub fn new(code: ErrorKind) -> Self {
        EngineError {
            code,
            tokens: None,
            limit: None,
        }
    }
}

/// The bridge's error codes (Swift `FM.describe`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    ContextExceeded,
    Guardrail,
    Refusal,
    UnsupportedLanguage,
    AssetsUnavailable,
    RateLimited,
    ConcurrentRequests,
    Decoding,
    Timeout,
    Cancelled,
    Unavailable,
    Other,
}

impl ErrorKind {
    pub fn from_code(code: &str) -> Self {
        match code {
            "contextExceeded" => Self::ContextExceeded,
            "guardrail" => Self::Guardrail,
            "refusal" => Self::Refusal,
            "unsupportedLanguage" => Self::UnsupportedLanguage,
            "assetsUnavailable" => Self::AssetsUnavailable,
            "rateLimited" => Self::RateLimited,
            "concurrentRequests" => Self::ConcurrentRequests,
            "decoding" => Self::Decoding,
            "timeout" => Self::Timeout,
            "cancelled" => Self::Cancelled,
            "unavailable" => Self::Unavailable,
            _ => Self::Other,
        }
    }
}

/// A running generation: its events, ending with exactly one `Done` or
/// `Error`, and a way to stop it.
pub struct Generation {
    pub events: UnboundedReceiver<EngineEvent>,
    pub cancel: Box<dyn Fn() + Send + Sync>,
}

pub trait Engine: Send + Sync {
    fn availability(&self) -> AiAvailability;
    /// Load the model ahead of a request (Apple: when there's a second or
    /// more before it). Cheap to call again.
    fn prewarm(&self, instructions: &str);
    fn start(&self, request: &GenRequest) -> Generation;
}

/// No on-device model on this platform (Linux builds).
pub struct Unavailable;

impl Engine for Unavailable {
    fn availability(&self) -> AiAvailability {
        AiAvailability {
            available: false,
            reason: Some(AiUnavailableReason::UnsupportedPlatform),
            context_tokens: 0,
        }
    }
    fn prewarm(&self, _: &str) {}
    fn start(&self, _: &GenRequest) -> Generation {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = tx.send(EngineEvent::Error(EngineError::new(ErrorKind::Unavailable)));
        Generation {
            events: rx,
            cancel: Box::new(|| {}),
        }
    }
}

/// The engine for this build.
pub fn system() -> Box<dyn Engine> {
    #[cfg(target_os = "macos")]
    {
        Box::new(super::apple::AppleEngine)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(Unavailable)
    }
}
