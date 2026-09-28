//! Search relevance evaluation for Penguin (dev-only, not shipped).
//!
//! - [`corpus`]: a deterministic synthetic mailbox that loads into a real
//!   `penguin_core::Store`.
//! - [`queries`] + [`judge`]: ~200 labelled queries with graded judgments.
//! - [`retrieve`]: keyword (`Store::search`), Ask, vector and hybrid runs.
//! - [`metrics`] + [`eval`]: nDCG@10, MRR, Recall@10/50, latency, JSON
//!   results and before/after diffs with significance tests.
//! - [`real`]: the same metrics over your own mailbox, read-only, with
//!   judgments you make locally.
//!
//! Methodology and sources: docs/SEARCH-EVAL.md.

pub mod corpus;
pub mod edge_queries;
pub mod eval;
pub mod judge;
pub mod metrics;
pub mod queries;
pub mod real;
pub mod retrieve;
pub mod rng;
pub mod window;
