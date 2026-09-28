//! Search by meaning, on device.
//!
//! This crate is the seam between mail and vectors. Everything else in the
//! workspace talks to it through two traits:
//!
//! - [`Embedder`] turns text into a unit-length vector. Queries and passages
//!   are embedded separately because most retrieval models prefix them
//!   differently ("query: …" / "passage: …").
//! - [`VectorIndex`] stores one vector per chunk of a message and returns the
//!   nearest chunks to a query vector, optionally filtered (account, date…).
//!
//! [`HashEmbedder`] and [`FlatIndex`] are small, deterministic stand-ins so
//! search ranking and Ask can be built and tested before (and without) the
//! real model. They are not meant for production ranking.

use std::collections::{HashMap, HashSet};

pub mod chunk;
#[cfg(feature = "onnx")]
pub mod embed;
pub mod index;
pub mod mem;
pub mod model;
pub mod qos;
pub mod quant;

#[cfg(feature = "onnx")]
pub use embed::{Accel, EmbedStats, EmbedderOptions, OnnxEmbedder};
pub use index::{DocInfo, DocKey, SemanticIndex};
pub use chunk::chunk_message;

/// Errors from embedding or the index.
#[derive(Debug, thiserror::Error)]
pub enum SemanticError {
    #[error("embedding model unavailable: {0}")]
    ModelUnavailable(String),
    #[error("embedding failed: {0}")]
    Embed(String),
    #[error("vector index: {0}")]
    Index(String),
}

pub type Result<T> = std::result::Result<T, SemanticError>;

/// Turns text into unit-length vectors of [`Embedder::dims`] floats.
pub trait Embedder: Send + Sync {
    /// Stable id of the model and its settings; stored with every vector so a
    /// model change re-embeds instead of mixing incompatible vectors.
    fn model_id(&self) -> &str;
    fn dims(&self) -> usize;
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    /// Passages someone is waiting for (Ask comparing sentences with the
    /// question): the same vectors as [`Embedder::embed_passages`], but an
    /// embedder that paces background indexing runs these at once and
    /// ahead of it, like a query.
    fn embed_passages_now(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed_passages(texts)
    }
}

/// Which chunk of which message a vector belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChunkRef {
    pub account_id: String,
    pub thread_id: String,
    pub message_id: String,
    /// 0 = subject + opening, then successive body chunks.
    pub chunk: u32,
    /// Message date, epoch ms (for date filters without a store lookup).
    pub date: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub chunk: ChunkRef,
    /// Cosine similarity, higher is closer.
    pub score: f32,
}

/// Nearest-neighbour search over chunk vectors.
pub trait VectorIndex: Send + Sync {
    fn upsert(&self, chunk: ChunkRef, vector: Vec<f32>) -> Result<()>;
    fn remove_message(&self, account_id: &str, message_id: &str) -> Result<()>;
    /// The `k` nearest chunks that pass `filter`.
    fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn Fn(&ChunkRef) -> bool + Sync)>,
    ) -> Result<Vec<VectorHit>>;
    /// The `k` nearest chunks under structured constraints. An index that
    /// can apply them during its scan (before the nearest-neighbour cut)
    /// overrides this: exact for selective filters and much faster than a
    /// predicate checked on candidates. The default checks them as a
    /// predicate through [`VectorIndex::search`].
    fn search_where(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Result<Vec<VectorHit>> {
        let f = |c: &ChunkRef| filter.allows(c);
        self.search(query, k, Some(&f))
    }
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Hard constraints for [`VectorIndex::search_where`]. `None` = no limit.
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    /// Only these accounts (empty = nothing).
    pub account_ids: Option<Vec<String>>,
    /// Message date ≥ this (epoch ms).
    pub after: Option<i64>,
    /// Message date < this (epoch ms).
    pub before: Option<i64>,
    /// Only these messages: account id → message ids (search's other
    /// filters, resolved to messages first).
    pub messages: Option<HashMap<String, HashSet<String>>>,
}

impl SearchFilter {
    pub fn allows(&self, c: &ChunkRef) -> bool {
        self.after.is_none_or(|a| c.date >= a)
            && self.before.is_none_or(|b| c.date < b)
            && self
                .account_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|a| *a == c.account_id))
            && self.messages.as_ref().is_none_or(|m| {
                m.get(&c.account_id)
                    .is_some_and(|ids| ids.contains(&c.message_id))
            })
    }
}

/// Scale `v` to unit length (no-op for the zero vector).
pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Test stand-in: hashed bag of lowercase words, so texts sharing words are
/// close. It knows nothing about meaning ("flight" ≠ "plane").
pub struct HashEmbedder {
    dims: usize,
}

impl HashEmbedder {
    pub fn new(dims: usize) -> Self {
        Self { dims: dims.max(8) }
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0; self.dims];
        for w in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let h = w
                .to_lowercase()
                .bytes()
                .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
            v[(h % self.dims as u64) as usize] += 1.0;
        }
        normalize(&mut v);
        v
    }
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> &str {
        "hash-bow"
    }
    fn dims(&self) -> usize {
        self.dims
    }
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.embed(text))
    }
    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed(t)).collect())
    }
}

/// Test stand-in: exact brute-force search in memory.
#[derive(Default)]
pub struct FlatIndex {
    rows: std::sync::RwLock<HashMap<ChunkRef, Vec<f32>>>,
}

impl VectorIndex for FlatIndex {
    fn upsert(&self, chunk: ChunkRef, vector: Vec<f32>) -> Result<()> {
        self.rows
            .write()
            .map_err(|e| SemanticError::Index(e.to_string()))?
            .insert(chunk, vector);
        Ok(())
    }
    fn remove_message(&self, account_id: &str, message_id: &str) -> Result<()> {
        self.rows
            .write()
            .map_err(|e| SemanticError::Index(e.to_string()))?
            .retain(|c, _| !(c.account_id == account_id && c.message_id == message_id));
        Ok(())
    }
    fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn Fn(&ChunkRef) -> bool + Sync)>,
    ) -> Result<Vec<VectorHit>> {
        let rows = self.rows.read().map_err(|e| SemanticError::Index(e.to_string()))?;
        let mut hits: Vec<VectorHit> = rows
            .iter()
            .filter(|(c, _)| filter.is_none_or(|f| f(c)))
            .map(|(c, v)| VectorHit { chunk: c.clone(), score: dot(query, v) })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(k);
        Ok(hits)
    }
    fn len(&self) -> usize {
        self.rows.read().map(|r| r.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str) -> ChunkRef {
        ChunkRef {
            account_id: "a".into(),
            thread_id: id.into(),
            message_id: id.into(),
            chunk: 0,
            date: 0,
        }
    }

    #[test]
    fn stand_ins_rank_shared_words_first_and_filter() {
        let e = HashEmbedder::new(256);
        let idx = FlatIndex::default();
        let docs = [("m1", "Your flight to Lisbon is confirmed"), ("m2", "Quarterly invoice attached")];
        for (id, text) in docs {
            idx.upsert(chunk(id), e.embed_passages(&[text]).unwrap().remove(0)).unwrap();
        }
        let q = e.embed_query("lisbon flight").unwrap();
        let hits = idx.search(&q, 2, None).unwrap();
        assert_eq!(hits[0].chunk.message_id, "m1");
        let only_m2 = |c: &ChunkRef| c.message_id == "m2";
        let hits = idx.search(&q, 2, Some(&only_m2)).unwrap();
        assert_eq!(hits.len(), 1);
        idx.remove_message("a", "m1").unwrap();
        assert_eq!(idx.len(), 1);
    }
}
