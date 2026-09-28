//! Every command registered in `generate_handler!` must be listed in
//! build.rs COMMANDS and granted in capabilities/default.json; otherwise
//! the UI gets "Command … not allowed by ACL" at runtime (mock mode does
//! not enforce the ACL, so this is easy to miss).

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

#[test]
fn every_handler_is_in_build_rs_and_capabilities() {
    let build = include_str!("../build.rs");
    let caps = include_str!("../capabilities/default.json");
    let mut missing = Vec::new();
    for cmd in handler_commands() {
        if !build.contains(&format!("\"{cmd}\"")) {
            missing.push(format!("build.rs COMMANDS: {cmd}"));
        }
        let perm = format!("\"allow-{}\"", cmd.replace('_', "-"));
        if !caps.contains(&perm) {
            missing.push(format!("capabilities/default.json: {perm}"));
        }
    }
    assert!(
        missing.is_empty(),
        "commands not allowed by ACL:\n{}",
        missing.join("\n")
    );
}
