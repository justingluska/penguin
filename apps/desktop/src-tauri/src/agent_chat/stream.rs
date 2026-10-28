//! Reading a CLI's JSON lines into chat events. Claude Code:
//! `--output-format stream-json --verbose --include-partial-messages`
//! (`system/init`, `stream_event` deltas, whole `assistant` messages with
//! `tool_use` blocks, `result`). Codex: `exec --json` (`thread.started`,
//! `item.started` / `item.completed` with `agent_message`, `mcp_tool_call`,
//! `command_execution`, `turn.completed`, `turn.failed`, `error`). Unknown
//! lines and fields are ignored, so a newer CLI degrades to fewer events.
//! Nothing here logs what the agent says.

use std::collections::HashSet;

use serde_json::Value;

use super::detect::AgentCliKind;

/// What one line said, for the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// The session id to resume with.
    Session(String),
    /// More of the agent's reply.
    Text(String),
    /// The agent used a tool: (tool name without the `mcp__penguin__` prefix).
    Tool(String),
    /// The turn is over. `Some` = it failed, with the CLI's words.
    Finished(Option<String>),
}

#[derive(Default)]
pub struct StreamParser {
    kind: Option<AgentCliKind>,
    /// Claude: text deltas seen in the current message (then the whole
    /// `assistant` message's text is a repeat).
    saw_delta: bool,
    tools_seen: HashSet<String>,
}

impl StreamParser {
    pub fn new(kind: AgentCliKind) -> StreamParser {
        StreamParser { kind: Some(kind), ..Default::default() }
    }

    pub fn line(&mut self, line: &str) -> Vec<Parsed> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return Vec::new();
        };
        match self.kind {
            Some(AgentCliKind::Claude) => self.claude(&v),
            Some(AgentCliKind::Codex) => self.codex(&v),
            None => Vec::new(),
        }
    }

    fn tool(&mut self, id: Option<&str>, name: &str) -> Vec<Parsed> {
        let key = id.map(str::to_string).unwrap_or_else(|| format!("{name}#{}", self.tools_seen.len()));
        if !self.tools_seen.insert(key) {
            return Vec::new();
        }
        vec![Parsed::Tool(short_tool(name).to_string())]
    }

    fn claude(&mut self, v: &Value) -> Vec<Parsed> {
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        match s("type") {
            Some("system") if s("subtype") == Some("init") => {
                s("session_id").map(|id| vec![Parsed::Session(id.into())]).unwrap_or_default()
            }
            Some("stream_event") => {
                let e = &v["event"];
                match e["type"].as_str() {
                    Some("message_start") => {
                        self.saw_delta = false;
                        Vec::new()
                    }
                    Some("content_block_delta") if e["delta"]["type"] == "text_delta" => {
                        let t = e["delta"]["text"].as_str().unwrap_or_default();
                        self.saw_delta = true;
                        if t.is_empty() {
                            Vec::new()
                        } else {
                            vec![Parsed::Text(t.into())]
                        }
                    }
                    _ => Vec::new(),
                }
            }
            Some("assistant") => {
                let mut out = Vec::new();
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") if !self.saw_delta => {
                            if let Some(t) = block["text"].as_str().filter(|t| !t.is_empty()) {
                                out.push(Parsed::Text(t.into()));
                            }
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or("tool");
                            out.extend(self.tool(block["id"].as_str(), name));
                        }
                        _ => {}
                    }
                }
                out
            }
            Some("result") => {
                let failed = v["is_error"].as_bool().unwrap_or(false) || s("subtype").is_some_and(|t| t != "success");
                let mut out = Vec::new();
                if let Some(id) = s("session_id") {
                    out.push(Parsed::Session(id.into()));
                }
                out.push(Parsed::Finished(failed.then(|| {
                    s("result")
                        .filter(|r| !r.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Claude Code stopped ({}).", s("subtype").unwrap_or("error")))
                })));
                out
            }
            _ => Vec::new(),
        }
    }

    fn codex(&mut self, v: &Value) -> Vec<Parsed> {
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        match s("type") {
            Some("thread.started") => s("thread_id").map(|id| vec![Parsed::Session(id.into())]).unwrap_or_default(),
            Some(t @ ("item.started" | "item.completed")) => {
                let item = &v["item"];
                let id = item["id"].as_str();
                match item["type"].as_str() {
                    Some("agent_message") if t == "item.completed" => item["text"]
                        .as_str()
                        .filter(|x| !x.is_empty())
                        .map(|x| vec![Parsed::Text(x.into())])
                        .unwrap_or_default(),
                    Some("mcp_tool_call") => {
                        let name = item["tool"].as_str().unwrap_or("tool");
                        self.tool(id, name)
                    }
                    Some("command_execution") => self.tool(id, "command"),
                    Some("web_search") => self.tool(id, "web_search"),
                    _ => Vec::new(),
                }
            }
            Some("turn.completed") => vec![Parsed::Finished(None)],
            Some("turn.failed") => vec![Parsed::Finished(Some(
                v["error"]["message"].as_str().unwrap_or("Codex stopped.").to_string(),
            ))],
            Some("error") => vec![Parsed::Finished(Some(s("message").unwrap_or("Codex stopped.").to_string()))],
            _ => Vec::new(),
        }
    }
}

/// `mcp__penguin__search` → `search`.
pub fn short_tool(name: &str) -> &str {
    name.strip_prefix("mcp__penguin__").unwrap_or(name)
}

/// What a tool chip says, by Penguin MCP tool name.
pub fn tool_label(name: &str) -> String {
    let label = match name {
        "search" => "Searched mail",
        "list_threads" => "Listed conversations",
        "get_thread" | "thread_context" => "Read a conversation",
        "people" => "Looked up people",
        "ask" => "Asked the index",
        "list_labels" => "Listed labels",
        "list_accounts" => "Listed accounts",
        "get_attachment_text" | "list_attachments" | "get_attachment" => "Read an attachment",
        "create_draft" | "update_draft" => "Wrote a draft",
        "list_drafts" => "Listed drafts",
        "delete_draft" => "Deleted a draft",
        "send_draft" | "send_message" => "Queued a send",
        "create_share_link" => "Made a share link",
        "archive" | "unarchive" => "Archived",
        "mark_read" | "mark_unread" => "Marked read or unread",
        "star" | "unstar" => "Starred",
        "add_label" | "remove_label" => "Changed labels",
        "snooze" | "unsnooze" => "Snoozed",
        "reply_later" | "clear_reply_later" => "Reply Later",
        "trash" | "untrash" => "Moved to Trash",
        "report_spam" | "not_spam" => "Reported spam",
        "command" => "Ran a command",
        "web_search" => "Searched the web",
        other => return format!("Used {other}"),
    };
    label.to_string()
}
