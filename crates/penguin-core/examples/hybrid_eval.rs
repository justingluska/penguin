//! Hybrid ranking evaluation: nDCG@10, MRR and recall@10 of keyword search
//! (today) against hybrid search, over a small labelled set of fictional
//! mail (examples/ranking/corpus.json, queries.json).
//!
//! Real embedding vectors come from files so the eval needs no model at run
//! time. To (re)make them after changing the corpus, queries or chunking:
//!
//!   cargo run -p penguin-core --example hybrid_eval -- dump > /tmp/texts.json
//!   uv run --python 3.12 --with fastembed python \
//!     crates/penguin-core/examples/ranking/embed.py /tmp/texts.json \
//!     crates/penguin-core/examples/ranking BAAI/bge-small-en-v1.5
//!
//! Then:
//!
//!   cargo run -p penguin-core --release --example hybrid_eval [-- --verbose]
//!
//! prints one table per vector file found (plus the hash stand-in).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use penguin_core::hybrid::{Fusion, SemNorm};
use penguin_core::{
    Account, Address, AttachmentMeta, HybridParams, Message, SearchRequest, SemanticHandles, Store,
};
use penguin_semantic::{
    chunk_message, ChunkRef, Embedder, FlatIndex, HashEmbedder, SemanticError, VectorIndex,
};
use serde::Deserialize;

const DAY: i64 = 86_400_000;

#[derive(Deserialize)]
struct Corpus {
    account: String,
    me: String,
    messages: Vec<Msg>,
}

#[derive(Deserialize)]
struct Msg {
    id: String,
    thread: String,
    days: i64,
    from: (String, String),
    #[serde(default)]
    to: Vec<(String, String)>,
    subject: String,
    body: String,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    starred: bool,
    #[serde(default)]
    list_unsubscribe: bool,
    #[serde(default)]
    attachments: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Queries {
    queries: Vec<Query>,
}

#[derive(Deserialize, Clone)]
struct Query {
    id: String,
    kind: String,
    q: String,
    rel: HashMap<String, u32>,
}

fn eval_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/ranking")
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn messages(c: &Corpus, now: i64) -> Vec<Message> {
    c.messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut labels = m.labels.clone().unwrap_or_else(|| vec!["INBOX".into()]);
            if m.unread {
                labels.push("UNREAD".into());
            }
            if m.starred {
                labels.push("STARRED".into());
            }
            let to = if m.to.is_empty() {
                vec![(c.me.clone(), c.account.clone())]
            } else {
                m.to.clone()
            };
            Message {
                account_id: c.account.clone(),
                id: m.id.clone(),
                thread_id: m.thread.clone(),
                // Distinct times within a day keep thread order stable.
                date: now - m.days * DAY + i as i64 * 1000,
                from: Address {
                    name: Some(m.from.0.clone()),
                    email: m.from.1.clone(),
                },
                to: to
                    .into_iter()
                    .map(|(n, e)| Address {
                        name: Some(n),
                        email: e,
                    })
                    .collect(),
                cc: vec![],
                bcc: vec![],
                reply_to: vec![],
                subject: m.subject.clone(),
                snippet: m.body.chars().take(120).collect(),
                body_text: m.body.clone(),
                body_html: None,
                label_ids: labels,
                attachments: m
                    .attachments
                    .iter()
                    .enumerate()
                    .map(|(k, (name, mime))| AttachmentMeta {
                        id: format!("{}-att{k}", m.id),
                        filename: name.clone(),
                        mime_type: mime.clone(),
                        size: 120_000,
                        content_id: None,
                        inline: false,
                    })
                    .collect(),
                message_id_header: Some(format!("<{}@eval.example>", m.id)),
                in_reply_to: None,
                references: vec![],
                list_unsubscribe: m
                    .list_unsubscribe
                    .then(|| "<mailto:unsubscribe@list.example>".to_string()),
                list_unsubscribe_post: None,
                sender_authenticated: true,
            }
        })
        .collect()
}

/// Chunks of each message, cut as the indexer cuts them.
fn chunks(m: &Message) -> Vec<String> {
    chunk_message(&m.subject, &penguin_core::text::strip_quoted(&m.body_text))
}

/// Vectors computed ahead of time by embed.py for exactly these texts.
struct Precomputed {
    model: String,
    dims: usize,
    passages: HashMap<String, Vec<f32>>,
    queries: HashMap<String, Vec<f32>>,
}

#[derive(Deserialize)]
struct VectorFile {
    model: String,
    dims: usize,
    passages: HashMap<String, String>,
    queries: HashMap<String, String>,
}

fn decode(hex: &str) -> Vec<f32> {
    // int8 per dimension (two's complement), as embed.py writes it.
    let mut v: Vec<f32> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap() as i8 as f32)
        .collect();
    penguin_semantic::normalize(&mut v);
    v
}

impl Precomputed {
    fn load(path: &Path) -> Precomputed {
        let f: VectorFile = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        Precomputed {
            model: f.model,
            dims: f.dims,
            passages: f.passages.iter().map(|(k, v)| (k.clone(), decode(v))).collect(),
            queries: f.queries.iter().map(|(k, v)| (k.clone(), decode(v))).collect(),
        }
    }
}

impl Embedder for Precomputed {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dims(&self) -> usize {
        self.dims
    }
    fn embed_query(&self, text: &str) -> penguin_semantic::Result<Vec<f32>> {
        self.queries.get(text).cloned().ok_or_else(|| {
            SemanticError::Embed(format!("no vector for query {text:?}: re-run dump + embed.py"))
        })
    }
    fn embed_passages(&self, texts: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
        texts
            .iter()
            .map(|t| {
                self.passages.get(*t).cloned().ok_or_else(|| {
                    SemanticError::Embed("no vector for a passage: re-run dump + embed.py".to_string())
                })
            })
            .collect()
    }
}

fn build(c: &Corpus, msgs: &[Message]) -> Store {
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_account(&Account {
            id: c.account.clone(),
            email: c.account.clone(),
            display_name: Some(c.me.clone()),
            nickname: None,
            color: "#0a84ff".into(),
            added_at: 1,
            ..Account::default()
        })
        .unwrap();
    store.upsert_messages(msgs).unwrap();
    store
}

fn index(msgs: &[Message], e: &dyn Embedder) -> Arc<FlatIndex> {
    let idx = Arc::new(FlatIndex::default());
    for m in msgs {
        let ch = chunks(m);
        let refs: Vec<&str> = ch.iter().map(String::as_str).collect();
        for (i, v) in e.embed_passages(&refs).unwrap().into_iter().enumerate() {
            idx.upsert(
                ChunkRef {
                    account_id: m.account_id.clone(),
                    thread_id: m.thread_id.clone(),
                    message_id: m.id.clone(),
                    chunk: i as u32,
                    date: m.date,
                },
                v,
            )
            .unwrap();
        }
    }
    idx
}

/// Per-query metrics.
#[derive(Clone, Copy, Default)]
struct Scores {
    ndcg: f64,
    mrr: f64,
    recall: f64,
}

fn score(ranked: &[String], rel: &HashMap<String, u32>) -> Scores {
    let gain = |g: u32| (2f64.powi(g as i32)) - 1.0;
    let disc = |i: usize| 1.0 / ((i + 2) as f64).log2();
    let dcg: f64 = ranked
        .iter()
        .take(10)
        .enumerate()
        .map(|(i, t)| gain(*rel.get(t).unwrap_or(&0)) * disc(i))
        .sum();
    let mut ideal: Vec<u32> = rel.values().copied().collect();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    let idcg: f64 = ideal.iter().take(10).enumerate().map(|(i, &g)| gain(g) * disc(i)).sum();
    let first = ranked.iter().take(10).position(|t| rel.get(t).is_some_and(|&g| g > 0));
    let found = ranked
        .iter()
        .take(10)
        .filter(|t| rel.get(*t).is_some_and(|&g| g > 0))
        .count();
    let wanted = rel.values().filter(|&&g| g > 0).count();
    Scores {
        ndcg: if idcg > 0.0 { dcg / idcg } else { 0.0 },
        mrr: first.map_or(0.0, |i| 1.0 / (i + 1) as f64),
        recall: found as f64 / wanted.max(1) as f64,
    }
}

fn run(store: &Store, q: &str, sem: Option<&SemanticHandles>, p: &HybridParams) -> Vec<String> {
    let r = store
        .search_hybrid_with(
            &SearchRequest {
                query: q.into(),
                account_id: None,
                account_ids: None,
                limit: 50,
            },
            sem,
            p,
        )
        .unwrap();
    r.hits.into_iter().map(|h| h.thread_id).collect()
}

const KINDS: [&str; 4] = ["keyword", "words", "meaning", "filtered"];

fn table_row(name: &str, qs: &[Query], per: &[Scores]) -> String {
    let mean = |f: &dyn Fn(&Scores) -> f64, kind: Option<&str>| {
        let v: Vec<f64> = qs
            .iter()
            .zip(per)
            .filter(|(q, _)| kind.is_none_or(|k| q.kind == k))
            .map(|(_, s)| f(s))
            .collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    let mut row = format!(
        "| {name} | {:.3} | {:.3} | {:.3} |",
        mean(&|s| s.ndcg, None),
        mean(&|s| s.mrr, None),
        mean(&|s| s.recall, None)
    );
    for k in KINDS {
        row.push_str(&format!(" {:.3} |", mean(&|s| s.ndcg, Some(k))));
    }
    row
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let corpus: Corpus =
        serde_json::from_slice(&std::fs::read(eval_dir().join("corpus.json")).unwrap()).unwrap();
    let queries: Queries =
        serde_json::from_slice(&std::fs::read(eval_dir().join("queries.json")).unwrap()).unwrap();
    let qs = queries.queries;
    let now = now();
    let msgs = messages(&corpus, now);

    let store = build(&corpus, &msgs);
    if args.first().map(String::as_str) == Some("dump") {
        let mut passages: Vec<String> = msgs.iter().flat_map(chunks).collect();
        passages.sort();
        passages.dedup();
        let mut texts: Vec<String> = qs
            .iter()
            .filter_map(|q| {
                // Embedded after the same rewrite search applies.
                let rw = store.hybrid_rewrite(&q.q, now).ok()?;
                let q = penguin_core::query::parse(&rw.embed_raw, now);
                penguin_core::hybrid::semantic_text(&q, &rw.embed_raw)
            })
            .collect();
        texts.sort();
        texts.dedup();
        println!(
            "{}",
            serde_json::json!({ "passages": passages, "queries": texts })
        );
        return;
    }
    let verbose = args.iter().any(|a| a == "--verbose");

    let mut models: Vec<Arc<dyn Embedder>> = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(eval_dir())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("vectors-") && n.ends_with(".json"))
        })
        .collect();
    files.sort();
    for f in &files {
        models.push(Arc::new(Precomputed::load(f)));
    }
    models.push(Arc::new(HashEmbedder::new(384)));

    let base = HybridParams::default();
    let off = |p: &HybridParams| HybridParams {
        participated: 0.0,
        important: 0.0,
        newsletter_penalty: 0.0,
        code_boost: 0.0,
        subject_boost: 0.0,
        sender_boost: 0.0,
        ..p.clone()
    };
    let fixed = |a: f64| HybridParams {
        alpha_keyword: a,
        alpha_balanced: a,
        alpha_semantic: a,
        ..base.clone()
    };
    let configs: Vec<(&str, HybridParams)> = vec![
        ("hybrid (default: convex, min-max, routed α)", base.clone()),
        (
            "convex, theoretical min-max (TM2C2)",
            HybridParams {
                fusion: Fusion::Convex(SemNorm::TheoreticalMinMax),
                ..base.clone()
            },
        ),
        (
            "weighted RRF, k=60",
            HybridParams {
                fusion: Fusion::Rrf { k: 60.0 },
                ..base.clone()
            },
        ),
        ("fixed α=0.5, no routing", fixed(0.5)),
        ("fixed α=0.7, no routing", fixed(0.7)),
        ("no thread-level signals", off(&base)),
        (
            "long queries stay meaning-led when words match",
            HybridParams {
                keyword_found_balances: false,
                ..base.clone()
            },
        ),
        (
            "never skip meaning (keyword_enough=0)",
            HybridParams {
                keyword_enough: 0,
                ..base.clone()
            },
        ),
    ];

    let baseline: Vec<(Vec<String>, Scores)> = qs
        .iter()
        .map(|q| {
            let r = run(&store, &q.q, None, &base);
            let s = score(&r, &q.rel);
            (r, s)
        })
        .collect();
    let base_scores: Vec<Scores> = baseline.iter().map(|b| b.1).collect();

    println!(
        "{} queries ({}), {} messages\n",
        qs.len(),
        KINDS
            .iter()
            .map(|k| format!("{} {k}", qs.iter().filter(|q| q.kind == *k).count()))
            .collect::<Vec<_>>()
            .join(", "),
        msgs.len()
    );
    for model in &models {
        let idx = index(&msgs, model.as_ref());
        let handles = SemanticHandles {
            embedder: model.clone(),
            index: idx,
            progress: None,
        };
        println!("## {} ({} dims)\n", model.model_id(), model.dims());
        println!("| ranking | nDCG@10 | MRR@10 | R@10 | nDCG keyword | nDCG words | nDCG meaning | nDCG filtered |");
        println!("|---|---:|---:|---:|---:|---:|---:|---:|");
        println!("{}", table_row("keyword only (today)", &qs, &base_scores));
        for (name, p) in &configs {
            let per: Vec<Scores> = qs
                .iter()
                .map(|q| score(&run(&store, &q.q, Some(&handles), p), &q.rel))
                .collect();
            println!("{}", table_row(name, &qs, &per));
            if verbose && *name == configs[0].0 {
                let mut lines = Vec::new();
                for ((q, s), b) in qs.iter().zip(&per).zip(&base_scores) {
                    if (s.ndcg - b.ndcg).abs() > 1e-9 {
                        let ranked = run(&store, &q.q, Some(&handles), p);
                        lines.push(format!(
                            "  {:4} {:48} nDCG {:.2} -> {:.2}   top3 {:?}",
                            q.id,
                            q.q,
                            b.ndcg,
                            s.ndcg,
                            ranked.iter().take(3).collect::<Vec<_>>()
                        ));
                    }
                }
                println!("\n  changed queries (default hybrid vs keyword):");
                for l in lines {
                    println!("{l}");
                }
                println!();
            }
        }
        if args.iter().any(|a| a == "--sweep") {
            let mean_ndcg = |p: &HybridParams| {
                qs.iter()
                    .map(|q| score(&run(&store, &q.q, Some(&handles), p), &q.rel).ndcg)
                    .sum::<f64>()
                    / qs.len() as f64
            };
            println!("Sensitivity (mean nDCG@10), one parameter at a time around the default:\n");
            let line = |name: &str, vals: &[f64], f: &dyn Fn(f64) -> HybridParams| {
                let cells: Vec<String> = vals
                    .iter()
                    .map(|&v| format!("{v}: {:.3}", mean_ndcg(&f(v))))
                    .collect();
                println!("- {name}: {}", cells.join(", "));
            };
            line("alpha_keyword", &[0.0, 0.1, 0.25, 0.4, 0.5], &|v| HybridParams {
                alpha_keyword: v,
                ..base.clone()
            });
            line("alpha_balanced", &[0.3, 0.4, 0.5, 0.6, 0.7], &|v| HybridParams {
                alpha_balanced: v,
                ..base.clone()
            });
            line("alpha_semantic", &[0.5, 0.6, 0.7, 0.8, 0.9], &|v| HybridParams {
                alpha_semantic: v,
                ..base.clone()
            });
            line("newsletter_penalty", &[0.0, 0.05, 0.1, 0.15, 0.25], &|v| HybridParams {
                newsletter_penalty: v,
                ..base.clone()
            });
            line("participated", &[0.0, 0.05, 0.1], &|v| HybridParams {
                participated: v,
                ..base.clone()
            });
            line("sem_floor", &[0.0, 0.1, 0.2, 0.3, 0.5], &|v| HybridParams {
                sem_floor: v,
                ..base.clone()
            });
            line("RRF k (fusion = weighted RRF)", &[5.0, 20.0, 60.0], &|v| HybridParams {
                fusion: Fusion::Rrf { k: v },
                ..base.clone()
            });
            println!();
        }
        println!();
    }
    // Queries whose answer never ranks in the top 10 by keywords.
    let missed: HashSet<&str> = qs
        .iter()
        .zip(&base_scores)
        .filter(|(_, s)| s.recall == 0.0)
        .map(|(q, _)| q.id.as_str())
        .collect();
    let mut missed: Vec<&str> = missed.into_iter().collect();
    missed.sort();
    println!("keyword search finds nothing relevant for {} of {} queries: {}", missed.len(), qs.len(), missed.join(" "));
}
