//! Agent audit log: one JSON line per call in `<log dir>/mcp-audit.log`.
//!
//! Two writers append to the same file:
//! - the MCP server (`penguin-cli mcp`), for each read tool it answers from
//!   the read-only database: the tool name, its arguments, the outcome and
//!   a result count ([`AuditEntry`]);
//! - the Penguin app, for each request it answers over the agent socket
//!   (drafts, sends, attachment downloads): the tool, which client asked,
//!   the account, how many recipients and attachments, the draft id and
//!   the outcome ([`AppAuditEntry`]).
//!
//! Never message content: no bodies, subjects, addresses or file names.
//! Read arguments are what the agent sent (queries, ids); results are
//! counted, not copied. Settings → Developer shows the latest lines
//! ([`read_recent`]).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

pub const AUDIT_FILE: &str = "mcp-audit.log";
/// Past this size the log is rotated to mcp-audit.log.1 before a write.
const MAX_AUDIT_BYTES: u64 = 5 * 1024 * 1024;
/// Long arguments (e.g. a pasted query) are cut in the log.
const MAX_ARG_CHARS: usize = 200;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry<'a> {
    pub ts: String,
    pub tool: &'a str,
    pub args: serde_json::Value,
    pub ok: bool,
    /// Rows returned (hits, messages, labels…); 0 on error.
    pub result_count: usize,
    pub error_code: Option<&'a str>,
    pub ms: f64,
}

/// A request the app answered over the agent socket. Counts and ids only.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppAuditEntry {
    pub ts: String,
    pub tool: String,
    /// Who asked: `mcp` or `cli`.
    pub via: String,
    pub ok: bool,
    pub error_code: Option<String>,
    pub ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// To + Cc + Bcc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_id: Option<String>,
    /// When a queued send goes (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_at: Option<String>,
    pub result_count: usize,
}

pub struct AuditLog {
    path: Option<PathBuf>,
    lock: Mutex<()>,
}

impl AuditLog {
    /// `dir = None` disables logging (no resolvable log dir).
    pub fn open(dir: Option<&Path>) -> AuditLog {
        let path = dir.and_then(|d| {
            std::fs::create_dir_all(d).ok()?;
            Some(d.join(AUDIT_FILE))
        });
        AuditLog {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn record<T: Serialize>(&self, entry: &T) {
        let Some(path) = &self.path else { return };
        let Ok(mut line) = serde_json::to_string(entry) else {
            return;
        };
        line.push('\n');
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        // The app runs for days: rotate here, not only at open.
        if std::fs::metadata(path)
            .map(|m| m.len() > MAX_AUDIT_BYTES)
            .unwrap_or(false)
        {
            let _ = std::fs::rename(path, rotated(path));
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(line.as_bytes()) {
                    tracing::warn!(error = %e, "could not write the agent audit log");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not open the agent audit log"),
        }
    }
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

/// The newest `limit` entries, newest first, from the log and (when it has
/// fewer) the rotated one. Lines that don't parse are skipped.
pub fn read_recent(path: &Path, limit: usize) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for file in [path.to_path_buf(), rotated(path)] {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines().rev() {
            if out.len() >= limit {
                return out;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                out.push(v);
            }
        }
    }
    out
}

/// Arguments as logged: strings cut to MAX_ARG_CHARS.
pub fn sanitize_args(args: &serde_json::Value) -> serde_json::Value {
    match args {
        serde_json::Value::String(s) if s.chars().count() > MAX_ARG_CHARS => {
            serde_json::Value::String(format!(
                "{}…",
                s.chars().take(MAX_ARG_CHARS).collect::<String>()
            ))
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), sanitize_args(v)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sanitize_args).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_one_private_line_per_call() {
        let dir = std::env::temp_dir().join(format!("penguin-audit-{}", std::process::id()));
        let log = AuditLog::open(Some(&dir));
        let long = "x".repeat(500);
        log.record(&AuditEntry {
            ts: "2026-01-01T00:00:00Z".into(),
            tool: "search",
            args: sanitize_args(&serde_json::json!({"query": long, "limit": 5})),
            ok: true,
            result_count: 3,
            error_code: None,
            ms: 1.0,
        });
        let text = std::fs::read_to_string(dir.join(AUDIT_FILE)).unwrap();
        assert_eq!(text.lines().count(), 1);
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["tool"], "search");
        assert_eq!(v["resultCount"], 3);
        assert_eq!(
            v["args"]["query"].as_str().unwrap().chars().count(),
            MAX_ARG_CHARS + 1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(AUDIT_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn app_entries_carry_counts_and_read_back_newest_first() {
        let dir = std::env::temp_dir().join(format!("penguin-audit-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = AuditLog::open(Some(&dir));
        for i in 0..3 {
            log.record(&AppAuditEntry {
                ts: format!("2026-01-01T00:00:0{i}Z"),
                tool: "create_draft".into(),
                via: "mcp".into(),
                ok: true,
                account: Some("ada@penguin.example".into()),
                recipient_count: Some(2),
                attachment_count: Some(1),
                draft_id: Some(format!("d{i}")),
                result_count: 1,
                ..AppAuditEntry::default()
            });
        }
        let path = dir.join(AUDIT_FILE);
        let recent = read_recent(&path, 2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0]["draftId"], "d2");
        assert_eq!(recent[1]["draftId"], "d1");
        assert_eq!(recent[0]["recipientCount"], 2);
        assert!(
            recent[0].get("sendAt").is_none(),
            "unset fields are left out"
        );
        // Rotated lines are read after the current ones.
        std::fs::rename(&path, rotated(&path)).unwrap();
        log.record(&AppAuditEntry {
            tool: "send_draft".into(),
            ..AppAuditEntry::default()
        });
        let all = read_recent(&path, 10);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0]["tool"], "send_draft");
        assert_eq!(all[3]["draftId"], "d0");
        let _ = std::fs::remove_dir_all(dir);
    }
}
