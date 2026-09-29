//! Attachments and pictures for agents (read level): `list_attachments`,
//! `get_attachment`, `penguin-cli attachments` / `attachment`.
//!
//! - Bytes come from Penguin's caches (attachments the app previewed or
//!   downloaded, inline pictures of messages it showed). Anything else is
//!   downloaded by the running app over the agent socket
//!   (`fetch_attachment`, agent/ipc.rs): this process still never holds
//!   credentials, and the app checks the agent level first.
//! - Pictures go to the model as MCP image content. Above
//!   [`MAX_IMAGE_BYTES`] or [`MAX_IMAGE_EDGE`] they are downscaled (and
//!   re-encoded as JPEG, or PNG when they have transparency), and the
//!   result says so. Text-like files come back as text. Other files come
//!   back as an embedded resource (base64 blob) up to [`MAX_BLOB_BYTES`].
//! - Remote pictures in an HTML body (https URLs) are never fetched here:
//!   loading one tells the sender (and every tracker on it) the user's IP
//!   address and that the mail was read. `list_attachments` lists their
//!   URLs with that warning; tracking pixels are listed apart.

use std::io::Cursor;

use base64::Engine;
use penguin_core::AttachmentMeta;
use rmcp::schemars::JsonSchema;
use serde::Serialize;

use super::AgentCtx;
use crate::attachments::{self, PreviewKind};
use crate::error::{CmdError, CmdResult};

/// Largest picture sent as is: 3.75 MB of bytes is 5 MB of base64, the most
/// MCP clients accept for one image.
pub const MAX_IMAGE_BYTES: usize = 3_750_000;
/// Pictures are scaled to fit this many pixels on the long edge when they
/// are larger (or heavier than MAX_IMAGE_BYTES).
pub const MAX_IMAGE_EDGE: u32 = 2048;
/// Largest non-picture file returned as a blob.
pub const MAX_BLOB_BYTES: usize = 10 * 1024 * 1024;
/// Picture formats MCP clients show.
const MODEL_IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

pub const REMOTE_IMAGES_NOTE: &str = "Remote pictures are not downloaded: fetching one tells the \
    sender (and any tracker) the user's IP address and that the email was read. Don't fetch these \
    URLs unless the user asks.";

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AttachmentInfoOut {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    /// An embedded (cid:) picture shown in the body, not a file.
    pub inline: bool,
    pub content_id: Option<String>,
    /// Penguin already has the bytes; otherwise get_attachment asks the
    /// running app to download them.
    pub cached: bool,
}

/// `attachments` (list_attachments, `penguin-cli attachments --json`).
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AttachmentsOut {
    pub account_id: String,
    pub message_id: String,
    pub thread_id: String,
    /// Files and embedded pictures, in message order.
    pub attachments: Vec<AttachmentInfoOut>,
    /// https pictures the HTML body would load (not fetched).
    pub remote_images: Vec<String>,
    /// Tracking pixels and tiny or hidden remote images (not fetched).
    pub tracking_images: Vec<String>,
    /// Set when there are remote pictures: why they aren't fetched.
    pub remote_images_note: Option<String>,
    /// The body isn't downloaded yet (outside the sync window), so the
    /// attachment list may be incomplete until the message is opened.
    pub body_pending: bool,
}

pub fn list_attachments(
    ctx: &AgentCtx,
    account: &str,
    message_id: &str,
) -> CmdResult<AttachmentsOut> {
    let account = account.trim().to_lowercase();
    let m = ctx
        .store
        .get_message(&account, message_id)?
        .ok_or_else(|| CmdError::not_found(format!("no message {message_id} in {account}")))?;
    let body_pending = ctx.store.is_body_pending(&account, message_id)? == Some(true);
    let attachments = m
        .attachments
        .iter()
        .map(|a| AttachmentInfoOut {
            id: a.id.clone(),
            filename: a.filename.clone(),
            mime_type: a.mime_type.clone(),
            size: a.size,
            inline: a.inline,
            content_id: a.content_id.clone(),
            cached: cached(ctx, &account, message_id, a).is_some(),
        })
        .collect();
    let remote = m
        .body_html
        .as_deref()
        .map(penguin_render::remote_image_urls)
        .unwrap_or_default();
    let note = (!remote.images.is_empty() || !remote.trackers.is_empty())
        .then(|| REMOTE_IMAGES_NOTE.to_string());
    Ok(AttachmentsOut {
        account_id: account,
        message_id: m.id.clone(),
        thread_id: m.thread_id.clone(),
        attachments,
        remote_images: remote.images,
        tracking_images: remote.trackers,
        remote_images_note: note,
        body_pending,
    })
}

/// The stored attachment `attachment_id` of a message (by id, or by the
/// Content-ID of an embedded picture).
pub fn find(
    ctx: &AgentCtx,
    account: &str,
    message_id: &str,
    attachment_id: &str,
) -> CmdResult<AttachmentMeta> {
    find_in(&ctx.store, account, message_id, attachment_id)
}

/// [`find`] on any store (the app's, for `create_share_link`).
pub fn find_in(
    store: &penguin_core::Store,
    account: &str,
    message_id: &str,
    attachment_id: &str,
) -> CmdResult<AttachmentMeta> {
    let account = account.trim().to_lowercase();
    let m = store
        .get_message(&account, message_id)?
        .ok_or_else(|| CmdError::not_found(format!("no message {message_id} in {account}")))?;
    let wanted = attachment_id.trim().trim_start_matches("cid:");
    m.attachments
        .into_iter()
        .find(|a| {
            a.id == attachment_id
                || a.content_id.as_deref().is_some_and(|c| {
                    penguin_render::normalize_cid(c) == penguin_render::normalize_cid(wanted)
                })
        })
        .ok_or_else(|| {
            CmdError::not_found(format!(
                "no attachment {attachment_id} on message {message_id} (list_attachments shows them)"
            ))
        })
}

/// Bytes from Penguin's caches: the attachment cache, then (for an
/// embedded picture) the inline picture cache.
pub fn cached(
    ctx: &AgentCtx,
    account: &str,
    message_id: &str,
    a: &AttachmentMeta,
) -> Option<Vec<u8>> {
    attachments::cached_bytes(&ctx.paths, account, message_id, &a.id).or_else(|| {
        a.content_id.as_deref().and_then(|cid| {
            crate::inline_images::cached_bytes(&ctx.paths, account, message_id, cid)
        })
    })
}

/// What get_attachment hands the model.
#[derive(Debug, PartialEq)]
pub enum Payload {
    /// base64 bytes in a type MCP clients show; `note` says when it was
    /// scaled down.
    Image {
        mime_type: String,
        data_base64: String,
        note: Option<String>,
    },
    Text {
        text: String,
        truncated: bool,
    },
    Blob {
        mime_type: String,
        data_base64: String,
        note: Option<String>,
    },
}

/// Shape `bytes` of attachment `a` for the model.
pub fn payload(a: &AttachmentMeta, bytes: &[u8]) -> CmdResult<Payload> {
    let mime = a.mime_type.to_ascii_lowercase();
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    if mime.starts_with("image/") && mime != "image/svg+xml" {
        match prepare_image(&mime, bytes) {
            Ok(Some((mime_type, data, note))) => {
                return Ok(Payload::Image {
                    mime_type,
                    data_base64: b64(&data),
                    note,
                })
            }
            Ok(None) => {}
            Err(why) => {
                if bytes.len() > MAX_BLOB_BYTES {
                    return Err(too_big(a, bytes.len()));
                }
                return Ok(Payload::Blob {
                    mime_type: a.mime_type.clone(),
                    data_base64: b64(bytes),
                    note: Some(format!("not shown as a picture: {why}")),
                });
            }
        }
    }
    let preview = attachments::from_bytes(a, bytes);
    if let (PreviewKind::Text, Some(text)) = (preview.kind, preview.text) {
        return Ok(Payload::Text {
            text,
            truncated: preview.truncated,
        });
    }
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(too_big(a, bytes.len()));
    }
    Ok(Payload::Blob {
        mime_type: a.mime_type.clone(),
        data_base64: b64(bytes),
        note: None,
    })
}

fn too_big(a: &AttachmentMeta, len: usize) -> CmdError {
    CmdError::invalid(format!(
        "{} is {:.1} MB; files over {} MB aren't returned through MCP. Use `penguin-cli attachment … --out <file>` on the Mac instead.",
        a.filename,
        len as f64 / (1024.0 * 1024.0),
        MAX_BLOB_BYTES >> 20
    ))
}

fn limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(20_000);
    l.max_image_height = Some(20_000);
    l.max_alloc = Some(512 * 1024 * 1024);
    l
}

/// `Ok(Some)` = send this picture; `Ok(None)` = not a picture after all;
/// `Err` = a picture this build can't decode (HEIC, TIFF…).
fn prepare_image(
    mime: &str,
    bytes: &[u8],
) -> Result<Option<(String, Vec<u8>, Option<String>)>, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(limits());
    let Some(format) = reader.format() else {
        return Err(format!("{mime} isn't a format Penguin can read"));
    };
    let sniffed = format.to_mime_type().to_string();
    let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
    if MODEL_IMAGE_TYPES.contains(&sniffed.as_str())
        && bytes.len() <= MAX_IMAGE_BYTES
        && w.max(h) <= MAX_IMAGE_EDGE
    {
        return Ok(Some((sniffed, bytes.to_vec(), None)));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(limits());
    let img = reader.decode().map_err(|e| e.to_string())?;
    let alpha = img.color().has_alpha();
    let mut edge = MAX_IMAGE_EDGE.min(w.max(h));
    for _ in 0..5 {
        let small = img.resize(edge, edge, image::imageops::FilterType::Triangle);
        let mut out = Cursor::new(Vec::new());
        let (mime_out, res) = if alpha {
            (
                "image/png",
                small.write_to(&mut out, image::ImageFormat::Png),
            )
        } else {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
            (
                "image/jpeg",
                image::DynamicImage::ImageRgb8(small.to_rgb8()).write_with_encoder(enc),
            )
        };
        res.map_err(|e| e.to_string())?;
        let data = out.into_inner();
        if data.len() <= MAX_IMAGE_BYTES {
            let note = format!(
                "scaled down from {w}×{h} ({:.1} MB) to {}×{} {}",
                bytes.len() as f64 / (1024.0 * 1024.0),
                small.width(),
                small.height(),
                mime_out.trim_start_matches("image/").to_uppercase()
            );
            return Ok(Some((mime_out.to_string(), data, Some(note))));
        }
        edge = edge * 2 / 3;
    }
    Err("too large even when scaled down".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testkit;

    fn meta(mime: &str, size: u64) -> AttachmentMeta {
        AttachmentMeta {
            id: "a".into(),
            filename: "x".into(),
            mime_type: mime.into(),
            size,
            content_id: None,
            inline: false,
        }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8])
        });
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn small_pictures_pass_through_and_big_ones_are_scaled() {
        let small = png(40, 30);
        match payload(&meta("image/png", small.len() as u64), &small).unwrap() {
            Payload::Image {
                mime_type,
                data_base64,
                note,
            } => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(data_base64)
                        .unwrap(),
                    small
                );
                assert!(note.is_none());
            }
            other => panic!("{other:?}"),
        }
        // Wider than the edge limit: scaled, re-encoded, and said so. The
        // declared type doesn't matter; the bytes are sniffed.
        let big = png(3000, 1000);
        match payload(&meta("application/octet-stream", big.len() as u64), &big).unwrap() {
            Payload::Blob { .. } => {} // not declared as a picture: a file
            other => panic!("{other:?}"),
        }
        match payload(&meta("image/png", big.len() as u64), &big).unwrap() {
            Payload::Image {
                mime_type,
                data_base64,
                note,
            } => {
                assert_eq!(mime_type, "image/jpeg");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .unwrap();
                let img = image::load_from_memory(&bytes).unwrap();
                assert_eq!((img.width(), img.height()), (2048, 683));
                assert!(note.unwrap().starts_with("scaled down from 3000×1000"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn undecodable_pictures_and_other_files() {
        // Says it's a picture, isn't one we can read: a blob with a note.
        match payload(&meta("image/heic", 4), b"ftyp").unwrap() {
            Payload::Blob { note, .. } => assert!(note.unwrap().contains("not shown as a picture")),
            other => panic!("{other:?}"),
        }
        let mut m = meta("text/plain", 5);
        m.filename = "notes.txt".into();
        assert_eq!(
            payload(&m, b"hello").unwrap(),
            Payload::Text {
                text: "hello".into(),
                truncated: false
            }
        );
        let pdf = b"%PDF-1.4".to_vec();
        assert!(matches!(
            payload(&meta("application/pdf", 8), &pdf).unwrap(),
            Payload::Blob { .. }
        ));
        let huge = vec![0u8; MAX_BLOB_BYTES + 1];
        assert!(payload(&meta("application/zip", huge.len() as u64), &huge)
            .unwrap_err()
            .message
            .contains("--out"));
    }

    #[test]
    fn lists_files_embedded_pictures_and_remote_urls_without_fetching() {
        let (ctx, root) = testkit::fixture("files");
        let mut m = ctx.store.get_message(testkit::ADA, "m3").unwrap().unwrap();
        m.body_html = Some(r#"<p>Hi</p><img src="cid:logo@acme"><img src="https://cdn.acme.example/banner.png"><img src="https://t.example/p.gif" width="1" height="1">"#.into());
        m.attachments.push(AttachmentMeta {
            id: "att-2".into(),
            filename: "logo.png".into(),
            mime_type: "image/png".into(),
            size: 10,
            content_id: Some("logo@acme".into()),
            inline: true,
        });
        // The fixture's store is read-only here; write through a second handle.
        penguin_core::Store::open(&ctx.paths.db_path())
            .unwrap()
            .upsert_messages(&[m])
            .unwrap();
        let out = list_attachments(&ctx, testkit::ADA, "m3").unwrap();
        assert_eq!(out.attachments.len(), 2);
        assert!(!out.attachments[0].inline && out.attachments[1].inline);
        assert!(!out.attachments[1].cached);
        assert_eq!(out.remote_images, ["https://cdn.acme.example/banner.png"]);
        assert_eq!(out.tracking_images, ["https://t.example/p.gif"]);
        assert!(out.remote_images_note.unwrap().contains("IP address"));
        // Found by id or by its Content-ID; cached from the inline cache.
        assert_eq!(
            find(&ctx, testkit::ADA, "m3", "cid:logo@acme").unwrap().id,
            "att-2"
        );
        let dir = ctx.paths.inline_cache_dir(testkit::ADA).join("m3");
        std::fs::create_dir_all(&dir).unwrap();
        let hex: String = "logo@acme".bytes().map(|b| format!("{b:02x}")).collect();
        std::fs::write(dir.join(hex), b"PNGBYTES").unwrap();
        let a = find(&ctx, testkit::ADA, "m3", "att-2").unwrap();
        assert_eq!(cached(&ctx, testkit::ADA, "m3", &a).unwrap(), b"PNGBYTES");
        assert!(find(&ctx, testkit::ADA, "m3", "nope").is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
