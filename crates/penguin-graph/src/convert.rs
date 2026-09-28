//! Graph messages → `penguin_core::Message` (docs/PROVIDERS-IMPL.md §3–4).
//!
//! - id: the immutable id; thread: `conversationId`; date:
//!   `receivedDateTime` in unix ms.
//! - labels: the folder's label (see `folders`), UNREAD for `isRead:
//!   false`, STARRED for a flagged message, `c:<name>` per category.
//! - body: Graph's HTML (unsanitized, like every provider's; rendering
//!   sanitizes) and its text; snippet from `bodyPreview`.
//! - headers (when fetched): In-Reply-To, References, List-Unsubscribe(-Post),
//!   and `sender_authenticated` from Exchange Online's own
//!   Authentication-Results (see [`sender_authenticated`]).

use penguin_core::{Address, AttachmentMeta, Message};
use penguin_provider::ids::system;
pub(crate) use penguin_provider::text::snippet;
use penguin_provider::{Error, Result};

use crate::folders::category_label_id;
use crate::wire::{Header, Recipient, WireMessage};

/// Unix ms of a Graph timestamp (`2024-05-01T09:30:00Z`, with or without
/// fractional seconds).
pub(crate) fn parse_date_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Graph's timestamp for a unix ms value (`$filter` bounds).
pub(crate) fn format_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms.max(0))
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn address(r: &Recipient) -> Option<Address> {
    let email = r.email_address.address.as_deref()?.trim();
    if email.is_empty() {
        return None;
    }
    Some(Address {
        name: r
            .email_address
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(email))
            .map(str::to_string),
        email: email.to_string(),
    })
}

fn addresses(list: &Option<Vec<Recipient>>) -> Vec<Address> {
    list.iter().flatten().filter_map(address).collect()
}

fn bare(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

/// Message ids in a References / In-Reply-To value, without brackets.
fn message_ids(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start..].find('>') else {
            break;
        };
        let id = bare(&rest[start + 1..start + len]);
        if !id.is_empty() {
            out.push(id);
        }
        rest = &rest[start + len + 1..];
    }
    if out.is_empty() {
        out.extend(value.split_whitespace().map(bare).filter(|s| !s.is_empty()));
    }
    out
}

/// The labels a message carries, from its folder and flags. `folder`
/// None (archive, or a folder Penguin doesn't know) adds no folder label.
pub(crate) fn labels_for(w: &WireMessage, folder_label: Option<String>) -> Vec<String> {
    let mut labels = Vec::new();
    labels.extend(folder_label);
    let in_drafts = labels.iter().any(|l| l == system::DRAFT);
    if w.is_read == Some(false) && !in_drafts {
        labels.push(system::UNREAD.to_string());
    }
    if w.flag
        .as_ref()
        .is_some_and(|f| f.flag_status.eq_ignore_ascii_case("flagged"))
    {
        labels.push(system::STARRED.to_string());
    }
    for c in w.categories.iter().flatten() {
        let c = c.trim();
        if c.is_empty() {
            continue;
        }
        let id = category_label_id(c);
        if !labels.contains(&id) {
            labels.push(id);
        }
    }
    labels
}

/// Build a stored message. `full` = body and attachments were fetched
/// (else a headers-only message for `insert_header_messages`).
pub(crate) fn to_message(
    account_id: &str,
    w: &WireMessage,
    folder_label: Option<String>,
    full: bool,
) -> Result<Message> {
    if w.id.is_empty() {
        return Err(Error::Other(
            "Graph returned a message without an id".into(),
        ));
    }
    let date = w
        .received_date_time
        .as_deref()
        .and_then(parse_date_ms)
        .or_else(|| w.sent_date_time.as_deref().and_then(parse_date_ms))
        .filter(|d| *d > 0)
        .ok_or_else(|| Error::Other(format!("message {} has no usable date", w.id)))?;
    let thread_id = w
        .conversation_id
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| w.id.clone());
    let from = w
        .from
        .as_ref()
        .and_then(address)
        .or_else(|| w.sender.as_ref().and_then(address))
        .unwrap_or(Address {
            name: None,
            email: String::new(),
        });
    let (body_text, body_html) = match (&w.body, full) {
        (Some(b), true) if b.content_type.eq_ignore_ascii_case("html") => (
            penguin_core::text::html_to_text(&b.content),
            Some(b.content.clone()),
        ),
        (Some(b), true) => (b.content.clone(), None),
        _ => (String::new(), None),
    };
    let preview = w.body_preview.clone().unwrap_or_default();
    let snippet = if preview.trim().is_empty() {
        snippet(&body_text)
    } else {
        snippet(&preview)
    };
    let headers: &[Header] = w.internet_message_headers.as_deref().unwrap_or(&[]);
    let have_headers = w.internet_message_headers.is_some();
    let in_reply_to =
        header(headers, "In-Reply-To").and_then(|v| message_ids(v).into_iter().next());
    let references = header(headers, "References")
        .map(message_ids)
        .unwrap_or_default();
    let list_unsubscribe = header(headers, "List-Unsubscribe")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let list_unsubscribe_post = have_headers.then(|| {
        header(headers, "List-Unsubscribe-Post").is_some_and(|v| {
            v.to_ascii_lowercase()
                .contains("list-unsubscribe=one-click")
        })
    });
    let attachments = if full {
        w.attachments
            .iter()
            .flatten()
            .filter(|a| a.is_file() || a.is_item())
            .map(|a| {
                let mut filename = a
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or("attachment")
                    .to_string();
                let mime_type = if a.is_item() {
                    if !filename.contains('.') {
                        filename.push_str(".eml");
                    }
                    "message/rfc822".to_string()
                } else {
                    a.content_type
                        .clone()
                        .filter(|c| !c.trim().is_empty())
                        .unwrap_or_else(|| "application/octet-stream".into())
                };
                let content_id = a.content_id.as_deref().map(bare).filter(|c| !c.is_empty());
                AttachmentMeta {
                    id: a.id.clone(),
                    filename,
                    mime_type,
                    size: a.size,
                    inline: a.is_inline && content_id.is_some(),
                    content_id,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let sender_authenticated = sender_authenticated(headers, &from.email);
    let mut m = Message {
        account_id: account_id.to_string(),
        id: w.id.clone(),
        thread_id,
        date,
        from,
        to: addresses(&w.to_recipients),
        cc: addresses(&w.cc_recipients),
        bcc: addresses(&w.bcc_recipients),
        reply_to: addresses(&w.reply_to),
        subject: w.subject.clone().unwrap_or_default(),
        snippet,
        body_text,
        body_html,
        label_ids: labels_for(w, folder_label),
        attachments,
        message_id_header: w
            .internet_message_id
            .as_deref()
            .map(bare)
            .filter(|s| !s.is_empty()),
        in_reply_to,
        references,
        list_unsubscribe,
        list_unsubscribe_post,
        sender_authenticated,
    };
    m.settle_inline();
    Ok(m)
}

/// Did Exchange Online authenticate the From domain? Only the topmost
/// Authentication-Results counts (Exchange prepends its own above anything
/// the sender wrote), and only in Exchange's shape: no authserv-id, results
/// first (`spf=… ; dkim=… ; dmarc=pass action=none header.from=<domain>;
/// compauth=…`). True for `dmarc=pass` whose header.from is the From
/// domain. Anything else, including mail without such a header, is false.
pub(crate) fn sender_authenticated(headers: &[Header], from_email: &str) -> bool {
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
    // Exchange's header starts with a method result, not an authserv-id.
    let first = ar.trim_start().split(';').next().unwrap_or_default();
    let first_token = first.split_whitespace().next().unwrap_or_default();
    if !first_token.contains('=') {
        return false;
    }
    if !ar.to_ascii_lowercase().contains("compauth=") {
        return false;
    }
    for resinfo in ar.split(';') {
        let mut tokens = resinfo.split_whitespace();
        let Some((method, result)) = tokens.next().and_then(|t| t.split_once('=')) else {
            continue;
        };
        if !method.eq_ignore_ascii_case("dmarc") || !result.eq_ignore_ascii_case("pass") {
            continue;
        }
        let from = tokens
            .filter_map(|t| t.split_once('='))
            .find(|(k, _)| k.eq_ignore_ascii_case("header.from"))
            .map(|(_, v)| {
                v.trim_matches('"')
                    .trim_end_matches('.')
                    .to_ascii_lowercase()
            });
        if from.as_deref() == Some(from_domain.as_str()) {
            return true;
        }
    }
    false
}

/// All header lines of a raw RFC 822 message, unfolded, in order.
pub(crate) fn raw_headers(raw: &[u8]) -> Vec<(String, String)> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(raw.len());
    let text = String::from_utf8_lossy(&raw[..end]);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Attachment, EmailAddress, Flag, ItemBody};

    const A: &str = "sam@outlook.example";

    fn rcpt(name: &str, addr: &str) -> Recipient {
        Recipient {
            email_address: EmailAddress {
                name: Some(name.into()),
                address: Some(addr.into()),
            },
        }
    }

    fn sample() -> WireMessage {
        WireMessage {
            id: "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0AUmYz7bIa2kOkZ8zVv-G4ZgAAAYo=".into(),
            conversation_id: Some("AAQkADAwATM0MDAAMS1iNWUzLWU4ZjYtMDACLTAwCgAQAO1m6==".into()),
            received_date_time: Some("2025-03-04T09:30:15Z".into()),
            subject: Some("Quarterly figures".into()),
            body_preview: Some("Hi Sam,\r\n  the  figures are attached.".into()),
            body: Some(ItemBody {
                content_type: "html".into(),
                content: "<p>Hi Sam,</p><p>the figures are attached.</p><img src=\"cid:logo@quill.example\">".into(),
            }),
            from: Some(rcpt("Cy Quill", "cy@quill.example")),
            to_recipients: Some(vec![rcpt("Sam Rivers", A)]),
            is_read: Some(false),
            flag: Some(Flag {
                flag_status: "flagged".into(),
            }),
            categories: Some(vec!["Red category".into(), "Travel".into()]),
            parent_folder_id: Some("F-inbox".into()),
            internet_message_id: Some("<q1@quill.example>".into()),
            internet_message_headers: Some(vec![
                Header {
                    name: "Authentication-Results".into(),
                    value: "spf=pass (sender IP is 192.0.2.1) smtp.mailfrom=quill.example; dkim=pass (signature was verified) header.d=quill.example;dmarc=pass action=none header.from=quill.example;compauth=pass reason=100".into(),
                },
                Header {
                    name: "In-Reply-To".into(),
                    value: "<p0@outlook.example>".into(),
                },
                Header {
                    name: "References".into(),
                    value: "<r0@outlook.example>\r\n <p0@outlook.example>".into(),
                },
                Header {
                    name: "List-Unsubscribe".into(),
                    value: "<https://quill.example/u/1>".into(),
                },
                Header {
                    name: "List-Unsubscribe-Post".into(),
                    value: "List-Unsubscribe=One-Click".into(),
                },
            ]),
            attachments: Some(vec![
                Attachment {
                    odata_type: "#microsoft.graph.fileAttachment".into(),
                    id: "ATT1".into(),
                    name: Some("figures.pdf".into()),
                    content_type: Some("application/pdf".into()),
                    size: 2048,
                    is_inline: false,
                    content_id: None,
                },
                Attachment {
                    odata_type: "#microsoft.graph.fileAttachment".into(),
                    id: "ATT2".into(),
                    name: Some("logo.png".into()),
                    content_type: Some("image/png".into()),
                    size: 64,
                    is_inline: true,
                    content_id: Some("<logo@quill.example>".into()),
                },
                Attachment {
                    odata_type: "#microsoft.graph.itemAttachment".into(),
                    id: "ATT3".into(),
                    name: Some("Forwarded note".into()),
                    content_type: None,
                    size: 900,
                    is_inline: false,
                    content_id: None,
                },
                Attachment {
                    odata_type: "#microsoft.graph.referenceAttachment".into(),
                    id: "ATT4".into(),
                    name: Some("Shared doc".into()),
                    ..Attachment::default()
                },
            ]),
            ..WireMessage::default()
        }
    }

    #[test]
    fn full_messages_convert_and_pass_the_conformance_checks() {
        let m = to_message(A, &sample(), Some("INBOX".into()), true).unwrap();
        assert!(penguin_provider::conformance::check_message(
            penguin_core::AccountProvider::Microsoft,
            A,
            &m
        )
        .is_empty());
        assert_eq!(m.date, 1_741_080_615_000);
        assert_eq!(
            m.thread_id,
            "AAQkADAwATM0MDAAMS1iNWUzLWU4ZjYtMDACLTAwCgAQAO1m6=="
        );
        assert_eq!(
            m.label_ids,
            vec!["INBOX", "UNREAD", "STARRED", "c:Red%20category", "c:Travel"]
        );
        assert_eq!(m.snippet, "Hi Sam, the figures are attached.");
        assert!(m.body_text.contains("figures are attached"));
        assert!(m.body_html.as_deref().unwrap().contains("cid:logo"));
        assert_eq!(m.message_id_header.as_deref(), Some("q1@quill.example"));
        assert_eq!(m.in_reply_to.as_deref(), Some("p0@outlook.example"));
        assert_eq!(
            m.references,
            vec!["r0@outlook.example", "p0@outlook.example"]
        );
        assert_eq!(
            m.list_unsubscribe.as_deref(),
            Some("<https://quill.example/u/1>")
        );
        assert_eq!(m.list_unsubscribe_post, Some(true));
        assert!(m.sender_authenticated);
        assert_eq!(m.from.name.as_deref(), Some("Cy Quill"));
        // Reference (cloud link) attachments have no bytes: left out.
        assert_eq!(m.attachments.len(), 3);
        let inline = &m.attachments[1];
        assert!(inline.inline);
        assert_eq!(inline.content_id.as_deref(), Some("logo@quill.example"));
        let item = &m.attachments[2];
        assert_eq!(item.filename, "Forwarded note.eml");
        assert_eq!(item.mime_type, "message/rfc822");
    }

    #[test]
    fn headers_only_messages_have_no_body() {
        let mut w = sample();
        w.internet_message_headers = None;
        w.attachments = None;
        let m = to_message(A, &w, None, false).unwrap();
        assert!(m.body_text.is_empty() && m.body_html.is_none());
        assert_eq!(m.list_unsubscribe_post, None);
        assert!(!m.sender_authenticated);
        assert_eq!(m.label_ids[0], "UNREAD", "archived: no folder label");
    }

    #[test]
    fn drafts_are_never_unread() {
        let mut w = sample();
        w.flag = None;
        w.categories = None;
        let m = to_message(A, &w, Some("DRAFT".into()), true).unwrap();
        assert_eq!(m.label_ids, vec!["DRAFT"]);
    }

    #[test]
    fn sender_authentication_trusts_only_exchanges_topmost_header() {
        let h = |v: &str| {
            vec![Header {
                name: "Authentication-Results".into(),
                value: v.into(),
            }]
        };
        let ok = "spf=pass smtp.mailfrom=quill.example; dkim=pass header.d=quill.example;dmarc=pass action=none header.from=quill.example;compauth=pass reason=100";
        assert!(sender_authenticated(&h(ok), "cy@quill.example"));
        assert!(!sender_authenticated(&h(ok), "cy@other.example"));
        // A sender-written header with an authserv-id is not Exchange's.
        assert!(!sender_authenticated(
            &h("mx.quill.example; dmarc=pass header.from=quill.example; compauth=pass"),
            "cy@quill.example"
        ));
        assert!(!sender_authenticated(
            &h("spf=fail smtp.mailfrom=quill.example;dmarc=fail action=quarantine header.from=quill.example;compauth=fail"),
            "cy@quill.example"
        ));
        assert!(!sender_authenticated(&[], "cy@quill.example"));
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(
            parse_date_ms("2025-03-04T09:30:15.123Z"),
            Some(1_741_080_615_123)
        );
        assert_eq!(format_date(1_741_080_615_123), "2025-03-04T09:30:15Z");
        assert_eq!(parse_date_ms("yesterday"), None);
    }

    #[test]
    fn raw_headers_unfold() {
        let raw = b"Subject: Hello\r\n there\r\nFrom: Cy <cy@quill.example>\r\n\r\nbody: not a header\r\n";
        assert_eq!(
            raw_headers(raw),
            vec![
                ("Subject".to_string(), "Hello there".to_string()),
                ("From".to_string(), "Cy <cy@quill.example>".to_string())
            ]
        );
    }
}
