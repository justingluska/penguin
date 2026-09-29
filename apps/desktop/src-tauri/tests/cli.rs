//! End-to-end tests of the real `penguin-cli` binary against a throwaway
//! data dir: stdout/stderr discipline, exit codes, JSON envelopes, and the
//! MCP server over actual stdio.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use penguin_core::{Account, Address, Message, Store};

// The in-process S3 stub the share tests use (never real storage). This
// file uses part of it; the rest is used by the library's own tests.
#[allow(dead_code)]
#[path = "../src/share/stub.rs"]
mod stub;

const ADA: &str = "ada@penguin.example";

fn data_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("penguin-cli-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let s = Store::open(&root.join("penguin.db")).unwrap();
    s.upsert_account(&Account {
        id: ADA.into(),
        email: ADA.into(),
        display_name: Some("Ada Lovelace".into()),
        nickname: None,
        color: "#4F7CFF".into(),
        added_at: 1,
        ..Account::default()
    })
    .unwrap();
    s.upsert_messages(&[Message {
        account_id: ADA.into(),
        id: "m1".into(),
        thread_id: "t1".into(),
        date: 1_767_261_600_000,
        from: Address {
            name: Some("Bo Park".into()),
            email: "bo@acme.example".into(),
        },
        to: vec![Address {
            name: None,
            email: ADA.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Walrus migration plan".into(),
        snippet: "The walrus migration starts Monday".into(),
        body_text: "The walrus migration starts Monday.\n\nOn Wed, Bo wrote:\n> older text".into(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }])
    .unwrap();
    root
}

fn set_mcp(root: &Path, enabled: bool) {
    std::fs::write(
        root.join("settings.json"),
        format!(r#"{{"mcp":{{"enabled":{enabled}}}}}"#),
    )
    .unwrap();
}

fn cli(root: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_penguin-cli"))
        .args(args)
        .env("PENGUIN_DATA_DIR", root)
        .env_remove("PENGUIN_LOG")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn search_json_on_stdout_status_on_stderr() {
    let root = data_dir("search");
    let (code, stdout, stderr) = cli(&root, &["search", "walrus", "--json", "--account", ADA]);
    assert_eq!(code, 0, "{stderr}");
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout is exactly one JSON document");
    assert_eq!(v["schemaVersion"], 1);
    assert_eq!(v["kind"], "search");
    assert_eq!(v["data"]["hits"][0]["threadId"], "t1");
    assert!(!stdout.contains('\u{1b}'), "no ANSI escapes");

    // Human mode: results on stdout, the summary line on stderr.
    let (code, stdout, stderr) = cli(&root, &["search", "walrus"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Walrus migration plan"));
    assert!(stderr.contains("1 hits"));

    // No matches: exit 2, still valid JSON on stdout.
    let (code, stdout, _) = cli(&root, &["search", "narwhal", "--json"]);
    assert_eq!(code, 2);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["data"]["hits"].as_array().unwrap().len(), 0);

    // Usage errors: exit 64, typed JSON error.
    let (code, stdout, _) = cli(&root, &["search", "walrus", "--json", "--profile", "Nope"]);
    assert_eq!(code, 64);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["kind"], "error");
    assert_eq!(v["data"]["code"], "invalidInput");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn thread_markdown_and_json() {
    let root = data_dir("thread");
    let (code, md, _) = cli(&root, &["thread", ADA, "t1", "--md"]);
    assert_eq!(code, 0);
    assert!(md.starts_with("# Walrus migration plan"));
    assert!(md.contains("The walrus migration starts Monday."));
    assert!(!md.contains("older text"), "quotes stripped");

    let (code, stdout, _) = cli(&root, &["thread", ADA, "t1", "--json", "--full"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["kind"], "thread");
    assert!(v["data"]["messages"][0]["fullText"]
        .as_str()
        .unwrap()
        .contains("older text"));

    let (code, _, stderr) = cli(&root, &["thread", ADA, "missing"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("no thread missing"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn query_commands_never_create_a_database() {
    let root = std::env::temp_dir().join(format!("penguin-cli-empty-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let (code, _, stderr) = cli(&root, &["search", "x"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("no Penguin database"));
    assert!(!root.join("penguin.db").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mcp_refuses_when_disabled() {
    let root = data_dir("mcp-off");
    let (code, stdout, stderr) = cli(&root, &["mcp"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty(), "stdout is reserved for JSON-RPC");
    assert!(stderr.contains("Settings → Developer"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mcp_over_real_stdio() {
    let root = data_dir("mcp-on");
    set_mcp(&root, true);
    let mut child = Command::new(env!("CARGO_BIN_EXE_penguin-cli"))
        .arg("mcp")
        .env("PENGUIN_DATA_DIR", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = move |v: serde_json::Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = || {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
    };
    send(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
    );
    let init = recv();
    assert_eq!(init["result"]["serverInfo"]["name"], "penguin");
    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search","arguments":{"query":"walrus"}}}),
    );
    let res = recv();
    assert_eq!(res["id"], 2);
    assert_eq!(res["result"]["isError"], false, "{res}");
    assert!(res["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("\"threadId\": \"t1\""));
    // Closing stdin ends the session; the server must exit on its own.
    drop(send);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    let audit = std::fs::read_to_string(root.join("logs").join("mcp-audit.log")).unwrap();
    assert!(audit.contains("\"tool\":\"search\""));
    let _ = std::fs::remove_dir_all(root);
}

// ---------- drafts and sends: the real binary → the agent socket → a fake app ----------

mod app {
    //! A stand-in for the running Penguin app: the real agent service on the
    //! real socket, with the fake provider behind it.
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use penguin_core::{AccountProvider, Store};
    use penguin_desktop_lib::agent::audit::AuditLog;
    use penguin_desktop_lib::agent::ipc;
    use penguin_desktop_lib::agent::writes::{AgentService, QueuedSend, WriteHost};
    use penguin_desktop_lib::error::{CmdError, CmdResult};
    use penguin_desktop_lib::ops::Paths;
    use penguin_desktop_lib::settings::{AgentAccess, McpSettings, Settings};
    use penguin_desktop_lib::share::config::{LinkLifetime, SecretStore, ShareLinkConfigInput};
    use penguin_desktop_lib::share::Share;
    use penguin_provider::credentials::MemorySecrets;
    use penguin_provider::fake::FakeProvider;
    use penguin_provider::{async_trait, MailProvider};

    pub struct Host {
        pub store: Store,
        pub paths: Paths,
        pub fake: FakeProvider,
        pub settings: Mutex<Settings>,
        pub lock: tokio::sync::Mutex<()>,
        pub queued: Mutex<Vec<QueuedSend>>,
        pub audit: AuditLog,
        pub home: PathBuf,
        /// Share links as the app holds them (the Keychain in memory).
        pub share: Arc<Share>,
    }

    impl Host {
        pub fn set_level(&self, level: AgentAccess) {
            self.settings.lock().unwrap().mcp = McpSettings::with_access(level);
        }

        /// Settings → Share links: storage at `endpoint` (a fictional key
        /// pair), and the agent switch.
        pub fn set_share(&self, endpoint: &str, allow_agents: bool) {
            self.share
                .set_config(&ShareLinkConfigInput {
                    endpoint: endpoint.into(),
                    bucket: "penguin-shares".into(),
                    region: String::new(),
                    access_key_id: "0123456789abcdef0123456789abcdef".into(),
                    secret_access_key: Some(
                        "fictional-secret-0123456789abcdef0123456789abcdef".into(),
                    ),
                    lifetime: LinkLifetime::Hour,
                    delete_on_expiry: true,
                    allow_agents,
                })
                .unwrap();
        }
    }

    #[async_trait]
    impl WriteHost for Host {
        fn store(&self) -> &Store {
            &self.store
        }
        fn paths(&self) -> &Paths {
            &self.paths
        }
        fn settings(&self) -> Settings {
            self.settings.lock().unwrap().clone()
        }
        async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>> {
            if account_id == super::ADA {
                Ok(Arc::new(self.fake.clone()))
            } else {
                Err(CmdError::not_found(account_id.to_string()))
            }
        }
        fn drafts_lock(&self) -> &tokio::sync::Mutex<()> {
            &self.lock
        }
        fn mail_changed(&self, _: &str, _: Vec<String>) {}
        fn send_queued(&self, q: &QueuedSend) {
            self.queued.lock().unwrap().push(q.clone());
        }
        fn audit(&self) -> &AuditLog {
            &self.audit
        }
        fn share(&self) -> Arc<Share> {
            self.share.clone()
        }
        fn home_dir(&self) -> Option<PathBuf> {
            Some(self.home.clone())
        }
    }

    /// Serve the socket for the data dir at `root` (made by `data_dir`).
    pub fn start(root: &Path, level: AgentAccess) -> (Arc<Host>, tokio::task::JoinHandle<()>) {
        let paths = Paths {
            data_dir: root.to_path_buf(),
            config_dir: root.to_path_buf(),
            cache_dir: root.join("cache"),
        };
        let store = Store::open(&paths.db_path()).unwrap();
        store.migrate_agent().unwrap();
        // I've written to Bo before, so Bo is someone I've emailed.
        let mut sent = store.get_message(super::ADA, "m1").unwrap().unwrap();
        sent.id = "m0".into();
        sent.from = sent.to[0].clone();
        sent.to = vec![penguin_core::Address {
            name: None,
            email: "bo@acme.example".into(),
        }];
        sent.label_ids = vec!["SENT".into()];
        store.upsert_messages(&[sent]).unwrap();
        // Beside the data dir: files inside it are never attachable.
        let home = super::home_of(root);
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        std::fs::write(home.join("Documents/plan.pdf"), b"%PDF-1.4 plan").unwrap();
        let mut settings = Settings::default();
        settings.mcp = McpSettings::with_access(level);
        let share = Arc::new(Share::new(
            paths.config_dir.clone(),
            &paths.data_dir,
            SecretStore::new(Box::new(Arc::new(MemorySecrets::default()))),
        ));
        let host = Arc::new(Host {
            share,
            fake: FakeProvider::new(super::ADA, AccountProvider::Gmail, store.clone()),
            store,
            audit: AuditLog::open(Some(&root.join("logs"))),
            paths: paths.clone(),
            settings: Mutex::new(settings),
            lock: tokio::sync::Mutex::new(()),
            queued: Mutex::new(Vec::new()),
            home,
        });
        let listener = ipc::Listener::bind(&paths).unwrap();
        let service = Arc::new(AgentService::new(host.clone()));
        let task = tokio::spawn(async move {
            let _ = listener.serve(service).await;
        });
        (host, task)
    }
}

/// `data_dir`, moved to a short path so the socket fits macOS's limit.
fn short_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = data_dir(tag);
    let short = std::env::temp_dir().join(format!("pgc-{tag}-{}", nanos % 1_000_000_000));
    std::fs::rename(&root, &short).unwrap();
    short
}

/// The stand-in home folder for `root`'s app (beside it, not inside).
fn home_of(root: &Path) -> PathBuf {
    PathBuf::from(format!("{}-home", root.display()))
}

async fn cli_async(root: &Path, args: &[&str]) -> (i32, String, String) {
    let root = root.to_path_buf();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        cli(&root, &args)
    })
    .await
    .unwrap()
}

#[test]
fn drafting_without_penguin_running_exits_69() {
    let root = short_dir("down");
    let (code, stdout, stderr) = cli(
        &root,
        &[
            "draft",
            "create",
            "--to",
            "bo@acme.example",
            "--body",
            "hi",
            "--json",
        ],
    );
    assert_eq!(code, 69, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["kind"], "error");
    assert_eq!(v["data"]["code"], "unavailable");
    assert!(stderr.contains("Penguin isn't running"), "{stderr}");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draft_list_send_through_the_app_with_typed_exit_codes() {
    use penguin_desktop_lib::settings::AgentAccess;
    let root = short_dir("app");
    let (host, server) = app::start(&root, AgentAccess::Draft);
    let plan = home_of(&root)
        .join("Documents/plan.pdf")
        .display()
        .to_string();
    // A reply with an attachment.
    let (code, stdout, stderr) = cli_async(
        &root,
        &[
            "draft",
            "create",
            "--reply-to",
            "m1",
            "--body",
            "On it.",
            "--attach",
            &plan,
            "--json",
        ],
    )
    .await;
    assert_eq!(code, 0, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["kind"], "draft");
    assert_eq!(v["data"]["threadId"], "t1");
    assert_eq!(v["data"]["subject"], "Re: Walrus migration plan");
    assert_eq!(v["data"]["attachments"][0]["filename"], "plan.pdf");
    assert_eq!(v["data"]["createdBy"], "cli");
    let draft_id = v["data"]["draftId"].as_str().unwrap().to_string();

    let (code, stdout, _) = cli_async(&root, &["draft", "list"]).await;
    assert_eq!(code, 0);
    assert!(stdout.contains(&draft_id), "{stdout}");

    // Sending needs the send level: 77, typed.
    let (code, stdout, stderr) = cli_async(&root, &["send", &draft_id, "--json"]).await;
    assert_eq!(code, 77, "{stderr}");
    let e: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(e["data"]["code"], "permissionDenied");
    assert!(host.queued.lock().unwrap().is_empty());

    // At the send level it's queued, and the output says it isn't sent yet.
    host.set_level(AgentAccess::Send);
    let (code, stdout, stderr) = cli_async(&root, &["send", &draft_id]).await;
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.starts_with(&format!("queued draft {draft_id}")),
        "{stdout}"
    );
    assert!(stderr.contains("isn't sent yet"), "{stderr}");
    assert_eq!(host.queued.lock().unwrap().len(), 1);
    assert!(host.fake.outgoing().is_empty());

    // Someone never emailed: refused (77), nothing saved.
    let before = host.fake.server.lock().unwrap().drafts.len();
    let (code, _, stderr) = cli_async(
        &root,
        &[
            "send",
            "--from",
            ADA,
            "--to",
            "stranger@elsewhere.example",
            "--body",
            "x",
        ],
    )
    .await;
    assert_eq!(code, 77, "{stderr}");
    assert_eq!(host.fake.server.lock().unwrap().drafts.len(), before);

    // Delete it, and its queued send with it.
    let (code, _, stderr) = cli_async(&root, &["draft", "delete", &draft_id]).await;
    assert_eq!(code, 0, "{stderr}");
    assert!(host.store.list_scheduled_sends(None).unwrap().is_empty());
    // Usage errors stay 64.
    let (code, _, _) = cli_async(&root, &["draft", "frobnicate"]).await;
    assert_eq!(code, 64);
    server.abort();
    let _ = std::fs::remove_dir_all(home_of(&root));
    let _ = std::fs::remove_dir_all(root);
}

/// `share-link`: the real binary → the agent socket → the stand-in app →
/// the S3 stub. 69 without Penguin, 77 for each gate (the level, then the
/// share-link switch), 0 with the link on stdout, and no link or file name
/// in the audit log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_link_through_the_app_with_typed_exit_codes() {
    use penguin_desktop_lib::settings::AgentAccess;
    let root = short_dir("share");
    // m1 carries a PDF Penguin already has (as if previewed).
    {
        let store = Store::open(&root.join("penguin.db")).unwrap();
        let mut m = store.get_message(ADA, "m1").unwrap().unwrap();
        m.attachments = vec![penguin_core::AttachmentMeta {
            id: "att-1".into(),
            filename: "Walrus plan.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 17,
            content_id: None,
            inline: false,
        }];
        store.upsert_messages(&[m]).unwrap();
    }
    let paths = penguin_desktop_lib::ops::Paths {
        data_dir: root.clone(),
        config_dir: root.clone(),
        cache_dir: root.join("cache"),
    };
    penguin_desktop_lib::attachments::put_cached(&paths, ADA, "m1", "att-1", b"%PDF-1.4 the plan")
        .unwrap();
    let args = ["share-link", ADA, "m1", "att-1", "--json"];

    // Penguin isn't running: 69.
    let (code, stdout, stderr) = cli_async(&root, &args).await;
    assert_eq!(code, 69, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["data"]["code"], "unavailable");

    let stub = stub::Stub::start().await;
    let (host, server) = app::start(&root, AgentAccess::Read);
    host.set_share(&stub.endpoint, true);
    // Read only: 77, naming the agent level.
    let (code, stdout, stderr) = cli_async(&root, &args).await;
    assert_eq!(code, 77, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["data"]["code"], "permissionDenied");
    assert!(stderr.contains("Settings → Developer → Agents"), "{stderr}");
    // Read and draft, the share-link switch off: 77, naming Share links.
    host.set_level(AgentAccess::Draft);
    host.set_share(&stub.endpoint, false);
    let (code, _, stderr) = cli_async(&root, &args).await;
    assert_eq!(code, 77, "{stderr}");
    assert!(stderr.contains("Settings → Share links"), "{stderr}");
    assert!(
        stderr.contains("Let agents (CLI and MCP) create share links"),
        "{stderr}"
    );
    assert!(stub.objects.lock().unwrap().is_empty(), "nothing uploaded");

    // Both on: 0, the link in the envelope, and it downloads the file.
    host.set_share(&stub.endpoint, true);
    let (code, stdout, stderr) = cli_async(&root, &args).await;
    assert_eq!(code, 0, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["kind"], "shareLink");
    assert_eq!(v["data"]["name"], "Walrus plan.pdf");
    assert_eq!(v["data"]["size"], 17);
    let url = v["data"]["url"].as_str().unwrap().to_string();
    let got = reqwest::get(&url).await.unwrap();
    assert_eq!(got.bytes().await.unwrap().as_ref(), b"%PDF-1.4 the plan");
    // Human mode: only the link on stdout, what it means on stderr.
    let (code, stdout, stderr) = cli_async(&root, &["share-link", ADA, "m1", "att-1"]).await;
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.trim().starts_with(&stub.endpoint), "{stdout}");
    assert_eq!(stdout.trim().lines().count(), 1);
    assert!(stderr.contains("Anyone with this link"), "{stderr}");
    // A remote picture by address: 77. Wrong arguments: 64.
    let (code, _, stderr) = cli_async(
        &root,
        &[
            "share-link",
            ADA,
            "m1",
            "https://pictures.acme.example/a.png",
        ],
    )
    .await;
    assert_eq!(code, 77, "{stderr}");
    let (code, _, _) = cli_async(&root, &["share-link", ADA, "m1"]).await;
    assert_eq!(code, 64);

    // The audit log: the tool, the account and the outcome, never the link
    // or the file's name.
    let log = std::fs::read_to_string(root.join("logs").join("mcp-audit.log")).unwrap();
    assert!(log.contains("\"tool\":\"create_share_link\""), "{log}");
    assert!(log.contains(&format!("\"account\":\"{ADA}\"")));
    let key = url.split('?').next().unwrap().rsplit('/').nth(1).unwrap();
    for secret in [url.as_str(), key, "Walrus plan", "Walrus-plan", "X-Amz"] {
        assert!(!log.contains(secret), "audit log has {secret:?}");
    }
    server.abort();
    let _ = std::fs::remove_dir_all(home_of(&root));
    let _ = std::fs::remove_dir_all(root);
}
