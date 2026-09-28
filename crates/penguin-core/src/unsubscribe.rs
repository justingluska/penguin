//! Unsubscribe: what the thread view's Unsubscribe button would do for a
//! message, decided from what's stored (no network).
//!
//! Sources, in order of trust:
//! 1. `List-Unsubscribe` (RFC 2369) with `List-Unsubscribe-Post:
//!    List-Unsubscribe=One-Click` (RFC 8058) and an authenticated sender:
//!    a one-click POST, made from Rust by the app.
//! 2. A `mailto:` in List-Unsubscribe: an email from the receiving account
//!    (sent directly only for an authenticated sender; otherwise the
//!    composer opens with it filled in).
//! 3. An https URL in List-Unsubscribe: opened in the browser.
//! 4. A link in the sanitized body whose text matches an unsubscribe phrase
//!    (`penguin_render::unsubscribe`): opened in the browser.
//!
//! Nothing here acts: every unsubscribe is a user click, confirmed.
//! Mail in Spam, and mail you sent, never gets an offer (unsubscribing from
//! spam tells the spammer the address is live).

use serde::{Deserialize, Serialize};

use crate::types::Message;

/// Longest subject / body taken from a mailto URI.
const MAX_MAILTO_SUBJECT: usize = 200;
const MAX_MAILTO_BODY: usize = 2000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UnsubscribeMethod {
    /// RFC 8058 POST from the app; nothing opens.
    OneClick,
    /// Send the mailto message from the receiving account.
    Mailto,
    /// Open the unsubscribe page in the browser.
    Link,
}

impl UnsubscribeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            UnsubscribeMethod::OneClick => "oneClick",
            UnsubscribeMethod::Mailto => "mailto",
            UnsubscribeMethod::Link => "link",
        }
    }

    pub fn parse(s: &str) -> Option<UnsubscribeMethod> {
        match s {
            "oneClick" => Some(UnsubscribeMethod::OneClick),
            "mailto" => Some(UnsubscribeMethod::Mailto),
            "link" => Some(UnsubscribeMethod::Link),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UnsubscribeSource {
    /// The List-Unsubscribe header.
    Header,
    /// A link in the message body.
    Body,
}

/// A parsed `mailto:` unsubscribe address.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Mailto {
    /// One addr-spec; extra recipients in the URI are ignored.
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// A remembered unsubscribe from a sender on one account.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeRecord {
    /// Unix ms.
    pub at: i64,
    pub method: UnsubscribeMethod,
}

/// What the Unsubscribe button offers for one message (sent to the UI).
/// Mirrored in apps/desktop/src/lib/types.ts. The UI never hands a URL back:
/// `unsubscribe` re-plans from the stored message. Only a `link` offer
/// carries its URL, for the confirm to show before the browser opens it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeOffer {
    pub method: UnsubscribeMethod,
    pub source: UnsubscribeSource,
    /// Host of the link, or the domain of the mailto address.
    pub domain: String,
    /// The mailto message (method `mailto`), for the confirm and "Edit first".
    pub mailto: Option<Mailto>,
    /// Gmail authenticated the sender (DMARC / aligned DKIM). One-click and
    /// sending the mailto directly need it.
    pub verified: bool,
    /// The message was stored before Penguin kept List-Unsubscribe-Post, so
    /// one-click might be available: `unsubscribe_check` fetches the headers.
    pub needs_check: bool,
    /// You already unsubscribed from this sender on this account.
    pub unsubscribed: Option<UnsubscribeRecord>,
    /// The page a `link` offer opens (https, or http for a body link), shown
    /// in the confirm so you can see where you're going. Display only: never
    /// read back from the UI. None for one-click (nothing opens) and mailto.
    pub link_url: Option<String>,
}

/// An offer plus the URL to act on (https; never sent to the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsubscribePlan {
    pub offer: UnsubscribeOffer,
    pub url: Option<String>,
}

/// The targets of a List-Unsubscribe header we can use: the first https URL
/// and the first mailto (RFC 2369: use the first method you support).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderTargets {
    pub https: Option<String>,
    pub mailto: Option<Mailto>,
}

/// URIs in a List-Unsubscribe value, in order: each `<…>` with whitespace
/// removed (RFC 2369 says to ignore it, and folding puts it there). Values
/// without angle brackets (non-conforming senders) are split on commas.
pub fn header_uris(value: &str) -> Vec<String> {
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
    if !value.contains('<') {
        return value
            .split(',')
            .map(clean)
            .filter(|s| !s.is_empty())
            .collect();
    }
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else { break };
        let uri = clean(&after[..close]);
        if !uri.is_empty() {
            out.push(uri);
        }
        rest = &after[close + 1..];
    }
    out
}

/// https and mailto targets of a List-Unsubscribe value. `http:` is ignored:
/// a POST or open over plain HTTP would expose the token.
pub fn parse_header(value: &str) -> HeaderTargets {
    let mut t = HeaderTargets::default();
    for uri in header_uris(value) {
        let lower = uri.to_ascii_lowercase();
        if lower.starts_with("https://") {
            if t.https.is_none() && https_host(&uri).is_some() {
                t.https = Some(uri);
            }
        } else if lower.starts_with("mailto:") && t.mailto.is_none() {
            t.mailto = parse_mailto(&uri);
        }
    }
    t
}

/// `List-Unsubscribe-Post: List-Unsubscribe=One-Click` (RFC 8058), exactly.
pub fn is_one_click(post: &str) -> bool {
    post.trim()
        .eq_ignore_ascii_case("List-Unsubscribe=One-Click")
}

/// Host of an `http(s)://` URL, lowercased, or None when there's no usable
/// host (userinfo is refused: `https://bank.example@evil.example/`).
pub fn https_host(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return None;
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    valid.then(|| host.trim_end_matches('.').to_string())
}

/// `mailto:addr?subject=…&body=…` → one address plus subject and body
/// (percent-decoded, single line subject, capped). None without a plain
/// `local@domain` address.
pub fn parse_mailto(uri: &str) -> Option<Mailto> {
    if !uri.get(..7)?.eq_ignore_ascii_case("mailto:") {
        return None;
    }
    let rest = &uri[7..];
    let (addrs, query) = rest.split_once('?').unwrap_or((rest, ""));
    let first = percent_decode(addrs.split(',').next().unwrap_or(""));
    let mut to = first.trim().to_string();
    let (mut subject, mut body) = (String::new(), String::new());
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k.to_ascii_lowercase().as_str() {
            "subject" => subject = percent_decode(v),
            "body" => body = percent_decode(v),
            // `mailto:?to=addr` form; extra recipients are never added.
            "to" if to.is_empty() => {
                to = percent_decode(v)
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            }
            _ => {}
        }
    }
    if !is_plain_address(&to) {
        return None;
    }
    let subject: String = subject
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_MAILTO_SUBJECT)
        .collect();
    let subject = subject.trim();
    Some(Mailto {
        to: to.to_ascii_lowercase(),
        subject: if subject.is_empty() {
            "unsubscribe".to_string()
        } else {
            subject.to_string()
        },
        body: body.chars().take(MAX_MAILTO_BODY).collect(),
    })
}

fn is_plain_address(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !local.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"' | '@')
        })
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a message can get an offer at all.
fn eligible(m: &Message) -> bool {
    !m.from.email.is_empty()
        && !m
            .label_ids
            .iter()
            .any(|l| matches!(l.as_str(), "SPAM" | "SENT" | "DRAFT"))
}

/// Decide what Unsubscribe does for `m`. `body_link` is called only when
/// the header gives nothing (it scans the rendered body).
pub fn plan(m: &Message, body_link: impl FnOnce() -> Option<String>) -> Option<UnsubscribePlan> {
    if !eligible(m) {
        return None;
    }
    let verified = m.sender_authenticated;
    let targets = m
        .list_unsubscribe
        .as_deref()
        .map(parse_header)
        .unwrap_or_default();
    // Only an authenticated sender's post flag matters, so only then is it
    // worth fetching headers to learn it.
    let needs_check = verified && targets.https.is_some() && m.list_unsubscribe_post.is_none();
    let offer = |method, source, domain: String, mailto: Option<Mailto>| UnsubscribeOffer {
        method,
        source,
        domain,
        mailto,
        verified,
        needs_check,
        unsubscribed: None,
        link_url: None,
    };
    if let Some(url) = &targets.https {
        if verified && m.list_unsubscribe_post == Some(true) {
            return Some(UnsubscribePlan {
                offer: offer(
                    UnsubscribeMethod::OneClick,
                    UnsubscribeSource::Header,
                    https_host(url)?,
                    None,
                ),
                url: Some(url.clone()),
            });
        }
    }
    if let Some(mailto) = targets.mailto {
        let domain = mailto.to.rsplit('@').next().unwrap_or_default().to_string();
        return Some(UnsubscribePlan {
            offer: offer(
                UnsubscribeMethod::Mailto,
                UnsubscribeSource::Header,
                domain,
                Some(mailto),
            ),
            url: None,
        });
    }
    if let Some(url) = targets.https {
        return Some(UnsubscribePlan {
            offer: UnsubscribeOffer {
                link_url: Some(url.clone()),
                ..offer(
                    UnsubscribeMethod::Link,
                    UnsubscribeSource::Header,
                    https_host(&url)?,
                    None,
                )
            },
            url: Some(url),
        });
    }
    let url = body_link()?;
    Some(UnsubscribePlan {
        offer: UnsubscribeOffer {
            needs_check: false,
            link_url: Some(url.clone()),
            ..offer(
                UnsubscribeMethod::Link,
                UnsubscribeSource::Body,
                https_host(&url)?,
                None,
            )
        },
        url: Some(url),
    })
}

#[cfg(test)]
#[path = "unsubscribe_tests.rs"]
mod tests;
