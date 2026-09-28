//! RFC 822 bytes (or a header block plus BODYSTRUCTURE) → `penguin_core::Message`.
//!
//! Attachment ids are IMAP body part paths (`1`, `2`, `1.2`), numbered the
//! way BODYSTRUCTURE and `BODY[<path>]` number them, so an attachment found
//! while parsing a whole message can later be fetched on its own.

use mail_parser::{MessageParser, MimeHeaders, PartType};
use penguin_core::{Address, AttachmentMeta, Message};
pub use penguin_provider::text::snippet;

use crate::proto::Value;

/// Everything about a message that doesn't come from its bytes.
#[derive(Debug, Clone)]
pub struct Meta {
    pub account_id: String,
    pub id: String,
    pub thread_id: String,
    /// INTERNALDATE, unix ms (None: the Date header, else now).
    pub date_ms: Option<i64>,
    pub labels: Vec<String>,
    /// authserv-id whose Authentication-Results we trust (the account's own
    /// receiving server), e.g. `mx.google.com`. None: never authenticated.
    pub trusted_authserv: Option<String>,
}

/// Headers Penguin needs for identity and threading, from a header block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Envelope {
    /// Message-ID without brackets.
    pub message_id: Option<String>,
    /// The Date header's raw value (for fallback ids).
    pub date_raw: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub subject: String,
}

/// Unfolded header fields of a block, in order (name, raw value trimmed).
pub fn header_fields(raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    out
}

/// The first `name` header's unfolded raw value.
pub fn header_value(raw: &[u8], name: &str) -> Option<String> {
    header_fields(raw)
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

fn bare(s: &str) -> String {
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// Message ids in a References / In-Reply-To value (`<a@b> <c@d>`).
pub fn msgid_list(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = v;
    while let Some(open) = rest.find('<') {
        match rest[open..].find('>') {
            Some(close) => {
                let id = bare(&rest[open..open + close + 1]);
                if !id.is_empty() && !id.contains(char::is_whitespace) {
                    out.push(id);
                }
                rest = &rest[open + close + 1..];
            }
            None => break,
        }
    }
    if out.is_empty() {
        out.extend(v.split_whitespace().map(bare).filter(|s| s.contains('@')));
    }
    out
}

pub fn envelope(raw_headers: &[u8]) -> Envelope {
    envelope_of(&header_fields(raw_headers))
}

/// [`envelope`] from already unfolded fields.
fn envelope_of(fields: &[(String, String)]) -> Envelope {
    let get = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    Envelope {
        message_id: get("Message-ID")
            .and_then(|v| msgid_list(&v).into_iter().next().or(Some(bare(&v))))
            .filter(|s| !s.is_empty()),
        date_raw: get("Date"),
        in_reply_to: get("In-Reply-To").and_then(|v| msgid_list(&v).into_iter().next()),
        references: get("References")
            .map(|v| msgid_list(&v))
            .unwrap_or_default(),
        subject: get("Subject").map(|s| decode_words(&s)).unwrap_or_default(),
    }
}

/// Decode RFC 2047 encoded words (subjects, filenames).
pub fn decode_words(s: &str) -> String {
    if !s.contains("=?") {
        return s.to_string();
    }
    let block = format!("Subject: {}\r\n\r\n", s.replace(['\r', '\n'], " "));
    MessageParser::default()
        .parse_headers(block.as_bytes())
        .and_then(|m| m.subject().map(str::to_string))
        .unwrap_or_else(|| s.to_string())
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

fn addrs(a: Option<&mail_parser::Address>) -> Vec<Address> {
    a.map(|a| a.iter().filter_map(convert_addr).collect())
        .unwrap_or_default()
}

fn text_list(v: &mail_parser::HeaderValue) -> Vec<String> {
    match v {
        mail_parser::HeaderValue::Text(t) => vec![bare(t)],
        mail_parser::HeaderValue::TextList(l) => l.iter().map(|t| bare(t)).collect(),
        _ => vec![],
    }
}

fn fallback_filename(mime: &str) -> String {
    let ext = match mime {
        "text/calendar" => "ics",
        "message/rfc822" => "eml",
        "text/vcard" | "text/x-vcard" => "vcf",
        "application/pdf" => "pdf",
        m if m.starts_with("image/") => m.trim_start_matches("image/"),
        _ => "bin",
    };
    format!("untitled.{ext}")
}

#[derive(Default)]
struct Walk {
    text: Vec<String>,
    html: Vec<String>,
    attachments: Vec<AttachmentMeta>,
}

fn part_mime(p: &mail_parser::MessagePart) -> String {
    match p.content_type() {
        Some(ct) => match ct.subtype() {
            Some(sub) => format!("{}/{}", ct.ctype(), sub).to_ascii_lowercase(),
            None => ct.ctype().to_ascii_lowercase(),
        },
        None => "text/plain".into(),
    }
}

fn walk_part(msg: &mail_parser::Message, id: usize, path: String, out: &mut Walk) {
    let Some(p) = msg.parts.get(id) else { return };
    if let PartType::Multipart(children) = &p.body {
        for (i, c) in children.iter().enumerate() {
            let child = if path.is_empty() {
                (i + 1).to_string()
            } else {
                format!("{path}.{}", i + 1)
            };
            walk_part(msg, *c as usize, child, out);
        }
        return;
    }
    let path = if path.is_empty() {
        "1".to_string()
    } else {
        path
    };
    let mime = part_mime(p);
    let disposition = p
        .content_disposition()
        .map(|d| d.ctype().to_ascii_lowercase())
        .unwrap_or_default();
    let name = p.attachment_name().map(decode_words);
    let is_attachment = disposition == "attachment" || name.is_some();
    match (&p.body, mime.as_str()) {
        (PartType::Text(t), "text/plain") | (PartType::Text(t), "text") if !is_attachment => {
            out.text.push(t.to_string());
            return;
        }
        (PartType::Html(h), "text/html") if !is_attachment => {
            out.html.push(h.to_string());
            return;
        }
        _ => {}
    }
    let size = match &p.body {
        PartType::Message(_) => p.offset_end.saturating_sub(p.offset_body) as u64,
        _ => p.contents().len() as u64,
    };
    if size == 0 && !matches!(p.body, PartType::Message(_)) {
        return;
    }
    let content_id = p.content_id().map(bare).filter(|c| !c.is_empty());
    // Inline = shown in the body through `cid:`; without a Content-ID it
    // can't be referenced, so it's listed as an attachment.
    let inline = content_id.is_some() && (disposition == "inline" || disposition.is_empty());
    out.attachments.push(AttachmentMeta {
        id: path,
        filename: name.unwrap_or_else(|| fallback_filename(&mime)),
        mime_type: mime,
        size,
        content_id,
        inline,
    });
}

/// The decoded bytes of part `path` (an attachment id) of a whole message.
pub fn part_bytes(raw: &[u8], path: &str) -> Option<Vec<u8>> {
    let msg = MessageParser::default().parse(raw)?;
    let mut id = 0usize;
    let root = msg.parts.first()?;
    let steps: Vec<usize> = path
        .split('.')
        .map(|s| s.parse::<usize>().ok())
        .collect::<Option<_>>()?;
    if !matches!(root.body, PartType::Multipart(_)) {
        return (steps == [1]).then(|| root.contents().to_vec());
    }
    for step in steps {
        let PartType::Multipart(children) = &msg.parts.get(id)?.body else {
            return None;
        };
        id = *children.get(step.checked_sub(1)?)? as usize;
    }
    let p = msg.parts.get(id)?;
    Some(match &p.body {
        PartType::Message(_) => raw
            .get(p.offset_body as usize..p.offset_end as usize)?
            .to_vec(),
        _ => p.contents().to_vec(),
    })
}

/// A whole message (BODY[]) → Message.
pub fn to_message(raw: &[u8], meta: &Meta) -> Message {
    let parsed = MessageParser::default().parse(raw);
    let mut w = Walk::default();
    if let Some(m) = &parsed {
        walk_part(m, 0, String::new(), &mut w);
    }
    let header_end = find_header_end(raw);
    // The whole-message parse has the headers already: no second parse.
    assemble(
        &raw[..header_end],
        parsed,
        w.text,
        w.html,
        w.attachments,
        meta,
    )
}

/// A header block alone (headers-only storage, server search).
pub fn headers_to_message(raw_headers: &[u8], meta: &Meta) -> Message {
    let parsed = MessageParser::default().parse_headers(raw_headers);
    assemble(
        raw_headers,
        parsed,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        meta,
    )
}

/// From a header block plus separately fetched text parts and
/// BODYSTRUCTURE attachments (large messages).
pub fn from_parts(
    raw_headers: &[u8],
    text: Vec<String>,
    html: Vec<String>,
    attachments: Vec<AttachmentMeta>,
    meta: &Meta,
) -> Message {
    let parsed = MessageParser::default().parse_headers(raw_headers);
    assemble(raw_headers, parsed, text, html, attachments, meta)
}

fn find_header_end(raw: &[u8]) -> usize {
    if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        return i + 4;
    }
    if let Some(i) = raw.windows(2).position(|w| w == b"\n\n") {
        return i + 2;
    }
    raw.len()
}

/// `parsed`: `raw_headers` parsed by mail_parser (alone, or as the head
/// of the whole message).
fn assemble(
    raw_headers: &[u8],
    parsed: Option<mail_parser::Message<'_>>,
    text: Vec<String>,
    html: Vec<String>,
    attachments: Vec<AttachmentMeta>,
    meta: &Meta,
) -> Message {
    let fields = header_fields(raw_headers);
    let field = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let env = envelope_of(&fields);
    let body_html = (!html.is_empty()).then(|| html.join("\n"));
    let body_text = if !text.is_empty() {
        text.join("\n")
    } else if let Some(h) = &body_html {
        penguin_core::text::html_to_text(h)
    } else {
        String::new()
    };
    let (from, to, cc, bcc, reply_to, subject, in_reply_to, references) = match &parsed {
        Some(m) => (
            addrs(m.from()).into_iter().next(),
            addrs(m.to()),
            addrs(m.cc()),
            addrs(m.bcc()),
            addrs(m.reply_to()),
            m.subject()
                .map(str::to_string)
                .unwrap_or(env.subject.clone()),
            text_list(m.in_reply_to()).into_iter().next(),
            text_list(m.references()),
        ),
        None => (
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            env.subject.clone(),
            env.in_reply_to.clone(),
            env.references.clone(),
        ),
    };
    let from = from.unwrap_or(Address {
        name: None,
        email: String::new(),
    });
    let date = meta
        .date_ms
        .filter(|d| *d > 0)
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|m| m.date())
                .map(|d| d.to_timestamp() * 1000)
        })
        .filter(|d| *d > 0)
        .unwrap_or_else(now_ms);
    let authenticated = meta
        .trusted_authserv
        .as_deref()
        .is_some_and(|id| sender_authenticated(&fields, &from.email, id));
    let mut m = Message {
        account_id: meta.account_id.clone(),
        id: meta.id.clone(),
        thread_id: meta.thread_id.clone(),
        date,
        from,
        to,
        cc,
        bcc,
        reply_to,
        subject,
        snippet: snippet(&body_text),
        body_text,
        body_html,
        label_ids: meta.labels.clone(),
        attachments,
        message_id_header: env.message_id.clone(),
        in_reply_to: in_reply_to.filter(|s| !s.is_empty()),
        references: references.into_iter().filter(|s| !s.is_empty()).collect(),
        list_unsubscribe: field("List-Unsubscribe").filter(|v| !v.is_empty()),
        list_unsubscribe_post: Some(
            field("List-Unsubscribe-Post")
                .is_some_and(|v| penguin_core::unsubscribe::is_one_click(&v)),
        ),
        sender_authenticated: authenticated,
    };
    m.settle_inline();
    m
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------- sender authentication ----------

/// Did the account's own receiving server authenticate the From domain?
/// Only the topmost Authentication-Results counts, and only when its
/// authserv-id is `trusted` (the receiving server prepends its own above
/// anything the sender wrote). DMARC pass, or DKIM pass aligned with From.
pub fn sender_authenticated(fields: &[(String, String)], from_email: &str, trusted: &str) -> bool {
    let Some(from_domain) = from_email
        .rsplit_once('@')
        .map(|(_, d)| d.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|d| d.contains('.'))
    else {
        return false;
    };
    let Some((_, ar)) = fields
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("Authentication-Results"))
    else {
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
    if !authserv.eq_ignore_ascii_case(trusted) {
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
            "dmarc" if prop("header.from").is_none_or(|d| d == from_domain) => return true,
            "dkim" => {
                let signer = prop("header.d")
                    .or_else(|| prop("header.i").and_then(|i| i.rsplit_once('@').map(|(_, d)| d)));
                if signer.is_some_and(|d| {
                    d.contains('.') && (from_domain == d || from_domain.ends_with(&format!(".{d}")))
                }) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

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

// ---------- BODYSTRUCTURE ----------

/// One node of a BODYSTRUCTURE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub path: String,
    /// `type/subtype`, lowercase.
    pub mime: String,
    pub params: Vec<(String, String)>,
    pub content_id: Option<String>,
    pub encoding: String,
    /// Encoded size in bytes.
    pub size: u64,
    pub disposition: Option<(String, Vec<(String, String)>)>,
    pub children: Vec<Part>,
}

fn params(v: Option<&Value>) -> Vec<(String, String)> {
    let Some(list) = v.and_then(Value::as_list) else {
        return Vec::new();
    };
    list.chunks(2)
        .filter_map(|p| {
            Some((
                p.first()?.as_text()?.to_ascii_lowercase(),
                p.get(1)?.as_text()?,
            ))
        })
        .collect()
}

fn disposition(v: Option<&Value>) -> Option<(String, Vec<(String, String)>)> {
    let list = v?.as_list()?;
    Some((
        list.first()?.as_text()?.to_ascii_lowercase(),
        params(list.get(1)),
    ))
}

/// Parse a BODYSTRUCTURE value.
pub fn parse_bodystructure(v: &Value) -> Option<Part> {
    node(v, "")
}

fn node(v: &Value, path: &str) -> Option<Part> {
    let items = v.as_list()?;
    if matches!(items.first(), Some(Value::List(_))) {
        let mut children = Vec::new();
        let mut i = 0;
        while let Some(Value::List(_)) = items.get(i) {
            let child = if path.is_empty() {
                (i + 1).to_string()
            } else {
                format!("{path}.{}", i + 1)
            };
            children.push(node(&items[i], &child)?);
            i += 1;
        }
        let sub = items.get(i).and_then(Value::as_text).unwrap_or_default();
        return Some(Part {
            path: if path.is_empty() {
                String::new()
            } else {
                path.to_string()
            },
            mime: format!("multipart/{}", sub.to_ascii_lowercase()),
            params: params(items.get(i + 1)),
            content_id: None,
            encoding: String::new(),
            size: 0,
            disposition: disposition(items.get(i + 2)),
            children,
        });
    }
    let path = if path.is_empty() {
        "1".to_string()
    } else {
        path.to_string()
    };
    let ty = items.first()?.as_text()?.to_ascii_lowercase();
    let sub = items
        .get(1)
        .and_then(Value::as_text)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = format!("{ty}/{sub}");
    let ext = match mime.as_str() {
        m if m.starts_with("text/") => 8,
        "message/rfc822" | "message/global" => 10,
        _ => 7,
    };
    Some(Part {
        path,
        params: params(items.get(2)),
        content_id: items.get(3).and_then(Value::as_text).map(|s| bare(&s)),
        encoding: items
            .get(5)
            .and_then(Value::as_text)
            .unwrap_or_default()
            .to_ascii_lowercase(),
        size: items.get(6).and_then(Value::as_u64).unwrap_or(0),
        disposition: disposition(items.get(ext + 1)),
        mime,
        children: Vec::new(),
    })
}

impl Part {
    pub fn find(&self, path: &str) -> Option<&Part> {
        if self.path == path {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(path))
    }

    fn param(&self, name: &str) -> Option<String> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    fn filename(&self) -> Option<String> {
        let from = |list: &[(String, String)]| -> Option<String> {
            for key in ["filename", "name"] {
                if let Some((_, v)) = list.iter().find(|(k, _)| k == key) {
                    return Some(decode_words(v));
                }
                if let Some((_, v)) = list.iter().find(|(k, _)| k == &format!("{key}*")) {
                    return Some(decode_rfc2231(v));
                }
            }
            None
        };
        self.disposition
            .as_ref()
            .and_then(|(_, p)| from(p))
            .or_else(|| from(&self.params))
    }

    fn leaves<'a>(&'a self, out: &mut Vec<&'a Part>) {
        if self.children.is_empty() {
            if !self.mime.starts_with("multipart/") {
                out.push(self);
            }
        } else {
            for c in &self.children {
                c.leaves(out);
            }
        }
    }

    fn is_attachment(&self) -> bool {
        let disp = self
            .disposition
            .as_ref()
            .map(|(d, _)| d.as_str())
            .unwrap_or("");
        let body_text = (self.mime == "text/plain" || self.mime == "text/html")
            && disp != "attachment"
            && self.filename().is_none();
        !body_text
    }

    /// Body text parts: (path, is_html, charset, encoding).
    pub fn text_parts(&self) -> Vec<(String, bool, Option<String>, String)> {
        let mut leaves = Vec::new();
        self.leaves(&mut leaves);
        leaves
            .into_iter()
            .filter(|p| !p.is_attachment())
            .map(|p| {
                (
                    p.path.clone(),
                    p.mime == "text/html",
                    p.param("charset"),
                    p.encoding.clone(),
                )
            })
            .collect()
    }

    /// Encoded sizes of the body text parts ([`Part::text_parts`]).
    pub fn text_sizes(&self) -> Vec<u64> {
        let mut leaves = Vec::new();
        self.leaves(&mut leaves);
        leaves
            .into_iter()
            .filter(|p| !p.is_attachment())
            .map(|p| p.size)
            .collect()
    }

    /// Attachments as the UI lists them (sizes approximate: decoded).
    pub fn attachments(&self) -> Vec<AttachmentMeta> {
        let mut leaves = Vec::new();
        self.leaves(&mut leaves);
        leaves
            .into_iter()
            .filter(|p| p.is_attachment() && p.size > 0)
            .map(|p| {
                let disp = p
                    .disposition
                    .as_ref()
                    .map(|(d, _)| d.as_str())
                    .unwrap_or("");
                let inline = (disp == "inline" || disp.is_empty()) && p.content_id.is_some();
                AttachmentMeta {
                    id: p.path.clone(),
                    filename: p.filename().unwrap_or_else(|| fallback_filename(&p.mime)),
                    mime_type: p.mime.clone(),
                    size: if p.encoding == "base64" {
                        p.size * 3 / 4
                    } else {
                        p.size
                    },
                    content_id: p.content_id.clone().filter(|c| !c.is_empty()),
                    inline,
                }
            })
            .collect()
    }
}

/// `UTF-8''n%C3%A4me.pdf` → `näme.pdf`.
fn decode_rfc2231(v: &str) -> String {
    let rest = match v.splitn(3, '\'').collect::<Vec<_>>().as_slice() {
        [_, _, rest] => rest.to_string(),
        _ => v.to_string(),
    };
    let mut bytes = Vec::with_capacity(rest.len());
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(n) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                bytes.push(n);
                i += 3;
                continue;
            }
        }
        bytes.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Undo a Content-Transfer-Encoding.
pub fn decode_transfer(bytes: &[u8], encoding: &str) -> Vec<u8> {
    match encoding.to_ascii_lowercase().as_str() {
        "base64" => {
            use base64::Engine;
            let compact: Vec<u8> = bytes
                .iter()
                .copied()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            let engine = base64::engine::GeneralPurpose::new(
                &base64::alphabet::STANDARD,
                base64::engine::GeneralPurposeConfig::new()
                    .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
                    .with_decode_allow_trailing_bits(true),
            );
            // Partial fetches can cut mid-quantum: decode whole quanta only.
            let usable = compact.len() - compact.len() % 4;
            engine
                .decode(&compact[..usable])
                .or_else(|_| engine.decode(&compact))
                .unwrap_or_default()
        }
        "quoted-printable" => decode_qp(bytes),
        _ => bytes.to_vec(),
    }
}

fn decode_qp(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            // Soft line break.
            if bytes.get(i + 1) == Some(&b'\r') && bytes.get(i + 2) == Some(&b'\n') {
                i += 3;
                continue;
            }
            if bytes.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if let (Some(a), Some(b)) = (bytes.get(i + 1), bytes.get(i + 2)) {
                let hex = [*a, *b];
                if let Some(n) = std::str::from_utf8(&hex)
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(n);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Text bytes in their declared charset (UTF-8 when unknown or when the
/// declared one fails but the bytes are valid UTF-8).
pub fn decode_text(bytes: &[u8], charset: Option<&str>) -> String {
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

#[cfg(test)]
#[path = "mime_tests.rs"]
mod tests;
