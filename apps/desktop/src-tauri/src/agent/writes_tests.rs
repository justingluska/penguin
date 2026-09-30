//! The agent write path against the fake provider: the permission gate at
//! every level, a downgrade applying to the next call, drafts landing in
//! the provider's Drafts and Penguin's, reply threading, the agent-only
//! scope, attachments, and sends waiting in the outbox.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use mail_parser::{MessageParser, MimeHeaders};
use penguin_core::{AccountProvider, Address};
use penguin_provider::fake::FakeProvider;
use serde_json::json;

use super::*;
use crate::actions::ActionHost;
use crate::agent::ipc::Handler;
use crate::settings::McpSettings;
use penguin_provider::MailProvider;

pub(crate) const ADA: &str = "ada@penguin.example";
pub(crate) const WORK: &str = "ada@work.example";
pub(crate) const BO: &str = "bo@acme.example";

/// The app, as the handlers see it: a fake provider per account, settings
/// the test changes at will, and what was announced.
pub(crate) struct TestHost {
    pub store: Store,
    pub paths: Paths,
    pub fakes: HashMap<String, FakeProvider>,
    pub settings: Mutex<Settings>,
    pub lock: tokio::sync::Mutex<()>,
    pub queued: Mutex<Vec<QueuedSend>>,
    pub changed: Mutex<Vec<(String, Vec<String>)>>,
    /// Actions a provider refused (action-failed).
    pub failures: Mutex<Vec<String>>,
    /// Organizing an agent did (the toast with Undo).
    pub organized: Mutex<Vec<crate::agent::organize::AgentOrganized>>,
    pub audit: AuditLog,
    /// Share links as the app holds them; not set up until `set_share`.
    pub share: Mutex<Arc<crate::share::Share>>,
    pub home: PathBuf,
    pub root: PathBuf,
    pub now: i64,
    /// Added to `now` (a test moving the clock forward).
    pub later: std::sync::atomic::AtomicI64,
}

/// A fictional storage key pair for the S3 stub.
pub(crate) const SHARE_KEY_ID: &str = "0123456789abcdef0123456789abcdef";
const SHARE_SECRET: &str = "fictional-secret-0123456789abcdef0123456789abcdef";

/// Share-link state in `dir`, as share-links.json and the Keychain would
/// hold it: set up for `endpoint` (with its secret) or not, and the agent
/// switch. The Keychain is in memory.
pub(crate) fn share_state(
    dir: &std::path::Path,
    endpoint: Option<&str>,
    allow_agents: bool,
) -> Arc<crate::share::Share> {
    use crate::share::config::{self, SecretStore, StoredConfig};
    use penguin_provider::credentials::MemorySecrets;
    let secrets = Arc::new(MemorySecrets::default());
    let mut stored = StoredConfig {
        allow_agents,
        ..StoredConfig::default()
    };
    if let Some(endpoint) = endpoint {
        secrets.items.lock().unwrap().insert(
            config::KEYCHAIN_ACCOUNT.to_string(),
            SHARE_SECRET.to_string(),
        );
        stored = StoredConfig {
            endpoint: endpoint.into(),
            bucket: "penguin-shares".into(),
            access_key_id: SHARE_KEY_ID.into(),
            secret_saved: true,
            ..stored
        };
    }
    config::save(dir, &stored).unwrap();
    Arc::new(crate::share::Share::new(
        dir.to_path_buf(),
        dir,
        SecretStore::new(Box::new(secrets)),
    ))
}

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: Some(name.into()),
        email: email.into(),
    }
}

fn message(id: &str, thread: &str, from: Address, to: Vec<Address>, labels: &[&str]) -> Message {
    Message {
        account_id: ADA.into(),
        id: id.into(),
        thread_id: thread.into(),
        date: 1_767_261_600_000,
        from,
        to,
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Walrus plan".into(),
        snippet: "The walrus migration".into(),
        body_text: "The walrus migration starts Monday.".into(),
        body_html: None,
        label_ids: labels.iter().map(|s| s.to_string()).collect(),
        attachments: vec![],
        message_id_header: Some(format!("<{id}@acme.example>")),
        in_reply_to: None,
        references: vec!["<m0@acme.example>".into()],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

impl TestHost {
    pub fn new(tag: &str, level: AgentAccess) -> Arc<TestHost> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("pg-w-{tag}-{}", nanos % 1_000_000_000));
        let paths = Paths {
            data_dir: root.join("data"),
            config_dir: root.join("data"),
            cache_dir: root.join("data/cache"),
        };
        Self::with_paths(paths, root, level)
    }

    pub fn with_paths(paths: Paths, root: PathBuf, level: AgentAccess) -> Arc<TestHost> {
        paths.create_all().unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join("Documents/plan.pdf"), b"%PDF-1.4 walrus plan").unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), b"PRIVATE").unwrap();
        let store = Store::open_in_memory().unwrap();
        store.migrate_agent().unwrap();
        for id in [ADA, WORK] {
            store
                .upsert_account(&Account {
                    id: id.into(),
                    email: id.into(),
                    display_name: Some("Ada Lovelace".into()),
                    ..Account::default()
                })
                .unwrap();
        }
        // Bo wrote to me (m1, the message replies answer), and I've written
        // to Bo before (m0), so Bo counts as someone I've emailed.
        let incoming = message(
            "m1",
            "t1",
            addr("Bo Park", BO),
            vec![addr("Ada", ADA)],
            &["INBOX"],
        );
        let mut sent = message(
            "m0",
            "t1",
            addr("Ada", ADA),
            vec![addr("Bo Park", BO)],
            &["SENT"],
        );
        sent.date -= 60_000;
        store.upsert_messages(&[sent, incoming]).unwrap();
        let mut fakes = HashMap::new();
        for id in [ADA, WORK] {
            fakes.insert(
                id.to_string(),
                FakeProvider::new(id, AccountProvider::Gmail, store.clone()),
            );
        }
        let mut settings = Settings::default();
        settings.mcp = McpSettings::with_access(level);
        Arc::new(TestHost {
            share: Mutex::new(share_state(&paths.config_dir, None, false)),
            audit: AuditLog::open(Some(&root.join("logs"))),
            store,
            paths,
            fakes,
            settings: Mutex::new(settings),
            lock: tokio::sync::Mutex::new(()),
            queued: Mutex::new(Vec::new()),
            changed: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            organized: Mutex::new(Vec::new()),
            home,
            root,
            now: 1_800_000_000_000,
            later: Default::default(),
        })
    }

    pub fn set_level(&self, level: AgentAccess) {
        let mut s = self.settings.lock().unwrap();
        let (delay, known) = (s.mcp.send_delay_seconds, s.mcp.send_known_only);
        s.mcp = McpSettings::with_access(level);
        s.mcp.send_delay_seconds = delay;
        s.mcp.send_known_only = known;
    }

    /// Share links set up for the storage at `endpoint` (None: not set
    /// up), with the agent switch `allow_agents`.
    pub fn set_share(&self, endpoint: Option<&str>, allow_agents: bool) {
        *self.share.lock().unwrap() = share_state(&self.paths.config_dir, endpoint, allow_agents);
    }

    pub fn fake(&self) -> &FakeProvider {
        &self.fakes[ADA]
    }

    pub fn audit_lines(&self) -> Vec<Value> {
        let text =
            std::fs::read_to_string(self.root.join("logs").join(super::super::audit::AUDIT_FILE))
                .unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[async_trait]
impl ActionHost for TestHost {
    fn store(&self) -> &Store {
        &self.store
    }
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>> {
        self.fakes
            .get(account_id)
            .map(|f| Arc::new(f.clone()) as Arc<dyn MailProvider>)
            .ok_or_else(|| CmdError::not_found(format!("no account {account_id}")))
    }
    fn emit_mail_changed(&self, account_id: &str, thread_ids: Vec<String>) {
        self.changed
            .lock()
            .unwrap()
            .push((account_id.to_string(), thread_ids));
    }
    fn emit_action_failed(&self, message: String) {
        self.failures.lock().unwrap().push(message);
    }
    fn poke(&self, _account_id: &str) {}
}

#[async_trait]
impl WriteHost for TestHost {
    fn paths(&self) -> &Paths {
        &self.paths
    }
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }
    fn drafts_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.lock
    }
    fn mail_changed(&self, account_id: &str, thread_ids: Vec<String>) {
        self.changed
            .lock()
            .unwrap()
            .push((account_id.to_string(), thread_ids));
    }
    fn send_queued(&self, queued: &QueuedSend) {
        self.queued.lock().unwrap().push(queued.clone());
    }
    fn organized(&self, event: &crate::agent::organize::AgentOrganized) {
        self.organized.lock().unwrap().push(event.clone());
    }
    fn audit(&self) -> &AuditLog {
        &self.audit
    }
    fn share(&self) -> Arc<crate::share::Share> {
        self.share.lock().unwrap().clone()
    }
    fn home_dir(&self) -> Option<PathBuf> {
        Some(self.home.clone())
    }
    fn log_dir(&self) -> Option<PathBuf> {
        Some(self.root.join("logs"))
    }
    fn now_ms(&self) -> i64 {
        self.now + self.later.load(std::sync::atomic::Ordering::Relaxed)
    }
}

fn service(host: &Arc<TestHost>) -> AgentService<TestHost> {
    AgentService::new(host.clone())
}

async fn call(host: &Arc<TestHost>, tool: &str, args: Value) -> CmdResult<Value> {
    service(host).handle("mcp", tool, args).await
}

fn data(v: &Value) -> &Value {
    &v["data"]
}

/// The MIME of a draft as the fake server holds it.
fn draft_mime(host: &TestHost, draft_id: &str) -> Vec<u8> {
    host.fake()
        .server
        .lock()
        .unwrap()
        .draft_mime
        .get(draft_id)
        .cloned()
        .unwrap_or_else(|| panic!("no draft {draft_id} on the server"))
}

fn header(mime: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(mime);
    let lower = format!("{}:", name.to_ascii_lowercase());
    let mut out: Option<String> = None;
    for line in text.lines() {
        if let Some(v) = &mut out {
            if line.starts_with([' ', '\t']) {
                v.push(' ');
                v.push_str(line.trim());
                continue;
            }
            break;
        }
        if line.to_ascii_lowercase().starts_with(&lower) {
            out = Some(line[lower.len()..].trim().to_string());
        }
    }
    out
}

#[tokio::test]
async fn each_level_allows_exactly_its_tools_and_a_downgrade_applies_to_the_next_call() {
    let host = TestHost::new("gate", AgentAccess::Off);
    let calls: [(&str, Value); 7] = [
        (
            "create_draft",
            json!({"to": [BO], "subject": "Hi", "body": "Hello"}),
        ),
        ("update_draft", json!({"draftId": "nope"})),
        ("list_drafts", json!({})),
        ("delete_draft", json!({"draftId": "nope"})),
        ("send_draft", json!({"draftId": "nope"})),
        (
            "send_message",
            json!({"to": [BO], "subject": "Hi", "body": "Hello"}),
        ),
        (
            "fetch_attachment",
            json!({"accountId": ADA, "messageId": "m1", "attachmentId": "x"}),
        ),
    ];
    for level in [
        AgentAccess::Off,
        AgentAccess::Read,
        AgentAccess::Draft,
        AgentAccess::Send,
    ] {
        host.set_level(level);
        for (tool, args) in &calls {
            let r = call(&host, tool, args.clone()).await;
            let denied = matches!(&r, Err(e) if e.code == ErrorCode::PermissionDenied);
            assert_eq!(
                denied,
                !permission::allows(level, tool),
                "{level:?} {tool}: {r:?}"
            );
        }
    }
    // Downgrade: the very next call is refused.
    host.set_level(AgentAccess::Draft);
    call(&host, "list_drafts", json!({})).await.unwrap();
    host.set_level(AgentAccess::Read);
    let e = call(&host, "list_drafts", json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(e.message.contains("Settings → Developer → Agents"));
    // Unknown tools are refused whatever the level, and logged as unknown.
    host.set_level(AgentAccess::Send);
    assert!(call(&host, "delete_everything", json!({})).await.is_err());
    assert_eq!(host.audit_lines().last().unwrap()["tool"], "unknown");
}

#[tokio::test]
async fn a_draft_lands_in_the_providers_drafts_and_penguins_and_is_audited_without_content() {
    let host = TestHost::new("create", AgentAccess::Draft);
    let v = call(
        &host,
        "create_draft",
        json!({
            "fromAccount": ADA,
            "to": ["Bo Park <bo@acme.example>"],
            "cc": ["ops@acme.example"],
            "subject": "Walrus budget",
            "body": "Secret body text: the budget is 42.",
        }),
    )
    .await
    .unwrap();
    assert_eq!(v["kind"], "draft");
    let d = data(&v);
    let draft_id = d["draftId"].as_str().unwrap();
    assert_eq!(d["accountId"], ADA);
    assert_eq!(d["to"], json!(["Bo Park <bo@acme.example>"]));
    assert_eq!(d["createdBy"], "mcp");
    // On the provider…
    assert!(host
        .fake()
        .server
        .lock()
        .unwrap()
        .drafts
        .contains_key(draft_id));
    // …in Penguin's store as a draft, at once…
    let mid = host
        .store
        .message_for_draft(ADA, draft_id)
        .unwrap()
        .unwrap();
    let local = host.store.get_message(ADA, &mid).unwrap().unwrap();
    assert!(local.label_ids.contains(&"DRAFT".to_string()));
    assert_eq!(local.subject, "Walrus budget");
    assert_eq!(host.changed.lock().unwrap().len(), 1);
    // …and recorded as the agent's.
    let row = host.store.get_agent_draft(ADA, draft_id).unwrap().unwrap();
    assert_eq!(row.client, "mcp");
    // Listed.
    let list = call(&host, "list_drafts", json!({})).await.unwrap();
    assert_eq!(data(&list)["drafts"][0]["draftId"], draft_id);
    assert_eq!(data(&list)["drafts"][0]["subject"], "Walrus budget");
    // Audited: counts and ids, never the subject, body or addresses.
    let lines = host.audit_lines();
    let create = &lines[0];
    assert_eq!(create["tool"], "create_draft");
    assert_eq!(create["via"], "mcp");
    assert_eq!(create["ok"], true);
    assert_eq!(create["account"], ADA);
    assert_eq!(create["recipientCount"], 2);
    assert_eq!(create["draftId"], draft_id);
    let log = std::fs::read_to_string(host.root.join("logs/mcp-audit.log")).unwrap();
    for secret in [
        "Secret body",
        "Walrus budget",
        "bo@acme.example",
        "ops@acme",
    ] {
        assert!(!log.contains(secret), "{secret} leaked into the audit log");
    }
    // A bad address is refused before anything is saved.
    let e = call(
        &host,
        "create_draft",
        json!({"fromAccount": ADA, "to": ["not an address"]}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
    assert_eq!(host.fake().server.lock().unwrap().drafts.len(), 1);
    // Two accounts and no hint: which one must be said.
    let e = call(&host, "create_draft", json!({"to": [BO]}))
        .await
        .unwrap_err();
    assert!(
        e.message.contains("fromAccount is required"),
        "{}",
        e.message
    );
}

#[tokio::test]
async fn a_reply_threads_with_in_reply_to_and_references_from_the_receiving_account() {
    let host = TestHost::new("reply", AgentAccess::Draft);
    let v = call(
        &host,
        "create_draft",
        json!({"replyToMessageId": "m1", "body": "Sounds good."}),
    )
    .await
    .unwrap();
    let d = data(&v);
    // The account that received it, the original's sender, "Re:", same thread.
    assert_eq!(d["accountId"], ADA);
    assert_eq!(d["to"], json!(["Bo Park <bo@acme.example>"]));
    assert_eq!(d["subject"], "Re: Walrus plan");
    assert_eq!(d["threadId"], "t1");
    assert_eq!(d["replyToMessageId"], "m1");
    let draft_id = d["draftId"].as_str().unwrap().to_string();
    let mime = draft_mime(&host, &draft_id);
    let parsed = MessageParser::default().parse(&mime).unwrap();
    assert_eq!(
        header(&mime, "In-Reply-To").as_deref(),
        Some("<m1@acme.example>")
    );
    let refs = header(&mime, "References").unwrap();
    assert!(
        refs.contains("<m0@acme.example>") && refs.ends_with("<m1@acme.example>"),
        "{refs}"
    );
    assert_eq!(parsed.subject(), Some("Re: Walrus plan"));

    // An update keeps it in the thread (even though the fake provider's
    // reopened draft forgets the reply).
    let v = call(
        &host,
        "update_draft",
        json!({"draftId": draft_id, "body": "Sounds good, ship it."}),
    )
    .await
    .unwrap();
    assert_eq!(data(&v)["threadId"], "t1");
    let mime = draft_mime(&host, data(&v)["draftId"].as_str().unwrap());
    assert_eq!(
        header(&mime, "In-Reply-To").as_deref(),
        Some("<m1@acme.example>")
    );
    assert_eq!(
        data(&v)["subject"],
        "Re: Walrus plan",
        "fields not given are kept"
    );

    // From another account: that account never received it.
    let e = call(
        &host,
        "create_draft",
        json!({"replyToMessageId": "m1", "fromAccount": WORK, "body": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(e.message.contains("account that received"), "{}", e.message);
}

#[tokio::test]
async fn agents_can_only_touch_drafts_an_agent_created() {
    let host = TestHost::new("scope", AgentAccess::Send);
    // The user's own draft, saved the way the composer saves.
    let mine = host
        .fake()
        .save_draft(
            &Draft {
                account_id: ADA.into(),
                to: vec![addr("Bo", BO)],
                cc: vec![],
                bcc: vec![],
                subject: "My half-written draft".into(),
                body_text: "Not ready".into(),
                body_html: None,
                reply_to_thread_id: None,
                reply_to_message_id: None,
                attachments: vec![],
                request_read_receipt: None,
            },
            &addr("Ada", ADA),
            None,
            &[],
        )
        .await
        .unwrap();
    for tool in ["update_draft", "delete_draft", "send_draft"] {
        let e = call(&host, tool, json!({"draftId": mine.draft_id}))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound, "{tool}: {}", e.message);
        assert!(e.message.contains("created by an agent"), "{}", e.message);
    }
    assert!(host
        .fake()
        .server
        .lock()
        .unwrap()
        .drafts
        .contains_key(&mine.draft_id));
    assert!(host.queued.lock().unwrap().is_empty());
    // Not listed either.
    let list = call(&host, "list_drafts", json!({})).await.unwrap();
    assert_eq!(data(&list)["drafts"], json!([]));

    // The agent's own: updated and deleted.
    let v = call(
        &host,
        "create_draft",
        json!({"fromAccount": ADA, "to": [BO], "subject": "A", "body": "b"}),
    )
    .await
    .unwrap();
    let id = data(&v)["draftId"].as_str().unwrap().to_string();
    let v = call(
        &host,
        "update_draft",
        json!({"draftId": id, "subject": "B", "cc": ["ops@acme.example"]}),
    )
    .await
    .unwrap();
    assert_eq!(data(&v)["subject"], "B");
    assert_eq!(data(&v)["to"], json!([BO]), "kept");
    assert_eq!(data(&v)["cc"], json!(["ops@acme.example"]));
    let id = data(&v)["draftId"].as_str().unwrap().to_string();
    let v = call(&host, "delete_draft", json!({"draftId": id}))
        .await
        .unwrap();
    assert_eq!(v["kind"], "draftDeleted");
    assert!(!host.fake().server.lock().unwrap().drafts.contains_key(&id));
    assert!(host.store.get_agent_draft(ADA, &id).unwrap().is_none());
    assert_eq!(
        call(&host, "delete_draft", json!({"draftId": id}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn attachments_by_path_and_by_content_and_the_path_policy() {
    let host = TestHost::new("files", AgentAccess::Draft);
    let plan = host.home.join("Documents/plan.pdf");
    let v = call(
        &host,
        "create_draft",
        json!({
            "fromAccount": ADA, "to": [BO], "subject": "Plan", "body": "Attached.",
            "attachments": [
                {"path": plan.display().to_string()},
                {"filename": "numbers.csv", "contentBase64": base64::engine::general_purpose::STANDARD.encode("a,b\n1,2\n")}
            ]
        }),
    )
    .await
    .unwrap();
    let d = data(&v);
    assert_eq!(d["attachments"][0]["filename"], "plan.pdf");
    assert_eq!(d["attachments"][0]["mimeType"], "application/pdf");
    assert_eq!(d["attachments"][0]["size"], 20);
    assert_eq!(d["attachments"][1]["mimeType"], "text/csv");
    let mime = draft_mime(&host, d["draftId"].as_str().unwrap());
    let parsed = MessageParser::default().parse(&mime).unwrap();
    let names: Vec<String> = parsed
        .attachments()
        .map(|a| a.attachment_name().unwrap_or_default().to_string())
        .collect();
    assert_eq!(names, ["plan.pdf", "numbers.csv"]);
    assert_eq!(
        parsed.attachment(0).unwrap().contents(),
        b"%PDF-1.4 walrus plan"
    );
    assert_eq!(host.audit_lines()[0]["attachmentCount"], 2);

    // A key in ~/.ssh: refused, and nothing saved.
    let before = host.fake().server.lock().unwrap().drafts.len();
    let e = call(
        &host,
        "create_draft",
        json!({"fromAccount": ADA, "to": [BO], "attachments": [{"path": host.home.join(".ssh/id_ed25519").display().to_string()}]}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    // Penguin's own data (the mail database, the agent token).
    std::fs::write(host.paths.data_dir.join("penguin.db"), b"SQLite").unwrap();
    let e = call(
        &host,
        "create_draft",
        json!({"fromAccount": ADA, "to": [BO], "attachments": [{"path": host.paths.data_dir.join("penguin.db").display().to_string()}]}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied, "{}", e.message);
    assert_eq!(host.fake().server.lock().unwrap().drafts.len(), before);
}

#[tokio::test]
async fn markdown_bodies_become_html_with_the_markdown_as_text() {
    let host = TestHost::new("md", AgentAccess::Draft);
    let v = call(
        &host,
        "create_draft",
        json!({"fromAccount": ADA, "to": [BO], "subject": "Plan", "format": "markdown",
               "body": "Hi **Bo**,\n\n- one\n- two\n\nThanks,\nAda"}),
    )
    .await
    .unwrap();
    let mime = draft_mime(&host, data(&v)["draftId"].as_str().unwrap());
    let parsed = MessageParser::default().parse(&mime).unwrap();
    let html = parsed.body_html(0).unwrap();
    assert!(html.contains("<strong>Bo</strong>"), "{html}");
    assert!(html.contains("<li>one</li>"), "{html}");
    assert!(parsed.body_text(0).unwrap().contains("Hi **Bo**"));
}

#[tokio::test]
async fn a_send_waits_in_the_outbox_announced_and_goes_at_the_delay() {
    let host = TestHost::new("send", AgentAccess::Send);
    let v = call(
        &host,
        "send_message",
        json!({"replyToMessageId": "m1", "body": "On it."}),
    )
    .await
    .unwrap();
    assert_eq!(v["kind"], "sendQueued");
    let d = data(&v).clone();
    assert_eq!(d["status"], "queued");
    assert_eq!(d["delaySeconds"], 60);
    assert_eq!(d["sendAt"], host.now + 60_000);
    assert_eq!(d["recipientCount"], 1);
    // Nothing has left yet; the user was told, with who it's going to.
    assert!(host.fake().outgoing().is_empty());
    let q = host.queued.lock().unwrap().clone();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].to, [BO]);
    assert_eq!(q[0].subject, "Re: Walrus plan");
    assert_eq!(q[0].thread_id, "t1");
    // It's an ordinary scheduled send: the composer and outbox see it.
    let scheduled = host.store.list_scheduled_sends(Some(ADA)).unwrap();
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].id, d["scheduleId"]);
    // Sending the same draft again is the same queued send, not a second one.
    let again = call(&host, "send_draft", json!({"draftId": d["draftId"]}))
        .await
        .unwrap();
    assert_eq!(data(&again)["scheduleId"], d["scheduleId"]);
    assert_eq!(host.store.list_scheduled_sends(None).unwrap().len(), 1);
    // A queued draft can't be changed under the user's nose.
    let e = call(
        &host,
        "update_draft",
        json!({"draftId": d["draftId"], "body": "changed"}),
    )
    .await
    .unwrap_err();
    assert!(e.message.contains("queued to send"), "{}", e.message);
    // Before the delay nothing goes; at it, the draft is sent as saved.
    let clients: HashMap<String, Arc<dyn MailProvider>> = HashMap::from([(
        ADA.to_string(),
        Arc::new(host.fake().clone()) as Arc<dyn MailProvider>,
    )]);
    penguin_provider::outbox::run_due(&host.store, &clients, host.now + 59_000)
        .await
        .unwrap();
    assert!(host.fake().outgoing().is_empty());
    penguin_provider::outbox::run_due(&host.store, &clients, host.now + 60_000)
        .await
        .unwrap();
    let out = host.fake().outgoing();
    assert_eq!(out.len(), 1);
    assert_eq!(
        header(&out[0], "In-Reply-To").as_deref(),
        Some("<m1@acme.example>")
    );
    let line = host
        .audit_lines()
        .into_iter()
        .find(|l| l["tool"] == "send_message")
        .unwrap();
    assert_eq!(line["recipientCount"], 1);
    assert!(line["sendAt"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn unknown_recipients_are_refused_before_anything_is_saved() {
    let host = TestHost::new("known", AgentAccess::Send);
    let e = call(
        &host,
        "send_message",
        json!({"fromAccount": ADA, "to": [BO, "exfil@evil.example"], "subject": "data", "body": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(e.message.contains("exfil@evil.example"), "{}", e.message);
    assert!(!e.message.contains(BO), "only the unknown ones are named");
    assert!(host.fake().server.lock().unwrap().drafts.is_empty());
    assert!(host.queued.lock().unwrap().is_empty());
    // My own other account is fine.
    call(
        &host,
        "send_message",
        json!({"fromAccount": ADA, "to": [WORK], "subject": "note", "body": "x"}),
    )
    .await
    .unwrap();
    // A draft to a stranger can be made, but not sent.
    let v = call(
        &host,
        "create_draft",
        json!({"fromAccount": ADA, "to": ["new@partner.example"], "body": "x"}),
    )
    .await
    .unwrap();
    let id = data(&v)["draftId"].clone();
    assert_eq!(
        call(&host, "send_draft", json!({"draftId": id}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    // With the limit off, it goes (after the delay).
    host.settings.lock().unwrap().mcp.send_known_only = false;
    call(&host, "send_draft", json!({"draftId": id}))
        .await
        .unwrap();
    // No recipients at all is never sendable.
    let v = call(
        &host,
        "create_draft",
        json!({"fromAccount": ADA, "body": "x"}),
    )
    .await
    .unwrap();
    let e = call(&host, "send_draft", json!({"draftId": data(&v)["draftId"]}))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
}

#[tokio::test]
async fn lowering_the_level_cancels_queued_agent_sends() {
    let host = TestHost::new("downgrade", AgentAccess::Send);
    let v = call(
        &host,
        "send_message",
        json!({"fromAccount": ADA, "to": [BO], "subject": "s", "body": "x"}),
    )
    .await
    .unwrap();
    let draft_id = data(&v)["draftId"].as_str().unwrap().to_string();
    // What the app does when the user lowers the level (agent_app.rs).
    host.set_level(AgentAccess::Draft);
    let cancelled = host.store.cancel_agent_sends().unwrap();
    assert_eq!(cancelled, vec![(ADA.to_string(), draft_id.clone())]);
    let clients: HashMap<String, Arc<dyn MailProvider>> = HashMap::from([(
        ADA.to_string(),
        Arc::new(host.fake().clone()) as Arc<dyn MailProvider>,
    )]);
    penguin_provider::outbox::run_due(&host.store, &clients, host.now + 3_600_000)
        .await
        .unwrap();
    assert!(host.fake().outgoing().is_empty(), "nothing went");
    // The draft stays for the user; the agent can still edit it at Draft.
    assert!(host
        .fake()
        .server
        .lock()
        .unwrap()
        .drafts
        .contains_key(&draft_id));
    call(
        &host,
        "update_draft",
        json!({"draftId": draft_id, "body": "y"}),
    )
    .await
    .unwrap();
    // …but not send it.
    assert_eq!(
        call(&host, "send_draft", json!({"draftId": draft_id}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
}

#[tokio::test]
async fn deleting_a_queued_draft_cancels_its_send() {
    let host = TestHost::new("delq", AgentAccess::Send);
    let v = call(
        &host,
        "send_message",
        json!({"fromAccount": ADA, "to": [BO], "subject": "s", "body": "x"}),
    )
    .await
    .unwrap();
    call(
        &host,
        "delete_draft",
        json!({"draftId": data(&v)["draftId"]}),
    )
    .await
    .unwrap();
    assert!(host.store.list_scheduled_sends(None).unwrap().is_empty());
}

#[tokio::test]
async fn fetch_attachment_downloads_through_the_app_at_read_level() {
    let host = TestHost::new("fetch", AgentAccess::Read);
    let mut m = host.store.get_message(ADA, "m1").unwrap().unwrap();
    m.attachments = vec![penguin_core::AttachmentMeta {
        id: "att-9".into(),
        filename: "photo.png".into(),
        mime_type: "image/png".into(),
        size: 4,
        content_id: None,
        inline: false,
    }];
    host.store.upsert_messages(&[m]).unwrap();
    host.fake().seed_attachment("m1", "att-9", b"\x89PNG");
    let v = call(
        &host,
        "fetch_attachment",
        json!({"accountId": ADA, "messageId": "m1", "attachmentId": "att-9"}),
    )
    .await
    .unwrap();
    assert_eq!(v["attachment"]["filename"], "photo.png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(v["dataBase64"].as_str().unwrap())
            .unwrap(),
        b"\x89PNG"
    );
    // Now cached for the CLI to read directly.
    assert_eq!(
        crate::attachments::cached_bytes(&host.paths, ADA, "m1", "att-9").unwrap(),
        b"\x89PNG"
    );
    host.set_level(AgentAccess::Off);
    assert_eq!(
        call(
            &host,
            "fetch_attachment",
            json!({"accountId": ADA, "messageId": "m1", "attachmentId": "att-9"})
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::PermissionDenied
    );
}

/// The whole path a CLI takes: the socket, the token, the app's gate.
#[tokio::test]
async fn over_the_socket() {
    let host = TestHost::new("sock", AgentAccess::Draft);
    let listener = ipc::Listener::bind(&host.paths).unwrap();
    let server = tokio::spawn(listener.serve(Arc::new(service(&host))));
    let v = ipc::call(
        &host.paths,
        "cli",
        "create_draft",
        json!({"fromAccount": ADA, "to": [BO], "subject": "via cli", "body": "x"}),
    )
    .await
    .unwrap();
    assert_eq!(v["kind"], "draft");
    assert_eq!(data(&v)["createdBy"], "cli");
    let e = ipc::call(
        &host.paths,
        "cli",
        "send_draft",
        json!({"draftId": data(&v)["draftId"]}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    server.abort();
    let _ = server.await;
}

#[test]
fn addresses_are_parsed_strictly() {
    let a = parse_address("\"Park, Bo\" <bo@acme.example>").unwrap();
    assert_eq!(
        (a.name.as_deref(), a.email.as_str()),
        (Some("Park, Bo"), BO)
    );
    assert_eq!(parse_address(" bo@acme.example ").unwrap().name, None);
    for bad in [
        "",
        "bo",
        "bo@",
        "@acme.example",
        "bo@acme",
        "bo@@acme.example",
        "bo @acme.example",
        "Bo <bo@acme.example",
        "a@b.example, c@d.example",
        "bo@acme.example\r\nBcc: x@evil.example",
    ] {
        assert!(parse_address(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        one_line("Hello\r\nBcc: x@evil.example"),
        "Hello Bcc: x@evil.example"
    );
    assert_eq!(reply_subject("RE: x"), "RE: x");
    assert_eq!(reply_subject("x"), "Re: x");
}
