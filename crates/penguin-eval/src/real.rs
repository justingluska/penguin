//! Real-mail mode: the same metrics over your own Penguin database, opened
//! read-only, with queries you write and judgments you make on your machine.
//!
//! Nothing here is committed: the queries file, the judgments and the run
//! files live in a private directory (default `eval-private/`, gitignored)
//! because they contain your subjects and senders.
//!
//! Queries file, one per line (`#` starts a comment):
//!
//! ```text
//! # category | query
//! known-item | lease pdf from mike
//! paraphrase | plane tickets to portugal
//! question | when does my flight leave?
//! ```

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use penguin_core::ask::AskScope;
use penguin_core::{SearchRequest, Store};
use serde::{Deserialize, Serialize};

use crate::corpus::doc_id;
use crate::eval::{EvalQuery, DEPTH};

/// Where the app keeps its database: `$PENGUIN_DATA_DIR/penguin.db`, else
/// the platform data dir + the app identifier (same rule as
/// `apps/desktop/src-tauri/src/ops.rs`): on macOS
/// `~/Library/Application Support/co.gluska.penguin/penguin.db`.
pub fn default_db() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("PENGUIN_DATA_DIR") {
        return Some(PathBuf::from(root).join("penguin.db"));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(x)
    } else {
        home.join(".local/share")
    };
    Some(data.join("co.gluska.penguin").join("penguin.db"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Judgment {
    pub grade: u8,
    /// For your own review later; stays on this machine.
    pub subject: String,
    pub from: String,
}

/// query text → doc id → judgment.
pub type Judgments = BTreeMap<String, BTreeMap<String, Judgment>>;

pub fn load_judgments(path: &Path) -> Judgments {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_judgments(path: &Path, j: &Judgments) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(j).unwrap())?;
    std::fs::rename(tmp, path)
}

pub struct RealQuery {
    pub category: String,
    pub text: String,
    /// Optional second query that finds the target another way (a code, an
    /// exact subject), used only to put it in front of you for judging.
    pub helper: Option<String>,
}

pub fn load_queries(path: &Path) -> Result<Vec<RealQuery>, String> {
    let s = std::fs::read_to_string(path).map_err(|e| {
        format!("can't read {}: {e}\nCreate it with lines like `known-item | lease pdf from mike` (see docs/SEARCH-EVAL.md).", path.display())
    })?;
    Ok(s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let parts: Vec<&str> = l.split('|').map(str::trim).collect();
            match parts.as_slice() {
                [q] => RealQuery {
                    category: "uncategorized".into(),
                    text: q.to_string(),
                    helper: None,
                },
                [c, q] => RealQuery {
                    category: c.to_lowercase(),
                    text: q.to_string(),
                    helper: None,
                },
                [c, q, h, ..] => RealQuery {
                    category: c.to_lowercase(),
                    text: q.to_string(),
                    helper: (!h.is_empty()).then(|| h.to_string()),
                },
                [] => unreachable!(),
            }
        })
        .collect())
}

/// One result shown for judging.
pub struct Shown {
    pub doc: String,
    pub subject: String,
    pub from: String,
    pub date: i64,
    pub snippet: String,
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// The top results with what a person needs to judge them.
pub fn top(store: &Store, mode: &str, query: &str, k: usize) -> Result<Vec<Shown>, String> {
    if mode == "ask" {
        let now = chrono::Utc::now().timestamp_millis();
        let offset = chrono::Local::now().offset().local_minus_utc();
        let a = store
            .ask(query, &AskScope::default(), now, offset)
            .map_err(|e| e.to_string())?;
        let mut seen = std::collections::HashSet::new();
        return Ok(a
            .items
            .into_iter()
            .filter(|i| seen.insert(doc_id(&i.account_id, &i.thread_id)))
            .take(k)
            .map(|i| Shown {
                doc: doc_id(&i.account_id, &i.thread_id),
                subject: i.subject,
                from: i.from.name.unwrap_or(i.from.email),
                date: i.date,
                snippet: i.snippet,
            })
            .collect());
    }
    let r = store
        .search(&SearchRequest {
            query: query.into(),
            account_id: None,
            account_ids: None,
            limit: k as u32,
        })
        .map_err(|e| e.to_string())?;
    Ok(r.hits
        .into_iter()
        .map(|h| Shown {
            doc: doc_id(&h.account_id, &h.thread_id),
            subject: h.subject,
            from: h.from.name.unwrap_or(h.from.email),
            date: h.date,
            snippet: strip_tags(&h.snippet_html),
        })
        .collect())
}

/// The judging pool (TREC-style pooling): the union of the top `k` from
/// keyword search and Ask for the query, plus the helper query's top `k`,
/// interleaved so every source's best results come first.
pub fn pool(store: &Store, q: &RealQuery, k: usize) -> Result<Vec<Shown>, String> {
    let mut lists = vec![
        top(store, "keyword", &q.text, k)?,
        top(store, "ask", &q.text, k)?,
    ];
    if let Some(h) = &q.helper {
        lists.push(top(store, "keyword", h, k)?);
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut iters: Vec<_> = lists.into_iter().map(|l| l.into_iter()).collect();
    loop {
        let mut any = false;
        for it in iters.iter_mut() {
            if let Some(s) = it.next() {
                any = true;
                if seen.insert(s.doc.clone()) {
                    out.push(s);
                }
            }
        }
        if !any {
            return Ok(out);
        }
    }
}

/// Ask for grades for the unjudged results among `shown`. Returns false when
/// the person wants to stop judging.
pub fn judge_interactively(
    query: &str,
    shown: &[Shown],
    judged: &mut BTreeMap<String, Judgment>,
) -> bool {
    let todo: Vec<(usize, &Shown)> = shown
        .iter()
        .enumerate()
        .filter(|(_, s)| !judged.contains_key(&s.doc))
        .collect();
    if todo.is_empty() {
        return true;
    }
    println!("\n── {query}");
    for (i, s) in shown.iter().enumerate() {
        let date = crate::corpus::fmt_local(s.date, "%Y-%m-%d");
        let mark = judged
            .get(&s.doc)
            .map_or("·".to_string(), |j| j.grade.to_string());
        let snip: String = s.snippet.chars().take(90).collect();
        println!(
            "{:>3} [{mark}] {date}  {:<24.24}  {}\n          {}",
            i + 1,
            s.from,
            s.subject,
            snip
        );
    }
    println!("Grade the unjudged results ([·]) in order, 0–3 separated by spaces: 3 = the one, 2 = highly relevant, 1 = related, 0 = not relevant.");
    println!("Use - to skip one, Enter to skip this query, q to stop.");
    print!("> ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    let line = line.trim();
    if line == "q" {
        return false;
    }
    for ((_, s), g) in todo.iter().zip(line.split_whitespace()) {
        if let Ok(g @ 0..=3) = g.parse::<u8>() {
            judged.insert(
                s.doc.clone(),
                Judgment {
                    grade: g,
                    subject: s.subject.clone(),
                    from: s.from.clone(),
                },
            );
        }
    }
    true
}

/// Show, judge (when `judge`: a terminal, or `--judge`) and turn into
/// evaluator queries.
pub fn prepare(
    store: &Store,
    mode: &str,
    queries: &[RealQuery],
    judgments: &mut Judgments,
    judge: bool,
    save: &Path,
) -> Result<Vec<EvalQuery>, String> {
    let mut judging = judge;
    let mut out = Vec::new();
    for (i, q) in queries.iter().enumerate() {
        let entry = judgments.entry(q.text.clone()).or_default();
        if judging {
            let shown = pool(store, q, 10)?;
            judging = judge_interactively(&q.text, &shown, entry);
            save_judgments(save, judgments).map_err(|e| e.to_string())?;
        } else if judgments.get(&q.text).is_none_or(|m| m.is_empty()) {
            // Unlabelled and not judging now: print the top 10 for a quick look.
            println!("\n── (unjudged) {}", q.text);
            for (n, s) in top(store, mode, &q.text, 10)?.iter().enumerate() {
                println!(
                    "{:>3} {}  {:<24.24}  {}",
                    n + 1,
                    crate::corpus::fmt_local(s.date, "%Y-%m-%d"),
                    s.from,
                    s.subject
                );
            }
        }
        let qrels = judgments
            .get(&q.text)
            .map(|m| m.iter().map(|(d, j)| (d.clone(), j.grade)).collect())
            .unwrap_or_default();
        out.push(EvalQuery {
            id: format!("r{:03}", i + 1),
            text: q.text.clone(),
            category: q.category.clone(),
            facets: Vec::new(),
            qrels,
        });
    }
    let _ = DEPTH;
    Ok(out)
}
