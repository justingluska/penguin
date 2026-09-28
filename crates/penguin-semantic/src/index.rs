//! The vector index: `semantic.db` on disk, a compact scan structure in RAM.
//!
//! **On disk** (its own SQLite file next to the mail database, so vectors
//! never cause a migration of the main schema): one `docs` row per embedded
//! message (account, ids, date, the model that embedded it, and the mail
//! DB rowid + signature the indexer compares against), and one `chunks` row
//! per passage holding the int8 codes plus their scale. Chunk row ids are
//! `doc_id << 6 | chunk`, so a message's chunks sit together and need no
//! extra index. Opening with a different model id drops every vector: a
//! query from one model can't be compared with passages from another.
//!
//! **In RAM** per chunk: its vector as int4 (dims/2 bytes, plus a 4-byte
//! scale) and 16 bytes of metadata (doc, chunk, account, date): 148 bytes
//! at 256 dimensions, 89 MB at 600k chunks. A search is
//!
//! 1. a linear scan scoring every chunk that passes the account/date
//!    filter with the int8 query against the int4 codes, keeping the best
//!    `max(k × OVERSAMPLE, MIN_CANDIDATES)`;
//! 2. rescoring those candidates with their int8 vectors (from RAM if
//!    `int8_in_ram`, else read from `semantic.db`), which gives cosine
//!    similarity within ~0.01;
//! 3. the top `k`.
//!
//! Sign bits (binary quantization) were the first design and were measured
//! out: at 256 dimensions a 400-candidate binary pool recovered only 93% of
//! the true top 10 on real EmbeddingGemma vectors, where int4 + int8
//! rescoring of 100 recovers 99%, the same as scanning int8 exhaustively.
//!
//! Filtering happens inside step 1, before the cut, so "the 20 nearest from
//! this account last spring" is exact over that subset, not the 20 global
//! nearest minus the ones that don't match. The numbers behind this design
//! (recall against exact search, latency and memory at 600k chunks) are in
//! docs/SEMANTIC.md.

use std::collections::{BinaryHeap, HashMap};
use std::path::Path;
use std::sync::{Mutex, RwLock};

use rusqlite::{params, Connection, OptionalExtension};

use crate::quant::{dot_i8, dot_i8_i4, pack_i4_from_i8, quantize_i8};
use crate::{ChunkRef, Result, SearchFilter, SemanticError, VectorHit, VectorIndex};

/// Chunks per message the row-id scheme allows (`doc << 6 | chunk`).
pub const MAX_CHUNKS_PER_DOC: u32 = 64;

/// Candidates kept by the int4 pass per result wanted.
const OVERSAMPLE: usize = 4;
/// ...but never fewer than this (small `k` still gets a decent pool). At
/// 600k chunks a pool of 200 recalls 0.961 of the exact top 10, where
/// exhaustive int8 recalls 0.962 (docs/SEMANTIC.md).
const MIN_CANDIDATES: usize = 200;
/// Indexes at least this large are scanned on several threads.
const PARALLEL_FROM: usize = 50_000;
/// A query uses at most this many threads for the scan (a short burst).
const MAX_SCAN_THREADS: usize = 4;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS docs(
    id INTEGER PRIMARY KEY,
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    date INTEGER NOT NULL,
    -- messages.rowid in the mail database (date-ordered) and a signature of
    -- what was embedded (see DocInfo::sig): the indexer's change detection.
    src_rowid INTEGER NOT NULL,
    sig INTEGER NOT NULL,
    model TEXT NOT NULL,
    UNIQUE(account_id, message_id)
);
CREATE INDEX IF NOT EXISTS docs_src ON docs(src_rowid);
CREATE TABLE IF NOT EXISTS chunks(
    id INTEGER PRIMARY KEY,
    v BLOB NOT NULL
);
"#;

/// A message whose passages are stored together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocInfo {
    pub account_id: String,
    pub message_id: String,
    pub thread_id: String,
    pub date: i64,
    /// The mail database rowid of the message (0 = unknown).
    pub src_rowid: i64,
    /// Whatever the indexer needs to notice the message changed (e.g. its
    /// body arrived after a headers-only sync). Opaque here.
    pub sig: i64,
}

/// What the indexer's sweep compares against the mail database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocKey {
    pub id: i64,
    pub src_rowid: i64,
    pub account_id: String,
    pub message_id: String,
    pub sig: i64,
}


#[derive(Clone, Copy)]
struct Row {
    doc: u32,
    chunk: u16,
    acct: u16,
    date: i64,
}

/// A first-pass candidate, ordered so `BinaryHeap` pops the lowest score.
struct Cand {
    score: f32,
    i: usize,
}

impl PartialEq for Cand {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == std::cmp::Ordering::Equal
    }
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.score.total_cmp(&self.score).then(self.i.cmp(&o.i))
    }
}

#[derive(Default)]
struct Mem {
    accounts: Vec<String>,
    rows: Vec<Row>,
    /// `dims / 2` bytes of packed int4 codes per row, and their scales.
    nib: Vec<u8>,
    nib_scale: Vec<f32>,
    /// `dims` codes per row when `int8_in_ram`.
    codes: Vec<i8>,
    scales: Vec<f32>,
    /// Distinct docs with at least one row.
    docs: usize,
}

impl Mem {
    fn acct(&mut self, id: &str) -> u16 {
        match self.accounts.iter().position(|a| a == id) {
            Some(i) => i as u16,
            None => {
                self.accounts.push(id.to_string());
                (self.accounts.len() - 1) as u16
            }
        }
    }
}

pub struct SemanticIndex {
    model_id: String,
    dims: usize,
    /// Bytes of packed int4 codes per row.
    half: usize,
    int8_in_ram: bool,
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
    mem: RwLock<Mem>,
}

fn db_err(e: rusqlite::Error) -> SemanticError {
    SemanticError::Index(e.to_string())
}

fn poisoned<T>(_: T) -> SemanticError {
    SemanticError::Index("index lock poisoned".into())
}

fn encode(codes: &[i8], scale: f32) -> Vec<u8> {
    let mut v = Vec::with_capacity(codes.len() + 4);
    v.extend(codes.iter().map(|c| *c as u8));
    v.extend_from_slice(&scale.to_le_bytes());
    v
}

fn decode(blob: &[u8], dims: usize) -> Option<(&[i8], f32)> {
    if blob.len() != dims + 4 {
        return None;
    }
    // SAFETY: i8 and u8 have the same size, alignment and validity.
    let codes = unsafe { std::slice::from_raw_parts(blob.as_ptr() as *const i8, dims) };
    let scale = f32::from_le_bytes(blob[dims..].try_into().ok()?);
    Some((codes, scale))
}

fn open_conn(path: Option<&Path>) -> Result<Connection> {
    let c = match path {
        Some(p) => Connection::open(p),
        None => Connection::open_in_memory(),
    }
    .map_err(db_err)?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;
         PRAGMA mmap_size=268435456;",
    )
    .map_err(db_err)?;
    Ok(c)
}

impl SemanticIndex {
    /// Open (or create) `semantic.db` at `path` for vectors of `model_id`
    /// with `dims` dimensions, and load it into memory. Vectors stored by
    /// any other model are deleted. Loading reads every chunk once (about a
    /// second at 600k chunks; see docs/SEMANTIC.md), so call it off the UI
    /// thread.
    pub fn open(path: &Path, model_id: &str, dims: usize, int8_in_ram: bool) -> Result<Self> {
        let writer = open_conn(Some(path))?;
        writer.execute_batch(SCHEMA).map_err(db_err)?;
        let reader = open_conn(Some(path))?;
        Self::init(writer, reader, model_id, dims, int8_in_ram)
    }

    /// An index that lives only in memory (tests, benchmarks).
    pub fn in_memory(model_id: &str, dims: usize, int8_in_ram: bool) -> Result<Self> {
        // Two connections to one shared in-memory database.
        let name = format!(
            "file:penguin-semantic-{}-{}?mode=memory&cache=shared",
            std::process::id(),
            MEM_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let writer = Connection::open_with_flags(&name, flags).map_err(db_err)?;
        writer.execute_batch(SCHEMA).map_err(db_err)?;
        let reader = Connection::open_with_flags(&name, flags).map_err(db_err)?;
        Self::init(writer, reader, model_id, dims, int8_in_ram)
    }

    fn init(
        writer: Connection,
        reader: Connection,
        model_id: &str,
        dims: usize,
        int8_in_ram: bool,
    ) -> Result<Self> {
        if dims == 0 {
            return Err(SemanticError::Index("zero dimensions".into()));
        }
        let stored: Option<String> = writer
            .query_row("SELECT value FROM meta WHERE key = 'model'", [], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        let model_key = format!("{model_id}/{dims}");
        if stored.as_deref() != Some(model_key.as_str()) {
            if stored.is_some() {
                tracing::info!("embedding model changed; dropping stored vectors");
            }
            // The indexer's cursor and flags describe the old vectors too.
            writer
                .execute_batch(
                    "BEGIN; DELETE FROM chunks; DELETE FROM docs;
                     DELETE FROM meta WHERE key LIKE 'state.%'; COMMIT;",
                )
                .map_err(db_err)?;
            writer
                .execute(
                    "INSERT OR REPLACE INTO meta(key, value) VALUES ('model', ?1)",
                    [&model_key],
                )
                .map_err(db_err)?;
        }
        let idx = SemanticIndex {
            model_id: model_id.to_string(),
            dims,
            half: dims.div_ceil(2),
            int8_in_ram,
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            mem: RwLock::new(Mem::default()),
        };
        idx.load()?;
        Ok(idx)
    }

    fn load(&self) -> Result<()> {
        let conn = self.reader.lock().map_err(poisoned)?;
        let mut mem = Mem::default();
        let mut stmt = conn
            .prepare(
                "SELECT c.id, c.v, d.account_id, d.date FROM chunks c
                 JOIN docs d ON d.id = c.id >> 6 AND d.model = ?1 ORDER BY c.id",
            )
            .map_err(db_err)?;
        let mut rows = stmt.query([&self.model_id]).map_err(db_err)?;
        let mut nib = Vec::new();
        let mut last_doc = None;
        while let Some(r) = rows.next().map_err(db_err)? {
            let id: i64 = r.get(0).map_err(db_err)?;
            let blob = r.get_ref(1).map_err(db_err)?.as_blob().map_err(|e| {
                SemanticError::Index(format!("chunk {id}: {e}"))
            })?;
            let Some((codes, scale)) = decode(blob, self.dims) else {
                continue;
            };
            let account: String = r.get(2).map_err(db_err)?;
            let date: i64 = r.get(3).map_err(db_err)?;
            let doc = (id >> 6) as u32;
            if last_doc != Some(doc) {
                mem.docs += 1;
                last_doc = Some(doc);
            }
            let acct = mem.acct(&account);
            mem.rows.push(Row { doc, chunk: (id & 63) as u16, acct, date });
            let s4 = pack_i4_from_i8(codes, scale, &mut nib);
            mem.nib.extend_from_slice(&nib);
            mem.nib_scale.push(s4);
            if self.int8_in_ram {
                mem.codes.extend_from_slice(codes);
                mem.scales.push(scale);
            }
        }
        drop(rows);
        drop(stmt);
        // Growth while loading doubled capacities; give the slack back.
        mem.rows.shrink_to_fit();
        mem.nib.shrink_to_fit();
        mem.nib_scale.shrink_to_fit();
        mem.codes.shrink_to_fit();
        mem.scales.shrink_to_fit();
        *self.mem.write().map_err(poisoned)? = mem;
        Ok(())
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Messages with vectors.
    pub fn message_count(&self) -> usize {
        self.mem.read().map(|m| m.docs).unwrap_or(0)
    }

    /// Bytes this index holds in RAM (the scan structure, not SQLite's cache).
    pub fn ram_bytes(&self) -> usize {
        self.mem
            .read()
            .map(|m| {
                m.rows.capacity() * std::mem::size_of::<Row>()
                    + m.nib.capacity()
                    + m.nib_scale.capacity() * 4
                    + m.codes.capacity()
                    + m.scales.capacity() * 4
            })
            .unwrap_or(0)
    }

    /// Replace every passage of these messages (one transaction). Each
    /// entry's vectors are chunk 0, 1, … in order; extra chunks past
    /// [`MAX_CHUNKS_PER_DOC`] are dropped.
    pub fn replace_messages(&self, items: &[(DocInfo, Vec<Vec<f32>>)]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut added: Vec<(u32, u16, String, i64, Vec<i8>, f32)> = Vec::new();
        let mut replaced = std::collections::HashSet::new();
        {
            let mut conn = self.writer.lock().map_err(poisoned)?;
            let tx = conn.transaction().map_err(db_err)?;
            let mut codes = Vec::with_capacity(self.dims);
            for (doc, vectors) in items {
                for v in vectors {
                    if v.len() != self.dims {
                        return Err(SemanticError::Index(format!(
                            "vector has {} dimensions, index has {}",
                            v.len(),
                            self.dims
                        )));
                    }
                }
                let id = upsert_doc(&tx, doc, &self.model_id)?;
                let (id, existed) = id;
                if existed {
                    tx.prepare_cached("DELETE FROM chunks WHERE id >= ?1 AND id < ?2")
                        .map_err(db_err)?
                        .execute(params![id << 6, (id + 1) << 6])
                        .map_err(db_err)?;
                    replaced.insert(id as u32);
                }
                let mut ins = tx
                    .prepare_cached("INSERT INTO chunks(id, v) VALUES (?1, ?2)")
                    .map_err(db_err)?;
                for (i, v) in vectors.iter().take(MAX_CHUNKS_PER_DOC as usize).enumerate() {
                    let scale = quantize_i8(v, &mut codes);
                    ins.execute(params![(id << 6) | i as i64, encode(&codes, scale)])
                        .map_err(db_err)?;
                    added.push((
                        id as u32,
                        i as u16,
                        doc.account_id.clone(),
                        doc.date,
                        codes.clone(),
                        scale,
                    ));
                }
            }
            tx.commit().map_err(db_err)?;
        }
        let mut mem = self.mem.write().map_err(poisoned)?;
        if !replaced.is_empty() {
            self.drop_rows(&mut mem, |r| replaced.contains(&r.doc));
        }
        let mut nib = Vec::new();
        let mut last = None;
        for (doc, chunk, account, date, codes, scale) in added {
            if last != Some(doc) {
                mem.docs += 1;
                last = Some(doc);
            }
            let acct = mem.acct(&account);
            mem.rows.push(Row { doc, chunk, acct, date });
            let s4 = pack_i4_from_i8(&codes, scale, &mut nib);
            mem.nib.extend_from_slice(&nib);
            mem.nib_scale.push(s4);
            if self.int8_in_ram {
                mem.codes.extend_from_slice(&codes);
                mem.scales.push(scale);
            }
        }
        Ok(())
    }

    /// Remove the rows of whole docs matching `dead` from every in-memory
    /// array (one pass).
    fn drop_rows(&self, mem: &mut Mem, dead: impl Fn(&Row) -> bool) {
        let (w, d) = (self.half, self.dims);
        let mut keep = 0;
        let mut docs_gone = std::collections::HashSet::new();
        for i in 0..mem.rows.len() {
            let r = mem.rows[i];
            if dead(&r) {
                docs_gone.insert(r.doc);
                continue;
            }
            if keep != i {
                mem.rows[keep] = r;
                mem.nib.copy_within(i * w..(i + 1) * w, keep * w);
                mem.nib_scale[keep] = mem.nib_scale[i];
                if self.int8_in_ram {
                    mem.codes.copy_within(i * d..(i + 1) * d, keep * d);
                    mem.scales[keep] = mem.scales[i];
                }
            }
            keep += 1;
        }
        mem.rows.truncate(keep);
        mem.nib.truncate(keep * w);
        mem.nib_scale.truncate(keep);
        if self.int8_in_ram {
            mem.codes.truncate(keep * d);
            mem.scales.truncate(keep);
        }
        mem.docs = mem.docs.saturating_sub(docs_gone.len());
    }

    /// Delete these docs (by `DocKey::id`) and their passages.
    pub fn remove_docs(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        {
            let mut conn = self.writer.lock().map_err(poisoned)?;
            let tx = conn.transaction().map_err(db_err)?;
            for id in ids {
                tx.prepare_cached("DELETE FROM chunks WHERE id >= ?1 AND id < ?2")
                    .map_err(db_err)?
                    .execute(params![id << 6, (id + 1) << 6])
                    .map_err(db_err)?;
                tx.prepare_cached("DELETE FROM docs WHERE id = ?1")
                    .map_err(db_err)?
                    .execute([id])
                    .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)?;
        }
        let set: std::collections::HashSet<u32> = ids.iter().map(|i| *i as u32).collect();
        let mut mem = self.mem.write().map_err(poisoned)?;
        self.drop_rows(&mut mem, |r| set.contains(&r.doc));
        Ok(())
    }

    /// Delete everything of one account (it was removed from Penguin).
    pub fn remove_account(&self, account_id: &str) -> Result<()> {
        {
            let mut conn = self.writer.lock().map_err(poisoned)?;
            let tx = conn.transaction().map_err(db_err)?;
            tx.execute(
                "DELETE FROM chunks WHERE (id >> 6) IN (SELECT id FROM docs WHERE account_id = ?1)",
                [account_id],
            )
            .map_err(db_err)?;
            tx.execute("DELETE FROM docs WHERE account_id = ?1", [account_id])
                .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
        }
        let mut mem = self.mem.write().map_err(poisoned)?;
        let Some(acct) = mem.accounts.iter().position(|a| a == account_id) else {
            return Ok(());
        };
        let acct = acct as u16;
        self.drop_rows(&mut mem, |r| r.acct == acct);
        Ok(())
    }

    /// Stored docs with `lo <= src_rowid < hi`, highest first: what the
    /// indexer's sweep merges against the mail database's rows.
    pub fn docs_in_range(&self, lo: i64, hi: i64) -> Result<Vec<DocKey>> {
        let conn = self.reader.lock().map_err(poisoned)?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, src_rowid, account_id, message_id, sig FROM docs
                 WHERE src_rowid >= ?1 AND src_rowid < ?2 ORDER BY src_rowid DESC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![lo, hi], |r| {
                Ok(DocKey {
                    id: r.get(0)?,
                    src_rowid: r.get(1)?,
                    account_id: r.get(2)?,
                    message_id: r.get(3)?,
                    sig: r.get(4)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<std::result::Result<_, _>>().map_err(db_err)
    }

    /// A small key/value store for the indexer (sweep cursor…).
    pub fn get_state(&self, key: &str) -> Result<Option<String>> {
        let conn = self.reader.lock().map_err(poisoned)?;
        conn.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [format!("state.{key}")],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.writer.lock().map_err(poisoned)?;
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
            params![format!("state.{key}"), value],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// The `k` nearest chunks passing `filter` (exact filter, approximate
    /// ranking; see the module docs).
    pub fn search_filtered(
        &self,
        query: &[f32],
        k: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<VectorHit>> {
        let (mut scored, _) = self.scored_candidates(query, k, filter)?;
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(k);
        self.hits(&scored)
    }

    /// Like [`Self::search_filtered`], collapsed to the best chunk of each
    /// message: the `k` nearest messages.
    pub fn search_messages(
        &self,
        query: &[f32],
        k: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<VectorHit>> {
        // Up to ~2 chunks per message on average: oversample accordingly.
        let (mut scored, _) = self.scored_candidates(query, k * 2, filter)?;
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut seen = std::collections::HashSet::new();
        scored.retain(|(r, _)| seen.insert(r.doc));
        scored.truncate(k);
        self.hits(&scored)
    }

    /// Steps 1 and 2: the Hamming pass over the rows passing `filter`,
    /// then int8 cosine for the candidates it kept.
    fn scored_candidates(
        &self,
        query: &[f32],
        k: usize,
        filter: &SearchFilter,
    ) -> Result<(Vec<(Row, f32)>, usize)> {
        if query.len() != self.dims {
            return Err(SemanticError::Index(format!(
                "query has {} dimensions, index has {}",
                query.len(),
                self.dims
            )));
        }
        let mut qcodes = Vec::new();
        let qscale = quantize_i8(query, &mut qcodes);
        // A message allow-list becomes doc ids (one unique-index lookup each).
        let docs: Option<std::collections::HashSet<u32>> = match &filter.messages {
            None => None,
            Some(m) => {
                let conn = self.reader.lock().map_err(poisoned)?;
                let mut look = conn
                    .prepare_cached("SELECT id FROM docs WHERE account_id = ?1 AND message_id = ?2")
                    .map_err(db_err)?;
                let mut set = std::collections::HashSet::new();
                for (account, ids) in m {
                    for id in ids {
                        if let Some(d) = look
                            .query_row(params![account, id], |r| r.get::<_, i64>(0))
                            .optional()
                            .map_err(db_err)?
                        {
                            set.insert(d as u32);
                        }
                    }
                }
                Some(set)
            }
        };
        let mem = self.mem.read().map_err(poisoned)?;
        let want = (k * OVERSAMPLE).max(MIN_CANDIDATES);
        let allowed: Option<Vec<bool>> = filter.account_ids.as_ref().map(|ids| {
            mem.accounts
                .iter()
                .map(|a| ids.iter().any(|i| i == a))
                .collect()
        });
        let (after, before) = (
            filter.after.unwrap_or(i64::MIN),
            filter.before.unwrap_or(i64::MAX),
        );
        let w = self.half;
        // Each thread scans a contiguous range with its own min-heap (root =
        // worst candidate kept); the heaps are merged after.
        let scan = |range: std::ops::Range<usize>| {
            let mut heap: BinaryHeap<Cand> = BinaryHeap::with_capacity(want + 1);
            for i in range {
                let r = &mem.rows[i];
                if r.date < after || r.date >= before {
                    continue;
                }
                if let Some(a) = &allowed {
                    if !a[r.acct as usize] {
                        continue;
                    }
                }
                if let Some(d) = &docs {
                    if !d.contains(&r.doc) {
                        continue;
                    }
                }
                let score = dot_i8_i4(&qcodes, &mem.nib[i * w..(i + 1) * w]) as f32 * mem.nib_scale[i];
                if heap.len() < want {
                    heap.push(Cand { score, i });
                } else if heap.peek().is_some_and(|t| score > t.score) {
                    heap.pop();
                    heap.push(Cand { score, i });
                }
            }
            heap
        };
        let n = mem.rows.len();
        let threads = if n < PARALLEL_FROM {
            1
        } else {
            std::thread::available_parallelism().map_or(1, |p| p.get()).clamp(1, MAX_SCAN_THREADS)
        };
        let mut heap = if threads == 1 {
            scan(0..n)
        } else {
            let per = n.div_ceil(threads);
            let parts: Vec<BinaryHeap<Cand>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..threads)
                    .map(|t| {
                        let scan = &scan;
                        s.spawn(move || scan(t * per..((t + 1) * per).min(n)))
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().expect("scan thread")).collect()
            });
            let mut all = BinaryHeap::with_capacity(want + 1);
            for c in parts.into_iter().flatten() {
                if all.len() < want {
                    all.push(c);
                } else if all.peek().is_some_and(|t: &Cand| c.score > t.score) {
                    all.pop();
                    all.push(c);
                }
            }
            all
        };
        let considered = heap.len();
        heap.shrink_to_fit();
        if self.int8_in_ram {
            let d = self.dims;
            let scored = heap
                .into_iter()
                .map(|Cand { i, .. }| {
                    let codes = &mem.codes[i * d..(i + 1) * d];
                    (mem.rows[i], dot_i8(&qcodes, codes) as f32 * qscale * mem.scales[i])
                })
                .collect();
            return Ok((scored, considered));
        }
        let cands: Vec<Row> = heap.into_iter().map(|c| mem.rows[c.i]).collect();
        drop(mem);
        Ok((self.rescore_from_disk(&cands, &qcodes, qscale)?, considered))
    }

    /// Step 2 when the int8 codes live only in `semantic.db`.
    fn rescore_from_disk(&self, cands: &[Row], qcodes: &[i8], qscale: f32) -> Result<Vec<(Row, f32)>> {
        let d = self.dims;
        let conn = self.reader.lock().map_err(poisoned)?;
        let mut stmt = conn
            .prepare_cached("SELECT v FROM chunks WHERE id = ?1")
            .map_err(db_err)?;
        let mut out = Vec::with_capacity(cands.len());
        for r in cands {
            let id = ((r.doc as i64) << 6) | r.chunk as i64;
            let score = stmt
                .query_row([id], |row| {
                    let b = row.get_ref(0)?.as_blob()?;
                    Ok(decode(b, d).map(|(codes, s)| dot_i8(qcodes, codes) as f32 * qscale * s))
                })
                .optional()
                .map_err(db_err)?
                .flatten();
            // None: removed since the scan.
            if let Some(s) = score {
                out.push((*r, s));
            }
        }
        Ok(out)
    }

    /// Step 3: rows → ChunkRefs (ids from `docs`).
    fn hits(&self, scored: &[(Row, f32)]) -> Result<Vec<VectorHit>> {
        let conn = self.reader.lock().map_err(poisoned)?;
        let mut stmt = conn
            .prepare_cached("SELECT account_id, thread_id, message_id, date FROM docs WHERE id = ?1")
            .map_err(db_err)?;
        let mut cache: HashMap<u32, Option<(String, String, String, i64)>> = HashMap::new();
        let mut out = Vec::with_capacity(scored.len());
        for (r, score) in scored {
            let doc = match cache.get(&r.doc) {
                Some(d) => d.clone(),
                None => {
                    let d = stmt
                        .query_row([r.doc as i64], |x| {
                            Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?))
                        })
                        .optional()
                        .map_err(db_err)?;
                    cache.insert(r.doc, d.clone());
                    d
                }
            };
            // Removed between the scan and now: skip.
            if let Some((account_id, thread_id, message_id, date)) = doc {
                out.push(VectorHit {
                    chunk: ChunkRef { account_id, thread_id, message_id, chunk: r.chunk as u32, date },
                    score: *score,
                });
            }
        }
        Ok(out)
    }
}

static MEM_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Insert or update the doc row; returns (id, whether it existed).
fn upsert_doc(tx: &rusqlite::Transaction, doc: &DocInfo, model: &str) -> Result<(i64, bool)> {
    let existing: Option<i64> = tx
        .prepare_cached("SELECT id FROM docs WHERE account_id = ?1 AND message_id = ?2")
        .map_err(db_err)?
        .query_row(params![doc.account_id, doc.message_id], |r| r.get(0))
        .optional()
        .map_err(db_err)?;
    match existing {
        Some(id) => {
            tx.prepare_cached(
                "UPDATE docs SET thread_id = ?2, date = ?3, src_rowid = ?4, sig = ?5, model = ?6 WHERE id = ?1",
            )
            .map_err(db_err)?
            .execute(params![id, doc.thread_id, doc.date, doc.src_rowid, doc.sig, model])
            .map_err(db_err)?;
            Ok((id, true))
        }
        None => {
            tx.prepare_cached(
                "INSERT INTO docs(account_id, message_id, thread_id, date, src_rowid, sig, model)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(db_err)?
            .execute(params![
                doc.account_id,
                doc.message_id,
                doc.thread_id,
                doc.date,
                doc.src_rowid,
                doc.sig,
                model
            ])
            .map_err(db_err)?;
            let id = tx.last_insert_rowid();
            if id >= (1i64 << 57) {
                return Err(SemanticError::Index("doc id space exhausted".into()));
            }
            Ok((id, false))
        }
    }
}

impl VectorIndex for SemanticIndex {
    /// One chunk at a time (tests, simple callers). The indexer uses
    /// [`SemanticIndex::replace_messages`], which is one transaction per batch.
    fn upsert(&self, chunk: ChunkRef, vector: Vec<f32>) -> Result<()> {
        if chunk.chunk >= MAX_CHUNKS_PER_DOC {
            return Err(SemanticError::Index(format!("chunk {} out of range", chunk.chunk)));
        }
        if vector.len() != self.dims {
            return Err(SemanticError::Index(format!(
                "vector has {} dimensions, index has {}",
                vector.len(),
                self.dims
            )));
        }
        let doc = DocInfo {
            account_id: chunk.account_id.clone(),
            message_id: chunk.message_id.clone(),
            thread_id: chunk.thread_id.clone(),
            date: chunk.date,
            src_rowid: 0,
            sig: 0,
        };
        let mut codes = Vec::new();
        let scale = quantize_i8(&vector, &mut codes);
        let id = {
            let mut conn = self.writer.lock().map_err(poisoned)?;
            let tx = conn.transaction().map_err(db_err)?;
            // Keep the doc's sweep fields if it exists; only add/replace this chunk.
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM docs WHERE account_id = ?1 AND message_id = ?2",
                    params![doc.account_id, doc.message_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_err)?;
            let id = match existing {
                Some(id) => id,
                None => upsert_doc(&tx, &doc, &self.model_id)?.0,
            };
            tx.execute(
                "INSERT OR REPLACE INTO chunks(id, v) VALUES (?1, ?2)",
                params![(id << 6) | chunk.chunk as i64, encode(&codes, scale)],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            id as u32
        };
        let mut mem = self.mem.write().map_err(poisoned)?;
        let c = chunk.chunk as u16;
        let acct = mem.acct(&chunk.account_id);
        let mut nib = Vec::new();
        let s4 = pack_i4_from_i8(&codes, scale, &mut nib);
        let row = Row { doc: id, chunk: c, acct, date: chunk.date };
        let (w, d) = (self.half, self.dims);
        match mem.rows.iter().position(|r| r.doc == id && r.chunk == c) {
            Some(i) => {
                mem.rows[i] = row;
                mem.nib[i * w..(i + 1) * w].copy_from_slice(&nib);
                mem.nib_scale[i] = s4;
                if self.int8_in_ram {
                    mem.codes[i * d..(i + 1) * d].copy_from_slice(&codes);
                    mem.scales[i] = scale;
                }
            }
            None => {
                if !mem.rows.iter().any(|r| r.doc == id) {
                    mem.docs += 1;
                }
                mem.rows.push(row);
                mem.nib.extend_from_slice(&nib);
                mem.nib_scale.push(s4);
                if self.int8_in_ram {
                    mem.codes.extend_from_slice(&codes);
                    mem.scales.push(scale);
                }
            }
        }
        Ok(())
    }

    /// Accounts, dates and a message allow-list, applied inside the scan.
    fn search_where(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Result<Vec<VectorHit>> {
        self.search_filtered(query, k, filter)
    }

    fn remove_message(&self, account_id: &str, message_id: &str) -> Result<()> {
        let id: Option<i64> = self
            .reader
            .lock()
            .map_err(poisoned)?
            .query_row(
                "SELECT id FROM docs WHERE account_id = ?1 AND message_id = ?2",
                params![account_id, message_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        match id {
            Some(id) => self.remove_docs(&[id]),
            None => Ok(()),
        }
    }

    /// Nearest chunks passing an arbitrary predicate. The predicate needs
    /// ids, so it runs after the Hamming pass on a growing candidate pool
    /// (doubling until `k` pass or every chunk was considered): exact, but
    /// slower than [`SemanticIndex::search_filtered`] when it rejects most
    /// chunks. Prefer `search_filtered` for account and date constraints.
    fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn Fn(&ChunkRef) -> bool + Sync)>,
    ) -> Result<Vec<VectorHit>> {
        let Some(f) = filter else {
            return self.search_filtered(query, k, &SearchFilter::default());
        };
        let total = self.len();
        let mut pool = k.max(1) * 4;
        loop {
            let (mut scored, considered) =
                self.scored_candidates(query, pool.min(total.max(1)), &SearchFilter::default())?;
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            let mut hits = self.hits(&scored)?;
            hits.retain(|h| f(&h.chunk));
            if hits.len() >= k || considered >= total {
                hits.truncate(k);
                return Ok(hits);
            }
            pool *= 4;
        }
    }

    fn len(&self) -> usize {
        self.mem.read().map(|m| m.rows.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Embedder, HashEmbedder};

    fn doc(account: &str, id: &str, date: i64) -> DocInfo {
        DocInfo {
            account_id: account.into(),
            message_id: id.into(),
            thread_id: format!("t-{id}"),
            date,
            src_rowid: date * 1024,
            sig: 0,
        }
    }

    fn corpus(e: &HashEmbedder) -> Vec<(DocInfo, Vec<Vec<f32>>)> {
        let texts = [
            ("a", "m1", 1_000, "flight to lisbon confirmed seat 12a"),
            ("a", "m2", 2_000, "quarterly invoice attached for march"),
            ("b", "m3", 3_000, "lisbon hotel booking and flight times"),
            ("b", "m4", 4_000, "team offsite agenda and dinner plans"),
            ("a", "m5", 5_000, "invoice overdue reminder second notice"),
        ];
        texts
            .iter()
            .map(|(a, id, date, t)| {
                let v = e.embed_passages(&[t, "unrelated filler words here"]).unwrap();
                (doc(a, id, *date), v)
            })
            .collect()
    }

    fn both() -> Vec<SemanticIndex> {
        vec![
            SemanticIndex::in_memory("hash-bow", 256, false).unwrap(),
            SemanticIndex::in_memory("hash-bow", 256, true).unwrap(),
        ]
    }

    #[test]
    fn finds_nearest_with_filters_and_collapses_messages() {
        let e = HashEmbedder::new(256);
        for idx in both() {
            idx.replace_messages(&corpus(&e)).unwrap();
            assert_eq!(idx.len(), 10);
            assert_eq!(idx.message_count(), 5);
            let q = e.embed_query("lisbon flight").unwrap();
            let hits = idx.search_messages(&q, 2, &SearchFilter::default()).unwrap();
            let ids: Vec<_> = hits.iter().map(|h| h.chunk.message_id.as_str()).collect();
            assert!(ids.contains(&"m1") && ids.contains(&"m3"), "{ids:?}");
            assert_eq!(hits[0].chunk.chunk, 0);
            assert!(hits[0].score > 0.3 && hits[0].score <= 1.01);
            // Account filter.
            let f = SearchFilter { account_ids: Some(vec!["a".into()]), ..Default::default() };
            let hits = idx.search_messages(&q, 1, &f).unwrap();
            assert_eq!(hits[0].chunk.message_id, "m1");
            // Date filter: [2500, 5000) leaves m3 and m4.
            let f = SearchFilter { after: Some(2_500), before: Some(5_000), ..Default::default() };
            let hits = idx.search_messages(&q, 5, &f).unwrap();
            let mut ids: Vec<_> = hits.iter().map(|h| h.chunk.message_id.clone()).collect();
            ids.sort();
            assert_eq!(ids, vec!["m3", "m4"]);
            // Empty account set = nothing.
            let f = SearchFilter { account_ids: Some(vec![]), ..Default::default() };
            assert!(idx.search_messages(&q, 5, &f).unwrap().is_empty());
            // Chunk-level search returns chunks, filler chunks included.
            let hits = idx.search_filtered(&q, 10, &SearchFilter::default()).unwrap();
            assert_eq!(hits.len(), 10);
        }
    }

    #[test]
    fn replace_remove_and_account_removal() {
        let e = HashEmbedder::new(256);
        for idx in both() {
            idx.replace_messages(&corpus(&e)).unwrap();
            // Re-embedding m1 with one chunk replaces both of its chunks.
            let v = e.embed_passages(&["completely different text about gardening"]).unwrap();
            idx.replace_messages(&[(doc("a", "m1", 1_000), v)]).unwrap();
            assert_eq!(idx.len(), 9);
            assert_eq!(idx.message_count(), 5);
            let q = e.embed_query("gardening").unwrap();
            let hits = idx.search_messages(&q, 1, &SearchFilter::default()).unwrap();
            assert_eq!(hits[0].chunk.message_id, "m1");
            idx.remove_message("a", "m1").unwrap();
            assert_eq!(idx.len(), 8);
            assert_eq!(idx.message_count(), 4);
            idx.remove_account("b").unwrap();
            assert_eq!(idx.len(), 4);
            assert_eq!(idx.message_count(), 2);
            let keys = idx.docs_in_range(0, i64::MAX).unwrap();
            let ids: Vec<_> = keys.iter().map(|k| k.message_id.as_str()).collect();
            assert_eq!(ids, vec!["m5", "m2"]);
            idx.remove_docs(&[keys[0].id]).unwrap();
            assert_eq!(idx.message_count(), 1);
        }
    }

    #[test]
    fn trait_search_with_a_predicate_is_exact() {
        let e = HashEmbedder::new(256);
        let idx = SemanticIndex::in_memory("hash-bow", 256, false).unwrap();
        idx.replace_messages(&corpus(&e)).unwrap();
        let q = e.embed_query("lisbon flight").unwrap();
        let only_m4 = |c: &ChunkRef| c.message_id == "m4";
        let hits = idx.search(&q, 3, Some(&only_m4)).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.chunk.message_id == "m4"));
        // Trait upsert adds one chunk to a new or existing message.
        let v = e.embed_query("zebra crossing").unwrap();
        idx.upsert(
            ChunkRef { account_id: "c".into(), thread_id: "t9".into(), message_id: "m9".into(), chunk: 0, date: 9 },
            v.clone(),
        )
        .unwrap();
        assert_eq!(idx.message_count(), 6);
        idx.upsert(
            ChunkRef { account_id: "c".into(), thread_id: "t9".into(), message_id: "m9".into(), chunk: 0, date: 9 },
            v.clone(),
        )
        .unwrap();
        assert_eq!(idx.len(), 11);
        assert_eq!(idx.message_count(), 6);
        let hits = idx.search(&v, 1, None).unwrap();
        assert_eq!(hits[0].chunk.message_id, "m9");
    }

    #[test]
    fn persists_and_drops_vectors_of_another_model() {
        let dir = std::env::temp_dir().join(format!("penguin-sem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("semantic.db");
        let _ = std::fs::remove_file(&path);
        let e = HashEmbedder::new(256);
        {
            let idx = SemanticIndex::open(&path, "hash-bow", 256, false).unwrap();
            idx.replace_messages(&corpus(&e)).unwrap();
            idx.set_state("cursor", "42").unwrap();
        }
        {
            let idx = SemanticIndex::open(&path, "hash-bow", 256, false).unwrap();
            assert_eq!(idx.len(), 10);
            assert_eq!(idx.message_count(), 5);
            assert_eq!(idx.get_state("cursor").unwrap().as_deref(), Some("42"));
            let q = e.embed_query("invoice").unwrap();
            let hits = idx.search_messages(&q, 1, &SearchFilter::default()).unwrap();
            assert!(["m2", "m5"].contains(&hits[0].chunk.message_id.as_str()));
        }
        {
            let idx = SemanticIndex::open(&path, "other-model", 256, false).unwrap();
            assert_eq!(idx.len(), 0);
            // The indexer's state went with the vectors.
            assert_eq!(idx.get_state("cursor").unwrap(), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
