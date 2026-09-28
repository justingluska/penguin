//! Embedding throughput and latency on realistic passages, for tuning the
//! embedder (docs/SEMANTIC.md, "Embedding speed"). Prints Markdown tables.
//!
//! ```text
//! search-eval texts --out /tmp/texts.json        # the harness mailbox, cut by chunk_mail
//! PENGUIN_SEMANTIC_MODEL_DIR=… PERF_TEXTS=/tmp/texts.json \
//!   cargo run -p penguin-semantic --release --features onnx --example embed_perf
//! ```
//!
//! `PERF_MODE` picks what is measured:
//! - `sweep` (default): one row per configuration: load time, passages/s,
//!   padding, CPU per passage, query p50/p95, Ask's 96 sentences (idle, and
//!   while indexing runs on another thread), memory.
//! - `tokenizer`: the tokenizer alone: resident memory before and after
//!   `mem::release_freed`, speed, and the passages' token lengths.
//! - `cycles`: memory left behind by loading, using and dropping the
//!   tokenizer repeatedly on one thread (`PERF_CYCLES`, `PERF_BPE_CACHE`).
//! - `padding`: do vectors depend on how passages are batched? Every
//!   passage embedded alone (no padding) against the default batching,
//!   pieces of two, the old batches of 16 and one wide batch.
//!
//! Environment:
//! - `PERF_TEXTS`: JSON `{passages: [..], queries: [..]}` (`search-eval
//!   texts`). Without it, the eval fixture's messages are chunked.
//! - `PERF_N` (480; 96 for `padding`): passages used, a fixed
//!   pseudo-random sample.
//! - `PERF_GROUP` (30): passages per `embed_passages` call, like the app's
//!   indexer (16 messages ≈ 30 passages).
//! - `PERF_THREADS` ("4"), `PERF_BATCH_TOKENS` ("0" = the default),
//!   `PERF_MAX_BATCH` ("0" = the default), `PERF_SPIN` ("0"),
//!   `PERF_MAX_TOKENS` ("0" = the model's cap): comma lists; every
//!   combination is run.
//! - `PERF_QUERY_REPS` (3): passes over the queries for query latency.
//! - `PERF_ACCEL` ("cpu"): comma list of `cpu`, `webgpu` (needs the
//!   `webgpu` feature). Accelerated vectors are compared with
//!   the CPU provider's ("min cosine vs CPU").
//!
//! Resident memory is per process: for clean memory numbers, run one
//! configuration per process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use penguin_semantic::model::{self, ModelSpec, DEFAULT};
use penguin_semantic::{Accel, Embedder, EmbedderOptions, OnnxEmbedder};

fn list(name: &str, default: &str) -> Vec<usize> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

fn one(name: &str, default: usize) -> usize {
    list(name, &default.to_string()).first().copied().unwrap_or(default)
}

/// Current resident set, MB.
#[cfg(target_os = "macos")]
#[allow(deprecated)] // libc points at the mach2 crate for these; same ABI.
fn rss_mb() -> f64 {
    // SAFETY: task_info fills at most `count` integers of the struct.
    unsafe {
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>() / std::mem::size_of::<libc::natural_t>())
            as libc::mach_msg_type_number_t;
        let r = libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as libc::task_info_t,
            &mut count,
        );
        if r == 0 {
            info.resident_size as f64 / 1e6
        } else {
            f64::NAN
        }
    }
}

/// Current resident set, MB (Linux; NaN elsewhere).
#[cfg(not(target_os = "macos"))]
fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

/// Physical footprint, MB: what Activity Monitor shows as Memory. Unlike
/// the resident set it leaves out freed pages the allocator has marked
/// reusable. macOS only; the resident set elsewhere.
#[cfg(target_os = "macos")]
fn footprint_mb() -> f64 {
    // SAFETY: proc_pid_rusage fills the struct of the flavor it is given.
    unsafe {
        let mut info: libc::rusage_info_v2 = std::mem::zeroed();
        let r = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            &mut info as *mut _ as *mut libc::rusage_info_t,
        );
        if r == 0 {
            info.ri_phys_footprint as f64 / 1e6
        } else {
            f64::NAN
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn footprint_mb() -> f64 {
    rss_mb()
}

fn usage() -> libc::rusage {
    // SAFETY: getrusage fills the struct it is given.
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        u
    }
}

/// User + system CPU seconds of this process.
fn cpu_secs() -> f64 {
    let u = usage();
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(u.ru_utime) + t(u.ru_stime)
}

/// Peak resident set, MB (ru_maxrss is KB on Linux, bytes on macOS).
fn peak_rss_mb() -> f64 {
    let m = usage().ru_maxrss as f64;
    if cfg!(target_os = "macos") {
        m / 1e6
    } else {
        m / 1024.0
    }
}

fn pct(xs: &mut [f64], p: f64) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[((xs.len() as f64 - 1.0) * p).round() as usize]
}

fn texts() -> (Vec<String>, Vec<String>) {
    #[derive(serde::Deserialize)]
    struct Texts {
        passages: Vec<String>,
        queries: Vec<String>,
    }
    if let Some(p) = std::env::var_os("PERF_TEXTS") {
        let t: Texts = serde_json::from_slice(&std::fs::read(p).expect("PERF_TEXTS")).expect("texts json");
        return (t.passages, t.queries);
    }
    #[derive(serde::Deserialize)]
    struct Fixture {
        messages: Vec<serde_json::Value>,
        queries: Vec<serde_json::Value>,
    }
    use penguin_semantic::chunk::{chunk_mail, ChunkConfig, MailDoc};
    let f: Fixture = serde_json::from_str(include_str!("../tests/fixtures/email_eval.json")).unwrap();
    let cfg = ChunkConfig::default();
    let mut passages = Vec::new();
    for m in &f.messages {
        let doc = MailDoc {
            subject: m["subject"].as_str().unwrap(),
            from_name: None,
            from_email: m["from"].as_str().unwrap(),
            authored: m["body"].as_str().unwrap(),
            filenames: &[],
            bulk: false,
        };
        passages.extend(chunk_mail(&doc, &cfg));
    }
    let queries = f.queries.iter().map(|q| q["q"].as_str().unwrap().to_string()).collect();
    (passages, queries)
}

/// A fixed pseudo-random sample of `n` passages, in random order (the app
/// embeds mail newest first, not sorted by length).
fn sample(all: &[String], n: usize) -> Vec<&str> {
    let mut idx: Vec<usize> = (0..all.len()).collect();
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    for i in (1..idx.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        idx.swap(i, (s % (i as u64 + 1)) as usize);
    }
    idx[..n.min(all.len())].iter().map(|&i| all[i].as_str()).collect()
}

/// The default model with another token cap (0 = unchanged).
fn spec_with_cap(max_tokens: usize) -> &'static ModelSpec {
    if max_tokens == 0 || max_tokens == DEFAULT.max_tokens {
        DEFAULT
    } else {
        Box::leak(Box::new(ModelSpec { max_tokens, ..*DEFAULT }))
    }
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::var_os("PENGUIN_SEMANTIC_MODEL_DIR").expect("PENGUIN_SEMANTIC_MODEL_DIR"));
    assert!(model::verify(DEFAULT, &dir).is_empty(), "model files don't match the spec");
    let (all, queries) = texts();
    match std::env::var("PERF_MODE").as_deref().unwrap_or("sweep") {
        "tokenizer" => tokenizer_only(&dir, &all),
        "cycles" => cycles(&dir, &all),
        "padding" => padding(&dir, &sample(&all, one("PERF_N", 96)), &queries),
        "sweep" => sweep(&dir, &all, &queries),
        other => panic!("unknown PERF_MODE {other:?} (sweep, tokenizer, cycles, padding)"),
    }
}

fn sweep(dir: &std::path::Path, all: &[String], queries: &[String]) {
    let n = one("PERF_N", 480).min(all.len());
    let sample = sample(all, n);
    // Ask compares up to 96 sentences with the question.
    let sentences: Vec<String> = all
        .iter()
        .flat_map(|p| p.split(". ").map(str::trim).filter(|s| s.split_whitespace().count() >= 4))
        .step_by(97)
        .take(96)
        .map(str::to_string)
        .collect();
    let sentences: Vec<&str> = sentences.iter().map(String::as_str).collect();
    let group = one("PERF_GROUP", 30).max(1);
    let reps = one("PERF_QUERY_REPS", 3);
    println!(
        "{} passages ({} in the source), groups of {group}, {} queries × {reps}\n",
        n,
        all.len(),
        queries.len()
    );
    // Reference vectors from the CPU provider, to check accelerators agree.
    let check: Vec<&str> = sample.iter().take(48).copied().collect();
    let accels: Vec<Accel> = std::env::var("PERF_ACCEL")
        .unwrap_or_else(|_| "cpu".into())
        .split(',')
        .map(|a| Accel::parse(a).unwrap_or_else(|| panic!("unknown accelerator {a:?}")))
        .collect();
    let reference = accels.iter().any(|a| *a != Accel::Cpu).then(|| {
        let e = OnnxEmbedder::load(dir, DEFAULT, EmbedderOptions { background_qos: false, ..Default::default() }).unwrap();
        let mut v = e.embed_passages(&check).unwrap();
        v.extend(queries.iter().take(48).map(|q| e.embed_query(q).unwrap()));
        v
    });
    let mut combos = Vec::new();
    for &accel in &accels {
        for &threads in &list("PERF_THREADS", "4") {
            for &batch_tokens in &list("PERF_BATCH_TOKENS", "0") {
                for &max_batch in &list("PERF_MAX_BATCH", "0") {
                    for &spin in &list("PERF_SPIN", "0") {
                        for &max_tokens in &list("PERF_MAX_TOKENS", "0") {
                            combos.push((accel, threads, batch_tokens, max_batch, spin, max_tokens));
                        }
                    }
                }
            }
        }
    }
    println!("| accel | threads | batch tokens | max batch | spin | max tokens | load s | passages/s | tokens/s | padded / real | CPU s per passage | query p50 / p95 ms | query CPU ms | Ask, 96 sentences, idle: background / foreground path ms | same, while indexing | min cosine vs CPU | RSS MB: after load / after passages / peak |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (accel, threads, batch_tokens, max_batch, spin, max_tokens) in combos {
        let mut opts = EmbedderOptions { threads, background_qos: false, accel, ..Default::default() };
        if batch_tokens > 0 {
            opts.batch_tokens = batch_tokens;
        }
        if max_batch > 0 {
            opts.max_batch = max_batch;
        }
        opts.spinning = spin != 0;
        let spec = spec_with_cap(max_tokens);
        let rss0 = rss_mb();
        let t = Instant::now();
        let e = OnnxEmbedder::load(dir, spec, opts).unwrap();
        let load = t.elapsed().as_secs_f64();
        let rss_load = rss_mb() - rss0;
        e.embed_query("warm up").unwrap();
        e.embed_passages(&sample[..group.min(n)]).unwrap();
        let agree = reference.as_ref().map(|r| {
            let mut v = e.embed_passages(&check).unwrap();
            v.extend(queries.iter().take(48).map(|q| e.embed_query(q).unwrap()));
            v.iter().zip(r).map(|(a, b)| penguin_semantic::dot(a, b)).fold(f32::MAX, f32::min)
        });
        let stats0 = e.stats();
        let (t, c) = (Instant::now(), cpu_secs());
        for g in sample.chunks(group) {
            e.embed_passages(g).unwrap();
        }
        let secs = t.elapsed().as_secs_f64();
        let cpu = cpu_secs() - c;
        let st = e.stats().since(&stats0);
        let rss_run = rss_mb() - rss0;
        let mut ms = Vec::new();
        let c = cpu_secs();
        for _ in 0..reps {
            for q in queries {
                let t = Instant::now();
                e.embed_query(q).unwrap();
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let qcpu = (cpu_secs() - c) * 1000.0 / ms.len() as f64;
        // Ask: the question's vector, then its sentences.
        let ask = |now: bool| {
            e.embed_query("when does the lease renew").unwrap();
            let t = Instant::now();
            if now {
                e.embed_passages_now(&sentences).unwrap();
            } else {
                e.embed_passages(&sentences).unwrap();
            }
            t.elapsed().as_secs_f64() * 1000.0
        };
        let (ask_bg, ask_now) = (ask(false), ask(true));
        // The same while the indexer embeds on another thread.
        let busy = |now: bool| {
            let stop = AtomicBool::new(false);
            std::thread::scope(|s| {
                s.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        e.embed_passages(&sample[..group.min(n)]).unwrap();
                    }
                });
                std::thread::sleep(Duration::from_millis(500));
                let ms = ask(now);
                stop.store(true, Ordering::Relaxed);
                ms
            })
        };
        let (busy_bg, busy_now) = (busy(false), busy(true));
        println!(
            "| {:?} | {threads} | {} | {} | {spin} | {} | {load:.1} | {:.2} | {:.0} | {:.2} | {:.2} | {:.1} / {:.1} | {:.0} | {ask_bg:.0} / {ask_now:.0} | {busy_bg:.0} / {busy_now:.0} | {} | {:.0} / {:.0} / {:.0} |",
            e.options().accel,
            e.options().batch_tokens,
            e.options().max_batch,
            spec.max_tokens,
            n as f64 / secs,
            st.tokens as f64 / secs,
            st.padded_tokens as f64 / st.tokens.max(1) as f64,
            cpu / n as f64,
            pct(&mut ms, 0.5),
            pct(&mut ms, 0.95),
            qcpu,
            agree.map_or("–".to_string(), |c| format!("{c:.5}")),
            rss_load,
            rss_run,
            peak_rss_mb(),
        );
    }
}

/// Cosine of each pair of unit vectors: (min, mean).
fn agreement(a: &[Vec<f32>], b: &[Vec<f32>]) -> (f32, f32) {
    let c: Vec<f32> = a.iter().zip(b).map(|(x, y)| penguin_semantic::dot(x, y)).collect();
    (c.iter().copied().fold(f32::MAX, f32::min), c.iter().sum::<f32>() / c.len().max(1) as f32)
}

/// `PERF_MODE=padding`: the same passages embedded alone and in batches.
fn padding(dir: &std::path::Path, sample: &[&str], queries: &[String]) {
    let embed = |batch_tokens: usize, max_batch: usize| {
        let opts = EmbedderOptions { background_qos: false, batch_tokens, max_batch, ..Default::default() };
        let e = OnnxEmbedder::load(dir, DEFAULT, opts).unwrap();
        let v = e.embed_passages(sample).unwrap();
        let st = e.stats();
        (v, st.padded_tokens as f64 / st.tokens.max(1) as f64)
    };
    let d = EmbedderOptions::default();
    let (alone, _) = embed(usize::MAX, 1);
    let (again, _) = embed(usize::MAX, 1);
    println!(
        "{} passages, {} thread(s), each compared with itself embedded alone (no padding).\n",
        sample.len(),
        d.threads
    );
    println!("| batching | padded / real tokens | min cosine | mean cosine |");
    println!("|---|---:|---:|---:|");
    let (lo, mean) = agreement(&alone, &again);
    println!("| alone, a second time | 1.00 | {lo:.6} | {mean:.6} |");
    for (name, bt, mb) in [
        (format!("default ({} tokens, ≤ {} passages)", d.batch_tokens, d.max_batch), d.batch_tokens, d.max_batch),
        ("pieces of 2".to_string(), usize::MAX, 2),
        ("16 at a time (the old batching)".to_string(), usize::MAX, 16),
        ("one wide batch".to_string(), usize::MAX, usize::MAX),
    ] {
        let (v, pad) = embed(bt, mb);
        let (lo, mean) = agreement(&alone, &v);
        println!("| {name} | {pad:.2} | {lo:.6} | {mean:.6} |");
    }
    // Batch size alone, without padding: each passage twice in one run.
    let opts = EmbedderOptions { background_qos: false, batch_tokens: usize::MAX, max_batch: 2, ..Default::default() };
    let e = OnnxEmbedder::load(dir, DEFAULT, opts).unwrap();
    let twice: Vec<Vec<f32>> = sample.iter().map(|p| e.embed_passages(&[p, p]).unwrap().remove(0)).collect();
    let (lo, mean) = agreement(&alone, &twice);
    println!("| the same passage twice in one run (2 rows, no padding) | 1.00 | {lo:.6} | {mean:.6} |");
    drop(e);
    // Queries always run alone: only run-to-run determinism to check.
    let e = OnnxEmbedder::load(dir, DEFAULT, EmbedderOptions { background_qos: false, ..Default::default() }).unwrap();
    let a: Vec<Vec<f32>> = queries.iter().take(48).map(|q| e.embed_query(q).unwrap()).collect();
    let b: Vec<Vec<f32>> = queries.iter().take(48).map(|q| e.embed_query(q).unwrap()).collect();
    let (lo, mean) = agreement(&a, &b);
    println!("| queries, twice | 1.00 | {lo:.6} | {mean:.6} |");
}

/// `PERF_MODE=tokenizer`: the tokenizer alone: resident memory, speed, and
/// the passages' token lengths.
fn tokenizer_only(dir: &std::path::Path, passages: &[String]) {
    let (r0, f0) = (rss_mb(), footprint_mb());
    let t = Instant::now();
    let tok = tokenizers::Tokenizer::from_file(dir.join(DEFAULT.tokenizer_file)).unwrap();
    let load = t.elapsed().as_secs_f64();
    let (r1, f1) = (rss_mb(), footprint_mb());
    penguin_semantic::mem::release_freed();
    let (r2, f2) = (rss_mb(), footprint_mb());
    let t = Instant::now();
    let mut lens: Vec<usize> = passages
        .iter()
        .map(|p| tok.encode_fast(format!("{}{p}", DEFAULT.passage_prefix), true).unwrap().len())
        .collect();
    let secs = t.elapsed().as_secs_f64();
    let (r3, f3) = (rss_mb(), footprint_mb());
    lens.sort_unstable();
    let q = |p: f64| lens[((lens.len() - 1) as f64 * p).round() as usize];
    let over = |cap: usize| 100.0 * lens.iter().filter(|&&l| l > cap).count() as f64 / lens.len() as f64;
    println!("| | |\n|---|---:|");
    println!("| Tokenizer load | {load:.2} s |");
    println!("| Growth after load: resident / footprint | {:.0} / {:.0} MB |", r1 - r0, f1 - f0);
    println!("| … after mem::release_freed (freed heap handed back) | {:.0} / {:.0} MB |", r2 - r0, f2 - f0);
    println!("| … after tokenizing every passage | {:.0} / {:.0} MB |", r3 - r0, f3 - f0);
    println!("| Peak RSS | {:.0} MB |", peak_rss_mb());
    println!(
        "| Tokenize {} passages | {:.2} s ({:.0} passages/s) |",
        passages.len(),
        secs,
        passages.len() as f64 / secs
    );
    println!(
        "| Tokens per passage (with the prompt) | mean {:.1}, p50 {}, p90 {}, p99 {}, max {} |",
        lens.iter().sum::<usize>() as f64 / lens.len() as f64,
        q(0.5),
        q(0.9),
        q(0.99),
        q(1.0)
    );
    println!(
        "| Passages over 128 / 192 / 256 tokens | {:.1}% / {:.1}% / {:.1}% |",
        over(128),
        over(192),
        over(256)
    );
    drop(tok);
}

/// `PERF_MODE=cycles`: the BPE word cache lives in thread-locals
/// (tokenizers 0.23) that nothing frees. Load the tokenizer, tokenize
/// every passage and drop it `PERF_CYCLES` times on one thread, as the
/// app's indexer thread does across idle unloads, and print the memory
/// left behind after each cycle (resident set; footprint on macOS).
/// `PERF_BPE_CACHE`: the cache capacity (unset = the crate's default,
/// 10,000 words).
fn cycles(dir: &std::path::Path, passages: &[String]) {
    let capacity: Option<usize> = std::env::var("PERF_BPE_CACHE").ok().and_then(|c| c.parse().ok());
    let n = one("PERF_CYCLES", 5);
    penguin_semantic::mem::release_freed();
    let base = footprint_mb();
    let mut after = Vec::new();
    let mut rate = 0.0;
    for _ in 0..n {
        let mut tok = tokenizers::Tokenizer::from_file(dir.join(DEFAULT.tokenizer_file)).unwrap();
        if let Some(c) = capacity {
            let mut m = tok.get_model().clone();
            m.resize_cache(c);
            tok.with_model(m);
        }
        let t = Instant::now();
        for p in passages {
            tok.encode_fast(format!("{}{p}", DEFAULT.passage_prefix), true).unwrap();
        }
        rate = passages.len() as f64 / t.elapsed().as_secs_f64();
        drop(tok);
        penguin_semantic::mem::release_freed();
        after.push(format!("{:.0}", footprint_mb() - base));
    }
    println!(
        "| BPE cache {}: memory left after each of {n} load / tokenize / drop cycles | {} MB | {rate:.0} passages/s |",
        capacity.map_or("default".to_string(), |c| c.to_string()),
        after.join(", ")
    );
}
