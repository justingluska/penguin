//! Penguin's backend benchmark: builds a deterministic synthetic mailbox and
//! times every local path the UI waits on, then writes machine-readable JSON.
//!
//!   scripts/bench.sh                     # everything, regenerates docs/PERFORMANCE.md
//!   cargo run --release -p penguin-core --example bench -- --corpus 100k
//!
//! Flags:
//!   --corpus 10k|100k|300k|<n>  messages to generate (default 100k)
//!   --out <file.json>           where to write results (default target/bench/backend-<n>.json)
//!   --dir <dir>                 database directory (default target/bench-corpus)
//!   --keep                      keep the database afterwards (default: delete it)
//!   --reuse                     reuse an existing database instead of rebuilding
//!   --quick                     fewer repetitions (a smoke run, noisier numbers)
//!
//! What it measures (all in-process, release build, no network):
//!   ingest    store.upsert_messages in 100-message transactions (sync's chunk
//!             size), i.e. the local half of sync: rows + FTS5 + thread views;
//!             then Store::optimize with writes issued meanwhile (how long
//!             they wait); and a 20k side test of arrival order (generator
//!             order vs newest first, the order a Gmail backfill arrives in)
//!   disk      database size after an FTS optimize, per 100k messages
//!   open      Store::open → first Inbox page, with the OS page cache evicted
//!             (Linux) and warm
//!   list      list_threads for Inbox / All / labels / unread-only, and
//!             infinite scroll (40 consecutive 50-row pages)
//!   thread    opening a thread the way get_thread does: store read, then
//!             sanitize + render each message (penguin-render), OTP and
//!             unsubscribe detection
//!   render    penguin-render alone on 50 KB / 200 KB / 2 MB newsletters
//!   search    words, phrases, operators, identifiers, every prefix typed
//!   ask       "Ask your inbox" questions
//!
//! The mailbox is fictional (people at `.example` domains) and seeded, so two
//! runs on the same commit generate the same mail. Dates are relative to the
//! time of the run so `date:"last spring"` keeps meaning something.

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/newsletter.rs"]
mod newsletter;
#[path = "support/story.rs"]
mod story;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use penguin_core::ask::AskScope;
use penguin_core::{
    Account, InboxTab, Label, ListQuery, MailboxView, Message, SearchRequest, Store,
};
use penguin_render::{render_html, render_text, RenderOptions};
use serde_json::{json, Value};

/// Messages per store transaction, the same as sync's FETCH_CHUNK.
const INGEST_CHUNK: usize = 100;
const SEED: u64 = 42;

struct Args {
    n: usize,
    out: Option<PathBuf>,
    dir: PathBuf,
    keep: bool,
    reuse: bool,
    quick: bool,
}

fn parse_count(s: &str) -> usize {
    let s = s.trim().to_ascii_lowercase().replace(['_', ','], "");
    let (num, mul) = match s.strip_suffix('k') {
        Some(n) => (n.to_string(), 1_000),
        None => match s.strip_suffix('m') {
            Some(n) => (n.to_string(), 1_000_000),
            None => (s.clone(), 1),
        },
    };
    num.parse::<usize>()
        .map(|n| n * mul)
        .unwrap_or_else(|_| panic!("--corpus: can't read {s:?} (try 10k, 100k, 300k)"))
}

fn parse_args() -> Args {
    let mut a = Args {
        n: 100_000,
        out: None,
        dir: PathBuf::from("target/bench-corpus"),
        keep: false,
        reuse: false,
        quick: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--corpus" => a.n = parse_count(&it.next().expect("--corpus needs a size")),
            "--out" => a.out = Some(PathBuf::from(it.next().expect("--out needs a path"))),
            "--dir" => a.dir = PathBuf::from(it.next().expect("--dir needs a path")),
            "--keep" => a.keep = true,
            "--reuse" => a.reuse = true,
            "--quick" => a.quick = true,
            "-h" | "--help" => {
                eprintln!("usage: bench [--corpus 100k] [--out file.json] [--dir db-dir] [--keep] [--reuse] [--quick]");
                std::process::exit(0);
            }
            other => panic!("unknown argument {other:?} (see --help)"),
        }
    }
    a
}

// ----- statistics -----

fn stats(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return json!({ "n": 0 });
    }
    v.sort_by(f64::total_cmp);
    let pct = |p: f64| v[(((v.len() - 1) as f64) * p).round() as usize];
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    json!({
        "n": v.len(),
        "p50": round3(pct(0.50)),
        "p95": round3(pct(0.95)),
        "p99": round3(pct(0.99)),
        "max": round3(*v.last().unwrap()),
        "mean": round3(mean),
    })
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// CPU time of the calling thread, in seconds. Ingest runs on one thread
/// (SQLite doesn't start its own), so this is ingest's CPU cost, which a
/// busy shared machine distorts much less than wall time.
fn thread_cpu_secs() -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid out-pointer; the call only writes the timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

fn time_n<T>(runs: usize, mut f: impl FnMut() -> T) -> (Vec<f64>, T) {
    let mut times = Vec::with_capacity(runs);
    let mut last = None;
    for _ in 0..runs {
        let t = Instant::now();
        last = Some(std::hint::black_box(f()));
        times.push(ms(t));
    }
    (times, last.expect("runs > 0"))
}

// ----- machine & commit -----

fn cmd(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn machine_info() -> Value {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let mut cpu = None;
    let mut ram_bytes: Option<u64> = None;
    let mut os = None;
    let mut virtualized = None;
    if cfg!(target_os = "linux") {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        cpu = cpuinfo
            .lines()
            .find(|l| l.starts_with("model name"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string());
        virtualized = Some(
            cpuinfo
                .lines()
                .any(|l| l.starts_with("flags") && l.contains(" hypervisor")),
        );
        ram_bytes = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|m| {
                m.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|kb| kb.parse::<u64>().ok())
            })
            .map(|kb| kb * 1024);
        os = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|s| {
                s.lines().find(|l| l.starts_with("PRETTY_NAME=")).map(|l| {
                    l.trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string()
                })
            });
    } else if cfg!(target_os = "macos") {
        cpu = cmd("sysctl", &["-n", "machdep.cpu.brand_string"]);
        ram_bytes = cmd("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.parse().ok());
        os = cmd("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}"));
        virtualized = cmd("sysctl", &["-n", "kern.hv_vmm_present"]).map(|v| v == "1");
    }
    json!({
        "os": os.unwrap_or_else(|| std::env::consts::OS.to_string()),
        "kernel": cmd("uname", &["-r"]),
        "arch": std::env::consts::ARCH,
        "cpu": cpu,
        "logicalCores": cores,
        "ramGiB": ram_bytes.map(|b| (b as f64 / 1073741824.0 * 10.0).round() / 10.0),
        "virtualized": virtualized,
    })
}

/// How busy the machine is right now: 1-minute load average, and on Linux
/// the share of the last minute runnable tasks waited for a CPU (PSI) and
/// the memory available for the page cache. Recorded at the start and end
/// of a run, because a shared machine makes tails noisy.
fn contention() -> Value {
    let load: Option<f64> = if cfg!(target_os = "macos") {
        cmd("sysctl", &["-n", "vm.loadavg"]).and_then(|s| {
            s.trim_matches(|c| c == '{' || c == '}' || c == ' ')
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
        })
    } else {
        std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
    };
    let cpu_wait = std::fs::read_to_string("/proc/pressure/cpu")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("some"))
                .and_then(|l| {
                    l.split_whitespace()
                        .find_map(|kv| kv.strip_prefix("avg60="))
                })
                .and_then(|v| v.parse::<f64>().ok())
        });
    let mem_available = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| {
        m.lines()
            .find(|l| l.starts_with("MemAvailable:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<f64>().ok())
    });
    json!({
        "loadAvg1m": load,
        "cpuPressureSomeAvg60Pct": cpu_wait,
        "memAvailableGiB": mem_available.map(|kb| (kb / 1048576.0 * 10.0).round() / 10.0),
    })
}

fn git_info() -> Value {
    let commit = cmd("git", &["rev-parse", "--short=12", "HEAD"]);
    let dirty = cmd(
        "git",
        &[
            "status",
            "--porcelain",
            "--untracked-files=no",
            "--",
            "crates",
        ],
    )
    .map(|s| !s.is_empty());
    json!({ "commit": commit, "dirtyCrates": dirty })
}

// ----- corpus -----

fn setup_accounts(store: &Store) {
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
    const SYSTEM: [&str; 12] = [
        "INBOX",
        "SENT",
        "DRAFT",
        "TRASH",
        "SPAM",
        "STARRED",
        "IMPORTANT",
        "UNREAD",
        "CATEGORY_PROMOTIONS",
        "CATEGORY_UPDATES",
        "CATEGORY_SOCIAL",
        "CATEGORY_FORUMS",
    ];
    for a in corpus::ACCOUNTS {
        let mut labels: Vec<Label> = SYSTEM
            .iter()
            .map(|id| Label {
                account_id: a.into(),
                id: id.to_string(),
                name: id.to_string(),
                kind: "system".into(),
                color: None,
                unread_count: None,
                hidden: false,
            })
            .collect();
        labels.extend((1..=20).map(|i| Label {
            account_id: a.into(),
            id: format!("Label_{i}"),
            name: format!("Project {i}"),
            kind: "user".into(),
            color: None,
            unread_count: None,
            hidden: false,
        }));
        store.replace_labels(a, &labels).unwrap();
    }
}

#[derive(Default)]
struct CorpusStats {
    messages: usize,
    threads: usize,
    thread_len: [usize; 4], // 1, 2-3, 4-8, 9+
    html_messages: usize,
    newsletters: usize,
    text_bytes: usize,
    html_bytes: usize,
    max_html: usize,
    attachments: usize,
    with_attachments: usize,
    per_account: [usize; 3],
    inbox: usize,
    unread: usize,
    user_labelled: usize,
}

impl CorpusStats {
    fn add_thread(&mut self, msgs: &[Message]) {
        if msgs.is_empty() {
            return;
        }
        self.threads += 1;
        self.thread_len[match msgs.len() {
            1 => 0,
            2..=3 => 1,
            4..=8 => 2,
            _ => 3,
        }] += 1;
        for m in msgs {
            self.messages += 1;
            self.text_bytes += m.body_text.len() + m.subject.len();
            if let Some(h) = &m.body_html {
                self.html_messages += 1;
                self.html_bytes += h.len();
                self.max_html = self.max_html.max(h.len());
            }
            if m.list_unsubscribe.is_some() {
                self.newsletters += 1;
            }
            let files = m.attachments.iter().filter(|a| !a.inline).count();
            self.attachments += files;
            self.with_attachments += usize::from(files > 0);
            if let Some(i) = corpus::ACCOUNTS.iter().position(|a| *a == m.account_id) {
                self.per_account[i] += 1;
            }
            self.inbox += usize::from(m.label_ids.iter().any(|l| l == "INBOX"));
            self.unread += usize::from(m.label_ids.iter().any(|l| l == "UNREAD"));
            self.user_labelled += usize::from(m.label_ids.iter().any(|l| l.starts_with("Label_")));
        }
    }

    fn json(&self) -> Value {
        let n = self.messages.max(1) as f64;
        json!({
            "messages": self.messages,
            "threads": self.threads,
            "accounts": corpus::ACCOUNTS.len(),
            "messagesPerAccount": self.per_account,
            "threadLength": {
                "1": self.thread_len[0], "2-3": self.thread_len[1],
                "4-8": self.thread_len[2], "9+": self.thread_len[3],
            },
            "htmlShare": round3(self.html_messages as f64 / n),
            "newsletterShare": round3(self.newsletters as f64 / n),
            "avgTextBytes": (self.text_bytes as f64 / n).round(),
            "avgHtmlBytes": (self.html_bytes as f64 / self.html_messages.max(1) as f64).round(),
            "maxHtmlBytes": self.max_html,
            "plainTextMiB": round3(self.text_bytes as f64 / 1048576.0),
            "rawHtmlMiB": round3(self.html_bytes as f64 / 1048576.0),
            "attachments": self.attachments,
            "messagesWithAttachments": self.with_attachments,
            "inboxMessages": self.inbox,
            "unreadMessages": self.unread,
            "userLabelledMessages": self.user_labelled,
            "seed": SEED,
        })
    }
}

/// Generate `n` messages and write them in sync-sized transactions. Returns
/// (ingest results, corpus description).
fn build(path: &Path, n: usize, now: i64) -> (Value, Value) {
    let store = Store::open(path).unwrap();
    setup_accounts(&store);
    let mut corpus = corpus::Corpus::new(SEED, now);
    let mut cs = CorpusStats::default();

    // Known items (planted) and the Ask story go in first, like old mail.
    let mut pending: Vec<Message> = corpus.planted();
    pending.extend(story::story(now));
    let mut by_thread: HashMap<String, Vec<Message>> = HashMap::new();
    for m in &pending {
        by_thread
            .entry(m.thread_id.clone())
            .or_default()
            .push(m.clone());
    }
    for t in by_thread.values() {
        cs.add_thread(t);
    }

    // A conversation to star and unstar while the index is maintained.
    let probe = (pending[0].account_id.clone(), pending[0].thread_id.clone());
    let mut insert_secs = 0.0;
    let mut cpu_secs = 0.0;
    let mut deciles: Vec<(usize, f64)> = Vec::new(); // (messages, secs) per 10%
    let (mut dec_msgs, mut dec_secs) = (0usize, 0.0f64);
    let mut total = pending.len();
    let mut next_report = n / 10;
    let mut acct_cycle = 0usize;
    loop {
        while pending.len() >= INGEST_CHUNK || (total >= n && !pending.is_empty()) {
            let take = pending.len().min(INGEST_CHUNK);
            let chunk: Vec<Message> = pending.drain(..take).collect();
            let t = Instant::now();
            let cpu = thread_cpu_secs();
            store.upsert_messages(&chunk).unwrap();
            cpu_secs += thread_cpu_secs() - cpu;
            let s = t.elapsed().as_secs_f64();
            insert_secs += s;
            dec_secs += s;
            dec_msgs += chunk.len();
            if dec_msgs >= n / 10 {
                deciles.push((dec_msgs, dec_secs));
                dec_msgs = 0;
                dec_secs = 0.0;
            }
        }
        if total >= n {
            break;
        }
        // 60% work, 30% personal, 10% side account.
        let acct = [0, 0, 0, 0, 0, 0, 1, 1, 1, 2][acct_cycle % 10];
        acct_cycle += 1;
        let mut th = corpus.thread(acct);
        th.truncate(n - total);
        total += th.len();
        cs.add_thread(&th);
        pending.extend(th);
        if total >= next_report {
            eprint!("\r  generated + stored {total}/{n}");
            next_report += n / 10;
        }
    }
    if dec_msgs > 0 {
        deciles.push((dec_msgs, dec_secs));
    }
    eprintln!();
    let (optimize_ms, write_waits) = maintain_with_writes(&store, &probe);
    drop(store);
    // Reopen once so the WAL is checkpointed into the main file.
    drop(Store::open(path).unwrap());

    let rate = |m: usize, s: f64| (m as f64 / s).round();
    let ingest = json!({
        "chunkSize": INGEST_CHUNK,
        "messages": cs.messages,
        "seconds": round3(insert_secs),
        "messagesPerSec": rate(cs.messages, insert_secs),
        "plainTextMiBPerSec": round3(cs.text_bytes as f64 / 1048576.0 / insert_secs),
        "firstTenthMessagesPerSec": deciles.first().map(|&(m, s)| rate(m, s)),
        "lastTenthMessagesPerSec": deciles.last().map(|&(m, s)| rate(m, s)),
        "messagesPerSecByTenth": deciles.iter().map(|&(m, s)| rate(m, s)).collect::<Vec<_>>(),
        "cpuMsPerMessage": round3(cpu_secs * 1000.0 / cs.messages as f64),
        "ftsOptimizeMs": round3(optimize_ms),
        "writesDuringOptimize": write_waits.len(),
        "writeWaitDuringOptimizeMs": stats(write_waits),
    });
    (ingest, cs.json())
}

/// Store::optimize (after a backfill, sync merges the full-text index)
/// while another thread stars and unstars a conversation every 20 ms, as
/// user actions and other accounts' sync do. Returns the optimize time and
/// how long each of those writes took.
fn maintain_with_writes(store: &Store, probe: &(String, String)) -> (f64, Vec<f64>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let done = AtomicBool::new(false);
    let star = ["STARRED".to_string()];
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let mut waits = Vec::new();
            let mut starred = false;
            while !done.load(Ordering::Acquire) {
                let (add, remove): (&[String], &[String]) =
                    if starred { (&[], &star) } else { (&star, &[]) };
                let t = Instant::now();
                store
                    .modify_thread_labels(&probe.0, &probe.1, add, remove)
                    .unwrap();
                waits.push(ms(t));
                starred = !starred;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if starred {
                store
                    .modify_thread_labels(&probe.0, &probe.1, &[], &star)
                    .unwrap();
            }
            waits
        });
        let t = Instant::now();
        store.optimize().unwrap();
        let optimize_ms = ms(t);
        done.store(true, Ordering::Release);
        (optimize_ms, writer.join().unwrap())
    })
}

/// Arrival order, measured on its own: the same `n` generated messages
/// ingested into fresh databases in the generator's order (random dates,
/// as the main run) and newest first (a Gmail backfill's order).
fn order_side_test(dir: &Path, n: usize, now: i64) -> Value {
    let mut msgs: Vec<Message> = Vec::with_capacity(n + 64);
    let mut corpus = corpus::Corpus::new(SEED + 1, now);
    let mut i = 0usize;
    while msgs.len() < n {
        let acct = [0, 0, 0, 0, 0, 0, 1, 1, 1, 2][i % 10];
        i += 1;
        msgs.extend(corpus.thread(acct));
    }
    msgs.truncate(n);
    let run = |msgs: &[Message], name: &str| {
        let path = dir.join(format!("order-{name}.db"));
        let store = Store::open(&path).unwrap();
        setup_accounts(&store);
        let (mut wall, mut cpu) = (0.0, 0.0);
        for chunk in msgs.chunks(INGEST_CHUNK) {
            let t = Instant::now();
            let c = thread_cpu_secs();
            store.upsert_messages(chunk).unwrap();
            cpu += thread_cpu_secs() - c;
            wall += t.elapsed().as_secs_f64();
        }
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        json!({
            "messagesPerSec": (msgs.len() as f64 / wall).round(),
            "cpuMsPerMessage": round3(cpu * 1000.0 / msgs.len() as f64),
        })
    };
    let generator = run(&msgs, "generator");
    msgs.sort_by_key(|m| std::cmp::Reverse(m.date));
    let newest_first = run(&msgs, "newest-first");
    json!({ "messages": n, "generatorOrder": generator, "newestFirst": newest_first })
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

fn disk(path: &Path, messages: usize) -> Value {
    let dir = path.parent().unwrap();
    let total = dir_size(dir);
    let mib = |b: f64| round3(b / 1048576.0);
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    // dbstat is compiled into rusqlite's bundled SQLite; group its pages.
    let mut groups: HashMap<&'static str, i64> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT name, sum(pgsize) FROM dbstat GROUP BY name") {
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap();
        for r in rows.flatten() {
            let (name, size) = r;
            let g = if name.starts_with("messages_fts") || name.starts_with("attachments_fts") {
                "fullTextIndex"
            } else if name.starts_with("events") || name.starts_with("event_") {
                "calendar"
            } else if name == "message_bodies" {
                "bodies (zstd)"
            } else if name.starts_with("messages") || name == "message_labels" {
                "messageRows"
            } else if name.starts_with("thread") {
                "threadsAndViews"
            } else if name.starts_with("attachments") {
                "attachmentRows"
            } else {
                "other"
            };
            *groups.entry(g).or_default() += size;
        }
    }
    let per_100k = |b: f64| mib(b * 100_000.0 / messages as f64);
    json!({
        "totalMiB": mib(total as f64),
        "mibPer100k": per_100k(total as f64),
        "bytesPerMessage": (total as f64 / messages as f64).round(),
        "breakdownMiB": groups.iter().map(|(k, v)| (k.to_string(), json!(mib(*v as f64)))).collect::<serde_json::Map<_, _>>(),
    })
}

// ----- cold open -----

/// Drop the database files from the OS page cache so the next open reads
/// from disk. Linux only (posix_fadvise); elsewhere returns false.
fn evict_page_cache(dir: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let mut ok = true;
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let Ok(f) = std::fs::File::open(e.path()) else {
                continue;
            };
            // Dirty pages can't be dropped: flush them first.
            let _ = f.sync_all();
            // SAFETY: a valid open fd; the call only advises the kernel.
            let rc = unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
            ok &= rc == 0;
        }
        ok
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = dir;
        false
    }
}

fn inbox_page() -> ListQuery {
    ListQuery {
        view: MailboxView::Inbox,
        tab: None,
        account_id: None,
        account_ids: None,
        limit: 50,
        before: None,
        unread_only: false,
        split: None,
    }
}

fn open_to_first_list(path: &Path, cold_runs: usize, warm_runs: usize) -> Value {
    let dir = path.parent().unwrap();
    let run = |evict: bool| {
        let evicted = evict && evict_page_cache(dir);
        let t = Instant::now();
        let store = Store::open(path).unwrap();
        let open = ms(t);
        let rows = store.list_threads(&inbox_page()).unwrap().len();
        let first_list = ms(t);
        store.list_labels(None).unwrap();
        let with_labels = ms(t);
        let t2 = Instant::now();
        store
            .search(&SearchRequest {
                query: "contract renewal".into(),
                account_id: None,
                account_ids: None,
                limit: 50,
            })
            .unwrap();
        let first_search = ms(t2);
        drop(store);
        assert!(rows > 0, "the Inbox is empty");
        (evicted, open, first_list, with_labels, first_search)
    };
    let side = |evict: bool, runs: usize| {
        let (mut o, mut l, mut lb, mut s) = (vec![], vec![], vec![], vec![]);
        let mut evicted = evict;
        for _ in 0..runs {
            let r = run(evict);
            evicted &= r.0;
            o.push(r.1);
            l.push(r.2);
            lb.push(r.3);
            s.push(r.4);
        }
        json!({
            "pageCacheEvicted": evicted,
            "openMs": stats(o),
            "timeToFirstListMs": stats(l),
            "plusLabelsMs": stats(lb),
            "firstSearchMs": stats(s),
        })
    };
    let cold = side(true, cold_runs);
    let warm = side(false, warm_runs);
    json!({ "cold": cold, "warm": warm, "diskRandomRead4kMs": disk_random_reads(path) })
}

/// Latency of 4 KiB reads at random offsets of the database file after the
/// page cache is evicted: what each page SQLite touches costs on a cold
/// start. Null where the cache can't be evicted (not Linux).
fn disk_random_reads(path: &Path) -> Value {
    use std::os::unix::fs::FileExt;
    if !evict_page_cache(path.parent().unwrap()) {
        return Value::Null;
    }
    let f = std::fs::File::open(path).unwrap();
    let pages = f.metadata().unwrap().len() / 4096;
    let mut rng = corpus::Rng::new(11);
    let mut buf = [0u8; 4096];
    let mut times = Vec::new();
    for _ in 0..300 {
        let off = (rng.next() % pages) * 4096;
        let t = Instant::now();
        f.read_at(&mut buf, off).unwrap();
        times.push(ms(t));
    }
    stats(times)
}

// ----- list -----

fn list_benches(store: &Store, runs: usize) -> Value {
    type Case = (
        &'static str,
        MailboxView,
        Option<InboxTab>,
        Option<&'static str>,
        bool,
    );
    let cases: Vec<Case> = vec![
        (
            "Inbox (all accounts)",
            MailboxView::Inbox,
            None,
            None,
            false,
        ),
        ("Inbox, unread only", MailboxView::Inbox, None, None, true),
        (
            "Inbox, one account",
            MailboxView::Inbox,
            None,
            Some(corpus::ACCOUNTS[1]),
            false,
        ),
        (
            "Inbox / Newsletters tab",
            MailboxView::Inbox,
            Some(InboxTab::Newsletters),
            None,
            false,
        ),
        ("All mail", MailboxView::All, None, None, false),
        ("All mail, unread only", MailboxView::All, None, None, true),
        ("Sent", MailboxView::Sent, None, None, false),
        ("Done (archived)", MailboxView::Done, None, None, false),
        (
            "Label (user label)",
            MailboxView::Label("Label_3".into()),
            None,
            None,
            false,
        ),
        ("Starred", MailboxView::Starred, None, None, false),
    ];
    let mut out = Vec::new();
    for (name, view, tab, acct, unread_only) in cases {
        let q = ListQuery {
            view,
            tab,
            account_id: acct.map(Into::into),
            account_ids: None,
            limit: 50,
            before: None,
            unread_only,
            split: None,
        };
        let (times, rows) = time_n(runs, || store.list_threads(&q).unwrap().len());
        out.push(json!({ "name": name, "rows": rows, "ms": stats(times) }));
    }

    // Split Inbox (store_split.rs): the default splits plus a people split,
    // each tab's first page, and the tab counts.
    let splits = [
        "from:@acme.example OR from:@globex.example",
        "is:important -is:newsletter -has:invite",
        "has:invite OR from:calendar-notification@google.com OR from:@calendly.com",
        "is:newsletter",
    ];
    let split_cases: [(&str, Option<usize>); 5] = [
        ("Split: VIP (people, first tab)", Some(0)),
        ("Split: Important", Some(1)),
        ("Split: Calendar (sparse)", Some(2)),
        ("Split: News", Some(3)),
        ("Split: Other (the rest)", None),
    ];
    for (name, i) in split_cases {
        let split = match i {
            Some(i) => penguin_core::SplitFilter {
                include: Some(splits[i].into()),
                exclude: splits[..i].iter().map(|q| q.to_string()).collect(),
            },
            None => penguin_core::SplitFilter {
                include: None,
                exclude: splits.iter().map(|q| q.to_string()).collect(),
            },
        };
        let q = ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 100,
            before: None,
            unread_only: false,
            split: Some(split),
        };
        let (times, rows) = time_n(runs, || store.list_threads(&q).unwrap().len());
        out.push(json!({ "name": name, "rows": rows, "ms": stats(times) }));
    }
    let queries: Vec<String> = splits.iter().map(|q| q.to_string()).collect();
    let (count_times, counted) = time_n((runs / 4).max(3), || {
        let c = store.split_counts(&queries, None).unwrap();
        c.splits.iter().map(|x| x.total).sum::<u32>()
    });
    out.push(json!({ "name": "Split counts (4 splits + Other)", "rows": counted, "ms": stats(count_times) }));

    // Infinite scroll: 40 consecutive pages of All mail (2,000 threads),
    // each page keyed by the previous page's last date.
    let mut pages = Vec::new();
    let mut whole = Vec::new();
    let reps = (runs / 20).max(3);
    let mut threads = 0;
    for _ in 0..reps {
        let mut before = None;
        let t_all = Instant::now();
        threads = 0;
        for _ in 0..40 {
            let t = Instant::now();
            let page = store
                .list_threads(&ListQuery {
                    view: MailboxView::All,
                    tab: None,
                    account_id: None,
                    account_ids: None,
                    limit: 50,
                    before,
                    unread_only: false,
                    split: None,
                })
                .unwrap();
            pages.push(ms(t));
            threads += page.len();
            before = page.last().map(|t| t.last_date);
        }
        whole.push(ms(t_all));
    }
    let (label_times, n_labels) = time_n(runs, || store.list_labels(None).unwrap().len());
    json!({
        "views": out,
        "scroll": { "pages": 40, "threads": threads, "pageMs": stats(pages), "all40PagesMs": stats(whole) },
        "labelsWithUnreadCounts": { "labels": n_labels, "ms": stats(label_times) },
    })
}

// ----- open thread -----

/// What get_thread does after the store read (apps/desktop/src-tauri/src/views.rs
/// message_view): sanitize/render the body, detect a one-time code, plan the
/// unsubscribe offer. Remote images blocked (the default).
fn render_message(m: &Message) -> usize {
    let rendered = match m.body_html.as_deref() {
        Some(html) if !html.trim().is_empty() => render_html(html, &RenderOptions::default()),
        _ => render_text(&m.body_text),
    };
    std::hint::black_box(penguin_core::otp::detect_message(m));
    std::hint::black_box(penguin_core::unsubscribe::plan(m, || {
        penguin_render::unsubscribe::find_unsubscribe_link(&rendered.html)
    }));
    rendered.html.len()
}

fn open_thread_benches(store: &Store, path: &Path, samples: usize) -> Value {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let all: Vec<(String, String, i64)> = conn
        .prepare("SELECT account_id, thread_id, message_count FROM threads")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    drop(conn);
    let mut rng = corpus::Rng::new(7);
    let mut picked: Vec<&(String, String, i64)> = (0..samples.min(all.len()))
        .map(|_| &all[rng.below(all.len())])
        .collect();
    // Always include the longest thread.
    if let Some(longest) = all.iter().max_by_key(|t| t.2) {
        picked.push(longest);
    }

    let (mut store_ms, mut render_ms, mut total_ms) = (vec![], vec![], vec![]);
    let (mut conv_total, mut news_total) = (vec![], vec![]);
    let mut html_out = 0usize;
    let mut longest = json!(null);
    // One pass, no warm-up: each open is that thread's first in this process
    // (OS page cache warm, SQLite's own cache not primed for it).
    for (acct, tid, count) in &picked {
        let t = Instant::now();
        let detail = store.get_thread(acct, tid).unwrap().expect("thread exists");
        let s = ms(t);
        let t2 = Instant::now();
        let mut bytes = 0;
        for m in &detail.messages {
            bytes += render_message(m);
        }
        let r = ms(t2);
        let total = s + r;
        html_out += bytes;
        store_ms.push(s);
        render_ms.push(r);
        total_ms.push(total);
        if detail.messages.iter().any(|m| m.list_unsubscribe.is_some()) {
            news_total.push(total);
        } else {
            conv_total.push(total);
        }
        if Some(*count) == all.iter().map(|t| t.2).max() && longest.is_null() {
            longest = json!({ "messages": count, "totalMs": round3(total) });
        }
    }
    json!({
        "threads": picked.len(),
        "storeReadMs": stats(store_ms),
        "renderMs": stats(render_ms),
        "totalMs": stats(total_ms),
        "conversationTotalMs": stats(conv_total),
        "newsletterTotalMs": stats(news_total),
        "avgRenderedKiB": round3(html_out as f64 / picked.len().max(1) as f64 / 1024.0),
        "longestThread": longest,
    })
}

fn render_benches(runs: usize) -> Value {
    let mut out = Vec::new();
    for (name, bytes) in [
        ("50 KB newsletter", 50 * 1024),
        ("200 KB newsletter", 200 * 1024),
        ("2 MB newsletter", 2 * 1024 * 1024),
    ] {
        let html = newsletter::newsletter(bytes);
        render_html(&html, &RenderOptions::default()); // warm-up
        let (times, r) = time_n(runs, || render_html(&html, &RenderOptions::default()));
        out.push(json!({
            "name": name,
            "inputBytes": html.len(),
            "outputBytes": r.html.len(),
            "trackersRemoved": r.trackers_removed,
            "blockedRemoteImages": r.blocked_remote_images,
            "ms": stats(times),
        }));
    }
    let line = "> quoted line with a link https://acme.example/x and mail ada@lovelace.example\n";
    let text = format!(
        "Hello\n\nOn Mon, Ada wrote:\n{}",
        line.repeat(2 * 1024 * 1024 / line.len())
    );
    let (times, _) = time_n(runs, || render_text(&text));
    out.push(
        json!({ "name": "2 MB plain text, quoted", "inputBytes": text.len(), "ms": stats(times) }),
    );
    json!(out)
}

// ----- search -----

fn search_once(store: &Store, q: &str) -> penguin_core::SearchResponse {
    store
        .search(&SearchRequest {
            query: q.to_string(),
            account_id: None,
            account_ids: None,
            limit: 50,
        })
        .unwrap()
}

fn ymd(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap()
        .format("%Y/%m/%d")
        .to_string()
}

fn search_benches(store: &Store, now: i64, runs: usize) -> Value {
    let after = ymd(now - 365 * corpus::DAY);
    let before = ymd(now - 180 * corpus::DAY);
    let queries: Vec<(&str, String)> = vec![
        ("words", "meeting".into()),
        ("words", "invoice".into()),
        ("words", "the".into()),
        ("words", "lease renewal".into()),
        ("words", "quarterly report deadline".into()),
        ("phrase", "\"early termination clause\"".into()),
        ("phrase", "\"to the\"".into()),
        ("operator", "from:mike".into()),
        ("operator", "from:mike.delgado@realty.example".into()),
        ("operator", "from:mike lease".into()),
        ("operator", "has:attachment".into()),
        ("operator", "has:pdf".into()),
        ("operator", "has:pdf date:\"last spring\"".into()),
        ("operator", format!("invoice after:{after} before:{before}")),
        ("operator", "label:Label_3".into()),
        ("operator", "label:Label_3 meeting".into()),
        ("operator", "is:unread".into()),
        ("operator", "is:unread invoice".into()),
        ("operator", "in:sent budget".into()),
        ("operator", "older_than:2y tax".into()),
        ("operator", "invoice -stripe".into()),
        ("operator", "lease OR mortgage".into()),
        ("operator", "filename:20417".into()),
        ("operator", "account:personal invoice".into()),
        ("identifier", "INV-20417".into()),
        (
            "natural",
            "the pdf mike sent about the lease date:\"last spring\"".into(),
        ),
        ("no match", "zzqxjv".into()),
    ];
    for q in ["warmup", "the", "invoice"] {
        search_once(store, q);
    }
    let mut rows = Vec::new();
    let mut by_kind: HashMap<&str, Vec<f64>> = HashMap::new();
    let mut pooled = Vec::new();
    for (kind, q) in &queries {
        let (times, r) = time_n(runs, || search_once(store, q));
        let top = r
            .hits
            .first()
            .map(|h| h.message_id.clone())
            .unwrap_or_default();
        by_kind.entry(kind).or_default().extend(&times);
        pooled.extend(&times);
        rows.push(json!({
            "kind": kind,
            "query": q,
            "threads": r.hits.len(),
            "topHitIsPlanted": top.starts_with("planted"),
            "ms": stats(times),
        }));
    }
    json!({
        "limit": 50,
        "queries": rows,
        "byKind": by_kind.into_iter().map(|(k, v)| (k.to_string(), stats(v))).collect::<serde_json::Map<_, _>>(),
        "pooledMs": stats(pooled),
    })
}

/// Search-as-you-type: every prefix is its own search, as each keystroke is.
fn as_you_type(store: &Store, reps: usize) -> Value {
    let typed = [
        "invoice from:mike",
        "quarterly report",
        "\"early termination",
        "lease has:pdf",
        "priya budget",
    ];
    let mut pooled = Vec::new();
    let mut per = Vec::new();
    for s in typed {
        let prefixes: Vec<&str> = s
            .char_indices()
            .map(|(i, c)| &s[..i + c.len_utf8()])
            .collect();
        let mut times = Vec::new();
        for _ in 0..reps {
            for p in &prefixes {
                let t = Instant::now();
                search_once(store, p);
                times.push(ms(t));
            }
        }
        pooled.extend(&times);
        per.push(json!({ "typed": s, "keystrokes": prefixes.len(), "ms": stats(times) }));
    }
    json!({ "strings": per, "pooledMs": stats(pooled) })
}

fn ask_benches(store: &Store, now: i64, runs: usize) -> Value {
    let p = corpus::Corpus::new(SEED, now).people;
    // (template for the docs, the question asked). People come from the
    // generated corpus, so their names are only known at run time.
    let questions: Vec<(String, String)> = [
        (
            "when did I last speak to Mike from Kettle on the Knoll / Fernwood?",
            "",
        ),
        ("when did I last email <name>", p[3].name.as_str()),
        ("first email from <address>", p[11].email.as_str()),
        ("how long have I known <address>", p[7].email.as_str()),
        ("who is <name>", p[20].name.as_str()),
        ("how many emails from <name> this year", p[5].name.as_str()),
        ("how much did I spend on Uber this year", ""),
        ("latest pdf from <name>", p[2].name.as_str()),
        ("latest invoice", ""),
        ("what am I waiting on", ""),
        ("what do I owe replies to", ""),
        ("who emails me the most", ""),
        ("who emailed me about the budget", ""),
        ("when is the dinner", ""),
    ]
    .into_iter()
    .map(|(t, who)| {
        let q = t.replace("<name>", who).replace("<address>", who);
        (t.to_string(), q)
    })
    .collect();
    let scope = AskScope::default();
    let mut rows = Vec::new();
    let mut pooled = Vec::new();
    for (template, q) in &questions {
        let (times, a) = time_n(runs, || store.ask(q, &scope, now, 0).unwrap());
        pooled.extend(&times);
        rows.push(json!({
            "template": template,
            "question": q,
            "answered": a.confidence != penguin_core::ask::AskConfidence::None,
            "ms": stats(times),
        }));
    }
    json!({ "questions": rows, "pooledMs": stats(pooled) })
}

fn main() {
    let args = parse_args();
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; numbers are meaningless. Use --release.");
    }
    let n = args.n;
    let r = |full: usize| if args.quick { (full / 5).max(3) } else { full };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let path = args.dir.join(format!("corpus-{n}.db"));
    let started = Instant::now();

    let mut out = serde_json::Map::new();
    out.insert("schema".into(), json!(1));
    out.insert("kind".into(), json!("backend"));
    out.insert("date".into(), json!(chrono::Utc::now().to_rfc3339()));
    out.insert("machine".into(), machine_info());
    let contention_at_start = contention();
    out.insert("git".into(), git_info());
    out.insert(
        "build".into(),
        json!({ "profile": if cfg!(debug_assertions) { "debug" } else { "release" }, "quick": args.quick }),
    );

    let messages;
    if args.reuse && path.exists() {
        eprintln!("reusing {}", path.display());
        let s = Store::open(&path).unwrap();
        messages = s.count_messages(None).unwrap() as usize;
    } else {
        let _ = std::fs::remove_dir_all(&args.dir);
        std::fs::create_dir_all(&args.dir).unwrap();
        eprintln!("building a {n}-message corpus in {}", args.dir.display());
        let (mut ingest, corpus_json) = build(&path, n, now);
        messages = corpus_json["messages"].as_u64().unwrap() as usize;
        eprintln!("arrival order side test…");
        ingest["orderSideTest"] = order_side_test(&args.dir, r(20_000), now);
        out.insert("ingest".into(), ingest);
        out.insert("corpus".into(), corpus_json);
    }
    out.insert("disk".into(), disk(&path, messages));
    eprintln!("cold open…");
    out.insert("open".into(), open_to_first_list(&path, r(8), r(30)));

    let store = Store::open(&path).unwrap();
    eprintln!("list…");
    out.insert("list".into(), list_benches(&store, r(200)));
    eprintln!("open thread…");
    out.insert("thread".into(), open_thread_benches(&store, &path, r(500)));
    eprintln!("render…");
    out.insert("render".into(), render_benches(r(20)));
    eprintln!("search…");
    out.insert("search".into(), search_benches(&store, now, r(30)));
    out.insert("asYouType".into(), as_you_type(&store, r(10)));
    eprintln!("ask…");
    out.insert("ask".into(), ask_benches(&store, now, r(20)));
    drop(store);
    out.insert(
        "contention".into(),
        json!({ "start": contention_at_start, "end": contention() }),
    );
    out.insert(
        "wallSeconds".into(),
        json!(round3(started.elapsed().as_secs_f64())),
    );

    let out_path = args
        .out
        .unwrap_or_else(|| PathBuf::from(format!("target/bench/backend-{n}.json")));
    if let Some(d) = out_path.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    let v = Value::Object(out);
    std::fs::write(&out_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    summary(&v);
    eprintln!("wrote {}", out_path.display());
    if !args.keep {
        std::fs::remove_dir_all(&args.dir).unwrap();
    }
}

/// A few lines for the terminal; the full table is docs/PERFORMANCE.md.
fn summary(v: &Value) {
    let p = |x: &Value| format!("p50 {} / p95 {} ms", x["p50"], x["p95"]);
    println!("messages: {}", v["corpus"]["messages"]);
    println!(
        "ingest: {} msg/s, {} ms CPU per message; newest first (20k): {} msg/s",
        v["ingest"]["messagesPerSec"],
        v["ingest"]["cpuMsPerMessage"],
        v["ingest"]["orderSideTest"]["newestFirst"]["messagesPerSec"]
    );
    println!(
        "optimize: {} ms; a write meanwhile waited {}",
        v["ingest"]["ftsOptimizeMs"],
        p(&v["ingest"]["writeWaitDuringOptimizeMs"])
    );
    println!("disk: {} MiB per 100k messages", v["disk"]["mibPer100k"]);
    println!(
        "cold open → first list: {}",
        p(&v["open"]["cold"]["timeToFirstListMs"])
    );
    println!("list Inbox: {}", p(&v["list"]["views"][0]["ms"]));
    println!("open thread: {}", p(&v["thread"]["totalMs"]));
    println!("search (all queries): {}", p(&v["search"]["pooledMs"]));
    println!("as-you-type keystroke: {}", p(&v["asYouType"]["pooledMs"]));
    println!("ask: {}", p(&v["ask"]["pooledMs"]));
}
