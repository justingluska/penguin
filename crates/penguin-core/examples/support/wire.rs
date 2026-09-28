//! Wire encodings of corpus messages, for the sync benchmarks: what each
//! provider actually sends for a message, so parsing and bytes-on-the-wire
//! can be measured offline.
//!
//! - [`gmail_full_json`]: Gmail `users.messages.get?format=full` (payload
//!   tree, base64url part bodies, attachment ids, a realistic top-level
//!   header block with Received / ARC / DKIM / Authentication-Results).
//! - [`rfc822`]: the same message as the RFC 5322 bytes an IMAP server
//!   returns for `BODY[]` (quoted-printable text, base64 attachments).
//! - [`graph_json`]: Microsoft Graph `GET /me/messages/{id}` with the
//!   `$select` / `$expand` Penguin uses.
//!
//! Deterministic: the same message always encodes to the same bytes. The
//! people are the corpus's fictional `.example` addresses.
//!
//! Included via `#[path]` by crates that have `base64` and `serde_json`.
#![allow(dead_code)]

use base64::Engine;
use penguin_core::Message;
use serde_json::{json, Value};

/// splitmix64 over a string, for deterministic per-message choices.
fn hash(s: &str, salt: u64) -> u64 {
    let mut z = salt ^ 0x9E37_79B9_7F4A_7C15;
    for b in s.bytes() {
        z = (z ^ b as u64).wrapping_mul(0x100_0000_01B3);
    }
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `n` pseudo-random base64 characters (signatures, attachment ids).
fn b64_noise(seed: u64, n: usize) -> String {
    let mut out = Vec::with_capacity(n * 3 / 4 + 8);
    let mut x = seed;
    while out.len() * 4 / 3 < n {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    let mut s = base64::engine::general_purpose::STANDARD_NO_PAD.encode(out);
    s.truncate(n);
    s
}

/// Pseudo-random bytes (attachment content: images and PDFs are already
/// compressed, so random bytes are the honest stand-in).
pub fn noise_bytes(seed: u64, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n + 8);
    let mut x = seed;
    while out.len() < n {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out.truncate(n);
    out
}

fn domain(email: &str) -> &str {
    email.rsplit_once('@').map_or("mail.example", |(_, d)| d)
}

fn addr(a: &penguin_core::Address) -> String {
    match &a.name {
        Some(n) => format!("\"{n}\" <{}>", a.email),
        None => a.email.clone(),
    }
}

fn addrs(list: &[penguin_core::Address]) -> String {
    list.iter().map(addr).collect::<Vec<_>>().join(", ")
}

fn rfc2822_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%a, %d %b %Y %H:%M:%S +0000")
        .to_string()
}

/// Whether a newsletter ships only an HTML part (common in real mail, and
/// the path where the client derives text from HTML itself).
pub fn html_only(m: &Message) -> bool {
    m.body_html.is_some() && m.list_unsubscribe.is_some() && hash(&m.id, 7) % 2 == 0
}

/// The top-level header block a message carries by the time Gmail stores
/// it: transport, authentication and content headers, in Gmail's order.
pub fn top_headers(m: &Message, content_type: &str) -> Vec<(String, String)> {
    let from_dom = domain(&m.from.email).to_string();
    let h = |salt| hash(&m.id, salt);
    let date = rfc2822_date(m.date);
    let mut out: Vec<(String, String)> = vec![
        ("Delivered-To".into(), m.account_id.clone()),
        (
            "Received".into(),
            format!("by 2002:a05:6a10:{:x}:b0:5d1:{:x}:{:x} with SMTP id {}csp{};        {date}", h(1) & 0xffff, h(2) & 0xfff, h(3) & 0xfff, &b64_noise(h(4), 6), h(5) % 10_000_000),
        ),
        (
            "X-Google-Smtp-Source".into(),
            format!("AGHT+{}", b64_noise(h(6), 68)),
        ),
        (
            "X-Received".into(),
            format!("by 2002:a17:90b:{:x}:b0:2f4:{:x} with SMTP id {}-{}.{}.{};        {date}", h(7) & 0xfff, h(8) & 0xfff, b64_noise(h(9), 20), h(10) % 100000, h(11) % 10000, h(12) % 1_000_000_000),
        ),
        (
            "ARC-Seal".into(),
            format!("i=1; a=rsa-sha256; t={}; cv=none;        d=google.com; s=arc-20240605;        b={}", m.date / 1000, b64_noise(h(13), 344)),
        ),
        (
            "ARC-Message-Signature".into(),
            format!("i=1; a=rsa-sha256; c=relaxed/relaxed; d=google.com; s=arc-20240605;        h=to:subject:message-id:date:from:mime-version:dkim-signature;        bh={}=;        fh=Ke4Bv6wZ0x1n3pQ=;        b={}", b64_noise(h(14), 43), b64_noise(h(15), 344)),
        ),
        (
            "ARC-Authentication-Results".into(),
            format!("i=1; mx.google.com;       dkim=pass header.i=@{from_dom} header.s=s1 header.b={};       spf=pass (google.com: domain of bounce@{from_dom} designates 192.0.2.{} as permitted sender) smtp.mailfrom=bounce@{from_dom};       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from={from_dom}", b64_noise(h(16), 8), h(17) % 250),
        ),
        ("Return-Path".into(), format!("<bounce@{from_dom}>")),
        (
            "Received".into(),
            format!("from mail-sor-f41.{from_dom} (mail-sor-f41.{from_dom}. [192.0.2.{}])        by mx.google.com with SMTPS id {}.{}.2025.{}.{}        for <{}>        (Google Transport Security);        {date}", h(18) % 250, b64_noise(h(19), 24), h(20) % 1000, h(21) % 100, h(22) % 1000, m.account_id),
        ),
        (
            "Received-SPF".into(),
            format!("pass (google.com: domain of bounce@{from_dom} designates 192.0.2.{} as permitted sender) client-ip=192.0.2.{};", h(17) % 250, h(17) % 250),
        ),
        (
            "Authentication-Results".into(),
            format!("mx.google.com;       dkim=pass header.i=@{from_dom} header.s=s1 header.b={};       spf=pass (google.com: domain of bounce@{from_dom} designates 192.0.2.{} as permitted sender) smtp.mailfrom=bounce@{from_dom};       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from={from_dom}", b64_noise(h(16), 8), h(17) % 250),
        ),
        (
            "DKIM-Signature".into(),
            format!("v=1; a=rsa-sha256; c=relaxed/relaxed;        d={from_dom}; s=s1; t={};        h=to:subject:message-id:date:from:mime-version:from:to:cc:subject:date:message-id:reply-to;        bh={}=;        b={}", m.date / 1000, b64_noise(h(23), 43), b64_noise(h(24), 344)),
        ),
        ("MIME-Version".into(), "1.0".into()),
        ("From".into(), addr(&m.from)),
        ("Date".into(), date),
    ];
    if let Some(id) = &m.message_id_header {
        out.push(("Message-ID".into(), id.clone()));
    } else {
        out.push((
            "Message-ID".into(),
            format!("<{}@{from_dom}>", b64_noise(h(25), 24)),
        ));
    }
    out.push(("Subject".into(), m.subject.clone()));
    out.push(("To".into(), addrs(&m.to)));
    if !m.cc.is_empty() {
        out.push(("Cc".into(), addrs(&m.cc)));
    }
    if let Some(u) = &m.list_unsubscribe {
        out.push(("List-Unsubscribe".into(), u.clone()));
        out.push((
            "X-Mailer".into(),
            "Example ESP Mailer (mta-41.esp.example)".into(),
        ));
        out.push((
            "Feedback-ID".into(),
            format!("{}:{}:esp", h(26) % 100000, h(27) % 1000),
        ));
    }
    out.push(("Content-Type".into(), content_type.into()));
    out
}

fn boundary(m: &Message, salt: u64) -> String {
    format!("000000000000{:016x}", hash(&m.id, salt))
}

fn gmail_headers(list: &[(String, String)]) -> Value {
    Value::Array(
        list.iter()
            .map(|(n, v)| json!({"name": n, "value": v}))
            .collect(),
    )
}

/// Gmail's attachment ids are long opaque tokens (~400 chars).
fn gmail_attachment_id(m: &Message, i: usize) -> String {
    format!("ANGjdJ{}", b64_noise(hash(&m.id, 100 + i as u64), 400))
        .replace('+', "-")
        .replace('/', "_")
}

fn gmail_text_part(part_id: &str, mime: &str, text: &str) -> Value {
    let data = base64::engine::general_purpose::URL_SAFE.encode(text.as_bytes());
    json!({
        "partId": part_id,
        "mimeType": mime,
        "filename": "",
        "headers": [
            {"name": "Content-Type", "value": format!("{mime}; charset=\"UTF-8\"")},
            {"name": "Content-Transfer-Encoding", "value": "quoted-printable"},
        ],
        "body": {"size": text.len(), "data": data},
    })
}

/// Gmail `messages.get?format=full` JSON for `m`, as Google serializes it.
pub fn gmail_full_json(m: &Message) -> Vec<u8> {
    let html_only = html_only(m);
    // The body: text/plain, text/html, or alternative of both.
    let body_part = |prefix: &str| -> (Value, String) {
        let pid = |i: usize| {
            if prefix.is_empty() {
                i.to_string()
            } else {
                format!("{prefix}.{i}")
            }
        };
        match (&m.body_html, html_only) {
            (Some(html), true) => (
                gmail_text_part(
                    if prefix.is_empty() { "" } else { prefix },
                    "text/html",
                    html,
                ),
                "text/html; charset=\"UTF-8\"".into(),
            ),
            (Some(html), false) => {
                let b = boundary(m, 1);
                let ct = format!("multipart/alternative; boundary=\"{b}\"");
                (
                    json!({
                        "partId": prefix,
                        "mimeType": "multipart/alternative",
                        "filename": "",
                        "headers": [{"name": "Content-Type", "value": ct}],
                        "body": {"size": 0},
                        "parts": [
                            gmail_text_part(&pid(0), "text/plain", &m.body_text),
                            gmail_text_part(&pid(1), "text/html", html),
                        ],
                    }),
                    ct,
                )
            }
            (None, _) => (
                gmail_text_part(
                    if prefix.is_empty() { "" } else { prefix },
                    "text/plain",
                    &m.body_text,
                ),
                "text/plain; charset=\"UTF-8\"".into(),
            ),
        }
    };
    let payload = if m.attachments.is_empty() {
        let (mut p, ct) = body_part("");
        p["partId"] = json!("");
        p["headers"] = gmail_headers(&top_headers(m, &ct));
        p
    } else {
        let b = boundary(m, 2);
        let ct = format!("multipart/mixed; boundary=\"{b}\"");
        let (first, _) = body_part("0");
        let mut parts = vec![first];
        for (i, a) in m.attachments.iter().enumerate() {
            let mut headers = vec![
                json!({"name": "Content-Type", "value": format!("{}; name=\"{}\"", a.mime_type, a.filename)}),
                json!({"name": "Content-Disposition", "value": format!("{}; filename=\"{}\"", if a.inline { "inline" } else { "attachment" }, a.filename)}),
                json!({"name": "Content-Transfer-Encoding", "value": "base64"}),
                json!({"name": "X-Attachment-Id", "value": format!("f_{:x}", hash(&m.id, 200 + i as u64) & 0xffff_ffff)}),
            ];
            if let Some(cid) = &a.content_id {
                headers.push(json!({"name": "Content-ID", "value": format!("<{cid}>")}));
            }
            parts.push(json!({
                "partId": (i + 1).to_string(),
                "mimeType": a.mime_type,
                "filename": a.filename,
                "headers": headers,
                "body": {"attachmentId": gmail_attachment_id(m, i), "size": a.size},
            }));
        }
        json!({
            "partId": "",
            "mimeType": "multipart/mixed",
            "filename": "",
            "headers": gmail_headers(&top_headers(m, &ct)),
            "body": {"size": 0},
            "parts": parts,
        })
    };
    let snippet = m
        .snippet
        .replace('&', "&amp;")
        .replace('\'', "&#39;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let size_estimate = m.body_text.len()
        + m.body_html.as_ref().map_or(0, String::len)
        + m.attachments.iter().map(|a| a.size as usize).sum::<usize>()
        + 4000;
    serde_json::to_vec(&json!({
        "id": m.id,
        "threadId": m.thread_id,
        "labelIds": m.label_ids,
        "snippet": snippet,
        "payload": payload,
        "sizeEstimate": size_estimate,
        "historyId": (hash(&m.id, 30) % 10_000_000).to_string(),
        "internalDate": m.date.to_string(),
    }))
    .expect("json")
}

/// Quoted-printable (RFC 2045 §6.7), soft breaks at 76 columns.
pub fn quoted_printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 20);
    let mut col = 0;
    for line in text.split('\n') {
        for &b in line.as_bytes() {
            let enc = match b {
                b'=' => format!("={b:02X}"),
                b'\t' | b' ' | 33..=126 => (b as char).to_string(),
                _ => format!("={b:02X}"),
            };
            if col + enc.len() > 75 {
                out.push_str("=\r\n");
                col = 0;
            }
            col += enc.len();
            out.push_str(&enc);
        }
        out.push_str("\r\n");
        col = 0;
    }
    out
}

/// Attachment bytes in the RFC 822 encoding are capped at this (the
/// corpus's sizes go to 5 MB; the cap keeps a benchmark corpus in memory
/// while still sending real, incompressible attachment data).
pub const RFC822_ATTACHMENT_CAP: usize = 256 * 1024;

fn base64_lines(bytes: &[u8]) -> String {
    let s = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut out = String::with_capacity(s.len() + s.len() / 38);
    for chunk in s.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push_str("\r\n");
    }
    out
}

/// RFC 5322 bytes of `m` (what IMAP `BODY[]` returns).
pub fn rfc822(m: &Message) -> Vec<u8> {
    let html_only = html_only(m);
    let text_part = |mime: &str, text: &str| {
        format!(
            "Content-Type: {mime}; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{}",
            quoted_printable(text)
        )
    };
    let alt_b = boundary(m, 1);
    let (body_ct, body) = match (&m.body_html, html_only) {
        (Some(html), true) => (
            "text/html; charset=\"UTF-8\"".to_string(),
            format!(
                "Content-Transfer-Encoding: quoted-printable\r\n\r\n{}",
                quoted_printable(html)
            ),
        ),
        (Some(html), false) => (
            format!("multipart/alternative; boundary=\"{alt_b}\""),
            format!(
                "\r\n--{alt_b}\r\n{}\r\n--{alt_b}\r\n{}\r\n--{alt_b}--\r\n",
                text_part("text/plain", &m.body_text),
                text_part("text/html", html)
            ),
        ),
        (None, _) => (
            "text/plain; charset=\"UTF-8\"".to_string(),
            format!(
                "Content-Transfer-Encoding: quoted-printable\r\n\r\n{}",
                quoted_printable(&m.body_text)
            ),
        ),
    };
    let mut out = String::new();
    let (ct, rest) = if m.attachments.is_empty() {
        (body_ct.clone(), body)
    } else {
        let b = boundary(m, 2);
        let mut s = format!("\r\n--{b}\r\nContent-Type: {body_ct}\r\n");
        // `body` starts with its own part headers (or a blank line for multipart).
        s.push_str(&body);
        for (i, a) in m.attachments.iter().enumerate() {
            let n = (a.size as usize).min(RFC822_ATTACHMENT_CAP);
            s.push_str(&format!(
                "\r\n--{b}\r\nContent-Type: {}; name=\"{}\"\r\nContent-Disposition: {}; filename=\"{}\"\r\nContent-Transfer-Encoding: base64\r\nX-Attachment-Id: f_{:x}\r\n",
                a.mime_type,
                a.filename,
                if a.inline { "inline" } else { "attachment" },
                a.filename,
                hash(&m.id, 200 + i as u64) & 0xffff_ffff
            ));
            if let Some(cid) = &a.content_id {
                s.push_str(&format!("Content-ID: <{cid}>\r\n"));
            }
            s.push_str("\r\n");
            s.push_str(&base64_lines(&noise_bytes(hash(&m.id, 300 + i as u64), n)));
        }
        s.push_str(&format!("--{b}--\r\n"));
        (format!("multipart/mixed; boundary=\"{b}\""), s)
    };
    for (n, v) in top_headers(m, &ct) {
        out.push_str(&n);
        out.push_str(": ");
        out.push_str(&v);
        out.push_str("\r\n");
    }
    // A single-part body starts with its Content-Transfer-Encoding header
    // line; a multipart one with the blank line that ends the headers.
    out.push_str(&rest);
    out.into_bytes()
}

fn graph_recipient(a: &penguin_core::Address) -> Value {
    json!({"emailAddress": {"name": a.name.clone().unwrap_or_else(|| a.email.clone()), "address": a.email}})
}

/// Graph `GET /me/messages/{id}?$select=…&$expand=attachments(…)` JSON, with
/// `Prefer: outlook.body-content-type="html"`.
pub fn graph_json(m: &Message) -> Vec<u8> {
    let html = m.body_html.clone().unwrap_or_else(|| {
        format!(
            "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body><div>{}</div></body></html>",
            m.body_text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('\n', "<br>")
        )
    });
    let date = chrono::DateTime::from_timestamp_millis(m.date)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let headers: Vec<Value> = top_headers(m, "multipart/alternative")
        .into_iter()
        .map(|(n, v)| json!({"name": n, "value": v}))
        .collect();
    let attachments: Vec<Value> = m
        .attachments
        .iter()
        .enumerate()
        .map(|(i, a)| {
            json!({
                "@odata.type": "#microsoft.graph.fileAttachment",
                "@odata.mediaContentType": a.mime_type,
                "id": format!("AAMkAG{}", b64_noise(hash(&m.id, 400 + i as u64), 140)),
                "name": a.filename,
                "contentType": a.mime_type,
                "size": a.size,
                "isInline": a.inline,
            })
        })
        .collect();
    let preview: String = m.body_text.chars().take(255).collect();
    serde_json::to_vec(&json!({
        "@odata.etag": format!("W/\"CQAAABYAAAB{}\"", b64_noise(hash(&m.id, 40), 30)),
        "id": format!("AAMkAG{}", b64_noise(hash(&m.id, 41), 140)),
        "conversationId": format!("AAQkAG{}", b64_noise(hash(&m.thread_id, 42), 60)),
        "receivedDateTime": date,
        "sentDateTime": date,
        "subject": m.subject,
        "bodyPreview": preview,
        "body": {"contentType": "html", "content": html},
        "from": graph_recipient(&m.from),
        "sender": graph_recipient(&m.from),
        "toRecipients": m.to.iter().map(graph_recipient).collect::<Vec<_>>(),
        "ccRecipients": m.cc.iter().map(graph_recipient).collect::<Vec<_>>(),
        "bccRecipients": [],
        "replyTo": [],
        "isRead": !m.label_ids.iter().any(|l| l == "UNREAD"),
        "isDraft": false,
        "flag": {"flagStatus": "notFlagged"},
        "categories": [],
        "parentFolderId": "AAMkAGInbox",
        "internetMessageId": m.message_id_header.clone().unwrap_or_else(|| format!("<{}@mail.example>", m.id)),
        "hasAttachments": m.attachments.iter().any(|a| !a.inline),
        "internetMessageHeaders": headers,
        "attachments": attachments,
    }))
    .expect("json")
}
