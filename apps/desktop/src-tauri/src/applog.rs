//! Settings → Developer → View log: the tail of penguin.log for the in-app
//! viewer, and `log_client_event`, which records what went wrong on the UI
//! side (a failed command, an error toast, an uncaught script error) so the
//! log holds everything the user saw fail, not just backend warnings.
//!
//! Same rule as the rest of the log: ids, codes and error text only. The UI
//! sends command names and error messages, never subjects or bodies; each
//! field is capped and flattened to one line here as well.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{CmdError, CmdResult};
use crate::logging::LOG_FILE;

/// What the viewer reads: the newest ~1 MB across penguin.log and the rotated penguin.log.1.
const TAIL_BYTES: u64 = 1024 * 1024;
const MAX_FIELD: usize = 600;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogTail {
    /// penguin.log, for "Show in Finder" and bug reports.
    pub path: Option<String>,
    /// Oldest first; continuation lines (panic output) are kept as their own lines.
    pub lines: Vec<String>,
    /// Older lines exist beyond what was read.
    pub truncated: bool,
}

#[tauri::command]
pub async fn read_log(app: AppHandle) -> CmdResult<LogTail> {
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| CmdError::other(format!("no log folder: {e}")))?;
    tauri::async_runtime::spawn_blocking(move || tail_dir(&dir))
        .await
        .map_err(|e| CmdError::other(e.to_string()))?
}

fn tail_dir(dir: &Path) -> CmdResult<LogTail> {
    let current = dir.join(LOG_FILE);
    let (mut text, mut truncated) = tail_file(&current, TAIL_BYTES)?;
    let used = text.len() as u64;
    if used < TAIL_BYTES {
        // Rotated at startup past 5 MB: the rest of the budget comes from the old file.
        let (older, older_truncated) =
            tail_file(&dir.join(format!("{LOG_FILE}.1")), TAIL_BYTES - used)?;
        if !older.is_empty() {
            text = older + &text;
            truncated = older_truncated;
        }
    }
    Ok(LogTail {
        path: current.exists().then(|| current.display().to_string()),
        lines: text.lines().map(str::to_owned).collect(),
        truncated,
    })
}

/// The last `budget` bytes of `path`, starting at a line boundary. A missing file is empty.
fn tail_file(path: &Path, budget: u64) -> CmdResult<(String, bool)> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((String::new(), false)),
        Err(e) => return Err(CmdError::other(format!("could not open the log: {e}"))),
    };
    let len = file
        .metadata()
        .map_err(|e| CmdError::other(e.to_string()))?
        .len();
    let start = len.saturating_sub(budget);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| CmdError::other(e.to_string()))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf)
        .map_err(|e| CmdError::other(format!("could not read the log: {e}")))?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        // Drop the partial first line.
        text = text
            .split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default();
    }
    Ok((text, start > 0))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientEvent {
    /// "error" or "warn".
    pub level: String,
    /// command | toast | window
    pub source: String,
    /// The command name, or where in the UI.
    pub what: String,
    pub message: String,
    /// CommandError code, when there is one.
    #[serde(default)]
    pub code: Option<String>,
}

fn one_line(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_FIELD)
        .collect();
    flat.trim().to_owned()
}

#[tauri::command]
pub fn log_client_event(event: ClientEvent) -> CmdResult<()> {
    let source = one_line(&event.source);
    let what = one_line(&event.what);
    // An Option field is left out of the line when None.
    let code = event
        .code
        .as_deref()
        .map(one_line)
        .filter(|c| !c.is_empty());
    let message = one_line(&event.message);
    if event.level == "error" {
        tracing::error!(target: "penguin_ui", source = %source, what = %what, code = code.as_deref(), "{message}");
    } else {
        tracing::warn!(target: "penguin_ui", source = %source, what = %what, code = code.as_deref(), "{message}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_starts_on_a_line_and_reaches_into_the_rotated_file() {
        let dir = std::env::temp_dir().join(format!("penguin-applog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{LOG_FILE}.1")), "old 1\nold 2\n").unwrap();
        std::fs::write(dir.join(LOG_FILE), "new 1\nnew 2\n").unwrap();
        let t = tail_dir(&dir).unwrap();
        assert_eq!(t.lines, ["old 1", "old 2", "new 1", "new 2"]);
        assert!(!t.truncated);

        let (text, truncated) = tail_file(&dir.join(LOG_FILE), 8).unwrap();
        assert_eq!(text, "new 2\n");
        assert!(truncated);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_log_is_empty() {
        let dir = std::env::temp_dir().join(format!("penguin-applog-none-{}", std::process::id()));
        let t = tail_dir(&dir).unwrap();
        assert!(t.lines.is_empty() && t.path.is_none());
    }

    #[test]
    fn client_fields_are_one_capped_line() {
        assert_eq!(one_line("a\nb\tc"), "a b c");
        assert_eq!(one_line(&"x".repeat(2000)).len(), MAX_FIELD);
    }
}
