use super::*;
use crate::{render_html, render_text, RenderOptions};

/// Through the real sanitizer, as the app calls it, mapped back to the
/// link as written.
fn find(html: &str) -> Option<String> {
    let opts = RenderOptions {
        strip_link_tracking: true,
        ..RenderOptions::default()
    };
    let r = render_html(html, &opts);
    find_sign_in_link(&r.html).map(|l| r.original_link(&l))
}

#[test]
fn the_button_by_its_text_not_the_footer() {
    let html = r#"<table><tr><td><a href="https://rydeo.example/"><img src="cid:logo" alt=""></a></td></tr>
        <tr><td><p>Click the button below to sign in to Rydeo. This link expires in 15 minutes.</p>
        <a href="https://rydeo.example/auth/magic?token=abc123&amp;fbclid=x">Sign in to Rydeo</a></td></tr>
        <tr><td>If you didn't request this, ignore this email.
        <a href="https://rydeo.example/privacy">Privacy</a> · <a href="https://rydeo.example/help">Help</a>
        <a href="https://rydeo.example/login">Log in</a></td></tr></table>"#;
    // The token survives tracking-parameter stripping (the original link).
    assert_eq!(
        find(html).as_deref(),
        Some("https://rydeo.example/auth/magic?token=abc123&fbclid=x")
    );
}

#[test]
fn a_generic_button_next_to_sign_in_wording() {
    let html = r#"<p>Here's the link you asked for to log in to Paperleaf.</p>
        <p><a href="https://paperleaf.example/a/9f8e7d">Continue</a></p>
        <p><a href="https://paperleaf.example/unsubscribe?u=1">Unsubscribe</a></p>"#;
    assert_eq!(
        find(html).as_deref(),
        Some("https://paperleaf.example/a/9f8e7d")
    );
}

#[test]
fn a_plain_text_email_with_one_link() {
    let text = "Hi,\n\nUse this link to sign in to Ledgerly:\nhttps://ledgerly.example/l/Zx81q\n\nIt expires in 10 minutes.";
    let r = render_text(text);
    assert_eq!(
        find_sign_in_link(&r.html).as_deref(),
        Some("https://ledgerly.example/l/Zx81q")
    );
}

#[test]
fn nothing_when_no_link_stands_out() {
    // Two unlabeled links: no guess.
    let html = r#"<p>Welcome!</p><a href="https://a.example/x">Read the guide</a>
        <a href="https://a.example/y">See what's new</a>"#;
    assert_eq!(find(html), None);
    // Only boilerplate and http.
    let html = r#"<a href="https://a.example/privacy">Privacy policy</a>
        <a href="http://a.example/login">Sign in</a>"#;
    assert_eq!(find(html), None);
    // mailto and the unsubscribe link never.
    let html = r#"<p>Sign in below.</p><a href="mailto:help@a.example">Sign in help</a>
        <a href="https://a.example/u">Unsubscribe</a>"#;
    assert_eq!(find(html), None);
}
