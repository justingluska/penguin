//! Time raw SQL against a bench db with the bundled SQLite:
//!   cargo run -p penguin-core --release --example probe_sql -- <db> "<sql>" [param]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let c =
        rusqlite::Connection::open_with_flags(&args[1], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    c.execute_batch("PRAGMA mmap_size=1073741824; PRAGMA cache_size=-65536;")
        .unwrap();
    let params: Vec<&str> = args[3..].iter().map(|s| s.as_str()).collect();
    let mut stmt = c.prepare(&args[2]).unwrap();
    for i in 0..4 {
        let t = std::time::Instant::now();
        let n = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |_| Ok(()))
            .unwrap()
            .count();
        if i > 0 {
            println!("{:.2} ms, {n} rows", t.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
