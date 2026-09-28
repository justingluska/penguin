//! "Message details" (click a message's time): the full address lists and
//! ids from the store, plus what only the headers know — who the mail was
//! mailed-by / signed-by (SPF / DKIM / DMARC from Gmail's own
//! Authentication-Results) and whether it arrived over TLS (Received). Those
//! headers aren't stored, so they come from a format=metadata fetch when the
//! modal opens; offline, the stored half still shows.

use penguin_core::{Address, AttachmentMeta, Message};
use serde::Serialize;

/// authserv-id of Gmail's receiving MX; only its verdicts are trusted (any
/// other Authentication-Results can be written by the sender).
const GOOGLE_AUTHSERV: &str = "mx.google.com";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MessageDetails {
    pub account_id: String,
    pub message_id: String,
    pub thread_id: String,
    pub subject: String,
    pub from: Address,
    pub reply_to: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    /// Unix ms (Gmail internalDate).
    pub date: i64,
    /// The Date header as written by the sender (keeps their timezone).
    pub date_header: Option<String>,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub label_ids: Vec<String>,
    pub attachments: Vec<AttachmentMeta>,
    /// Gmail's size estimate for the whole message, when headers were fetched.
    pub size: Option<u64>,
    /// Stored verdict: DMARC pass or aligned DKIM pass (see penguin-core).
    pub sender_authenticated: bool,
    pub auth: AuthInfo,
    pub transport: TransportInfo,
    /// Parsed List-Unsubscribe targets (https and mailto only).
    pub unsubscribe: Vec<String>,
    /// False when the header fetch failed (offline): auth and transport are unknown.
    pub headers_fetched: bool,
    /// Why the header fetch failed, for the UI to say so.
    pub headers_error: Option<String>,
}

/// Gmail's verdicts from its topmost Authentication-Results.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthInfo {
    /// "pass", "fail", "softfail", "neutral", "none", …
    pub spf: Option<String>,
    /// Domain of the envelope sender SPF checked (Gmail's "mailed-by").
    pub mailed_by: Option<String>,
    pub dkim: Option<String>,
    /// Domains whose DKIM signature passed (Gmail's "signed-by").
    pub signed_by: Vec<String>,
    pub dmarc: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransportInfo {
    /// The hop into Google used TLS (true), didn't (false), or unknown (None).
    pub tls: Option<bool>,
    /// e.g. "TLS1_3 · TLS_AES_256_GCM_SHA384".
    pub detail: Option<String>,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Result of `method=` in an Authentication-Results value ("spf=pass (…)").
fn method_result(ar: &str, method: &str) -> Option<String> {
    ar.split(';').find_map(|clause| {
        let c = clause.trim();
        let rest = c.strip_prefix(method)?.trim_start().strip_prefix('=')?;
        Some(rest.split_whitespace().next()?.to_ascii_lowercase())
    })
}

/// Value of `prop=` inside the clause for `method` (e.g. dkim … header.i=@x).
fn clause_props<'a>(ar: &'a str, method: &str) -> Vec<&'a str> {
    ar.split(';')
        .map(str::trim)
        .filter(|c| c.starts_with(method))
        .collect()
}

fn prop<'a>(clause: &'a str, name: &str) -> Option<&'a str> {
    clause.split_whitespace().find_map(|tok| {
        let v = tok.strip_prefix(name)?.strip_prefix('=')?;
        Some(v.trim_end_matches([';', ')', ',']))
    })
}

fn domain_of(v: &str) -> String {
    let v = v.trim_start_matches('@');
    v.rsplit('@')
        .next()
        .unwrap_or(v)
        .trim_matches(['<', '>', '"'])
        .to_ascii_lowercase()
}

/// Gmail's topmost Authentication-Results (anything below it, or from
/// another authserv-id, may have been written by the sender).
pub fn auth_info(headers: &[(String, String)]) -> AuthInfo {
    let Some(ar) = header(headers, "Authentication-Results") else {
        return AuthInfo::default();
    };
    let authserv = ar.split(';').next().unwrap_or("").trim();
    if !authserv.eq_ignore_ascii_case(GOOGLE_AUTHSERV) {
        return AuthInfo::default();
    }
    let spf = method_result(ar, "spf");
    let mailed_by = clause_props(ar, "spf")
        .into_iter()
        .find_map(|c| prop(c, "smtp.mailfrom").or_else(|| prop(c, "smtp.helo")))
        .map(domain_of)
        .filter(|d| !d.is_empty());
    let dkim = method_result(ar, "dkim");
    let mut signed_by: Vec<String> = Vec::new();
    for c in clause_props(ar, "dkim") {
        if !c
            .trim_start_matches("dkim")
            .trim_start()
            .starts_with("=pass")
        {
            continue;
        }
        if let Some(d) = prop(c, "header.d")
            .or_else(|| prop(c, "header.i"))
            .map(domain_of)
        {
            if !d.is_empty() && !signed_by.contains(&d) {
                signed_by.push(d);
            }
        }
    }
    AuthInfo {
        spf,
        mailed_by,
        dkim,
        signed_by,
        dmarc: method_result(ar, "dmarc"),
    }
}

/// TLS on the hop into Google: the topmost Received header written by a
/// Google MX ("by mx.google.com … with ESMTPS … (version=TLS1_3 cipher=…)").
pub fn transport_info(headers: &[(String, String)]) -> TransportInfo {
    let received = headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("Received"))
        .map(|(_, v)| v.as_str())
        .find(|v| v.contains("by mx.google.com"));
    let Some(r) = received else {
        return TransportInfo::default();
    };
    let token = |name: &str| {
        r.split(|c: char| c.is_whitespace() || c == '(')
            .find_map(|t| t.strip_prefix(name))
            .map(|v| v.trim_end_matches([')', ';']))
    };
    let version = token("version=");
    let cipher = token("cipher=");
    let with = r
        .split_whitespace()
        .skip_while(|t| *t != "with")
        .nth(1)
        .unwrap_or("");
    let tls = version.is_some()
        || with.eq_ignore_ascii_case("ESMTPS")
        || with.eq_ignore_ascii_case("ESMTPSA");
    let detail = match (version, cipher) {
        (Some(v), Some(c)) => Some(format!("{v} · {c}")),
        (Some(v), None) => Some(v.to_string()),
        _ => None,
    };
    TransportInfo {
        tls: Some(tls),
        detail,
    }
}

/// List-Unsubscribe targets we'd open: https and mailto links only.
pub fn unsubscribe_links(value: Option<&str>) -> Vec<String> {
    let Some(v) = value else { return vec![] };
    v.split(',')
        .filter_map(|p| {
            let u = p
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .trim();
            let lower = u.to_ascii_lowercase();
            (lower.starts_with("https://") || lower.starts_with("mailto:")).then(|| u.to_string())
        })
        .collect()
}

/// Headers fetched on demand (name, value in message order) plus Gmail's sizeEstimate.
pub type FetchedHeaders<'a> = (&'a [(String, String)], Option<u64>);

pub fn build(m: &Message, fetched: Option<FetchedHeaders<'_>>) -> MessageDetails {
    let (auth, transport, size, date_header) = match fetched {
        Some((h, size)) => (
            auth_info(h),
            transport_info(h),
            size,
            header(h, "Date").map(str::to_string),
        ),
        None => (AuthInfo::default(), TransportInfo::default(), None, None),
    };
    MessageDetails {
        account_id: m.account_id.clone(),
        message_id: m.id.clone(),
        thread_id: m.thread_id.clone(),
        subject: m.subject.clone(),
        from: m.from.clone(),
        reply_to: m.reply_to.clone(),
        to: m.to.clone(),
        cc: m.cc.clone(),
        bcc: m.bcc.clone(),
        date: m.date,
        date_header,
        message_id_header: m.message_id_header.clone(),
        in_reply_to: m.in_reply_to.clone(),
        references: m.references.clone(),
        label_ids: m.label_ids.clone(),
        attachments: m
            .attachments
            .iter()
            .filter(|a| !a.inline)
            .cloned()
            .collect(),
        size,
        sender_authenticated: m.sender_authenticated,
        auth,
        transport,
        unsubscribe: unsubscribe_links(m.list_unsubscribe.as_deref()),
        headers_fetched: fetched.is_some(),
        headers_error: None,
    }
}

/// Raw source for "Show original": lossy UTF-8 (the UI shows it as escaped
/// text), capped so a huge message can't stall the webview.
pub const MAX_SOURCE_BYTES: usize = 5 * 1024 * 1024;

pub fn source_text(raw: &[u8]) -> String {
    let shown = &raw[..raw.len().min(MAX_SOURCE_BYTES)];
    let mut s = String::from_utf8_lossy(shown).into_owned();
    if raw.len() > MAX_SOURCE_BYTES {
        s.push_str(&format!(
            "\n\n[… {} more bytes not shown. Download the message from Gmail for the full source.]\n",
            raw.len() - MAX_SOURCE_BYTES
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    const AR: &str = "mx.google.com;\r\n       dkim=pass header.i=@linden.example header.s=s1 header.b=abc;\r\n       spf=pass (google.com: domain of priya@linden.example designates 1.2.3.4 as permitted sender) smtp.mailfrom=priya@linden.example;\r\n       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=linden.example";

    #[test]
    fn reads_googles_verdicts() {
        let a = auth_info(&h(&[("Authentication-Results", AR)]));
        assert_eq!(a.spf.as_deref(), Some("pass"));
        assert_eq!(a.mailed_by.as_deref(), Some("linden.example"));
        assert_eq!(a.dkim.as_deref(), Some("pass"));
        assert_eq!(a.signed_by, vec!["linden.example"]);
        assert_eq!(a.dmarc.as_deref(), Some("pass"));
    }

    #[test]
    fn ignores_sender_written_results() {
        let forged =
            "evil.example; spf=pass smtp.mailfrom=bank.example; dkim=pass header.d=bank.example";
        assert_eq!(
            auth_info(&h(&[("Authentication-Results", forged)])),
            AuthInfo::default()
        );
        // Only the topmost header counts: a forged one below Google's is ignored,
        // and Google's own failing verdict is reported as is.
        let google_fail = "mx.google.com; spf=softfail smtp.mailfrom=x@bank.example; dkim=fail header.d=bank.example; dmarc=fail";
        let a = auth_info(&h(&[
            ("Authentication-Results", google_fail),
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.d=bank.example",
            ),
        ]));
        assert_eq!(a.spf.as_deref(), Some("softfail"));
        assert!(a.signed_by.is_empty());
        assert_eq!(a.dmarc.as_deref(), Some("fail"));
    }

    #[test]
    fn reads_tls_from_googles_received() {
        let r = "from mail-a.linden.example (mail-a.linden.example. [1.2.3.4])\r\n        by mx.google.com with ESMTPS id x1si\r\n        for <sam@northwind.example>\r\n        (version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384 bits=256/256);\r\n        Tue, 23 Sep 2026 09:41:02 -0700 (PDT)";
        let t = transport_info(&h(&[
            ("Received", "by 2002:a05:6a10:1234 with SMTP id x;"),
            ("Received", r),
        ]));
        assert_eq!(t.tls, Some(true));
        assert_eq!(t.detail.as_deref(), Some("TLS1_3 · TLS_AES_256_GCM_SHA384"));
        let plain =
            "from relay.example by mx.google.com with SMTP id y; Tue, 23 Sep 2026 09:41:02 -0700";
        assert_eq!(transport_info(&h(&[("Received", plain)])).tls, Some(false));
        assert_eq!(transport_info(&[]).tls, None);
    }

    #[test]
    fn unsubscribe_keeps_only_https_and_mailto() {
        let v = "<mailto:unsub@news.example?subject=unsubscribe>, <https://news.example/u/1>, <http://insecure.example/u>, <javascript:alert(1)>";
        assert_eq!(
            unsubscribe_links(Some(v)),
            vec![
                "mailto:unsub@news.example?subject=unsubscribe",
                "https://news.example/u/1"
            ]
        );
    }

    #[test]
    fn source_is_capped() {
        let big = vec![b'a'; MAX_SOURCE_BYTES + 10];
        let s = source_text(&big);
        assert!(s.contains("10 more bytes not shown"));
        assert_eq!(source_text(b"From: a\r\n\r\nhi"), "From: a\r\n\r\nhi");
    }
}
