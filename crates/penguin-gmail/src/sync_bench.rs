//! Sync benchmarks (ignored by default; run them in release):
//!
//!   scripts/box-cargo.sh test --release -p penguin-gmail --lib bench_ -- --ignored --nocapture --test-threads=1
//!
//! - `bench_gmail_parse`: CPU, allocations and bytes per message for
//!   `messages.get?format=full` JSON → `Message`, over the synthetic corpus
//!   (penguin-core/examples/support) encoded the way Gmail sends it.
//! - `bench_gmail_backfill`: the real `AccountSync` backfill loop against an
//!   in-process Gmail (no network, no quota) and an on-disk store: the
//!   engine's own ceiling in messages/s, and how much it slows the UI's
//!   reads and writes while it runs.
//!
//! Each prints one `BENCH {json}` line. `PENGUIN_BENCH_N` sets the number
//! of messages (default 3000), `PENGUIN_BENCH_LATENCY_MS` a per-call delay
//! for the fake Gmail (default 0).

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Instant as StdInstant;

use penguin_core::{ListQuery, MailboxView};

use super::*;
use crate::convert::{self, GmailMessage};

#[path = "../../penguin-core/examples/support/corpus.rs"]
mod corpus;
#[path = "../../penguin-core/examples/support/wire.rs"]
mod wire;

// ---------- allocation counting (this test binary only) ----------

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, AtomicOrdering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, AtomicOrdering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, AtomicOrdering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, AtomicOrdering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> (u64, u64) {
    (
        ALLOCS.load(AtomicOrdering::Relaxed),
        ALLOC_BYTES.load(AtomicOrdering::Relaxed),
    )
}

// ---------- corpus ----------

const BENCH_ACCT: &str = "alex@work.example";

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `n` corpus messages of one account, newest first (Gmail's list order).
fn corpus_messages(n: usize) -> Vec<Message> {
    let now = 1_790_000_000_000;
    let mut c = corpus::Corpus::new(42, now);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        for mut m in c.thread(0) {
            m.account_id = BENCH_ACCT.into();
            out.push(m);
        }
    }
    out.truncate(n);
    out.sort_by(|a, b| b.date.cmp(&a.date));
    out
}

fn gzip_len(bytes: &[u8]) -> usize {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    e.write_all(bytes).unwrap();
    e.finish().unwrap().len()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// What the client does with a `format=full` body: JSON → wire struct →
/// Message (the same calls as `GmailClient::get_one`).
fn decode(json: &[u8]) -> Message {
    let gm: GmailMessage = serde_json::from_slice(json).expect("json");
    assert!(convert::out_of_line_bodies(&gm).is_empty());
    convert::to_message(BENCH_ACCT, &gm, &HashMap::new())
}

#[test]
#[ignore]
fn bench_gmail_parse() {
    let n = env_usize("PENGUIN_BENCH_N", 3000);
    let messages = corpus_messages(n);
    let wire: Vec<Vec<u8>> = messages.iter().map(wire::gmail_full_json).collect();
    let json_bytes: usize = wire.iter().map(Vec::len).sum();
    let gzip_bytes: usize = wire.iter().map(|w| gzip_len(w)).sum();

    // Correctness guard: what comes out is what went in.
    for (m, w) in messages.iter().zip(&wire).take(200) {
        let got = decode(w);
        assert_eq!(got.id, m.id);
        assert_eq!(got.subject, m.subject);
        assert_eq!(got.body_html, m.body_html);
        assert_eq!(
            got.attachments.len(),
            m.attachments.len(),
            "attachments of {}",
            m.id
        );
    }

    // Warm up, then 7 timed rounds.
    for w in &wire {
        std::hint::black_box(decode(w));
    }
    let mut rounds = Vec::new();
    let mut deser_rounds = Vec::new();
    for _ in 0..7 {
        let t = StdInstant::now();
        for w in &wire {
            std::hint::black_box(serde_json::from_slice::<GmailMessage>(w).unwrap());
        }
        deser_rounds.push(t.elapsed().as_secs_f64() * 1e6 / n as f64);
        let t = StdInstant::now();
        for w in &wire {
            std::hint::black_box(decode(w));
        }
        rounds.push(t.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
    let (a0, b0) = allocs();
    for w in &wire {
        std::hint::black_box(decode(w));
    }
    let (a1, b1) = allocs();
    let out = serde_json::json!({
        "bench": "gmail_parse",
        "messages": n,
        "json_bytes_per_msg": json_bytes / n,
        "gzip_bytes_per_msg": gzip_bytes / n,
        "decode_us_per_msg_p50": median(rounds.clone()),
        "decode_us_per_msg_min": rounds.iter().cloned().fold(f64::MAX, f64::min),
        "deserialize_only_us_per_msg_p50": median(deser_rounds),
        "allocs_per_msg": (a1 - a0) as f64 / n as f64,
        "alloc_kb_per_msg": (b1 - b0) as f64 / n as f64 / 1024.0,
    });
    println!("BENCH {out}");
}

// ---------- in-process Gmail for the engine benchmark ----------

#[derive(Clone)]
struct WireGmail {
    /// (id, thread id, format=full JSON), newest first.
    mail: Arc<Vec<(String, String, Vec<u8>)>>,
    latency: Duration,
}

impl WireGmail {
    async fn delay(&self) {
        if !self.latency.is_zero() {
            tokio::time::sleep(self.latency).await;
        }
    }
}

impl GmailApi for WireGmail {
    async fn profile(&self) -> Result<Profile> {
        Ok(Profile {
            email: BENCH_ACCT.into(),
            messages_total: self.mail.len() as u64,
            history_id: 100,
        })
    }
    async fn list_message_ids(&self, page_token: Option<&str>, _q: Option<&str>) -> Result<IdPage> {
        self.delay().await;
        let start: usize = page_token.map_or(0, |t| t.parse().unwrap());
        let end = (start + 500).min(self.mail.len());
        Ok(IdPage {
            ids: self.mail[start..end]
                .iter()
                .map(|(id, t, _)| (id.clone(), t.clone()))
                .collect(),
            next_page_token: (end < self.mail.len()).then(|| end.to_string()),
            result_size_estimate: self.mail.len() as u64,
        })
    }
    async fn get_messages_each(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        self.delay().await;
        let by_id: HashMap<&str, &Vec<u8>> = self
            .mail
            .iter()
            .map(|(id, _, w)| (id.as_str(), w))
            .collect();
        ids.iter()
            .map(|id| (id.clone(), Ok(by_id.get(id.as_str()).map(|w| decode(w)))))
            .collect()
    }
    async fn get_message_labels(&self, _ids: &[String]) -> Result<Vec<MessageLabels>> {
        Ok(vec![])
    }
    async fn list_history(&self, start: u64, _t: Option<&str>) -> Result<HistoryPage> {
        Ok(HistoryPage {
            history_id: start,
            ..Default::default()
        })
    }
    async fn list_labels(&self) -> Result<Vec<Label>> {
        Ok(["INBOX", "UNREAD", "STARRED", "SENT", "IMPORTANT", "TRASH"]
            .into_iter()
            .map(|id| Label {
                account_id: BENCH_ACCT.into(),
                id: id.into(),
                name: id.into(),
                kind: "system".into(),
                color: None,
                unread_count: None,
                hidden: false,
            })
            .collect())
    }
    async fn label_messages_total(&self, _label_id: &str) -> Result<u64> {
        Ok(0)
    }
}

struct NullObserver;
impl SyncObserver for NullObserver {
    fn status(&self, _status: SyncStatus) {}
    fn mail_changed(&self, _account_id: &str, _thread_ids: Vec<String>) {}
    fn messages_added(&self, _account_id: &str, _message_ids: Vec<String>) {}
    fn labels_added(&self, _account_id: &str, _changes: Vec<(String, Vec<String>)>) {}
}

/// Latencies (ms) of the UI's reads (Inbox page) and writes (archive a
/// thread and put it back) sampled every 5 ms until `stop`.
fn ui_probe(
    store: Store,
    thread_id: String,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<(Vec<f64>, Vec<f64>)> {
    std::thread::spawn(move || {
        let q = ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 50,
            before: None,
            unread_only: false,
            split: None,
        };
        let (mut reads, mut writes) = (Vec::new(), Vec::new());
        let inbox = vec!["INBOX".to_string()];
        let mut flip = false;
        while !stop.load(Ordering::Relaxed) {
            let t = StdInstant::now();
            std::hint::black_box(store.list_threads(&q).unwrap());
            reads.push(t.elapsed().as_secs_f64() * 1e3);
            let t = StdInstant::now();
            let (add, remove) = if flip {
                (&inbox[..], &[][..])
            } else {
                (&[][..], &inbox[..])
            };
            store
                .modify_thread_labels(BENCH_ACCT, &thread_id, add, remove)
                .unwrap();
            writes.push(t.elapsed().as_secs_f64() * 1e3);
            flip = !flip;
            std::thread::sleep(Duration::from_millis(5));
        }
        (reads, writes)
    })
}

fn stats(label: &str, mut v: Vec<f64>) -> serde_json::Value {
    let n = v.len();
    serde_json::json!({
        label: {
            "n": n,
            "p50_ms": percentile(&mut v, 0.5),
            "p95_ms": percentile(&mut v, 0.95),
            "p99_ms": percentile(&mut v, 0.99),
            "max_ms": percentile(&mut v, 1.0),
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_gmail_backfill() {
    let n = env_usize("PENGUIN_BENCH_N", 3000);
    let latency = Duration::from_millis(env_usize("PENGUIN_BENCH_LATENCY_MS", 0) as u64);
    let messages = corpus_messages(n);
    let mail: Vec<(String, String, Vec<u8>)> = messages
        .iter()
        .map(|m| (m.id.clone(), m.thread_id.clone(), wire::gmail_full_json(m)))
        .collect();
    let dir = std::env::temp_dir().join(format!("penguin-sync-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir.join("mail.db")).unwrap();
    // One thread the UI probe archives and unarchives meanwhile.
    let probe_thread = {
        let mut m = messages[0].clone();
        m.id = "ui-probe".into();
        m.thread_id = "ui-probe-thread".into();
        m.label_ids = vec!["INBOX".into()];
        store.upsert_messages(&[m]).unwrap();
        "ui-probe-thread".to_string()
    };

    // UI alone (idle baseline).
    let stop = Arc::new(AtomicBool::new(false));
    let probe = ui_probe(store.clone(), probe_thread.clone(), stop.clone());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    stop.store(true, Ordering::Relaxed);
    let (idle_reads, idle_writes) = probe.join().unwrap();

    // Backfill with the UI probe running.
    let shared = Shared::new(BENCH_ACCT, Arc::new(NullObserver));
    let client = WireGmail {
        mail: Arc::new(mail),
        latency,
    };
    let mut sync = AccountSync::new(BENCH_ACCT, client, store.clone(), shared);
    let stop = Arc::new(AtomicBool::new(false));
    let probe = ui_probe(store.clone(), probe_thread, stop.clone());
    let (a0, _) = allocs();
    let t = StdInstant::now();
    sync.init().await.unwrap();
    while !sync.cursor.backfill_done {
        sync.backfill_page().await.unwrap();
    }
    let secs = t.elapsed().as_secs_f64();
    let (a1, _) = allocs();
    stop.store(true, Ordering::Relaxed);
    let (busy_reads, busy_writes) = probe.join().unwrap();
    let stored = store.count_messages(Some(BENCH_ACCT)).unwrap();
    assert_eq!(stored as usize, n + 1);

    let mut out = serde_json::json!({
        "bench": "gmail_backfill",
        "messages": n,
        "latency_ms_per_call": latency.as_millis() as u64,
        "seconds": secs,
        "messages_per_s": n as f64 / secs,
        "allocs_per_msg": (a1 - a0) as f64 / n as f64,
    });
    for v in [
        stats("ui_read_idle", idle_reads),
        stats("ui_read_during_sync", busy_reads),
        stats("ui_write_idle", idle_writes),
        stats("ui_write_during_sync", busy_writes),
    ] {
        out.as_object_mut()
            .unwrap()
            .extend(v.as_object().unwrap().clone());
    }
    println!("BENCH {out}");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
