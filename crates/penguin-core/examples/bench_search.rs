//! Search/store benchmark over a synthetic mailbox.
//!
//!   cargo run -p penguin-core --release --example bench_search
//!
//! Env: PENGUIN_BENCH_N (messages, default 300000), PENGUIN_BENCH_DIR
//! (db location, default target/bench-db), PENGUIN_BENCH_REUSE=1 to reuse an
//! existing db instead of rebuilding.

#[path = "support/corpus.rs"]
mod corpus;

use std::time::Instant;

use penguin_core::store::CalendarCursor;
use penguin_core::{
    Account, CalendarEvent, CalendarInfo, EventAttendee, InboxTab, ListQuery, MailboxView,
    SearchRequest, Store,
};

fn pct(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn dir_size(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

fn main() {
    if std::env::var("PENGUIN_BENCH_TRACE").is_ok_and(|v| v == "1") {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stderr)
            .init();
    }
    let n: usize = std::env::var("PENGUIN_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300_000);
    let dir = std::path::PathBuf::from(
        std::env::var("PENGUIN_BENCH_DIR").unwrap_or_else(|_| "target/bench-db".into()),
    );
    let reuse = std::env::var("PENGUIN_BENCH_REUSE").is_ok_and(|v| v == "1");
    let path = dir.join(format!("bench-{n}.db"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    if !reuse || !path.exists() {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&path).unwrap();
        for (i, a) in corpus::ACCOUNTS.iter().enumerate() {
            store
                .upsert_account(&Account {
                    id: a.to_string(),
                    email: a.to_string(),
                    display_name: Some(format!("Alex {i}")),
                    nickname: None,
                    color: "#0a84ff".into(),
                    added_at: i as i64,
                    ..Account::default()
                })
                .unwrap();
        }
        let system = [
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
            let mut labels: Vec<penguin_core::Label> = system
                .iter()
                .map(|id| penguin_core::Label {
                    account_id: a.into(),
                    id: id.to_string(),
                    name: id.to_string(),
                    kind: "system".into(),
                    color: None,
                    unread_count: None,
                    hidden: false,
                })
                .collect();
            labels.extend((1..=20).map(|i| penguin_core::Label {
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
        let mut corpus = corpus::Corpus::new(42, now);
        let mut gen_time = 0.0;
        let mut text_bytes = 0usize;
        let mut html_bytes = 0usize;
        let started = Instant::now();
        let mut batch = corpus.planted();
        let mut total = 0;
        while total < n {
            let g = Instant::now();
            let acct = if total % 10 < 6 {
                0
            } else if total % 10 < 9 {
                1
            } else {
                2
            };
            let mut th = corpus.thread(acct);
            th.truncate(n - total);
            total += th.len();
            batch.extend(th);
            gen_time += g.elapsed().as_secs_f64();
            if batch.len() >= 500 || total >= n {
                for m in &batch {
                    text_bytes += m.body_text.len() + m.subject.len();
                    html_bytes += m.body_html.as_ref().map_or(0, |h| h.len());
                }
                store.upsert_messages(&batch).unwrap();
                batch.clear();
            }
            if total % 50_000 < 3 {
                eprint!("\r  inserted {total}/{n}");
            }
        }
        let insert = started.elapsed().as_secs_f64() - gen_time;
        eprintln!();
        insert_events(&store, n, now);
        let size_before = dir_size(&dir);
        let o = Instant::now();
        store.optimize().unwrap();
        let optimize = o.elapsed().as_secs_f64();
        drop(store);
        // Reopen so the WAL is checkpointed into the main file.
        drop(Store::open(&path).unwrap());
        println!("## Build\n");
        println!("| metric | value |\n|---|---|");
        println!("| messages | {} |", n + 3);
        println!(
            "| plain text (subject+body) | {:.0} MiB |",
            text_bytes as f64 / 1048576.0
        );
        println!(
            "| raw HTML bodies | {:.0} MiB |",
            html_bytes as f64 / 1048576.0
        );
        println!(
            "| insert + index (excl. generation) | {:.1} s ({:.0} msg/s) |",
            insert,
            n as f64 / insert
        );
        println!(
            "| size before optimize (db+wal) | {:.0} MiB |",
            size_before as f64 / 1048576.0
        );
        println!("| FTS optimize | {:.1} s |", optimize);
        println!(
            "| db size on disk | {:.0} MiB |",
            dir_size(&dir) as f64 / 1048576.0
        );
        print_breakdown(&path, n + 3);
    } else {
        println!("(reusing {})", path.display());
    }

    let store = Store::open(&path).unwrap();
    // Warm up.
    for q in ["warmup", "the", "invoice"] {
        store
            .search(&SearchRequest {
                query: q.into(),
                account_id: None,
                account_ids: None,
                limit: 50,
            })
            .unwrap();
    }
    // PENGUIN_BENCH_ONLY=hybrid: just the keyword vs hybrid section.
    if std::env::var("PENGUIN_BENCH_ONLY").is_ok_and(|v| v == "hybrid") {
        bench_hybrid(&store);
        return;
    }
    for q in ["warmup", "the", "invoice"] {
        store
            .search(&SearchRequest {
                query: q.into(),
                account_id: None,
                account_ids: None,
                limit: 50,
            })
            .unwrap();
    }

    println!("\n## list_threads (50 rows, warm, 200 runs)\n");
    println!("| view | p50 ms | p95 ms | p99 ms | rows |\n|---|---:|---:|---:|---:|");
    type Case<'a> = (
        &'a str,
        MailboxView,
        Option<InboxTab>,
        Option<&'a str>,
        bool,
    );
    let views: Vec<Case> = vec![
        ("Inbox (unified)", MailboxView::Inbox, None, None, false),
        (
            "Inbox / Important",
            MailboxView::Inbox,
            Some(InboxTab::Important),
            None,
            false,
        ),
        (
            "Inbox / Newsletters",
            MailboxView::Inbox,
            Some(InboxTab::Newsletters),
            None,
            false,
        ),
        (
            "Inbox (one account)",
            MailboxView::Inbox,
            None,
            Some(corpus::ACCOUNTS[1]),
            false,
        ),
        ("All mail", MailboxView::All, None, None, false),
        (
            "All mail, page 20 (cursor)",
            MailboxView::All,
            None,
            None,
            true,
        ),
        ("Done (archived)", MailboxView::Done, None, None, false),
        ("Sent", MailboxView::Sent, None, None, false),
        (
            "Label_3",
            MailboxView::Label("Label_3".into()),
            None,
            None,
            false,
        ),
        ("Starred", MailboxView::Starred, None, None, false),
    ];
    for (name, view, tab, acct, deep) in views {
        let mut before = None;
        if deep {
            for _ in 0..20 {
                let page = store
                    .list_threads(&ListQuery {
                        view: view.clone(),
                        tab,
                        account_id: acct.map(Into::into),
                        account_ids: None,
                        limit: 50,
                        before,
                        unread_only: false,
                        split: None,
                    })
                    .unwrap();
                before = page.last().map(|t| t.last_date);
            }
        }
        let mut times = Vec::new();
        let mut rows = 0;
        for _ in 0..200 {
            let t = Instant::now();
            let page = store
                .list_threads(&ListQuery {
                    view: view.clone(),
                    tab,
                    account_id: acct.map(Into::into),
                    account_ids: None,
                    limit: 50,
                    before,
                    unread_only: false,
                    split: None,
                })
                .unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            rows = page.len();
        }
        times.sort_by(f64::total_cmp);
        println!(
            "| {name} | {:.2} | {:.2} | {:.2} | {rows} |",
            pct(&times, 0.5),
            pct(&times, 0.95),
            pct(&times, 0.99)
        );
    }

    {
        let mut times = Vec::new();
        let mut n_labels = 0;
        for _ in 0..200 {
            let t = Instant::now();
            n_labels = store.list_labels(None).unwrap().len();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "\nlist_labels(None) with local unread counts ({n_labels} labels): p50 {:.2} ms, p95 {:.2} ms",
            pct(&times, 0.5),
            pct(&times, 0.95)
        );
    }

    let queries: &[(&str, &str)] = &[
        ("single common word", "meeting"),
        ("single mid-frequency word", "invoice"),
        ("single rare word", "escrow"),
        ("very common word", "the"),
        ("two words", "lease renewal"),
        ("two common words", "time people"),
        ("three words", "quarterly report deadline"),
        ("prefix 2 chars", "in"),
        ("prefix 3 chars", "inv"),
        ("prefix 4 chars", "invo"),
        ("prefix 4 chars, common stem", "ther"),
        ("prefix complete", "invoice"),
        ("prefix, 2nd word", "lease ren"),
        ("from: name", "from:mike"),
        ("from: email", "from:mike.delgado@realty.example"),
        ("from: + word", "from:mike lease"),
        ("has:pdf + date:", "has:pdf date:\"last spring\""),
        (
            "has:pdf + word + date:",
            "lease has:pdf date:\"last spring\"",
        ),
        ("rare identifier", "INV-20417"),
        ("identifier digits only", "20417"),
        ("filename: substring", "filename:20417"),
        ("quoted phrase", "\"early termination clause\""),
        ("phrase, common words", "\"to the\""),
        (
            "natural language",
            "the pdf mike sent about the lease date:\"last spring\"",
        ),
        ("exclusion", "invoice -stripe"),
        ("OR", "lease OR mortgage"),
        ("is:unread (filter only)", "is:unread"),
        ("in:sent + word", "in:sent budget"),
        ("label + word", "label:Label_3 meeting"),
        ("account + word", "account:personal invoice"),
        ("account + filter only", "account:work is:unread"),
        ("in:anywhere + word", "password in:anywhere"),
        ("older_than + word", "tax older_than:2y"),
        ("no results", "zzqxjv"),
        ("type:event (upcoming)", "type:event"),
        ("type:event + word", "type:event review"),
        ("type:event + person", "type:event to:priya"),
        ("has:invite (filter only)", "has:invite"),
        ("has:invite + word", "has:invite review"),
        // docs/SEARCH.md operators.
        ("is:new-sender (filter only)", "is:new-sender"),
        ("new senders last week", "is:new-sender date:\"last week\""),
        ("is:new-sender + word", "is:new-sender invoice"),
        ("to:new (filter only)", "to:new"),
        ("to:new + this month", "to:new date:\"this month\""),
        ("is:unanswered (filter only)", "is:unanswered"),
        (
            "is:unanswered + from + month",
            "is:unanswered from:mike date:\"this month\"",
        ),
        ("is:awaiting (filter only)", "is:awaiting"),
        ("is:replied + word", "is:replied invoice"),
        ("is:reply + word", "is:reply budget"),
        ("from:me + date", "from:me date:\"last month\""),
        ("sent on a day", "in:sent date:\"august 27\""),
        ("to:me + word", "to:me invoice"),
        ("domain: (filter only)", "domain:acme.example"),
        ("with: + has:pdf", "with:mike has:pdf"),
        ("larger: (filter only)", "larger:4M"),
        ("smaller: + word", "smaller:20k invoice"),
        ("messages:>5 (filter only)", "messages:>5"),
        ("day:weekend + word", "day:weekend invoice"),
        ("date range a..b", "date:aug1..aug15"),
        ("grouping", "(from:mike OR from:priya) has:pdf -invoice"),
        ("negated group", "invoice -(has:pdf OR has:image)"),
        ("has:link + word", "has:link invoice"),
        ("has:otp (filter only)", "has:otp"),
        ("is:newsletter + word", "is:newsletter sale"),
        ("is:snoozed (filter only)", "is:snoozed"),
        (
            "rare combination (worst case scan)",
            "is:first-outbound has:invite larger:4M",
        ),
        // Word forms and formats (lexicon.rs): each looks words up in the
        // index before searching.
        ("plural of an indexed word", "quarterly reports"),
        ("British spelling", "colour meeting"),
        ("unknown compound", "leaserenewal"),
        ("code typed without its dash", "INV20417"),
        ("digits typed without separators", "4155550138"),
        ("typo", "invioce"),
        ("three words, one inflected", "reports deadline quarterly"),
    ];
    println!("\n## search (limit 50, warm, 60 runs each)\n");
    println!("| query | kind | p50 ms | p95 ms | p99 ms | threads | top hit |\n|---|---|---:|---:|---:|---:|---|");
    let mut all = Vec::new();
    for (kind, q) in queries {
        let mut times = Vec::new();
        let mut hits = 0;
        let mut top = String::new();
        for _ in 0..60 {
            let t = Instant::now();
            let r = store
                .search(&SearchRequest {
                    query: q.to_string(),
                    account_id: None,
                    account_ids: None,
                    limit: 50,
                })
                .unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            hits = r.hits.len();
            top = r
                .hits
                .first()
                .map(|h| h.message_id.clone())
                .unwrap_or_default();
        }
        times.sort_by(f64::total_cmp);
        all.extend(times.iter().copied());
        let top = if top.starts_with("planted") {
            top
        } else if top.is_empty() {
            "-".into()
        } else {
            "(synthetic)".into()
        };
        println!(
            "| `{q}` | {kind} | {:.1} | {:.1} | {:.1} | {hits} | {top} |",
            pct(&times, 0.5),
            pct(&times, 0.95),
            pct(&times, 0.99)
        );
    }
    all.sort_by(f64::total_cmp);
    println!(
        "\nAll queries pooled: p50 {:.1} ms, p95 {:.1} ms, p99 {:.1} ms",
        pct(&all, 0.5),
        pct(&all, 0.95),
        pct(&all, 0.99)
    );

    // A profile: two of the three accounts, passed as account_ids.
    {
        let profile: Vec<String> = vec![corpus::ACCOUNTS[0].into(), corpus::ACCOUNTS[2].into()];
        let lq = |view: MailboxView, before: Option<i64>| ListQuery {
            view,
            tab: None,
            account_id: None,
            account_ids: Some(profile.clone()),
            limit: 50,
            before,
            unread_only: false,
            split: None,
        };
        println!("\n## profile scope (account_ids = 2 of 3 accounts, warm)\n");
        println!("| operation | p50 ms | p95 ms | p99 ms | rows |\n|---|---:|---:|---:|---:|");
        let mut deep = None;
        for _ in 0..20 {
            deep = store
                .list_threads(&lq(MailboxView::All, deep))
                .unwrap()
                .last()
                .map(|t| t.last_date);
        }
        let cases: Vec<(&str, MailboxView, Option<i64>)> = vec![
            ("list Inbox", MailboxView::Inbox, None),
            ("list All mail", MailboxView::All, None),
            ("list All mail, page 20 (cursor)", MailboxView::All, deep),
            ("list Sent", MailboxView::Sent, None),
            ("list Label_3", MailboxView::Label("Label_3".into()), None),
        ];
        for (name, view, before) in cases {
            let mut times = Vec::new();
            let mut rows = 0;
            for _ in 0..200 {
                let t = Instant::now();
                rows = store.list_threads(&lq(view.clone(), before)).unwrap().len();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            println!(
                "| {name} | {:.2} | {:.2} | {:.2} | {rows} |",
                pct(&times, 0.5),
                pct(&times, 0.95),
                pct(&times, 0.99)
            );
        }
        let mut times = Vec::new();
        let mut rows = 0;
        for _ in 0..200 {
            let t = Instant::now();
            rows = store.list_labels_in(Some(&profile)).unwrap().len();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "| list_labels_in | {:.2} | {:.2} | {:.2} | {rows} |",
            pct(&times, 0.5),
            pct(&times, 0.95),
            pct(&times, 0.99)
        );

        let mut pooled = Vec::new();
        for (kind, q) in queries {
            let mut times = Vec::new();
            let mut hits = 0;
            for _ in 0..60 {
                let t = Instant::now();
                hits = store
                    .search(&SearchRequest {
                        query: q.to_string(),
                        account_id: None,
                        account_ids: Some(profile.clone()),
                        limit: 50,
                    })
                    .unwrap()
                    .hits
                    .len();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            pooled.extend(times.iter().copied());
            println!(
                "| search `{q}` ({kind}) | {:.1} | {:.1} | {:.1} | {hits} |",
                pct(&times, 0.5),
                pct(&times, 0.95),
                pct(&times, 0.99)
            );
        }
        pooled.sort_by(f64::total_cmp);
        println!(
            "\nProfile searches pooled: p50 {:.1} ms, p95 {:.1} ms, p99 {:.1} ms\n",
            pct(&pooled, 0.5),
            pct(&pooled, 0.95),
            pct(&pooled, 0.99)
        );
    }

    // As-you-type: each keystroke is a separate search.
    let typed = "invoice from:mike";
    let mut ks = Vec::new();
    for _ in 0..20 {
        for i in 1..=typed.len() {
            let t = Instant::now();
            store
                .search(&SearchRequest {
                    query: typed[..i].to_string(),
                    account_id: None,
                    account_ids: None,
                    limit: 50,
                })
                .unwrap();
            ks.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    ks.sort_by(f64::total_cmp);
    println!(
        "As-you-type \"{typed}\" (every prefix): p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        pct(&ks, 0.5),
        pct(&ks, 0.95),
        ks.last().unwrap()
    );

    bench_hybrid(&store);

    // Cold-ish: first query on a fresh connection pool.
    drop(store);
    let store = Store::open(&path).unwrap();
    let t = Instant::now();
    store
        .search(&SearchRequest {
            query: "contract renewal".into(),
            account_id: None,
            account_ids: None,
            limit: 50,
        })
        .unwrap();
    println!(
        "First query after reopen (OS page cache warm): {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Text pipeline throughput.
    let mut c = corpus::Corpus::new(7, now);
    let mut htmls = Vec::new();
    while htmls.len() < 2000 {
        for m in c.thread(0) {
            if let Some(h) = m.body_html {
                htmls.push(h);
            }
        }
    }
    let bytes: usize = htmls.iter().map(|h| h.len()).sum();
    let t = Instant::now();
    let mut out = 0;
    for h in &htmls {
        out += penguin_core::text::html_to_text(h).len();
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "\nhtml_to_text: {} docs, {:.1} MiB in {:.0} ms = {:.0} MiB/s ({} bytes out)",
        htmls.len(),
        bytes as f64 / 1048576.0,
        secs * 1000.0,
        bytes as f64 / 1048576.0 / secs,
        out
    );
    // Same corpus through html2text (html5ever DOM + layout), for comparison.
    let t = Instant::now();
    let mut out2 = 0;
    for h in &htmls {
        out2 += html2text::from_read(h.as_bytes(), 10_000)
            .map(|s| s.len())
            .unwrap_or(0);
    }
    let secs2 = t.elapsed().as_secs_f64();
    println!(
        "html2text crate: {:.0} ms = {:.0} MiB/s ({} bytes out), {:.1}x slower",
        secs2 * 1000.0,
        bytes as f64 / 1048576.0 / secs2,
        out2,
        secs2 / secs
    );
    // A realistic heavy marketing email: ~200 KB of nested tables and inline CSS.
    let big = format!("<html><head><style>{}</style></head><body>{}</body></html>", ".x{color:red}".repeat(2000), "<table><tr><td style=\"padding:0;font-family:Arial\"><a href=\"https://t.example/c?id=1&amp;u=2\">Shop the sale &rarr;</a> caf&eacute; &#8217;s</td></tr></table>".repeat(1500));
    let t = Instant::now();
    for _ in 0..20 {
        std::hint::black_box(penguin_core::text::html_to_text(&big));
    }
    let a = t.elapsed().as_secs_f64() / 20.0;
    let t = Instant::now();
    for _ in 0..5 {
        std::hint::black_box(html2text::from_read(big.as_bytes(), 10_000).ok());
    }
    let b = t.elapsed().as_secs_f64() / 5.0;
    println!(
        "{} KB marketing email: html_to_text {:.2} ms, html2text {:.2} ms",
        big.len() / 1024,
        a * 1000.0,
        b * 1000.0
    );
}

/// Stand-in for the vector index at mailbox scale: one chunk per message,
/// visited in a query-dependent pseudo-random order, the filter called on
/// each until `k` pass (what a filtered ANN search does on the nodes it
/// visits), scores descending. No vector math: the index's own cost belongs
/// to the index; this measures what hybrid search adds around it.
struct BenchIndex {
    chunks: Vec<penguin_semantic::ChunkRef>,
}

impl penguin_semantic::VectorIndex for BenchIndex {
    fn upsert(&self, _: penguin_semantic::ChunkRef, _: Vec<f32>) -> penguin_semantic::Result<()> {
        Ok(())
    }
    fn remove_message(&self, _: &str, _: &str) -> penguin_semantic::Result<()> {
        Ok(())
    }
    fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn Fn(&penguin_semantic::ChunkRef) -> bool + Sync)>,
    ) -> penguin_semantic::Result<Vec<penguin_semantic::VectorHit>> {
        let n = self.chunks.len();
        let seed = query.iter().fold(0u64, |h, x| h.wrapping_mul(31).wrapping_add(x.to_bits() as u64));
        let step = 7919usize; // prime: visits every chunk once
        let mut out = Vec::with_capacity(k);
        for i in 0..n {
            let c = &self.chunks[(seed as usize).wrapping_add(i * step) % n];
            if filter.is_none_or(|f| f(c)) {
                out.push(penguin_semantic::VectorHit {
                    chunk: c.clone(),
                    score: 0.9 - out.len() as f32 * 0.002,
                });
                if out.len() >= k {
                    break;
                }
            }
        }
        Ok(out)
    }
    fn len(&self) -> usize {
        self.chunks.len()
    }
}

/// Instant query vectors that differ per text (the model's cost is measured
/// separately; see docs/SEARCH-RANKING.md).
struct BenchEmbedder;

impl penguin_semantic::Embedder for BenchEmbedder {
    fn model_id(&self) -> &str {
        "bench"
    }
    fn dims(&self) -> usize {
        4
    }
    fn embed_query(&self, text: &str) -> penguin_semantic::Result<Vec<f32>> {
        let h = text.bytes().fold(0u32, |h, b| h.wrapping_mul(33) ^ b as u32);
        Ok(vec![h as f32, 1.0, 0.0, 0.0])
    }
    fn embed_passages(&self, t: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
        Ok(t.iter().map(|_| vec![0.0; 4]).collect())
    }
}

/// Keyword search against hybrid search on the same queries.
fn bench_hybrid(store: &Store) {
    use std::sync::Arc;
    let t = Instant::now();
    let chunks: Vec<penguin_semantic::ChunkRef> = store
        .match_query("in:anywhere", None, None, i64::MAX as usize)
        .unwrap()
        .into_iter()
        .map(|m| penguin_semantic::ChunkRef {
            account_id: m.account_id,
            thread_id: m.thread_id,
            message_id: m.message_id,
            chunk: 0,
            date: m.date,
        })
        .collect();
    eprintln!("hybrid bench: {} chunk refs in {:.1} s", chunks.len(), t.elapsed().as_secs_f64());
    let handles = penguin_core::SemanticHandles {
        embedder: Arc::new(BenchEmbedder),
        index: Arc::new(BenchIndex { chunks }),
        progress: None,
    };
    let queries: &[(&str, &str)] = &[
        ("keyword route, many matches (meaning skipped)", "invoice"),
        ("keyword route, code", "INV-20417"),
        ("balanced", "lease renewal"),
        ("balanced, common words", "time people"),
        ("semantic, 3 words", "quarterly report deadline"),
        ("semantic, descriptive", "that thing about the lease renewal"),
        ("semantic, question", "when is the budget review meeting"),
        ("+ from: (pre-filtered set)", "from:mike what did he say about the lease"),
        ("+ label: (pre-filtered set)", "label:Label_3 notes from the planning meeting"),
        ("+ is:unread (pre-filtered set)", "is:unread what did they say about the budget"),
        ("+ in:sent (checked after)", "in:sent budget planning thoughts"),
        ("+ has:pdf (checked after)", "has:pdf the signed contract"),
        ("+ date:", "lease renewal paperwork date:\"last spring\""),
        ("exact syntax (no meaning)", "\"early termination clause\""),
    ];
    println!("\n## hybrid search (limit 50, warm, 40 runs each; stand-in index and model)\n");
    println!("| query | route | keyword p50 / p95 ms | hybrid p50 / p95 ms | threads (kw → hybrid) |\n|---|---|---:|---:|---:|");
    let req = |q: &str| SearchRequest {
        query: q.to_string(),
        account_id: None,
        account_ids: None,
        limit: 50,
    };
    let mut kw_all = Vec::new();
    let mut hy_all = Vec::new();
    for (kind, q) in queries {
        let mut kw = Vec::new();
        let mut hy = Vec::new();
        let (mut nk, mut nh) = (0, 0);
        for _ in 0..40 {
            let t = Instant::now();
            nk = store.search(&req(q)).unwrap().hits.len();
            kw.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            nh = store.search_hybrid(&req(q), Some(&handles)).unwrap().hits.len();
            hy.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        kw.sort_by(f64::total_cmp);
        hy.sort_by(f64::total_cmp);
        kw_all.extend(kw.iter().copied());
        hy_all.extend(hy.iter().copied());
        println!(
            "| `{q}` | {kind} | {:.1} / {:.1} | {:.1} / {:.1} | {nk} → {nh} |",
            pct(&kw, 0.5),
            pct(&kw, 0.95),
            pct(&hy, 0.5),
            pct(&hy, 0.95)
        );
    }
    kw_all.sort_by(f64::total_cmp);
    hy_all.sort_by(f64::total_cmp);
    println!(
        "\nPooled: keyword p50 {:.1} / p95 {:.1} ms; hybrid p50 {:.1} / p95 {:.1} ms (plus the model's query embedding)",
        pct(&kw_all, 0.5),
        pct(&kw_all, 0.95),
        pct(&hy_all, 0.5),
        pct(&hy_all, 0.95)
    );
    // Typing a descriptive query: every prefix is a search.
    let typed = "that thing about the lease renewal";
    for (name, hybrid) in [("keyword", false), ("hybrid", true)] {
        let mut ks = Vec::new();
        for _ in 0..10 {
            for i in 1..=typed.len() {
                let t = Instant::now();
                if hybrid {
                    store.search_hybrid(&req(&typed[..i]), Some(&handles)).unwrap();
                } else {
                    store.search(&req(&typed[..i])).unwrap();
                }
                ks.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        ks.sort_by(f64::total_cmp);
        println!(
            "As-you-type \"{typed}\", {name}: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            pct(&ks, 0.5),
            pct(&ks, 0.95),
            ks.last().unwrap()
        );
    }
}

fn print_breakdown(path: &std::path::Path, n: usize) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let has_dbstat: bool = conn
        .query_row(
            "SELECT count(*) FROM pragma_module_list WHERE name = 'dbstat'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !has_dbstat {
        return;
    }
    println!(
        "\n| table / index | MiB total | MiB per 100k msgs | bytes per msg |\n|---|---:|---:|---:|"
    );
    let mut stmt = conn
        .prepare("SELECT name, sum(pgsize) FROM dbstat GROUP BY name ORDER BY 2 DESC")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .unwrap();
    let mut total = 0i64;
    for r in rows {
        let (name, size) = r.unwrap();
        total += size;
        println!(
            "| {name} | {:.1} | {:.1} | {:.0} |",
            size as f64 / 1048576.0,
            size as f64 / 1048576.0 * 100_000.0 / n as f64,
            size as f64 / n as f64
        );
    }
    println!(
        "| **total (all pages)** | **{:.1}** | **{:.1}** | **{:.0}** |",
        total as f64 / 1048576.0,
        total as f64 / 1048576.0 * 100_000.0 / n as f64,
        total as f64 / n as f64
    );
    let (text, html): (i64, i64) = conn
        .query_row("SELECT sum(length(body_text)), sum(coalesce(length(body_html), 0)) FROM message_bodies", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    println!(
        "\nstored (compressed) body_text {:.0} MiB, body_html {:.0} MiB",
        text as f64 / 1048576.0,
        html as f64 / 1048576.0
    );
}

/// Synthetic calendars: one per account, n/30 events (10k at 300k messages)
/// spread over two years back and one ahead, with titles that share words
/// with the mail corpus ("meeting", "review", "lease") so mixed searches pay
/// for the Calendar group too.
fn insert_events(store: &Store, n: usize, now: i64) {
    const TITLES: &[&str] = &[
        "Weekly sync",
        "Lease review meeting",
        "Quarterly report review",
        "1:1",
        "Budget planning",
        "Vendor call",
        "Design review",
        "Team meeting",
    ];
    const PEOPLE: &[(&str, &str)] = &[
        ("mike.delgado@realty.example", "Mike Delgado"),
        ("priya.shah@contoso.example", "Priya Shah"),
        ("theo.laurent@harbor.example", "Theo Laurent"),
        ("ana.sousa@linden.example", "Ana Sousa"),
    ];
    let hour = 3_600_000i64;
    let span = 3 * 365 * 24 * hour;
    let per_account = (n / 30).max(1) / corpus::ACCOUNTS.len();
    for (ai, a) in corpus::ACCOUNTS.iter().enumerate() {
        store
            .replace_calendars(
                a,
                &[CalendarInfo {
                    account_id: a.to_string(),
                    id: a.to_string(),
                    summary: a.to_string(),
                    color: None,
                    selected: true,
                    primary: true,
                    access_role: "owner".into(),
                }],
                &[None],
            )
            .unwrap();
        let events: Vec<CalendarEvent> = (0..per_account)
            .map(|i| {
                let start = now - 2 * 365 * 24 * hour + (i as i64 * span / per_account as i64);
                let (email, name) = PEOPLE[(i + ai) % PEOPLE.len()];
                CalendarEvent {
                    account_id: a.to_string(),
                    calendar_id: a.to_string(),
                    id: format!("ev{i}"),
                    ical_uid: Some(format!("ev{i}-{ai}@bench.example")),
                    status: "confirmed".into(),
                    summary: TITLES[i % TITLES.len()].into(),
                    description: "Agenda: status, blockers, next steps".into(),
                    location: if i % 3 == 0 {
                        "Room 4".into()
                    } else {
                        String::new()
                    },
                    start,
                    end: start + hour,
                    all_day: false,
                    start_date: None,
                    end_date: None,
                    organizer: None,
                    attendees: vec![
                        EventAttendee {
                            email: a.to_string(),
                            name: None,
                            response: "accepted".into(),
                            organizer: false,
                            is_self: true,
                            optional: false,
                            resource: false,
                        },
                        EventAttendee {
                            email: email.into(),
                            name: Some(name.into()),
                            response: "accepted".into(),
                            organizer: true,
                            is_self: false,
                            optional: false,
                            resource: false,
                        },
                    ],
                    my_response: Some("accepted".into()),
                    html_link: None,
                    conference_url: None,
                    conference_kind: None,
                    recurring_event_id: None,
                    free: false,
                    updated: 0,
                }
            })
            .collect();
        store
            .replace_calendar_events(a, a, &events, &CalendarCursor::default())
            .unwrap();
    }
}
