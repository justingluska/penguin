//! `search-eval`: judge Penguin's search against a labelled query set.
//!
//!   search-eval run   [--mode keyword|ask|vector|hybrid|penguin] [--embedder hash|file:PATH] [--name N] [--out FILE]
//!                     [--no-extract]
//!                     [--seed 42] [--reps 5] [--dir DIR] [--trec DIR] [--category C] [--verbose]
//!   search-eval diff  BEFORE.json AFTER.json [--top 10]
//!   search-eval check [--seed 42]
//!   search-eval show  QUERY_ID [--mode …] [--seed 42]
//!   search-eval texts [--seed 42] [--out FILE]   (texts to embed offline for --embedder file:)
//!   search-eval real  [--db PATH] [--queries FILE] [--private DIR] [--mode keyword|ask] [--name N] [--no-judge | --judge]
//!
//! See docs/SEARCH-EVAL.md.

use std::collections::{BTreeMap, HashMap};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use penguin_core::Store;
use penguin_eval::corpus::Corpus;
use penguin_eval::eval::{self, EvalQuery, RunFile, RunMeta, DEPTH};
use penguin_eval::judge::{self, Category};
use penguin_eval::queries::all_queries;
use penguin_eval::real;
use penguin_core::SemanticHandles;
use penguin_eval::retrieve::{self, Ask, Hybrid, Keyword, Penguin, Retriever, Vector};

struct Args {
    pos: Vec<String>,
    flags: HashMap<String, String>,
}

impl Args {
    fn parse() -> Args {
        let mut pos = Vec::new();
        let mut flags = HashMap::new();
        let mut it = std::env::args().skip(1).peekable();
        while let Some(a) = it.next() {
            if let Some(k) = a.strip_prefix("--") {
                let v = match it.peek() {
                    Some(v) if !v.starts_with("--") => it.next().unwrap(),
                    _ => "true".into(),
                };
                flags.insert(k.to_string(), v);
            } else {
                pos.push(a);
            }
        }
        Args { pos, flags }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).map(String::as_str)
    }
    fn num(&self, k: &str, d: u64) -> u64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let r = match args.pos.first().map(String::as_str) {
        Some("run") => run(&args),
        Some("diff") => diff(&args),
        Some("check") => check(&args),
        Some("show") => show(&args),
        Some("real") => real_mode(&args),
        Some("texts") => texts(&args),
        _ => Err(include_str!("main.rs")
            .lines()
            .skip(2)
            .take(8)
            .map(|l| l.trim_start_matches("//!").to_string())
            .collect::<Vec<_>>()
            .join("\n")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn git_rev() -> Option<String> {
    let o = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()
        .is_some_and(|o| !o.stdout.is_empty());
    o.status.success().then(|| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout).trim(),
            if dirty { "-dirty" } else { "" }
        )
    })
}

fn host() -> String {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    format!(
        "{} {}, {cpus} threads",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Generate the corpus and load it into a fresh store. With `extract`, the
/// structured-fact scanner then reads every message, as the app's
/// background task does after sync (Ask answers flights, orders, bills…
/// from those facts; without it, Ask extracts search hits on the fly).
fn build_store(seed: u64, dir: &Path, extract: bool) -> Result<(Corpus, Store), String> {
    let corpus = Corpus::generate(seed, now_ms());
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("corpus-{seed}.db"));
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
    let store = Store::open(&path).map_err(|e| e.to_string())?;
    let t = std::time::Instant::now();
    corpus.load_into(&store).map_err(|e| e.to_string())?;
    eprintln!(
        "corpus: {} messages, {} conversations, loaded in {:.1}s ({})",
        corpus.messages.len(),
        corpus.threads.len(),
        t.elapsed().as_secs_f64(),
        path.display()
    );
    if extract {
        let t = std::time::Instant::now();
        let (mut scanned, mut found) = (0, 0);
        loop {
            let p = store.extract_pending(5_000).map_err(|e| e.to_string())?;
            scanned += p.scanned;
            found += p.found;
            if p.remaining == 0 || p.scanned == 0 {
                break;
            }
        }
        eprintln!(
            "facts: {found} extracted from {scanned} messages in {:.1}s",
            t.elapsed().as_secs_f64()
        );
    }
    Ok((corpus, store))
}

fn synthetic_queries(corpus: &Corpus, only: Option<&str>) -> Vec<EvalQuery> {
    let specs = all_queries(corpus.now);
    let qrels = judge::qrels(corpus, &specs);
    specs
        .iter()
        .filter(|q| only.is_none_or(|c| q.category.name() == c))
        .map(|q| EvalQuery {
            id: q.id.to_string(),
            text: q.text.clone(),
            category: q.category.name().to_string(),
            facets: q.facets.iter().map(|f| f.to_string()).collect(),
            qrels: qrels[q.id].clone(),
        })
        .collect()
}

fn make_retriever<'a>(
    mode: &str,
    store: &'a Store,
    vector: Option<&'a Vector>,
    penguin: Option<&SemanticHandles>,
) -> Result<Box<dyn Retriever + 'a>, String> {
    Ok(match mode {
        "penguin" => Box::new(Penguin {
            store,
            handles: penguin.ok_or("penguin mode needs an index")?.clone(),
        }),
        "keyword" => Box::new(Keyword { store }),
        "ask" => Box::new(Ask { store }),
        "vector" => Box::new(VectorRef(vector.ok_or("vector mode needs an index")?)),
        "hybrid" => Box::new(Hybrid {
            keyword: Keyword { store },
            vector: vector.ok_or("hybrid mode needs an index")?,
            k: 60.0,
        }),
        m => return Err(format!("unknown mode {m:?} (keyword, ask, vector, hybrid, penguin)")),
    })
}

struct VectorRef<'a>(&'a Vector);
impl Retriever for VectorRef<'_> {
    fn search(&self, q: &str, k: usize) -> Result<Vec<String>, String> {
        self.0.search(q, k)
    }
}

fn run(args: &Args) -> Result<(), String> {
    let seed = args.num("seed", 42);
    let mode = args.get("mode").unwrap_or("keyword");
    let dir = PathBuf::from(args.get("dir").unwrap_or("target/search-eval"));
    let (corpus, store) = build_store(seed, &dir, args.get("no-extract").is_none())?;
    let needs_vec = matches!(mode, "vector" | "hybrid");
    let emb_name = (needs_vec || mode == "penguin")
        .then(|| args.get("embedder").unwrap_or("hash").to_string());
    let penguin = penguin_index(mode, emb_name.as_deref(), &corpus)?;
    let vector = match emb_name.as_ref().filter(|_| needs_vec) {
        Some(n) => {
            let v = Vector::build(retrieve::embedder_by_name(n)?, &corpus.messages)?;
            eprintln!(
                "vector index: {} chunks with {} in {:.1}s",
                penguin_semantic::VectorIndex::len(&v.index),
                v.embedder.model_id(),
                v.build_secs
            );
            Some(v)
        }
        None => None,
    };
    let retriever = make_retriever(mode, &store, vector.as_ref(), penguin.as_ref())?;
    let queries = synthetic_queries(&corpus, args.get("category"));
    let reps = args.num("reps", 5) as usize;
    let (results, times) = eval::evaluate(retriever.as_ref(), &queries, reps)?;
    let name = args
        .get("name")
        .map(String::from)
        .unwrap_or_else(|| match &emb_name {
            Some(e) => format!("{mode}-{}", e.rsplit('/').next().unwrap_or(e).trim_end_matches(".json")),
            None => mode.to_string(),
        });
    let meta = RunMeta {
        name: name.clone(),
        mode: mode.into(),
        embedder: emb_name,
        corpus: format!("synthetic v1, seed {seed}, {}", corpus_day(&corpus)),
        corpus_seed: Some(seed),
        corpus_fingerprint: Some(corpus.fingerprint()),
        corpus_day: Some(corpus_day(&corpus)),
        corpus_messages: corpus.messages.len(),
        corpus_threads: corpus.threads.len(),
        created: chrono::Local::now().to_rfc3339(),
        git: git_rev(),
        host: host(),
        reps,
        depth: DEPTH,
    };
    let file = eval::build_file(meta, results, &times);
    if args.get("verbose").is_some() {
        for q in &file.queries {
            println!(
                "{:<28} {:<11} nDCG {:.2} RR {:.2} R@10 {:.2} {:>3} res  {}",
                q.id, q.category, q.ndcg10, q.rr, q.r10, q.results, q.text
            );
        }
    }
    println!("{}", eval::report(&file));
    let out = PathBuf::from(
        args.get("out")
            .map(String::from)
            .unwrap_or_else(|| format!("{}/{name}.json", dir.display())),
    );
    write_json(&out, &file)?;
    eprintln!("results: {}", out.display());
    if let Some(t) = args.get("trec") {
        write_trec(Path::new(t), &queries, retriever.as_ref(), &name)?;
    }
    Ok(())
}

fn corpus_day(c: &Corpus) -> String {
    penguin_eval::corpus::fmt_local(c.now, "%Y-%m-%d")
}

fn write_json<T: serde::Serialize>(path: &Path, f: &T) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_string_pretty(f).unwrap() + "\n").map_err(|e| e.to_string())
}

/// TREC formats, to check the metrics independently with trec_eval:
///   trec_eval -c -l 2 -m ndcg_cut.10 -m recip_rank qrels.txt run.txt
/// (trec_eval's recall is not capped at min(k, R); nDCG@10 and RR match.)
fn write_trec(
    dir: &Path,
    queries: &[EvalQuery],
    r: &dyn Retriever,
    name: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut qrels = String::new();
    let mut run = String::new();
    for q in queries {
        for (d, g) in &q.qrels {
            qrels.push_str(&format!("{} 0 {d} {g}\n", q.id));
        }
        for (i, d) in r.search(&q.text, DEPTH)?.iter().enumerate() {
            run.push_str(&format!("{} Q0 {d} {} {} {name}\n", q.id, i + 1, DEPTH - i));
        }
    }
    std::fs::write(dir.join("qrels.txt"), qrels).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("run.txt"), run).map_err(|e| e.to_string())?;
    eprintln!("trec: {}/qrels.txt and run.txt", dir.display());
    Ok(())
}

fn read_run(p: &str) -> Result<RunFile, String> {
    let s = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("{p}: {e}"))
}

fn diff(args: &Args) -> Result<(), String> {
    let (a, b) = match (args.pos.get(1), args.pos.get(2)) {
        (Some(a), Some(b)) => (read_run(a)?, read_run(b)?),
        _ => return Err("usage: search-eval diff BEFORE.json AFTER.json [--top 10]".into()),
    };
    println!("{}", eval::diff(&a, &b, args.num("top", 10) as usize));
    Ok(())
}

fn check(args: &Args) -> Result<(), String> {
    let seed = args.num("seed", 42);
    let corpus = Corpus::generate(seed, now_ms());
    let specs = all_queries(corpus.now);
    let qrels = judge::qrels(&corpus, &specs);
    let problems = judge::validate(&corpus, &specs);
    let mut kinds: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let per_thread: HashMap<(&str, &str), usize> =
        corpus.messages.iter().fold(HashMap::new(), |mut m, x| {
            *m.entry((x.account_id.as_str(), x.thread_id.as_str()))
                .or_default() += 1;
            m
        });
    for t in &corpus.threads {
        let k = t
            .tags
            .iter()
            .find_map(|x| x.strip_prefix("kind:"))
            .unwrap_or("?")
            .to_string();
        let e = kinds.entry(k).or_default();
        e.0 += 1;
        e.1 += per_thread[&(t.account_id.as_str(), t.thread_id.as_str())];
    }
    println!(
        "Corpus seed {seed} ({}): {} messages, {} conversations, fingerprint {}\n",
        corpus_day(&corpus),
        corpus.messages.len(),
        corpus.threads.len(),
        corpus.fingerprint()
    );
    println!("| kind | conversations | messages |\n|---|---:|---:|");
    for (k, (t, m)) in &kinds {
        println!("| {k} | {t} | {m} |");
    }
    let es = corpus.threads.iter().filter(|t| t.has("lang:es")).count();
    let ld = corpus
        .threads
        .iter()
        .filter(|t| t.tags.iter().any(|x| x.starts_with("jsonld:")))
        .count();
    let att = corpus
        .messages
        .iter()
        .filter(|m| !m.attachments.is_empty())
        .count();
    let accts: BTreeMap<&str, usize> = corpus.messages.iter().fold(BTreeMap::new(), |mut m, x| {
        *m.entry(x.account_id.as_str()).or_default() += 1;
        m
    });
    println!("\nSpanish conversations: {es}. With schema.org JSON-LD: {ld}. Messages with attachments: {att}.");
    println!("Messages per account: {accts:?}\n");
    println!(
        "| category | queries | mean relevant (≥2) | mean judged (≥1) |\n|---|---:|---:|---:|"
    );
    for c in Category::ALL {
        let qs: Vec<_> = specs.iter().filter(|q| q.category == c).collect();
        let rel: usize = qs
            .iter()
            .map(|q| qrels[q.id].values().filter(|g| **g >= 2).count())
            .sum();
        let jud: usize = qs.iter().map(|q| qrels[q.id].len()).sum();
        println!(
            "| {} | {} | {:.1} | {:.1} |",
            c.name(),
            qs.len(),
            rel as f64 / qs.len().max(1) as f64,
            jud as f64 / qs.len().max(1) as f64
        );
    }
    println!("| **all** | {} | | |", specs.len());
    if problems.is_empty() {
        println!("\nQuery set OK.");
        Ok(())
    } else {
        Err(format!(
            "\n{} problem(s):\n{}",
            problems.len(),
            problems.join("\n")
        ))
    }
}

fn show(args: &Args) -> Result<(), String> {
    let id = args
        .pos
        .get(1)
        .ok_or("usage: search-eval show QUERY_ID [--mode keyword]")?;
    let seed = args.num("seed", 42);
    let dir = PathBuf::from(args.get("dir").unwrap_or("target/search-eval"));
    let (corpus, store) = build_store(seed, &dir, true)?;
    let q = synthetic_queries(&corpus, None)
        .into_iter()
        .find(|q| &q.id == id)
        .ok_or(format!("no query {id}"))?;
    let mode = args.get("mode").unwrap_or("keyword");
    let vector = if matches!(mode, "vector" | "hybrid") {
        Some(Vector::build(
            retrieve::embedder_by_name(args.get("embedder").unwrap_or("hash"))?,
            &corpus.messages,
        )?)
    } else {
        None
    };
    let penguin = penguin_index(mode, Some(args.get("embedder").unwrap_or("hash")), &corpus)?;
    let r = make_retriever(mode, &store, vector.as_ref(), penguin.as_ref())?;
    let ranked = r.search(&q.text, 20)?;
    let subject: HashMap<String, &str> = corpus
        .threads
        .iter()
        .map(|t| (t.doc_id(), t.subject.as_str()))
        .collect();
    println!("{} [{}] {}", q.id, q.category, q.text);
    if mode == "penguin" {
        let rw = store
            .hybrid_rewrite(&q.text, now_ms())
            .map_err(|e| e.to_string())?;
        println!(
            "Rewritten: keyword side {:?}, embedded {:?}, soft date {:?}, respelled {:?}",
            rw.raw, rw.embed_raw, rw.soft_date, rw.respelled
        );
    }
    println!("\nJudged relevant:");
    let mut rel: Vec<_> = q.qrels.iter().collect();
    rel.sort_by(|a, b| b.1.cmp(a.1));
    for (d, g) in rel.iter().take(15) {
        println!("  {g}  {d}  {}", subject.get(*d).unwrap_or(&""));
    }
    if rel.len() > 15 {
        println!("  … {} more", rel.len() - 15);
    }
    println!("\nTop 20 ({mode}):");
    for (i, d) in ranked.iter().enumerate() {
        println!(
            "{:>3}  {}  {d}  {}",
            i + 1,
            q.qrels.get(d).copied().unwrap_or(0),
            subject.get(d).unwrap_or(&"")
        );
    }
    Ok(())
}

fn real_mode(args: &Args) -> Result<(), String> {
    let db = match args.get("db") {
        Some(p) => PathBuf::from(p),
        None => real::default_db().ok_or("can't find the Penguin database; pass --db PATH")?,
    };
    let private = PathBuf::from(args.get("private").unwrap_or("eval-private"));
    let queries_path = args
        .get("queries")
        .map(PathBuf::from)
        .unwrap_or_else(|| private.join("queries.txt"));
    let judgments_path = private.join("judgments.json");
    let mode = args.get("mode").unwrap_or("keyword");
    if !matches!(mode, "keyword" | "ask") {
        return Err(
            "real-mail mode runs keyword or ask (vector modes need the app's own index)".into(),
        );
    }
    let store = Store::open_read_only(&db).map_err(|e| e.to_string())?;
    eprintln!("opened {} read-only", db.display());
    let queries = real::load_queries(&queries_path)?;
    let mut judgments = real::load_judgments(&judgments_path);
    let eval_queries = real::prepare(
        &store,
        mode,
        &queries,
        &mut judgments,
        args.get("no-judge").is_none()
            && (args.get("judge").is_some() || std::io::stdin().is_terminal()),
        &judgments_path,
    )?;
    real::save_judgments(&judgments_path, &judgments).map_err(|e| e.to_string())?;
    // Score only queries with at least one relevant judgment (as trec_eval
    // skips queries with no relevant documents).
    let judged: Vec<EvalQuery> = eval_queries
        .into_iter()
        .filter(|q| q.qrels.values().any(|g| *g >= 2))
        .collect();
    let r = make_retriever(mode, &store, None, None)?;
    let reps = args.num("reps", 3) as usize;
    let (results, times) = eval::evaluate(r.as_ref(), &judged, reps)?;
    let name = args.get("name").unwrap_or(mode).to_string();
    let meta = RunMeta {
        name: name.clone(),
        mode: mode.into(),
        embedder: None,
        corpus: "your mailbox (read-only)".into(),
        corpus_seed: None,
        corpus_fingerprint: None,
        corpus_day: None,
        corpus_messages: store
            .search(&penguin_core::SearchRequest {
                query: String::new(),
                account_id: None,
                account_ids: None,
                limit: 1,
            })
            .map(|r| r.indexed_messages as usize)
            .unwrap_or(0),
        corpus_threads: 0,
        created: chrono::Local::now().to_rfc3339(),
        git: git_rev(),
        host: host(),
        reps,
        depth: DEPTH,
    };
    let file = eval::build_file(meta, results, &times);
    println!("{}", eval::report(&file));
    let low: Vec<_> = file.queries.iter().filter(|q| q.judged10 < 1.0).collect();
    if !low.is_empty() {
        println!("{} queries have unjudged results in their top 10 (counted as not relevant); run again without --no-judge to grade them.", low.len());
    }
    println!(
        "{} of {} queries have a relevant judgment and are scored.",
        file.queries.len(),
        queries.len()
    );
    let out = private.join("runs").join(format!("{name}.json"));
    write_json(&out, &file)?;
    eprintln!("results: {} (private; never commit)", out.display());
    Ok(())
}

/// Penguin's own vector index (product chunking) for `--mode penguin`.
fn penguin_index(
    mode: &str,
    embedder: Option<&str>,
    corpus: &Corpus,
) -> Result<Option<SemanticHandles>, String> {
    if mode != "penguin" {
        return Ok(None);
    }
    let name = embedder.unwrap_or("hash");
    let (h, secs) = Penguin::index(retrieve::embedder_by_name(name)?, &corpus.messages)?;
    eprintln!(
        "penguin index: {} chunks with {} in {secs:.1}s",
        penguin_semantic::VectorIndex::len(h.index.as_ref()),
        h.embedder.model_id()
    );
    Ok(Some(h))
}

/// Every text `--mode penguin` embeds: the product chunks of each message
/// and the meaning text of each query. Feed it to
/// crates/penguin-core/examples/ranking/embed.py, then `--embedder file:…`.
fn texts(args: &Args) -> Result<(), String> {
    let seed = args.num("seed", 42);
    let dir = PathBuf::from(args.get("dir").unwrap_or("target/search-eval"));
    // The store, because search rewrites queries against its index
    // (misspellings) before embedding them.
    let (corpus, store) = build_store(seed, &dir, false)?;
    let mut passages: Vec<String> = corpus
        .messages
        .iter()
        .flat_map(retrieve::product_chunks)
        .collect();
    passages.sort();
    passages.dedup();
    let now = now_ms();
    let mut queries: Vec<String> = all_queries(corpus.now)
        .iter()
        .filter_map(|q| {
            let rw = store.hybrid_rewrite(&q.text, now).ok()?;
            let q = penguin_core::query::parse(&rw.embed_raw, now);
            penguin_core::hybrid::semantic_text(&q, &rw.embed_raw)
        })
        .collect();
    queries.sort();
    queries.dedup();
    let out = PathBuf::from(args.get("out").unwrap_or("target/search-eval/texts.json"));
    write_json(&out, &serde_json::json!({ "passages": passages, "queries": queries }))?;
    eprintln!("{} passages, {} queries: {}", passages.len(), queries.len(), out.display());
    Ok(())
}
