use super::*;
use crate::types::{Address, Message};

fn msg(list: Option<&str>, post: Option<bool>, authenticated: bool) -> Message {
    Message {
        account_id: "me@example.com".into(),
        id: "m1".into(),
        thread_id: "t1".into(),
        date: 0,
        from: Address {
            name: Some("Weekly".into()),
            email: "news@weekly.example".into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "This week".into(),
        snippet: String::new(),
        body_text: String::new(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: list.map(str::to_string),
        list_unsubscribe_post: post,
        sender_authenticated: authenticated,
    }
}

fn no_body() -> Option<String> {
    None
}

#[test]
fn header_uris_multiple_in_order() {
    assert_eq!(
        header_uris("<mailto:u@list.example?subject=unsub>, <https://list.example/u?t=1,2>"),
        vec![
            "mailto:u@list.example?subject=unsub".to_string(),
            // A comma inside the brackets belongs to the URI.
            "https://list.example/u?t=1,2".to_string(),
        ]
    );
}

#[test]
fn header_uris_ignore_whitespace_and_folding() {
    let folded = "<https://list.example/unsub/\r\n abc123>,\r\n\t<mailto: u@list.example >";
    assert_eq!(
        header_uris(folded),
        vec![
            "https://list.example/unsub/abc123".to_string(),
            "mailto:u@list.example".to_string()
        ]
    );
    // Non-conforming: no brackets.
    assert_eq!(
        header_uris("https://list.example/u , mailto:u@list.example"),
        vec![
            "https://list.example/u".to_string(),
            "mailto:u@list.example".to_string()
        ]
    );
    assert!(header_uris("").is_empty());
    assert!(header_uris("<>, < >").is_empty());
}

#[test]
fn parse_header_takes_first_https_and_mailto() {
    let t = parse_header(
        "<http://insecure.example/u>, <https://a.example/1>, <https://b.example/2>, <mailto:x@a.example>, <javascript:alert(1)>",
    );
    assert_eq!(t.https.as_deref(), Some("https://a.example/1"));
    assert_eq!(t.mailto.unwrap().to, "x@a.example");
    // Only unusable schemes.
    assert_eq!(
        parse_header("<http://a.example/u>, <javascript:alert(1)>, <data:text/html,x>"),
        HeaderTargets::default()
    );
    // Userinfo host tricks are refused.
    assert_eq!(
        parse_header("<https://bank.example@evil.example/u>").https,
        None
    );
}

#[test]
fn mailto_subject_body_and_decoding() {
    let m =
        parse_mailto("mailto:Unsub%2Bx@List.example?subject=Remove%20me&body=Please%20remove%0Ame")
            .unwrap();
    assert_eq!(m.to, "unsub+x@list.example");
    assert_eq!(m.subject, "Remove me");
    assert_eq!(m.body, "Please remove\nme");
    // Defaults and case-insensitive keys.
    let m = parse_mailto("MAILTO:u@list.example?SUBJECT=").unwrap();
    assert_eq!(m.subject, "unsubscribe");
    assert_eq!(m.body, "");
    // Subject is one line (no header injection).
    let m = parse_mailto("mailto:u@list.example?subject=a%0D%0ABcc:%20x@evil.example").unwrap();
    assert!(!m.subject.contains('\n') && !m.subject.contains('\r'));
    // Only the first recipient; `to=` never adds more.
    let m = parse_mailto("mailto:a@list.example,b@other.example?to=c@evil.example").unwrap();
    assert_eq!(m.to, "a@list.example");
    let m = parse_mailto("mailto:?to=c@list.example").unwrap();
    assert_eq!(m.to, "c@list.example");
    // Not addresses.
    for bad in [
        "mailto:",
        "mailto:nobody",
        "mailto:a@b",
        "mailto:\"a b\"@list.example",
        "mailto:a@list.example<script>",
        "https://list.example",
        "mail",
    ] {
        assert!(parse_mailto(bad).is_none(), "{bad}");
    }
}

#[test]
fn one_click_post_value() {
    assert!(is_one_click("List-Unsubscribe=One-Click"));
    assert!(is_one_click("  list-unsubscribe=one-click\r\n"));
    assert!(!is_one_click("List-Unsubscribe=One-Click, x=1"));
    assert!(!is_one_click(""));
}

#[test]
fn host_extraction() {
    assert_eq!(
        https_host("https://Mail.List.example/u?x").as_deref(),
        Some("mail.list.example")
    );
    assert_eq!(
        https_host("http://list.example:8080/u").as_deref(),
        Some("list.example")
    );
    assert_eq!(
        https_host("https://list.example."),
        Some("list.example".into())
    );
    assert_eq!(https_host("https://u:p@list.example/"), None);
    assert_eq!(https_host("https:///path"), None);
    assert_eq!(https_host("ftp://list.example"), None);
}

#[test]
fn one_click_needs_post_https_and_authentication() {
    let h = "<mailto:u@weekly.example>, <https://weekly.example/u/tok>";
    let p = plan(&msg(Some(h), Some(true), true), no_body).unwrap();
    assert_eq!(p.offer.method, UnsubscribeMethod::OneClick);
    assert_eq!(p.offer.domain, "weekly.example");
    assert_eq!(p.url.as_deref(), Some("https://weekly.example/u/tok"));
    assert!(!p.offer.needs_check);
    // Nothing opens for one-click, so the confirm gets no URL.
    assert_eq!(p.offer.link_url, None);

    // Unauthenticated: never one-click; the mailto is offered (to compose).
    let p = plan(&msg(Some(h), Some(true), false), no_body).unwrap();
    assert_eq!(p.offer.method, UnsubscribeMethod::Mailto);
    assert!(!p.offer.verified);

    // No post header: mailto first, then the link.
    let p = plan(&msg(Some(h), Some(false), true), no_body).unwrap();
    assert_eq!(p.offer.method, UnsubscribeMethod::Mailto);
    let only_https = "<https://weekly.example/u/tok>";
    let p = plan(&msg(Some(only_https), Some(false), true), no_body).unwrap();
    assert_eq!(p.offer.method, UnsubscribeMethod::Link);
    assert_eq!(p.offer.source, UnsubscribeSource::Header);
    assert_eq!(
        p.offer.link_url.as_deref(),
        Some("https://weekly.example/u/tok")
    );

    // Post unknown (stored before we kept it): a check is offered, and only
    // when the answer could matter.
    assert!(
        plan(&msg(Some(only_https), None, true), no_body)
            .unwrap()
            .offer
            .needs_check
    );
    assert!(
        !plan(&msg(Some(only_https), None, false), no_body)
            .unwrap()
            .offer
            .needs_check
    );
    assert!(
        !plan(&msg(Some("<mailto:u@weekly.example>"), None, true), no_body)
            .unwrap()
            .offer
            .needs_check
    );
}

#[test]
fn body_link_only_without_a_usable_header() {
    let body = || Some("https://click.weekly.example/unsub?id=1".to_string());
    let p = plan(&msg(None, None, true), body).unwrap();
    assert_eq!(p.offer.method, UnsubscribeMethod::Link);
    assert_eq!(p.offer.source, UnsubscribeSource::Body);
    assert_eq!(p.offer.domain, "click.weekly.example");
    assert_eq!(
        p.offer.link_url.as_deref(),
        Some("https://click.weekly.example/unsub?id=1")
    );
    // An http-only header is unusable, so the body link wins.
    let p = plan(&msg(Some("<http://weekly.example/u>"), None, true), body).unwrap();
    assert_eq!(p.offer.source, UnsubscribeSource::Body);
    // A usable header means the body is never scanned.
    let p = plan(&msg(Some("<mailto:u@weekly.example>"), None, true), || {
        panic!("body scanned")
    });
    let p = p.unwrap();
    assert_eq!(p.offer.method, UnsubscribeMethod::Mailto);
    assert_eq!(p.offer.link_url, None);
    assert!(plan(&msg(None, None, true), no_body).is_none());
}

#[test]
fn spam_sent_and_drafts_get_nothing() {
    let h = Some("<https://weekly.example/u>");
    for label in ["SPAM", "SENT", "DRAFT"] {
        let mut m = msg(h, Some(true), true);
        m.label_ids.push(label.into());
        assert!(plan(&m, no_body).is_none(), "{label}");
    }
}
