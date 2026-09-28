//! The quoted original in outgoing replies and forwards, in Gmail's own
//! markup so Gmail (and clients that know its classes) fold it the way they
//! fold Gmail's: a forward is `<div class="gmail_quote">` holding the
//! "Forwarded message" header block (`gmail_attr`) and the original's HTML;
//! a reply is the attribution line plus `<blockquote class="gmail_quote">`.
//!
//! The original's HTML is untrusted mail: it goes through
//! [`penguin_render::sanitize_quoted_html`] (the composer's outgoing
//! allowlist with tables kept as blocks), so formatting, lists and links
//! survive and nothing active, remote or tracking does. Its inline (`cid:`)
//! images stay when [`Original::cids`] maps them to the ids the new message
//! gives their copies ([`crate::inline::quote_images`]). A message with no
//! HTML part is quoted from its text, escaped, one paragraph per blank-line
//! block.
//!
//! The composer builds the same markup in TypeScript
//! (apps/desktop/src/features/compose/quote.ts); this copy serves the
//! backend's own sends (rule forwards). Keep them alike.

use penguin_core::Address;

/// Style of a reply's quote bar (Gmail's, in values the outgoing sanitizer
/// keeps: no `rgb()`).
pub const QUOTE_STYLE: &str = "margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex";

/// First line of a forwarded message's header block (Gmail's text).
pub const FORWARD_MARKER: &str = "---------- Forwarded message ---------";

/// The message being forwarded or replied to.
pub struct Original<'a> {
    pub from: &'a Address,
    pub to: &'a [Address],
    pub cc: &'a [Address],
    pub subject: &'a str,
    /// Already formatted for the reader ("Mon, Sep 22, 2026 at 9:41 AM").
    pub date: &'a str,
    /// The stored HTML body (raw, untrusted), if the message had one.
    pub html: Option<&'a str>,
    pub text: &'a str,
    /// Its inline images the quote keeps, rewritten to the new message's
    /// ids; None (or empty) drops every image.
    pub cids: Option<&'a penguin_render::CidMap>,
}

/// Escape for text content (never used inside attributes). Only what the
/// HTML serializer itself escapes in text, so the send-time sanitizer
/// leaves the result byte for byte.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            _ => out.push(c),
        }
    }
    out
}

/// "Name <email>" (or the bare address) for text headers.
pub fn address_text(a: &Address) -> String {
    match a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} <{}>", a.email),
        None => a.email.clone(),
    }
}

fn address_html(a: &Address) -> String {
    match a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!(
            "<strong>{}</strong> &lt;{}&gt;",
            escape(n),
            escape(&a.email)
        ),
        None => format!("&lt;{}&gt;", escape(&a.email)),
    }
}

/// Plain text as HTML paragraphs (escaped; single newlines become `<br>`).
pub fn text_to_html(text: &str) -> String {
    text.split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            format!(
                "<p>{}</p>",
                escape(p.trim_matches('\n')).replace('\n', "<br>")
            )
        })
        .collect()
}

/// The original's body as quotable HTML: its HTML part sanitized, else its
/// text as paragraphs.
pub fn body_html(original: &Original<'_>) -> String {
    match original.html.filter(|h| !h.trim().is_empty()) {
        Some(h) => match original.cids {
            Some(cids) => penguin_render::sanitize_quoted_html_with_images(h, cids),
            None => penguin_render::sanitize_quoted_html(h),
        },
        None => text_to_html(original.text),
    }
}

/// The forward header block as text lines (marker, From, Date, Subject, To, Cc).
pub fn forward_header_lines(original: &Original<'_>) -> Vec<String> {
    let list = |l: &[Address]| l.iter().map(address_text).collect::<Vec<_>>().join(", ");
    let mut lines = vec![
        FORWARD_MARKER.to_string(),
        format!("From: {}", address_text(original.from)),
        format!("Date: {}", original.date),
        format!("Subject: {}", original.subject),
        format!("To: {}", list(original.to)),
    ];
    if !original.cc.is_empty() {
        lines.push(format!("Cc: {}", list(original.cc)));
    }
    lines
}

/// A forward's text part below what the sender wrote: header block, blank
/// line, the original's text.
pub fn forward_text(original: &Original<'_>) -> String {
    format!(
        "{}\n\n{}",
        forward_header_lines(original).join("\n"),
        original.text
    )
}

/// A forward's HTML below what the sender wrote.
pub fn forward_html(original: &Original<'_>) -> String {
    let list = |l: &[Address]| l.iter().map(address_html).collect::<Vec<_>>().join(", ");
    let mut header = format!(
        "{FORWARD_MARKER}<br>From: {}<br>Date: {}<br>Subject: {}<br>To: {}<br>",
        address_html(original.from),
        escape(original.date),
        escape(original.subject),
        list(original.to)
    );
    if !original.cc.is_empty() {
        header.push_str(&format!("Cc: {}<br>", list(original.cc)));
    }
    format!(
        r#"<div class="gmail_quote"><div dir="ltr" class="gmail_attr">{header}</div><br><br>{}</div>"#,
        body_html(original)
    )
}

/// A reply's HTML below what the sender wrote: the attribution line
/// ("On …, Dana <dana@…> wrote:") and the original in a quote bar.
pub fn reply_html(attribution: &str, original: &Original<'_>) -> String {
    format!(
        r#"<div class="gmail_quote"><div dir="ltr" class="gmail_attr">{}<br></div><blockquote class="gmail_quote" style="{QUOTE_STYLE}">{}</blockquote></div>"#,
        escape(attribution),
        body_html(original)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(name: Option<&str>, email: &str) -> Address {
        Address {
            name: name.map(str::to_string),
            email: email.to_string(),
        }
    }

    #[test]
    fn forward_keeps_the_originals_formatting_in_gmails_markup() {
        let from = addr(Some("Dana <Ops> & Co"), "dana@acme.example");
        let to = [addr(None, "me@penguin.example")];
        let o = Original {
            from: &from,
            to: &to,
            cc: &[],
            subject: "Scope <v3>",
            date: "Mon, Sep 22, 2026 at 9:41 AM",
            html: Some(
                r#"<h2>Plan</h2><p><b>bold</b> <a href="https://acme.example/x">x</a></p><script>alert(1)</script><img src="https://t.example/p.gif">"#,
            ),
            text: "Plan\n\nbold x",
            cids: None,
        };
        let html = forward_html(&o);
        assert!(html.starts_with(r#"<div class="gmail_quote"><div dir="ltr" class="gmail_attr">---------- Forwarded message ---------<br>From: <strong>Dana &lt;Ops&gt; &amp; Co</strong> &lt;dana@acme.example&gt;<br>"#), "{html}");
        assert!(html.contains("Subject: Scope &lt;v3&gt;<br>To: &lt;me@penguin.example&gt;<br></div><br><br><h2>Plan</h2><p><b>bold</b> <a href=\"https://acme.example/x\">x</a></p>"), "{html}");
        assert!(!html.contains("script") && !html.contains("<img"), "{html}");
        // It passes the send-time sanitizer unchanged.
        assert_eq!(penguin_render::sanitize_outgoing_html(&html), html);
        assert_eq!(
            forward_text(&o),
            "---------- Forwarded message ---------\nFrom: Dana <Ops> & Co <dana@acme.example>\nDate: Mon, Sep 22, 2026 at 9:41 AM\nSubject: Scope <v3>\nTo: me@penguin.example\n\nPlan\n\nbold x"
        );
    }

    #[test]
    fn mapped_inline_images_stay_in_the_quote() {
        let from = addr(None, "dana@acme.example");
        let cids = penguin_render::cid_map([("logo@acme.example", "img-1@penguin")]);
        let o = Original {
            from: &from,
            to: &[],
            cc: &[],
            subject: "s",
            date: "d",
            html: Some(
                r#"<p><img src="cid:logo@acme.example" alt="Acme"><img src="https://t.example/p.gif"></p>"#,
            ),
            text: "",
            cids: Some(&cids),
        };
        let html = forward_html(&o);
        assert!(
            html.contains(r#"<p><img src="cid:img-1@penguin" alt="Acme"></p>"#),
            "{html}"
        );
        assert!(!html.contains("t.example"), "{html}");
        let keep = penguin_render::keep_cids(["img-1@penguin"]);
        assert_eq!(
            penguin_render::sanitize_outgoing_html_with_images(&html, &keep),
            html
        );
        // Without a map the image goes.
        let o = Original { cids: None, ..o };
        assert!(!forward_html(&o).contains("<img"));
    }

    #[test]
    fn reply_quotes_in_a_gmail_blockquote_and_text_only_mail_is_escaped() {
        let from = addr(None, "dana@acme.example");
        let o = Original {
            from: &from,
            to: &[],
            cc: &[],
            subject: "s",
            date: "d",
            html: None,
            text: "a <b>\nline two\n\npara",
            cids: None,
        };
        let html = reply_html("On d, dana@acme.example wrote:", &o);
        assert_eq!(
            html,
            r#"<div class="gmail_quote"><div dir="ltr" class="gmail_attr">On d, dana@acme.example wrote:<br></div><blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex"><p>a &lt;b&gt;<br>line two</p><p>para</p></blockquote></div>"#
        );
        assert_eq!(penguin_render::sanitize_outgoing_html(&html), html);
    }
}
