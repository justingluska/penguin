//! Behavior tests for the public API: remote images, tracker stripping,
//! cid: rewriting, links, newsletter fidelity, and plain text.

use std::collections::HashMap;

use penguin_render::{render_html, render_text, RenderOptions, HTML_DOC_PREFIX, TEXT_DOC_PREFIX};

const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn blocked() -> RenderOptions {
    RenderOptions::default()
}

fn allowed() -> RenderOptions {
    RenderOptions {
        allow_remote_images: true,
        ..Default::default()
    }
}

/// The sanitized body, without our wrapper's head.
fn body(html: &str) -> &str {
    let start = html.find("<div class=\"pg-root\"").expect("root wrapper");
    &html[start..]
}

#[test]
fn remote_images_are_blocked_and_counted_by_distinct_url() {
    let html = r#"<img src="https://cdn.acme.example/hero.jpg" width="600">
        <img src="https://cdn.acme.example/hero.jpg" width="600">
        <img src="http://cdn.acme.example/logo.png" alt="Acme">
        <table background="https://cdn.acme.example/bg.png"><tr><td style="background-image:url('https://cdn.acme.example/cell.png')">x</td></tr></table>
        <img srcset="https://cdn.acme.example/a.png 1x, https://cdn.acme.example/a@2x.png 2x">"#;
    let r = render_html(html, &blocked());
    assert_eq!(r.blocked_remote_images, 6, "{}", r.html);
    assert_eq!(r.trackers_removed, 0);
    let b = body(&r.html);
    assert!(!b.contains("acme.example"), "remote URL leaked: {b}");
    assert!(
        b.contains("alt=\"Acme\""),
        "alt text kept for the placeholder"
    );
    assert!(b.contains("background-image:none"));
    assert!(
        r.html.contains("img-src data:;"),
        "CSP must not allow remote images when blocked"
    );
}

#[test]
fn allowed_images_load_over_https_only() {
    let html = r#"<img src="http://cdn.acme.example/logo.png"><img src="https://cdn.acme.example/hero.jpg">
        <img srcset="https://cdn.acme.example/a.png 1x, javascript:alert(1) 2x, https://cdn.acme.example/b.png 2x">"#;
    let r = render_html(html, &allowed());
    let b = body(&r.html);
    assert!(
        b.contains("src=\"https://cdn.acme.example/logo.png\""),
        "http upgraded: {b}"
    );
    assert!(b.contains("src=\"https://cdn.acme.example/hero.jpg\""));
    assert!(
        b.contains(
            "srcset=\"https://cdn.acme.example/a.png 1x, https://cdn.acme.example/b.png 2x\""
        ),
        "{b}"
    );
    assert_eq!(r.blocked_remote_images, 0);
    assert!(r.html.contains("img-src data: https:;"));
}

#[test]
fn tracking_pixels_are_removed_even_when_images_are_allowed() {
    let html = r#"<img src="https://cdn.acme.example/hero.jpg" width="600" height="300">
        <img src="https://news.acme.example/o/abc123" width="1" height="1" alt="">
        <img src="https://news.acme.example/o/zero" style="width:0;height:0;border:0">
        <img src="https://news.acme.example/o/hidden" style="display:none">
        <img src="https://acme.us1.list-manage.com/track/open.php?u=1&amp;id=2">
        <img src="https://u1.ct.sendgrid.net/wf/open?upn=abc">
        <img src="https://t.sidekickopen87.com/e1t/o/5/abc">
        <img src="https://r.superhuman.com/xyz.png">
        <img src="https://track.mixmax.com/api/track/v2/abc">
        <img src="https://t.yesware.com/t/abc/o.gif">
        <img src="https://mailfoogae.appspot.com/t?sender=abc">
        <div style="background:url(https://mailtrack.io/trace/mail/abc.png)">x</div>"#;
    for opts in [allowed(), blocked()] {
        let r = render_html(html, &opts);
        assert_eq!(
            r.trackers_removed, 11,
            "allow={}: {}",
            opts.allow_remote_images, r.html
        );
        let b = body(&r.html);
        for t in [
            "/o/abc123",
            "/o/zero",
            "/o/hidden",
            "list-manage",
            "sendgrid",
            "sidekickopen",
            "superhuman",
            "mixmax",
            "yesware",
            "mailfoogae",
            "mailtrack",
        ] {
            assert!(!b.contains(t), "tracker {t} survived");
        }
        if opts.allow_remote_images {
            assert!(
                b.contains("https://cdn.acme.example/hero.jpg"),
                "real image kept"
            );
            assert_eq!(r.blocked_remote_images, 0);
        } else {
            // Trackers are not counted as "blocked images" the user could load.
            assert_eq!(r.blocked_remote_images, 1);
        }
    }
}

#[test]
fn cid_images_are_inlined_from_the_map() {
    let mut cid_map = HashMap::new();
    cid_map.insert("<Logo@Acme.example>".to_string(), PNG.to_string());
    cid_map.insert(
        "page@acme.example".to_string(),
        "data:text/html;base64,PHNjcmlwdD4=".to_string(),
    );
    let opts = RenderOptions {
        allow_remote_images: false,
        cid_map,
        ..Default::default()
    };
    let html = r#"<img src="cid:logo@acme.example" alt="logo"><img src="CID:Logo%40Acme.example">
        <img src="cid:missing@acme.example" alt="missing"><img src="cid:page@acme.example" alt="page">
        <table><tr><td background="cid:logo@acme.example"></td></tr></table><div style="background:#fff url(cid:logo@acme.example) no-repeat">x</div>"#;
    let r = render_html(html, &opts);
    let b = body(&r.html);
    assert_eq!(b.matches(PNG).count(), 4, "{b}");
    assert!(
        b.contains(&format!("url(&quot;{PNG}&quot;)")),
        "css background rewritten: {b}"
    );
    assert!(!b.contains("cid:"), "no cid: left: {b}");
    assert!(!b.contains("text/html"), "non-image cid part rejected");
    assert!(b.contains("alt=\"missing\""));
    assert_eq!(r.blocked_remote_images, 0);
}

#[test]
fn links_open_in_a_new_context_without_referrer() {
    let r = render_html(
        r##"<a href="https://acme.example/sale" target="_self">Shop</a><a href="mailto:ada@lovelace.example">Mail</a><a href="#top">Top</a><a name="x">anchor</a>"##,
        &blocked(),
    );
    let b = body(&r.html);
    assert!(b.contains(r#"<a href="https://acme.example/sale" target="_blank" rel="noopener noreferrer">Shop</a>"#), "{b}");
    assert!(b.contains(r#"<a href="mailto:ada@lovelace.example" target="_blank" rel="noopener noreferrer">Mail</a>"#), "{b}");
    assert!(
        b.contains(r#"<a target="_blank" rel="noopener noreferrer">Top</a>"#),
        "relative href dropped: {b}"
    );
    assert!(r
        .html
        .contains("<meta name=\"referrer\" content=\"no-referrer\">"));
}

#[test]
fn newsletter_layout_survives() {
    let html = r##"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Weekly</title>
        <style type="text/css">
          body { margin:0; padding:0; } .wrapper { width:100%; background-color:#f4f4f4; }
          .btn a { color:#ffffff !important; text-decoration:none; }
          @media only screen and (max-width: 600px) { .container { width:100% !important; } }
          @media (prefers-color-scheme: dark) { .wrapper { background:#000 } }
        </style></head>
        <body bgcolor="#f4f4f4" style="margin:0;padding:0">
        <table class="wrapper" width="100%" cellpadding="0" cellspacing="0" border="0" role="presentation">
          <tr><td align="center" valign="top">
            <table class="container" width="600" cellpadding="20" style="background:#ffffff;border-radius:8px">
              <tr><td style="font-family:Helvetica,Arial,sans-serif;font-size:16px;line-height:24px;color:#333333">
                <h1 style="margin:0 0 12px;font-size:24px">This week</h1>
                <p>Hello <strong>reader</strong>, <font color="#ff6600" face="Georgia">news</font>.</p>
                <table class="btn" cellpadding="0" cellspacing="0"><tr><td bgcolor="#0b63ce" style="border-radius:6px;padding:10px 18px">
                  <a href="https://acme.example/read?utm_source=x&amp;id=1" style="color:#fff;font-weight:bold">Read more</a>
                </td></tr></table>
                <ul><li>One</li><li>Two</li></ul><hr size="1" color="#eeeeee">
              </td></tr>
            </table>
          </td></tr>
        </table></body></html>"##;
    let r = render_html(html, &blocked());
    let doc = &r.html;
    assert!(doc.starts_with(HTML_DOC_PREFIX));
    assert!(
        doc.contains(".wrapper{width:100%;background-color:#f4f4f4;}"),
        "{doc}"
    );
    assert!(doc.contains(".btn a{color:#ffffff !important;text-decoration:none;}"));
    assert!(doc
        .contains("@media only screen and (max-width: 600px){.container{width:100% !important;}}"));
    assert!(!doc.contains("prefers-color-scheme"));
    assert!(
        doc.contains(
            "<div class=\"pg-root\" style=\"background-color:#f4f4f4;margin:0;padding:0;\">"
        ),
        "{doc}"
    );
    let b = body(doc);
    for keep in [
        "<table class=\"wrapper\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\">",
        "<td align=\"center\" valign=\"top\">",
        "style=\"background:#ffffff;border-radius:8px;\"",
        "<font color=\"#ff6600\" face=\"Georgia\">news</font>",
        "<td bgcolor=\"#0b63ce\" style=\"border-radius:6px;padding:10px 18px;\">",
        "href=\"https://acme.example/read?utm_source=x&amp;id=1\"",
        "<hr size=\"1\" color=\"#eeeeee\">",
    ] {
        assert!(b.contains(keep), "missing {keep:?} in {b}");
    }
    assert!(
        !b.contains("<title>") && !b.contains("Weekly"),
        "title content removed"
    );
}

#[test]
fn plain_text_is_escaped_linkified_and_folded() {
    let text = "Hi Ada,\r\n\r\nSee https://acme.example/doc?a=1&b=2 or mail grace@hopper.example.\n<script>alert(1)</script>\n\nOn Tue, 3 Sep 2026, Grace <grace@hopper.example> wrote:\n> Earlier message\n> > even earlier\n> with <b>tags</b>\n\nThanks!";
    let r = render_text(text);
    let h = &r.html;
    assert!(h.starts_with(TEXT_DOC_PREFIX));
    assert!(h.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!h.contains("<script"));
    assert!(h.contains("<a href=\"https://acme.example/doc?a=1&amp;b=2\" target=\"_blank\" rel=\"noopener noreferrer\">"));
    assert!(h.contains("<a href=\"mailto:grace@hopper.example\""));
    assert_eq!(
        h.matches("<details class=\"pg-quote\">").count(),
        2,
        "outer + nested quote folded: {h}"
    );
    assert!(
        h.contains("<div class=\"pg-attribution\">On Tue, 3 Sep 2026, Grace &lt;"),
        "{h}"
    );
    assert!(h.contains("with &lt;b&gt;tags&lt;/b&gt;"));
    assert!(h.contains("</details>Thanks!"), "{h}");
    assert!(!h.contains("allow-scripts") && h.contains("default-src 'none'"));
}

#[test]
fn outlook_style_original_message_is_folded() {
    let r = render_text(
        "Sounds good.\n\n-----Original Message-----\nFrom: Grace\nSubject: Plan\n\nDetails",
    );
    assert!(
        r.html.contains("Sounds good.<details class=\"pg-quote\">"),
        "{}",
        r.html
    );
    assert!(r.html.contains("-----Original Message-----\nFrom: Grace"));
}

#[test]
fn loaded_images_never_target_this_machine_or_the_lan() {
    let html = r#"<img src="https://avatar.localhost/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.png">
        <img src="http://ipc.localhost/open_external"><img src="https://tauri.localhost/index.html"><img src="http://localhost:8080/x.png">
        <img src="http://192.168.1.1/cgi-bin/reboot"><img src="http://10.0.0.1/a.png"><img src="http://127.0.0.1/a.png">
        <img src="http://169.254.169.254/latest/meta-data/"><img src="http://[::1]/a.png"><img src="http://[fd00::1]/a.png">
        <img src="http://[::ffff:192.168.0.1]/a.png"><img src="http://2130706433/a.png"><img src="http://0x7f.1/a.png">
        <img src="http://printer.local/a.png"><img src="http://intranet/a.png"><img src="http://100.64.0.1/a.png">
        <div style="background:url(http://192.168.0.1/b.png)">x</div>
        <img src="https://cdn.acme.example/ok.png">"#;
    let r = render_html(html, &allowed());
    let b = body(&r.html);
    assert!(b.contains("https://cdn.acme.example/ok.png"), "{b}");
    assert_eq!(
        b.matches("src=\"").count(),
        1,
        "only the public image keeps a src: {b}"
    );
    for bad in [
        "localhost",
        "192.168",
        "10.0.0.1",
        "127.0.0.1",
        "169.254",
        "[::1]",
        "fd00",
        "printer.local",
        "intranet",
        "100.64",
        "0x7f",
        "2130706433",
    ] {
        assert!(!b.contains(bad), "{bad} survived: {b}");
    }
}
