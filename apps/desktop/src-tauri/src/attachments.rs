//! Attachment byte cache + in-app preview.
//!
//! Layout mirrors the inline-image cache (inline_images.rs): plain files under
//! `<cache>/attachments/<account>/<message id>/<attachment file name>`. An
//! attachment is immutable per (message, attachment id), so the cache needs no
//! invalidation, the OS may wipe it (we refetch), and it is deleted with the
//! account. Preview and "Download" share it, so bytes are fetched once.
//!
//! Preview is deliberately conservative about what it renders:
//! - raster images (PNG/JPEG/GIF/WebP, verified by magic bytes) → data: URL
//! - PDFs (verified by the `%PDF-` header) → data: URL; the UI renders it from
//!   a blob: URL in an unsandboxed iframe (WebKit disables its PDF viewer in
//!   sandboxed frames), which is why the header check matters
//! - text, CSV, JSON, Markdown, source code, and HTML/SVG *as text* → a string
//!   the UI shows escaped in a <pre>; HTML and SVG are never rendered
//! - everything else → `unsupported`, without touching the network

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use penguin_core::{AttachmentMeta, Store};
use penguin_provider::MailProvider;
use serde::Serialize;

use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops::{safe_component, Paths};

/// Images and PDFs travel to the UI as base64 data: URLs; past this they get
/// the "No preview · Download" panel instead.
pub const MAX_BINARY_PREVIEW: u64 = 20 * 1024 * 1024;
/// Text previews show at most this many bytes (the rest is marked truncated).
pub const MAX_TEXT_PREVIEW: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    Image,
    Pdf,
    Text,
    Unsupported,
}

/// Why a preview is `unsupported` (the UI words its panel from this).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NoPreviewReason {
    /// A type Penguin doesn't preview (docx, xlsx, zip, heic, …).
    Type,
    /// Bigger than MAX_BINARY_PREVIEW.
    TooLarge,
    /// Claimed an image/PDF type but the bytes aren't one, or text that is binary.
    Unreadable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentPreview {
    pub kind: PreviewKind,
    /// For images: the sniffed type; otherwise the attachment's own.
    pub mime_type: String,
    pub filename: String,
    pub size: u64,
    /// image: `data:image/...;base64,…`; pdf: `data:application/pdf;base64,…`.
    pub data_url: Option<String>,
    /// text: UTF-8 (lossy), at most MAX_TEXT_PREVIEW bytes.
    pub text: Option<String>,
    /// text: the file was longer than what `text` holds.
    pub truncated: bool,
    /// unsupported: why.
    pub reason: Option<NoPreviewReason>,
}

/// What the metadata alone says we could preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Planned {
    Image,
    Pdf,
    Text,
    None,
}

fn ext_of(filename: &str) -> String {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "jpe", "gif", "webp"];
const IMAGE_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/pjpeg",
    "image/gif",
    "image/webp",
];
const TEXT_EXTS: &[&str] = &[
    "txt",
    "text",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonl",
    "ndjson",
    "md",
    "markdown",
    "rst",
    "xml",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "ics",
    "vcf",
    "srt",
    "vtt",
    "diff",
    "patch",
    "sql",
    "graphql",
    // source code
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "cjs",
    "py",
    "rb",
    "go",
    "java",
    "kt",
    "kts",
    "swift",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "php",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "lua",
    "pl",
    "r",
    "scala",
    "dart",
    "ex",
    "exs",
    "erl",
    "hs",
    "clj",
    "css",
    "scss",
    "sass",
    "less",
    "vue",
    "svelte",
    "gradle",
    "dockerfile",
    "makefile",
    "tf",
    // markup shown as text, never rendered
    "html",
    "htm",
    "xhtml",
    "svg",
];
const TEXT_MIMES: &[&str] = &[
    "application/json",
    "application/xml",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/javascript",
    "application/x-javascript",
    "application/typescript",
    "application/x-sh",
    "application/sql",
    "application/x-ndjson",
    "image/svg+xml",
];

fn plan(att: &AttachmentMeta) -> Planned {
    let mime = att.mime_type.to_ascii_lowercase();
    let ext = ext_of(&att.filename);
    if IMAGE_MIMES.contains(&mime.as_str()) || IMAGE_EXTS.contains(&ext.as_str()) {
        return Planned::Image;
    }
    if mime == "application/pdf" || ext == "pdf" {
        return Planned::Pdf;
    }
    if mime.starts_with("text/")
        || TEXT_MIMES.contains(&mime.as_str())
        || TEXT_EXTS.contains(&ext.as_str())
    {
        return Planned::Text;
    }
    Planned::None
}

/// The image type the bytes really are (never trust the declared type: it
/// ends up in a data: URL).
pub(crate) fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// PDF readers accept the header anywhere in the first 1 KiB.
fn is_pdf(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
}

/// NUL bytes early on mean it isn't text we should show.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn preview_base(att: &AttachmentMeta, kind: PreviewKind) -> AttachmentPreview {
    AttachmentPreview {
        kind,
        mime_type: att.mime_type.clone(),
        filename: att.filename.clone(),
        size: att.size,
        data_url: None,
        text: None,
        truncated: false,
        reason: None,
    }
}

fn unsupported(att: &AttachmentMeta, reason: NoPreviewReason) -> AttachmentPreview {
    AttachmentPreview {
        reason: Some(reason),
        ..preview_base(att, PreviewKind::Unsupported)
    }
}

/// Preview decided from metadata alone, when it needs no bytes (unsupported
/// type, or too large). `None` means: fetch the bytes and call `from_bytes`.
pub fn without_bytes(att: &AttachmentMeta) -> Option<AttachmentPreview> {
    match plan(att) {
        Planned::None => Some(unsupported(att, NoPreviewReason::Type)),
        Planned::Image | Planned::Pdf if att.size > MAX_BINARY_PREVIEW => {
            Some(unsupported(att, NoPreviewReason::TooLarge))
        }
        _ => None,
    }
}

/// Build the preview from the attachment's bytes.
pub fn from_bytes(att: &AttachmentMeta, bytes: &[u8]) -> AttachmentPreview {
    let size = bytes.len() as u64;
    match plan(att) {
        Planned::None => unsupported(att, NoPreviewReason::Type),
        Planned::Image | Planned::Pdf if size > MAX_BINARY_PREVIEW => AttachmentPreview {
            size,
            ..unsupported(att, NoPreviewReason::TooLarge)
        },
        Planned::Image => match sniff_image(bytes) {
            Some(mime) => AttachmentPreview {
                mime_type: mime.to_string(),
                size,
                data_url: Some(data_url(mime, bytes)),
                ..preview_base(att, PreviewKind::Image)
            },
            None => AttachmentPreview {
                size,
                ..unsupported(att, NoPreviewReason::Unreadable)
            },
        },
        Planned::Pdf if is_pdf(bytes) => AttachmentPreview {
            mime_type: "application/pdf".to_string(),
            size,
            data_url: Some(data_url("application/pdf", bytes)),
            ..preview_base(att, PreviewKind::Pdf)
        },
        Planned::Pdf => AttachmentPreview {
            size,
            ..unsupported(att, NoPreviewReason::Unreadable)
        },
        Planned::Text if looks_binary(bytes) => AttachmentPreview {
            size,
            ..unsupported(att, NoPreviewReason::Unreadable)
        },
        Planned::Text => {
            let shown = &bytes[..bytes.len().min(MAX_TEXT_PREVIEW)];
            let text = String::from_utf8_lossy(shown).into_owned();
            // Strip a BOM so it doesn't render as a stray glyph.
            let text = text
                .strip_prefix('\u{feff}')
                .map(str::to_string)
                .unwrap_or(text);
            AttachmentPreview {
                size,
                text: Some(text),
                truncated: bytes.len() > MAX_TEXT_PREVIEW,
                ..preview_base(att, PreviewKind::Text)
            }
        }
    }
}

/// Largest composer file previewed from its bytes: the 25 MB draft limit,
/// with room. Anything bigger isn't a file the composer could send.
pub const MAX_OUTGOING_PREVIEW: usize = 30 * 1024 * 1024;

/// The preview of a file in the composer that isn't on any message yet
/// (picked, pasted or dropped; standard base64, as it's sent), under the
/// same checks as a received attachment: the bytes decide what renders,
/// never the claimed type.
pub fn outgoing_preview(
    filename: String,
    mime_type: String,
    data_base64: &str,
) -> CmdResult<AttachmentPreview> {
    if data_base64.len() / 4 * 3 > MAX_OUTGOING_PREVIEW {
        return Err(CmdError::invalid("That file is too large to preview"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|_| CmdError::invalid("That file's contents are unreadable"))?;
    let att = AttachmentMeta {
        id: String::new(),
        filename,
        mime_type,
        size: bytes.len() as u64,
        content_id: None,
        inline: false,
    };
    Ok(from_bytes(&att, &bytes))
}

// ---------------------------------------------------------------------------
// Byte cache
// ---------------------------------------------------------------------------

pub fn cache_dir(paths: &Paths, account_id: &str) -> PathBuf {
    paths
        .cache_dir
        .join("attachments")
        .join(safe_component(account_id))
}

/// File name for an attachment id: Gmail ids are long base64url strings, so
/// keep a readable prefix plus a hash (same approach as inline cid names).
fn id_file_name(id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let safe = safe_component(id);
    if !safe.is_empty() && safe.len() <= 96 {
        return safe;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    format!("{}-{:016x}", &safe[..safe.len().min(48)], h.finish())
}

fn cache_file(paths: &Paths, account_id: &str, message_id: &str, attachment_id: &str) -> PathBuf {
    cache_dir(paths, account_id)
        .join(safe_component(message_id))
        .join(id_file_name(attachment_id))
}

/// Bytes already in the cache, without touching the network (agent reads).
pub fn cached_bytes(
    paths: &Paths,
    account_id: &str,
    message_id: &str,
    attachment_id: &str,
) -> Option<Vec<u8>> {
    std::fs::read(cache_file(paths, account_id, message_id, attachment_id)).ok()
}

/// Seed the cache with bytes we already hold (e.g. an attachment just
/// uploaded in a saved draft, now known under the draft's new ids), so the
/// next save or send doesn't download them again.
pub fn put_cached(
    paths: &Paths,
    account_id: &str,
    message_id: &str,
    attachment_id: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_atomic(
        &cache_file(paths, account_id, message_id, attachment_id),
        bytes,
    )
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Unique temp name: a preview and a download of the same file may race.
    let tmp = path.with_extension(format!(
        "part{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------
// Lookup + errors (Download, Preview, Open)
// ---------------------------------------------------------------------------

/// The stored attachment `attachment_id` of a message, with a specific error
/// when either is gone. Blocking (SQLite).
pub fn find(
    store: &Store,
    account_id: &str,
    message_id: &str,
    attachment_id: &str,
) -> CmdResult<AttachmentMeta> {
    let message = store.get_message(account_id, message_id)?.ok_or_else(|| {
        CmdError::not_found(
            "This message is no longer in Penguin (it was deleted or moved out of reach). \
             Reopen the conversation to see what's there now.",
        )
    })?;
    message
        .attachments
        .into_iter()
        .find(|a| a.id == attachment_id)
        .ok_or_else(|| {
            CmdError::not_found(
                "Penguin no longer lists this attachment on the message. \
                 Reopen the conversation and try again.",
            )
        })
}

/// Google's own one-line reason from an error body
/// (`{"error":{"message":"…"}}`), when there is one.
fn google_reason(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let msg = v.pointer("/error/message")?.as_str()?.trim();
    (!msg.is_empty()).then(|| msg.chars().take(200).collect())
}

/// A failed attachment download from `service` (the account's provider:
/// "Gmail", "Outlook", "the mail server"), worded for the user.
pub fn fetch_error(service: &str, e: penguin_provider::Error) -> CmdError {
    use penguin_provider::Error as P;
    let svc = crate::commands::cap_first(service);
    let because = |body: &str| {
        google_reason(body)
            .map(|r| format!(" {svc} said: {r}"))
            .unwrap_or_default()
    };
    match e {
        P::Http { status: 404, .. } | P::NotFound(_) => CmdError::not_found(format!(
            "{svc} no longer has this attachment. The message may have been deleted \
             or moved somewhere this account can't see."
        )),
        P::Http { status: 400, body } => CmdError::other(format!(
            "{svc} refused the attachment download (HTTP 400).{}",
            because(&body)
        )),
        P::Http { status, body } if status == 401 || status == 403 => CmdError::other(format!(
            "{svc} denied access to this attachment (HTTP {status}).{}",
            because(&body)
        )),
        P::Http { status, .. } if status >= 500 || status == 429 => CmdError::new(
            ErrorCode::Network,
            format!(
                "{svc} had a problem sending the attachment (HTTP {status}). Try again in a moment."
            ),
        ),
        P::Http { status, body } => CmdError::other(format!(
            "{svc} couldn't send the attachment (HTTP {status}).{}",
            because(&body)
        )),
        P::RateLimited => CmdError::new(
            ErrorCode::Network,
            format!(
                "{svc} is rate-limiting this account right now, so the attachment didn't download. \
                 Try again in a minute."
            ),
        ),
        P::Network(detail) => CmdError::new(
            ErrorCode::Network,
            format!(
                "Couldn't reach {service} to download the attachment ({detail}). \
                 Check your connection and try again."
            ),
        ),
        P::Other(detail) => CmdError::other(format!(
            "{svc} sent the attachment in a form Penguin couldn't read ({detail})."
        )),
        other => CmdError::from(other),
    }
}

/// The attachment's bytes: from the cache, else fetched from the account's
/// provider and cached.
pub async fn bytes(
    paths: &Paths,
    provider: &dyn MailProvider,
    account_id: &str,
    message_id: &str,
    att: &AttachmentMeta,
) -> CmdResult<Vec<u8>> {
    let path = cache_file(paths, account_id, message_id, &att.id);
    let cached = path.clone();
    let hit = crate::state::blocking(move || match std::fs::read(&cached) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            // Unreadable cache entry: refetch rather than fail the preview.
            tracing::warn!(error = %e, "attachment cache read failed; refetching");
            Ok(None)
        }
    })
    .await?;
    if let Some(b) = hit {
        return Ok(b);
    }
    let fetched = match provider.get_attachment(message_id, att).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(account = %account_id, message = %message_id, attachment = %att.id, provider = provider.provider().as_str(), error = %e, "attachment download failed");
            return Err(fetch_error(provider.provider().service_name(), e));
        }
    };
    let to_write = fetched.clone();
    let write_path = path.clone();
    if let Err(e) = crate::state::blocking(move || Ok(write_atomic(&write_path, &to_write)?)).await
    {
        // The bytes are still good; only the cache write failed.
        tracing::warn!(account = %account_id, message = %message_id, error = %e, "could not cache attachment");
    }
    Ok(fetched)
}

pub fn remove_account(paths: &Paths, account_id: &str) {
    match std::fs::remove_dir_all(cache_dir(paths, account_id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(error = %e, "could not delete attachment cache"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, mime: &str, size: u64) -> AttachmentMeta {
        AttachmentMeta {
            id: "a1".into(),
            filename: name.into(),
            mime_type: mime.into(),
            size,
            content_id: None,
            inline: false,
        }
    }

    #[test]
    fn composer_files_preview_from_their_own_bytes() {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let p = outgoing_preview(
            "notes.txt".into(),
            "text/plain".into(),
            &b64(b"Floe agenda"),
        )
        .unwrap();
        assert_eq!(
            (p.kind, p.text.as_deref(), p.size),
            (PreviewKind::Text, Some("Floe agenda"), 11)
        );
        let p = outgoing_preview(
            "Brief.pdf".into(),
            "application/pdf".into(),
            &b64(b"%PDF-1.7\n"),
        )
        .unwrap();
        assert_eq!(p.kind, PreviewKind::Pdf);
        assert!(p
            .data_url
            .unwrap()
            .starts_with("data:application/pdf;base64,"));
        // The claimed type never decides: HTML named .pdf isn't shown as one.
        let p = outgoing_preview(
            "Brief.pdf".into(),
            "application/pdf".into(),
            &b64(b"<html>"),
        )
        .unwrap();
        assert_eq!(
            (p.kind, p.reason),
            (PreviewKind::Unsupported, Some(NoPreviewReason::Unreadable))
        );
        let p = outgoing_preview(
            "Plan.docx".into(),
            "application/octet-stream".into(),
            &b64(b"PK"),
        )
        .unwrap();
        assert_eq!(p.reason, Some(NoPreviewReason::Type));
        assert!(outgoing_preview("x.txt".into(), "text/plain".into(), "not base64!").is_err());
        let huge = "A".repeat(MAX_OUTGOING_PREVIEW / 3 * 4 + 8);
        assert!(outgoing_preview("x.txt".into(), "text/plain".into(), &huge).is_err());
    }

    #[test]
    fn unsupported_types_need_no_bytes() {
        let p = without_bytes(&att(
            "Budget.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            10,
        ))
        .unwrap();
        assert_eq!(p.kind, PreviewKind::Unsupported);
        assert_eq!(p.reason, Some(NoPreviewReason::Type));
        let p = without_bytes(&att("IMG_1.heic", "image/heic", 10)).unwrap();
        assert_eq!(p.kind, PreviewKind::Unsupported);
        let p = without_bytes(&att("big.pdf", "application/pdf", MAX_BINARY_PREVIEW + 1)).unwrap();
        assert_eq!(p.reason, Some(NoPreviewReason::TooLarge));
        assert!(without_bytes(&att("a.png", "image/png", 10)).is_none());
        assert!(without_bytes(&att("notes.md", "application/octet-stream", 10)).is_none());
    }

    #[test]
    fn images_are_sniffed_not_trusted() {
        let png = b"\x89PNG\r\n\x1a\nrest";
        let p = from_bytes(&att("x.jpg", "image/jpeg", 12), png);
        assert_eq!(p.kind, PreviewKind::Image);
        assert_eq!(p.mime_type, "image/png");
        assert!(p.data_url.unwrap().starts_with("data:image/png;base64,"));
        let p = from_bytes(&att("x.png", "image/png", 5), b"<svg onload=alert(1)>");
        assert_eq!(p.kind, PreviewKind::Unsupported);
        assert_eq!(p.reason, Some(NoPreviewReason::Unreadable));
    }

    #[test]
    fn pdf_requires_header() {
        assert_eq!(
            from_bytes(&att("a.pdf", "application/pdf", 9), b"%PDF-1.4\n").kind,
            PreviewKind::Pdf
        );
        assert_eq!(
            from_bytes(&att("a.pdf", "application/pdf", 9), b"<html>").kind,
            PreviewKind::Unsupported
        );
    }

    #[test]
    fn html_and_svg_are_text_never_markup() {
        let p = from_bytes(&att("page.html", "text/html", 20), b"<script>x()</script>");
        assert_eq!(p.kind, PreviewKind::Text);
        assert_eq!(p.text.as_deref(), Some("<script>x()</script>"));
        assert!(p.data_url.is_none());
        let p = from_bytes(
            &att("logo.svg", "image/svg+xml", 20),
            b"<svg onload=x()></svg>",
        );
        assert_eq!(p.kind, PreviewKind::Text);
    }

    #[test]
    fn text_is_capped_and_binary_refused() {
        let big = vec![b'a'; MAX_TEXT_PREVIEW + 10];
        let p = from_bytes(&att("big.log", "text/plain", big.len() as u64), &big);
        assert!(p.truncated);
        assert_eq!(p.text.unwrap().len(), MAX_TEXT_PREVIEW);
        let p = from_bytes(&att("x.txt", "text/plain", 4), b"ab\0c");
        assert_eq!(p.kind, PreviewKind::Unsupported);
    }

    fn message(id: &str, atts: Vec<AttachmentMeta>) -> penguin_core::Message {
        penguin_core::Message {
            account_id: "a@x.example".into(),
            id: id.into(),
            thread_id: "t1".into(),
            date: 1_700_000_000_000,
            from: penguin_core::Address {
                name: Some("Ana Ruiz".into()),
                email: "ana@ruiz.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Deck".into(),
            snippet: String::new(),
            body_text: "see attached".into(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: atts,
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    /// The reported bug: the thread (or preview) holds the attachment ids
    /// from when it rendered; the message is then stored again from a fresh
    /// messages.get, where Gmail hands out new ids. Download and Preview
    /// must still find the attachment by the id the UI has.
    #[test]
    fn ids_the_ui_holds_survive_a_refetch() {
        let store = Store::open_in_memory().unwrap();
        let mut first = att("deck.pdf", "application/pdf", 2048);
        first.id = "ANGjdJ_first".into();
        store
            .upsert_messages(&[message("m1", vec![first.clone()])])
            .unwrap();
        let mut refetched = first.clone();
        refetched.id = "ANGjdJ_second".into();
        store
            .upsert_messages(&[message("m1", vec![refetched])])
            .unwrap();

        let found = find(&store, "a@x.example", "m1", "ANGjdJ_first").unwrap();
        assert_eq!(found.filename, "deck.pdf");

        let e = find(&store, "a@x.example", "m1", "nope").unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(e.message.contains("no longer lists this attachment"));
        let e = find(&store, "a@x.example", "gone", "ANGjdJ_first").unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(e.message.contains("message is no longer in Penguin"));
    }

    #[test]
    fn gmail_failures_read_as_what_happened() {
        use penguin_gmail::Error as G;
        let fetch_error = |e: G| super::fetch_error("Gmail", e.into());
        let e = fetch_error(G::Http {
            status: 400,
            body: r#"{"error":{"code":400,"message":"Invalid attachment token","status":"INVALID_ARGUMENT"}}"#.into(),
        });
        assert_eq!(e.code, ErrorCode::Other);
        assert_eq!(
            e.message,
            "Gmail refused the attachment download (HTTP 400). Gmail said: Invalid attachment token"
        );
        let e = fetch_error(G::Http {
            status: 404,
            body: "message not found".into(),
        });
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(e.message.starts_with("Gmail no longer has this attachment"));
        let e = fetch_error(G::Http {
            status: 503,
            body: "<html>".into(),
        });
        assert_eq!(e.code, ErrorCode::Network);
        assert!(!e.message.contains("<html>"));
        assert_eq!(fetch_error(G::RateLimited).code, ErrorCode::Network);
        assert_eq!(
            fetch_error(G::Network("timed out".into())).code,
            ErrorCode::Network
        );
        assert_eq!(
            fetch_error(G::NeedsReauth("a@x.example".into())).code,
            ErrorCode::NeedsReauth
        );
        let e = fetch_error(G::Other("attachment: bad base64".into()));
        assert!(e.message.contains("couldn't read"));
    }

    #[test]
    fn cache_file_names_are_safe_and_bounded() {
        let long = "A".repeat(400);
        let name = id_file_name(&long);
        assert!(name.len() < 80);
        assert!(!id_file_name("../../etc").contains('/'));
        let root = std::env::temp_dir().join(format!("penguin-att-test-{}", std::process::id()));
        let paths = Paths {
            data_dir: root.clone(),
            config_dir: root.clone(),
            cache_dir: root.clone(),
        };
        let f = cache_file(&paths, "ada@x.example", "m/1", "a1");
        assert!(f.starts_with(root.join("attachments").join("ada@x.example")));
        write_atomic(&f, b"hi").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"hi");
        remove_account(&paths, "ada@x.example");
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
