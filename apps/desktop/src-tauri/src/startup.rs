//! Launch milestones in penguin.log: milliseconds from the start of
//! `run()` to each step, each logged once per launch (`launch` lines). How
//! to read them, and the numbers measured: docs/PERFORMANCE.md, "The app
//! process".

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static T0: OnceLock<Instant> = OnceLock::new();
static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// Start the clock (first thing in `run()`).
pub fn begin() {
    T0.get_or_init(Instant::now);
}

/// Milliseconds since `begin()` (0 if it wasn't called).
pub fn elapsed_ms() -> u64 {
    T0.get().map_or(0, |t| t.elapsed().as_millis() as u64)
}

/// Log `what` with the time since launch, the first time only.
pub fn mark(what: &'static str) {
    let first = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(what);
    if first {
        tracing::info!(ms = elapsed_ms(), step = what, "launch");
    }
}
