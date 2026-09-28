//! Run one query against a bench db with stage timings:
//!   cargo run -p penguin-core --release --example probe_search -- <db> "<query>"
use penguin_core::{SearchRequest, Store};

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().collect();
    let store = Store::open(std::path::Path::new(&args[1])).unwrap();
    for q in &args[2..] {
        for _ in 0..3 {
            let r = store
                .search(&SearchRequest {
                    query: q.clone(),
                    account_id: None,
                    account_ids: None,
                    limit: 50,
                })
                .unwrap();
            eprintln!(
                "{q:?}: {:.1} ms, {} hits, {} files, {} people",
                r.took_ms,
                r.hits.len(),
                r.attachments.len(),
                r.people.len()
            );
        }
        let r = store
            .search(&SearchRequest {
                query: q.clone(),
                account_id: None,
                account_ids: None,
                limit: 50,
            })
            .unwrap();
        for h in r.hits.iter().take(5) {
            let age = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
                - h.date)
                / 86_400_000;
            eprintln!(
                "  {:.3} {:>5}d {:?} | {}",
                h.score,
                age,
                h.subject,
                h.snippet_html.chars().take(120).collect::<String>()
            );
        }
    }
}
