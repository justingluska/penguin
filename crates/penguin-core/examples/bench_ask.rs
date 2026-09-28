//! Ask latency over a synthetic mailbox, answered from a read-only store
//! (how the MCP server opens it).
//!
//!   cargo run -p penguin-core --release --example bench_ask
//!
//! Env: PENGUIN_BENCH_N (messages, default 300000), PENGUIN_BENCH_DIR
//! (default target/bench-ask-db), PENGUIN_BENCH_REUSE=1 to reuse the db.
//! Target: every question under 100 ms at p95.

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/story.rs"]
mod story;

use std::time::Instant;

use penguin_core::ask::AskScope;
use penguin_core::{Account, Store};

const RUNS: usize = 20;

fn main() {
    let n: usize = std::env::var("PENGUIN_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300_000);
    let dir = std::path::PathBuf::from(
        std::env::var("PENGUIN_BENCH_DIR").unwrap_or_else(|_| "target/bench-ask-db".into()),
    );
    let reuse = std::env::var("PENGUIN_BENCH_REUSE").is_ok_and(|v| v == "1");
    let path = dir.join(format!("ask-{n}.db"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let mut corpus = corpus::Corpus::new(42, now);
    if !reuse || !path.exists() {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&path).unwrap();
        for (i, a) in corpus::ACCOUNTS.iter().enumerate() {
            store
                .upsert_account(&Account {
                    id: a.to_string(),
                    email: a.to_string(),
                    display_name: Some("Alex".into()),
                    nickname: None,
                    color: "#0a84ff".into(),
                    added_at: i as i64,
                    ..Account::default()
                })
                .unwrap();
        }
        let started = Instant::now();
        let mut batch = story::story(now);
        let mut total = batch.len();
        while total < n {
            let acct = [0, 0, 0, 0, 0, 0, 1, 1, 1, 2][total % 10];
            let mut th = corpus.thread(acct);
            th.truncate(n - total);
            total += th.len();
            batch.extend(th);
            if batch.len() >= 500 || total >= n {
                store.upsert_messages(&batch).unwrap();
                batch.clear();
            }
        }
        store.optimize().unwrap();
        eprintln!(
            "built {total} messages in {:.1}s",
            started.elapsed().as_secs_f64()
        );
    }

    // Structured extraction (the app's background scanner): the one-time
    // backfill over the whole mailbox, then a pass with nothing new.
    {
        let size = |p: &std::path::Path| {
            ["", "-wal"]
                .iter()
                .map(|s| std::fs::metadata(format!("{}{s}", p.display())).map_or(0, |m| m.len()))
                .sum::<u64>()
        };
        let w = Store::open(&path).unwrap();
        let pending = w.extract_remaining().unwrap();
        let bytes0 = size(&path);
        let t = Instant::now();
        let (mut scanned, mut found) = (0usize, 0usize);
        loop {
            let p = w.extract_pending(2_000).unwrap();
            scanned += p.scanned;
            found += p.found;
            if p.remaining == 0 || p.scanned == 0 {
                break;
            }
        }
        let secs = t.elapsed().as_secs_f64();
        if scanned > 0 {
            println!(
                "## Fact extraction\n\nbackfill: {scanned} of {pending} messages in {secs:.1} s ({:.0} messages/s, {:.3} ms each), {found} facts, database +{:.1} MB\n",
                scanned as f64 / secs.max(1e-9),
                secs * 1000.0 / scanned as f64,
                (size(&path) as f64 - bytes0 as f64) / 1e6
            );
        }
        let t = Instant::now();
        let p = w.extract_pending(2_000).unwrap();
        println!(
            "caught up: a pass with nothing to read took {:.2} ms ({} scanned)\n",
            t.elapsed().as_secs_f64() * 1000.0,
            p.scanned
        );
    }

    let ro = Store::open_read_only(&path).unwrap();
    let p = &corpus.people;
    let questions = [
        "when did I last speak to Mike from Kettle on the Knoll / Fernwood?".to_string(),
        "when did we work for Mike KOTK".to_string(),
        format!("when did I last email {}", p[3].name),
        format!("first email from {}", p[11].email),
        format!("how long have I known {}", p[7].email),
        format!("who is {}", p[20].name),
        format!("how many emails from {} this year", p[5].name),
        "how much did I spend on Uber this year".to_string(),
        format!("latest pdf from {}", p[2].name),
        "latest invoice".to_string(),
        "what am I waiting on".to_string(),
        "what do I owe replies to".to_string(),
        "who emails me the most".to_string(),
        "who emailed me about the budget".to_string(),
        "how many emails about the budget".to_string(),
        "when is the dinner".to_string(),
        "why is the sky blue".to_string(),
        "when is my flight to Lisbon".to_string(),
        "where is my package".to_string(),
        "what bills are due".to_string(),
        "what did I order from Uber".to_string(),
        "what did the team say about the budget".to_string(),
        "what is the wifi password for the offsite".to_string(),
        "¿Cuánto gasté en Uber este año?".to_string(),
    ];
    let scope = AskScope::default();
    println!("## Ask latency ({n} messages, read-only store, {RUNS} runs each)\n");
    println!("| question | p50 ms | p95 ms | max ms | answer |\n|---|---:|---:|---:|---|");
    let mut worst: f64 = 0.0;
    for q in &questions {
        let mut ms = Vec::with_capacity(RUNS);
        let mut headline = String::new();
        for _ in 0..RUNS {
            let t = Instant::now();
            let a = ro.ask(q, &scope, now, 0).unwrap();
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            headline = a.headline;
        }
        ms.sort_by(|a, b| a.total_cmp(b));
        let pct = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        worst = worst.max(pct(0.95));
        let h: String = headline.chars().take(70).collect();
        println!(
            "| {q} | {:.1} | {:.1} | {:.1} | {h} |",
            pct(0.5),
            pct(0.95),
            ms[ms.len() - 1]
        );
    }
    println!("\nworst p95: {worst:.1} ms (target < 100 ms)");
    if worst >= 100.0 {
        std::process::exit(1);
    }
}
