//! End-to-end tests of the real `penguin-cli` binary against a throwaway
//! data dir: stdout/stderr discipline, exit codes, JSON envelopes, and the
//! MCP server over actual stdio.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use penguin_core::{Account, Address, Message, Store};

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
