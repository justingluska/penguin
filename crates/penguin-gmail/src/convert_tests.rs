use super::*;

fn fixture(name: &str) -> GmailMessage {
    let json = match name {
        "alternative" => include_str!("../tests/fixtures/alternative.json"),
        "mixed_attachments" => include_str!("../tests/fixtures/mixed_attachments.json"),
        "inline_cid" => include_str!("../tests/fixtures/inline_cid.json"),
        "windows1252" => include_str!("../tests/fixtures/windows1252.json"),
        "malformed" => include_str!("../tests/fixtures/malformed.json"),
        "google_invite" => include_str!("../tests/fixtures/google_invite.json"),
        "google_invite_updated" => include_str!("../tests/fixtures/google_invite_updated.json"),
        "google_invite_updated_out_of_line" => {
            include_str!("../tests/fixtures/google_invite_updated_out_of_line.json")
        }
        other => panic!("unknown fixture {other}"),
    };
    serde_json::from_str(json).expect("fixture parses")
}

fn convert(name: &str) -> Message {
    to_message("me@penguin.example", &fixture(name), &HashMap::new())
}

fn addr(name: Option<&str>, email: &str) -> Address {
    Address {
        name: name.map(str::to_string),
        email: email.to_string(),
    }
}

#[test]
fn multipart_alternative_headers_and_bodies() {
    let m = convert("alternative");
    assert_eq!(m.account_id, "me@penguin.example");
    assert_eq!((m.id.as_str(), m.thread_id.as_str()), ("18f0a1", "18f0a0"));
    assert_eq!(m.date, 1_758_600_000_000);
    assert_eq!(m.from, addr(Some("Doe, Jane"), "jane@acme.example"));
    assert_eq!(
        m.to,
        vec![
            addr(Some("Bob Stone"), "bob@acme.example"),
            addr(None, "carol@acme.example")
        ]
    );
    assert_eq!(
        m.cc,
        vec![addr(Some("Zoë Łukasiewicz"), "zoe@acme.example")]
    );
    assert_eq!(
        m.reply_to,
        vec![addr(Some("Planning"), "plans@acme.example")]
    );
    assert!(m.bcc.is_empty());
    assert_eq!(m.subject, "Café ☕ plans");
    assert_eq!(m.snippet, "Hi team, here's the plan for Thursday & Friday");
    assert_eq!(
        m.body_text,
        "Hi team,\nHere's the plan — Thursday & Friday.\n"
    );
    assert!(m.body_html.as_deref().unwrap().contains("<b>Thursday</b>"));
    assert_eq!(
        m.message_id_header.as_deref(),
        Some("abc123@mail.acme.example")
    );
    assert_eq!(m.in_reply_to.as_deref(), Some("parent@mail.acme.example"));
    assert_eq!(
        m.references,
        vec!["root@mail.acme.example", "parent@mail.acme.example"]
    );
    assert_eq!(
        m.list_unsubscribe.as_deref(),
        Some("<mailto:unsub@acme.example>, <https://acme.example/u/1>")
    );
    assert_eq!(m.label_ids, vec!["INBOX", "UNREAD", "IMPORTANT"]);
    assert!(m.is_unread());
    assert!(m.attachments.is_empty());
}

#[test]
fn nested_mixed_with_attachments() {
    let m = convert("mixed_attachments");
    assert_eq!(m.body_text, "See attached.\n");
    assert_eq!(m.body_html.as_deref(), Some("<p>See attached.</p>"));
    let summary: Vec<(&str, &str, &str, u64, bool)> = m
        .attachments
        .iter()
        .map(|a| {
            (
                a.id.as_str(),
                a.filename.as_str(),
                a.mime_type.as_str(),
                a.size,
                a.inline,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "ANGjdJ_pdf",
                "Q3 report.pdf",
                "application/pdf",
                204_800,
                false
            ),
            ("ANGjdJ_txt", "notes.txt", "text/plain", 1200, false),
            ("ANGjdJ_eml", "untitled.eml", "message/rfc822", 5000, false),
            // Small part Gmail inlined: addressed by part id.
            ("part:4", "untitled.ics", "text/calendar", 30, false),
        ]
    );
    // The inlined part's bytes are recoverable for get_attachment.
    let gm = fixture("mixed_attachments");
    assert_eq!(
        inline_part_bytes(&gm, "4").unwrap(),
        b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"
    );
    // Stale attachment ids are re-matched by filename + size.
    let mut stale = m.attachments[0].clone();
    stale.id = "OLD".into();
    assert_eq!(
        rematch_attachment_id(&gm, &stale).as_deref(),
        Some("ANGjdJ_pdf")
    );
}

#[test]
fn inline_cid_image_and_html_only_body() {
    let m = convert("inline_cid");
    assert_eq!(m.attachments.len(), 1);
    let a = &m.attachments[0];
    assert_eq!(a.content_id.as_deref(), Some("logo123@acme.example"));
    assert!(a.inline);
    assert_eq!(a.filename, "logo.png");
    assert_eq!(a.mime_type, "image/png");
    assert!(m
        .body_html
        .as_deref()
        .unwrap()
        .contains("cid:logo123@acme.example"));
    // No text/plain part → body_text derived from the HTML.
    assert!(
        m.body_text.contains("Welcome to"),
        "body_text = {:?}",
        m.body_text
    );
    assert!(m.body_text.contains("Acme"));
    assert!(!m.body_text.contains('<'));
}

/// Apple Mail sends an attached PDF as `Content-Disposition: inline`, with a
/// Content-ID and an `<img src="cid:…">` placeholder. The body can't draw a
/// PDF, so it's listed as a file; so is an inline part with no Content-ID.
#[test]
fn inline_disposition_pdf_is_listed() {
    let mut json: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/inline_cid.json")).unwrap();
    let part = |id: &str, mime: &str, name: &str, cid: Option<&str>| {
        let mut headers = vec![
            serde_json::json!({"name": "Content-Disposition", "value": format!("inline; filename={name}")}),
        ];
        if let Some(cid) = cid {
            headers.push(serde_json::json!({"name": "Content-ID", "value": format!("<{cid}>")}));
        }
        serde_json::json!({
            "partId": id, "mimeType": mime, "filename": name, "headers": headers,
            "body": {"attachmentId": format!("ANGjdJ_{id}"), "size": 4096}
        })
    };
    let parts = json["payload"]["parts"].as_array_mut().unwrap();
    parts.push(part(
        "2",
        "application/pdf",
        "Lease_signed.pdf",
        Some("pdf-2@ruiz.example"),
    ));
    parts.push(part("3", "image/jpeg", "photo.jpg", None));
    let gm: GmailMessage = serde_json::from_value(json).unwrap();
    let m = to_message("me@penguin.example", &gm, &HashMap::new());
    let inline: Vec<_> = m
        .attachments
        .iter()
        .map(|a| (a.filename.as_str(), a.inline))
        .collect();
    assert_eq!(
        inline,
        [
            ("logo.png", true),
            ("Lease_signed.pdf", false),
            ("photo.jpg", false)
        ]
    );
}

#[test]
fn non_utf8_charset_body_and_encoded_headers() {
    let m = convert("windows1252");
    assert_eq!(m.body_text, "Grüße aus Köln — naïve café");
    assert_eq!(m.subject, "Grüße");
    assert_eq!(
        m.from,
        addr(Some("Jürgen Müller"), "juergen@beispiel.example")
    );
    assert!(m.body_html.is_none());
}

#[test]
fn malformed_parts_do_not_panic_or_drop_good_parts() {
    let m = convert("malformed");
    assert_eq!(m.subject, "Broken");
    assert_eq!(m.from.email, "");
    assert_eq!(m.date, 0);
    // Undecodable part skipped, valid sibling kept; empty attachment dropped.
    assert_eq!(m.body_text, "fine\n");
    assert!(m.body_html.is_none());
    assert!(m.attachments.is_empty());

    // No payload at all.
    let bare: GmailMessage = serde_json::from_str(r#"{"id":"x","threadId":"y"}"#).unwrap();
    let m = to_message("me@penguin.example", &bare, &HashMap::new());
    assert_eq!(
        (m.id.as_str(), m.body_text.as_str(), m.subject.as_str()),
        ("x", "", "")
    );
}

#[test]
fn out_of_line_body_is_fetched_via_extra_map() {
    let gm: GmailMessage = serde_json::from_value(serde_json::json!({
        "id": "big", "threadId": "t", "internalDate": "1",
        "payload": {
            "mimeType": "text/plain", "partId": "",
            "headers": [{ "name": "Content-Type", "value": "text/plain; charset=iso-8859-1" }],
            "body": { "attachmentId": "BODY1", "size": 9_000_000 }
        }
    }))
    .unwrap();
    assert_eq!(out_of_line_bodies(&gm), vec!["BODY1"]);
    let extra = HashMap::from([("BODY1".to_string(), b"caf\xe9".to_vec())]);
    let m = to_message("me@penguin.example", &gm, &extra);
    assert_eq!(m.body_text, "café");
    assert!(m.attachments.is_empty());
}

#[test]
fn decode_text_charsets() {
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("日本語のメール");
    assert_eq!(decode_text(&sjis, Some("Shift_JIS")), "日本語のメール");
    assert_eq!(decode_text("héllo".as_bytes(), None), "héllo");
    // Unknown label → UTF-8.
    assert_eq!(decode_text("héllo".as_bytes(), Some("x-made-up")), "héllo");
    assert_eq!(decode_text(b"plain", Some("us-ascii")), "plain");
    // Mislabelled: declared Shift_JIS, bytes are UTF-8 and invalid SJIS → UTF-8 wins.
    assert_eq!(
        decode_text("résumé ✓".as_bytes(), Some("shift_jis")),
        "résumé ✓"
    );
}

#[test]
fn snippet_entities() {
    assert_eq!(
        unescape_snippet("a &amp; b &lt;c&gt; &#39;d&#39; &#x263A; &bogus; &"),
        "a & b <c> 'd' ☺ &bogus; &"
    );
}

#[test]
fn base64url_padding_and_whitespace() {
    assert_eq!(b64url_decode("aGk").unwrap(), b"hi");
    assert_eq!(b64url_decode("aGk=").unwrap(), b"hi");
    assert_eq!(b64url_decode("aG\r\nk=").unwrap(), b"hi");
    assert!(b64url_decode("!!").is_none());
}

/// `cargo test -p penguin-gmail --release -- --ignored convert_throughput --nocapture`
#[test]
#[ignore = "benchmark"]
fn convert_throughput() {
    let fixtures: Vec<GmailMessage> = [
        "alternative",
        "mixed_attachments",
        "inline_cid",
        "windows1252",
    ]
    .into_iter()
    .map(fixture)
    .collect();
    let n = 20_000;
    let t = std::time::Instant::now();
    for i in 0..n {
        std::hint::black_box(to_message(
            "me@penguin.example",
            &fixtures[i % fixtures.len()],
            &HashMap::new(),
        ));
    }
    let per_sec = n as f64 / t.elapsed().as_secs_f64();
    println!("convert: {per_sec:.0} messages/s");
    // Must never be the bottleneck against the ~50 msgs/s Gmail quota.
    assert!(per_sec > 1_000.0);
}

// ---------- sender authentication ----------

const GOOGLE_PASS: &str = "mx.google.com;\r\n       dkim=pass header.i=@acme.example header.s=s1 header.b=AbC123;\r\n       spf=pass (google.com: domain of bounce@acme.example designates 192.0.2.1 as permitted sender) smtp.mailfrom=bounce@acme.example;\r\n       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=acme.example";

fn with_headers(from: &str, headers: &[(&str, &str)]) -> Message {
    let mut all = vec![serde_json::json!({ "name": "From", "value": from })];
    all.extend(
        headers
            .iter()
            .map(|(n, v)| serde_json::json!({ "name": n, "value": v })),
    );
    let gm: GmailMessage = serde_json::from_value(serde_json::json!({
        "id": "a", "threadId": "t",
        "payload": { "mimeType": "text/plain", "headers": all, "body": { "size": 2, "data": "aGk" } }
    }))
    .unwrap();
    to_message("me@penguin.example", &gm, &HashMap::new())
}

#[test]
fn auth_dmarc_pass_from_gmail_mx() {
    assert!(
        with_headers(
            "Acme <news@acme.example>",
            &[("Authentication-Results", GOOGLE_PASS)]
        )
        .sender_authenticated
    );
}

#[test]
fn auth_fail_results() {
    let ar = "mx.google.com; dkim=fail header.i=@acme.example; spf=softfail smtp.mailfrom=x@acme.example; dmarc=fail (p=NONE) header.from=acme.example";
    assert!(
        !with_headers("news@acme.example", &[("Authentication-Results", ar)]).sender_authenticated
    );
    // No Authentication-Results at all (e.g. your own sent mail).
    assert!(!with_headers("news@acme.example", &[]).sender_authenticated);
    // DMARC passed for a different domain than the one in From.
    let other = "mx.google.com; dmarc=pass (p=NONE) header.from=other.example";
    assert!(
        !with_headers("news@acme.example", &[("Authentication-Results", other)])
            .sender_authenticated
    );
}

#[test]
fn auth_ignores_spoofed_results_from_other_authserv() {
    // Gmail's real verdict is on top; the sender injected a rosier one below.
    let real = "mx.google.com; dkim=none; spf=fail smtp.mailfrom=ceo@bank.example; dmarc=fail (p=NONE) header.from=bank.example";
    let forged =
        "mx.google.com; dkim=pass header.d=bank.example; dmarc=pass header.from=bank.example";
    let m = with_headers(
        "CEO <ceo@bank.example>",
        &[
            ("Authentication-Results", real),
            ("Authentication-Results", forged),
        ],
    );
    assert!(!m.sender_authenticated);
    // A pass stamped by some other server never counts.
    let foreign = "mail.attacker.example; dkim=pass header.d=bank.example; dmarc=pass header.from=bank.example";
    assert!(
        !with_headers("ceo@bank.example", &[("Authentication-Results", foreign)])
            .sender_authenticated
    );
    // Nor does a comment pretending to be the authserv-id.
    let sneaky = "(mx.google.com) attacker.example; dmarc=pass header.from=bank.example";
    assert!(
        !with_headers("ceo@bank.example", &[("Authentication-Results", sneaky)])
            .sender_authenticated
    );
}

#[test]
fn auth_dkim_alignment() {
    let ar = |d: &str| {
        format!(
            "mx.google.com; dkim=pass header.d={d} header.s=s1; spf=none; dmarc=none header.from=x"
        )
    };
    // Signing domain equals, or is a parent of, the From domain → aligned.
    assert!(
        with_headers(
            "a@acme.example",
            &[("Authentication-Results", &ar("acme.example"))]
        )
        .sender_authenticated
    );
    assert!(
        with_headers(
            "a@mail.acme.example",
            &[("Authentication-Results", &ar("acme.example"))]
        )
        .sender_authenticated
    );
    // Misaligned: a third party's valid signature proves nothing about From.
    assert!(
        !with_headers(
            "a@acme.example",
            &[("Authentication-Results", &ar("esp-mailer.example"))]
        )
        .sender_authenticated
    );
    assert!(
        !with_headers(
            "a@notacme.example",
            &[("Authentication-Results", &ar("acme.example"))]
        )
        .sender_authenticated
    );
    // A tenant subdomain can't vouch for its parent.
    assert!(
        !with_headers(
            "a@host.example",
            &[("Authentication-Results", &ar("tenant.host.example"))]
        )
        .sender_authenticated
    );
    // header.i form (Gmail's usual), case-insensitive.
    let i_form = "mx.google.com; DKIM=PASS header.i=@Acme.Example header.s=s1";
    assert!(
        with_headers("a@acme.example", &[("Authentication-Results", i_form)]).sender_authenticated
    );
}

#[test]
fn snippet_ampersand_before_multibyte_text_does_not_panic() {
    // The 12-byte entity lookahead used to land inside a multi-byte char.
    assert_eq!(unescape_snippet("Q&A 🎉🎉🎉 party"), "Q&A 🎉🎉🎉 party");
    assert_eq!(unescape_snippet("&日本語のメール"), "&日本語のメール");
    assert_eq!(
        unescape_snippet("R&D — ünïcödé &amp; more"),
        "R&D — ünïcödé & more"
    );
}

/// Hostile-input sweep: random mixes of entity/tag/encoded-word fragments and
/// multi-byte text through every conversion path. Must never panic.
#[test]
fn conversion_never_panics_on_random_input() {
    const PIECES: &[&str] = &[
        "&",
        "&#",
        "&#x",
        "&#1234567890;",
        "&amp",
        "&amp;",
        ";",
        "<",
        ">",
        "</",
        "<!--",
        "-->",
        "<style>",
        "</style>",
        "<head>",
        "<body>",
        "<b",
        "=?",
        "?=",
        "=?utf-8?q?",
        "=?iso-8859-1?b?",
        "=?x?",
        "é",
        "🎉",
        "日本",
        "\u{200b}",
        "\r\n",
        "\n",
        " ",
        "a",
        "Z",
        "\"",
        "'",
        "(",
        ")",
        "\\",
        "@",
        ",",
        ":",
        "<x@y>",
        "\u{0}",
        "\u{fffd}",
        "%",
        "=",
        "dkim=pass",
        "header.d=",
        "mx.google.com;",
        "dmarc=pass",
    ];
    let mut rng = fastrand::Rng::with_seed(0x9e3779b9);
    let random_text = |rng: &mut fastrand::Rng| -> String {
        (0..rng.usize(0..40))
            .map(|_| PIECES[rng.usize(..PIECES.len())])
            .collect()
    };
    for _ in 0..20_000 {
        let text = random_text(&mut rng);
        let html = random_text(&mut rng);
        let header = random_text(&mut rng);
        let mut raw = text.clone().into_bytes();
        if rng.bool() {
            raw.extend((0..rng.usize(0..16)).map(|_| rng.u8(..)));
        }
        let date = ["", "-5", "99999999999999999999", "1758600000000", "x"][rng.usize(..5)];
        let charset = [
            "utf-8",
            "iso-8859-1",
            "shift_jis",
            "iso-2022-jp",
            "bogus",
            "",
        ][rng.usize(..6)];
        let gm: GmailMessage = serde_json::from_value(serde_json::json!({
            "id": "f", "threadId": "t", "snippet": random_text(&mut rng),
            "internalDate": date,
            "payload": {
                "mimeType": "multipart/mixed",
                "headers": [
                    { "name": "From", "value": header },
                    { "name": "To", "value": random_text(&mut rng) },
                    { "name": "Subject", "value": random_text(&mut rng) },
                    { "name": "References", "value": random_text(&mut rng) },
                    { "name": "Authentication-Results", "value": random_text(&mut rng) }
                ],
                "parts": [
                    { "partId": "0", "mimeType": "text/plain",
                      "headers": [{ "name": "Content-Type", "value": format!("text/plain; charset={charset}") }],
                      "body": { "size": raw.len(), "data": B64URL.encode(&raw) } },
                    { "partId": "1", "mimeType": "text/html", "headers": [],
                      "body": { "size": html.len(), "data": B64URL.encode(html.as_bytes()) } },
                    { "partId": "2", "mimeType": "application/octet-stream", "filename": random_text(&mut rng),
                      "headers": [{ "name": "Content-Disposition", "value": random_text(&mut rng) }],
                      "body": { "size": 3, "attachmentId": "A" } }
                ]
            }
        }))
        .unwrap();
        let m = to_message("me@penguin.example", &gm, &HashMap::new());
        std::hint::black_box((
            &m,
            penguin_core::text::html_to_text(&html),
            unescape_snippet(&text),
        ));
    }
}

#[test]
fn metadata_payload_converts_to_headers_only_message() {
    // format=metadata: headers, snippet, labels; no parts or body data.
    let gm: GmailMessage = serde_json::from_value(serde_json::json!({
        "id": "md1", "threadId": "t", "labelIds": ["INBOX"], "snippet": "Hello &amp; welcome",
        "internalDate": "1758600000000",
        "payload": {
            "mimeType": "multipart/mixed",
            "headers": [
                { "name": "From", "value": "Acme <hi@acme.example>" },
                { "name": "Subject", "value": "Welcome" },
                { "name": "List-Unsubscribe", "value": "<mailto:u@acme.example>" }
            ]
        }
    }))
    .unwrap();
    let m = to_message("me@penguin.example", &gm, &HashMap::new());
    assert_eq!(m.subject, "Welcome");
    assert_eq!(m.from.email, "hi@acme.example");
    assert_eq!(m.snippet, "Hello & welcome");
    assert!(m.body_text.is_empty() && m.body_html.is_none() && m.attachments.is_empty());
    assert!(m.list_unsubscribe.is_some());
}

#[test]
fn list_unsubscribe_post_is_kept_at_ingest() {
    let msg = |post: Option<&str>| {
        let mut headers = vec![
            serde_json::json!({ "name": "From", "value": "News <news@weekly.example>" }),
            serde_json::json!({ "name": "List-Unsubscribe", "value": "<https://weekly.example/u/abc>" }),
        ];
        if let Some(p) = post {
            headers.push(serde_json::json!({ "name": "List-Unsubscribe-Post", "value": p }));
        }
        let gm: GmailMessage = serde_json::from_value(serde_json::json!({
            "id": "u1", "threadId": "t", "labelIds": ["INBOX"], "snippet": "",
            "internalDate": "1758600000000",
            "payload": { "mimeType": "text/plain", "headers": headers }
        }))
        .unwrap();
        to_message("me@penguin.example", &gm, &HashMap::new()).list_unsubscribe_post
    };
    assert_eq!(msg(Some("List-Unsubscribe=One-Click")), Some(true));
    assert_eq!(msg(Some(" list-unsubscribe=one-click ")), Some(true));
    assert_eq!(msg(Some("List-Unsubscribe=Maybe")), Some(false));
    assert_eq!(msg(None), Some(false));
}

/// Google Calendar invitations as messages.get(format=full) returns them:
/// multipart/mixed { multipart/alternative { text/plain, text/html,
/// text/calendar; method=REQUEST (no filename) }, application/ics
/// invite.ics }. Both calendar parts are recorded, with bytes that parse.
#[test]
fn google_calendar_invitations_keep_both_calendar_parts() {
    let me = "alex.morgan@gmail.example";
    for (name, ics, calendar_id) in [
        (
            "google_invite",
            include_str!("../tests/fixtures/google_invite.ics"),
            "part:0.2",
        ),
        (
            "google_invite_updated",
            include_str!("../tests/fixtures/google_invite_updated.ics"),
            "part:0.2",
        ),
        // The same message with the calendar part out of line.
        (
            "google_invite_updated_out_of_line",
            include_str!("../tests/fixtures/google_invite_updated.ics"),
            "ANGjdJ9update-calendar-part",
        ),
    ] {
        let gm = fixture(name);
        let m = to_message(me, &gm, &HashMap::new());
        let parts: Vec<_> = m
            .attachments
            .iter()
            .map(|a| {
                (
                    a.id.as_str(),
                    a.filename.as_str(),
                    a.mime_type.as_str(),
                    a.size,
                    a.inline,
                )
            })
            .collect();
        let size = ics.len() as u64;
        let ics_id = if name == "google_invite" {
            "ANGjdJ8first-invite-ics"
        } else {
            "ANGjdJ9update-invite-ics"
        };
        assert_eq!(
            parts,
            vec![
                (calendar_id, "untitled.ics", "text/calendar", size, false),
                (ics_id, "invite.ics", "application/ics", size, false),
            ],
            "{name}"
        );
        assert!(m.body_html.as_deref().unwrap().contains("Maybe"), "{name}");
        if let Some(part) = calendar_id.strip_prefix(PART_ID_PREFIX) {
            assert_eq!(
                inline_part_bytes(&gm, part).unwrap(),
                ics.as_bytes(),
                "{name}"
            );
        }
        let inv = crate::ics::parse_invite(ics, me).expect("parses");
        assert_eq!(inv.method.as_deref(), Some("REQUEST"));
        assert_eq!(inv.summary, "Sam <> Alex / Lunch");
        assert_eq!(inv.location, "Café Olé, 12 Harbor St, Springfield");
        assert_eq!(
            inv.organizer.as_ref().unwrap().email,
            "sam@northwind.example"
        );
        let mine = inv.attendees.iter().find(|a| a.is_self).expect("my row");
        assert_eq!(mine.response, "needsAction");
        assert_eq!(inv.raw.self_address.as_deref(), Some(me));
        assert_eq!(inv.end - inv.start, 3_600_000);
    }
    let first =
        crate::ics::parse_invite(include_str!("../tests/fixtures/google_invite.ics"), me).unwrap();
    let updated = crate::ics::parse_invite(
        include_str!("../tests/fixtures/google_invite_updated.ics"),
        me,
    )
    .unwrap();
    // 2026-09-28 12:00 EDT and 2026-09-29 10:00 EDT, through the VTIMEZONE.
    assert_eq!((first.sequence, first.start), (0, 1_790_611_200_000));
    assert_eq!((updated.sequence, updated.start), (1, 1_790_690_400_000));
    assert_eq!(first.uid, updated.uid);
}
