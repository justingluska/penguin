//! Files kept with composer snippets (Settings → Compose → Snippets): a
//! snippet can attach files when it's inserted. The bytes live on this Mac
//! at `<data dir>/snippets/<sha256>` (0700 directory, 0600 files, written
//! atomically), named by the hash of their content, so saving the same file
//! twice stores it once. `settings.json` keeps only `SnippetFile` metadata
//! (id, name, type, size) in each snippet.
//!
//! Unreferenced files (a snippet deleted, or a file removed from it) are
//! pruned at startup once they're a day old, so a file saved while a
//! snippet is still being edited is never removed under it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use sha2::{Digest, Sha256};
use tauri::State;

use crate::error::{CmdError, CmdResult};
use crate::settings::{Settings, SnippetFile};
use crate::state::{blocking, AppState};

/// The composer's attachment limit (Gmail's 25 MB).
pub const MAX_SNIPPET_FILE_BYTES: usize = 25 * 1024 * 1024;
/// Unreferenced files younger than this are kept (a snippet being edited).
pub const PRUNE_AFTER: Duration = Duration::from_secs(24 * 3600);
const MAX_NAME_CHARS: usize = 200;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("snippets")
}

/// 64 lowercase hex digits (a sha256).
pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The name shown for a file: its last path component, control characters
/// dropped, trimmed, ≤ 200 characters; "file" when nothing is left.
pub fn display_name(filename: &str) -> String {
    let last = filename.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = last.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    let name: String = clean.chars().take(MAX_NAME_CHARS).collect();
    if name.is_empty() || name == "." || name == ".." {
        "file".into()
    } else {
        name
    }
}

/// `type/subtype` lowercased, or `application/octet-stream` for anything else.
pub fn mime_type(m: &str) -> String {
    let m = m.trim().to_ascii_lowercase();
    let ok_part = |p: &str| {
        !p.is_empty()
            && p.len() <= 100
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    };
    match m.split_once('/') {
        Some((a, b)) if ok_part(a) && ok_part(b) => m,
        _ => "application/octet-stream".into(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Store `bytes` under their hash (no-op when already stored).
pub fn store(data_dir: &Path, filename: &str, mime: &str, bytes: &[u8]) -> CmdResult<SnippetFile> {
    if bytes.len() > MAX_SNIPPET_FILE_BYTES {
        return Err(CmdError::invalid(
            "Files kept with a snippet can be up to 25 MB.",
        ));
    }
    let id = hex(&Sha256::digest(bytes));
    let d = dir(data_dir);
    std::fs::create_dir_all(&d)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = d.join(&id);
    if !path.is_file() {
        write_private(&d, &path, bytes)?;
    }
    Ok(SnippetFile {
        id,
        filename: display_name(filename),
        mime_type: mime_type(mime),
        size: bytes.len() as u64,
    })
}

/// Write to a 0600 temp file, fsync, then rename into place.
fn write_private(d: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = d.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The stored bytes of `id`.
pub fn read(data_dir: &Path, id: &str) -> CmdResult<Vec<u8>> {
    if !valid_id(id) {
        return Err(CmdError::invalid("That isn't a snippet file id."));
    }
    match std::fs::read(dir(data_dir).join(id)) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CmdError::not_found(
            "This snippet's file is no longer on this Mac. Edit the snippet to add it again.",
        )),
        Err(e) => Err(e.into()),
    }
}

/// Every file id the snippets refer to.
pub fn referenced(settings: &Settings) -> HashSet<String> {
    settings
        .snippets
        .iter()
        .flat_map(|s| s.attachments.iter().map(|f| f.id.clone()))
        .collect()
}

/// Delete files in `dir` that no snippet refers to and that are older than
/// `older_than` (leftover temp files too). Returns how many were removed.
pub fn prune(dir: &Path, referenced: &HashSet<String>, older_than: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if referenced.contains(&name) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= older_than);
        if !old {
            continue;
        }
        match std::fs::remove_file(e.path()) {
            Ok(()) => removed += 1,
            Err(err) => tracing::warn!(error = %err, "could not remove an unused snippet file"),
        }
    }
    removed
}

/// At startup, in the background: drop files no snippet uses any more.
pub fn spawn_prune(state: Arc<AppState>) {
    tauri::async_runtime::spawn_blocking(move || {
        let keep = referenced(&state.settings.get());
        let n = prune(&dir(&state.paths.data_dir), &keep, PRUNE_AFTER);
        if n > 0 {
            tracing::info!(removed = n, "pruned unused snippet files");
        }
    });
}

#[tauri::command]
pub async fn save_snippet_file(
    state: AppStateRef<'_>,
    filename: String,
    mime_type: String,
    data_base64: String,
) -> CmdResult<SnippetFile> {
    // Base64 is 4 characters per 3 bytes: refuse a too-big file before decoding it.
    if data_base64.len() / 4 * 3 > MAX_SNIPPET_FILE_BYTES + 3 {
        return Err(CmdError::invalid(
            "Files kept with a snippet can be up to 25 MB.",
        ));
    }
    let st = state.inner().clone();
    blocking(move || {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_base64.trim())
            .map_err(|_| CmdError::invalid("That file couldn't be read."))?;
        let file = store(&st.paths.data_dir, &filename, &mime_type, &bytes)?;
        tracing::info!(size = file.size, "snippet file saved");
        Ok(file)
    })
    .await
}

#[tauri::command]
pub async fn read_snippet_file(state: AppStateRef<'_>, id: String) -> CmdResult<String> {
    let st = state.inner().clone();
    blocking(move || {
        let bytes = read(&st.paths.data_dir, &id)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "penguin-snippet-files-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn files_are_stored_once_under_their_hash_and_read_back() {
        let data = temp("store");
        let a = store(
            &data,
            "/Users/sam/Desktop/Price list.pdf",
            "Application/PDF",
            b"%PDF-1.7 prices",
        )
        .unwrap();
        assert!(valid_id(&a.id));
        assert_eq!(a.filename, "Price list.pdf");
        assert_eq!(a.mime_type, "application/pdf");
        assert_eq!(a.size, 15);
        let b = store(&data, "copy.pdf", "application/pdf", b"%PDF-1.7 prices").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(std::fs::read_dir(dir(&data)).unwrap().count(), 1);
        assert_eq!(read(&data, &a.id).unwrap(), b"%PDF-1.7 prices");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir(&data).join(&a.id))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = std::fs::metadata(dir(&data)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn bad_ids_missing_files_and_big_files_are_refused() {
        let data = temp("refuse");
        assert!(read(&data, "../settings.json").is_err());
        assert!(read(&data, &"A".repeat(64)).is_err());
        let missing = read(&data, &"a".repeat(64)).unwrap_err();
        assert_eq!(missing.code, crate::error::ErrorCode::NotFound);
        let big = vec![0u8; MAX_SNIPPET_FILE_BYTES + 1];
        assert!(store(&data, "big.bin", "application/octet-stream", &big).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn names_and_types_are_tidied() {
        assert_eq!(display_name("C:\\docs\\terms.docx"), "terms.docx");
        assert_eq!(display_name("  \u{7}  "), "file");
        assert_eq!(display_name(".."), "file");
        assert_eq!(display_name(&"n".repeat(300)).chars().count(), 200);
        assert_eq!(mime_type("image/PNG"), "image/png");
        assert_eq!(
            mime_type("text/html; charset=utf-8"),
            "application/octet-stream"
        );
        assert_eq!(mime_type(""), "application/octet-stream");
        assert_eq!(
            mime_type("application/vnd.ms-excel"),
            "application/vnd.ms-excel"
        );
    }

    #[test]
    fn prune_keeps_referenced_and_recent_files() {
        let data = temp("prune");
        let kept = store(&data, "a.txt", "text/plain", b"keep me").unwrap();
        let gone = store(&data, "b.txt", "text/plain", b"drop me").unwrap();
        let keep: HashSet<String> = [kept.id.clone()].into();
        // Everything is brand new: nothing goes yet.
        assert_eq!(prune(&dir(&data), &keep, PRUNE_AFTER), 0);
        assert_eq!(prune(&dir(&data), &keep, Duration::ZERO), 1);
        assert!(read(&data, &kept.id).is_ok());
        assert!(read(&data, &gone.id).is_err());
        assert_eq!(prune(&temp("absent"), &keep, Duration::ZERO), 0);
        let _ = std::fs::remove_dir_all(&data);
    }
}
