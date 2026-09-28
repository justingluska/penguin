//! Statement profiler for benchmarks (`--features sql-profile`; never in
//! the app). Every connection the store opens reports each finished
//! statement here through SQLite's `SQLITE_TRACE_PROFILE` hook: its SQL
//! as written (the key), one expanded copy with the bound values (for
//! `EXPLAIN QUERY PLAN`), and how long it ran. `examples/bench_sql.rs`
//! drains it per workload and prints the statements that cost the most.
//! SQLite's profile clock ticks in whole milliseconds on Linux, so one
//! short statement reads 0 ms: compare totals over many runs, and take
//! latencies from the untraced build. A statement's time includes the Rust
//! work done between its steps (a loop over its rows).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Connection, StatementStatus};

/// One statement shape.
#[derive(Debug, Clone, Default)]
pub struct Profiled {
    /// Wall time of each run, in the order they finished.
    pub runs: Vec<Duration>,
    /// The first run's SQL with its parameters bound as literals.
    pub expanded: String,
    /// SQLite's own counters, seen on any run: rows stepped by a full
    /// table scan, sorts, and automatic (transient) indexes built.
    pub fullscan: bool,
    pub sorted: bool,
    pub autoindex: bool,
}

static STATS: Mutex<Option<HashMap<String, Profiled>>> = Mutex::new(None);

fn record(event: TraceEvent<'_>) {
    if let TraceEvent::Profile(stmt, took) = event {
        let sql = stmt.sql().to_string();
        let mut stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
        let entry = stats
            .get_or_insert_with(HashMap::new)
            .entry(sql)
            .or_default();
        if entry.expanded.is_empty() {
            entry.expanded = stmt.expanded_sql().unwrap_or_default();
        }
        entry.runs.push(took);
        entry.fullscan |= stmt.get_status(StatementStatus::FullscanStep) > 0;
        entry.sorted |= stmt.get_status(StatementStatus::Sort) > 0;
        entry.autoindex |= stmt.get_status(StatementStatus::AutoIndex) > 0;
    }
}

/// Report this connection's statements (called by the store on open).
pub(crate) fn install(conn: &Connection) {
    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, Some(record));
}

/// Everything recorded since the last call, by SQL text.
pub fn take() -> HashMap<String, Profiled> {
    STATS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_default()
}
