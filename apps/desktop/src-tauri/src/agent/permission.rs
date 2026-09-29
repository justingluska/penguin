//! Which agent level (Settings → Developer → Agents) each tool needs. The
//! Penguin app checks this on every write request it receives over the
//! agent socket (agent/ipc.rs), against its in-memory settings, so a
//! downgrade applies to the very next call. The MCP server uses the same
//! table to list only the tools the current level allows; that is a
//! convenience for the model, not the gate.
//!
//! `create_share_link` has a second gate of its own: share links must be set
//! up and "Let agents (CLI and MCP) create share links" on, in Settings →
//! Share links (`share::agent_gate`). Both are checked by the app.

use crate::error::{CmdError, CmdResult};
use crate::settings::AgentAccess;

/// Tools that read the local index (the MCP server answers these itself,
/// from the read-only database).
pub const READ_TOOLS: [&str; 11] = [
    "search",
    "list_threads",
    "get_thread",
    "thread_context",
    "people",
    "list_labels",
    "list_accounts",
    "ask",
    "get_attachment_text",
    "list_attachments",
    "get_attachment",
];

/// Read requests only the app can answer (over the socket), not MCP tools:
/// `fetch_attachment` downloads an attachment that isn't cached yet.
pub const APP_READS: [&str; 1] = ["fetch_attachment"];

/// Tools that need "Read and draft". Answered by the app over the socket.
pub const DRAFT_TOOLS: [&str; 4] = [
    "create_draft",
    "update_draft",
    "list_drafts",
    "delete_draft",
];

/// Tools that need "Read, draft and send". Answered by the app over the socket.
pub const SEND_TOOLS: [&str; 2] = ["send_draft", "send_message"];

/// Publishing a file: `create_share_link` uploads an attachment to the
/// user's storage and returns a link anyone holding it can open. It needs
/// "Read and draft", not "Read only": the read level promises that nothing
/// is changed and nothing leaves the Mac on the agent's word, and a share
/// link does both (a new object in the user's storage, made with the user's
/// key, reachable from the internet). It doesn't need the send level: it
/// delivers nothing to anyone, the link goes back to the agent only. It
/// also needs the share-link switch (`share::agent_gate`). Answered by the
/// app over the socket.
pub const SHARE_TOOLS: [&str; 1] = ["create_share_link"];

/// The level `tool` needs; None for a name that isn't a tool.
pub fn required(tool: &str) -> Option<AgentAccess> {
    if READ_TOOLS.contains(&tool) || APP_READS.contains(&tool) {
        Some(AgentAccess::Read)
    } else if DRAFT_TOOLS.contains(&tool) || SHARE_TOOLS.contains(&tool) {
        Some(AgentAccess::Draft)
    } else if SEND_TOOLS.contains(&tool) {
        Some(AgentAccess::Send)
    } else {
        None
    }
}

/// Whether `level` allows `tool` (unknown tools never are).
pub fn allows(level: AgentAccess, tool: &str) -> bool {
    required(tool).is_some_and(|need| level >= need)
}

/// The gate: Ok, or a permission-denied error that says which level the
/// tool needs and where to change it.
pub fn check(level: AgentAccess, tool: &str) -> CmdResult<()> {
    let Some(need) = required(tool) else {
        return Err(CmdError::invalid(format!("unknown tool {tool}")));
    };
    if level >= need {
        return Ok(());
    }
    let what = match need {
        _ if SHARE_TOOLS.contains(&tool) => "Creating share links",
        AgentAccess::Send => "Sending",
        AgentAccess::Draft => "Drafting",
        _ => "Reading mail",
    };
    Err(CmdError::denied(format!(
        "{what} isn't allowed: Penguin's agent access is \"{}\" and {tool} needs \"{}\". \
         Only the user can change this, in Penguin → Settings → Developer → Agents.",
        level.label(),
        need.label()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    const LEVELS: [AgentAccess; 4] = [
        AgentAccess::Off,
        AgentAccess::Read,
        AgentAccess::Draft,
        AgentAccess::Send,
    ];

    fn all_tools() -> Vec<&'static str> {
        READ_TOOLS
            .iter()
            .chain(&DRAFT_TOOLS)
            .chain(&SHARE_TOOLS)
            .chain(&SEND_TOOLS)
            .copied()
            .collect()
    }

    /// Each level allows exactly its own tools plus the lower levels'.
    #[test]
    fn each_level_allows_exactly_its_tools() {
        let expect = |level: AgentAccess| -> Vec<&'static str> {
            let mut v: Vec<&str> = Vec::new();
            if level >= AgentAccess::Read {
                v.extend(READ_TOOLS);
            }
            if level >= AgentAccess::Draft {
                v.extend(DRAFT_TOOLS);
                v.extend(SHARE_TOOLS);
            }
            if level >= AgentAccess::Send {
                v.extend(SEND_TOOLS);
            }
            v
        };
        for level in LEVELS {
            let allowed: Vec<&str> = all_tools()
                .into_iter()
                .filter(|t| allows(level, t))
                .collect();
            assert_eq!(allowed, expect(level), "{level:?}");
            for t in all_tools() {
                assert_eq!(check(level, t).is_ok(), allows(level, t), "{level:?} {t}");
            }
        }
        assert!(!allows(AgentAccess::Off, "search"));
        assert_eq!(expect(AgentAccess::Draft).len(), 16);
        // Publishing a file is a write: not at the read level.
        assert!(check(AgentAccess::Read, "create_share_link").is_err());
        assert!(check(AgentAccess::Draft, "create_share_link").is_ok());
        // The app-side read (downloading an uncached attachment) is Read.
        assert!(check(AgentAccess::Off, "fetch_attachment").is_err());
        assert!(check(AgentAccess::Read, "fetch_attachment").is_ok());
    }

    #[test]
    fn a_denial_is_typed_and_says_where_to_change_it() {
        let e = check(AgentAccess::Draft, "send_message").unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert!(e.message.contains("Read and draft"), "{}", e.message);
        assert!(e.message.contains("Read, draft and send"));
        assert!(e.message.contains("Settings → Developer → Agents"));
        let e = check(AgentAccess::Read, "create_draft").unwrap_err();
        assert!(e.message.starts_with("Drafting isn't allowed"));
        let e = check(AgentAccess::Read, "create_share_link").unwrap_err();
        assert!(
            e.message.starts_with("Creating share links isn't allowed"),
            "{}",
            e.message
        );
        assert!(e.message.contains("\"Read and draft\""));
        // Unknown names are never allowed, at any level.
        for level in LEVELS {
            assert!(!allows(level, "delete_everything"));
            assert_eq!(
                check(level, "delete_everything").unwrap_err().code,
                ErrorCode::InvalidInput
            );
        }
    }
}
