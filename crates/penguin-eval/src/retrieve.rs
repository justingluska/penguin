//! What gets evaluated: anything that turns a query into a ranked list of
//! conversations. Documents are conversations (`account/thread`), because
//! that is what Penguin's search list shows.
//!
//! - [`Keyword`]: `Store::search`, exactly as the app calls it. Once hybrid
//!   ranking lands inside `Store::search`, this measures it with no change.
//! - [`Ask`]: `Store::ask`, the cited messages of its answer in order.
//! - [`Vector`]: an [`Embedder`] + [`VectorIndex`] over message chunks, alone.
//! - [`Hybrid`]: reference fusion of Keyword and Vector with reciprocal rank
//!   fusion (Cormack, Clarke & Büttcher, SIGIR 2009, k = 60). Operators stay
//!   hard filters: with operators, only conversations that pass them are
//!   fused in from the vector side.
//!
//! Plugging in the real model: implement `Embedder` in penguin-semantic and
//! add it to [`embedder_by_name`]; `--mode vector|hybrid --embedder <name>`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use penguin_core::ask::AskScope;
use penguin_core::{Message, SearchRequest, SemanticHandles, Store};
use penguin_semantic::{ChunkRef, Embedder, FlatIndex, HashEmbedder, VectorIndex};

use crate::corpus::doc_id;

pub trait Retriever {
    /// Up to `k` conversations, best first.
    fn search(&self, query: &str, k: usize) -> Result<Vec<String>, String>;
}

pub struct Keyword<'a> {
    pub store: &'a Store,
}

impl Retriever for Keyword<'_> {
    fn search(&self, query: &str, k: usize) -> Result<Vec<String>, String> {
        let r = self
            .store
            .search(&SearchRequest {
                query: query.to_string(),
                account_id: None,
                account_ids: None,
                limit: k as u32,
            })
            .map_err(|e| e.to_string())?;
        Ok(r.hits
            .iter()
            .map(|h| doc_id(&h.account_id, &h.thread_id))
            .collect())
    }
}

pub struct Ask<'a> {
    pub store: &'a Store,
}

impl Retriever for Ask<'_> {
    fn search(&self, query: &str, k: usize) -> Result<Vec<String>, String> {
        let now = chrono::Utc::now().timestamp_millis();
        let offset = chrono::Local::now().offset().local_minus_utc();
        let a = self
            .store
            .ask(query, &AskScope::default(), now, offset)
            .map_err(|e| e.to_string())?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let cites = a
            .facts
            .iter()
            .filter_map(|f| f.cite.as_ref())
            .map(|c| doc_id(&c.account_id, &c.thread_id));
        let items = a.items.iter().map(|i| doc_id(&i.account_id, &i.thread_id));
        for d in items.chain(cites) {
            if seen.insert(d.clone()) {
                out.push(d);
            }
        }
        out.truncate(k);
        Ok(out)
    }
}

/// Embedders the harness knows by name.
///
/// - `hash`: the stand-in (shared words only).
/// - `file:PATH`: vectors a real model computed offline (see [`Precomputed`]).
/// - `gemma`: the model the app ships (`penguin_semantic::model::DEFAULT`,
///   EmbeddingGemma 4-bit, 256 dims). Files from `PENGUIN_SEMANTIC_MODEL_DIR`.
/// - `e5-small`: multilingual-e5-small int8, the permissive alternative.
///   Files from `PENGUIN_E5_MODEL_DIR`.
///
/// Both lay the files out as the app does (`<dir>/onnx/…`, `tokenizer.json`)
/// and are checked against the pinned sha256s. Passage vectors are cached
/// on disk (`target/search-eval/embeddings-<model>.bin`, append-only), so a
/// second run, or `--mode hybrid` after `--mode vector`, doesn't re-embed
/// the corpus, and an interrupted build resumes.
pub fn embedder_by_name(name: &str) -> Result<Box<dyn Embedder>, String> {
    match name {
        "hash" | "hash-bow" => Ok(Box::new(HashEmbedder::new(512))),
        "gemma" => onnx(&penguin_semantic::model::EMBEDDINGGEMMA_Q4, "PENGUIN_SEMANTIC_MODEL_DIR"),
        "e5-small" => onnx(&penguin_semantic::model::E5_SMALL_INT8, "PENGUIN_E5_MODEL_DIR"),
        other => match other.strip_prefix("file:") {
            Some(path) => Ok(Box::new(Precomputed::load(Path::new(path))?)),
            None => Err(format!("unknown embedder {other:?} (known: hash, gemma, e5-small, file:PATH)")),
        },
    }
}

fn onnx(spec: &'static penguin_semantic::model::ModelSpec, var: &str) -> Result<Box<dyn Embedder>, String> {
    let dir = std::env::var_os(var).ok_or_else(|| format!("set {var} to the model directory"))?;
    let dir = std::path::PathBuf::from(dir);
    if let Some((f, state)) = penguin_semantic::model::verify(spec, &dir).first() {
        return Err(format!("{}: {} is {state:?}", dir.display(), f.path));
    }
    let num = |var: &str| std::env::var(var).ok().and_then(|t| t.parse::<usize>().ok());
    let mut opts = penguin_semantic::EmbedderOptions {
        threads: num("EVAL_THREADS").unwrap_or(4),
        background_qos: false,
        ..Default::default()
    };
    // Batching experiments (docs/SEMANTIC.md, "Batching"): the vectors
    // depend a little on what shares a run.
    if let Some(n) = num("EVAL_BATCH_TOKENS") {
        opts.batch_tokens = n;
    }
    if let Some(n) = num("EVAL_MAX_BATCH") {
        opts.max_batch = n;
    }
    let e = penguin_semantic::OnnxEmbedder::load(&dir, spec, opts).map_err(|e| e.to_string())?;
    let path = std::path::PathBuf::from("target/search-eval").join(format!("embeddings-{}.bin", spec.key));
    Ok(Box::new(Cached::open(Box::new(e), path)?))
}

/// Passage vectors cached on disk by (model id, text). Records are
/// `u64 key, dims × f32`, appended after every batch.
pub struct Cached {
    inner: Box<dyn Embedder>,
    map: std::sync::Mutex<HashMap<u64, Vec<f32>>>,
    file: std::sync::Mutex<std::fs::File>,
}

impl Cached {
    pub fn open(inner: Box<dyn Embedder>, path: std::path::PathBuf) -> Result<Cached, String> {
        use std::io::Read;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let d = inner.dims();
        let rec = 8 + 4 * d;
        let mut map = HashMap::new();
        if let Ok(mut f) = std::fs::File::open(&path) {
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            for r in bytes.chunks_exact(rec) {
                let key = u64::from_le_bytes(r[..8].try_into().unwrap());
                let v = r[8..].chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
                map.insert(key, v);
            }
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        eprintln!("embedding cache {}: {} vectors", path.display(), map.len());
        Ok(Cached { inner, map: std::sync::Mutex::new(map), file: std::sync::Mutex::new(file) })
    }

    fn key(&self, text: &str) -> u64 {
        self.inner
            .model_id()
            .bytes()
            .chain([0u8])
            .chain(text.bytes())
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
    }
}

impl Embedder for Cached {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dims(&self) -> usize {
        self.inner.dims()
    }
    fn embed_query(&self, text: &str) -> penguin_semantic::Result<Vec<f32>> {
        self.inner.embed_query(text)
    }
    fn embed_passages(&self, texts: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
        use std::io::Write;
        let keys: Vec<u64> = texts.iter().map(|t| self.key(t)).collect();
        let missing: Vec<usize> = {
            let map = self.map.lock().unwrap();
            (0..texts.len()).filter(|&i| !map.contains_key(&keys[i])).collect()
        };
        // Small batches so progress is saved often.
        for group in missing.chunks(64) {
            let batch: Vec<&str> = group.iter().map(|&i| texts[i]).collect();
            let vecs = self.inner.embed_passages(&batch)?;
            let mut buf = Vec::new();
            let mut map = self.map.lock().unwrap();
            for (&i, v) in group.iter().zip(vecs) {
                buf.extend_from_slice(&keys[i].to_le_bytes());
                v.iter().for_each(|x| buf.extend_from_slice(&x.to_le_bytes()));
                map.insert(keys[i], v);
            }
            drop(map);
            self.file
                .lock()
                .unwrap()
                .write_all(&buf)
                .map_err(|e| penguin_semantic::SemanticError::Embed(e.to_string()))?;
        }
        let map = self.map.lock().unwrap();
        Ok(keys.iter().map(|k| map[k].clone()).collect())
    }
}

/// Vectors a real model computed offline for exactly the texts `search-eval
/// texts` lists (`crates/penguin-core/examples/ranking/embed.py`), looked up
/// by text: `{model, dims, passages: {text: hex}, queries: {text: hex}}`,
/// int8 per dimension. A text without a vector is an error, never a guess.
pub struct Precomputed {
    model: String,
    dims: usize,
    passages: HashMap<String, Vec<f32>>,
    queries: HashMap<String, Vec<f32>>,
}

impl Precomputed {
    pub fn load(path: &Path) -> Result<Precomputed, String> {
        #[derive(serde::Deserialize)]
        struct File {
            model: String,
            dims: usize,
            passages: HashMap<String, String>,
            queries: HashMap<String, String>,
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let f: File = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let decode = |hex: &String| {
            let mut v: Vec<f32> = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap_or(0) as i8 as f32)
                .collect();
            penguin_semantic::normalize(&mut v);
            v
        };
        Ok(Precomputed {
            model: f.model,
            dims: f.dims,
            passages: f.passages.iter().map(|(k, v)| (k.clone(), decode(v))).collect(),
            queries: f.queries.iter().map(|(k, v)| (k.clone(), decode(v))).collect(),
        })
    }
}

impl Embedder for Precomputed {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dims(&self) -> usize {
        self.dims
    }
    fn embed_query(&self, text: &str) -> penguin_semantic::Result<Vec<f32>> {
        self.queries.get(text).cloned().ok_or_else(|| {
            penguin_semantic::SemanticError::Embed(format!(
                "no precomputed vector for query {text:?}; re-run `search-eval texts` and embed.py"
            ))
        })
    }
    fn embed_passages(&self, texts: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
        texts
            .iter()
            .map(|t| {
                self.passages.get(*t).cloned().ok_or_else(|| {
                    penguin_semantic::SemanticError::Embed(
                        "no precomputed vector for a passage; re-run `search-eval texts` and embed.py"
                            .into(),
                    )
                })
            })
            .collect()
    }
}

/// The passages Penguin itself embeds for a message (the indexer's cut:
/// `penguin_semantic::chunk_message` over the authored text).
pub fn product_chunks(m: &Message) -> Vec<String> {
    let body = if m.body_text.trim().is_empty() {
        m.snippet.as_str()
    } else {
        m.body_text.as_str()
    };
    let authored = penguin_core::text::strip_quoted(body);
    // What the app's indexer embeds (apps/desktop/src-tauri/src/semantic):
    // sender and attachment names in chunk 0, bulk mail one chunk.
    let files: Vec<String> = m
        .attachments
        .iter()
        .filter(|a| !a.inline)
        .map(|a| a.filename.clone())
        .collect();
    let bulk = is_bulk(m);
    penguin_semantic::chunk::chunk_mail(
        &penguin_semantic::chunk::MailDoc {
            subject: &m.subject,
            from_name: m.from.name.as_deref(),
            from_email: &m.from.email,
            authored: &authored,
            filenames: &files,
            bulk,
        },
        &penguin_semantic::chunk::ChunkConfig::default(),
    )
}

/// Newsletter / promotions / updates / forums mail, as the app's store
/// flags it (`F_NEWSLETTER`).
pub fn is_bulk(m: &Message) -> bool {
    m.list_unsubscribe.is_some()
        || m.label_ids.iter().any(|l| {
            matches!(l.as_str(), "CATEGORY_PROMOTIONS" | "CATEGORY_UPDATES" | "CATEGORY_FORUMS")
        })
}

/// Penguin's own search, hybrid: `Store::search_hybrid` with a vector index
/// of [`product_chunks`]. What the app runs once search by meaning is on.
pub struct Penguin<'a> {
    pub store: &'a Store,
    pub handles: SemanticHandles,
}

impl Penguin<'_> {
    pub fn index(
        embedder: Box<dyn Embedder>,
        messages: &[Message],
    ) -> Result<(SemanticHandles, f64), String> {
        let t = Instant::now();
        let index = Arc::new(FlatIndex::default());
        // EVAL_SEMANTIC_BULK=skip leaves bulk mail out of the vector index
        // (it is still found by its words): what embedding it is worth
        // (docs/SEMANTIC.md, "Low-value mail").
        let skip_bulk = std::env::var("EVAL_SEMANTIC_BULK").is_ok_and(|v| v == "skip");
        let bulk = messages.iter().filter(|m| is_bulk(m)).count();
        eprintln!(
            "bulk mail: {bulk} of {} messages{}",
            messages.len(),
            if skip_bulk { ", left out of the vector index" } else { "" }
        );
        for batch in messages.chunks(256) {
            let mut refs = Vec::new();
            let mut texts = Vec::new();
            for m in batch.iter().filter(|m| !(skip_bulk && is_bulk(m))) {
                for (i, c) in product_chunks(m).into_iter().enumerate() {
                    refs.push(ChunkRef {
                        account_id: m.account_id.clone(),
                        thread_id: m.thread_id.clone(),
                        message_id: m.id.clone(),
                        chunk: i as u32,
                        date: m.date,
                    });
                    texts.push(c);
                }
            }
            let strs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let vecs = embedder.embed_passages(&strs).map_err(|e| e.to_string())?;
            for (r, v) in refs.into_iter().zip(vecs) {
                index.upsert(r, v).map_err(|e| e.to_string())?;
            }
        }
        Ok((
            SemanticHandles {
                embedder: Arc::from(embedder),
                index,
                progress: None,
            },
            t.elapsed().as_secs_f64(),
        ))
    }
}

impl Retriever for Penguin<'_> {
    fn search(&self, query: &str, k: usize) -> Result<Vec<String>, String> {
        let r = self
            .store
            .search_hybrid(
                &SearchRequest {
                    query: query.to_string(),
                    account_id: None,
                    account_ids: None,
                    limit: k as u32,
                },
                Some(&self.handles),
            )
            .map_err(|e| e.to_string())?;
        Ok(r.hits
            .iter()
            .map(|h| doc_id(&h.account_id, &h.thread_id))
            .collect())
    }
}

/// Split a message into the texts that get a vector: chunk 0 is sender,
/// subject and the opening; then ~120-word windows of the authored text
/// (quoted history is left out, it belongs to the message it quotes).
pub fn chunks(m: &Message) -> Vec<String> {
    const WORDS: usize = 120;
    const MAX: usize = 4;
    let (authored, _) = penguin_core::text::split_quoted(&m.body_text);
    let words: Vec<&str> = authored.split_whitespace().collect();
    let from = m.from.name.clone().unwrap_or_else(|| m.from.email.clone());
    let files: Vec<&str> = m.attachments.iter().map(|a| a.filename.as_str()).collect();
    let mut out = vec![format!(
        "From {from}. {}\n{}{}",
        m.subject,
        words
            .iter()
            .take(WORDS)
            .copied()
            .collect::<Vec<_>>()
            .join(" "),
        if files.is_empty() {
            String::new()
        } else {
            format!("\nAttachments: {}", files.join(", "))
        }
    )];
    let mut i = WORDS;
    while i < words.len() && out.len() < MAX {
        out.push(words[i..(i + WORDS).min(words.len())].join(" "));
        i += WORDS;
    }
    out
}

pub struct Vector {
    pub embedder: Box<dyn Embedder>,
    pub index: FlatIndex,
    pub build_secs: f64,
}

impl Vector {
    pub fn build(embedder: Box<dyn Embedder>, messages: &[Message]) -> Result<Vector, String> {
        let t = Instant::now();
        let index = FlatIndex::default();
        for batch in messages.chunks(256) {
            let mut refs = Vec::new();
            let mut texts = Vec::new();
            for m in batch {
                for (i, c) in chunks(m).into_iter().enumerate() {
                    refs.push(ChunkRef {
                        account_id: m.account_id.clone(),
                        thread_id: m.thread_id.clone(),
                        message_id: m.id.clone(),
                        chunk: i as u32,
                        date: m.date,
                    });
                    texts.push(c);
                }
            }
            let strs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let vecs = embedder.embed_passages(&strs).map_err(|e| e.to_string())?;
            for (r, v) in refs.into_iter().zip(vecs) {
                index.upsert(r, v).map_err(|e| e.to_string())?;
            }
        }
        Ok(Vector {
            embedder,
            index,
            build_secs: t.elapsed().as_secs_f64(),
        })
    }

    fn ranked(
        &self,
        text: &str,
        k: usize,
        allow: Option<&HashSet<String>>,
    ) -> Result<Vec<String>, String> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let qv = self.embedder.embed_query(text).map_err(|e| e.to_string())?;
        let filter =
            |c: &ChunkRef| allow.is_none_or(|a| a.contains(&doc_id(&c.account_id, &c.thread_id)));
        let hits = self
            .index
            .search(&qv, k * 8, Some(&filter))
            .map_err(|e| e.to_string())?;
        let mut best: Vec<(String, f32)> = Vec::new();
        let mut pos: HashMap<String, usize> = HashMap::new();
        for h in hits {
            let d = doc_id(&h.chunk.account_id, &h.chunk.thread_id);
            if !pos.contains_key(&d) {
                pos.insert(d.clone(), best.len());
                best.push((d, h.score));
            }
        }
        best.truncate(k);
        Ok(best.into_iter().map(|(d, _)| d).collect())
    }
}

impl Retriever for Vector {
    fn search(&self, query: &str, k: usize) -> Result<Vec<String>, String> {
        let (text, _) = split_operators(query);
        self.ranked(&text, k, None)
    }
}

/// Split a query into its free text and its operator part (`from:`, `-x`,
/// `date:"…"`), respecting quotes. Queries with `OR` or parentheses return
/// `None` for the operator part: they are left to the keyword side.
pub fn split_operators(query: &str) -> (String, Option<String>) {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in query.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let complex = tokens
        .iter()
        .any(|t| t == "OR" || t.starts_with('(') || t.starts_with("-("));
    let is_op = |t: &str| {
        let t = t.strip_prefix('-').unwrap_or(t);
        match t.split_once(':') {
            Some((k, _)) => !k.is_empty() && k.chars().all(|c| c.is_ascii_alphabetic() || c == '_'),
            None => false,
        }
    };
    let text: Vec<String> = tokens
        .iter()
        .filter(|t| !is_op(t) && !t.starts_with('-') && *t != "OR" && *t != "AND")
        .map(|t| {
            t.trim_matches(|c| c == '"' || c == '(' || c == ')')
                .to_string()
        })
        .collect();
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| is_op(t) || (t.len() > 1 && t.starts_with('-')))
        .map(String::as_str)
        .collect();
    let ops = if complex || ops.is_empty() {
        None
    } else {
        Some(ops.join(" "))
    };
    (text.join(" "), ops)
}

pub struct Hybrid<'a> {
    pub keyword: Keyword<'a>,
    pub vector: &'a Vector,
    /// RRF constant.
    pub k: f64,
}

impl Retriever for Hybrid<'_> {
    fn search(&self, query: &str, k: usize) -> Result<Vec<String>, String> {
        let depth = k.max(100);
        let kw = self.keyword.search(query, depth)?;
        let (text, ops) = split_operators(query);
        let has_ops = ops.is_some() || query.contains(" OR ") || query.contains('(');
        let vec = match (&ops, has_ops) {
            (Some(ops), _) => {
                // Operators are hard filters: fuse only conversations that pass them.
                let allowed: HashSet<String> = self.keyword.search(ops, 500)?.into_iter().collect();
                self.vector.ranked(&text, depth, Some(&allowed))?
            }
            (None, true) => Vec::new(),
            (None, false) => self.vector.ranked(&text, depth, None)?,
        };
        let mut score: HashMap<&str, f64> = HashMap::new();
        for list in [&kw, &vec] {
            for (i, d) in list.iter().enumerate() {
                *score.entry(d.as_str()).or_default() += 1.0 / (self.k + (i + 1) as f64);
            }
        }
        let mut out: Vec<(&str, f64)> = score.into_iter().collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
        Ok(out
            .into_iter()
            .take(k)
            .map(|(d, _)| d.to_string())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::split_operators;

    #[test]
    fn splits_text_from_operators() {
        assert_eq!(
            split_operators("from:mike lease"),
            ("lease".into(), Some("from:mike".into()))
        );
        assert_eq!(
            split_operators("dental date:\"last spring\""),
            ("dental".into(), Some("date:\"last spring\"".into()))
        );
        assert_eq!(
            split_operators("when is the desk arriving?"),
            ("when is the desk arriving?".into(), None)
        );
        assert_eq!(
            split_operators("(from:marco OR from:julia) invoice").1,
            None
        );
        assert_eq!(
            split_operators("invoice -crestline"),
            ("invoice".into(), Some("-crestline".into()))
        );
        assert_eq!(split_operators("\"past due\" from:theo").0, "past due");
    }
}
