//! The image viewer's native half (docs/SECURITY.md → "Image viewer").
//!
//! 1. **The click bridge.** An email body is a sandboxed iframe that runs no
//!    script, and WebKit never delivers its clicks to parent listeners. After
//!    a body loads, the UI (MessageBody.tsx) wraps each picture in an anchor
//!    of its own, `penguin-image://open/<nonce>/<index>`, `target=_blank`. A
//!    click becomes a new-window navigation that the link guard (lib.rs)
//!    hands to [`parse_open`]; a well-formed request is emitted to the UI as
//!    `penguin://image-open` `{nonce, index}` and the navigation is always
//!    cancelled. The sanitizer only lets http/https/mailto hrefs through, so
//!    email markup can never produce this scheme; the UI additionally accepts
//!    only a nonce it minted for a live frame and an index it assigned there.
//! 2. **Image bytes for Save / Copy / Open in Preview.** Pictures that are
//!    already in a rendered body: `data:image/…` (inline `cid:` parts the
//!    renderer embedded) or an https URL that the user's image setting let
//!    load. Remote bytes are fetched with the sender-avatar fetcher's
//!    hardening (HTTPS only, public addresses only, no cookies or Referer,
//!    size cap), only on an explicit Save/Copy, and every result must sniff
//!    as an image before it is used.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

use crate::avatars::net::{acceptable_url, FetchError, HttpNet, Net};
use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops;
use crate::state::{blocking, AppState};

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// The scheme of the viewer anchors the UI injects into message frames.
pub const SCHEME: &str = "penguin-image";
/// Emitted to the UI for each well-formed click on such an anchor.
pub const EVENT_IMAGE_OPEN: &str = "penguin://image-open";
/// The largest picture Save / Copy handles (the attachment preview's limit).
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// Viewer indexes are small counters (the UI decorates at most a few hundred).
const MAX_INDEX: u32 = 9999;

/// One click on a viewer anchor: which frame (the UI's per-document nonce)
/// and which of its pictures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenRequest {
    pub nonce: String,
    pub index: u32,
}

/// `penguin-image://open/<32 lowercase hex>/<decimal index>` and nothing
/// else: no userinfo, port, query, fragment, extra segments, padding zeros
/// or uppercase. Anything else is dropped (and the navigation still cancelled).
pub fn parse_open(url: &url::Url) -> Option<OpenRequest> {
    if url.scheme() != SCHEME
        || url.host_str() != Some("open")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut parts = url.path().strip_prefix('/')?.split('/');
    let (nonce, index) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    if nonce.len() != 32 || !nonce.bytes().all(hex) {
        return None;
    }
    if index.is_empty()
        || index.len() > 4
        || !index.bytes().all(|b| b.is_ascii_digit())
        || (index.len() > 1 && index.starts_with('0'))
    {
        return None;
    }
    let index: u32 = index.parse().ok()?;
    (index <= MAX_INDEX).then(|| OpenRequest {
        nonce: nonce.to_string(),
        index,
    })
}

/// Where a picture's bytes come from.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    /// Already in the message (an embedded `cid:` part), decoded.
    Data(Vec<u8>),
    /// A remote image the body loaded; fetched again only on Save/Copy.
    Remote(reqwest::Url),
}

/// The image types penguin-render lets through as `data:` URLs.
const DATA_PREFIXES: &[&str] = &[
    "data:image/png;base64,",
    "data:image/jpeg;base64,",
    "data:image/gif;base64,",
    "data:image/webp;base64,",
    "data:image/bmp;base64,",
    "data:image/avif;base64,",
    "data:image/x-icon;base64,",
];

fn source(url: &str) -> CmdResult<Source> {
    let url = url.trim();
    if let Some(prefix) = DATA_PREFIXES
        .iter()
        .find(|p| url.len() >= p.len() && url[..p.len()].eq_ignore_ascii_case(p))
    {
        let b64 = &url[prefix.len()..];
        if b64.len() > MAX_IMAGE_BYTES / 3 * 4 + 4 {
            return Err(CmdError::invalid("That picture is too large to save"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| CmdError::invalid("That picture's data is damaged"))?;
        return Ok(Source::Data(bytes));
    }
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| CmdError::invalid("Not a picture Penguin can save"))?;
    if parsed.scheme() != "https" || !acceptable_url(&parsed) {
        return Err(CmdError::invalid("Not a picture Penguin can save"));
    }
    Ok(Source::Remote(parsed))
}

/// The image type the bytes really are; the attachment sniffer's four plus
/// the other types the renderer embeds.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    crate::attachments::sniff_image(bytes).or_else(|| {
        if bytes.starts_with(b"BM") && bytes.len() > 14 {
            Some("image/bmp")
        } else if bytes.starts_with(&[0, 0, 1, 0]) && bytes.len() > 6 {
            Some("image/x-icon")
        } else if bytes.len() >= 12
            && &bytes[4..8] == b"ftyp"
            && matches!(&bytes[8..12], b"avif" | b"avis")
        {
            Some("image/avif")
        } else {
            None
        }
    })
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/avif" => "avif",
        _ => "ico",
    }
}

/// A Downloads-safe name whose extension matches what the bytes are.
pub(crate) fn file_name(requested: &str, mime: &str) -> String {
    let clean = ops::sanitize_filename(requested);
    // Drop an existing extension (1–5 letters or digits after the last dot).
    let stem = match clean.rfind('.') {
        Some(i)
            if i > 0
                && (2..=6).contains(&(clean.len() - i))
                && clean[i + 1..].bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            &clean[..i]
        }
        _ => clean.as_str(),
    };
    let stem = if stem.is_empty() || stem == "attachment" {
        "image"
    } else {
        stem
    };
    format!("{stem}.{}", extension(mime))
}

/// A body picture's bytes and sniffed type (see [`source`]): Save, Copy,
/// Save As and the drag-out file (file_export.rs) all go through here.
pub(crate) async fn image_bytes(url: &str) -> CmdResult<(Vec<u8>, &'static str)> {
    fetch(source(url)?).await
}

async fn fetch(source: Source) -> CmdResult<(Vec<u8>, &'static str)> {
    let bytes = match source {
        Source::Data(bytes) => bytes,
        Source::Remote(url) => {
            let host = url.host_str().unwrap_or("").to_string();
            let got = HttpNet::new()
                .get(url.as_str(), MAX_IMAGE_BYTES, None)
                .await;
            match got {
                Ok(bytes) => bytes,
                Err(FetchError::Transient(e)) => {
                    tracing::warn!(host = %host, error = %e, "message image fetch failed");
                    return Err(CmdError::new(
                        ErrorCode::Network,
                        format!("Couldn't reach {host}"),
                    ));
                }
                Err(_) => {
                    tracing::warn!(host = %host, "message image unavailable");
                    return Err(CmdError::not_found(format!(
                        "{host} didn't return the picture"
                    )));
                }
            }
        }
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(CmdError::invalid("That picture is too large to save"));
    }
    let mime =
        sniff(&bytes).ok_or_else(|| CmdError::invalid("That isn't a picture Penguin can save"))?;
    Ok((bytes, mime))
}

/// A remote picture's bytes, for Copy in the viewer (a data: picture is
/// already in the UI). Sniffed: the result is always a real image data: URL.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageBytes {
    pub mime_type: String,
    pub size: u64,
    pub data_url: String,
}

#[tauri::command]
pub async fn fetch_message_image(url: String) -> CmdResult<ImageBytes> {
    let (bytes, mime) = image_bytes(&url).await?;
    Ok(ImageBytes {
        mime_type: mime.to_string(),
        size: bytes.len() as u64,
        data_url: format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ),
    })
}

/// Save a picture from a message body to Downloads (unique name, extension
/// from the sniffed type) and return the path, which open_path then accepts.
#[tauri::command]
pub async fn save_message_image(
    state: AppStateRef<'_>,
    url: String,
    filename: String,
) -> CmdResult<String> {
    let (bytes, mime) = image_bytes(&url).await?;
    let dir = dirs::download_dir()
        .ok_or_else(|| CmdError::other("Couldn't find your Downloads folder"))?;
    let name = file_name(&filename, mime);
    let path = blocking(move || {
        ops::write_unique(&dir, &name, &bytes)
            .map_err(|e| CmdError::other(format!("Couldn't write the file to Downloads: {e}")))
    })
    .await?;
    tracing::info!(kind = mime, "message image saved");
    state.remember_saved_path(path.clone());
    Ok(path.display().to_string())
}

// ---------------------------------------------------------------------------
// Save all images (the message body's "Save all images" button, the viewer's
// and the message's right-click menus)
// ---------------------------------------------------------------------------

/// The most pictures one Save All writes: the UI decorates at most 200 body
/// pictures per message, plus its image attachments.
const MAX_SAVE_ALL: usize = 400;

/// One picture of a message for Save All, as the UI names it: an attachment
/// of that message by id, or a body picture by its source (the same `data:`
/// image or public https URL save_message_image takes).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind")]
pub enum SaveAllItem {
    #[serde(rename = "attachment")]
    Attachment {
        #[serde(rename = "attachmentId")]
        attachment_id: String,
    },
    #[serde(rename = "body")]
    Body { src: String, name: String },
}

/// What Save All did: the folder it made (open_path and reveal_saved_path
/// accept it) and how many of the pictures it wrote there.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedImages {
    pub folder: String,
    pub saved: u32,
    pub total: u32,
}

/// A validated Save All item.
#[derive(Debug)]
enum Planned {
    Attachment { id: String },
    Body { source: Source, name: String },
}

/// Check every item before anything is fetched or written, exactly as the
/// single-picture commands do: an attachment must be one of this message's,
/// and a body source must pass [`source`]. One bad item refuses the batch (a
/// UI that sends one is broken). Exact repeats are dropped.
fn plan_all(items: Vec<SaveAllItem>, attachment_ids: &HashSet<&str>) -> CmdResult<Vec<Planned>> {
    if items.is_empty() {
        return Err(CmdError::invalid("No pictures to save"));
    }
    if items.len() > MAX_SAVE_ALL {
        return Err(CmdError::invalid("Too many pictures to save at once"));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if !seen.insert(item.clone()) {
            continue;
        }
        out.push(match item {
            SaveAllItem::Attachment { attachment_id } => {
                if !attachment_ids.contains(attachment_id.as_str()) {
                    return Err(CmdError::invalid(
                        "Penguin no longer lists one of these attachments on the message",
                    ));
                }
                Planned::Attachment { id: attachment_id }
            }
            SaveAllItem::Body { src, name } => Planned::Body {
                source: source(&src)?,
                name,
            },
        });
    }
    Ok(out)
}

/// The folder Save All creates in Downloads: `<subject> images`, the subject
/// made file-name safe, its whitespace collapsed and cut to 80 characters.
pub(crate) fn folder_name(subject: &str) -> String {
    let words = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    // No leading dot (a hidden folder), however it is spaced.
    let words = words.trim_start_matches(['.', ' ']);
    let stem = if words.is_empty() {
        "Email".to_string()
    } else {
        let clean = ops::sanitize_filename(words);
        clean
            .chars()
            .take(80)
            .collect::<String>()
            .trim_end()
            .to_string()
    };
    format!("{stem} images")
}

/// Create `parent/name`, or `name (1)`, `name (2)`, … if taken. Never reuses
/// or follows an existing entry (create_dir fails on anything already there).
pub(crate) fn create_unique_dir(parent: &Path, name: &str) -> std::io::Result<PathBuf> {
    for n in 0..1000 {
        let candidate = if n == 0 {
            parent.join(name)
        } else {
            parent.join(format!("{name} ({n})"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many folders with this name",
    ))
}

/// One picture's file name and bytes. Attachments come through the same
/// lookup and cache as save_attachment, body pictures through [`fetch`]; both
/// must sniff as an image, and the extension follows the sniffed type.
async fn planned_bytes(
    state: &AppState,
    account_id: &str,
    message_id: &str,
    item: Planned,
) -> CmdResult<(String, Vec<u8>)> {
    let (name, bytes) = match item {
        Planned::Attachment { id } => {
            let (att, bytes) =
                crate::commands::attachment_bytes(state, account_id, message_id, &id).await?;
            (att.filename, bytes)
        }
        Planned::Body { source, name } => (name, fetch(source).await?.0),
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(CmdError::invalid("That picture is too large to save"));
    }
    let mime =
        sniff(&bytes).ok_or_else(|| CmdError::invalid("That isn't a picture Penguin can save"))?;
    Ok((file_name(&name, mime), bytes))
}

/// Save every picture of a message into a new folder in Downloads named after
/// the email (`<subject> images`), each under a unique name. A picture that
/// fails is counted and logged (its kind and error, never a URL); the command
/// fails only when none could be saved, and then leaves no folder behind.
#[tauri::command]
pub async fn save_message_images(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
    items: Vec<SaveAllItem>,
) -> CmdResult<SavedImages> {
    let store = state.store.clone();
    let (acct, mid) = (account_id.clone(), message_id.clone());
    let message = blocking(move || Ok(store.get_message(&acct, &mid)?))
        .await?
        .ok_or_else(|| {
            CmdError::not_found(
                "This message is no longer in Penguin. Reopen the conversation and try again.",
            )
        })?;
    let ids: HashSet<&str> = message.attachments.iter().map(|a| a.id.as_str()).collect();
    let planned = plan_all(items, &ids)?;
    let total = planned.len() as u32;
    let downloads = dirs::download_dir()
        .ok_or_else(|| CmdError::other("Couldn't find your Downloads folder"))?;
    let label = folder_name(&message.subject);

    // The folder is made on the first picture that arrives, so a batch where
    // everything fails leaves nothing in Downloads.
    let mut folder: Option<PathBuf> = None;
    let mut saved = 0u32;
    let mut first_error: Option<CmdError> = None;
    for (index, item) in planned.into_iter().enumerate() {
        let kind = match &item {
            Planned::Attachment { .. } => "attachment",
            Planned::Body {
                source: Source::Data(_),
                ..
            } => "inline",
            Planned::Body { .. } => "remote",
        };
        let written = async {
            let (name, bytes) = planned_bytes(&state, &account_id, &message_id, item).await?;
            let dir = match &folder {
                Some(d) => d.clone(),
                None => {
                    let (parent, label) = (downloads.clone(), label.clone());
                    let d = blocking(move || {
                        create_unique_dir(&parent, &label).map_err(|e| {
                            CmdError::other(format!("Couldn't create a folder in Downloads: {e}"))
                        })
                    })
                    .await?;
                    folder = Some(d.clone());
                    d
                }
            };
            blocking(move || {
                ops::write_unique(&dir, &name, &bytes).map_err(|e| {
                    CmdError::other(format!("Couldn't write the file to Downloads: {e}"))
                })
            })
            .await
        }
        .await;
        match written {
            Ok(path) => {
                saved += 1;
                state.remember_saved_path(path);
            }
            Err(e) => {
                tracing::warn!(account = %account_id, message = %message_id, index, kind, code = ?e.code, error = %e.message, "save all images: a picture failed");
                first_error.get_or_insert(e);
            }
        }
    }
    let folder = match folder {
        Some(folder) if saved > 0 => folder,
        made => {
            if let Some(empty) = made {
                // Made, then every write failed: remove_dir only takes an empty one.
                if let Err(e) = blocking(move || Ok(std::fs::remove_dir(&empty)?)).await {
                    tracing::warn!(error = %e.message, "save all images: couldn't remove the empty folder");
                }
            }
            return Err(first_error.unwrap_or_else(|| CmdError::other("Nothing was saved")));
        }
    };
    tracing::info!(account = %account_id, message = %message_id, saved, total, "message images saved");
    state.remember_saved_path(folder.clone());
    Ok(SavedImages {
        folder: folder.display().to_string(),
        saved,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(s: &str) -> Option<OpenRequest> {
        parse_open(&url::Url::parse(s).ok()?)
    }

    const N: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn well_formed_requests_parse() {
        assert_eq!(
            open(&format!("penguin-image://open/{N}/0")),
            Some(OpenRequest {
                nonce: N.into(),
                index: 0
            })
        );
        assert_eq!(
            open(&format!("penguin-image://open/{N}/9999")).map(|r| r.index),
            Some(9999)
        );
    }

    #[test]
    fn anything_else_is_refused() {
        let bad = [
            format!("https://open/{N}/0"),
            format!("penguin-image://other/{N}/0"),
            format!("penguin-image://open/{N}/00"),
            format!("penguin-image://open/{N}/01"),
            format!("penguin-image://open/{N}/10000"),
            format!("penguin-image://open/{N}/-1"),
            format!("penguin-image://open/{N}/1/2"),
            format!("penguin-image://open/{N}/"),
            format!("penguin-image://open/{N}"),
            format!("penguin-image://open/{N}/1?x=1"),
            format!("penguin-image://open/{N}/1#x"),
            format!("penguin-image://u:p@open/{N}/1"),
            format!("penguin-image://open:8080/{N}/1"),
            format!("penguin-image://open/{}/1", N.to_uppercase()),
            format!("penguin-image://open/{}/1", &N[1..]),
            format!("penguin-image://open/{N}0/1"),
            "penguin-image://open/../../etc/1".to_string(),
            format!("penguin-image://open/{N}/1%2F2"),
            format!("penguin-image:open/{N}/1"),
        ];
        for b in bad {
            assert_eq!(open(&b), None, "{b}");
        }
    }

    #[test]
    fn sources_are_data_images_or_public_https() {
        let png = "data:image/png;base64,iVBORw0KGgo=";
        assert!(matches!(source(png), Ok(Source::Data(_))));
        assert!(matches!(
            source("https://cdn.example/a.png"),
            Ok(Source::Remote(_))
        ));
        for bad in [
            "http://cdn.example/a.png",
            "https://127.0.0.1/a.png",
            "https://localhost/a.png",
            "https://printer.local/a.png",
            "https://user:pw@cdn.example/a.png",
            "https://cdn.example:8443/a.png",
            "file:///etc/passwd",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:text/html;base64,PHNjcmlwdD4=",
            "data:image/png,rawbytes",
            "javascript:alert(1)",
            "avatar://localhost/x.png",
        ] {
            assert!(source(bad).is_err(), "{bad}");
        }
        assert!(source("data:image/png;base64,***").is_err());
    }

    #[tokio::test]
    async fn saved_bytes_must_be_an_image() {
        let html = base64::engine::general_purpose::STANDARD.encode("<script>x</script>");
        let err = fetch(source(&format!("data:image/png;base64,{html}")).unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 16]].concat();
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        let (bytes, mime) = fetch(source(&url).unwrap()).await.unwrap();
        assert_eq!((bytes.len(), mime), (24, "image/png"));
    }

    #[test]
    fn file_names_follow_the_bytes() {
        assert_eq!(file_name("hero.jpg", "image/png"), "hero.png");
        assert_eq!(file_name("../../x", "image/gif"), "_.._x.gif");
        assert_eq!(file_name("report.final", "image/png"), "report.png");
        assert_eq!(file_name("", "image/jpeg"), "image.jpg");
        assert_eq!(file_name("launch", "image/webp"), "launch.webp");
    }

    #[test]
    fn save_all_folder_is_named_after_the_email() {
        assert_eq!(
            folder_name("Penguin launch: final key art"),
            "Penguin launch_ final key art images"
        );
        assert_eq!(folder_name("  a\tb\n  c  "), "a b c images");
        assert_eq!(folder_name("../../etc/passwd"), "_.._etc_passwd images");
        assert_eq!(folder_name(".hidden"), "hidden images");
        assert_eq!(folder_name("a\u{0}b/c\\d"), "a_b_c_d images");
        for empty in ["", "   ", "...", " . . "] {
            assert_eq!(folder_name(empty), "Email images", "{empty:?}");
        }
        let long = format!("{} tail", "x".repeat(300));
        assert_eq!(folder_name(&long), format!("{} images", "x".repeat(80)));
    }

    #[test]
    fn save_all_folders_never_reuse_an_existing_one() {
        let parent = std::env::temp_dir().join(format!(
            "penguin-save-all-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&parent).unwrap();
        let a = create_unique_dir(&parent, "Hi images").unwrap();
        let b = create_unique_dir(&parent, "Hi images").unwrap();
        std::fs::write(parent.join("Hi images (2)"), b"a file, not a folder").unwrap();
        let c = create_unique_dir(&parent, "Hi images").unwrap();
        assert_eq!(a, parent.join("Hi images"));
        assert_eq!(b, parent.join("Hi images (1)"));
        assert_eq!(c, parent.join("Hi images (3)"));
        // Two pictures with the same name land side by side, extension from the bytes.
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 16]].concat();
        let one = ops::write_unique(&a, &file_name("hero.jpg", "image/png"), &png).unwrap();
        let two = ops::write_unique(&a, &file_name("hero.png", "image/png"), &png).unwrap();
        assert_eq!(one, a.join("hero.png"));
        assert_eq!(two, a.join("hero (1).png"));
        std::fs::remove_dir_all(&parent).unwrap();
    }

    #[test]
    fn save_all_items_are_checked_like_single_saves() {
        let ids: HashSet<&str> = ["att-1", "att-2"].into_iter().collect();
        let att = |id: &str| SaveAllItem::Attachment {
            attachment_id: id.into(),
        };
        let body = |src: &str| SaveAllItem::Body {
            src: src.into(),
            name: "x.png".into(),
        };
        let png = "data:image/png;base64,iVBORw0KGgo=";
        let ok = plan_all(
            vec![
                att("att-1"),
                body(png),
                body("https://cdn.example/a.png"),
                att("att-1"),
                body(png),
            ],
            &ids,
        )
        .unwrap();
        assert_eq!(ok.len(), 3, "exact repeats are saved once");

        let code = |items: Vec<SaveAllItem>| plan_all(items, &ids).unwrap_err().code;
        assert_eq!(code(vec![]), ErrorCode::InvalidInput);
        assert_eq!(
            code(vec![body(png); MAX_SAVE_ALL + 1]),
            ErrorCode::InvalidInput
        );
        // Another message's attachment, or a source no single save accepts,
        // refuses the whole batch.
        for bad in [
            vec![att("att-1"), att("other-message-att")],
            vec![body(png), body("http://cdn.example/a.png")],
            vec![body("https://127.0.0.1/a.png")],
            vec![body("https://printer.local/a.png")],
            vec![body("file:///etc/passwd")],
            vec![body("data:image/svg+xml;base64,PHN2Zz4=")],
            vec![body("avatar://localhost/x.png")],
        ] {
            assert_eq!(code(bad), ErrorCode::InvalidInput);
        }
    }

    #[test]
    fn save_all_items_deserialize_from_the_ui_shape() {
        let items: Vec<SaveAllItem> = serde_json::from_str(
            r#"[{"kind":"attachment","attachmentId":"a1"},{"kind":"body","src":"https://cdn.example/a.png","name":"a.png"}]"#,
        )
        .unwrap();
        assert_eq!(
            items,
            vec![
                SaveAllItem::Attachment {
                    attachment_id: "a1".into()
                },
                SaveAllItem::Body {
                    src: "https://cdn.example/a.png".into(),
                    name: "a.png".into()
                }
            ]
        );
        assert!(
            serde_json::from_str::<Vec<SaveAllItem>>(r#"[{"kind":"path","path":"/etc"}]"#).is_err()
        );
    }
}
