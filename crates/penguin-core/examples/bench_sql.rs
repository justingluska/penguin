//! Every query path the UI waits on, statement by statement, over a corpus
//! `examples/bench.rs` built (`--keep`). For each workload: p50 / p95 of the
//! whole call, then (with `--features sql-profile`) the statements that cost
//! the most, whether SQLite scanned a table, sorted or built an automatic
//! index for them, and their `EXPLAIN QUERY PLAN`.
//!
//!   cargo run --release -p penguin-core --example bench -- --corpus 300k --keep
//!   cargo run --release -p penguin-core --features sql-profile --example bench_sql -- \
//!       --db target/bench-corpus/corpus-300000.db [--runs 30] [--out file.json] [--only list,search] [--read-only]
//!
//! Without the feature only the call timings are printed (no tracing
//! overhead), which is what before/after tables should quote.
//!
//! Workloads: `list` (every mailbox view, both triage views, every smart
//! view, a saved search, 40-page scroll), `counts` (sidebar labels with
//! unread counts, triage and smart-view badges, message counts), `thread`
//! (open 300 random conversations), `search` (bench.rs's query set and
//! every keystroke of five typed strings), `ask` (bench.rs's questions plus
//! the fact questions), `write` (archive/unarchive and read/unread a
//! conversation, the actions' write path), `ingest` / `ingest_fresh` (sync's
//! write path into the corpus and into a new database), `sizes` (bytes per
//! table and index). The first run over a corpus also
//! extracts facts (the app's background scanner) and reports its speed.

#[path = "support/corpus.rs"]
mod corpus;

use std::path::PathBuf;
use std::time::Instant;

use penguin_core::ask::AskScope;
use penguin_core::{
    InboxTab, ListQuery, MailboxView, SearchRequest, Store, SMART_FILE_KINDS, SMART_VIEWS,
};
use serde_json::{json, Value};

struct Args {
    db: PathBuf,
    runs: usize,
    out: Option<PathBuf>,
    only: Option<Vec<String>>,
    plans: usize,
    read_only: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        db: PathBuf::from("target/bench-corpus/corpus-300000.db"),
        runs: 30,
        out: None,
        only: None,
        plans: 12,
        read_only: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => a.db = it.next().expect("--db path").into(),
            "--runs" => a.runs = it.next().expect("--runs n").parse().expect("--runs n"),
            "--out" => a.out = Some(it.next().expect("--out path").into()),
            "--only" => {
                a.only = Some(
                    it.next()
                        .expect("--only list,search")
                        .split(',')
                        .map(String::from)
                        .collect(),
                )
            }
            "--read-only" => a.read_only = true,
            "--plans" => a.plans = it.next().expect("--plans n").parse().expect("--plans n"),
            other => panic!("unknown argument {other:?}"),
        }
    }
    a
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        return 0.0;
    }
    v[(((v.len() - 1) as f64) * p).round() as usize]
}

fn stats(mut v: Vec<f64>) -> Value {
    let p50 = pct(&mut v, 0.5);
    let p95 = pct(&mut v, 0.95);
    json!({ "n": v.len(), "p50": (p50 * 1000.0).round() / 1000.0, "p95": (p95 * 1000.0).round() / 1000.0 })
}

fn time_n<T>(runs: usize, mut f: impl FnMut() -> T) -> (Vec<f64>, T) {
    let mut times = Vec::with_capacity(runs);
    let mut last = None;
    for _ in 0..runs {
        let t = Instant::now();
        last = Some(std::hint::black_box(f()));
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    (times, last.expect("runs > 0"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// One timed case of a workload.
struct Cases {
    rows: Vec<(String, Vec<f64>, usize)>,
}

impl Cases {
    fn new() -> Cases {
        Cases { rows: Vec::new() }
    }
    fn run<T>(
        &mut self,
        name: &str,
        runs: usize,
        size: impl Fn(&T) -> usize,
        f: impl FnMut() -> T,
    ) {
        let (times, last) = time_n(runs, f);
        self.rows.push((name.to_string(), times, size(&last)));
    }
    fn print(&self, title: &str) -> Value {
        println!("\n## {title}\n\n| case | p50 ms | p95 ms | rows |\n|---|---:|---:|---:|");
        let mut out = Vec::new();
        let mut pooled = Vec::new();
        for (name, times, rows) in &self.rows {
            let mut t = times.clone();
            let p50 = pct(&mut t, 0.5);
            let p95 = pct(&mut t, 0.95);
            println!("| {name} | {p50:.3} | {p95:.3} | {rows} |");
            pooled.extend(times);
            out.push(json!({ "case": name, "rows": rows, "ms": stats(times.clone()) }));
        }
        let all = stats(pooled);
        println!("| **all** | {} | {} | |", all["p50"], all["p95"]);
        json!({ "cases": out, "pooled": all })
    }
}

fn list_query(view: MailboxView) -> ListQuery {
    ListQuery {
        view,
        tab: None,
        account_id: None,
        account_ids: None,
        limit: 50,
        before: None,
        unread_only: false,
        split: None,
    }
}

fn list_workload(store: &Store, runs: usize) -> Value {
    let mut c = Cases::new();
    let a1 = corpus::ACCOUNTS[1].to_string();
    let profile: Vec<String> = corpus::ACCOUNTS[..2]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut views: Vec<(String, ListQuery)> = vec![
        ("Inbox".into(), list_query(MailboxView::Inbox)),
        (
            "Inbox unread".into(),
            ListQuery {
                unread_only: true,
                ..list_query(MailboxView::Inbox)
            },
        ),
        (
            "Inbox one account".into(),
            ListQuery {
                account_id: Some(a1.clone()),
                ..list_query(MailboxView::Inbox)
            },
        ),
        (
            "Inbox profile (2 accounts)".into(),
            ListQuery {
                account_ids: Some(profile.clone()),
                ..list_query(MailboxView::Inbox)
            },
        ),
        (
            "Inbox Important tab".into(),
            ListQuery {
                tab: Some(InboxTab::Important),
                ..list_query(MailboxView::Inbox)
            },
        ),
        (
            "Inbox Newsletters tab".into(),
            ListQuery {
                tab: Some(InboxTab::Newsletters),
                ..list_query(MailboxView::Inbox)
            },
        ),
        ("All mail".into(), list_query(MailboxView::All)),
        ("Sent".into(), list_query(MailboxView::Sent)),
        ("Done".into(), list_query(MailboxView::Done)),
        ("Starred".into(), list_query(MailboxView::Starred)),
        ("Drafts".into(), list_query(MailboxView::Drafts)),
        ("Trash".into(), list_query(MailboxView::Trash)),
        (
            "Label".into(),
            list_query(MailboxView::Label("Label_3".into())),
        ),
        ("Snoozed".into(), list_query(MailboxView::Snoozed)),
        ("Reply Later".into(), list_query(MailboxView::ReplyLater)),
        ("Follow up".into(), list_query(MailboxView::FollowUp)),
        (
            "Follow up unread".into(),
            ListQuery {
                unread_only: true,
                ..list_query(MailboxView::FollowUp)
            },
        ),
        (
            "Saved search".into(),
            list_query(MailboxView::Query("from:mike has:pdf".into())),
        ),
    ];
    for v in SMART_VIEWS {
        views.push((
            format!("Smart: {v}"),
            list_query(MailboxView::Smart(v.to_string())),
        ));
    }
    for k in SMART_FILE_KINDS {
        views.push((
            format!("Smart: files:{k}"),
            list_query(MailboxView::Smart(format!("files:{k}"))),
        ));
    }
    views.push((
        "Smart: people (one account)".into(),
        ListQuery {
            account_id: Some(a1),
            ..list_query(MailboxView::Smart("people".into()))
        },
    ));
    for (name, q) in &views {
        c.run(
            name,
            runs,
            |v: &Vec<_>| v.len(),
            || store.list_threads(q).unwrap(),
        );
    }
    // Scrolling All mail and the Inbox: 40 pages each, keyed by the last date.
    for view in [MailboxView::All, MailboxView::Inbox] {
        let name = format!("scroll 40 pages: {view:?}");
        c.run(
            &name,
            (runs / 5).max(3),
            |n: &usize| *n,
            || {
                let mut before = None;
                let mut n = 0;
                for _ in 0..40 {
                    let page = store
                        .list_threads(&ListQuery {
                            before,
                            ..list_query(view.clone())
                        })
                        .unwrap();
                    n += page.len();
                    before = page.last().map(|t| t.last_date);
                }
                n
            },
        );
    }
    c.print("list_threads")
}

fn counts_workload(store: &Store, runs: usize) -> Value {
    let mut c = Cases::new();
    let now = now_ms();
    let views: Vec<String> = SMART_VIEWS.iter().map(|s| s.to_string()).collect();
    c.run(
        "list_labels (sidebar, unread counts)",
        runs,
        |v: &Vec<_>| v.len(),
        || store.list_labels(None).unwrap(),
    );
    c.run(
        "triage_counts",
        runs,
        |v: &Vec<_>| v.len(),
        || store.triage_counts(None, 3, now).unwrap(),
    );
    c.run(
        "smart_counts (all views)",
        runs,
        |v: &Vec<_>| v.len(),
        || store.smart_counts(&views, None, now, 0).unwrap(),
    );
    for v in ["receipts", "bills", "packages", "travel", "subscriptions"] {
        c.run(
            &format!("smart_view_info {v}"),
            runs,
            |_: &_| 1,
            || store.smart_view_info(v, None, now, 0).unwrap(),
        );
    }
    c.run(
        "count_messages (all)",
        runs,
        |n: &u64| *n as usize,
        || store.count_messages(None).unwrap(),
    );
    c.run(
        "count_messages (each account)",
        runs,
        |n: &u64| *n as usize,
        || {
            corpus::ACCOUNTS
                .iter()
                .map(|a| store.count_messages(Some(a)).unwrap())
                .sum::<u64>()
        },
    );
    c.print("counts and badges")
}

fn thread_ids(store: &Store, n: usize) -> Vec<(String, String)> {
    // Every 7th conversation of a 200-row page of All mail, then 20 days
    // further back, so the sample spans the mailbox.
    let mut out = Vec::new();
    let mut before = None;
    let mut i = 0usize;
    while out.len() < n {
        let page = store
            .list_threads(&ListQuery {
                before,
                limit: 200,
                ..list_query(MailboxView::All)
            })
            .unwrap();
        if page.is_empty() {
            break;
        }
        for t in &page {
            i += 1;
            if i % 7 == 0 {
                out.push((t.account_id.clone(), t.thread_id.clone()));
            }
        }
        before = page.last().map(|t| t.last_date);
        // Skip ahead to spread the sample over the mailbox.
        before = before.map(|b| b - 20 * corpus::DAY);
    }
    out.truncate(n);
    out
}

fn thread_workload(store: &Store, runs: usize) -> Value {
    let ids = thread_ids(store, 300);
    let mut c = Cases::new();
    let mut times = Vec::new();
    let mut msgs = 0;
    for _ in 0..(runs / 10).max(2) {
        for (a, t) in &ids {
            let s = Instant::now();
            let d = store.get_thread(a, t).unwrap();
            times.push(s.elapsed().as_secs_f64() * 1000.0);
            msgs += d.map_or(0, |d| d.messages.len());
        }
    }
    c.rows.push((
        format!("get_thread ({} conversations)", ids.len()),
        times,
        msgs,
    ));
    c.print("open a conversation (store read only)")
}

fn ymd(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap()
        .format("%Y/%m/%d")
        .to_string()
}

fn search_workload(store: &Store, runs: usize) -> Value {
    let now = now_ms();
    let after = ymd(now - 365 * corpus::DAY);
    let before = ymd(now - 180 * corpus::DAY);
    let queries: Vec<String> = vec![
        "meeting".into(),
        "invoice".into(),
        "the".into(),
        "lease renewal".into(),
        "quarterly report deadline".into(),
        "\"early termination clause\"".into(),
        "\"to the\"".into(),
        "from:mike".into(),
        "from:mike.delgado@realty.example".into(),
        "from:mike lease".into(),
        "has:attachment".into(),
        "has:pdf".into(),
        "has:pdf date:\"last spring\"".into(),
        format!("invoice after:{after} before:{before}"),
        "label:Label_3".into(),
        "label:Label_3 meeting".into(),
        "is:unread".into(),
        "is:unread invoice".into(),
        "in:sent budget".into(),
        "older_than:2y tax".into(),
        "invoice -stripe".into(),
        "lease OR mortgage".into(),
        "filename:20417".into(),
        "account:personal invoice".into(),
        "INV-20417".into(),
        "the pdf mike sent about the lease date:\"last spring\"".into(),
        "zzqxjv".into(),
        // Beyond bench.rs: thread-state filters and typed-out domains.
        "is:unanswered budget".into(),
        "is:awaiting".into(),
        "domain:realty.example".into(),
        "to:me invoice".into(),
        "is:starred".into(),
        "has:invite".into(),
    ];
    let search = |q: &str| {
        store
            .search(&SearchRequest {
                query: q.to_string(),
                account_id: None,
                account_ids: None,
                limit: 50,
            })
            .unwrap()
    };
    for q in ["warmup", "the", "invoice"] {
        search(q);
    }
    let mut c = Cases::new();
    for q in &queries {
        c.run(
            q,
            runs,
            |r: &penguin_core::SearchResponse| r.hits.len(),
            || search(q),
        );
    }
    for s in [
        "invoice from:mike",
        "quarterly report",
        "lease has:pdf",
        "priya budget",
    ] {
        let prefixes: Vec<&str> = s
            .char_indices()
            .map(|(i, ch)| &s[..i + ch.len_utf8()])
            .collect();
        let mut times = Vec::new();
        for _ in 0..(runs / 5).max(2) {
            for p in &prefixes {
                let t = Instant::now();
                search(p);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        c.rows.push((
            format!("typing `{s}` (per keystroke)"),
            times,
            prefixes.len(),
        ));
    }
    c.print("search (keyword)")
}

fn ask_workload(store: &Store, runs: usize) -> Value {
    let now = now_ms();
    let p = corpus::Corpus::new(42, now).people;
    let questions: Vec<String> = vec![
        "when did I last speak to Mike from Kettle on the Knoll / Fernwood?".into(),
        format!("when did I last email {}", p[3].name),
        format!("first email from {}", p[11].email),
        format!("how long have I known {}", p[7].email),
        format!("who is {}", p[20].name),
        format!("how many emails from {} this year", p[5].name),
        "how much did I spend on Uber this year".into(),
        format!("latest pdf from {}", p[2].name),
        "latest invoice".into(),
        "what am I waiting on".into(),
        "what do I owe replies to".into(),
        "who emails me the most".into(),
        "who emailed me about the budget".into(),
        "how many emails about the budget".into(),
        "when is the dinner".into(),
        "when is my flight to Lisbon".into(),
        "where is my package".into(),
        "what bills are due".into(),
        "what did I order from Uber".into(),
        "what is the wifi password for the offsite".into(),
    ];
    let scope = AskScope::default();
    let mut c = Cases::new();
    for q in &questions {
        c.run(
            q,
            runs,
            |a: &penguin_core::ask::AskAnswer| {
                usize::from(a.confidence != penguin_core::ask::AskConfidence::None)
            },
            || store.ask(q, &scope, now, 0).unwrap(),
        );
    }
    c.print("ask")
}

fn write_workload(store: &Store, runs: usize) -> Value {
    let ids = thread_ids(store, 40);
    let mut c = Cases::new();
    let inbox = vec!["INBOX".to_string()];
    let unread = vec!["UNREAD".to_string()];
    let mut times = Vec::new();
    for _ in 0..(runs / 10).max(2) {
        for (a, t) in &ids {
            let s = Instant::now();
            store.modify_thread_labels(a, t, &inbox, &[]).unwrap();
            store.modify_thread_labels(a, t, &[], &inbox).unwrap();
            times.push(s.elapsed().as_secs_f64() * 1000.0 / 2.0);
        }
    }
    c.rows.push((
        "archive / move to inbox (per action)".into(),
        times,
        ids.len(),
    ));
    let mut times = Vec::new();
    for _ in 0..(runs / 10).max(2) {
        for (a, t) in &ids {
            let s = Instant::now();
            store.modify_thread_labels(a, t, &unread, &[]).unwrap();
            store.modify_thread_labels(a, t, &[], &unread).unwrap();
            times.push(s.elapsed().as_secs_f64() * 1000.0 / 2.0);
        }
    }
    c.rows
        .push(("mark unread / read (per action)".into(), times, ids.len()));
    c.print("user actions (writes)")
}

/// CPU time of the calling thread, in seconds (Linux; wall time elsewhere).
fn thread_cpu_secs() -> f64 {
    #[cfg(target_os = "linux")]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a valid out-pointer; the call only writes the timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
    }
    #[cfg(not(target_os = "linux"))]
    {
        0.0
    }
}

/// Sync's write path into the big mailbox: 5,000 new messages (ids that
/// aren't stored yet, dates all over the eight years, like a backfill) in
/// 100-message transactions, then the fact extractor over them. Last,
/// because it changes the corpus.
fn ingest_workload(store: &Store, _runs: usize) -> Value {
    const N: usize = 5_000;
    let mut corpus = corpus::Corpus::new(4242, now_ms());
    let mut msgs = Vec::with_capacity(N + 64);
    let mut i = 0usize;
    while msgs.len() < N {
        let acct = [0, 0, 0, 0, 0, 0, 1, 1, 1, 2][i % 10];
        i += 1;
        for mut m in corpus.thread(acct) {
            m.id = format!("new-{}", m.id);
            m.thread_id = format!("new-{}", m.thread_id);
            msgs.push(m);
        }
    }
    msgs.truncate(N);
    let mut c = Cases::new();
    let mut times = Vec::new();
    let cpu0 = thread_cpu_secs();
    let t0 = Instant::now();
    for chunk in msgs.chunks(100) {
        let t = Instant::now();
        store.upsert_messages(chunk).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu = thread_cpu_secs() - cpu0;
    println!(
        "\ningest: {N} messages, {:.0} msg/s, {:.3} ms CPU per message",
        N as f64 / wall,
        cpu * 1000.0 / N as f64
    );
    c.rows
        .push(("upsert_messages, 100-message batch".into(), times, N));
    let t = Instant::now();
    let cpu0 = thread_cpu_secs();
    let mut scanned = 0;
    loop {
        let p = store.extract_pending(500).unwrap();
        scanned += p.scanned;
        if p.remaining == 0 || p.scanned == 0 {
            break;
        }
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "extract: {scanned} messages, {:.0} msg/s, {:.3} ms CPU per message",
        scanned as f64 / secs,
        (thread_cpu_secs() - cpu0) * 1000.0 / scanned.max(1) as f64
    );
    c.rows.push((
        "extract_pending (all new), per message".into(),
        vec![secs * 1000.0 / scanned.max(1) as f64],
        scanned,
    ));
    c.print("sync writes")
}

/// The same write path into a new, empty database next to the corpus:
/// 20,000 messages, where SQLite's page traffic is small and the CPU spent
/// per statement shows.
fn ingest_fresh_workload(_store: &Store, _runs: usize) -> Value {
    const N: usize = 20_000;
    let dir = std::env::temp_dir();
    let path = FRESH_DIR
        .with(|d| d.borrow().clone())
        .unwrap_or(dir)
        .join("ingest-fresh.db");
    for s in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{s}", path.display()));
    }
    let fresh = Store::open(&path).unwrap();
    for (i, a) in corpus::ACCOUNTS.iter().enumerate() {
        fresh
            .upsert_account(&penguin_core::Account {
                id: a.to_string(),
                email: a.to_string(),
                color: "#0a84ff".into(),
                added_at: i as i64,
                ..penguin_core::Account::default()
            })
            .unwrap();
    }
    let mut corpus = corpus::Corpus::new(4343, now_ms());
    let mut msgs = Vec::with_capacity(N + 64);
    let mut i = 0usize;
    while msgs.len() < N {
        msgs.extend(corpus.thread([0, 0, 0, 0, 0, 0, 1, 1, 1, 2][i % 10]));
        i += 1;
    }
    msgs.truncate(N);
    let mut times = Vec::new();
    let cpu0 = thread_cpu_secs();
    let t0 = Instant::now();
    for chunk in msgs.chunks(100) {
        let t = Instant::now();
        fresh.upsert_messages(chunk).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu = thread_cpu_secs() - cpu0;
    println!(
        "\ningest (fresh db): {N} messages, {:.0} msg/s, {:.3} ms CPU per message",
        N as f64 / wall,
        cpu * 1000.0 / N as f64
    );
    drop(fresh);
    for s in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{s}", path.display()));
    }
    let mut c = Cases::new();
    c.rows.push((
        "upsert_messages, 100-message batch (fresh db)".into(),
        times,
        N,
    ));
    c.rows.push((
        "CPU ms per message (fresh db)".into(),
        vec![cpu * 1000.0 / N as f64],
        N,
    ));
    c.print("sync writes, fresh database")
}

/// Bytes on disk of every table and index (`Store::table_sizes`, dbstat),
/// for what a new index costs. Visits every page.
fn sizes_workload(store: &Store, _runs: usize) -> Value {
    let sizes = store.table_sizes().unwrap();
    let total: u64 = sizes.iter().map(|s| s.bytes).sum();
    println!(
        "\n## sizes ({:.1} MiB total)\n\n| name | kind | KiB |\n|---|---|---:|",
        total as f64 / 1048576.0
    );
    let mut out = Vec::new();
    for s in &sizes {
        println!("| {} | {} | {} |", s.name, s.kind, s.bytes / 1024);
        out.push(json!({ "name": s.name, "kind": s.kind, "bytes": s.bytes }));
    }
    json!({ "cases": [], "sizes": out, "total": total })
}

thread_local! {
    static FRESH_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "sql-profile")]
mod profile {
    use super::*;
    use penguin_core::sqlprof;
    use std::collections::BTreeMap;

    /// The statements that cost the most since the last call, with plans.
    pub fn report(db: &std::path::Path, title: &str, plans: usize) -> Value {
        let stats = sqlprof::take();
        let conn =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let mut rows: Vec<(String, sqlprof::Profiled, f64)> = stats
            .into_iter()
            .map(|(sql, p)| {
                let total: f64 = p.runs.iter().map(|d| d.as_secs_f64() * 1000.0).sum();
                (sql, p, total)
            })
            .collect();
        rows.sort_by(|a, b| b.2.total_cmp(&a.2));
        let grand: f64 = rows.iter().map(|r| r.2).sum();
        println!(
            "\n### {title}: statements by total time ({grand:.1} ms, {} shapes)\n",
            rows.len()
        );
        let mut out = Vec::new();
        for (i, (sql, p, total)) in rows.iter().enumerate() {
            let mut t: Vec<f64> = p.runs.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
            let p50 = pct(&mut t, 0.5);
            let p95 = pct(&mut t, 0.95);
            let flags = [
                (p.fullscan, "FULLSCAN"),
                (p.sorted, "SORT"),
                (p.autoindex, "AUTOINDEX"),
            ]
            .iter()
            .filter(|f| f.0)
            .map(|f| f.1)
            .collect::<Vec<_>>()
            .join(" ");
            let plan = explain(&conn, &p.expanded);
            let bad = plan.iter().any(|l| {
                (l.contains("SCAN ") && !l.contains("VIRTUAL TABLE") && !l.contains("CONSTANT ROW"))
                    || l.contains("TEMP B-TREE")
                    || l.contains("AUTOMATIC")
            });
            let show = i < plans || bad || !flags.is_empty();
            if show {
                let one_line: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
                println!(
                    "- {total:.1} ms total, {} runs, p50 {p50:.3} p95 {p95:.3} {flags}\n  `{}`",
                    p.runs.len(),
                    one_line.chars().take(400).collect::<String>()
                );
                for l in &plan {
                    println!("    {l}");
                }
            }
            out.push(json!({
                "sql": sql, "runs": p.runs.len(), "totalMs": total, "p50": p50, "p95": p95,
                "flags": flags, "plan": plan,
            }));
        }
        Value::Array(out)
    }

    fn explain(conn: &rusqlite::Connection, sql: &str) -> Vec<String> {
        let s = sql.trim_start();
        let head = s.get(..6).unwrap_or("").to_ascii_uppercase();
        if !(head.starts_with("SELECT") || head.starts_with("WITH")) {
            return Vec::new();
        }
        let Ok(mut stmt) = conn.prepare(&format!("EXPLAIN QUERY PLAN {s}")) else {
            return vec!["(plan unavailable)".into()];
        };
        let mut depth: BTreeMap<i64, usize> = BTreeMap::new();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        let mut out = Vec::new();
        for r in rows.flatten() {
            let d = depth.get(&r.1).map_or(0, |d| d + 1);
            depth.insert(r.0, d);
            out.push(format!("{}{}", "  ".repeat(d), r.2));
        }
        out
    }
}

fn main() {
    let args = parse_args();
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; numbers are meaningless. Use --release.");
    }
    let t = Instant::now();
    // --read-only: no migrations or writes (e.g. `--only sizes` on a
    // corpus an older build made).
    let store = if args.read_only {
        Store::open_read_only(&args.db).unwrap()
    } else {
        Store::open(&args.db).unwrap()
    };
    // The first open of a copied corpus includes any new migrations.
    println!(
        "Store::open (migrations, statistics check): {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    let pending = if args.read_only {
        0
    } else {
        store.extract_remaining().unwrap()
    };
    if pending > 0 {
        let t = Instant::now();
        let (mut scanned, mut found) = (0usize, 0usize);
        loop {
            let p = store.extract_pending(2_000).unwrap();
            scanned += p.scanned;
            found += p.found;
            if p.remaining == 0 || p.scanned == 0 {
                break;
            }
        }
        let secs = t.elapsed().as_secs_f64();
        println!(
            "fact extraction backfill: {scanned} messages in {secs:.1} s ({:.0}/s), {found} facts",
            scanned as f64 / secs
        );
        store.optimize().unwrap();
    }
    #[cfg(feature = "sql-profile")]
    penguin_core::sqlprof::take();
    let wants = |w: &str| args.only.as_ref().is_none_or(|o| o.iter().any(|x| x == w));
    let mut out = serde_json::Map::new();
    type Workload = fn(&Store, usize) -> Value;
    FRESH_DIR.with(|d| *d.borrow_mut() = args.db.parent().map(PathBuf::from));
    let workloads: [(&str, Workload); 9] = [
        ("list", list_workload),
        ("counts", counts_workload),
        ("thread", thread_workload),
        ("search", search_workload),
        ("ask", ask_workload),
        ("write", write_workload),
        ("ingest", ingest_workload),
        ("ingest_fresh", ingest_fresh_workload),
        ("sizes", sizes_workload),
    ];
    for (name, f) in workloads {
        if !wants(name) {
            continue;
        }
        let mut v = f(&store, args.runs);
        #[cfg(feature = "sql-profile")]
        {
            v["statements"] = profile::report(&args.db, name, args.plans);
        }
        out.insert(name.to_string(), std::mem::take(&mut v));
    }
    if let Some(p) = args.out {
        std::fs::write(
            p,
            serde_json::to_string_pretty(&Value::Object(out)).unwrap(),
        )
        .unwrap();
    }
}
