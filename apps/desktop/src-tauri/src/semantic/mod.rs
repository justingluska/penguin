//! Search by meaning: the background indexer and the handles search uses.
//!
//! Design, model choice and measurements: docs/SEMANTIC.md. In short:
//!
//! - The model (penguin_semantic::model::DEFAULT) is downloaded once from a
//!   pinned Hugging Face commit into `<data dir>/models/<key>/` and checked
//!   byte for byte (download.rs). Vectors live in `<data dir>/semantic.db`,
//!   separate from the mail database.
//! - One thread ("penguin-semantic") owns the work: download → load →
//!   sweep. A sweep walks the mail database newest first, compares each
//!   page of messages with what semantic.db holds, embeds what's missing or
//!   changed (a body arrived) and deletes vectors of messages that are gone
//!   (deleted, spam, drafts). Its position is saved, so a quit mid-backfill
//!   resumes where it stopped. New mail (a `mail_changed` from sync) runs a
//!   short pass over the messages newer than the last pass first.
//! - Work is throttled by power.rs (paused in Low Power Mode or when hot,
//!   20% of the time on battery) and runs at utility QoS. Queries preempt
//!   indexing inside the embedder, so searching never waits for it.
//! - Memory: the vector index stays loaded while the setting is on (tens of
//!   MB). The model (~300 MB with its tokenizer) is loaded for indexing and
//!   searching and dropped after [`IDLE_UNLOAD`] with neither.
//!
//! **For search**: the indexer fills `AppState::semantic` (the one slot
//! search and Ask read, `penguin_core::SemanticHandles`) once the model has
//! loaded, with `progress` = messages embedded / messages to embed until
//! the first full pass completes (search and Ask use what is embedded so
//! far, newest mail first, and report "indexing"; see
//! `SemanticHandles::status`), then 1.0 for good: new mail is embedded
//! within seconds and doesn't send search back to keywords. The slot's
//! embedder is this module itself: while the model is unloaded (idle), a
//! query gets `ModelUnavailable` at once (search falls back to keywords for
//! that keystroke) and the model starts loading. Off, or no model yet: the
//! slot is empty.

pub mod download;
pub mod power;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use penguin_core::store::SemanticRow;
use penguin_core::Store;
use penguin_semantic::chunk::{chunk_mail, ChunkConfig, MailDoc};
use penguin_semantic::model::{self, ModelSpec};
use penguin_core::SemanticHandles;
use penguin_semantic::{DocInfo, DocKey, Embedder, SemanticIndex, VectorIndex};
use serde::Serialize;

use crate::error::CmdResult;
use crate::state::AppState;

type AppStateRef<'a> = tauri::State<'a, Arc<AppState>>;

/// Rows the sweep compares per step.
const PAGE: usize = 500;
/// Messages embedded per batch (≈ 30 passages).
const MSG_BATCH: usize = 16;
/// A full pass (to catch deletions deep in the archive) at most this often
/// once caught up.
const FULL_PASS_EVERY: Duration = Duration::from_secs(30 * 60);
/// The model is dropped after this long without a search or indexing work.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);

/// `pausedReason` while a new install hasn't finished the Welcome setup.
pub const WAITING_FOR_SETUP: &str = "you finish setting up Penguin";

/// Where the indexer publishes the handles search uses (the app's
/// `AppState::set_semantic` / `set_semantic_progress`).
pub struct Sink {
    pub handles: Box<dyn Fn(Option<SemanticHandles>) + Send + Sync>,
    pub progress: Box<dyn Fn(f32) + Send + Sync>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SemanticIndexState {
    /// The setting is off.
    Off,
    /// Fetching the model (first run only).
    Downloading,
    /// Loading the model and the vectors into memory.
    Loading,
    /// Embedding mail (search by meaning already works for what's done).
    Indexing,
    /// Waiting for better conditions (see `pausedReason`).
    Paused,
    /// Caught up.
    Ready,
    Error,
}

/// `semantic_status`. Mirrored in src/lib/types.ts.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticIndexStatus {
    pub state: SemanticIndexState,
    pub enabled: bool,
    /// Messages with vectors.
    pub indexed: u64,
    /// Messages that should have them (everything but spam and drafts).
    pub total: u64,
    /// Passages (vectors) stored.
    pub chunks: u64,
    pub model: String,
    pub model_id: String,
    pub model_license: String,
    pub download_done: u64,
    pub download_total: u64,
    pub paused_reason: Option<String>,
    pub error: Option<String>,
    /// RAM held by the vector scan structure (not the model).
    pub index_ram_bytes: u64,
    /// Messages embedded per second, recent average (0 when idle).
    pub rate: f64,
}

pub struct Semantic {
    spec: &'static ModelSpec,
    model_dir: PathBuf,
    db_path: PathBuf,
    enabled: AtomicBool,
    /// New mail since the last pass over the newest messages.
    dirty: AtomicBool,
    /// Search asked for the model while it was unloaded.
    want_model: AtomicBool,
    /// The model files passed their checksums in this process.
    verified: AtomicBool,
    /// The model may be downloaded. False on a new install until the user
    /// has seen the choice in the Welcome setup (it finished or was skipped,
    /// or search by meaning was confirmed on there); files already on disk
    /// are used either way. See docs/ONBOARDING.md.
    downloads_allowed: AtomicBool,
    status: Mutex<SemanticIndexStatus>,
    index: RwLock<Option<Arc<SemanticIndex>>>,
    embedder: RwLock<Option<Arc<dyn Embedder>>>,
    /// Last search that asked for the handles, and last indexing batch.
    last_used: Mutex<Instant>,
    wake: (Mutex<bool>, Condvar),
    model_id: String,
    sink: std::sync::OnceLock<Sink>,
    /// Itself, to hand out as the slot's embedder.
    me: std::sync::OnceLock<std::sync::Weak<Semantic>>,
    /// Handles are in the app's slot.
    published: AtomicBool,
}

enum Stop {
    /// The setting was turned off.
    Disabled,
}

enum SweepError {
    Stop(Stop),
    Failed(String),
}

fn failed(e: impl std::fmt::Display) -> SweepError {
    SweepError::Failed(e.to_string())
}

impl Semantic {
    pub fn new(data_dir: &Path, enabled: bool) -> Arc<Semantic> {
        let spec = model::DEFAULT;
        Arc::new(Semantic {
            spec,
            model_dir: data_dir.join("models").join(spec.key),
            db_path: data_dir.join("semantic.db"),
            enabled: AtomicBool::new(enabled),
            dirty: AtomicBool::new(true),
            want_model: AtomicBool::new(false),
            verified: AtomicBool::new(false),
            downloads_allowed: AtomicBool::new(true),
            status: Mutex::new(SemanticIndexStatus {
                state: if enabled { SemanticIndexState::Loading } else { SemanticIndexState::Off },
                enabled,
                indexed: 0,
                total: 0,
                chunks: 0,
                model: spec.display.to_string(),
                model_id: spec.model_id(),
                model_license: spec.license.to_string(),
                download_done: 0,
                download_total: spec.download_bytes(),
                paused_reason: None,
                error: None,
                index_ram_bytes: 0,
                rate: 0.0,
            }),
            index: RwLock::new(None),
            embedder: RwLock::new(None),
            last_used: Mutex::new(Instant::now()),
            wake: (Mutex::new(false), Condvar::new()),
            model_id: spec.model_id(),
            sink: std::sync::OnceLock::new(),
            me: std::sync::OnceLock::new(),
            published: AtomicBool::new(false),
        })
    }

    /// Put the handles in the app's slot (or take them out).
    fn publish(self: &Arc<Self>, on: bool) {
        let Some(sink) = self.sink.get() else {
            return;
        };
        if !on {
            if self.published.swap(false, Ordering::SeqCst) {
                (sink.handles)(None);
            }
            return;
        }
        let Some(index) = self.index.read().unwrap().clone() else {
            return;
        };
        if !self.published.swap(true, Ordering::SeqCst) {
            (sink.handles)(Some(SemanticHandles {
                embedder: self.clone(),
                index,
                progress: Some(self.progress()),
            }));
        }
    }

    /// Share of mail embedded, as search should see it: 1.0 once a full
    /// pass has completed on this semantic.db (see the module docs).
    fn progress(&self) -> f32 {
        let complete = self
            .index
            .read()
            .unwrap()
            .as_ref()
            .and_then(|ix| ix.get_state("complete").ok().flatten())
            .is_some();
        if complete {
            return 1.0;
        }
        let s = self.status();
        if s.total == 0 {
            0.0
        } else {
            (s.indexed as f32 / s.total as f32).min(0.999)
        }
    }

    fn report_progress(&self) {
        if self.published.load(Ordering::SeqCst) {
            if let Some(sink) = self.sink.get() {
                (sink.progress)(self.progress());
            }
        }
    }

    pub fn status(&self) -> SemanticIndexStatus {
        let mut s = self.status.lock().unwrap().clone();
        if let Some(ix) = self.index.read().unwrap().as_ref() {
            s.indexed = ix.message_count() as u64;
            s.chunks = ix.len() as u64;
            s.index_ram_bytes = ix.ram_bytes() as u64;
        }
        s
    }

    /// Settings → Search → "Search by meaning".
    pub fn set_enabled(&self, on: bool) {
        if self.enabled.swap(on, Ordering::SeqCst) != on {
            self.poke();
        }
    }

    /// Allow (or hold back) the one-time model download. The Welcome setup
    /// opens the gate; nothing else downloads the model before it.
    pub fn set_downloads_allowed(&self, allowed: bool) {
        if self.downloads_allowed.swap(allowed, Ordering::SeqCst) != allowed {
            self.poke();
        }
    }

    /// The model still has to be downloaded and that isn't allowed yet.
    fn waiting_for_consent(&self) -> bool {
        !self.downloads_allowed.load(Ordering::SeqCst)
            && !self.verified.load(Ordering::SeqCst)
            && !model::verify_cached(self.spec, &self.model_dir).is_empty()
    }

    /// Mail changed (sync): look at the newest messages soon.
    pub fn mail_changed(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        self.poke();
    }

    /// An account was removed: its vectors go now, not at the next sweep.
    pub fn remove_account(&self, account_id: &str) {
        let index = self.index.read().unwrap().clone();
        if let Some(ix) = index {
            if let Err(e) = ix.remove_account(account_id) {
                tracing::warn!(error = %e, "semantic: removing an account's vectors failed");
            }
        }
    }

    fn poke(&self) {
        let (lock, cv) = &self.wake;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Sleep until poked or `timeout` passes. True if poked.
    fn wait(&self, timeout: Duration) -> bool {
        let (lock, cv) = &self.wake;
        let mut poked = lock.lock().unwrap();
        if !*poked {
            poked = cv.wait_timeout(poked, timeout).unwrap().0;
        }
        std::mem::replace(&mut *poked, false)
    }

    fn set(&self, f: impl FnOnce(&mut SemanticIndexStatus)) {
        f(&mut self.status.lock().unwrap());
    }

    fn set_state(&self, state: SemanticIndexState) {
        self.set(|s| {
            s.state = state;
            if state != SemanticIndexState::Paused {
                s.paused_reason = None;
            }
            if state != SemanticIndexState::Error {
                s.error = None;
            }
            if state != SemanticIndexState::Indexing {
                s.rate = 0.0;
            }
        });
    }

    fn fail(&self, message: String) {
        tracing::warn!(error = %message, "semantic index");
        self.set(|s| {
            s.state = SemanticIndexState::Error;
            s.error = Some(message);
        });
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Start the worker thread; it publishes the search handles to `sink`.
    pub fn spawn(self: &Arc<Self>, store: Store, sink: Sink) {
        let _ = self.sink.set(sink);
        let _ = self.me.set(Arc::downgrade(self));
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("penguin-semantic".into())
            .spawn(move || {
                penguin_semantic::qos::set_utility_qos();
                me.run(store)
            });
        if let Err(e) = spawned {
            self.fail(format!("could not start the indexer: {e}"));
        }
    }

    fn run(self: &Arc<Self>, store: Store) {
        let mut backoff = Duration::from_secs(60);
        loop {
            if !self.is_enabled() {
                // Free the model and the vectors; semantic.db stays on disk.
                self.publish(false);
                let had_model = self.embedder.write().unwrap().take().is_some();
                *self.index.write().unwrap() = None;
                if had_model {
                    crate::memory::release_free_heap("semantic model dropped");
                }
                self.set(|s| s.enabled = false);
                self.set_state(SemanticIndexState::Off);
                self.wait(Duration::from_secs(3600));
                continue;
            }
            self.set(|s| s.enabled = true);
            // A new install: no download until the Welcome setup has asked.
            if self.waiting_for_consent() {
                self.set(|s| {
                    s.state = SemanticIndexState::Paused;
                    s.paused_reason = Some(WAITING_FOR_SETUP.into());
                });
                self.wait(Duration::from_secs(3600));
                continue;
            }
            // The index, and the model files (downloaded on first run), then
            // the handles go into search's slot; the model itself loads
            // when indexing or a search needs it.
            if let Err(e) = self.open_index().and_then(|()| self.ensure_files()) {
                self.fail(e);
                self.wait(backoff);
                backoff = (backoff * 3).min(Duration::from_secs(3600));
                continue;
            }
            self.refresh_total(&store);
            self.publish(true);
            match self.sweep(&store) {
                Ok(()) => {
                    backoff = Duration::from_secs(60);
                    if let Some(ix) = self.index.read().unwrap().clone() {
                        let _ = ix.set_state("complete", "1");
                    }
                    self.set_state(SemanticIndexState::Ready);
                    self.refresh_total(&store);
                    self.report_progress();
                    // A sweep that embedded anything loaded the model and
                    // grew its arena; the arena shrinks after each batch,
                    // and this returns those pages to the OS (memory.rs).
                    if self.embedder.read().unwrap().is_some() {
                        crate::memory::release_free_heap("semantic index caught up");
                    }
                    self.idle(&store);
                }
                Err(SweepError::Stop(Stop::Disabled)) => {}
                Err(SweepError::Failed(m)) => {
                    self.fail(m);
                    self.wait(backoff);
                    backoff = (backoff * 3).min(Duration::from_secs(3600));
                }
            }
        }
    }

    /// Caught up: follow new mail, load the model when search wants it,
    /// drop it when nobody does, until the next full pass is due.
    fn idle(&self, store: &Store) {
        let caught_up = Instant::now();
        while self.is_enabled() && caught_up.elapsed() < FULL_PASS_EVERY {
            // Sleep until something is due: everything else (new mail, a
            // search wanting the model, the setting) pokes. A fixed
            // 60-second tick here woke the process 60 times an hour.
            let poked = self.wait(self.next_due(caught_up));
            if !self.is_enabled() {
                return;
            }
            if self.want_model.swap(false, Ordering::SeqCst) {
                if let Err(e) = self.embedder_for_work() {
                    if let SweepError::Failed(m) = e {
                        self.fail(m);
                    }
                    return;
                }
                self.set_state(SemanticIndexState::Ready);
            }
            if poked && self.dirty.load(Ordering::SeqCst) {
                match self.top_pass(store) {
                    Ok(()) => {
                        self.set_state(SemanticIndexState::Ready);
                        self.refresh_total(store);
                    }
                    Err(SweepError::Stop(Stop::Disabled)) => return,
                    Err(SweepError::Failed(m)) => {
                        self.fail(m);
                        return;
                    }
                }
            }
            let idle_for = self.last_used.lock().unwrap().elapsed();
            if idle_for >= IDLE_UNLOAD && self.embedder.write().unwrap().take().is_some() {
                tracing::info!("semantic: model unloaded after {} min idle", IDLE_UNLOAD.as_secs() / 60);
                crate::memory::release_free_heap("semantic model unloaded");
            }
        }
    }

    /// How long `idle` may sleep: until the next full pass, or until the
    /// loaded model has been idle for `IDLE_UNLOAD`.
    fn next_due(&self, caught_up: Instant) -> Duration {
        let loaded = self.embedder.read().unwrap().is_some();
        let unused_for = loaded.then(|| self.last_used.lock().unwrap().elapsed());
        next_due(caught_up.elapsed(), unused_for)
    }

    fn index_handle(&self) -> Result<Arc<SemanticIndex>, SweepError> {
        self.index
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| failed("the vector index isn't open"))
    }

    /// Open semantic.db (and read its vectors into memory) if not yet open.
    fn open_index(&self) -> Result<(), String> {
        if self.index.read().unwrap().is_some() {
            return Ok(());
        }
        self.set_state(SemanticIndexState::Loading);
        let started = Instant::now();
        let ix = SemanticIndex::open(&self.db_path, &self.spec.model_id(), self.spec.dims, false)
            .map_err(|e| e.to_string())?;
        tracing::info!(
            vectors = ix.len(),
            ms = started.elapsed().as_millis() as u64,
            "semantic index opened"
        );
        *self.index.write().unwrap() = Some(Arc::new(ix));
        Ok(())
    }

    /// The loaded embedder, loading (and on first run downloading) it if
    /// needed.
    fn embedder_for_work(&self) -> Result<Arc<dyn Embedder>, SweepError> {
        if let Some(e) = self.embedder.read().unwrap().clone() {
            return Ok(e);
        }
        let e = self.load_embedder().map_err(SweepError::Failed)?;
        *self.embedder.write().unwrap() = Some(e.clone());
        if let Some(me) = self.me.get().and_then(std::sync::Weak::upgrade) {
            me.publish(true);
        }
        Ok(e)
    }

    /// The model files, downloaded on first use and checked once per run
    /// (hashed only if they changed since they last passed; see
    /// `model::verify_cached`).
    fn ensure_files(&self) -> Result<(), String> {
        let spec = self.spec;
        if !self.verified.load(Ordering::SeqCst) {
            let started = Instant::now();
            let bad = model::verify_cached(spec, &self.model_dir);
            tracing::info!(ms = started.elapsed().as_millis() as u64, ok = bad.is_empty(), "semantic model files checked");
            if !bad.is_empty() {
                self.set_state(SemanticIndexState::Downloading);
                let dir = self.model_dir.clone();
                tauri::async_runtime::block_on(download::ensure(spec, &dir, |done, total| {
                    self.set(|s| {
                        s.download_done = done;
                        s.download_total = total;
                    })
                }))
                .map_err(|e| format!("couldn't download the search model: {e}"))?;
                let bad = model::verify_cached(spec, &self.model_dir);
                if let Some((file, _)) = bad.first() {
                    return Err(format!("the search model failed its checksum ({})", file.path));
                }
            }
            self.verified.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Download (first run), verify, and load the model.
    fn load_embedder(&self) -> Result<Arc<dyn Embedder>, String> {
        let spec = self.spec;
        self.ensure_files()?;
        let previous = self.status.lock().unwrap().state;
        self.set_state(SemanticIndexState::Loading);
        let started = Instant::now();
        // `--features embed-webgpu` tries the GPU (macOS; not in release
        // builds yet: docs/SEMANTIC.md, "Accelerators"). It falls back to
        // the CPU when WebGPU can't start.
        let accel = if cfg!(feature = "embed-webgpu") {
            penguin_semantic::Accel::WebGpu
        } else {
            penguin_semantic::Accel::Cpu
        };
        let embedder = penguin_semantic::OnnxEmbedder::load(
            &self.model_dir,
            spec,
            penguin_semantic::EmbedderOptions { accel, ..Default::default() },
        )
        .map_err(|e| e.to_string())?;
        tracing::info!(
            model = spec.key,
            accel = ?embedder.options().accel,
            ms = started.elapsed().as_millis() as u64,
            "semantic model loaded"
        );
        // OnnxEmbedder::load already handed back the tokenizer parse's
        // freed heap (penguin_semantic::mem); the app's logged release
        // (memory.rs) runs on drop and unload.
        if previous != SemanticIndexState::Downloading {
            self.set_state(previous);
        }
        Ok(Arc::new(embedder))
    }

    fn refresh_total(&self, store: &Store) {
        if let Ok(n) = store.semantic_eligible_count() {
            self.set(|s| s.total = n);
        }
        self.report_progress();
    }

    fn state_i64(ix: &SemanticIndex, key: &str) -> Option<i64> {
        ix.get_state(key).ok().flatten().and_then(|v| v.parse().ok())
    }

    fn newest_rowid(store: &Store) -> Result<i64, SweepError> {
        Ok(store
            .semantic_rows(i64::MAX, 1)
            .map_err(failed)?
            .first()
            .map_or(0, |r| r.rowid))
    }

    /// One full pass, from where the last one stopped (a quit mid-backfill
    /// resumes here) down to the oldest message. When a pass starts, the
    /// newest rowid is remembered as `top`; mail arriving above it is the
    /// top pass's job, run first whenever sync reports changes.
    fn sweep(&self, store: &Store) -> Result<(), SweepError> {
        let ix = self.index_handle()?;
        self.refresh_total(store);
        let mut before = Self::state_i64(&ix, "cursor").unwrap_or(i64::MAX);
        if before == i64::MAX || Self::state_i64(&ix, "top").is_none() {
            let top = Self::newest_rowid(store)?;
            ix.set_state("top", &top.to_string()).map_err(failed)?;
            self.dirty.store(false, Ordering::SeqCst);
        }
        let mut pages = 0usize;
        loop {
            if self.dirty.load(Ordering::SeqCst) {
                self.top_pass(store)?;
            }
            let lo = self.page(store, &ix, before, i64::MIN)?;
            ix.set_state("cursor", &lo.to_string()).map_err(failed)?;
            pages += 1;
            if pages % 20 == 0 {
                self.refresh_total(store);
            }
            if lo == i64::MIN {
                ix.set_state("cursor", &i64::MAX.to_string()).map_err(failed)?;
                return Ok(());
            }
            before = lo;
        }
    }

    /// Mail newer than `top` (arrived since the full pass started or the
    /// last top pass): new mail is searchable by meaning within seconds.
    fn top_pass(&self, store: &Store) -> Result<(), SweepError> {
        self.dirty.store(false, Ordering::SeqCst);
        let ix = self.index_handle()?;
        let Some(top) = Self::state_i64(&ix, "top") else {
            return Ok(());
        };
        let newest = Self::newest_rowid(store)?;
        let floor = top.saturating_add(1);
        let mut before = i64::MAX;
        while before > floor {
            let lo = self.page(store, &ix, before, floor)?;
            if lo <= floor {
                break;
            }
            before = lo;
        }
        if newest > top {
            ix.set_state("top", &newest.to_string()).map_err(failed)?;
        }
        Ok(())
    }

    /// Compare one page of mail rows (`floor <= rowid < before`) with
    /// semantic.db and fix the differences. Returns the page's lower bound
    /// (inclusive): `floor` once the range is exhausted.
    fn page(&self, store: &Store, ix: &SemanticIndex, before: i64, floor: i64) -> Result<i64, SweepError> {
        let mut rows = store.semantic_rows(before, PAGE).map_err(failed)?;
        let full = rows.len() == PAGE;
        rows.retain(|r| r.rowid >= floor);
        let lo = match rows.last() {
            Some(last) if full && rows.len() == PAGE => last.rowid,
            _ => floor,
        };
        let docs = ix.docs_in_range(lo, before).map_err(failed)?;
        let (todo, stale) = diff(&rows, &docs);
        if !stale.is_empty() {
            ix.remove_docs(&stale).map_err(failed)?;
        }
        if !todo.is_empty() {
            self.embed(store, ix, &todo)?;
        }
        Ok(lo)
    }

    /// Embed these messages in batches, throttled.
    fn embed(&self, store: &Store, ix: &SemanticIndex, rowids: &[i64]) -> Result<(), SweepError> {
        let cfg = ChunkConfig::default();
        let mut rate = RateMeter::default();
        for batch in rowids.chunks(MSG_BATCH) {
            let duty = self.wait_for_budget()?;
            let embedder = self.embedder_for_work()?;
            let started = Instant::now();
            let texts = store.semantic_texts(batch).map_err(failed)?;
            let mut passages: Vec<String> = Vec::new();
            let mut spans = Vec::with_capacity(texts.len());
            for t in &texts {
                let doc = MailDoc {
                    subject: &t.subject,
                    from_name: t.from_name.as_deref(),
                    from_email: &t.from_email,
                    authored: &t.authored,
                    filenames: &t.filenames,
                    bulk: t.bulk,
                };
                let chunks = chunk_mail(&doc, &cfg);
                spans.push((passages.len(), chunks.len()));
                passages.extend(chunks);
            }
            let refs: Vec<&str> = passages.iter().map(String::as_str).collect();
            let vectors = embedder.embed_passages(&refs).map_err(failed)?;
            drop(embedder);
            let items: Vec<(DocInfo, Vec<Vec<f32>>)> = texts
                .iter()
                .zip(&spans)
                .map(|(t, &(start, n))| {
                    (
                        DocInfo {
                            account_id: t.account_id.clone(),
                            message_id: t.id.clone(),
                            thread_id: t.thread_id.clone(),
                            date: t.date,
                            src_rowid: t.rowid,
                            sig: t.sig,
                        },
                        vectors[start..start + n].to_vec(),
                    )
                })
                .collect();
            ix.replace_messages(&items).map_err(failed)?;
            let took = started.elapsed();
            *self.last_used.lock().unwrap() = Instant::now();
            rate.add(texts.len(), took);
            self.set(|s| {
                s.state = SemanticIndexState::Indexing;
                s.paused_reason = None;
                s.error = None;
                s.rate = rate.per_sec();
            });
            self.report_progress();
            // Rest so the work takes `duty` of wall time.
            if duty < 1.0 {
                let rest = took.mul_f32((1.0 - duty) / duty);
                if self.wait(rest.min(Duration::from_secs(30))) && !self.is_enabled() {
                    return Err(SweepError::Stop(Stop::Disabled));
                }
            }
        }
        Ok(())
    }

    /// Block while power/thermal conditions say pause; the duty cycle to
    /// run at otherwise.
    fn wait_for_budget(&self) -> Result<f32, SweepError> {
        loop {
            if !self.is_enabled() {
                return Err(SweepError::Stop(Stop::Disabled));
            }
            match power::budget(power::read()) {
                power::Budget::Run { duty } => return Ok(duty),
                power::Budget::Pause(why) => {
                    self.set(|s| {
                        s.state = SemanticIndexState::Paused;
                        s.paused_reason = Some(why.to_string());
                        s.rate = 0.0;
                    });
                    self.wait(Duration::from_secs(30));
                }
            }
        }
    }
}

/// The slot's embedder: the loaded model, or (unloaded after idle) an
/// immediate error that also starts loading it. Never blocks a search.
impl Embedder for Semantic {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn dims(&self) -> usize {
        self.spec.dims
    }
    fn embed_query(&self, text: &str) -> penguin_semantic::Result<Vec<f32>> {
        *self.last_used.lock().unwrap() = Instant::now();
        let loaded = self.embedder.read().unwrap().clone();
        match loaded {
            Some(e) => e.embed_query(text),
            None => {
                self.want_model.store(true, Ordering::SeqCst);
                self.poke();
                Err(penguin_semantic::SemanticError::ModelUnavailable(
                    "the search model is loading".into(),
                ))
            }
        }
    }
    fn embed_passages(&self, texts: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
        let loaded = self.embedder.read().unwrap().clone();
        match loaded {
            Some(e) => e.embed_passages(texts),
            None => Err(penguin_semantic::SemanticError::ModelUnavailable(
                "the search model isn't loaded".into(),
            )),
        }
    }
    fn embed_passages_now(&self, texts: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
        *self.last_used.lock().unwrap() = Instant::now();
        let loaded = self.embedder.read().unwrap().clone();
        match loaded {
            Some(e) => e.embed_passages_now(texts),
            None => Err(penguin_semantic::SemanticError::ModelUnavailable(
                "the search model isn't loaded".into(),
            )),
        }
    }
}

/// `Semantic::next_due` given how long ago the indexer caught up and, if
/// the model is loaded, how long it has gone unused. Never zero, so an
/// overdue deadline can't spin.
fn next_due(since_caught_up: Duration, unused_for: Option<Duration>) -> Duration {
    let full_pass = FULL_PASS_EVERY.saturating_sub(since_caught_up);
    let unload = unused_for.map_or(Duration::MAX, |u| IDLE_UNLOAD.saturating_sub(u));
    full_pass.min(unload).max(Duration::from_millis(100))
}

/// Messages per second over the last few batches.
#[derive(Default)]
struct RateMeter {
    recent: std::collections::VecDeque<(usize, Duration)>,
}

impl RateMeter {
    fn add(&mut self, n: usize, took: Duration) {
        self.recent.push_back((n, took));
        if self.recent.len() > 20 {
            self.recent.pop_front();
        }
    }
    fn per_sec(&self) -> f64 {
        let n: usize = self.recent.iter().map(|x| x.0).sum();
        let t: f64 = self.recent.iter().map(|x| x.1.as_secs_f64()).sum();
        if t > 0.0 {
            n as f64 / t
        } else {
            0.0
        }
    }
}

/// Mail rows vs stored docs over the same rowid range: rows to (re-)embed
/// (missing, moved, or changed signature) and doc ids to delete (no longer
/// in the mail database in this range).
fn diff(rows: &[SemanticRow], docs: &[DocKey]) -> (Vec<i64>, Vec<i64>) {
    let by_key: HashMap<(&str, &str), &DocKey> = docs
        .iter()
        .map(|d| ((d.account_id.as_str(), d.message_id.as_str()), d))
        .collect();
    let mut kept = std::collections::HashSet::new();
    let mut todo = Vec::new();
    for r in rows {
        match by_key.get(&(r.account_id.as_str(), r.id.as_str())) {
            Some(d) if d.src_rowid == r.rowid && d.sig == r.sig => {
                kept.insert(d.id);
            }
            Some(d) => {
                // Re-embedded under the same doc id (replace_messages upserts).
                kept.insert(d.id);
                todo.push(r.rowid);
            }
            None => todo.push(r.rowid),
        }
    }
    let stale = docs.iter().filter(|d| !kept.contains(&d.id)).map(|d| d.id).collect();
    (todo, stale)
}

// ---------- command ----------

/// Settings → Search progress line and debug info. Local, instant.
#[tauri::command]
pub async fn semantic_status(state: AppStateRef<'_>) -> CmdResult<SemanticIndexStatus> {
    Ok(state.semantic_indexer.status())
}

/// Welcome setup → "Search by meaning" confirmed on: start the one-time
/// model download now instead of when the setup ends.
#[tauri::command]
pub async fn start_model_download(state: AppStateRef<'_>) -> CmdResult<()> {
    state.semantic_indexer.set_downloads_allowed(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_install_waits_for_the_welcome_before_downloading() {
        let dir = std::env::temp_dir().join(format!("penguin-semantic-consent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sem = Semantic::new(&dir, true);
        // No model files on disk and no consent yet: hold.
        sem.set_downloads_allowed(false);
        assert!(sem.waiting_for_consent());
        // The setup asked (or was finished or skipped): go.
        sem.set_downloads_allowed(true);
        assert!(!sem.waiting_for_consent());
        // Files that already passed (an existing user's model) never wait.
        sem.set_downloads_allowed(false);
        sem.verified.store(true, Ordering::SeqCst);
        assert!(!sem.waiting_for_consent());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn row(rowid: i64, id: &str, sig: i64) -> SemanticRow {
        SemanticRow { rowid, account_id: "a".into(), id: id.into(), sig }
    }

    fn doc(id: i64, src: i64, mid: &str, sig: i64) -> DocKey {
        DocKey { id, src_rowid: src, account_id: "a".into(), message_id: mid.into(), sig }
    }

    #[test]
    fn diff_finds_new_changed_moved_and_gone() {
        let rows = [row(50, "m5", 0), row(40, "m4", 0), row(30, "m3", 1), row(20, "m2", 0)];
        let docs = [
            doc(1, 50, "m5", 0), // unchanged
            doc(2, 30, "m3", 0), // body arrived: sig 0 → 1
            doc(3, 25, "m2", 0), // date changed: rowid 25 → 20
            doc(4, 10, "gone", 0),
        ];
        let (todo, stale) = diff(&rows, &docs);
        assert_eq!(todo, vec![40, 30, 20]);
        assert_eq!(stale, vec![4]);
    }

    fn msg(id: &str, date: i64, labels: &[&str], body: &str) -> penguin_core::Message {
        penguin_core::Message {
            account_id: "acct@one.example".into(),
            id: id.into(),
            thread_id: format!("t-{id}"),
            date,
            from: penguin_core::Address { name: Some("Rosa Dalisay".into()), email: "rosa@harbor.example".into() },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: format!("Subject {id}"),
            snippet: String::new(),
            body_text: body.into(),
            body_html: None,
            label_ids: labels.iter().map(|s| s.to_string()).collect(),
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    /// The stand-in embedder and an in-memory index in place of the model.
    fn with_stand_ins(sem: &Semantic) {
        *sem.index.write().unwrap() = Some(Arc::new(SemanticIndex::in_memory("hash-bow", 64, false).unwrap()));
        *sem.embedder.write().unwrap() = Some(Arc::new(penguin_semantic::HashEmbedder::new(64)));
    }

    #[test]
    fn sweeps_a_store_into_the_index_and_follows_changes() {
        let store = Store::open_in_memory().unwrap();
        let mut batch: Vec<_> = (0..1200)
            .map(|i| msg(&format!("m{i}"), 1_000_000 + i as i64 * 1000, &["INBOX"], "hello there"))
            .collect();
        batch.push(msg("spam", 5_000_000, &["SPAM"], "buy now"));
        store.upsert_messages(&batch).unwrap();

        let dir = std::env::temp_dir().join(format!("penguin-semantic-app-{}", std::process::id()));
        let sem = Semantic::new(&dir, true);
        with_stand_ins(&sem);
        assert!(sem.sweep(&store).is_ok());
        let ix = sem.index_handle().ok().unwrap();
        assert_eq!(ix.message_count(), 1200);
        assert_eq!(store.semantic_eligible_count().unwrap(), 1200);
        // Cursor reset for the next full pass.
        assert_eq!(ix.get_state("cursor").unwrap(), Some(i64::MAX.to_string()));

        // New mail: the top pass picks it up without a full sweep.
        store.upsert_messages(&[msg("new", 9_000_000, &["INBOX"], "fresh mail")]).unwrap();
        sem.mail_changed();
        assert!(sem.top_pass(&store).is_ok());
        assert_eq!(ix.message_count(), 1201);
        let q = sem.embed_query("fresh mail").unwrap();
        let hits = ix.search_messages(&q, 1, &Default::default()).unwrap();
        assert_eq!(hits[0].chunk.message_id, "new");

        // Deleted and spam: gone after the next full pass.
        store.delete_messages("acct@one.example", &["m5".to_string()]).unwrap();
        store.upsert_messages(&[msg("m6", 1_006_000, &["SPAM"], "hello there")]).unwrap();
        assert!(sem.sweep(&store).is_ok());
        assert_eq!(ix.message_count(), 1199);
        let keys = ix.docs_in_range(i64::MIN, i64::MAX).unwrap();
        assert!(!keys.iter().any(|k| ["m5", "m6", "spam"].contains(&k.message_id.as_str())));

        // Removing an account drops its vectors at once.
        sem.remove_account("acct@one.example");
        assert_eq!(ix.message_count(), 0);
    }

    #[test]
    fn a_sweep_interrupted_midway_resumes_from_its_cursor() {
        let store = Store::open_in_memory().unwrap();
        let batch: Vec<_> = (0..1500)
            .map(|i| msg(&format!("m{i}"), 1_000_000 + i as i64 * 1000, &["INBOX"], "hello"))
            .collect();
        store.upsert_messages(&batch).unwrap();
        let dir = std::env::temp_dir().join(format!("penguin-semantic-resume-{}", std::process::id()));
        let sem = Semantic::new(&dir, true);
        with_stand_ins(&sem);
        let ix = sem.index_handle().ok().unwrap();
        // Two pages by hand, as if the app quit after them.
        ix.set_state("top", &Semantic::newest_rowid(&store).ok().unwrap().to_string()).unwrap();
        let lo = sem.page(&store, &ix, i64::MAX, i64::MIN).ok().unwrap();
        let lo = sem.page(&store, &ix, lo, i64::MIN).ok().unwrap();
        ix.set_state("cursor", &lo.to_string()).unwrap();
        assert_eq!(ix.message_count(), 1000);
        // The oldest 500 are still missing; the sweep continues from there.
        let oldest = ix.docs_in_range(i64::MIN, i64::MAX).unwrap().iter().map(|d| d.src_rowid).min().unwrap();
        assert!(oldest >= lo);
        assert!(sem.sweep(&store).is_ok());
        assert_eq!(ix.message_count(), 1500);
    }

    #[test]
    fn publishes_to_the_slot_and_loads_the_model_on_demand() {
        let dir = std::env::temp_dir().join(format!("penguin-semantic-slot-{}", std::process::id()));
        let sem = Semantic::new(&dir, true);
        with_stand_ins(&sem);
        let slot: Arc<Mutex<Option<SemanticHandles>>> = Arc::default();
        let progress: Arc<Mutex<Vec<f32>>> = Arc::default();
        let (s1, p1) = (slot.clone(), progress.clone());
        let _ = sem.sink.set(Sink {
            handles: Box::new(move |h| *s1.lock().unwrap() = h),
            progress: Box::new(move |p| p1.lock().unwrap().push(p)),
        });
        let _ = sem.me.set(Arc::downgrade(&sem));
        sem.publish(true);
        let h = slot.lock().unwrap().clone().expect("published");
        // Nothing indexed yet, total unknown: indexing, not ready.
        assert_eq!(h.progress, Some(0.0));
        assert_eq!(h.status().0, penguin_core::SemanticStatus::Indexing);
        // The slot's embedder is the lazy one: with the model loaded it embeds…
        assert_eq!(h.embedder.embed_query("x").unwrap().len(), 64);
        // …unloaded (idle), it answers at once with an error and asks for the model.
        *sem.embedder.write().unwrap() = None;
        assert!(h.embedder.embed_query("x").is_err());
        assert!(sem.want_model.load(Ordering::SeqCst));
        // A completed pass reports 1.0 from then on.
        sem.index_handle().ok().unwrap().set_state("complete", "1").unwrap();
        sem.report_progress();
        assert_eq!(progress.lock().unwrap().last(), Some(&1.0));
        // Turning off empties the slot.
        sem.publish(false);
        assert!(slot.lock().unwrap().is_none());
    }

    #[test]
    fn idle_sleeps_until_the_next_deadline() {
        let min = Duration::from_secs(60);
        // No model loaded: only the next full pass is due.
        assert_eq!(next_due(10 * min, None), FULL_PASS_EVERY - 10 * min);
        // Model loaded, unused for 4 minutes: it unloads in 6.
        assert_eq!(next_due(10 * min, Some(4 * min)), IDLE_UNLOAD - 4 * min);
        // Whichever is sooner.
        assert_eq!(next_due(28 * min, Some(4 * min)), FULL_PASS_EVERY - 28 * min);
        // Overdue: a short wait, never zero (no busy loop).
        assert_eq!(next_due(10 * min, Some(20 * min)), Duration::from_millis(100));
        assert_eq!(next_due(FULL_PASS_EVERY, None), Duration::from_millis(100));

        // The method reads the loaded model and its last use.
        let dir = std::env::temp_dir().join(format!("penguin-semantic-due-{}", std::process::id()));
        let sem = Semantic::new(&dir, true);
        let now = Instant::now();
        assert!(sem.next_due(now) > IDLE_UNLOAD);
        with_stand_ins(&sem);
        assert!(sem.next_due(now) <= IDLE_UNLOAD);
    }
}
