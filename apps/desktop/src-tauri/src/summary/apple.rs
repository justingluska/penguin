//! Apple Foundation Models through the Swift bridge
//! (`swift/PenguinAI/Sources/PenguinAI/PenguinAI.swift`, built and linked by
//! build.rs on macOS). The C ABI is plain: bytes in, a callback out.
//!
//! Ownership of the callback context: `start` boxes an event sender and
//! hands Swift the raw pointer. Swift calls back any number of snapshots,
//! then exactly one terminal event (done or error), and never again; the
//! terminal event is where the box is freed. If the receiver was dropped
//! (the summary was cancelled), sends just fail quietly.

use std::ffi::c_void;

use penguin_core::summary::DEFAULT_CONTEXT_TOKENS;
use penguin_core::{AiAvailability, AiUnavailableReason};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use super::engine::{Engine, EngineError, EngineEvent, ErrorKind, GenRequest, Generation};

type Callback = extern "C" fn(ctx: *mut c_void, kind: i32, data: *const u8, len: isize);

extern "C" {
    fn penguin_ai_build_features() -> i32;
    fn penguin_ai_availability() -> i32;
    fn penguin_ai_context_size() -> i64;
    fn penguin_ai_prewarm(instructions: *const u8, len: isize);
    fn penguin_ai_generate(request: *const u8, len: isize, ctx: *mut c_void, cb: Callback) -> u64;
    fn penguin_ai_cancel(handle: u64);
}

const EVENT_SNAPSHOT: i32 = 0;
const EVENT_DONE: i32 = 1;

pub struct AppleEngine;

/// What the bridge was compiled with (Settings → Developer, CI smoke test).
pub fn build_features() -> i32 {
    // SAFETY: no arguments, returns a plain integer.
    unsafe { penguin_ai_build_features() }
}

impl Engine for AppleEngine {
    fn availability(&self) -> AiAvailability {
        // SAFETY: no arguments; reads SystemLanguageModel.default.availability.
        let code = unsafe { penguin_ai_availability() };
        let reason = match code {
            0 => None,
            1 => Some(AiUnavailableReason::DeviceNotEligible),
            2 => Some(AiUnavailableReason::AppleIntelligenceNotEnabled),
            3 => Some(AiUnavailableReason::ModelNotReady),
            4 => Some(AiUnavailableReason::OsTooOld),
            5 => Some(AiUnavailableReason::NotBuilt),
            _ => Some(AiUnavailableReason::Unknown),
        };
        let context_tokens = if reason.is_none() {
            // SAFETY: as above.
            let n = unsafe { penguin_ai_context_size() };
            if n > 0 {
                n as u32
            } else {
                DEFAULT_CONTEXT_TOKENS as u32
            }
        } else {
            0
        };
        AiAvailability {
            available: reason.is_none(),
            reason,
            context_tokens,
        }
    }

    fn prewarm(&self, instructions: &str) {
        // SAFETY: the pointer and length describe `instructions`, which
        // outlives the call; Swift copies it before returning.
        unsafe { penguin_ai_prewarm(instructions.as_ptr(), instructions.len() as isize) }
    }

    fn start(&self, request: &GenRequest) -> Generation {
        let (tx, rx) = unbounded_channel();
        let body = match serde_json::to_vec(request) {
            Ok(b) => b,
            Err(_) => {
                let _ = tx.send(EngineEvent::Error(EngineError::new(ErrorKind::Other)));
                return Generation {
                    events: rx,
                    cancel: Box::new(|| {}),
                };
            }
        };
        let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;
        // SAFETY: `body` outlives the call (Swift copies it first); `ctx` is
        // a leaked Box<UnboundedSender> that `on_event` frees on the
        // terminal event, which Swift sends exactly once, possibly before
        // this returns.
        let handle =
            unsafe { penguin_ai_generate(body.as_ptr(), body.len() as isize, ctx, on_event) };
        Generation {
            events: rx,
            cancel: Box::new(move || {
                if handle != 0 {
                    // SAFETY: cancelling an unknown or finished handle is a no-op.
                    unsafe { penguin_ai_cancel(handle) }
                }
            }),
        }
    }
}

extern "C" fn on_event(ctx: *mut c_void, kind: i32, data: *const u8, len: isize) {
    if ctx.is_null() {
        return;
    }
    let bytes: &[u8] = if data.is_null() || len <= 0 {
        &[]
    } else {
        // SAFETY: Swift passes a buffer of `len` bytes valid for this call.
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null);
    let event = match kind {
        EVENT_SNAPSHOT => EngineEvent::Snapshot(value),
        EVENT_DONE => EngineEvent::Done(value),
        _ => EngineEvent::Error(EngineError {
            code: ErrorKind::from_code(value.get("code").and_then(|c| c.as_str()).unwrap_or("")),
            tokens: value.get("tokens").and_then(|t| t.as_u64()),
            limit: value.get("limit").and_then(|t| t.as_u64()),
        }),
    };
    if kind == EVENT_SNAPSHOT {
        // SAFETY: ctx is the live Box<UnboundedSender> from `start`; it is
        // freed only by the terminal event, which comes after every snapshot.
        let tx = unsafe { &*(ctx as *const UnboundedSender<EngineEvent>) };
        let _ = tx.send(event);
    } else {
        // SAFETY: the terminal event: take the Box back and drop it. Swift
        // never calls again with this ctx.
        let tx = unsafe { Box::from_raw(ctx as *mut UnboundedSender<EngineEvent>) };
        let _ = tx.send(event);
    }
}
