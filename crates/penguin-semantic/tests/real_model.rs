//! End-to-end tests with the real model. They need the model files, so
//! they're skipped unless `PENGUIN_SEMANTIC_MODEL_DIR` points at a
//! directory holding the default spec's files (as the app stores them:
//! `<data dir>/models/<key>/`), e.g.
//!
//! ```text
//! PENGUIN_SEMANTIC_MODEL_DIR=~/Library/Application\ Support/co.gluska.penguin/models/embeddinggemma-300m-q4 \
//!   cargo test -p penguin-semantic --features onnx --release --test real_model -- --nocapture
//! ```
#![cfg(feature = "onnx")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use penguin_semantic::chunk::{chunk_mail, ChunkConfig, MailDoc};
use penguin_semantic::model::{self, DEFAULT};
use penguin_semantic::{DocInfo, Embedder, EmbedderOptions, OnnxEmbedder, SearchFilter, SemanticIndex};

fn model_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("PENGUIN_SEMANTIC_MODEL_DIR")?);
    let bad = model::verify(DEFAULT, &dir);
    assert!(bad.is_empty(), "model files in {} don't match the spec: {bad:?}", dir.display());
    Some(dir)
}

fn load() -> Option<OnnxEmbedder> {
    let dir = model_dir()?;
    Some(OnnxEmbedder::load(&dir, DEFAULT, EmbedderOptions { threads: 4, background_qos: false, ..Default::default() }).expect("model loads"))
}

#[derive(serde::Deserialize)]
struct Fixture {
    messages: Vec<Msg>,
    queries: Vec<Query>,
}

#[derive(serde::Deserialize)]
struct Msg {
    id: String,
    from: String,
    subject: String,
    body: String,
    files: Vec<String>,
    #[serde(default)]
    bulk: bool,
}

#[derive(serde::Deserialize)]
struct Query {
    q: String,
    relevant: Vec<String>,
    kind: String,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/email_eval.json")).expect("fixture parses")
}

/// The whole pipeline on the eval set: chunk → embed → int8/binary index →
/// search_messages. Recall@5 per query kind must hold up after
/// quantization and Matryoshka truncation.
#[test]
fn retrieves_the_eval_set_through_the_index() {
    let Some(e) = load() else {
        eprintln!("skipped: set PENGUIN_SEMANTIC_MODEL_DIR");
        return;
    };
    let f = fixture();
    let ix = SemanticIndex::in_memory(e.model_id(), e.dims(), false).unwrap();
    let cfg = ChunkConfig::default();
    let started = Instant::now();
    let mut items = Vec::new();
    for (i, m) in f.messages.iter().enumerate() {
        let (name, email) = match m.from.split_once(" <") {
            Some((n, rest)) => (Some(n), rest.trim_end_matches('>')),
            None => (None, m.from.as_str()),
        };
        let doc = MailDoc {
            subject: &m.subject,
            from_name: name,
            from_email: email,
            authored: &m.body,
            filenames: &m.files,
            bulk: m.bulk,
        };
        let chunks = chunk_mail(&doc, &cfg);
        let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let vectors = e.embed_passages(&refs).unwrap();
        items.push((
            DocInfo {
                account_id: if i % 3 == 0 { "b".into() } else { "a".into() },
                message_id: m.id.clone(),
                thread_id: format!("t-{}", m.id),
                date: i as i64,
                src_rowid: i as i64,
                sig: 0,
            },
            vectors,
        ));
    }
    ix.replace_messages(&items).unwrap();
    eprintln!("embedded {} messages in {:.1} s", f.messages.len(), started.elapsed().as_secs_f64());

    let mut by_kind: std::collections::BTreeMap<&str, (f64, f64, usize)> = Default::default();
    let mut latencies = Vec::new();
    for q in &f.queries {
        let t = Instant::now();
        let v = e.embed_query(&q.q).unwrap();
        latencies.push(t.elapsed().as_secs_f64() * 1000.0);
        let hits = ix.search_messages(&v, 5, &SearchFilter::default()).unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.chunk.message_id.as_str()).collect();
        let found = q.relevant.iter().filter(|r| ids.contains(&r.as_str())).count();
        let r1 = f64::from(u8::from(ids.first().is_some_and(|id| q.relevant.iter().any(|r| r == id))));
        let entry = by_kind.entry(q.kind.as_str()).or_default();
        entry.0 += found as f64 / q.relevant.len() as f64;
        entry.1 += r1;
        entry.2 += 1;
    }
    latencies.sort_by(f64::total_cmp);
    eprintln!(
        "query embed p50 {:.1} ms, max {:.1} ms",
        latencies[latencies.len() / 2],
        latencies[latencies.len() - 1]
    );
    for (kind, (r5, r1, n)) in &by_kind {
        eprintln!("{kind:13} R@1 {:.2}  R@5 {:.2}  ({n} queries)", r1 / *n as f64, r5 / *n as f64);
    }
    let r5 = |k: &str| by_kind.get(k).map_or(1.0, |(s, _, n)| s / *n as f64);
    assert!(r5("paraphrase") >= 0.9, "paraphrase R@5 {}", r5("paraphrase"));
    assert!(r5("crosslingual") >= 0.85, "crosslingual R@5 {}", r5("crosslingual"));

    // An account filter applies before the cut: every hit is from "b".
    let v = e.embed_query("invoice").unwrap();
    let f_b = SearchFilter { account_ids: Some(vec!["b".into()]), ..Default::default() };
    let hits = ix.search_messages(&v, 10, &f_b).unwrap();
    assert_eq!(hits.len(), 10);
    assert!(hits.iter().all(|h| h.chunk.account_id == "b"));
}

/// Vectors are unit length and deterministic, and a batch interrupted by a
/// query (then retried) gives the vectors an undisturbed run does, up to
/// float noise from different padding.
#[test]
fn queries_preempt_passages_without_changing_results() {
    let Some(e) = load() else {
        eprintln!("skipped: set PENGUIN_SEMANTIC_MODEL_DIR");
        return;
    };
    let e = Arc::new(e);
    let passages: Vec<String> = fixture()
        .messages
        .iter()
        .map(|m| format!("{}\n{}", m.subject, m.body))
        .collect();
    let refs: Vec<&str> = passages.iter().map(String::as_str).collect();
    let calm = e.embed_passages(&refs).unwrap();
    for v in &calm {
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-3);
        assert_eq!(v.len(), e.dims());
    }
    let busy = {
        let e2 = e.clone();
        let passages = passages.clone();
        let worker = std::thread::spawn(move || {
            let refs: Vec<&str> = passages.iter().map(String::as_str).collect();
            e2.embed_passages(&refs).unwrap()
        });
        let mut lat = Vec::new();
        while !worker.is_finished() {
            let t = Instant::now();
            e.embed_query("when is my daughter's dentist visit").unwrap();
            lat.push(t.elapsed().as_secs_f64() * 1000.0);
            // Someone typing: a query every ~80 ms.
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        lat.sort_by(f64::total_cmp);
        if !lat.is_empty() {
            eprintln!(
                "{} queries during indexing: p50 {:.1} ms, max {:.1} ms",
                lat.len(),
                lat[lat.len() / 2],
                lat[lat.len() - 1]
            );
        }
        worker.join().unwrap()
    };
    for (a, b) in calm.iter().zip(&busy) {
        let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        // Not bit-identical: a batch that stops accepting interruptions runs
        // in pieces, and a passage's vector depends a little on what else
        // is in its run. On x86-64 that is float accumulation order (cos ≥
        // 0.9997). On Apple silicon ONNX Runtime's KleidiAI int8 kernels
        // make it larger (cos ≥ 0.9977 against the same passage alone;
        // with mlas.disable_kleidiai it is ≥ 0.9996 again): embed_perf
        // PERF_MODE=padding, docs/SEMANTIC.md "Batching". Both are well
        // under the int8 storage error (~0.01). A vector landing on the
        // wrong passage, the bug this guards against, is checked below.
        assert!(d > 0.995, "interrupted batch changed a vector (cos {d})");
    }
    // And every vector is still nearest its own passage.
    for (i, b) in busy.iter().enumerate() {
        let own: f32 = b.iter().zip(&calm[i]).map(|(x, y)| x * y).sum();
        for (j, c) in calm.iter().enumerate().filter(|&(j, _)| j != i) {
            let other: f32 = b.iter().zip(c).map(|(x, y)| x * y).sum();
            assert!(own > other, "vector {i} is nearer passage {j} ({other}) than its own ({own})");
        }
    }
}
