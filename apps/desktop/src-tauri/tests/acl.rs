//! Every command registered in `generate_handler!` must be listed in
//! build.rs COMMANDS and granted to a window in capabilities/*.json;
//! otherwise the UI gets "Command … not allowed by ACL" at runtime (mock mode
//! does not enforce the ACL, so this is easy to miss). Conversation and
//! compose windows (capabilities/windows.json) get only what reading,
//! triaging and writing need.

use std::collections::BTreeSet;

fn handler_commands() -> BTreeSet<String> {
    let lib = include_str!("../src/lib.rs");
    let start = lib
        .find("generate_handler![")
        .expect("generate_handler! in lib.rs");
    let body = &lib[start + "generate_handler![".len()..];
    let body = &body[..body.find(']').expect("closing ]")];
    body.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.rsplit("::").next().unwrap().to_string())
        .collect()
}

const MAIN: &str = include_str!("../capabilities/default.json");
const WINDOWS: &str = include_str!("../capabilities/windows.json");

fn perm(cmd: &str) -> String {
    format!("\"allow-{}\"", cmd.replace('_', "-"))
}

/// The `allow-*` app-command grants in a capability file.
fn app_grants(caps: &str) -> BTreeSet<String> {
    let v: serde_json::Value = serde_json::from_str(caps).expect("capability JSON");
    v["permissions"]
        .as_array()
        .expect("permissions")
        .iter()
        .filter_map(|p| p.as_str())
        .filter(|p| p.starts_with("allow-"))
        .map(|p| p["allow-".len()..].replace('-', "_"))
        .collect()
}

#[test]
fn every_handler_is_in_build_rs_and_capabilities() {
    let build = include_str!("../build.rs");
    let mut missing = Vec::new();
    for cmd in handler_commands() {
        if !build.contains(&format!("\"{cmd}\"")) {
            missing.push(format!("build.rs COMMANDS: {cmd}"));
        }
        let p = perm(&cmd);
        if !MAIN.contains(&p) && !WINDOWS.contains(&p) {
            missing.push(format!("capabilities/*.json: {p}"));
        }
    }
    assert!(
        missing.is_empty(),
        "commands not allowed by ACL:\n{}",
        missing.join("\n")
    );
}

#[test]
fn secondary_windows_get_only_what_they_need() {
    let v: serde_json::Value = serde_json::from_str(WINDOWS).unwrap();
    let labels: Vec<&str> = v["windows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w.as_str())
        .collect();
    assert_eq!(labels, ["thread-*", "compose-*"], "never the main window");
    let handlers = handler_commands();
    for cmd in app_grants(WINDOWS) {
        assert!(handlers.contains(&cmd), "windows.json grants unknown {cmd}");
    }
    // What stays in the main window: accounts and sign-in, OAuth clients,
    // sync control, notifications, diagnostics and logs, the CLI and MCP,
    // rules editing, the updater, storage.
    for cmd in [
        "add_account",
        "remove_account",
        "reconnect_account",
        "connect_account",
        "set_oauth_client",
        "set_microsoft_client",
        "sign_in_link",
        "sync_now",
        "retry_account_sync",
        "set_notify_context",
        "request_notification_permission",
        "diagnostics",
        "read_log",
        "install_cli",
        "mcp_info",
        "delete_rule",
        "set_allow_hooks",
        "restart_to_update",
        "check_for_updates",
        "free_up_space",
        "clear_avatar_cache",
        "start_model_download",
    ] {
        assert!(
            !WINDOWS.contains(&perm(cmd)),
            "windows.json must not grant {cmd}"
        );
    }
    // The main window has no seed to take.
    assert!(!MAIN.contains(&perm("take_window_seed")));
    assert!(MAIN.contains(&perm("open_window")));
}
