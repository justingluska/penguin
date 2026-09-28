//! Local rules & automations (Settings → Rules). OWNER: rules agent.
//!
//! - `model`: rule definitions in `<config dir>/rules.json`.
//! - `engine`: matching (the search language via `Store::match_query`),
//!   idempotent claims, actions, history. Tauri-free and tested with fakes.
//! - `hooks`: shell hooks (argv + JSON on stdin) and signed webhooks.
//! - `schedule`: daily / weekly slots.
//! - `runtime`: the background task, the app's `Effects`, commands.
//!
//! Triggers come from the sync engine (`SyncObserver::messages_added` /
//! `labels_added`, history.list only) through the durable `rule_queue`, so
//! backfill never fires a rule and a crash replays instead of losing events.

pub mod engine;
pub mod hooks;
pub mod model;
pub mod runtime;
pub mod schedule;

pub use runtime::*;
