//! `ask-eval`: Ask's exact answers on real question shapes.
//!
//!   ask-eval [--out FILE] [--parser] [--set tuned|heldout] [--category C] [--id ID] [--verbose] [--reps N]
//!
//! The questions (`questions.tsv`) are real questions people ask their
//! mail, from the sources in `sources.tsv`, mapped onto the synthetic
//! mailbox's people, merchants and cities. Each has a gold query; the
//! expected answer (a number, amount, date, code, set of conversations,
//! winner, yes/no, or the conversation a topic question needs) is computed
//! from the corpus itself (`oracle.rs`), not from Penguin.
//!
//! Two sets: `questions.tsv` (318, used while building the grammar and the
//! executor) and `heldout.tsv` (62, written after that and never used to
//! change them). Each gets its own table.
//!
//! The mailbox is generated for a fixed day (2026-09-27, noon local), so
//! "in August" and "last year" mean the same thing on every run.
//!
//! `--parser` also runs, for each question the grammar doesn't answer
//! exactly, the question's gold query through `Store::ask_draft`: what the
//! on-device model's parse would give if it read the question perfectly.
//! It tests the plumbing and bounds what the model can add; it is not the
//! model. See docs/ASK-QUESTIONS.md.

mod draft;
mod oracle;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{Local, NaiveDate, TimeZone};
use penguin_core::ask::AskScope;
use penguin_core::Store;
use penguin_eval::corpus::{doc_id, Corpus};
use serde_json::{json, Value};

use oracle::{expect, parse_gold, Expect, Expected, Gold, Op, Truth};

const QUESTIONS: &str = include_str!("questions.tsv");
const HELDOUT: &str = include_str!("heldout.tsv");

pub struct Question {
    pub set: &'static str,
    pub id: String,
    pub src: String,
    pub category: String,
    pub text: String,
    pub gold: String,
}

fn questions() -> Vec<Question> {
    [("tuned", QUESTIONS), ("heldout", HELDOUT)]
        .into_iter()
        .flat_map(|(set, text)| text.lines().map(move |l| (set, l)))
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|(set, l)| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 5, "bad line: {l}");
            Question {
                set,
                id: f[0].into(),
                src: f[1].into(),
                category: f[2].into(),
                text: f[3].into(),
                gold: f[4].into(),
            }
        })
        .collect()
}

struct Args {
    set: Option<String>,
    out: Option<String>,
    parser: bool,
    category: Option<String>,
    id: Option<String>,
    verbose: bool,
    reps: usize,
}

fn args() -> Args {
    let v: Vec<String> = std::env::args().skip(1).collect();
    let get = |k: &str| v.iter().position(|a| a == k).and_then(|i| v.get(i + 1)).cloned();
    Args {
        set: get("--set"),
        out: get("--out"),
        parser: v.iter().any(|a| a == "--parser"),
        category: get("--category"),
        id: get("--id"),
        verbose: v.iter().any(|a| a == "--verbose"),
        reps: get("--reps").and_then(|r| r.parse().ok()).unwrap_or(3),
    }
}

/// 2026-09-27 12:00 local.
fn fixed_now() -> i64 {
    let d = NaiveDate::from_ymd_opt(2026, 9, 27).unwrap().and_hms_opt(12, 0, 0).unwrap();
    Local.from_local_datetime(&d).earliest().unwrap().timestamp_millis()
}

fn build(now: i64) -> (Corpus, Store) {
    let corpus = Corpus::generate(42, now);
    let dir = std::path::Path::new("target/ask-eval");
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("corpus-42.db");
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
    let store = Store::open(&path).unwrap();
    corpus.load_into(&store).unwrap();
    loop {
        let p = store.extract_pending(5_000).unwrap();
        if p.remaining == 0 || p.scanned == 0 {
            break;
        }
    }
    (corpus, store)
}

// --------------------------------------------------------------- reading

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn norm(x: &str) -> String {
    x.chars()
        .map(|c| match c {
            'á' | 'à' | 'Á' => 'a',
            'é' | 'è' | 'É' => 'e',
            'í' | 'Í' => 'i',
            'ó' | 'Ó' => 'o',
            'ú' | 'ü' | 'Ú' => 'u',
            'ñ' => 'n',
            c => c,
        })
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Conversations an answer cites, in order.
fn cited(a: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |c: &Value| {
        let d = doc_id(s(c, "accountId"), s(c, "threadId"));
        if !s(c, "threadId").is_empty() && !out.contains(&d) {
            out.push(d);
        }
    };
    for p in a["passages"].as_array().into_iter().flatten() {
        push(&p["cite"]);
    }
    for c in a["cards"].as_array().into_iter().flatten() {
        push(&c["cite"]);
    }
    for i in a["items"].as_array().into_iter().flatten() {
        push(i);
    }
    for f in a["facts"].as_array().into_iter().flatten() {
        if f["cite"].is_object() {
            push(&f["cite"]);
        }
    }
    out
}

fn exact(a: &Value) -> bool {
    !matches!(s(a, "intent"), "passage" | "unknown" | "find")
}

/// Integers in the headline that aren't part of a date.
fn first_int(h: &str) -> Option<u64> {
    let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let words: Vec<&str> = h.split(|c: char| c.is_whitespace() || c == '(' || c == ')').collect();
    for (i, w) in words.iter().enumerate() {
        let t = w.trim_matches(|c: char| !c.is_alphanumeric());
        if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || c == ',') {
            continue;
        }
        let n: u64 = match t.replace(',', "").parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let prev = i.checked_sub(1).map(|p| words[p].to_lowercase()).unwrap_or_default();
        if (1990..=2100).contains(&n) && t.len() == 4 {
            continue;
        }
        if months.iter().any(|m| prev.starts_with(m)) || prev.starts_with('q') && prev.len() <= 2 {
            continue;
        }
        if w.starts_with('$') || w.starts_with('€') {
            continue;
        }
        return Some(n);
    }
    let l = h.to_lowercase();
    (l.starts_with("no ") || l.starts_with("no:") || l.contains("couldn't find") || l.starts_with("none")).then_some(0)
}

fn money_str(v: f64, cur: &str) -> String {
    let neg = v < 0.0;
    let cents = (v.abs() * 100.0).round() as i64;
    let int = (cents / 100).to_string();
    let mut g = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            g.push(',');
        }
        g.push(ch);
    }
    let body = format!("{g}.{:02}", cents % 100);
    let body = match cur {
        "USD" => format!("${body}"),
        "EUR" => format!("€{body}"),
        c => format!("{body} {c}"),
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

fn totals(a: &Value) -> BTreeMap<String, f64> {
    let mut m = BTreeMap::new();
    let src = if a["result"]["totals"].as_array().is_some_and(|t| !t.is_empty()) {
        &a["result"]["totals"]
    } else {
        &a["sum"]["totals"]
    };
    for t in src.as_array().into_iter().flatten() {
        m.insert(s(t, "currency").to_string(), t["value"].as_f64().unwrap_or(0.0));
    }
    m
}

/// Dates the answer states: its result, the first card's, the headline's.
fn first_card_date(a: &Value) -> Option<String> {
    let f = &a["cards"][0]["fact"];
    for k in ["departTime", "checkin", "start", "dueDate", "expected"] {
        if let Some(d) = f.get(k).and_then(Value::as_str) {
            return Some(d.chars().take(10).collect());
        }
    }
    None
}

fn text_blob(a: &Value) -> String {
    format!(
        "{} {} {} {}",
        s(a, "headline"),
        s(a, "detail"),
        a["result"]["text"].as_str().unwrap_or(""),
        a["cards"].get(0).map(|c| c.to_string()).unwrap_or_default()
    )
}

fn label_match(got: &str, want: &str) -> bool {
    let (g, w) = (norm(got), norm(want));
    !g.is_empty() && !w.is_empty() && (g == w || g.contains(&w) || w.contains(&g))
}

/// Is the answer right? (the check, and a word on why not)
fn grade(a: &Value, e: &Expected, g: &Gold) -> (bool, String) {
    let h = s(a, "headline");
    let r = &a["result"];
    let ok = |b: bool, why: String| (b, if b { String::new() } else { why });
    match &e.value {
        Expect::Count(n) => {
            if !exact(a) {
                return (false, "not an exact answer".into());
            }
            let got = r["count"].as_u64().or_else(|| first_int(h));
            ok(got == Some(*n), format!("count {got:?}, want {n}"))
        }
        Expect::Money(want) => {
            if !exact(a) {
                return (false, "not an exact answer".into());
            }
            let got = totals(a);
            let by_totals = !got.is_empty()
                && want.iter().all(|(c, v)| got.get(c).is_some_and(|x| (x - v).abs() < 0.011))
                && got.iter().all(|(c, v)| v.abs() < 0.011 || want.iter().any(|w| &w.0 == c));
            let by_text = want.iter().all(|(c, v)| text_blob(a).contains(&money_str(*v, c)));
            ok(by_totals || by_text, format!("money {got:?}, want {want:?}"))
        }
        Expect::Value(v) => {
            let got = r["value"].as_f64();
            let text = format!("{v:.1}");
            ok(got.is_some_and(|x| (x - v).abs() < 0.051) || (exact(a) && h.contains(&text)), format!("value {got:?}, want {v:.2}"))
        }
        Expect::Winner(ws) => {
            let got = r["winner"].as_str().map(String::from);
            if ws.len() == 1 && ws[0] == "the same" {
                return ok(h.starts_with("The same"), format!("want a tie, got {h:?}"));
            }
            let hit = |x: &str| ws.iter().any(|w| label_match(x, &w.replace('-', " ")));
            let first = h.split([':', ',']).next().unwrap_or("");
            ok(
                got.as_deref().is_some_and(hit) || (exact(a) && hit(first)),
                format!("winner {got:?} / {first:?}, want {ws:?}"),
            )
        }
        Expect::Groups(rows) => {
            let got: Vec<(String, f64)> = a["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| {
                    let label = s(x, "label").to_string();
                    let v = match g.measure {
                        oracle::Measure::Money => x["totals"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|t| s(t, "currency") == "USD")
                            .filter_map(|t| t["value"].as_f64())
                            .sum(),
                        oracle::Measure::Nights => x["nights"].as_f64().unwrap_or(0.0),
                        _ => x["count"].as_f64().unwrap_or(0.0),
                    };
                    (label, v)
                })
                .collect();
            let all_found = rows.iter().all(|(l, v)| got.iter().any(|(gl, gv)| label_match(gl, &l.replace('-', " ")) && (gv - v).abs() < 0.011));
            let no_extra = got.iter().all(|(gl, gv)| gv.abs() < 0.011 || rows.iter().any(|(l, _)| label_match(gl, &l.replace('-', " "))));
            ok(!got.is_empty() && all_found && no_extra, format!("groups {got:?}, want {rows:?}"))
        }
        Expect::Set(docs) => {
            let c: BTreeSet<String> = cited(a).into_iter().collect();
            ok(exact(a) && &c == docs, format!("cited {} of {} (+{} extra)", c.intersection(docs).count(), docs.len(), c.difference(docs).count()))
        }
        Expect::Date(d) => {
            let iso = d.format("%Y-%m-%d").to_string();
            let short = d.format("%b %-d").to_string();
            let got = r["date"].as_str().map(String::from).or_else(|| first_card_date(a));
            let in_text = h.contains(&format!("{short},")) || h.contains(&format!("{short} ")) || h.ends_with(&short) || h.contains(&format!("{short})"));
            ok(got.as_deref() == Some(iso.as_str()) || (exact(a) && got.is_none() && in_text), format!("date {got:?} / {h:?}, want {iso}"))
        }
        Expect::Text(t) => ok(!t.is_empty() && norm(&text_blob(a)).contains(&norm(t)), format!("want {t} in {h:?}")),
        Expect::Yes(b) => {
            let l = h.to_lowercase();
            let got = if let Some(y) = r["yes"].as_bool() {
                Some(y)
            } else if l.starts_with("yes") {
                Some(true)
            } else if l.starts_with("no") || l.starts_with("i couldn't") || l.starts_with("none") || s(a, "confidence") == "none" {
                Some(false)
            } else if exact(a) {
                match first_int(h) {
                    Some(n) if matches!(s(a, "intent"), "count" | "query") => Some(n > 0),
                    _ => Some(!a["cards"].as_array().is_none_or(|c| c.is_empty()) || !a["items"].as_array().is_none_or(|c| c.is_empty())),
                }
            } else {
                None
            };
            ok(got == Some(*b), format!("yes {got:?}, want {b}"))
        }
        Expect::Sources(docs) => {
            let c = cited(a);
            ok(c.iter().take(3).any(|d| docs.contains(d)), format!("top cites {:?}", c.iter().take(3).collect::<Vec<_>>()))
        }
    }
}

/// Recall of the conversations behind the answer, for fact answers.
fn source_recall(a: &Value, e: &Expected) -> Option<f64> {
    if e.docs.is_empty() || matches!(e.value, Expect::Sources(_)) {
        return None;
    }
    let c: BTreeSet<String> = cited(a).into_iter().collect();
    let cap = e.docs.len().min(60);
    Some(c.intersection(&e.docs).count().min(cap) as f64 / cap as f64)
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let i = ((p * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1;
    s[i]
}

fn main() {
    let args = args();
    let now = fixed_now();
    let off = Local.timestamp_millis_opt(now).unwrap().offset().local_minus_utc();
    let t0 = Instant::now();
    let (corpus, store) = build(now);
    eprintln!("corpus: {} messages, facts extracted in {:.1}s", corpus.messages.len(), t0.elapsed().as_secs_f64());
    let truth = Truth::from_corpus(&corpus);
    let today = truth.today;
    let qs: Vec<Question> = questions()
        .into_iter()
        .filter(|q| args.set.as_deref().is_none_or(|s| q.set == s))
        .filter(|q| args.category.as_deref().is_none_or(|c| q.category == c))
        .filter(|q| args.id.as_deref().is_none_or(|i| q.id == i))
        .collect();
    let scope = AskScope::default();
    // Warm up (page cache, prepared statements).
    for q in &qs {
        let _ = store.ask(&q.text, &scope, now, off);
    }
    #[derive(Default)]
    struct Cat {
        n: usize,
        grammar: usize,
        exact: usize,
        with_parser: usize,
        parser_used: usize,
        recall: Vec<f64>,
    }
    let mut cats: BTreeMap<(&'static str, String), Cat> = BTreeMap::new();
    let mut lat: Vec<f64> = Vec::new();
    let mut rows: Vec<Value> = Vec::new();
    for q in &qs {
        let gold = parse_gold(&q.gold, today);
        let e = expect(&truth, &gold);
        let mut times = Vec::new();
        let mut answer = None;
        for _ in 0..args.reps.max(1) {
            let t = Instant::now();
            let a = store.ask(&q.text, &scope, now, off).expect("ask");
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            answer = Some(a);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        let ms = times[times.len() / 2];
        lat.push(ms);
        let a = serde_json::to_value(answer.unwrap()).unwrap();
        let (ok, why) = grade(&a, &e, &gold);
        let is_exact = exact(&a) || (gold.op == Op::Passage && a["passages"].as_array().is_some_and(|p| !p.is_empty()));
        let mut parser_ok = ok;
        let mut parser_used = false;
        let mut parser_why = String::new();
        if args.parser && !ok && !is_exact {
            if let Some(pa) = draft::simulated(&store, &q.text, &gold, &scope, now, off) {
                let pa = serde_json::to_value(pa).unwrap();
                let (pok, pwhy) = grade(&pa, &e, &gold);
                parser_used = true;
                parser_ok = pok;
                parser_why = pwhy;
            }
        }
        let c = cats.entry((q.set, q.category.clone())).or_default();
        c.n += 1;
        c.grammar += ok as usize;
        c.exact += is_exact as usize;
        c.with_parser += parser_ok as usize;
        c.parser_used += parser_used as usize;
        if let Some(r) = source_recall(&a, &e) {
            c.recall.push(r);
        }
        if args.verbose || args.id.is_some() {
            println!(
                "{} [{}] {} {:?}\n    → {} ({}){}{}",
                if ok { "✓" } else if parser_ok { "~" } else { "✗" },
                q.category,
                q.id,
                q.text,
                s(&a, "headline"),
                s(&a, "intent"),
                if why.is_empty() { String::new() } else { format!("\n    ! {why}") },
                if parser_used { format!("\n    parser: {}", if parser_ok { "✓".to_string() } else { parser_why.clone() }) } else { String::new() }
            );
        }
        rows.push(json!({
            "set": q.set, "id": q.id, "src": q.src, "category": q.category, "question": q.text, "gold": q.gold,
            "expected": format!("{:?}", e.value), "headline": s(&a, "headline"), "intent": s(&a, "intent"),
            "correct": ok, "exact": is_exact, "why": why, "parserUsed": parser_used, "parserCorrect": parser_ok,
            "ms": ms,
            "understood": a["understood"]["summary"],
        }));
    }
    for set in ["tuned", "heldout"] {
    if !cats.keys().any(|k| k.0 == set) {
        continue;
    }
    println!("\n{} set", if set == "tuned" { "Tuning" } else { "Held-out" });
    println!("\n| Category | n | Grammar correct | Answered exactly | + parser (simulated) | Parser used | Source recall |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    let mut tot = Cat::default();
    for ((s, name), c) in &cats {
        if *s != set {
            continue;
        }
        let f = |x: usize| format!("{} ({:.0}%)", x, 100.0 * x as f64 / c.n.max(1) as f64);
        let rec = if c.recall.is_empty() { "—".into() } else { format!("{:.2}", c.recall.iter().sum::<f64>() / c.recall.len() as f64) };
        println!("| {name} | {} | {} | {} | {} | {} | {rec} |", c.n, f(c.grammar), f(c.exact), if args.parser { f(c.with_parser) } else { "—".into() }, c.parser_used);
        tot.n += c.n;
        tot.grammar += c.grammar;
        tot.exact += c.exact;
        tot.with_parser += c.with_parser;
        tot.parser_used += c.parser_used;
        tot.recall.extend(c.recall.iter());
    }
    let f = |x: usize| format!("{} ({:.1}%)", x, 100.0 * x as f64 / tot.n.max(1) as f64);
    println!(
        "| **all** | {} | {} | {} | {} | {} | {:.2} |",
        tot.n,
        f(tot.grammar),
        f(tot.exact),
        if args.parser { f(tot.with_parser) } else { "—".into() },
        tot.parser_used,
        tot.recall.iter().sum::<f64>() / tot.recall.len().max(1) as f64
    );
    }
    println!("\nLatency over {} questions (median of {} runs each): p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms", lat.len(), args.reps, pct(&lat, 0.5), pct(&lat, 0.95), pct(&lat, 1.0));
    if let Some(out) = args.out {
        let v = json!({
            "now": "2026-09-27T12:00 local", "fingerprint": corpus.fingerprint(),
            "p50": pct(&lat, 0.5), "p95": pct(&lat, 0.95), "questions": rows,
        });
        std::fs::write(&out, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {out}");
    }
}
