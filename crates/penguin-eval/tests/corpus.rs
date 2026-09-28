//! The corpus is deterministic, fictional, loads through the public Store
//! API, and every query has something to find.

use penguin_core::Store;
use penguin_eval::corpus::Corpus;
use penguin_eval::eval::{self, EvalQuery};
use penguin_eval::judge;
use penguin_eval::queries::all_queries;
use penguin_eval::retrieve::Keyword;

/// The wall clock, for tests that search: `Store::search` resolves
/// relative dates ("last month") against it, so the corpus has to be
/// generated for today too.
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A fixed moment (2026-09-26 00:30 UTC) for tests that only look at the
/// generated mail. Just after midnight on purpose: the corpus's own "now"
/// is noon of that local day, hours after the wall clock, which is when
/// comparing message dates with the wall clock used to fail.
const FIXED: i64 = 1_790_382_600_000;

#[test]
fn same_seed_same_mailbox_and_different_seed_differs() {
    let t = FIXED;
    let a = Corpus::generate(42, t);
    let b = Corpus::generate(42, t);
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert_eq!(a.messages.len(), b.messages.len());
    assert_ne!(a.fingerprint(), Corpus::generate(7, t).fingerprint());
}

#[test]
fn realistic_size_and_only_fictional_addresses() {
    let c = Corpus::generate(42, FIXED);
    assert!(
        (17_000..24_000).contains(&c.messages.len()),
        "{} messages",
        c.messages.len()
    );
    assert!(
        (5_500..7_500).contains(&c.threads.len()),
        "{} threads",
        c.threads.len()
    );
    for m in &c.messages {
        for a in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
            assert!(
                a.email.ends_with(".example"),
                "non-.example address {}",
                a.email
            );
        }
        // Nothing after the moment the mailbox describes: its own "now"
        // (noon of the generation day), not the wall clock.
        assert!(m.date < c.now, "future message {}", m.id);
    }
}

#[test]
fn query_set_is_valid() {
    let c = Corpus::generate(42, now());
    let qs = all_queries(c.now);
    assert!(qs.len() >= 200, "{} queries", qs.len());
    let problems = judge::validate(&c, &qs);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn loads_into_a_store_and_finds_exact_identifiers() {
    let dir = std::env::temp_dir().join(format!(
        "penguin-eval-test-{}-{}",
        std::process::id(),
        now()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let corpus = Corpus::generate(42, now());
    let store = Store::open(&dir.join("eval.db")).unwrap();
    corpus.load_into(&store).unwrap();
    let specs: Vec<_> = all_queries(corpus.now)
        .into_iter()
        .filter(|q| q.category == judge::Category::Identifier)
        .collect();
    let qrels = judge::qrels(&corpus, &specs);
    let queries: Vec<EvalQuery> = specs
        .iter()
        .map(|q| EvalQuery {
            id: q.id.into(),
            text: q.text.clone(),
            category: q.category.name().into(),
            facets: vec![],
            qrels: qrels[q.id].clone(),
        })
        .collect();
    let (results, _) = eval::evaluate(&Keyword { store: &store }, &queries, 1).unwrap();
    // Exact codes and numbers are keyword search's home turf: all on top.
    for r in &results {
        assert_eq!(r.rr, 1.0, "{} ({}) not first", r.id, r.text);
    }
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}
