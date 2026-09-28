//! Sync benchmark (ignored by default; run it in release):
//!
//!   scripts/box-cargo.sh test --release -p penguin-graph --lib bench_ -- --ignored --nocapture --test-threads=1
//!
//! `bench_graph_backfill`: the real sync engine backfilling an Inbox of
//! `PENGUIN_BENCH_N` corpus messages (default 2000, all inside the sync
//! window) from the in-process Graph, each request held for
//! `PENGUIN_BENCH_LATENCY_MS` (default 50) to stand in for Graph's response
//! time. Prints one `BENCH {json}` line: wall time, messages/s, and the
//! requests it took by kind. Requests are what Outlook limits: 10,000 per
//! 10 minutes per app and mailbox, 4 at a time.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use penguin_core::{Account, AccountProvider, Store};
use penguin_provider::credentials::{MemorySecrets, MicrosoftCredential, SecretVault};
use penguin_provider::window::{OlderMail, WindowPolicy, DAY_MS};
use penguin_provider::{async_trait, Backend, Result, SyncObserver};

use crate::auth::Auth;
use crate::fake_graph::{endpoints, FAtt, FMsg, FakeGraph, ME};
use crate::http::{Request, Response, Transport};
use crate::GraphBackend;

#[path = "../../penguin-core/examples/support/corpus.rs"]
mod corpus;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The fake Graph behind a fixed response time, counting requests by kind.
struct Slow {
    inner: Arc<FakeGraph>,
    latency: Duration,
    requests: Mutex<BTreeMap<String, usize>>,
}

fn kind(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let path = path.split("/v1.0").nth(1).unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["me", "messages", _] => "GET message".into(),
        ["me", "messages", _, "attachments", ..] => "GET attachment".into(),
        ["me", "mailFolders", _, "messages", "delta"] => "delta page".into(),
        ["me", "mailFolders", _, "messages"] => "folder listing".into(),
        ["me", "messages"] => "message listing".into(),
        ["me", "mailFolders", ..] => "folders".into(),
        _ => parts.join("/"),
    }
}

#[async_trait]
impl Transport for Slow {
    async fn send(&self, request: Request) -> Result<Response> {
        *self
            .requests
            .lock()
            .unwrap()
            .entry(kind(&request.url))
            .or_default() += 1;
        tokio::time::sleep(self.latency).await;
        self.inner.send(request).await
    }
}

struct Quiet;
impl SyncObserver for Quiet {
    fn status(&self, _: penguin_core::SyncStatus) {}
    fn mail_changed(&self, _: &str, _: Vec<String>) {}
    fn messages_added(&self, _: &str, _: Vec<String>) {}
    fn labels_added(&self, _: &str, _: Vec<(String, Vec<String>)>) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_graph_backfill() {
    let n = env_usize("PENGUIN_BENCH_N", 2000);
    let latency = Duration::from_millis(env_usize("PENGUIN_BENCH_LATENCY_MS", 50) as u64);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let fake = FakeGraph::new();
    let mut c = corpus::Corpus::new(42, now);
    let mut added = 0;
    let mut i = 0i64;
    while added < n {
        for m in c.thread(0) {
            if added == n {
                break;
            }
            // Spread over the last 150 days (inside a 6-month window),
            // newest first as mail arrives.
            let received_ms = now - 1_000 - (i * 150 * DAY_MS) / n as i64;
            i += 1;
            let body_html = m
                .body_html
                .clone()
                .unwrap_or_else(|| format!("<pre>{}</pre>", m.body_text.replace('<', "&lt;")));
            let attachments = m
                .attachments
                .iter()
                .map(|a| FAtt {
                    id: format!("ATT-{}-{}", added, a.id),
                    name: a.filename.clone(),
                    content_type: a.mime_type.clone(),
                    bytes: vec![0u8; (a.size as usize).min(4096)],
                    inline: a.inline,
                    content_id: a.content_id.clone(),
                })
                .collect();
            fake.add(
                "inbox",
                FMsg {
                    received_ms,
                    subject: m.subject.clone(),
                    body_html,
                    from: (
                        m.from.name.clone().unwrap_or_default(),
                        m.from.email.clone(),
                    ),
                    to: m
                        .to
                        .iter()
                        .map(|a| (a.name.clone().unwrap_or_default(), a.email.clone()))
                        .collect(),
                    is_read: true,
                    attachments,
                    ..FMsg::default()
                },
            );
            added += 1;
        }
    }

    let mem = Arc::new(MemorySecrets::default());
    let vault: SecretVault<MicrosoftCredential> = SecretVault::new(Box::new(mem));
    vault
        .save(
            ME,
            &MicrosoftCredential {
                refresh_token: "rt-0".into(),
                client_id: "1b2c3d4e-0000-1111-2222-333344445555".into(),
                scopes: vec![],
                tenant_id: None,
                object_id: None,
                obtained_at: 0,
            },
        )
        .unwrap();
    let slow = Arc::new(Slow {
        inner: fake.clone(),
        latency,
        requests: Mutex::default(),
    });
    let auth = Arc::new(Auth::new(fake.clone(), endpoints(), Arc::new(vault)));
    let dir = std::env::temp_dir().join(format!("penguin-graph-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir.join("mail.db")).unwrap();
    let account = Account {
        id: ME.into(),
        email: ME.into(),
        provider: AccountProvider::Microsoft,
        ..Account::default()
    };
    store.upsert_account(&account).unwrap();
    let backend =
        GraphBackend::with_parts(store.clone(), Arc::new(Quiet), auth, slow.clone()).unwrap();
    backend.set_window_policy(WindowPolicy {
        months: 6,
        older: OlderMail::Headers,
    });
    let mut s = backend.account_sync(ME);
    let t = Instant::now();
    s.init().await.unwrap();
    s.run_until_idle().await.unwrap();
    let secs = t.elapsed().as_secs_f64();
    let stored = store.count_messages(Some(ME)).unwrap();
    assert_eq!(stored as usize, n);
    let requests = slow.requests.lock().unwrap().clone();
    let total: usize = requests.values().sum();
    let out = serde_json::json!({
        "bench": "graph_backfill",
        "messages": n,
        "latency_ms": latency.as_millis() as u64,
        "seconds": secs,
        "messages_per_s": n as f64 / secs,
        "requests": total,
        "requests_per_100_msgs": total as f64 * 100.0 / n as f64,
        "requests_by_kind": requests,
    });
    println!("BENCH {out}");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
