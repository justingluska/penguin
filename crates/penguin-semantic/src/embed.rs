//! The real [`Embedder`]: an ONNX text-embedding model run by ONNX Runtime
//! (the `ort` crate, statically linked, CPU execution provider; WebGPU with
//! the `webgpu` feature) with a Hugging Face `tokenizers` tokenizer.
//!
//! Which model, and why, is in [`crate::model`] and docs/SEMANTIC.md. This
//! file only knows how to run "a BERT-style encoder with a pooling rule":
//! tokenize, pad to the longest in the batch, run, pool (mean / CLS / a
//! pooled output of the graph), cut to the Matryoshka size if the spec asks
//! for one, normalize.
//!
//! **Queries never wait for indexing.** One ONNX session is shared (a
//! second would double the model's memory), and `Session::run` needs
//! exclusive access. A passage batch runs with its own `RunOptions`; a
//! query arriving meanwhile calls `terminate()` on those options, which
//! ONNX Runtime checks between graph nodes, so the batch stops within a few
//! milliseconds, the query runs, and the batch is retried. Background
//! indexing therefore adds no latency to searching. Batches also wait for a
//! quarter second without queries before starting, so typing isn't
//! interrupted over and over; after 10 s of continuous queries a batch runs
//! anyway, in pieces of two passages (the most a query then waits for).
//! Background runs never take the session while a query is waiting for it
//! (the mutex is not fair), and Ask's sentences
//! ([`Embedder::embed_passages_now`]) are treated like a query.
//!
//! **Batching.** Passages are tokenized, sorted by length and packed into
//! runs of at most `batch_tokens` padded tokens (docs/SEMANTIC.md,
//! "Batching").

use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ort::session::builder::GraphOptimizationLevel;
use ort::session::{RunOptions, Session};
use ort::value::TensorRef;
use tokenizers::{Tokenizer, TruncationParams};

use crate::model::{ModelSpec, Pooling};
use crate::{normalize, Embedder, Result, SemanticError};

/// Passage batches wait for this long without a query before starting.
const QUIET: Duration = Duration::from_millis(250);
/// ...but never defer one batch longer than this.
const MAX_DEFER: Duration = Duration::from_secs(10);
/// Passages per run once a batch has been deferred MAX_DEFER.
const FORCED_PIECE: usize = 2;
/// ...and how long each piece lets waiting queries go first (a query takes
/// tens of milliseconds), so queries back to back can't starve indexing.
const FORCED_YIELD: Duration = Duration::from_millis(250);

/// Where the model runs. [`Accel::WebGpu`] needs the `webgpu` cargo
/// feature (macOS builds) and falls back to the CPU when the provider can't
/// be set up; graph nodes it can't take run on the CPU either way. Core ML
/// was measured and isn't used: it has no kernels for this model's quantized
/// and attention operators and ran 2-3x slower (docs/SEMANTIC.md,
/// "Accelerators").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accel {
    /// ONNX Runtime's CPU execution provider.
    #[default]
    Cpu,
    /// WebGPU (Dawn over Metal on macOS).
    WebGpu,
}

impl Accel {
    /// `cpu` or `webgpu`.
    pub fn parse(s: &str) -> Option<Accel> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "cpu" => Accel::Cpu,
            "webgpu" => Accel::WebGpu,
            _ => return None,
        })
    }
}

/// How the session's threads behave and how passages are batched.
#[derive(Debug, Clone, Copy)]
pub struct EmbedderOptions {
    /// ONNX Runtime intra-op threads (the calling thread counts as one).
    pub threads: usize,
    /// macOS: run ONNX Runtime's worker threads at this QoS class, so
    /// background indexing lands on the efficiency cores. Ignored elsewhere.
    pub background_qos: bool,
    /// Most padded tokens (passages × longest passage) in one ONNX run.
    /// Passages are sorted by token count and packed up to this budget, so
    /// a batch of short passages is wide and a batch of long ones narrow.
    pub batch_tokens: usize,
    /// Most passages in one ONNX run.
    pub max_batch: usize,
    /// Let ONNX Runtime's workers spin between operators instead of
    /// sleeping (lower latency, more CPU).
    pub spinning: bool,
    /// Execution provider.
    pub accel: Accel,
}

impl Default for EmbedderOptions {
    fn default() -> Self {
        EmbedderOptions {
            threads: std::thread::available_parallelism()
                .map(|n| (n.get() / 2).clamp(1, 4))
                .unwrap_or(2),
            background_qos: true,
            // Measured (docs/SEMANTIC.md, "Batching"): on the CPU, padding
            // costs as much as real tokens and wide batches buy nothing, so
            // runs stay small and tightly packed. 256 tokens used 20–25%
            // less CPU per passage than the old 16-passage batches on
            // x86-64 and matched them on Apple silicon, where small runs
            // also keep vectors closest to a passage embedded alone.
            batch_tokens: 256,
            max_batch: 64,
            // Spinning made queries faster on Apple silicon (~17 → 9–16 ms
            // p50), but on a busy x86-64 machine it raised the CPU per
            // passage by 40% and per query by 70%: off until its energy
            // cost is measured on a Mac (docs/SEMANTIC.md, "Threads").
            spinning: false,
            accel: Accel::Cpu,
        }
    }
}

/// Token counters since load (for benchmarks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmbedStats {
    /// Passages and queries embedded.
    pub texts: u64,
    /// Real tokens run through the model.
    pub tokens: u64,
    /// Tokens including padding (what the model computed).
    pub padded_tokens: u64,
}

impl EmbedStats {
    pub fn since(&self, earlier: &EmbedStats) -> EmbedStats {
        EmbedStats {
            texts: self.texts - earlier.texts,
            tokens: self.tokens - earlier.tokens,
            padded_tokens: self.padded_tokens - earlier.padded_tokens,
        }
    }
}

/// Split token lengths, already sorted ascending, into runs: consecutive
/// ranges `start..end` where `len × longest ≤ budget` and `len ≤ max`.
/// A single passage longer than the budget still gets its own run.
pub fn plan_batches(sorted_lens: &[usize], budget: usize, max: usize) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < sorted_lens.len() {
        let mut end = start + 1;
        while end < sorted_lens.len()
            && end - start < max.max(1)
            && (end - start + 1) * sorted_lens[end].max(1) <= budget
        {
            end += 1;
        }
        out.push(start..end);
        start = end;
    }
    out
}

pub struct OnnxEmbedder {
    spec: &'static ModelSpec,
    model_id: String,
    opts: EmbedderOptions,
    tokenizer: Tokenizer,
    session: Mutex<Session>,
    has_token_types: bool,
    /// Bumped when a query starts: a passage run that fails while this
    /// changed was interrupted, not broken.
    query_seq: AtomicU64,
    /// When the last query started or finished (ms since `born`).
    last_query_ms: AtomicU64,
    born: Instant,
    /// Options of passage runs; `terminate()` interrupts them.
    passage_run: RunOptions,
    /// Queries and foreground passages waiting for or holding the session.
    /// Background runs don't take the session while this is above zero:
    /// the session's mutex is not fair, and without this an indexer
    /// running pieces back to back could re-take it ahead of a waiting
    /// query again and again (seconds, measured on Apple silicon).
    foreground: AtomicUsize,
    texts: AtomicU64,
    tokens: AtomicU64,
    padded_tokens: AtomicU64,
}

fn embed_err(e: impl std::fmt::Display) -> SemanticError {
    SemanticError::Embed(e.to_string())
}

impl OnnxEmbedder {
    /// Load the model files from `dir` (as laid out by the spec: the ONNX
    /// file and tokenizer.json). The caller verifies checksums first
    /// ([`crate::model::verify`]); this only fails if the files don't load.
    pub fn load(dir: &Path, spec: &'static ModelSpec, opts: EmbedderOptions) -> Result<Self> {
        let mut tokenizer = Tokenizer::from_file(dir.join(spec.tokenizer_file))
            .map_err(|e| SemanticError::ModelUnavailable(format!("tokenizer: {e}")))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: spec.max_tokens,
                ..Default::default()
            }))
            .map_err(|e| SemanticError::ModelUnavailable(format!("tokenizer: {e}")))?;
        tokenizer.with_padding(None);
        // No BPE word cache. In tokenizers 0.23 it lives in thread-locals
        // that are never freed: every thread that tokenizes keeps up to
        // 10,000 words per tokenizer instance, even after the tokenizer is
        // dropped, so each reload after an idle unload left another ~40 MB
        // on the indexer thread (docs/SEMANTIC.md, "Memory"). Without it,
        // tokenizing a passage takes at most ~0.1 ms, against 35–220 ms to
        // embed it.
        let mut model = tokenizer.get_model().clone();
        model.resize_cache(0);
        tokenizer.with_model(model);
        let (session, accel) = match opts.accel {
            Accel::Cpu => (Self::session(dir, spec, &opts, Accel::Cpu)?, Accel::Cpu),
            want => match Self::session(dir, spec, &opts, want) {
                Ok(s) => (s, want),
                Err(e) => {
                    tracing::warn!(error = %e, accel = ?want, "embedder: accelerator unavailable, using the CPU");
                    (Self::session(dir, spec, &opts, Accel::Cpu)?, Accel::Cpu)
                }
            },
        };
        let has_token_types = session.inputs().iter().any(|i| i.name() == "token_type_ids");
        // The tokenizer's JSON parse freed ~190 MB the allocator would keep.
        crate::mem::release_freed();
        // A passage batch grows ONNX Runtime's CPU arena to that batch's
        // peak (+80 MB for 16 passages on the build VM), and the arena keeps
        // it for as long as the model stays loaded. Shrinking it at the end
        // of each passage run hands that back to the allocator; measured no
        // slower (docs/PERFORMANCE.md, "The app process"). Queries are one
        // short text and don't grow it.
        let mut passage_run = RunOptions::new().map_err(embed_err)?;
        passage_run
            .set("memory.enable_memory_arena_shrinkage", "cpu:0")
            .map_err(embed_err)?;
        Ok(OnnxEmbedder {
            spec,
            model_id: spec.model_id(),
            // `accel` is what actually runs (after any fallback).
            opts: EmbedderOptions { accel, ..opts },
            tokenizer,
            session: Mutex::new(session),
            has_token_types,
            query_seq: AtomicU64::new(0),
            last_query_ms: AtomicU64::new(0),
            born: Instant::now(),
            passage_run,
            foreground: AtomicUsize::new(0),
            texts: AtomicU64::new(0),
            tokens: AtomicU64::new(0),
            padded_tokens: AtomicU64::new(0),
        })
    }

    fn session(dir: &Path, spec: &ModelSpec, opts: &EmbedderOptions, accel: Accel) -> Result<Session> {
        let mut builder = Session::builder()
            .map_err(embed_err)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(embed_err)?
            .with_intra_threads(opts.threads.max(1))
            .map_err(embed_err)?
            .with_inter_threads(1)
            .map_err(embed_err)?
            .with_intra_op_spinning(opts.spinning)
            .map_err(embed_err)?
            .with_independent_thread_pool()
            .map_err(embed_err)?;
        if opts.background_qos {
            builder = builder.with_thread_manager(QosThreads).map_err(embed_err)?;
        }
        match accel {
            Accel::Cpu => {}
            #[cfg(feature = "webgpu")]
            Accel::WebGpu => {
                let ep = ort::ep::WebGPU::default().build().error_on_failure();
                builder = builder.with_execution_providers([ep]).map_err(embed_err)?;
            }
            #[allow(unreachable_patterns)]
            other => {
                return Err(SemanticError::ModelUnavailable(format!(
                    "{other:?} isn't compiled into this build"
                )))
            }
        }
        builder
            .commit_from_file(dir.join(spec.onnx_file))
            .map_err(|e| SemanticError::ModelUnavailable(format!("model: {e}")))
    }

    /// Background work waits here until no query is waiting, or `max`.
    fn yield_to_foreground(&self, max: Duration) {
        let since = Instant::now();
        while self.foreground.load(Ordering::SeqCst) > 0 && since.elapsed() < max {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn touch_query(&self) {
        let now = self.born.elapsed().as_millis() as u64;
        self.last_query_ms.store(now.max(1), Ordering::SeqCst);
    }

    fn since_last_query(&self) -> Duration {
        match self.last_query_ms.load(Ordering::SeqCst) {
            0 => Duration::MAX,
            t => self.born.elapsed().saturating_sub(Duration::from_millis(t)),
        }
    }

    pub fn spec(&self) -> &'static ModelSpec {
        self.spec
    }

    pub fn options(&self) -> EmbedderOptions {
        self.opts
    }

    /// Counters since load.
    pub fn stats(&self) -> EmbedStats {
        EmbedStats {
            texts: self.texts.load(Ordering::Relaxed),
            tokens: self.tokens.load(Ordering::Relaxed),
            padded_tokens: self.padded_tokens.load(Ordering::Relaxed),
        }
    }

    /// Token count of `text` as the model sees it (after truncation).
    pub fn token_count(&self, text: &str) -> usize {
        self.tokenizer
            .encode_fast(text, true)
            .map(|e| e.get_ids().len())
            .unwrap_or(0)
    }

    /// Token ids of `text` (prefix already applied), truncated.
    fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        // encode_fast: no character offsets, which nothing here needs.
        Ok(self.tokenizer.encode_fast(text, true).map_err(embed_err)?.get_ids().to_vec())
    }

    /// Embed one batch of tokenized texts, padded to the longest.
    /// `interruptible` = a passage batch that a query may stop.
    fn run_batch(&self, batch: &[&[u32]], interruptible: bool) -> Result<Vec<Vec<f32>>> {
        let b = batch.len();
        let n = batch.iter().map(|ids| ids.len()).max().unwrap_or(0).max(1);
        let mut ids = vec![0i64; b * n];
        let mut mask = vec![0i64; b * n];
        for (i, e) in batch.iter().enumerate() {
            for (j, id) in e.iter().enumerate() {
                ids[i * n + j] = *id as i64;
                mask[i * n + j] = 1;
            }
        }
        let types = vec![0i64; if self.has_token_types { b * n } else { 0 }];
        let shape = [b, n];
        let mut session = self.session.lock().map_err(|_| embed_err("session lock poisoned"))?;
        let ids_t = TensorRef::from_array_view((shape, ids.as_slice())).map_err(embed_err)?;
        let mask_t = TensorRef::from_array_view((shape, mask.as_slice())).map_err(embed_err)?;
        let outputs = if self.has_token_types {
            let types_t = TensorRef::from_array_view((shape, types.as_slice())).map_err(embed_err)?;
            let inputs = ort::inputs![
                "input_ids" => ids_t,
                "attention_mask" => mask_t,
                "token_type_ids" => types_t
            ];
            if interruptible {
                session.run_with_options(inputs, &self.passage_run)
            } else {
                session.run(inputs)
            }
        } else {
            let inputs = ort::inputs!["input_ids" => ids_t, "attention_mask" => mask_t];
            if interruptible {
                session.run_with_options(inputs, &self.passage_run)
            } else {
                session.run(inputs)
            }
        }
        .map_err(embed_err)?;
        let mut out = Vec::with_capacity(b);
        match self.spec.pooling {
            Pooling::Output(name) => {
                let (shape, data) = outputs[name].try_extract_tensor::<f32>().map_err(embed_err)?;
                let h = *shape.last().unwrap_or(&0) as usize;
                for i in 0..b {
                    out.push(data[i * h..(i + 1) * h].to_vec());
                }
            }
            Pooling::Cls | Pooling::Mean => {
                let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(embed_err)?;
                let (seq, h) = (shape[1] as usize, shape[2] as usize);
                for i in 0..b {
                    let row = &data[i * seq * h..(i + 1) * seq * h];
                    if matches!(self.spec.pooling, Pooling::Cls) {
                        out.push(row[..h].to_vec());
                    } else {
                        let mut v = vec![0f32; h];
                        let mut count = 0f32;
                        for t in 0..seq.min(n) {
                            if mask[i * n + t] == 1 {
                                count += 1.0;
                                for (x, y) in v.iter_mut().zip(&row[t * h..(t + 1) * h]) {
                                    *x += y;
                                }
                            }
                        }
                        v.iter_mut().for_each(|x| *x /= count.max(1.0));
                        out.push(v);
                    }
                }
            }
        }
        for v in &mut out {
            v.truncate(self.spec.dims);
            normalize(v);
        }
        drop(outputs);
        drop(session);
        self.texts.fetch_add(b as u64, Ordering::Relaxed);
        self.tokens
            .fetch_add(batch.iter().map(|e| e.len() as u64).sum(), Ordering::Relaxed);
        self.padded_tokens.fetch_add((b * n) as u64, Ordering::Relaxed);
        Ok(out)
    }
}

impl Embedder for OnnxEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dims(&self) -> usize {
        self.spec.dims
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let _fg = Foreground::enter(&self.foreground);
        self.query_seq.fetch_add(1, Ordering::SeqCst);
        self.touch_query();
        // Stop a passage batch in flight; it retries after us.
        let _ = self.passage_run.terminate();
        let ids = self.tokenize(&format!("{}{}", self.spec.query_prefix, text.trim()))?;
        let r = self.run_batch(&[&ids], false);
        self.touch_query();
        Ok(r?.remove(0))
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let (tokens, order, runs) = self.plan(texts)?;
        let mut out: Vec<Vec<f32>> = vec![Vec::new(); texts.len()];
        for run in runs {
            let group = &order[run];
            let batch: Vec<&[u32]> = group.iter().map(|&i| tokens[i].as_slice()).collect();
            let deferring_since = Instant::now();
            let vectors = loop {
                // Defer to typing: wait for a quiet moment with no query.
                // A stream of queries longer than MAX_DEFER no longer stops
                // indexing: the batch then runs in small uninterruptible
                // pieces, so a query waits for at most FORCED_PIECE passages.
                while self.since_last_query() < QUIET && deferring_since.elapsed() < MAX_DEFER {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if deferring_since.elapsed() >= MAX_DEFER {
                    let mut v = Vec::with_capacity(batch.len());
                    for piece in batch.chunks(FORCED_PIECE) {
                        self.yield_to_foreground(FORCED_YIELD);
                        v.extend(self.run_batch(piece, false)?);
                    }
                    break v;
                }
                self.yield_to_foreground(MAX_DEFER.saturating_sub(deferring_since.elapsed()));
                let seq = self.query_seq.load(Ordering::SeqCst);
                let _ = self.passage_run.unterminate();
                // A query that arrived before the unterminate would find
                // its terminate() undone: let it go first. One arriving
                // after it stops this run.
                if self.foreground.load(Ordering::SeqCst) > 0 {
                    continue;
                }
                match self.run_batch(&batch, true) {
                    Ok(v) => break v,
                    // A query started meanwhile and stopped this run: retry.
                    Err(_) if self.query_seq.load(Ordering::SeqCst) != seq => continue,
                    Err(e) => return Err(e),
                }
            };
            for (&i, v) in group.iter().zip(vectors) {
                out[i] = v;
            }
        }
        Ok(out)
    }

    fn embed_passages_now(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let _fg = Foreground::enter(&self.foreground);
        let (tokens, order, runs) = self.plan(texts)?;
        let mut out: Vec<Vec<f32>> = vec![Vec::new(); texts.len()];
        for run in runs {
            // Like a query: stop an indexing batch in flight and keep the
            // indexer deferring until we're done.
            self.query_seq.fetch_add(1, Ordering::SeqCst);
            self.touch_query();
            let _ = self.passage_run.terminate();
            let group = &order[run];
            let batch: Vec<&[u32]> = group.iter().map(|&i| tokens[i].as_slice()).collect();
            for (&i, v) in group.iter().zip(self.run_batch(&batch, false)?) {
                out[i] = v;
            }
        }
        self.touch_query();
        Ok(out)
    }
}

impl OnnxEmbedder {
    /// Tokenize every passage (prompt prefix applied), sort by token
    /// count and pack runs up to the padded-token budget: little padding,
    /// and wide batches of short passages (docs/SEMANTIC.md, "Batching").
    /// Returns the token ids, the sorted order and the runs over it.
    #[allow(clippy::type_complexity)]
    fn plan(&self, texts: &[&str]) -> Result<(Vec<Vec<u32>>, Vec<usize>, Vec<std::ops::Range<usize>>)> {
        let tokens: Vec<Vec<u32>> = texts
            .iter()
            .map(|t| self.tokenize(&format!("{}{}", self.spec.passage_prefix, t)))
            .collect::<Result<_>>()?;
        let mut order: Vec<usize> = (0..texts.len()).collect();
        order.sort_by_key(|&i| tokens[i].len());
        let lens: Vec<usize> = order.iter().map(|&i| tokens[i].len()).collect();
        let runs = plan_batches(&lens, self.opts.batch_tokens, self.opts.max_batch);
        Ok((tokens, order, runs))
    }
}

/// A query or foreground passages in progress (see
/// `OnnxEmbedder::foreground`), counted until dropped.
struct Foreground<'a>(&'a AtomicUsize);

impl<'a> Foreground<'a> {
    fn enter(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Foreground(count)
    }
}

impl Drop for Foreground<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// ONNX Runtime worker threads at a low QoS class (macOS): utility QoS
/// keeps them schedulable on performance cores but below anything the user
/// is doing; the indexer's own thread picks background QoS when on
/// battery (see the app's semantic module).
struct QosThreads;

impl ort::environment::ThreadManager for QosThreads {
    type Thread = std::thread::JoinHandle<()>;

    fn create(&self, work: impl FnOnce() + Send + 'static) -> ort::Result<Self::Thread> {
        std::thread::Builder::new()
            .name("penguin-embed".into())
            .spawn(move || {
                crate::qos::set_utility_qos();
                work()
            })
            .map_err(|e| ort::Error::new(e.to_string()))
    }

    fn join(thread: Self::Thread) -> ort::Result<()> {
        thread
            .join()
            .map_err(|_| ort::Error::new("embedding worker panicked"))
    }
}

#[cfg(test)]
mod tests {
    use super::plan_batches;

    #[test]
    fn batches_pack_sorted_lengths_up_to_the_budget() {
        // 4 × 10 = 40 fits a budget of 40; the fifth would make 5 × 10.
        let lens = [10, 10, 10, 10, 10, 30, 30, 200];
        let runs = plan_batches(&lens, 40, 32);
        assert_eq!(runs, vec![0..4, 4..5, 5..6, 6..7, 7..8]);
        // The passage count cap applies too.
        assert_eq!(plan_batches(&[5; 7], 1000, 3), vec![0..3, 3..6, 6..7]);
        // A passage longer than the budget runs alone.
        assert_eq!(plan_batches(&[500], 40, 32), vec![0..1]);
        assert!(plan_batches(&[], 40, 32).is_empty());
        // Every index exactly once, in order.
        let lens: Vec<usize> = (1..300).map(|i| i % 97 + 1).collect::<Vec<_>>();
        let mut sorted = lens.clone();
        sorted.sort();
        let runs = plan_batches(&sorted, 2048, 32);
        assert_eq!(runs.iter().map(|r| r.len()).sum::<usize>(), sorted.len());
        assert!(runs.windows(2).all(|w| w[0].end == w[1].start));
        for r in &runs {
            assert!(r.len() <= 32);
            assert!(r.len() == 1 || r.len() * sorted[r.end - 1] <= 2048);
        }
    }
}
