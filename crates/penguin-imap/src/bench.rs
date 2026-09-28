//! Sync benchmarks (ignored by default; run them in release):
//!
//!   scripts/box-cargo.sh test --release -p penguin-imap --lib bench_ -- --ignored --nocapture --test-threads=1
//!
//! - `bench_imap_parse`: CPU and allocations per message for `BODY[]` →
//!   `Message` (mime.rs) over the synthetic corpus as RFC 5322 bytes.
//! - `bench_imap_backfill`: the real sync engine filling INBOX from the
//!   scripted server (testserver.rs) over loopback TCP: messages/s, IMAP
//!   commands and bytes on the wire per message, with the server holding
//!   each reply for `PENGUIN_BENCH_RTT_MS` (default 0; try 30) to stand in
//!   for a real network's round trip, and pacing replies to
//!   `PENGUIN_BENCH_MBPS` (default unlimited). `PENGUIN_BENCH_CAPS` adds
//!   capabilities (e.g. `COMPRESS=DEFLATE`).
//!
//! Each prints one `BENCH {json}` line. `PENGUIN_BENCH_N` sets the number of
//! messages (default 2000).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use penguin_core::Message;
use penguin_provider::window::WindowPolicy;

use crate::mime::{self, Meta};
use crate::tests::Env;
use crate::testserver::GENERIC_CAPS;

#[path = "../../penguin-core/examples/support/corpus.rs"]
mod corpus;
#[path = "../../penguin-core/examples/support/wire.rs"]
mod wire;

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `n` corpus messages, newest first.
fn corpus_messages(n: usize) -> Vec<Message> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let mut c = corpus::Corpus::new(42, now);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        for mut m in c.thread(0) {
            m.account_id = crate::testserver::USER.into();
            out.push(m);
        }
    }
    out.truncate(n);
    out.sort_by(|a, b| b.date.cmp(&a.date));
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
#[ignore]
fn bench_imap_parse() {
    let n = env_usize("PENGUIN_BENCH_N", 2000);
    let messages = corpus_messages(n);
    let raws: Vec<Vec<u8>> = messages.iter().map(wire::rfc822).collect();
    let bytes: usize = raws.iter().map(Vec::len).sum();
    let meta = |m: &Message| Meta {
        account_id: m.account_id.clone(),
        id: m.id.clone(),
        thread_id: String::new(),
        date_ms: Some(m.date),
        labels: Vec::new(),
        trusted_authserv: None,
    };
    let metas: Vec<Meta> = messages.iter().map(meta).collect();
    for ((m, raw), meta) in messages.iter().zip(&raws).zip(&metas).take(200) {
        let got = mime::to_message(raw, meta);
        assert_eq!(got.subject, m.subject);
        assert_eq!(got.attachments.len(), m.attachments.len(), "{}", m.id);
    }
    for (raw, meta) in raws.iter().zip(&metas) {
        std::hint::black_box(mime::to_message(raw, meta));
    }
    let mut rounds = Vec::new();
    for _ in 0..7 {
        let t = Instant::now();
        for (raw, meta) in raws.iter().zip(&metas) {
            std::hint::black_box(mime::to_message(raw, meta));
        }
        rounds.push(t.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
    let (a0, b0) = (
        ALLOCS.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    );
    for (raw, meta) in raws.iter().zip(&metas) {
        std::hint::black_box(mime::to_message(raw, meta));
    }
    let (a1, b1) = (
        ALLOCS.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    );
    // What COMPRESS=DEFLATE puts on the wire for the same bytes (one
    // context for the whole session, a sync flush per message as a server
    // does per response), and what inflating it costs the client.
    let mut z = flate2::Compress::new(flate2::Compression::new(6), false);
    let mut stream = Vec::with_capacity(bytes);
    for r in &raws {
        let mut at = 0usize;
        loop {
            stream.reserve(r.len() / 2 + 4096);
            let before = z.total_in();
            z.compress_vec(&r[at..], &mut stream, flate2::FlushCompress::Sync)
                .unwrap();
            at += (z.total_in() - before) as usize;
            if at == r.len() && stream.len() < stream.capacity() {
                break;
            }
        }
    }
    let deflated = stream.len();
    let mut inflate_rounds = Vec::new();
    let mut out = vec![0u8; 64 * 1024];
    for _ in 0..5 {
        let mut d = flate2::Decompress::new(false);
        let t = Instant::now();
        let mut at = 0usize;
        while at < stream.len() {
            let before = d.total_in();
            d.decompress(&stream[at..], &mut out, flate2::FlushDecompress::None)
                .unwrap();
            at += (d.total_in() - before) as usize;
        }
        assert_eq!(d.total_out() as usize, bytes);
        inflate_rounds.push(t.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
    let out = serde_json::json!({
        "bench": "imap_parse",
        "messages": n,
        "rfc822_bytes_per_msg": bytes / n,
        "deflate_bytes_per_msg": deflated / n,
        "inflate_us_per_msg_p50": median(inflate_rounds),
        "parse_us_per_msg_p50": median(rounds),
        "allocs_per_msg": (a1 - a0) as f64 / n as f64,
        "alloc_kb_per_msg": (b1 - b0) as f64 / n as f64 / 1024.0,
    });
    println!("BENCH {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_imap_backfill() {
    let n = env_usize("PENGUIN_BENCH_N", 2000);
    let rtt = Duration::from_millis(env_usize("PENGUIN_BENCH_RTT_MS", 0) as u64);
    let caps: Vec<&str> = match std::env::var("PENGUIN_BENCH_CAPS") {
        Ok(extra) if !extra.is_empty() => GENERIC_CAPS
            .iter()
            .copied()
            .chain(
                extra
                    .split(',')
                    .map(|s| &*Box::leak(s.to_string().into_boxed_str())),
            )
            .collect(),
        _ => GENERIC_CAPS.to_vec(),
    };
    let env = Env::new(&caps, false, "fastmail").await;
    let messages = corpus_messages(n);
    let mut raw_bytes = 0usize;
    // Oldest first, so UIDs rise with date as on a real server.
    for m in messages.iter().rev() {
        let raw = wire::rfc822(m);
        raw_bytes += raw.len();
        env.server.deliver("INBOX", &raw, &["\\Seen"], m.date);
    }
    let mbps = env_usize("PENGUIN_BENCH_MBPS", 0);
    {
        let mut st = env.server.state.lock().unwrap();
        // What a real server has in its index by now (structures, part
        // offsets): the benchmark times the client, not the test server.
        for b in &st.mailboxes {
            for m in &b.msgs {
                m.parsed();
            }
        }
        st.latency = rtt;
        st.bandwidth = mbps as u64 * 1_000_000 / 8;
        st.bytes_out = 0;
        st.wire_out.store(0, Ordering::Relaxed);
        st.commands.clear();
    }
    let t = Instant::now();
    env.sync(WindowPolicy::EVERYTHING).await;
    let secs = t.elapsed().as_secs_f64();
    let (bytes_out, wire_out, commands) = {
        let st = env.server.state.lock().unwrap();
        (
            st.bytes_out,
            st.wire_out.load(Ordering::Relaxed),
            st.commands.clone(),
        )
    };
    assert_eq!(env.count() as usize, n);
    let mut by_cmd: std::collections::BTreeMap<String, usize> = Default::default();
    for c in &commands {
        *by_cmd.entry(c.clone()).or_default() += 1;
    }
    let out = serde_json::json!({
        "bench": "imap_backfill",
        "messages": n,
        "caps": caps,
        "rtt_ms": rtt.as_millis() as u64,
        "seconds": secs,
        "messages_per_s": n as f64 / secs,
        "rfc822_bytes_per_msg": raw_bytes / n,
        "mbps": mbps,
        "reply_bytes_per_msg": bytes_out as usize / n,
        "wire_bytes_per_msg": wire_out as usize / n,
        "commands": commands.len(),
        "commands_by_kind": by_cmd,
    });
    println!("BENCH {out}");
}

/// The cost of a full poll when nothing changed (every 5 minutes, per
/// account, for the app's whole life): `PENGUIN_BENCH_FOLDERS` folders
/// (default 30) with 20 messages each, polled 7 times.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_imap_poll() {
    let folders = env_usize("PENGUIN_BENCH_FOLDERS", 30);
    let rtt = Duration::from_millis(env_usize("PENGUIN_BENCH_RTT_MS", 0) as u64);
    let caps: Vec<&str> = match std::env::var("PENGUIN_BENCH_CAPS") {
        Ok(c) if !c.is_empty() => c
            .split(',')
            .map(|s| &*Box::leak(s.to_string().into_boxed_str()))
            .collect(),
        _ => GENERIC_CAPS.to_vec(),
    };
    let env = Env::new(&caps, false, "fastmail").await;
    let messages = corpus_messages(folders * 20);
    for (i, m) in messages.iter().enumerate() {
        let name = format!("Projects/P{}", i % folders);
        if i < folders {
            env.server.add_folder(&name);
        }
        env.server
            .deliver(&name, &wire::rfc822(m), &["\\Seen"], m.date);
    }
    let mut e = env.sync(WindowPolicy::EVERYTHING).await;
    e.poll(true).await.unwrap();
    env.server.state.lock().unwrap().latency = rtt;
    let mut times = Vec::new();
    let mut per_poll = 0;
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for _ in 0..7 {
        env.server.state.lock().unwrap().commands.clear();
        let t = Instant::now();
        e.poll(true).await.unwrap();
        times.push(t.elapsed().as_secs_f64() * 1e3);
        let cmds = env.server.state.lock().unwrap().commands.clone();
        per_poll = cmds.len();
        kinds.clear();
        for c in cmds {
            *kinds.entry(c).or_default() += 1;
        }
    }
    let out = serde_json::json!({
        "bench": "imap_poll",
        "caps": caps,
        "folders": folders + 7,
        "rtt_ms": rtt.as_millis() as u64,
        "poll_ms_p50": median(times),
        "commands_per_poll": per_poll,
        "commands_by_kind": kinds,
    });
    println!("BENCH {out}");
}
