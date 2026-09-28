//! MCP audit log: one JSON line per tool call in `<log dir>/mcp-audit.log`
//! with the tool name, its arguments, the outcome and a result count.
//! Never message content: arguments are what the agent sent (queries, ids),
//! results are counted, not copied.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

pub const AUDIT_FILE: &str = "mcp-audit.log";
/// Past this size the log is rotated to mcp-audit.log.1 when opened.
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

pub struct AuditLog {
    path: Option<PathBuf>,
    lock: Mutex<()>,
}

impl AuditLog {
    /// `dir = None` disables logging (no resolvable log dir).
    pub fn open(dir: Option<&Path>) -> AuditLog {
        let path = dir.and_then(|d| {
            std::fs::create_dir_all(d).ok()?;
            let path = d.join(AUDIT_FILE);
            if std::fs::metadata(&path)
                .map(|m| m.len() > MAX_AUDIT_BYTES)
                .unwrap_or(false)
            {
                let _ = std::fs::rename(&path, d.join(format!("{AUDIT_FILE}.1")));
            }
            Some(path)
        });
        AuditLog {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn record(&self, entry: &AuditEntry<'_>) {
        let Some(path) = &self.path else { return };
        let Ok(mut line) = serde_json::to_string(entry) else {
            return;
        };
        line.push('\n');
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
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
                    tracing::warn!(error = %e, "could not write the MCP audit log");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not open the MCP audit log"),
        }
    }
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
}
