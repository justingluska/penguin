//! Files an agent attaches to a draft. The Penguin app reads them (never
//! penguin-cli), from the path the request names or from bytes the request
//! carries (for an agent on another machine, whose files aren't on this
//! Mac).
//!
//! A path must be absolute and, once symlinks are resolved, a regular file
//! inside the home folder that isn't hidden or private:
//! - outside the home folder → refused (copy it into your home folder, or
//!   send the bytes instead);
//! - any hidden component (`~/.ssh/id_ed25519`, `~/.aws/credentials`,
//!   `~/.config/…`, `~/Documents/.env`) → refused;
//! - `~/Library` (Keychains, Cookies, Mail, other apps' data) → refused;
//! - Penguin's own data, config, cache and log folders (the mail database,
//!   the agent token) → refused, wherever they are.
//!
//! The file is opened once and checked on the open handle (same device and
//! inode as the path that passed the checks, a regular file), so swapping
//! the path for a symlink between the check and the read doesn't work.
//! Sizes: 25 MB for all attachments together (Gmail's limit, what every
//! provider's compose enforces), at most 20 files.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use penguin_provider::compose::{OutgoingAttachment, MAX_ATTACHMENTS_BYTES};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CmdError, CmdResult, ErrorCode};

pub const MAX_AGENT_ATTACHMENTS: usize = 20;

/// One attachment: either `path` (a file on the Mac running Penguin), or
/// `filename` + `contentBase64` (the bytes themselves). `mimeType` is
/// optional; it's guessed from the file name.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct AttachmentArg {
    /// Absolute path of a file on the Mac running Penguin, inside the home
    /// folder (not hidden, not in ~/Library).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// File name for `contentBase64` (and to rename a `path` attachment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// The file's bytes, base64 (standard alphabet). For agents that aren't
    /// on the Mac.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Where attachments may come from.
pub struct AttachPolicy {
    /// The user's home folder; None refuses every path.
    pub home: Option<PathBuf>,
    /// Folders that are never read (Penguin's own data and logs).
    pub protected: Vec<PathBuf>,
}

fn denied(msg: String) -> CmdError {
    CmdError::new(ErrorCode::PermissionDenied, msg)
}

/// Resolve every attachment to a `file` attachment with its bytes, in order.
pub fn load_all(
    args: &[AttachmentArg],
    policy: &AttachPolicy,
) -> CmdResult<Vec<OutgoingAttachment>> {
    if args.len() > MAX_AGENT_ATTACHMENTS {
        return Err(CmdError::invalid(format!(
            "at most {MAX_AGENT_ATTACHMENTS} attachments per draft"
        )));
    }
    let mut budget = MAX_ATTACHMENTS_BYTES;
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let (filename, bytes) = match (&a.path, &a.content_base64) {
            (Some(p), None) => {
                let (name, bytes) = read_path(p, policy, budget)?;
                (a.filename.clone().unwrap_or(name), bytes)
            }
            (None, Some(b64)) => {
                let name = a.filename.clone().ok_or_else(|| {
                    CmdError::invalid("an attachment with contentBase64 needs a filename")
                })?;
                let bytes = decode(b64, &name, budget)?;
                (name, bytes)
            }
            _ => {
                return Err(CmdError::invalid(
                    "each attachment needs either a path or a filename with contentBase64",
                ))
            }
        };
        budget -= bytes.len() as u64;
        let filename = crate::ops::sanitize_filename(&filename);
        let mime_type = a
            .mime_type
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| mime_for(&filename).to_string());
        out.push(OutgoingAttachment::File {
            filename,
            mime_type,
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            content_id: None,
        });
    }
    Ok(out)
}

fn too_big(name: &str) -> CmdError {
    CmdError::invalid(format!(
        "{name} doesn't fit: attachments can total at most {} MB",
        MAX_ATTACHMENTS_BYTES / (1024 * 1024)
    ))
}

fn decode(b64: &str, name: &str, budget: u64) -> CmdResult<Vec<u8>> {
    // Base64 is 4 chars per 3 bytes: reject before decoding anything huge.
    if b64.len() as u64 / 4 * 3 > budget + 3 {
        return Err(too_big(name));
    }
    let compact: String = b64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|e| {
            CmdError::invalid(format!("{name}: contentBase64 isn't valid base64 ({e})"))
        })?;
    if bytes.len() as u64 > budget {
        return Err(too_big(name));
    }
    Ok(bytes)
}

/// Why `canonical` may not be attached, if it may not.
fn refusal(canonical: &Path, policy: &AttachPolicy) -> Option<String> {
    let shown = canonical.display();
    for p in &policy.protected {
        if canonical.starts_with(p) {
            return Some(format!(
                "{shown} is in Penguin's own data; it can't be attached"
            ));
        }
    }
    let Some(home) = &policy.home else {
        return Some("no home folder to attach files from".into());
    };
    let Ok(rest) = canonical.strip_prefix(home) else {
        return Some(format!(
            "{shown} is outside your home folder ({}); only files in it can be attached. \
             Copy the file into your home folder, or send its bytes as contentBase64.",
            home.display()
        ));
    };
    let parts: Vec<String> = rest
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.first().map(String::as_str) == Some("Library") {
        return Some(format!(
            "{shown} is in ~/Library (app data, keychains, cookies); it can't be attached"
        ));
    }
    if parts.iter().any(|p| p.starts_with('.')) {
        return Some(format!(
            "{shown} is hidden or inside a hidden folder (like ~/.ssh); it can't be attached"
        ));
    }
    None
}

/// Check and read one file. Returns (its name, its bytes).
fn read_path(raw: &str, policy: &AttachPolicy, budget: u64) -> CmdResult<(String, Vec<u8>)> {
    let given = Path::new(raw);
    if !given.is_absolute() {
        return Err(CmdError::invalid(format!(
            "attachment path {raw} must be absolute (penguin-cli resolves relative ones for you)"
        )));
    }
    let canonical = std::fs::canonicalize(given).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CmdError::not_found(format!("no file at {raw}")),
        std::io::ErrorKind::PermissionDenied => {
            CmdError::invalid(format!("{raw} can't be read (permission denied)"))
        }
        _ => CmdError::invalid(format!("{raw}: {e}")),
    })?;
    if let Some(why) = refusal(&canonical, policy) {
        return Err(denied(why));
    }
    // The resolved path itself: no symlink left, a regular file.
    let before = std::fs::symlink_metadata(&canonical)
        .map_err(|e| CmdError::invalid(format!("{raw}: {e}")))?;
    if !before.file_type().is_file() {
        return Err(CmdError::invalid(format!("{raw} isn't a regular file")));
    }
    let name = given
        .file_name()
        .or_else(|| canonical.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".into());
    if before.len() > budget {
        return Err(too_big(&name));
    }
    let mut file = std::fs::File::open(&canonical).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            CmdError::invalid(format!("{raw} can't be read (permission denied)"))
        }
        _ => CmdError::invalid(format!("{raw}: {e}")),
    })?;
    let opened = file
        .metadata()
        .map_err(|e| CmdError::invalid(format!("{raw}: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (opened.dev(), opened.ino()) != (before.dev(), before.ino()) {
            return Err(CmdError::invalid(format!(
                "{raw} changed while it was being read"
            )));
        }
    }
    if !opened.file_type().is_file() {
        return Err(CmdError::invalid(format!("{raw} isn't a regular file")));
    }
    let mut bytes = Vec::with_capacity(opened.len().min(budget) as usize);
    (&mut file)
        .take(budget + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| CmdError::invalid(format!("{raw}: {e}")))?;
    if bytes.len() as u64 > budget {
        return Err(too_big(&name));
    }
    Ok((name, bytes))
}

/// MIME type from a file name's extension; octet-stream when unknown.
pub fn mime_for(filename: &str) -> &'static str {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "json" => "application/json",
        "xml" => "application/xml",
        "rtf" => "application/rtf",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        policy: AttachPolicy,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A fake home folder with ordinary, hidden and Library files, a
    /// Penguin data dir inside it, and a folder outside it.
    fn fixture(tag: &str) -> Fixture {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "penguin-attach-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let home = root.join("home");
        for d in [
            "Documents",
            ".ssh",
            "Library/Keychains",
            "Documents/.secret",
            "penguin-data",
            "outside",
        ] {
            let dir = if d == "outside" {
                root.join(d)
            } else {
                home.join(d)
            };
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(home.join("Documents/report.pdf"), b"%PDF-1.4 report").unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), b"PRIVATE KEY").unwrap();
        std::fs::write(home.join("Library/Keychains/login.db"), b"keychain").unwrap();
        std::fs::write(home.join("Documents/.secret/notes.txt"), b"secret").unwrap();
        std::fs::write(home.join("penguin-data/penguin.db"), b"SQLite").unwrap();
        std::fs::write(root.join("outside/file.txt"), b"outside").unwrap();
        let home = std::fs::canonicalize(home).unwrap();
        Fixture {
            policy: AttachPolicy {
                protected: vec![home.join("penguin-data")],
                home: Some(home),
            },
            root: std::fs::canonicalize(&root).unwrap(),
        }
    }

    fn path(p: &Path) -> AttachmentArg {
        AttachmentArg {
            path: Some(p.display().to_string()),
            ..AttachmentArg::default()
        }
    }

    fn bytes_of(a: &OutgoingAttachment) -> (String, String, Vec<u8>) {
        match a {
            OutgoingAttachment::File {
                filename,
                mime_type,
                data_base64,
                ..
            } => (
                filename.clone(),
                mime_type.clone(),
                base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .unwrap(),
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_a_regular_file_in_home() {
        let f = fixture("ok");
        let home = f.policy.home.clone().unwrap();
        let out = load_all(&[path(&home.join("Documents/report.pdf"))], &f.policy).unwrap();
        assert_eq!(
            bytes_of(&out[0]),
            (
                "report.pdf".into(),
                "application/pdf".into(),
                b"%PDF-1.4 report".to_vec()
            )
        );
        // A rename and an explicit type are honored.
        let out = load_all(
            &[AttachmentArg {
                filename: Some("Q3 report.pdf".into()),
                mime_type: Some("application/x-custom".into()),
                ..path(&home.join("Documents/report.pdf"))
            }],
            &f.policy,
        )
        .unwrap();
        let (name, mime, _) = bytes_of(&out[0]);
        assert_eq!(
            (name.as_str(), mime.as_str()),
            ("Q3 report.pdf", "application/x-custom")
        );
    }

    #[test]
    fn refuses_paths_outside_home_hidden_library_and_penguins_own() {
        let f = fixture("deny");
        let home = f.policy.home.clone().unwrap();
        let cases = [
            f.root.join("outside/file.txt"),
            home.join(".ssh/id_ed25519"),
            home.join("Documents/.secret/notes.txt"),
            home.join("Library/Keychains/login.db"),
            home.join("penguin-data/penguin.db"),
        ];
        for p in cases {
            let e = load_all(&[path(&p)], &f.policy).unwrap_err();
            assert_eq!(
                e.code,
                ErrorCode::PermissionDenied,
                "{}: {}",
                p.display(),
                e.message
            );
        }
        let e = load_all(&[path(&f.root.join("outside/file.txt"))], &f.policy).unwrap_err();
        assert!(
            e.message.contains("outside your home folder"),
            "{}",
            e.message
        );
        // `..` can't climb out either (the path is resolved first).
        let sneaky = home.join("Documents/../.ssh/id_ed25519");
        assert_eq!(
            load_all(&[path(&sneaky)], &f.policy).unwrap_err().code,
            ErrorCode::PermissionDenied
        );
        // Relative paths, directories and missing files.
        let rel = AttachmentArg {
            path: Some("Documents/report.pdf".into()),
            ..AttachmentArg::default()
        };
        assert_eq!(
            load_all(&[rel], &f.policy).unwrap_err().code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            load_all(&[path(&home.join("Documents"))], &f.policy)
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            load_all(&[path(&home.join("Documents/nope.pdf"))], &f.policy)
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        // No home folder: nothing by path.
        let none = AttachPolicy {
            home: None,
            protected: vec![],
        };
        assert!(load_all(&[path(&home.join("Documents/report.pdf"))], &none).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_judged_by_its_target() {
        let f = fixture("link");
        let home = f.policy.home.clone().unwrap();
        let link = home.join("Documents/innocent.pdf");
        std::os::unix::fs::symlink(home.join(".ssh/id_ed25519"), &link).unwrap();
        let e = load_all(&[path(&link)], &f.policy).unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{}", e.message);
        let ok = home.join("Documents/alias.pdf");
        std::os::unix::fs::symlink(home.join("Documents/report.pdf"), &ok).unwrap();
        let out = load_all(&[path(&ok)], &f.policy).unwrap();
        // Named as the agent named it.
        assert_eq!(bytes_of(&out[0]).0, "alias.pdf");
    }

    #[test]
    fn inline_bytes_sizes_and_counts() {
        let f = fixture("inline");
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"a,b\n1,2\n");
        let out = load_all(
            &[AttachmentArg {
                filename: Some("../../numbers.csv".into()),
                content_base64: Some(b64),
                ..AttachmentArg::default()
            }],
            &f.policy,
        )
        .unwrap();
        let (name, mime, bytes) = bytes_of(&out[0]);
        assert!(!name.contains('/'), "{name}");
        assert_eq!(
            (mime.as_str(), bytes.as_slice()),
            ("text/csv", &b"a,b\n1,2\n"[..])
        );
        // Both or neither of path / content is an error.
        for bad in [
            AttachmentArg::default(),
            AttachmentArg {
                path: Some("/x".into()),
                content_base64: Some("eA==".into()),
                filename: Some("x".into()),
                ..AttachmentArg::default()
            },
            AttachmentArg {
                content_base64: Some("eA==".into()),
                ..AttachmentArg::default()
            },
            AttachmentArg {
                content_base64: Some("not base64!".into()),
                filename: Some("x.bin".into()),
                ..AttachmentArg::default()
            },
        ] {
            assert_eq!(
                load_all(&[bad], &f.policy).unwrap_err().code,
                ErrorCode::InvalidInput
            );
        }
        // Over the total: refused before decoding.
        let huge = "A".repeat((MAX_ATTACHMENTS_BYTES as usize / 3 + 10) * 4);
        let e = load_all(
            &[AttachmentArg {
                filename: Some("big.bin".into()),
                content_base64: Some(huge),
                ..AttachmentArg::default()
            }],
            &f.policy,
        )
        .unwrap_err();
        assert!(e.message.contains("25 MB"), "{}", e.message);
        let many = vec![
            AttachmentArg {
                filename: Some("x.txt".into()),
                content_base64: Some("eA==".into()),
                ..AttachmentArg::default()
            };
            MAX_AGENT_ATTACHMENTS + 1
        ];
        assert!(load_all(&many, &f.policy).is_err());
        // A file over the remaining budget is refused by its size.
        let home = f.policy.home.clone().unwrap();
        let big = home.join("Documents/big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_ATTACHMENTS_BYTES + 1)
            .unwrap();
        assert!(load_all(&[path(&big)], &f.policy)
            .unwrap_err()
            .message
            .contains("25 MB"));
    }

    #[test]
    fn mime_types_by_extension() {
        assert_eq!(mime_for("a.PDF"), "application/pdf");
        assert_eq!(mime_for("photo.jpeg"), "image/jpeg");
        assert_eq!(mime_for("noext"), "application/octet-stream");
        assert_eq!(mime_for("x.weird"), "application/octet-stream");
    }
}
