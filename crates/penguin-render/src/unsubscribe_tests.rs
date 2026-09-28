use super::*;
use crate::{render_html, render_text, RenderOptions};

/// Through the real sanitizer, as the app calls it.
fn find(html: &str) -> Option<String> {
    find_unsubscribe_link(&render_html(html, &RenderOptions::default()).html)
}

#[test]
fn footer_link_by_its_text() {
    let html = r#"<html><head><style>.u{color:#888}</style></head><body>
        <p>Big news this week. <a href="https://weekly.example/post/1">Read more</a></p>
        <table><tr><td class="u">You are receiving this because you subscribed.
        <a href="https://click.weekly.example/u?id=1&amp;t=abc">Unsubscribe</a> |
        <a href="https://click.weekly.example/prefs?id=1">Manage preferences</a></td></tr></table>
        </body></html>"#;
    // Strong beats weak even though the weak one comes later.
    assert_eq!(
        find(html).as_deref(),
        Some("https://click.weekly.example/u?id=1&t=abc")
    );
}

#[test]
fn weak_phrase_when_nothing_stronger() {
    let html = r#"<p>Hi</p><p><a href="https://app.example/settings/notifications">Notification settings</a></p>"#;
    assert_eq!(
        find(html).as_deref(),
        Some("https://app.example/settings/notifications")
    );
}

#[test]
fn nearby_text_for_click_here_links() {
    let html = r#"<p>Don't want these emails? To stop receiving them, <a href="https://news.example/x/9">click here</a>.</p>"#;
    assert_eq!(find(html).as_deref(), Some("https://news.example/x/9"));
    let after = r#"<p><a href="https://news.example/x/10">Click here</a> to unsubscribe.</p>"#;
    assert_eq!(find(after).as_deref(), Some("https://news.example/x/10"));
}

#[test]
fn nearest_the_footer_wins() {
    let html = r#"<p><a href="https://news.example/u/top">Unsubscribe</a></p>
        <p>Story</p>
        <p><a href="https://news.example/u/bottom">Unsubscribe</a></p>"#;
    assert_eq!(find(html).as_deref(), Some("https://news.example/u/bottom"));
}

#[test]
fn multilingual() {
    for (text, lang) in [
        ("Darse de baja", "es"),
        ("Cancelar suscripción", "es"),
        ("Se désabonner", "fr"),
        ("Désinscrivez-vous", "fr"),
        ("Newsletter abbestellen", "de"),
        ("Hier abmelden", "de"),
        ("Annulla iscrizione", "it"),
        ("Cancelar inscrição", "pt"),
        ("点击退订", "zh"),
        ("取消訂閱", "zh-Hant"),
        ("配信停止はこちら", "ja"),
        ("수신거부", "ko"),
        ("Отписаться от рассылки", "ru"),
        ("OPT-OUT", "en"),
    ] {
        let html =
            format!(r#"<p>Hallo</p><p><a href="https://list.example/{lang}">{text}</a></p>"#);
        assert_eq!(
            find(&html).as_deref(),
            Some(format!("https://list.example/{lang}").as_str()),
            "{text}"
        );
    }
}

#[test]
fn no_link_or_unrelated_links_find_nothing() {
    // The word without a link.
    assert_eq!(
        find("<p>You can unsubscribe at any time from your account page.</p>"),
        None
    );
    // "unsubscribe" in prose, but the link is a long unrelated sentence.
    assert_eq!(
        find(
            r#"<p>Reply to unsubscribe. <a href="https://blog.example/p">Read our long article about the new pricing model</a></p>"#
        ),
        None
    );
    // Word boundaries: not "resubscribers" or "unsubscribed" talk.
    assert_eq!(
        find(
            r#"<p><a href="https://x.example/a">Ops out today</a> <a href="https://x.example/b">removement</a></p>"#
        ),
        None
    );
    // Text in a different block doesn't count as "near".
    assert_eq!(
        find(
            r#"<p>To unsubscribe, reply STOP.</p><div><a href="https://shop.example/">Shop now</a></div>"#
        ),
        None
    );
    // <style> content is not text.
    assert_eq!(
        find(
            r#"<html><head><style>.unsubscribe{color:red}</style></head><body><p><a href="https://shop.example/">Go</a></p></body></html>"#
        ),
        None
    );
}

#[test]
fn unsafe_schemes_are_never_returned() {
    for href in [
        "javascript:alert(1)",
        "data:text/html,unsubscribe",
        "mailto:unsub@list.example",
        "vbscript:x",
        "/relative/unsubscribe",
        "https://user@evil.example/u",
    ] {
        let html = format!(r#"<p><a href="{href}">Unsubscribe</a></p>"#);
        assert_eq!(find(&html), None, "{href}");
        // Even if handed unsanitized markup directly.
        assert_eq!(find_unsubscribe_link(&html), None, "{href} (raw)");
    }
    // http is fine for opening in the browser.
    assert_eq!(
        find(r#"<p><a href="http://old.example/u">Unsubscribe</a></p>"#).as_deref(),
        Some("http://old.example/u")
    );
}

#[test]
fn plain_text_bodies() {
    let doc = render_text("Thanks for reading.\n\nUnsubscribe: https://list.example/u/42\n");
    // render_text links the URL; its own text is the URL, and the label
    // right before it names it.
    assert_eq!(
        find_unsubscribe_link(&doc.html).as_deref(),
        Some("https://list.example/u/42")
    );
}

#[test]
fn phrase_list_is_normalized() {
    for (p, _) in UNSUBSCRIBE_PHRASES {
        assert_eq!(normalize(p), *p, "phrases must be stored normalized");
    }
}

#[test]
fn entity_decoding() {
    assert_eq!(
        decode_entities("a&amp;b&lt;&#x41;&#66;&nbsp;&bogus;&"),
        "a&b<AB\u{a0}&bogus;&"
    );
    assert_eq!(decode_entities("é&amp;"), "é&");
}
