//! Search-by-meaning benchmark: the vector index at mailbox scale, and the
//! embedding model when its files are available. Numbers go in
//! docs/SEMANTIC.md.
//!
//! ```text
//! cargo run -p penguin-semantic --release --features onnx --example bench
//! ```
//!
//! Environment:
//! - `BENCH_CHUNKS` (600000): vectors in the index (≈ 300k messages × 2).
//! - `BENCH_DIMS` (the default model's dims).
//! - `PENGUIN_SEMANTIC_VECTORS`: a directory with `passages.f32` and
//!   `queries.f32` (raw little-endian f32, 768 per row) of real embeddings.
//!   The index is then built by tiling them with small noise, so the
//!   vector distribution (and hence quantization recall) is realistic.
//!   Without it, synthetic anisotropic vectors are used.
//! - `PENGUIN_SEMANTIC_MODEL_DIR`: the model files → model latency,
//!   throughput and memory.

use std::path::Path;
use std::time::Instant;

use penguin_semantic::quant::{binarize_i8, bit_words, dot_i8, dot_i8_i4, hamming, pack_i4_from_i8, quantize_i8};
use penguin_semantic::{dot, normalize, DocInfo, SearchFilter, SemanticIndex};

const ACCOUNTS: [&str; 3] = ["alex@work.example", "alex@personal.example", "alex@side.example"];
const DAY: i64 = 86_400_000;
const NOW: i64 = 1_790_000_000_000;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Standard normal (Box–Muller).
    fn gauss(&mut self) -> f32 {
        let u = self.f().max(1e-7);
        let v = self.f();
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
}

fn env(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn read_f32(path: &Path, width: usize) -> Vec<Vec<f32>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes
        .chunks_exact(4 * width)
        .map(|row| row.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
        .collect()
}

/// First `dims` of a vector, renormalized (Matryoshka truncation).
fn cut(v: &[f32], dims: usize) -> Vec<f32> {
    let mut v = v[..dims.min(v.len())].to_vec();
    normalize(&mut v);
    v
}

fn rss_mb() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmRSS:"))?;
    Some(line.split_whitespace().nth(1)?.parse::<f64>().ok()? / 1024.0)
}

fn pct(xs: &mut [f64], p: f64) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[((xs.len() as f64 - 1.0) * p).round() as usize]
}

fn main() {
    let dims = env("BENCH_DIMS", penguin_semantic::model::DEFAULT.dims);
    let n = env("BENCH_CHUNKS", 600_000);
    println!("# penguin-semantic bench: {n} chunks × {dims} dims\n");
    // BENCH_ONLY=model: skip the index section, so the model's memory
    // numbers aren't masked by memory the index section freed.
    if std::env::var("BENCH_ONLY").as_deref() != Ok("model") {
        index_bench(n, dims);
    }
    #[cfg(feature = "onnx")]
    model_bench();
}

fn index_bench(n: usize, dims: usize) {
    let mut rng = Rng(42);
    // Base vectors: real embeddings if given, else synthetic anisotropic ones.
    let (base, queries, source) = match std::env::var_os("PENGUIN_SEMANTIC_VECTORS") {
        Some(dir) => {
            let dir = Path::new(&dir);
            let p = read_f32(&dir.join("passages.f32"), 768);
            let q = read_f32(&dir.join("queries.f32"), 768);
            let p: Vec<Vec<f32>> = p.iter().map(|v| cut(v, dims)).collect();
            let q: Vec<Vec<f32>> = q.iter().take(200).map(|v| cut(v, dims)).collect();
            (p, q, format!("real embeddings ({} passages) tiled with noise", read_len(dir)))
        }
        None => {
            let mean: Vec<f32> = (0..dims).map(|_| rng.gauss()).collect();
            let make = |rng: &mut Rng| {
                let mut v: Vec<f32> = mean.iter().map(|m| 0.6 * m + rng.gauss()).collect();
                normalize(&mut v);
                v
            };
            let p: Vec<Vec<f32>> = (0..20_000).map(|_| make(&mut rng)).collect();
            let q: Vec<Vec<f32>> = (0..200).map(|_| make(&mut rng)).collect();
            (p, q, "synthetic anisotropic vectors".to_string())
        }
    };
    println!("## Vector index\n\nSource: {source}.\n");
    // Tile to n vectors: base + noise, so neighbours stay meaningful.
    let sigma = 0.35 / (dims as f32).sqrt();
    let vectors: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            let b = &base[i % base.len()];
            let mut v: Vec<f32> = if i < base.len() {
                b.clone()
            } else {
                b.iter().map(|x| x + sigma * rng.gauss()).collect()
            };
            normalize(&mut v);
            v
        })
        .collect();

    // Recall of the quantized search vs exact f32, before any SQLite.
    recall_table(&vectors, &queries, dims);

    // Build semantic.db on disk.
    let dir = std::env::temp_dir().join(format!("penguin-semantic-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("semantic.db");
    let rss0 = rss_mb();
    let started = Instant::now();
    {
        let ix = SemanticIndex::open(&path, "bench", dims, false).unwrap();
        let mut batch = Vec::new();
        for m in 0..n / 2 {
            let date = NOW - (m as i64 * 8 * 365 * DAY) / (n as i64 / 2);
            let acct = match m % 10 {
                0..=5 => 0,
                6..=8 => 1,
                _ => 2,
            };
            batch.push((
                DocInfo {
                    account_id: ACCOUNTS[acct].into(),
                    message_id: format!("m{m:08x}"),
                    thread_id: format!("t{:08x}", m / 2),
                    date,
                    src_rowid: date * 1024,
                    sig: 0,
                },
                vec![vectors[2 * m].clone(), vectors[2 * m + 1].clone()],
            ));
            if batch.len() == 256 {
                ix.replace_messages(&batch).unwrap();
                batch.clear();
            }
        }
        ix.replace_messages(&batch).unwrap();
    }
    let build = started.elapsed().as_secs_f64();
    let db_mb = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as f64 / 1e6
        + std::fs::metadata(dir.join("semantic.db-wal")).map(|m| m.len()).unwrap_or(0) as f64 / 1e6;
    println!("\n| | |\n|---|---:|");
    println!("| Insert {n} vectors (int8 into semantic.db) | {:.1} s ({:.0} vectors/s) |", build, n as f64 / build);
    println!("| semantic.db size | {db_mb:.0} MB |");

    for int8_in_ram in [false, true] {
        let t = Instant::now();
        let ix = SemanticIndex::open(&path, "bench", dims, int8_in_ram).unwrap();
        let load = t.elapsed().as_secs_f64() * 1000.0;
        let label = if int8_in_ram { "int4 + int8 in RAM" } else { "int4 in RAM, int8 rescored from semantic.db" };
        println!("| Open + load ({label}) | {load:.0} ms |");
        println!("| Index RAM ({label}) | {:.1} MB |", ix.ram_bytes() as f64 / 1e6);
        if let (Some(a), Some(b)) = (rss0, rss_mb()) {
            println!("| Process RSS growth after load ({label}) | {:.0} MB |", b - a);
        }
        let filters: [(&str, SearchFilter); 4] = [
            ("all mail", SearchFilter::default()),
            ("one account (10% of mail)", SearchFilter { account_ids: Some(vec![ACCOUNTS[2].into()]), ..Default::default() }),
            ("last 12 months", SearchFilter { after: Some(NOW - 365 * DAY), ..Default::default() }),
            ("one account + last 12 months", SearchFilter { account_ids: Some(vec![ACCOUNTS[2].into()]), after: Some(NOW - 365 * DAY), ..Default::default() }),
        ];
        for (name, f) in &filters {
            let mut ms = Vec::new();
            for q in &queries {
                let t = Instant::now();
                let hits = ix.search_messages(q, 50, f).unwrap();
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(hits);
            }
            println!(
                "| search_messages k=50, {name} ({label}) | p50 {:.1} ms, p95 {:.1} ms |",
                pct(&mut ms, 0.5),
                pct(&mut ms, 0.95)
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn read_len(dir: &Path) -> usize {
    std::fs::metadata(dir.join("passages.f32")).map(|m| m.len() as usize / (4 * 768)).unwrap_or(0)
}

/// Recall@10 of (a) exact int8 and (b) binary candidates + int8 rescoring,
/// against exact f32 cosine, for several candidate-pool sizes, with and
/// without centering the vectors before taking signs.
fn recall_table(vectors: &[Vec<f32>], queries: &[Vec<f32>], dims: usize) {
    let k = 10;
    let qs = &queries[..queries.len().min(100)];
    let mut codes: Vec<Vec<i8>> = Vec::with_capacity(vectors.len());
    let mut scales = Vec::with_capacity(vectors.len());
    let mut nibs: Vec<Vec<u8>> = Vec::with_capacity(vectors.len());
    let mut nib_scales = Vec::with_capacity(vectors.len());
    for v in vectors {
        let mut c = Vec::new();
        let s = quantize_i8(v, &mut c);
        let mut n = Vec::new();
        nib_scales.push(pack_i4_from_i8(&c, s, &mut n));
        nibs.push(n);
        scales.push(s);
        codes.push(c);
    }
    let mean: Vec<f32> = (0..dims)
        .map(|d| vectors.iter().map(|v| v[d]).sum::<f32>() / vectors.len() as f32)
        .collect();
    let bits_of = |v: &[f32], center: bool| {
        let c: Vec<f32> = if center { v.iter().zip(&mean).map(|(x, m)| x - m).collect() } else { v.to_vec() };
        let mut q = Vec::new();
        quantize_i8(&c, &mut q);
        let mut b = Vec::new();
        binarize_i8(&q, &mut b);
        b
    };
    let w = bit_words(dims);
    let plain: Vec<u64> = vectors.iter().flat_map(|v| bits_of(v, false)).collect();
    let centered: Vec<u64> = vectors.iter().flat_map(|v| bits_of(v, true)).collect();
    let pools = [50, 100, 200, 400, 1600];
    let mut int8_recall = 0.0;
    let mut bin = vec![[0.0f64; 2]; pools.len()];
    let mut i4 = vec![0.0f64; pools.len()];
    let started = Instant::now();
    for q in qs {
        let mut exact: Vec<(f32, usize)> = vectors.iter().enumerate().map(|(i, v)| (dot(q, v), i)).collect();
        exact.select_nth_unstable_by(k, |a, b| b.0.total_cmp(&a.0));
        let truth: std::collections::HashSet<usize> = exact[..k].iter().map(|x| x.1).collect();
        let mut qc = Vec::new();
        let qscale = quantize_i8(q, &mut qc);
        let score = |i: usize| dot_i8(&qc, &codes[i]) as f32 * qscale * scales[i];
        let mut all: Vec<(f32, usize)> = (0..vectors.len()).map(|i| (score(i), i)).collect();
        all.select_nth_unstable_by(k, |a, b| b.0.total_cmp(&a.0));
        int8_recall += all[..k].iter().filter(|x| truth.contains(&x.1)).count() as f64 / k as f64;
        let mut s4: Vec<(f32, usize)> =
            (0..vectors.len()).map(|i| (dot_i8_i4(&qc, &nibs[i]) as f32 * nib_scales[i], i)).collect();
        s4.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        for (p, &pool) in pools.iter().enumerate() {
            let mut cand: Vec<(f32, usize)> = s4[..pool.min(s4.len())].iter().map(|&(_, i)| (score(i), i)).collect();
            cand.sort_by(|a, b| b.0.total_cmp(&a.0));
            i4[p] += cand[..k].iter().filter(|x| truth.contains(&x.1)).count() as f64 / k as f64;
        }
        for (c, (bits, center)) in [(&plain, false), (&centered, true)].into_iter().enumerate() {
            let qb = bits_of(q, center);
            let mut ham: Vec<(u32, usize)> =
                (0..vectors.len()).map(|i| (hamming(&qb, &bits[i * w..(i + 1) * w]), i)).collect();
            ham.sort_unstable();
            for (p, &pool) in pools.iter().enumerate() {
                let mut cand: Vec<(f32, usize)> = ham[..pool.min(ham.len())].iter().map(|&(_, i)| (score(i), i)).collect();
                cand.sort_by(|a, b| b.0.total_cmp(&a.0));
                bin[p][c] += cand[..k].iter().filter(|x| truth.contains(&x.1)).count() as f64 / k as f64;
            }
        }
    }
    let nq = qs.len() as f64;
    println!(
        "Recall@10 against exact f32 cosine over {} vectors ({} queries, {:.0} s):\n",
        vectors.len(),
        qs.len(),
        started.elapsed().as_secs_f64()
    );
    println!("| Method | Recall@10 |\n|---|---:|");
    println!("| int8, exhaustive | {:.3} |", int8_recall / nq);
    for (p, pool) in pools.iter().enumerate() {
        println!(
            "| top {pool}: int4 → int8 rescore {:.3} · binary → int8 rescore {:.3} (centered {:.3}) |",
            i4[p] / nq,
            bin[p][0] / nq,
            bin[p][1] / nq
        );
    }
}

#[cfg(feature = "onnx")]
fn model_bench() {
    use penguin_semantic::chunk::{chunk_mail, ChunkConfig, MailDoc};
    use penguin_semantic::model::{self, DEFAULT};
    use penguin_semantic::{Embedder, EmbedderOptions, OnnxEmbedder};

    let Some(dir) = std::env::var_os("PENGUIN_SEMANTIC_MODEL_DIR") else {
        println!("\n(model section skipped: set PENGUIN_SEMANTIC_MODEL_DIR)");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    assert!(model::verify(DEFAULT, &dir).is_empty(), "model files don't match the spec");
    println!("\n## Model: {} ({})\n", DEFAULT.display, DEFAULT.model_id());
    #[derive(serde::Deserialize)]
    struct Fixture {
        messages: Vec<serde_json::Value>,
        queries: Vec<serde_json::Value>,
    }
    let f: Fixture = serde_json::from_str(include_str!("../tests/fixtures/email_eval.json")).unwrap();
    let queries: Vec<String> = f.queries.iter().map(|q| q["q"].as_str().unwrap().to_string()).collect();
    // Realistic passages: every fixture message chunked, plus long bodies
    // made of fixture text so body windows are full length too.
    let cfg = ChunkConfig::default();
    let mut passages = Vec::new();
    let all_text: Vec<&str> = f.messages.iter().map(|m| m["body"].as_str().unwrap()).collect();
    for (i, m) in f.messages.iter().enumerate() {
        let long = format!("{} {} {}", all_text[i], all_text[(i + 7) % all_text.len()], all_text[(i + 13) % all_text.len()]);
        let body = if i % 2 == 0 { m["body"].as_str().unwrap().to_string() } else { long.repeat(3) };
        let doc = MailDoc {
            subject: m["subject"].as_str().unwrap(),
            from_name: None,
            from_email: m["from"].as_str().unwrap(),
            authored: &body,
            filenames: &[],
            bulk: false,
        };
        passages.extend(chunk_mail(&doc, &cfg));
    }
    let rss0 = rss_mb();
    println!("| | |\n|---|---:|");
    for threads in [1usize, 2, 4] {
        let t = Instant::now();
        let e = OnnxEmbedder::load(&dir, DEFAULT, EmbedderOptions { threads, background_qos: false, ..Default::default() }).unwrap();
        let load = t.elapsed().as_secs_f64() * 1000.0;
        if threads == 4 {
            println!("| Load model + tokenizer | {load:.0} ms |");
            if let (Some(a), Some(b)) = (rss0, rss_mb()) {
                println!("| RSS growth after load | {:.0} MB |", b - a);
            }
        }
        e.embed_query("warm up").unwrap();
        let mut ms = Vec::new();
        for _ in 0..3 {
            for q in &queries {
                let t = Instant::now();
                e.embed_query(q).unwrap();
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        println!(
            "| Query embed, {threads} thread(s) | p50 {:.1} ms, p95 {:.1} ms |",
            pct(&mut ms, 0.5),
            pct(&mut ms, 0.95)
        );
        let tokens: usize = passages.iter().map(|p| e.token_count(p)).sum();
        let refs: Vec<&str> = passages.iter().map(String::as_str).collect();
        let t = Instant::now();
        e.embed_passages(&refs).unwrap();
        let s = t.elapsed().as_secs_f64();
        println!(
            "| Passages, {threads} thread(s) ({} passages, avg {:.0} tokens) | {:.1} passages/s ({:.0} tokens/s) |",
            passages.len(),
            tokens as f64 / passages.len() as f64,
            passages.len() as f64 / s,
            tokens as f64 / s
        );
        if threads == 4 {
            if let (Some(a), Some(b)) = (rss0, rss_mb()) {
                println!("| RSS growth after indexing batches | {:.0} MB |", b - a);
            }
        }
    }
}
