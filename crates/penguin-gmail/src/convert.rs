//! Gmail `format=full` JSON → `penguin_core::Message`. OWNER: gmail-sync agent.
//!
//! Gmail hands back body part bytes after undoing the Content-Transfer-Encoding
//! but still in the part's declared charset, so text is decoded with
//! `encoding_rs` using the part's `Content-Type; charset=`. Header values are
//! re-parsed with `mail-parser`, which handles quoted display names, groups
//! and any RFC 2047 encoded words Gmail leaves in place.

use std::collections::{BTreeMap, HashMap};

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use penguin_core::{Address, AttachmentMeta, Message};
use serde::Deserialize;

/// base64url that accepts both padded and unpadded input (Gmail emits both).
pub(crate) const B64URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

pub(crate) fn b64url_decode(data: &str) -> Option<Vec<u8>> {
    // Some producers (and older Gmail responses) sprinkle whitespace in.
    if data.bytes().any(|b| b.is_ascii_whitespace()) {
        let compact: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        return B64URL.decode(compact).ok();
    }
    B64URL.decode(data).ok()
}

// ---------- Gmail wire types ----------

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GmailMessage {
    pub id: String,
    pub thread_id: String,
    pub label_ids: Vec<String>,
    pub snippet: String,
    pub history_id: Option<String>,
    pub internal_date: Option<String>,
    pub payload: Option<Part>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Part {
    pub part_id: String,
    pub mime_type: String,
    pub filename: String,
    pub headers: Vec<Header>,
    pub body: Body,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Body {
    pub attachment_id: Option<String>,
    pub size: u64,
    pub data: Option<String>,
}

/// Prefix for attachment ids of parts whose bytes Gmail inlined in the
/// message (no `attachmentId`). `get_attachment` resolves these by partId.
pub const PART_ID_PREFIX: &str = "part:";

impl Part {
    fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// Depth-first search for a part by Gmail partId.
    pub fn find(&self, part_id: &str) -> Option<&Part> {
        if self.part_id == part_id {
            return Some(self);
        }
        self.parts.iter().find_map(|p| p.find(part_id))
    }
}

fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

// ---------- MIME walk ----------

#[derive(Debug, PartialEq, Eq)]
enum Role {
    Container,
    Text,
    Html,
    Attachment,
}

fn role(part: &Part) -> Role {
    let mime = part.mime_type.to_ascii_lowercase();
    if mime.starts_with("multipart/") {
        return Role::Container;
    }
    // Children but no attachmentId: a forwarded message/rfc822 Gmail
    // expanded, or a container with a missing/garbled mimeType. Descend so
    // its text is at least searchable.
    if !part.parts.is_empty() && part.body.attachment_id.is_none() {
        return Role::Container;
    }
    let disposition = part
        .header("Content-Disposition")
        .map(|d| d.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let is_attachment = !part.filename.is_empty() || disposition.starts_with("attachment");
    match mime.as_str() {
        "text/plain" if !is_attachment => Role::Text,
        "text/html" if !is_attachment => Role::Html,
        // Malformed messages sometimes omit mimeType on the only part.
        "" if !is_attachment && part.parts.is_empty() => Role::Text,
        _ => Role::Attachment,
    }
}

/// Attachment ids of body parts (text/plain, text/html) that Gmail moved out
/// of line because they were too large. Fetch these and pass them to
/// [`to_message`] or the body would be empty.
pub fn out_of_line_bodies(msg: &GmailMessage) -> Vec<String> {
    let Some(payload) = &msg.payload else {
        return vec![];
    };
    let mut out = Vec::new();
    fn walk(p: &Part, out: &mut Vec<String>) {
        match role(p) {
            Role::Container => p.parts.iter().for_each(|c| walk(c, out)),
            Role::Text | Role::Html => {
                if p.body.data.is_none() {
                    if let Some(id) = &p.body.attachment_id {
                        out.push(id.clone());
                    }
                }
            }
            Role::Attachment => {}
        }
    }
    walk(payload, &mut out);
    out
}

/// Parameter value from a structured header (`Content-Type: text/plain; charset="x"`).
fn header_param(value: &str, param: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k.trim().eq_ignore_ascii_case(param) {
            let v = v.trim().trim_matches('"').trim();
            (!v.is_empty()).then(|| v.to_string())
        } else {
            None
        }
    })
}

/// Decode body bytes in their declared charset. If the declared charset
/// produces replacement characters but the bytes are valid UTF-8 (mislabelled
/// mail is common), trust UTF-8 instead.
pub(crate) fn decode_text(bytes: &[u8], charset: Option<&str>) -> String {
    let declared = charset
        .and_then(|c| encoding_rs::Encoding::for_label(c.trim().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, had_errors) = declared.decode(bytes);
    if had_errors && declared != encoding_rs::UTF_8 {
        if let Ok(utf8) = std::str::from_utf8(bytes) {
            return utf8.to_string();
        }
    }
    text.into_owned()
}

#[derive(Default)]
struct Walk {
    text: Vec<String>,
    html: Vec<String>,
    attachments: Vec<AttachmentMeta>,
}

fn walk(part: &Part, extra: &HashMap<String, Vec<u8>>, out: &mut Walk) {
    match role(part) {
        Role::Container => part.parts.iter().for_each(|c| walk(c, extra, out)),
        r @ (Role::Text | Role::Html) => {
            let bytes = match (&part.body.data, &part.body.attachment_id) {
                (Some(d), _) => b64url_decode(d),
                (None, Some(id)) => extra.get(id).cloned(),
                (None, None) => None,
            };
            let Some(bytes) = bytes else { return };
            if bytes.is_empty() {
                return;
            }
            let charset = part
                .header("Content-Type")
                .and_then(|ct| header_param(ct, "charset"));
            let s = decode_text(&bytes, charset.as_deref());
            if r == Role::Text {
                out.text.push(s);
            } else {
                out.html.push(s);
            }
        }
        Role::Attachment => {
            let id = match &part.body.attachment_id {
                Some(id) => id.clone(),
                None if part.body.data.is_some() => format!("{PART_ID_PREFIX}{}", part.part_id),
                // Nothing to download (empty part).
                None => return,
            };
            let content_type = part.header("Content-Type").unwrap_or_default();
            let disposition = part.header("Content-Disposition").unwrap_or_default();
            let content_id = part
                .header("Content-ID")
                .or_else(|| part.header("X-Attachment-Id"))
                .map(|c| {
                    c.trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .trim()
                        .to_string()
                })
                .filter(|c| !c.is_empty());
            let filename = Some(part.filename.clone())
                .filter(|f| !f.is_empty())
                .or_else(|| header_param(disposition, "filename"))
                .or_else(|| header_param(content_type, "name"))
                .map(|f| decode_words(&f))
                .unwrap_or_else(|| fallback_filename(&part.mime_type));
            let disp = disposition.trim().to_ascii_lowercase();
            let inline = disp.starts_with("inline") || (disp.is_empty() && content_id.is_some());
            out.attachments.push(AttachmentMeta {
                id,
                filename,
                mime_type: if part.mime_type.is_empty() {
                    "application/octet-stream".into()
                } else {
                    part.mime_type.to_ascii_lowercase()
                },
                size: part.body.size,
                content_id,
                inline,
            });
        }
    }
}

fn fallback_filename(mime: &str) -> String {
    let mime = mime.to_ascii_lowercase();
    let ext = match mime.as_str() {
        "text/calendar" => "ics",
        "message/rfc822" => "eml",
        "text/vcard" | "text/x-vcard" => "vcf",
        "application/pdf" => "pdf",
        m if m.starts_with("image/") => m.trim_start_matches("image/"),
        _ => "bin",
    };
    format!("untitled.{ext}")
}

// ---------- headers ----------

/// Parsed view of the headers we care about, via mail-parser.
struct ParsedHeaders {
    from: Vec<Address>,
    to: Vec<Address>,
    cc: Vec<Address>,
    bcc: Vec<Address>,
    reply_to: Vec<Address>,
    subject: Option<String>,
    message_id: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
}

const PARSED: [&str; 9] = [
    "From",
    "To",
    "Cc",
    "Bcc",
    "Reply-To",
    "Subject",
    "Message-ID",
    "In-Reply-To",
    "References",
];

fn parse_headers(headers: &[Header]) -> ParsedHeaders {
    let mut block = String::new();
    for name in PARSED {
        if let Some(v) = header(headers, name) {
            // Gmail unfolds headers; guard against stray CR/LF anyway so one
            // bad value can't inject or swallow the next header.
            let v: String = v
                .chars()
                .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
                .collect();
            block.push_str(name);
            block.push_str(": ");
            block.push_str(&v);
            block.push_str("\r\n");
        }
    }
    block.push_str("\r\n");

    let parsed = mail_parser::MessageParser::default().parse_headers(block.as_bytes());
    let Some(m) = parsed else {
        return ParsedHeaders {
            from: fallback_addrs(header(headers, "From")),
            to: fallback_addrs(header(headers, "To")),
            cc: fallback_addrs(header(headers, "Cc")),
            bcc: fallback_addrs(header(headers, "Bcc")),
            reply_to: fallback_addrs(header(headers, "Reply-To")),
            subject: header(headers, "Subject").map(decode_words),
            message_id: header(headers, "Message-ID").map(strip_angle),
            in_reply_to: header(headers, "In-Reply-To").map(strip_angle),
            references: header(headers, "References")
                .map(|r| r.split_whitespace().map(strip_angle).collect())
                .unwrap_or_default(),
        };
    };
    let addrs = |a: Option<&mail_parser::Address>| -> Vec<Address> {
        a.map(|a| a.iter().filter_map(convert_addr).collect())
            .unwrap_or_default()
    };
    let text_list = |v: &mail_parser::HeaderValue| -> Vec<String> {
        match v {
            mail_parser::HeaderValue::Text(t) => vec![t.to_string()],
            mail_parser::HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
            _ => vec![],
        }
    };
    ParsedHeaders {
        from: addrs(m.from()),
        to: addrs(m.to()),
        cc: addrs(m.cc()),
        bcc: addrs(m.bcc()),
        reply_to: addrs(m.reply_to()),
        subject: m.subject().map(str::to_string),
        message_id: m.message_id().map(str::to_string),
        in_reply_to: text_list(m.in_reply_to()).into_iter().next(),
        references: text_list(m.references()),
    }
}

fn convert_addr(a: &mail_parser::Addr) -> Option<Address> {
    let email = a.address.as_deref()?.trim().to_string();
    if email.is_empty() {
        return None;
    }
    let name = a
        .name
        .as_deref()
        .map(|n| n.trim().trim_matches('"').trim().to_string())
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&email));
    Some(Address { name, email })
}

fn fallback_addrs(v: Option<&str>) -> Vec<Address> {
    v.map(|v| {
        v.split(',')
            .filter_map(|s| {
                let s = s.trim();
                let email = match (s.rfind('<'), s.rfind('>')) {
                    (Some(a), Some(b)) if a < b => s[a + 1..b].trim(),
                    _ => s,
                };
                (!email.is_empty()).then(|| Address {
                    name: None,
                    email: email.to_string(),
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn strip_angle(s: &str) -> String {
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// Decode RFC 2047 encoded words in a free-text value (filenames, subjects
/// on the fallback path) by round-tripping through mail-parser.
fn decode_words(s: &str) -> String {
    if !s.contains("=?") {
        return s.to_string();
    }
    let block = format!("Subject: {}\r\n\r\n", s.replace(['\r', '\n'], " "));
    mail_parser::MessageParser::default()
        .parse_headers(block.as_bytes())
        .and_then(|m| m.subject().map(str::to_string))
        .unwrap_or_else(|| s.to_string())
}

/// Gmail snippets are HTML-escaped (`&#39;`, `&amp;`, …). Entity decoding
/// is penguin-core's (`text::decode_entity`), which only ever slices at
/// ASCII positions, so snippets full of emoji/CJK can't split a character.
pub(crate) fn unescape_snippet(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match penguin_core::text::decode_entity(rest) {
            Some((decoded, used)) => {
                out.push_str(&decoded);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------- sender authentication ----------

/// authserv-id Gmail's receiving MX stamps on its Authentication-Results.
const GMAIL_AUTHSERV_ID: &str = "mx.google.com";

/// Did Gmail's receiving MX authenticate the From domain? True for
/// `dmarc=pass`, or `dkim=pass` with a signing domain aligned to From.
///
/// Only the topmost Authentication-Results header is considered, and only if
/// Gmail's MX wrote it: Google prepends its own above the sender's headers,
/// so any lower one (including a forged "mx.google.com" one) came from the
/// sender and is ignored.
fn sender_authenticated(headers: &[Header], from_email: &str) -> bool {
    let Some(from_domain) = from_email
        .rsplit_once('@')
        .map(|(_, d)| d.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|d| d.contains('.'))
    else {
        return false;
    };
    let Some(ar) = header(headers, "Authentication-Results") else {
        return false;
    };
    let ar = strip_comments(ar);
    let mut parts = ar.split(';');
    let authserv = parts
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default();
    if !authserv.eq_ignore_ascii_case(GMAIL_AUTHSERV_ID) {
        return false;
    }
    for resinfo in parts {
        let mut tokens = resinfo.split_whitespace();
        let Some((method, result)) = tokens.next().and_then(|t| t.split_once('=')) else {
            continue;
        };
        if !result.eq_ignore_ascii_case("pass") {
            continue;
        }
        let props: Vec<(String, String)> = tokens
            .filter_map(|t| t.split_once('='))
            .map(|(k, v)| {
                (
                    k.to_ascii_lowercase(),
                    v.trim_matches('"')
                        .trim_end_matches('.')
                        .to_ascii_lowercase(),
                )
            })
            .collect();
        let prop = |key: &str| {
            props
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        match method.to_ascii_lowercase().as_str() {
            // Gmail evaluated DMARC against this From; double-check it names it.
            "dmarc" if prop("header.from").is_none_or(|d| d == from_domain) => return true,
            "dkim" => {
                let signer = prop("header.d")
                    .or_else(|| prop("header.i").and_then(|i| i.rsplit_once('@').map(|(_, d)| d)));
                if signer.is_some_and(|d| dkim_aligned(d, &from_domain)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// DKIM signing domain `d` aligns with the From domain: the same domain, or
/// From is a subdomain of `d` (DMARC relaxed alignment). The reverse (a
/// subdomain signing for its parent) is not accepted: without the public
/// suffix list it can't be told apart from a tenant of a shared domain
/// (`someone.example-host.com` signing for `example-host.com`).
fn dkim_aligned(d: &str, from_domain: &str) -> bool {
    d.contains('.') && (from_domain == d || from_domain.ends_with(&format!(".{d}")))
}

/// Remove RFC 5322 comments `( … )` (nested, with `\` escapes) outside
/// quoted strings.
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut depth, mut quoted, mut escaped) = (0u32, false, false);
    for c in s.chars() {
        if escaped {
            escaped = false;
            if depth == 0 {
                out.push(c);
            }
            continue;
        }
        match c {
            '\\' => {
                escaped = true;
                if depth == 0 {
                    out.push(c);
                }
            }
            '"' if depth == 0 => {
                quoted = !quoted;
                out.push(c);
            }
            '(' if !quoted => depth += 1,
            ')' if !quoted && depth > 0 => {
                depth -= 1;
                out.push(' ');
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

// ---------- entry point ----------

/// Convert one Gmail message. `extra_bodies` maps attachmentId → bytes for
/// the ids returned by [`out_of_line_bodies`] (empty map is fine).
pub fn to_message(
    account_id: &str,
    gm: &GmailMessage,
    extra_bodies: &HashMap<String, Vec<u8>>,
) -> Message {
    let empty = Part::default();
    let payload = gm.payload.as_ref().unwrap_or(&empty);
    let headers = parse_headers(&payload.headers);

    let mut w = Walk::default();
    walk(payload, extra_bodies, &mut w);

    let body_html = (!w.html.is_empty()).then(|| w.html.join("\n"));
    let body_text = if !w.text.is_empty() {
        w.text.join("\n")
    } else if let Some(html) = &body_html {
        penguin_core::text::html_to_text(html)
    } else {
        String::new()
    };

    let date = gm
        .internal_date
        .as_deref()
        .and_then(|d| d.parse::<i64>().ok())
        .unwrap_or(0);

    let from = headers.from.into_iter().next().unwrap_or(Address {
        name: None,
        email: String::new(),
    });
    let sender_authenticated = sender_authenticated(&payload.headers, &from.email);
    let mut m = Message {
        account_id: account_id.to_string(),
        id: gm.id.clone(),
        thread_id: gm.thread_id.clone(),
        date,
        from,
        to: headers.to,
        cc: headers.cc,
        bcc: headers.bcc,
        reply_to: headers.reply_to,
        subject: headers.subject.unwrap_or_default(),
        snippet: unescape_snippet(&gm.snippet),
        body_text,
        body_html,
        label_ids: gm.label_ids.clone(),
        attachments: w.attachments,
        message_id_header: headers.message_id.filter(|s| !s.is_empty()),
        in_reply_to: headers.in_reply_to.filter(|s| !s.is_empty()),
        references: headers
            .references
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect(),
        list_unsubscribe: header(&payload.headers, "List-Unsubscribe")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
        list_unsubscribe_post: Some(
            header(&payload.headers, "List-Unsubscribe-Post")
                .is_some_and(penguin_core::unsubscribe::is_one_click),
        ),
        sender_authenticated,
    };
    m.settle_inline();
    m
}

/// Locate a leaf part's inline bytes (for `part:` attachment ids).
pub fn inline_part_bytes(gm: &GmailMessage, part_id: &str) -> Option<Vec<u8>> {
    let p = gm.payload.as_ref()?.find(part_id)?;
    p.body.data.as_deref().and_then(b64url_decode)
}

/// Find the current attachmentId for an attachment whose id went stale, by
/// matching filename + size (+ Content-ID when present).
pub fn rematch_attachment_id(gm: &GmailMessage, old: &AttachmentMeta) -> Option<String> {
    let mut w = Walk::default();
    walk(gm.payload.as_ref()?, &HashMap::new(), &mut w);
    let mut candidates: BTreeMap<u8, String> = BTreeMap::new();
    for a in w.attachments {
        if a.filename == old.filename && a.size == old.size {
            let rank = if a.content_id == old.content_id { 0 } else { 1 };
            candidates.entry(rank).or_insert(a.id);
        }
    }
    candidates.into_values().next()
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
