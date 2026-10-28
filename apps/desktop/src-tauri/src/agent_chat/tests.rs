use std::path::{Path, PathBuf};
use std::sync::Arc;

use penguin_provider::credentials::MemorySecrets;

use super::config::{self, AgentChatConfig, AgentChatConfigPatch, ApiKeys};
use super::detect::{self, AgentCliFound, AgentCliKind, FoundIn};
use super::stream::{tool_label, Parsed, StreamParser};
use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penguin-agent-chat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[cfg(unix)]
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn versions_read_from_both_clis() {
    assert_eq!(detect::parse_version("2.1.289 (Claude Code)\n").as_deref(), Some("2.1.289"));
    assert_eq!(detect::parse_version("codex-cli 0.159.0").as_deref(), Some("0.159.0"));
    assert_eq!(detect::parse_version("\nv1.2.3").as_deref(), Some("1.2.3"));
    assert_eq!(detect::parse_version("command not found"), None);
}

#[cfg(unix)]
#[test]
fn finds_clis_on_path_then_in_install_spots() {
    let home = tmp("detect");
    let on_path = home.join("tools");
    script(&on_path.join("codex"), "echo 'codex-cli 0.159.0'");
    // Not on PATH, but where the native installer puts it.
    script(&home.join(".claude/local/claude"), "echo '2.1.289 (Claude Code)'");
    // A non-executable file is no CLI.
    std::fs::create_dir_all(home.join(".local/bin")).unwrap();
    std::fs::write(home.join(".local/bin/codex"), "").unwrap();
    std::fs::write(home.join(".claude.json"), r#"{"oauthAccount":{"x":1}}"#).unwrap();

    let path = on_path.display().to_string();
    let env = |_: &str| None;
    let found = detect::detect(&home, Some(&path), None, &env, &|_| false);
    assert_eq!(found.len(), 2);
    let claude = found.iter().find(|f| f.kind == AgentCliKind::Claude).unwrap();
    assert_eq!(claude.found_in, FoundIn::Common);
    assert_eq!(claude.version.as_deref(), Some("2.1.289"));
    assert_eq!(claude.signed_in, Some(true));
    let codex = found.iter().find(|f| f.kind == AgentCliKind::Codex).unwrap();
    assert_eq!(codex.found_in, FoundIn::Path);
    assert_eq!(codex.path, on_path.join("codex").display().to_string());
    // No ~/.codex/auth.json and no key.
    assert_eq!(codex.signed_in, Some(false));

    // An API key in the environment counts as signed in.
    let env = |k: &str| (k == "CODEX_API_KEY").then(|| "sk-test".to_string());
    assert!(detect::looks_signed_in(AgentCliKind::Codex, &home, &env, false));
    // A custom path wins over the search.
    let custom = home.join("elsewhere/claude");
    script(&custom, "echo '9.9.9 (Claude Code)'");
    let found = detect::detect(&home, Some(&path), Some((AgentCliKind::Claude, &custom)), &|_| None, &|_| false);
    let claude = found.iter().find(|f| f.kind == AgentCliKind::Claude).unwrap();
    assert_eq!((claude.found_in, claude.version.as_deref()), (FoundIn::Custom, Some("9.9.9")));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn config_patches_validate_and_keys_stay_in_the_keychain() {
    let dir = tmp("config");
    let path = config::path_in(&dir);
    assert_eq!(config::load(&path), AgentChatConfig::default());
    assert!(!config::load(&path).enabled, "off until the user turns it on");

    let cfg = config::apply(
        AgentChatConfig::default(),
        &AgentChatConfigPatch { enabled: Some(true), cli: Some("codex".into()), custom_path: Some("/opt/x/codex".into()) },
    )
    .unwrap();
    assert_eq!((cfg.enabled, cfg.cli, cfg.custom_path.as_deref()), (true, Some(AgentCliKind::Codex), Some("/opt/x/codex")));
    assert!(config::apply(cfg.clone(), &AgentChatConfigPatch { cli: Some("gpt".into()), ..Default::default() }).is_err());
    assert!(config::apply(cfg.clone(), &AgentChatConfigPatch { custom_path: Some("bin/claude".into()), ..Default::default() }).is_err());
    let cleared = config::apply(cfg.clone(), &AgentChatConfigPatch { cli: Some("".into()), custom_path: Some(" ".into()), ..Default::default() }).unwrap();
    assert_eq!((cleared.cli, cleared.custom_path), (None, None));

    let mem = Arc::new(MemorySecrets::default());
    let keys = ApiKeys::new(Box::new(mem.clone()));
    let mut cfg = cfg;
    keys.set(&mut cfg, AgentCliKind::Claude, Some("  sk-ant-test  ")).unwrap();
    assert!(cfg.claude_key_saved && !cfg.codex_key_saved);
    assert_eq!(keys.get(AgentCliKind::Claude).unwrap().as_deref(), Some("sk-ant-test"));
    assert!(keys.set(&mut cfg, AgentCliKind::Claude, Some("two words")).is_err());
    config::save(&path, &cfg).unwrap();
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("sk-ant"), "the key is never in the file");
    assert_eq!(config::load(&path), cfg);
    keys.set(&mut cfg, AgentCliKind::Claude, None).unwrap();
    assert!(!cfg.claude_key_saved);
    assert_eq!(keys.get(AgentCliKind::Claude).unwrap(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

fn found(kind: AgentCliKind) -> AgentCliFound {
    AgentCliFound { kind, path: format!("/usr/local/bin/{}", kind.as_str()), version: None, signed_in: Some(true), found_in: FoundIn::Path }
}

#[test]
fn what_blocks_a_chat_and_which_cli_it_uses() {
    let on = AgentChatConfig { enabled: true, ..Default::default() };
    let both = [found(AgentCliKind::Claude), found(AgentCliKind::Codex)];
    assert!(blocked_reason(&AgentChatConfig::default(), Some(&both), AgentAccess::Read, true).unwrap().contains("Chat is off"));
    assert!(blocked_reason(&on, Some(&both), AgentAccess::Off, true).unwrap().contains("Agents are off"));
    assert!(blocked_reason(&on, Some(&both), AgentAccess::Read, false).unwrap().contains("penguin-cli"));
    assert!(blocked_reason(&on, Some(&[]), AgentAccess::Read, true).unwrap().contains("didn't find"));
    assert_eq!(blocked_reason(&on, Some(&both), AgentAccess::Draft, true), None);

    assert_eq!(pick(&on, Some(&both)).unwrap().kind, AgentCliKind::Claude);
    let codex = AgentChatConfig { cli: Some(AgentCliKind::Codex), ..on.clone() };
    assert_eq!(pick(&codex, Some(&both)).unwrap().kind, AgentCliKind::Codex);
    // The chosen one isn't installed: use what is.
    assert_eq!(pick(&codex, Some(&both[..1])).unwrap().kind, AgentCliKind::Claude);
}

#[test]
fn the_prompt_names_the_conversation_by_id_only() {
    let ctx = AgentChatContext { account_id: "sam@northwind.example".into(), thread_id: "18c2f".into() };
    let p = preamble(Some(&ctx), true, AgentAccess::Draft);
    assert!(p.contains("untrusted"));
    assert!(p.contains("thread `18c2f`") && p.contains("sam@northwind.example"));
    assert!(p.contains("not send"));
    let later = preamble(None, false, AgentAccess::Draft);
    assert!(!later.contains("untrusted") && later.contains("no conversation open"));
}

#[test]
fn claude_runs_with_penguins_tools_only() {
    let a = cli_args(AgentCliKind::Claude, Path::new("/tmp/mcp.json"), Path::new("/App/penguin-cli"), Some("abc"));
    let joined = a.join(" ");
    for want in ["-p", "--output-format stream-json", "--strict-mcp-config", "--allowedTools mcp__penguin", "--mcp-config /tmp/mcp.json", "--resume abc"] {
        assert!(joined.contains(want), "{want} in {joined}");
    }
    // `--tools ""`: no built-in shell or file tools.
    let i = a.iter().position(|x| x == "--tools").unwrap();
    assert_eq!(a[i + 1], "");
    let mcp: serde_json::Value = serde_json::from_str(&mcp_config_json(Path::new("/Applications/Pen guin.app/penguin-cli"))).unwrap();
    assert_eq!(mcp["mcpServers"]["penguin"]["command"], "/Applications/Pen guin.app/penguin-cli");
    assert_eq!(mcp["mcpServers"]["penguin"]["args"][0], "mcp");
}

#[test]
fn codex_gets_penguin_as_its_mcp_server() {
    let a = cli_args(AgentCliKind::Codex, Path::new("/tmp/mcp.json"), Path::new("/App/Pen \"guin\"/penguin-cli"), None);
    assert_eq!(&a[..2], ["exec", "--json"]);
    assert!(a.contains(&r#"mcp_servers.penguin.command="/App/Pen \"guin\"/penguin-cli""#.to_string()));
    assert!(a.contains(&r#"sandbox_mode="read-only""#.to_string()));
    assert_eq!(a.last().unwrap(), "-");
    let resumed = cli_args(AgentCliKind::Codex, Path::new("/tmp/mcp.json"), Path::new("/p"), Some("01a1"));
    assert_eq!(&resumed[..3], ["exec", "resume", "01a1"]);
}

/// Lines as Claude Code 2.1 prints them (trimmed to the fields we read).
#[test]
fn claude_stream_becomes_chat_events() {
    let mut p = StreamParser::new(AgentCliKind::Claude);
    let lines = [
        r#"{"type":"system","subtype":"init","session_id":"s-1","tools":["mcp__penguin__search"]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me "}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"look."}}}"#,
        r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Let me look."},{"type":"tool_use","id":"tu1","name":"mcp__penguin__search","input":{"query":"invoice"}}]}}"#,
        r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"tu1","name":"mcp__penguin__search","input":{}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"..."}]}}"#,
        r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m2"}}}"#,
        r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Found 2."}]}}"#,
        r#"{"type":"rate_limit_event","rate_limit_info":{}}"#,
        "not json",
        r#"{"type":"result","subtype":"success","is_error":false,"result":"Found 2.","session_id":"s-1"}"#,
    ];
    let got: Vec<Parsed> = lines.iter().flat_map(|l| p.line(l)).collect();
    assert_eq!(
        got,
        vec![
            Parsed::Session("s-1".into()),
            Parsed::Text("Let me ".into()),
            Parsed::Text("look.".into()),
            Parsed::Tool("search".into()),
            // No deltas for m2 (e.g. partial messages off): the whole text.
            Parsed::Text("Found 2.".into()),
            Parsed::Session("s-1".into()),
            Parsed::Finished(None),
        ]
    );
    let mut p = StreamParser::new(AgentCliKind::Claude);
    assert_eq!(
        p.line(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#),
        vec![Parsed::Finished(Some("Not logged in · Please run /login".into()))]
    );
}

/// Lines as codex-cli 0.159 prints them.
#[test]
fn codex_stream_becomes_chat_events() {
    let mut p = StreamParser::new(AgentCliKind::Codex);
    let lines = [
        r#"{"type":"thread.started","thread_id":"01a10c93"}"#,
        r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Configured value for `permission_profile` is disallowed"}}"#,
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","server":"penguin","tool":"thread_context","status":"in_progress"}}"#,
        r#"{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","server":"penguin","tool":"thread_context","status":"completed"}}"#,
        r#"{"type":"item.completed","item":{"id":"item_2","type":"reasoning","text":"thinking"}}"#,
        r#"{"type":"item.completed","item":{"id":"item_3","type":"agent_message","text":"Priya asked for the deck by Friday."}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#,
    ];
    let got: Vec<Parsed> = lines.iter().flat_map(|l| p.line(l)).collect();
    assert_eq!(
        got,
        vec![
            Parsed::Session("01a10c93".into()),
            Parsed::Tool("thread_context".into()),
            Parsed::Text("Priya asked for the deck by Friday.".into()),
            Parsed::Finished(None),
        ]
    );
    let mut p = StreamParser::new(AgentCliKind::Codex);
    assert_eq!(
        p.line(r#"{"type":"turn.failed","error":{"message":"401 Unauthorized"}}"#),
        vec![Parsed::Finished(Some("401 Unauthorized".into()))]
    );
}

#[test]
fn tool_chips_say_what_happened() {
    assert_eq!(tool_label("search"), "Searched mail");
    assert_eq!(tool_label("thread_context"), "Read a conversation");
    assert_eq!(tool_label("create_draft"), "Wrote a draft");
    assert_eq!(tool_label("frobnicate"), "Used frobnicate");
}

#[test]
fn events_serialize_as_the_ui_reads_them() {
    let e = AgentChatEvent::Tool { chat_id: "c1".into(), name: "search".into(), label: "Searched mail".into() };
    assert_eq!(
        serde_json::to_value(e).unwrap(),
        serde_json::json!({ "kind": "tool", "chatId": "c1", "name": "search", "label": "Searched mail" })
    );
    let d = AgentChatEvent::Done { chat_id: "c1".into(), error: None, stopped: true };
    assert_eq!(serde_json::to_value(d).unwrap()["stopped"], true);
}
