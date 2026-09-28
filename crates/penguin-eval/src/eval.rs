//! Running a query set through a retriever, the results file, the report
//! tables, and before/after diffs with significance tests.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::metrics::{self, mean, percentile};
use crate::retrieve::Retriever;

/// Documents retrieved per query (Recall@50 needs at least 50).
pub const DEPTH: usize = 100;

/// A query as the evaluator sees it (synthetic or real-mail).
pub struct EvalQuery {
    pub id: String,
    pub text: String,
    pub category: String,
    pub facets: Vec<String>,
    /// doc id → grade (0 = judged irrelevant).
    pub qrels: BTreeMap<String, u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub id: String,
    pub text: String,
    pub category: String,
    #[serde(default)]
    pub facets: Vec<String>,
    pub ndcg10: f64,
    pub rr: f64,
    pub r10: f64,
    pub r50: f64,
    /// Share of the top 10 with a judgment (always 1 on the synthetic corpus).
    pub judged10: f64,
    /// Number of relevant (grade ≥ 2) documents.
    pub relevant: usize,
    pub results: usize,
    /// Median latency over the repetitions, ms.
    pub ms: f64,
    /// Hash of this query's judgments, so a diff can spot queries whose
    /// judgments differ between the two runs.
    #[serde(default)]
    pub qrels_hash: String,
    /// Top 10 as (doc id, grade; -1 = unjudged).
    pub top10: Vec<(String, i8)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub queries: usize,
    pub ndcg10: f64,
    pub mrr: f64,
    pub r10: f64,
    pub r50: f64,
    /// Queries with no results at all.
    pub empty: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub name: String,
    pub mode: String,
    pub embedder: Option<String>,
    pub corpus: String,
    pub corpus_seed: Option<u64>,
    pub corpus_fingerprint: Option<String>,
    /// The local day the synthetic corpus was generated for.
    #[serde(default)]
    pub corpus_day: Option<String>,
    pub corpus_messages: usize,
    pub corpus_threads: usize,
    pub created: String,
    pub git: Option<String>,
    pub host: String,
    pub reps: usize,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunFile {
    pub schema: u32,
    pub meta: RunMeta,
    pub summary: Summary,
    pub by_category: BTreeMap<String, Summary>,
    pub by_facet: BTreeMap<String, Summary>,
    pub queries: Vec<QueryResult>,
}

/// Latencies (ms) per category, per `facet:<name>` and for "all".
pub type Timings = BTreeMap<String, Vec<f64>>;

/// Run every query `reps` times (after one warm-up) and score the ranking.
pub fn evaluate(
    r: &dyn Retriever,
    queries: &[EvalQuery],
    reps: usize,
) -> Result<(Vec<QueryResult>, Timings), String> {
    let mut out = Vec::new();
    let mut times: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for q in queries {
        let ranked = r.search(&q.text, DEPTH)?;
        let mut ts = Vec::new();
        for _ in 0..reps.max(1) {
            let t = Instant::now();
            let again = r.search(&q.text, DEPTH)?;
            ts.push(t.elapsed().as_secs_f64() * 1000.0);
            debug_assert_eq!(again, ranked, "nondeterministic ranking for {}", q.id);
        }
        ts.sort_by(f64::total_cmp);
        times
            .entry(q.category.clone())
            .or_default()
            .extend(ts.iter().copied());
        for f in &q.facets {
            times
                .entry(format!("facet:{f}"))
                .or_default()
                .extend(ts.iter().copied());
        }
        times
            .entry("all".into())
            .or_default()
            .extend(ts.iter().copied());
        // A query whose right answer is nothing (conflicting filters)
        // scores 1 for an empty ranking and 0 for any result.
        let expect_empty = q.facets.iter().any(|f| f == crate::judge::EXPECT_EMPTY);
        let empty_score = if ranked.is_empty() { 1.0 } else { 0.0 };
        let m = |x: f64| if expect_empty { empty_score } else { x };
        out.push(QueryResult {
            id: q.id.clone(),
            text: q.text.clone(),
            category: q.category.clone(),
            facets: q.facets.clone(),
            ndcg10: m(metrics::ndcg_at(&ranked, &q.qrels, 10)),
            rr: m(metrics::rr(&ranked, &q.qrels)),
            r10: m(metrics::recall_at(&ranked, &q.qrels, 10)),
            r50: m(metrics::recall_at(&ranked, &q.qrels, 50)),
            judged10: metrics::judged_at(&ranked, &q.qrels, 10),
            relevant: q.qrels.values().filter(|g| **g >= metrics::REL).count(),
            results: ranked.len(),
            qrels_hash: format!(
                "{:016x}",
                q.qrels
                    .iter()
                    .fold(0xcbf2_9ce4_8422_2325, |h, (d, g)| crate::corpus::fnv(
                        h,
                        &format!("{d}={g}")
                    ))
            ),
            ms: percentile(&ts, 0.5),
            top10: ranked
                .iter()
                .take(10)
                .map(|d| (d.clone(), q.qrels.get(d).map_or(-1, |g| *g as i8)))
                .collect(),
        });
    }
    Ok((out, times))
}

fn summarize(rs: &[&QueryResult], times: Option<&Vec<f64>>) -> Summary {
    let col = |f: fn(&QueryResult) -> f64| mean(&rs.iter().map(|r| f(r)).collect::<Vec<_>>());
    let mut ts = times.cloned().unwrap_or_default();
    ts.sort_by(f64::total_cmp);
    Summary {
        queries: rs.len(),
        ndcg10: col(|r| r.ndcg10),
        mrr: col(|r| r.rr),
        r10: col(|r| r.r10),
        r50: col(|r| r.r50),
        empty: rs.iter().filter(|r| r.results == 0).count(),
        p50_ms: percentile(&ts, 0.5),
        p95_ms: percentile(&ts, 0.95),
    }
}

pub fn build_file(
    meta: RunMeta,
    results: Vec<QueryResult>,
    times: &BTreeMap<String, Vec<f64>>,
) -> RunFile {
    let all: Vec<&QueryResult> = results.iter().collect();
    let mut by_category = BTreeMap::new();
    let mut cats: Vec<String> = results.iter().map(|r| r.category.clone()).collect();
    cats.dedup();
    cats.sort();
    cats.dedup();
    for c in cats {
        let rs: Vec<&QueryResult> = results.iter().filter(|r| r.category == c).collect();
        by_category.insert(c.clone(), summarize(&rs, times.get(&c)));
    }
    let mut by_facet = BTreeMap::new();
    let mut facets: Vec<String> = results
        .iter()
        .flat_map(|r| r.facets.iter().cloned())
        .collect();
    facets.sort();
    facets.dedup();
    for f in facets {
        let rs: Vec<&QueryResult> = results.iter().filter(|r| r.facets.contains(&f)).collect();
        by_facet.insert(f.clone(), summarize(&rs, times.get(&format!("facet:{f}"))));
    }
    RunFile {
        schema: 1,
        meta,
        summary: summarize(&all, times.get("all")),
        by_category,
        by_facet,
        queries: results,
    }
}

fn row(name: &str, s: &Summary) -> String {
    format!(
        "| {name} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {:.1} | {:.1} |",
        s.queries, s.ndcg10, s.mrr, s.r10, s.r50, s.empty, s.p50_ms, s.p95_ms
    )
}

/// Markdown report: per-category table, facets, worst queries.
pub fn report(f: &RunFile) -> String {
    let mut s = String::new();
    let m = &f.meta;
    s.push_str(&format!(
        "## {} ({}{})\n\nCorpus: {} ({} messages{}{}). {} queries, {} timed runs each, depth {}.\n\n",
        m.name,
        m.mode,
        m.embedder.as_deref().map(|e| format!(", embedder {e}")).unwrap_or_default(),
        m.corpus,
        m.corpus_messages,
        if m.corpus_threads > 0 { format!(", {} conversations", m.corpus_threads) } else { String::new() },
        m.corpus_fingerprint.as_deref().map(|x| format!(", fingerprint {x}")).unwrap_or_default(),
        f.summary.queries,
        m.reps,
        m.depth
    ));
    let head = "| category | n | nDCG@10 | MRR | R@10 | R@50 | empty | p50 ms | p95 ms |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n";
    s.push_str(head);
    for (c, sum) in &f.by_category {
        s.push_str(&row(c, sum));
        s.push('\n');
    }
    s.push_str(&row("**all**", &f.summary));
    s.push('\n');
    if !f.by_facet.is_empty() {
        s.push_str("\nFacets (a query can have several):\n\n");
        s.push_str(&head.replace("category", "facet"));
        for (c, sum) in &f.by_facet {
            s.push_str(&row(c, sum));
            s.push('\n');
        }
    }
    let mut worst: Vec<&QueryResult> = f.queries.iter().collect();
    worst.sort_by(|a, b| a.ndcg10.total_cmp(&b.ndcg10).then(a.id.cmp(&b.id)));
    let zero = worst.iter().filter(|q| q.ndcg10 == 0.0).count();
    s.push_str(&format!(
        "\n{zero} of {} queries score nDCG@10 = 0.\n",
        f.queries.len()
    ));
    s
}

/// Per-category comparison of two runs, with paired randomization tests.
pub fn diff(a: &RunFile, b: &RunFile, top: usize) -> String {
    let mut s = String::new();
    s.push_str(&format!("## {} → {}\n\n", a.meta.name, b.meta.name));
    if a.meta.corpus_fingerprint != b.meta.corpus_fingerprint {
        s.push_str(&format!(
            "**Warning:** different corpora ({:?} vs {:?}): a different seed or corpus generator. Rerun both sides with the same build of penguin-eval.\n\n",
            a.meta.corpus_fingerprint, b.meta.corpus_fingerprint
        ));
    }
    let bmap: BTreeMap<&str, &QueryResult> = b.queries.iter().map(|q| (q.id.as_str(), q)).collect();
    let pairs: Vec<(&QueryResult, &QueryResult)> = a
        .queries
        .iter()
        .filter_map(|q| bmap.get(q.id.as_str()).map(|r| (q, *r)))
        .collect();
    if a.meta.corpus_day != b.meta.corpus_day {
        s.push_str(&format!(
            "Note: corpora generated on different days ({:?} vs {:?}); calendar windows (\"last month\", \"last spring\") moved.\n\n",
            a.meta.corpus_day, b.meta.corpus_day
        ));
    }
    let changed: Vec<&str> = pairs
        .iter()
        .filter(|(x, y)| !x.qrels_hash.is_empty() && x.qrels_hash != y.qrels_hash)
        .map(|(x, _)| x.id.as_str())
        .collect();
    if !changed.is_empty() {
        s.push_str(&format!(
            "**{} queries have different judgments in the two runs** (compared anyway): {}\n\n",
            changed.len(),
            changed.join(", ")
        ));
    }
    let missing = a.queries.len() + b.queries.len() - 2 * pairs.len();
    if missing > 0 {
        s.push_str(&format!(
            "{missing} queries are in only one run and are left out.\n\n"
        ));
    }
    s.push_str("| category | n | nDCG@10 before → after (Δ) | p | MRR Δ | p | R@10 Δ | R@50 Δ | wins / losses | p50 ms Δ |\n|---|---:|---|---:|---:|---:|---:|---:|---|---:|\n");
    let mut cats: Vec<String> = pairs.iter().map(|(q, _)| q.category.clone()).collect();
    cats.sort();
    cats.dedup();
    cats.push("all".into());
    for c in &cats {
        let ps: Vec<&(&QueryResult, &QueryResult)> = pairs
            .iter()
            .filter(|(q, _)| c == "all" || &q.category == c)
            .collect();
        let col = |f: fn(&QueryResult) -> f64| -> (Vec<f64>, Vec<f64>) {
            (
                ps.iter().map(|(x, _)| f(x)).collect(),
                ps.iter().map(|(_, y)| f(y)).collect(),
            )
        };
        let (na, nb) = col(|r| r.ndcg10);
        let (ra, rb) = col(|r| r.rr);
        let (r10a, r10b) = col(|r| r.r10);
        let (r50a, r50b) = col(|r| r.r50);
        let wins = ps
            .iter()
            .filter(|(x, y)| y.ndcg10 > x.ndcg10 + 1e-9)
            .count();
        let losses = ps
            .iter()
            .filter(|(x, y)| y.ndcg10 + 1e-9 < x.ndcg10)
            .count();
        let (ta, tb) = (
            a.by_category
                .get(c)
                .map(|s| s.p50_ms)
                .unwrap_or(a.summary.p50_ms),
            b.by_category
                .get(c)
                .map(|s| s.p50_ms)
                .unwrap_or(b.summary.p50_ms),
        );
        let (ta, tb) = if c == "all" {
            (a.summary.p50_ms, b.summary.p50_ms)
        } else {
            (ta, tb)
        };
        let p_n = metrics::randomization_test(&na, &nb, 100_000, 1);
        let p_r = metrics::randomization_test(&ra, &rb, 100_000, 2);
        let name = if c == "all" {
            "**all**".to_string()
        } else {
            c.clone()
        };
        s.push_str(&format!(
            "| {name} | {} | {:.3} → {:.3} ({:+.3}){} | {} | {:+.3} | {} | {:+.3} | {:+.3} | {wins} / {losses} | {:+.1} |\n",
            ps.len(),
            mean(&na),
            mean(&nb),
            mean(&nb) - mean(&na),
            if p_n < 0.05 { " *" } else { "" },
            fmt_p(p_n),
            mean(&rb) - mean(&ra),
            fmt_p(p_r),
            mean(&r10b) - mean(&r10a),
            mean(&r50b) - mean(&r50a),
            tb - ta
        ));
    }
    s.push_str("\n`*` p < 0.05, two-sided paired randomization test on per-query scores (100,000 permutations). With many categories, expect about one in twenty to pass by chance.\n");
    let mut moved: Vec<(&QueryResult, &QueryResult)> = pairs
        .iter()
        .filter(|(x, y)| (y.ndcg10 - x.ndcg10).abs() > 1e-9)
        .copied()
        .collect();
    moved.sort_by(|(x1, y1), (x2, y2)| (y1.ndcg10 - x1.ndcg10).total_cmp(&(y2.ndcg10 - x2.ndcg10)));
    let losses: Vec<_> = moved
        .iter()
        .filter(|(x, y)| y.ndcg10 < x.ndcg10)
        .take(top)
        .collect();
    let wins: Vec<_> = moved
        .iter()
        .rev()
        .filter(|(x, y)| y.ndcg10 > x.ndcg10)
        .take(top)
        .collect();
    for (title, list) in [("Biggest wins", wins), ("Biggest losses", losses)] {
        if list.is_empty() {
            continue;
        }
        s.push_str(&format!(
            "\n### {title}\n\n| query | category | nDCG@10 | RR |\n|---|---|---|---|\n"
        ));
        for (x, y) in list {
            s.push_str(&format!(
                "| `{}` | {} | {:.2} → {:.2} | {:.2} → {:.2} |\n",
                x.text, x.category, x.ndcg10, y.ndcg10, x.rr, y.rr
            ));
        }
    }
    s
}

fn fmt_p(p: f64) -> String {
    if p < 0.001 {
        "<0.001".into()
    } else {
        format!("{p:.3}")
    }
}
