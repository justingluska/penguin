//! Outgoing MIME for every provider (moved here from penguin-gmail when the
//! provider seam landed; penguin-gmail re-exports it as `compose`). Gmail
//! sends the bytes with messages.send, IMAP over SMTP, Microsoft Graph as a
//! MIME sendMail.
//!
//! Built with `mail-builder`, which RFC 2047-encodes non-ASCII display names
//! and subjects and picks quoted-printable/base64 for bodies as needed.
//!
//! Structure: `multipart/alternative` (text, then HTML) for the body; with
//! inline images the HTML becomes `multipart/related` (HTML, then each image
//! with its `Content-ID`), so text-only readers never see them as files;
//! files wrap it all in `multipart/mixed`. Which attachments are inline is
//! [`crate::inline::plan`].
//!
//! Bcc: Gmail's `messages.send` takes its recipients from the To, Cc and Bcc
//! headers of the raw message, and removes the Bcc header from the copy
//! recipients receive (the sender's Sent copy keeps it). So the Bcc header
//! MUST be present here — dropping it would silently not deliver to Bcc
//! recipients — and it is not leaked to To/Cc recipients.

use base64::Engine;
use mail_builder::headers::address::Address as MbAddress;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::date::Date;
use mail_builder::headers::raw::Raw;
use mail_builder::mime::{BodyPart, MimePart};
use mail_builder::MessageBuilder;
use penguin_core::Address;
use serde::{Deserialize, Serialize};

/// Mirrors `Draft` in apps/desktop/src/lib/types.ts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub account_id: String,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub body_text: String,
    /// Optional HTML alternative (composer output). Plain text is always sent.
    pub body_html: Option<String>,
    /// Reply context: thread to attach to and the message being replied to.
    pub reply_to_thread_id: Option<String>,
    pub reply_to_message_id: Option<String>,
    /// Files to send. The caller resolves each to bytes (see
    /// [`AttachmentBytes`]); compose does no network I/O.
    #[serde(default)]
    pub attachments: Vec<OutgoingAttachment>,
    /// Ask the recipients' mail apps for a read receipt (RFC 8098: a
    /// `Disposition-Notification-To` header naming the From address; Graph:
    /// `isReadReceiptRequested`). Only `Some(true)` asks. None = not decided
    /// by the caller: `send_message` fills it from Settings → Privacy "Ask
    /// for read receipts"; rule forwards and replies Penguin sends by itself
    /// never ask.
    #[serde(default)]
    pub request_read_receipt: Option<bool>,
}

/// An attachment on an outgoing message. Mirrors `OutgoingAttachment` in
/// apps/desktop/src/lib/types.ts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OutgoingAttachment {
    /// A local file, base64 (standard alphabet) in the payload.
    File {
        filename: String,
        mime_type: String,
        data_base64: String,
        /// Set for an inline image: the id the HTML shows it by
        /// (`<img src="cid:…">`). See [`crate::inline`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_id: Option<String>,
    },
    /// An attachment of an existing stored message, in any provider
    /// (forwarding, or a draft's saved attachment); the caller fetches its
    /// bytes. Named `gmail` on the wire for compatibility with saved UI state.
    Gmail {
        message_id: String,
        attachment_id: String,
        filename: String,
        mime_type: String,
        size: u64,
        /// The account `message_id` is stored in, when it isn't the sending
        /// account (a forward sent from another account). Absent = the
        /// sending account.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
        /// Set for an inline image, as on `File`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_id: Option<String>,
    },
}

impl OutgoingAttachment {
    pub fn filename(&self) -> &str {
        match self {
            OutgoingAttachment::File { filename, .. }
            | OutgoingAttachment::Gmail { filename, .. } => filename,
        }
    }

    pub fn mime_type(&self) -> &str {
        match self {
            OutgoingAttachment::File { mime_type, .. }
            | OutgoingAttachment::Gmail { mime_type, .. } => mime_type,
        }
    }

    /// The inline image id, if this is (meant to be) an inline image.
    pub fn content_id(&self) -> Option<&str> {
        match self {
            OutgoingAttachment::File { content_id, .. }
            | OutgoingAttachment::Gmail { content_id, .. } => content_id.as_deref(),
        }
    }
}

/// An attachment resolved to its bytes, ready to encode.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentBytes {
    pub filename: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
    /// The draft attachment's `content_id` (an inline image).
    pub content_id: Option<String>,
}

/// Gmail's limit on a message's attachments (decoded bytes).
pub const MAX_ATTACHMENTS_BYTES: u64 = 25 * 1024 * 1024;

/// Decode a `file` attachment's payload; `None` for `gmail` references,
/// whose bytes the caller must fetch.
pub fn decode_file_attachment(a: &OutgoingAttachment) -> crate::Result<Option<AttachmentBytes>> {
    match a {
        OutgoingAttachment::File {
            filename,
            mime_type,
            data_base64,
            content_id,
        } => {
            let compact: String = data_base64
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(compact)
                .map_err(|e| {
                    crate::Error::Other(format!("attachment {filename}: invalid base64: {e}"))
                })?;
            Ok(Some(AttachmentBytes {
                filename: filename.clone(),
                mime_type: mime_type.clone(),
                bytes,
                content_id: content_id.clone(),
            }))
        }
        OutgoingAttachment::Gmail { .. } => Ok(None),
    }
}

/// `type/subtype` made of RFC 2045 token characters, lowercased; anything
/// else becomes application/octet-stream.
fn safe_mime_type(m: &str) -> String {
    let m = m.trim().to_ascii_lowercase();
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    match m.split_once('/') {
        Some((t, sub)) if token(t) && token(sub) => m,
        _ => "application/octet-stream".into(),
    }
}

/// `Content-Disposition` for an attachment (`kind` attachment or inline):
/// the exact name as RFC 2231 `filename*=UTF-8''…`, plus `filename=` for
/// readers that only look there, RFC 2047-encoded when non-ASCII (what
/// Gmail and Outlook themselves emit, so either parameter yields the real
/// name whichever a reader prefers).
fn content_disposition(kind: &str, filename: &str) -> String {
    let name = filename.trim();
    let name = if name.is_empty() { "attachment" } else { name };
    let fallback = if name
        .bytes()
        .all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\')
    {
        name.to_string()
    } else {
        format!(
            "=?UTF-8?B?{}?=",
            base64::engine::general_purpose::STANDARD.encode(name)
        )
    };
    let mut encoded = String::with_capacity(name.len() * 3);
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn mb_addr(a: &Address) -> MbAddress<'_> {
    let name = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
    MbAddress::new_address(name, a.email.trim())
}

fn mb_list(list: &[Address]) -> Option<MbAddress<'_>> {
    let items: Vec<MbAddress> = list
        .iter()
        .filter(|a| !a.email.trim().is_empty())
        .map(mb_addr)
        .collect();
    (!items.is_empty()).then(|| MbAddress::new_list(items))
}

fn bare_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// `<random@sender-domain>`; avoids leaking the machine hostname, which is
/// what mail-builder would use by default.
fn new_message_id(from: &Address) -> String {
    let domain = from
        .email
        .rsplit_once('@')
        .map(|(_, d)| d.trim())
        .filter(|d| !d.is_empty())
        .unwrap_or("penguin.invalid");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!(
        "{now:x}.{:016x}{:016x}@{domain}",
        fastrand::u64(..),
        fastrand::u64(..)
    )
}

/// Build RFC 5322 bytes (multipart/alternative when html present), with
/// In-Reply-To/References taken from the replied-to message headers.
///
/// `in_reply_to` is the parent's Message-ID and `references` the parent's
/// References; per RFC 5322 §3.6.4 the parent id is appended to References
/// if it isn't already the last entry. Ids may be given with or without
/// angle brackets.
///
/// `file` attachments are decoded here; a draft with `gmail` attachment
/// references must go through [`build_rfc822_with_attachments`] instead.
pub fn build_rfc822(
    draft: &Draft,
    from: &Address,
    in_reply_to: Option<&str>,
    references: &[String],
) -> crate::Result<Vec<u8>> {
    let attachments = decode_all_files(draft)?;
    build_rfc822_with_attachments(draft, from, in_reply_to, references, &attachments)
}

/// [`build_rfc822`] without the recipient check, for saving Gmail drafts
/// (which may not have recipients yet).
pub fn build_rfc822_draft(
    draft: &Draft,
    from: &Address,
    in_reply_to: Option<&str>,
    references: &[String],
) -> crate::Result<Vec<u8>> {
    let attachments = decode_all_files(draft)?;
    build_rfc822_draft_with_attachments(draft, from, in_reply_to, references, &attachments)
}

/// Decode every attachment of `draft`, which must all be `file`s.
fn decode_all_files(draft: &Draft) -> crate::Result<Vec<AttachmentBytes>> {
    draft
        .attachments
        .iter()
        .map(|a| {
            decode_file_attachment(a)?.ok_or_else(|| {
                crate::Error::Other(format!(
                    "attachment {} must be fetched before sending (use build_rfc822_with_attachments)",
                    a.filename()
                ))
            })
        })
        .collect()
}

/// [`build_rfc822`] with every attachment already resolved to bytes (one
/// entry per `draft.attachments`, in order; `file` ones via
/// [`decode_file_attachment`], `gmail` ones fetched by the caller).
pub fn build_rfc822_with_attachments(
    draft: &Draft,
    from: &Address,
    in_reply_to: Option<&str>,
    references: &[String],
    attachments: &[AttachmentBytes],
) -> crate::Result<Vec<u8>> {
    if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
        return Err(crate::Error::Other("draft has no recipients".into()));
    }
    build_rfc822_draft_with_attachments(draft, from, in_reply_to, references, attachments)
}

/// [`build_rfc822_with_attachments`] without the recipient check.
/// With attachments the body is multipart/mixed: the text (or the
/// text/html alternative) first, then one base64 part per attachment.
pub fn build_rfc822_draft_with_attachments(
    draft: &Draft,
    from: &Address,
    in_reply_to: Option<&str>,
    references: &[String],
    attachments: &[AttachmentBytes],
) -> crate::Result<Vec<u8>> {
    if attachments.len() != draft.attachments.len() {
        return Err(crate::Error::Other(format!(
            "{} attachments on the draft but {} resolved",
            draft.attachments.len(),
            attachments.len()
        )));
    }
    let total: u64 = attachments.iter().map(|a| a.bytes.len() as u64).sum();
    if total > MAX_ATTACHMENTS_BYTES {
        return Err(crate::Error::Other(format!(
            "attachments total {:.1} MB; Gmail's limit is {} MB",
            total as f64 / (1024.0 * 1024.0),
            MAX_ATTACHMENTS_BYTES / (1024 * 1024)
        )));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut builder = MessageBuilder::new()
        .from(mb_addr(from))
        .subject(draft.subject.as_str())
        .date(Date::new(now))
        .message_id(new_message_id(from));
    if let Some(to) = mb_list(&draft.to) {
        builder = builder.to(to);
    }
    if let Some(cc) = mb_list(&draft.cc) {
        builder = builder.cc(cc);
    }
    if let Some(bcc) = mb_list(&draft.bcc) {
        builder = builder.bcc(bcc);
    }

    let parent = in_reply_to.map(bare_id).filter(|s| !s.is_empty());
    let mut refs: Vec<String> = references
        .iter()
        .map(|r| bare_id(r))
        .filter(|r| !r.is_empty())
        .collect();
    if let Some(p) = &parent {
        if refs.last() != Some(p) {
            refs.retain(|r| r != p);
            refs.push(p.clone());
        }
        builder = builder.in_reply_to(p.clone());
    }
    if !refs.is_empty() {
        builder = builder.references(refs);
    }
    if draft.request_read_receipt == Some(true) {
        // RFC 8098 §2.1: where the receipt goes. A request only: the
        // recipient's app decides, usually by asking them.
        builder = builder.header("Disposition-Notification-To", mb_addr(from));
    }

    // The composer's HTML is re-sanitized on the way out (strict allowlist:
    // basic formatting, http(s)/mailto links, minimal inline CSS, and `cid:`
    // images of this message's own inline parts), so nothing it picked up
    // from pasted or quoted mail can ride along.
    let parts: Vec<(Option<&str>, &str)> = attachments
        .iter()
        .map(|a| (a.content_id.as_deref(), a.mime_type.as_str()))
        .collect();
    let plan = crate::inline::plan(draft.body_html.as_deref(), &parts);
    let text = MimePart::new("text/plain", draft.body_text.as_str());
    let body = match plan.html {
        Some(html) => {
            let mut related = vec![MimePart::new("text/html", html)];
            for (a, _) in attachments.iter().zip(&plan.inline).filter(|(_, i)| **i) {
                let cid = a.content_id.as_deref().unwrap_or_default();
                related.push(
                    MimePart::new(
                        ContentType::new(safe_mime_type(&a.mime_type)),
                        BodyPart::Binary(a.bytes.as_slice().into()),
                    )
                    .cid(
                        cid.trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .trim(),
                    )
                    .header(
                        "Content-Disposition",
                        Raw::new(content_disposition("inline", &a.filename)),
                    ),
                );
            }
            let html_part = if related.len() > 1 {
                MimePart::new(
                    ContentType::new("multipart/related").attribute("type", "text/html"),
                    BodyPart::Multipart(related),
                )
            } else {
                related.pop().expect("the HTML part")
            };
            MimePart::new(
                "multipart/alternative",
                BodyPart::Multipart(vec![text, html_part]),
            )
        }
        None => text,
    };
    let files: Vec<&AttachmentBytes> = attachments
        .iter()
        .zip(&plan.inline)
        .filter(|(_, i)| !**i)
        .map(|(a, _)| a)
        .collect();
    if files.is_empty() {
        builder = builder.body(body);
    } else {
        let mut parts = vec![body];
        for a in files {
            parts.push(
                MimePart::new(
                    ContentType::new(safe_mime_type(&a.mime_type)),
                    BodyPart::Binary(a.bytes.as_slice().into()),
                )
                .header(
                    "Content-Disposition",
                    Raw::new(content_disposition("attachment", &a.filename)),
                ),
            );
        }
        builder = builder.body(MimePart::new("multipart/mixed", BodyPart::Multipart(parts)));
    }
    builder
        .write_to_vec()
        .map_err(|e| crate::Error::Other(format!("compose: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::{MessageParser, MimeHeaders};

    fn addr(name: Option<&str>, email: &str) -> Address {
        Address {
            name: name.map(str::to_string),
            email: email.to_string(),
        }
    }

    fn draft() -> Draft {
        Draft {
            request_read_receipt: None,
            account_id: "me@penguin.example".into(),
            to: vec![
                addr(Some("Zoë Łukasiewicz"), "zoe@acme.example"),
                addr(None, "ops@acme.example"),
            ],
            cc: vec![addr(Some("Doe, Jane"), "jane@acme.example")],
            bcc: vec![addr(None, "secret@hidden.example")],
            subject: "Re: Café meeting — 日本語 ✓".into(),
            body_text: "Hi Zoë,\n\nSee you at the café.\n".into(),
            body_html: Some("<p>Hi Zoë,</p><p>See you at the <b>café</b>.</p>".into()),
            reply_to_thread_id: Some("t1".into()),
            reply_to_message_id: Some("m1".into()),
            attachments: vec![],
        }
    }

    #[test]
    fn read_receipt_request_only_when_asked() {
        let from = addr(Some("Sam Park"), "sam@mail.example");
        let header = |d: &Draft| {
            let raw = build_rfc822(d, &from, None, &[]).unwrap();
            let text = String::from_utf8_lossy(&raw).to_string();
            text.lines()
                .find(|l| l.to_ascii_lowercase().starts_with("disposition-notification-to:"))
                .map(str::to_string)
        };
        assert_eq!(header(&draft()), None);
        assert_eq!(
            header(&Draft {
                request_read_receipt: Some(false),
                ..draft()
            }),
            None
        );
        let asked = Draft {
            request_read_receipt: Some(true),
            ..draft()
        };
        let line = header(&asked).expect("header present");
        assert!(line.contains("<sam@mail.example>"), "{line}");
        assert!(line.contains("Sam Park"), "{line}");
        let raw = build_rfc822(&asked, &from, None, &[]).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let v = parsed.header_raw("Disposition-Notification-To").unwrap();
        assert!(v.contains("sam@mail.example"), "{v}");
        // The Draft's JSON from the UI may leave it out.
        let d: Draft = serde_json::from_str(r#"{"accountId":"a","to":[],"cc":[],"bcc":[],"subject":"","bodyText":"","bodyHtml":null,"replyToThreadId":null,"replyToMessageId":null}"#).unwrap();
        assert_eq!(d.request_read_receipt, None);
    }

    #[test]
    fn round_trip_multipart_alternative() {
        let from = addr(Some("Pénélope Penguin"), "me@penguin.example");
        let raw = build_rfc822(
            &draft(),
            &from,
            Some("<parent@acme.example>"),
            &["root@acme.example".into()],
        )
        .unwrap();
        let text = String::from_utf8_lossy(&raw);
        // Headers are 7-bit clean (non-ASCII is RFC 2047 encoded).
        let head = &text[..text.find("\r\n\r\n").unwrap()];
        assert!(head.is_ascii(), "headers must be ASCII:\n{head}");
        assert!(!head.to_ascii_lowercase().contains("gethostname"));

        let m = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(m.subject(), Some("Re: Café meeting — 日本語 ✓"));
        let f = m.from().unwrap().first().unwrap();
        assert_eq!(f.name.as_deref(), Some("Pénélope Penguin"));
        assert_eq!(f.address.as_deref(), Some("me@penguin.example"));
        let to: Vec<_> = m
            .to()
            .unwrap()
            .iter()
            .map(|a| {
                (
                    a.name.as_deref().map(str::to_string),
                    a.address.as_deref().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            to,
            vec![
                (
                    Some("Zoë Łukasiewicz".to_string()),
                    "zoe@acme.example".to_string()
                ),
                (None, "ops@acme.example".to_string())
            ]
        );
        let cc = m.cc().unwrap().first().unwrap();
        assert_eq!(cc.name.as_deref(), Some("Doe, Jane"));
        // Bcc is present for Gmail to route; Gmail strips it on delivery.
        assert_eq!(
            m.bcc().unwrap().first().unwrap().address.as_deref(),
            Some("secret@hidden.example")
        );

        assert_eq!(m.in_reply_to().as_text(), Some("parent@acme.example"));
        let refs: Vec<String> = m
            .references()
            .as_text_list()
            .unwrap()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(refs, vec!["root@acme.example", "parent@acme.example"]);
        assert!(m.message_id().unwrap().ends_with("@penguin.example"));
        assert!(m.date().is_some());

        assert_eq!(m.content_type().unwrap().subtype(), Some("alternative"));
        assert_eq!(
            m.body_text(0).unwrap().replace("\r\n", "\n"),
            "Hi Zoë,\n\nSee you at the café.\n"
        );
        assert!(m.body_html(0).unwrap().contains("<b>café</b>"));
    }

    #[test]
    fn plain_only_and_no_reply_headers() {
        let mut d = draft();
        d.body_html = None;
        d.cc.clear();
        d.bcc.clear();
        d.subject = "Hello".into();
        let raw = build_rfc822(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(m.content_type().unwrap().ctype(), "text");
        assert!(m.in_reply_to().is_empty());
        assert!(m.references().is_empty());
        assert!(m.cc().is_none() && m.bcc().is_none());
    }

    #[test]
    fn parent_already_last_in_references_is_not_duplicated() {
        let raw = build_rfc822(
            &draft(),
            &addr(None, "me@penguin.example"),
            Some("p@x.example"),
            &["<a@x.example>".into(), "<p@x.example>".into()],
        )
        .unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        let refs: Vec<String> = m
            .references()
            .as_text_list()
            .unwrap()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(refs, vec!["a@x.example", "p@x.example"]);
    }

    #[test]
    fn rejects_no_recipients_but_drafts_may_have_none() {
        let mut d = draft();
        d.to.clear();
        d.cc.clear();
        d.bcc.clear();
        assert!(build_rfc822(&d, &addr(None, "me@penguin.example"), None, &[]).is_err());
        let raw = build_rfc822_draft(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        assert!(m.to().is_none() && m.subject().is_some());
    }

    fn file(name: &str, mime: &str, bytes: &[u8]) -> OutgoingAttachment {
        OutgoingAttachment::File {
            filename: name.into(),
            mime_type: mime.into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            content_id: None,
        }
    }

    #[test]
    fn attachments_round_trip_as_multipart_mixed() {
        let pdf: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        let mut d = draft();
        d.attachments = vec![
            file("Q3 report — Zoë.pdf", "application/pdf", &pdf),
            file("notes.txt", "Text/Plain", b"plain notes\n"),
            file("odd", "not a mime type", b"\x00\x01"),
        ];
        let raw = build_rfc822(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("filename*=UTF-8''Q3%20report%20%E2%80%94%20Zo%C3%AB.pdf"),
            "{text}"
        );
        assert!(text.contains("filename=\"=?UTF-8?B?"), "{text}");

        let m = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(m.content_type().unwrap().subtype(), Some("mixed"));
        // Bodies survive next to the attachments.
        assert_eq!(
            m.body_text(0).unwrap().replace("\r\n", "\n"),
            "Hi Zoë,\n\nSee you at the café.\n"
        );
        assert!(m.body_html(0).unwrap().contains("<b>café</b>"));
        let atts: Vec<_> = m.attachments().collect();
        assert_eq!(atts.len(), 3);
        assert_eq!(atts[0].attachment_name(), Some("Q3 report — Zoë.pdf"));
        assert_eq!(atts[0].content_type().unwrap().ctype(), "application");
        assert_eq!(atts[0].contents(), pdf.as_slice());
        assert_eq!(atts[1].attachment_name(), Some("notes.txt"));
        assert_eq!(atts[1].content_type().unwrap().subtype(), Some("plain"));
        assert_eq!(
            atts[2].content_type().unwrap().subtype(),
            Some("octet-stream")
        );
        assert_eq!(atts[2].contents(), b"\x00\x01");
    }

    #[test]
    fn plain_text_draft_with_an_attachment() {
        let mut d = draft();
        d.body_html = None;
        d.attachments = vec![file("a.bin", "application/octet-stream", b"abc")];
        let raw = build_rfc822_draft(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            m.body_text(0).unwrap().replace("\r\n", "\n"),
            "Hi Zoë,\n\nSee you at the café.\n"
        );
        assert_eq!(m.attachments().next().unwrap().contents(), b"abc");
    }

    #[test]
    fn gmail_refs_need_resolved_bytes() {
        let mut d = draft();
        d.attachments = vec![
            file("local.txt", "text/plain", b"x"),
            OutgoingAttachment::Gmail {
                message_id: "m9".into(),
                attachment_id: "ANGj".into(),
                filename: "fwd.pdf".into(),
                mime_type: "application/pdf".into(),
                size: 3,
                account_id: None,
                content_id: None,
            },
        ];
        let from = addr(None, "me@penguin.example");
        let err = build_rfc822(&d, &from, None, &[]).unwrap_err().to_string();
        assert!(err.contains("fwd.pdf"), "{err}");

        let mut resolved: Vec<AttachmentBytes> = d
            .attachments
            .iter()
            .filter_map(|a| decode_file_attachment(a).unwrap())
            .collect();
        resolved.push(AttachmentBytes {
            filename: "fwd.pdf".into(),
            mime_type: "application/pdf".into(),
            bytes: b"%PD".to_vec(),
            content_id: None,
        });
        let raw = build_rfc822_with_attachments(&d, &from, None, &[], &resolved).unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        let names: Vec<_> = m
            .attachments()
            .map(|a| a.attachment_name().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["local.txt", "fwd.pdf"]);
        // Count mismatch is an error, not a silent drop.
        assert!(build_rfc822_with_attachments(&d, &from, None, &[], &resolved[..1]).is_err());
    }

    #[test]
    fn attachments_over_25_mb_are_rejected() {
        let mut d = draft();
        d.attachments = vec![file("big.bin", "application/octet-stream", b"")];
        let big = AttachmentBytes {
            filename: "big.bin".into(),
            mime_type: "application/octet-stream".into(),
            bytes: vec![0; MAX_ATTACHMENTS_BYTES as usize + 1],
            content_id: None,
        };
        let err =
            build_rfc822_with_attachments(&d, &addr(None, "me@penguin.example"), None, &[], &[big])
                .unwrap_err()
                .to_string();
        assert!(err.contains("25 MB"), "{err}");
    }

    fn inline(name: &str, mime: &str, bytes: &[u8], cid: &str) -> OutgoingAttachment {
        OutgoingAttachment::File {
            filename: name.into(),
            mime_type: mime.into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            content_id: Some(cid.into()),
        }
    }

    /// (content type, Content-ID) of every part, depth first, indented by depth.
    fn tree(m: &mail_parser::Message) -> Vec<String> {
        fn walk(m: &mail_parser::Message, id: usize, depth: usize, out: &mut Vec<String>) {
            let p = &m.parts[id];
            let ct = p
                .content_type()
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("")))
                .unwrap_or_default();
            let cid = p
                .content_id()
                .map(|c| format!(" <{c}>"))
                .unwrap_or_default();
            out.push(format!("{}{ct}{cid}", "  ".repeat(depth)));
            if let mail_parser::PartType::Multipart(kids) = &p.body {
                for k in kids {
                    walk(m, *k as usize, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(m, 0, 0, &mut out);
        out
    }

    #[test]
    fn inline_images_go_in_multipart_related_next_to_the_html() {
        let png = b"\x89PNG\r\n\x1a\nfake";
        let mut d = draft();
        d.body_html = Some(
            r#"<p>Chart:</p><p><img src="cid:img-1@penguin" alt="chart.png" width="600" style="max-width:100%;height:auto"></p><p><img src="https://t.example/p.gif"><img src="cid:not-attached@x"></p>"#.into(),
        );
        d.attachments = vec![
            file("report.pdf", "application/pdf", b"%PDF-1.4"),
            inline("chart.png", "image/png", png, "img-1@penguin"),
            // An image the HTML doesn't show goes as a file.
            inline("spare.png", "image/png", png, "img-2@penguin"),
        ];
        let raw = build_rfc822(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            tree(&m),
            vec![
                "multipart/mixed",
                "  multipart/alternative",
                "    text/plain",
                "    multipart/related",
                "      text/html",
                "      image/png <img-1@penguin>",
                "  application/pdf",
                "  image/png",
            ],
            "{}",
            String::from_utf8_lossy(&raw)
        );
        let html = m.body_html(0).unwrap();
        assert!(html.contains(r#"<img src="cid:img-1@penguin" alt="chart.png" width="600" style="max-width:100%;height:auto">"#), "{html}");
        assert!(
            !html.contains("t.example") && !html.contains("not-attached"),
            "{html}"
        );
        // The related part says what it relates to; the image is inline.
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("multipart/related; type=\"text/html\""),
            "{text}"
        );
        assert!(
            text.contains("Content-Disposition: inline; filename=\"chart.png\""),
            "{text}"
        );
        assert!(
            text.contains("Content-Disposition: attachment; filename=\"spare.png\""),
            "{text}"
        );
        let img = m
            .parts
            .iter()
            .find(|p| p.content_id() == Some("img-1@penguin"))
            .unwrap();
        assert_eq!(img.contents(), png);
        // Every cid the HTML uses is a part of this message.
        for cid in penguin_render::referenced_cids(html.as_ref()) {
            assert!(
                m.parts.iter().any(|p| p.content_id() == Some(cid.as_str())),
                "{cid}"
            );
        }
    }

    #[test]
    fn inline_images_without_files_have_no_mixed_wrapper() {
        let mut d = draft();
        d.body_html = Some(r#"<p><img src="cid:a@penguin"><img src="cid:b@penguin"></p>"#.into());
        d.attachments = vec![
            inline("a.gif", "image/gif", b"GIF89a", "a@penguin"),
            inline("b.jpg", "image/jpeg", b"\xff\xd8\xff", "<b@penguin>"),
        ];
        let raw = build_rfc822_draft(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            tree(&m),
            vec![
                "multipart/alternative",
                "  text/plain",
                "  multipart/related",
                "    text/html",
                "    image/gif <a@penguin>",
                "    image/jpeg <b@penguin>",
            ]
        );
        // Plain text only: the images go as files.
        d.body_html = None;
        let raw = build_rfc822_draft(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let m = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            tree(&m),
            vec![
                "multipart/mixed",
                "  text/plain",
                "  image/gif",
                "  image/jpeg"
            ]
        );
    }

    #[test]
    fn unsafe_content_ids_never_reach_a_header() {
        let mut d = draft();
        d.body_html = Some(r#"<img src="cid:x%0d%0aBcc:%20a@b">"#.into());
        d.attachments = vec![inline("x.png", "image/png", b"x", "x\r\nBcc: a@b")];
        let raw = build_rfc822_draft(&d, &addr(None, "me@penguin.example"), None, &[]).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.contains("Content-ID"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("bcc: a@b"), "{text}");
    }

    #[test]
    fn draft_json_without_attachments_still_parses() {
        let d: Draft = serde_json::from_value(serde_json::json!({
            "accountId": "a", "to": [], "cc": [], "bcc": [], "subject": "", "bodyText": "",
            "bodyHtml": null, "replyToThreadId": null, "replyToMessageId": null
        }))
        .unwrap();
        assert!(d.attachments.is_empty());
        let a: OutgoingAttachment = serde_json::from_value(serde_json::json!({
            "kind": "gmail", "messageId": "m", "attachmentId": "x", "filename": "f", "mimeType": "a/b", "size": 1
        }))
        .unwrap();
        assert_eq!(a.filename(), "f");
        assert_eq!(a.content_id(), None);
        // The inline image id travels as contentId and is omitted when unset.
        let a: OutgoingAttachment = serde_json::from_value(serde_json::json!({
            "kind": "file", "filename": "p.png", "mimeType": "image/png", "dataBase64": "", "contentId": "img-1@penguin"
        }))
        .unwrap();
        assert_eq!(a.content_id(), Some("img-1@penguin"));
        let back = serde_json::to_value(&a).unwrap();
        assert_eq!(back["contentId"], "img-1@penguin");
        let plain = serde_json::to_value(file("a", "a/b", b"")).unwrap();
        assert!(plain.get("contentId").is_none(), "{plain}");
    }
}
