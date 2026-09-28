use penguin_core::AccountProvider;
use penguin_provider::conformance::check_message;

use super::*;
use crate::proto::Tokens;

const A: &str = "sam@mail.example";

fn meta(labels: &[&str]) -> Meta {
    Meta {
        account_id: A.into(),
        id: "h:0123456789ab0123456789ab".into(),
        thread_id: "t:0123456789ab0123456789ab".into(),
        date_ms: Some(1_760_000_000_000),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        trusted_authserv: Some("mx.google.com".into()),
    }
}

const MIXED: &[u8] = b"From: \"Bea Lark\" <bea@lark.example>\r\n\
To: Sam <sam@mail.example>, cy@quill.example\r\n\
Cc: =?UTF-8?Q?D=C3=A9_Rivera?= <de@rivera.example>\r\n\
Subject: =?UTF-8?B?UXVhcnRhbCByZXBvcnQg4oCU?=\r\n\
Message-ID: <q1@lark.example>\r\n\
In-Reply-To: <p0@lark.example>\r\n\
References: <r0@lark.example>\r\n <p0@lark.example>\r\n\
Date: Mon, 6 Oct 2025 10:00:00 +0000\r\n\
List-Unsubscribe: <https://lark.example/u/1>\r\n\
List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\
Authentication-Results: mx.google.com; dkim=pass header.i=@lark.example; dmarc=pass header.from=lark.example\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: multipart/related; boundary=\"rel\"\r\n\
\r\n\
--rel\r\n\
Content-Type: multipart/alternative; boundary=\"alt\"\r\n\
\r\n\
--alt\r\n\
Content-Type: text/plain; charset=iso-8859-1\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Caf=E9   numbers\r\n\
inside\r\n\
--alt\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Caf\xc3\xa9 <img src=\"cid:logo@lark\"></p>\r\n\
--alt--\r\n\
--rel\r\n\
Content-Type: image/png\r\n\
Content-ID: <logo@lark>\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBORw0KGgo=\r\n\
--rel--\r\n\
--outer\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename*=UTF-8''r%C3%A9port.pdf\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--outer\r\n\
Content-Type: message/rfc822\r\n\
\r\n\
From: x@y.example\r\n\
Subject: inner\r\n\
\r\n\
inner body\r\n\
--outer--\r\n";

#[test]
fn a_rich_message_converts_with_imap_part_paths() {
    let m = to_message(MIXED, &meta(&["INBOX", "UNREAD"]));
    assert!(
        check_message(AccountProvider::Imap, A, &m).is_empty(),
        "{:?}",
        check_message(AccountProvider::Imap, A, &m)
    );
    assert_eq!(m.from.email, "bea@lark.example");
    assert_eq!(m.from.name.as_deref(), Some("Bea Lark"));
    assert_eq!(m.to.len(), 2);
    assert_eq!(m.cc[0].name.as_deref(), Some("Dé Rivera"));
    assert_eq!(m.subject, "Quartal report —");
    assert_eq!(m.message_id_header.as_deref(), Some("q1@lark.example"));
    assert_eq!(m.in_reply_to.as_deref(), Some("p0@lark.example"));
    assert_eq!(m.references, vec!["r0@lark.example", "p0@lark.example"]);
    assert!(m.body_text.starts_with("Café"), "{}", m.body_text);
    assert_eq!(m.snippet, "Café numbers inside");
    assert!(m.body_html.as_deref().unwrap().contains("cid:logo@lark"));
    assert_eq!(
        m.list_unsubscribe.as_deref(),
        Some("<https://lark.example/u/1>")
    );
    assert_eq!(m.list_unsubscribe_post, Some(true));
    assert!(m.sender_authenticated);
    assert_eq!(m.date, 1_760_000_000_000);
    let ids: Vec<(&str, &str, bool)> = m
        .attachments
        .iter()
        .map(|a| (a.id.as_str(), a.filename.as_str(), a.inline))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("1.2", "untitled.png", true),
            ("2", "réport.pdf", false),
            ("3", "untitled.eml", false)
        ]
    );
    assert_eq!(m.attachments[0].content_id.as_deref(), Some("logo@lark"));
    // Part bytes come back decoded, by the same paths.
    assert_eq!(part_bytes(MIXED, "2").unwrap(), b"%PDF-1.4\n");
    assert!(String::from_utf8(part_bytes(MIXED, "3").unwrap())
        .unwrap()
        .contains("inner body"));
    assert_eq!(
        part_bytes(MIXED, "1.2").unwrap()[..4],
        [0x89, b'P', b'N', b'G']
    );
    assert!(part_bytes(MIXED, "9").is_none());
}

#[test]
fn authentication_is_only_trusted_from_the_receiving_server() {
    let mut m = meta(&[]);
    m.trusted_authserv = None;
    assert!(!to_message(MIXED, &m).sender_authenticated);
    let forged = String::from_utf8_lossy(MIXED).replace(
        "Authentication-Results: mx.google.com",
        "Authentication-Results: mx.evil.example",
    );
    assert!(!to_message(forged.as_bytes(), &meta(&[])).sender_authenticated);
}

#[test]
fn headers_only_and_envelopes() {
    let end = MIXED.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let m = headers_to_message(&MIXED[..end], &meta(&["INBOX"]));
    assert!(m.body_text.is_empty() && m.attachments.is_empty());
    assert_eq!(m.subject, "Quartal report —");
    assert!(check_message(AccountProvider::Imap, A, &m).is_empty());
    let env = envelope(&MIXED[..end]);
    assert_eq!(env.message_id.as_deref(), Some("q1@lark.example"));
    assert_eq!(
        env.date_raw.as_deref(),
        Some("Mon, 6 Oct 2025 10:00:00 +0000")
    );
    assert_eq!(env.references, vec!["r0@lark.example", "p0@lark.example"]);
    assert_eq!(
        header_value(MIXED, "subject").unwrap(),
        "=?UTF-8?B?UXVhcnRhbCByZXBvcnQg4oCU?="
    );
    assert_eq!(msgid_list("<a@b> junk <c@d>"), vec!["a@b", "c@d"]);
    assert_eq!(msgid_list("bare@id.example"), vec!["bare@id.example"]);
}

#[test]
fn bodystructure_paths_match_the_parser() {
    // What a server returns for MIXED (abridged to the parts that matter).
    let bs = b"((((\"text\" \"plain\" (\"charset\" \"iso-8859-1\") NIL NIL \"quoted-printable\" 24 2 NIL NIL NIL)(\"text\" \"html\" (\"charset\" \"utf-8\") NIL NIL \"7bit\" 40 1 NIL NIL NIL) \"alternative\")(\"image\" \"png\" NIL \"<logo@lark>\" NIL \"base64\" 12 NIL NIL NIL) \"related\")(\"application\" \"pdf\" (\"name\" \"report.pdf\") NIL NIL \"base64\" 12 NIL (\"attachment\" (\"filename*\" \"UTF-8''r%C3%A9port.pdf\")) NIL)(\"message\" \"rfc822\" NIL NIL NIL \"7bit\" 50 (NIL NIL NIL NIL NIL NIL NIL NIL NIL NIL) (\"text\" \"plain\" NIL NIL NIL \"7bit\" 10 1) 3 NIL NIL NIL) \"mixed\" (\"boundary\" \"outer\") NIL NIL)";
    let v = Tokens::new(bs).value().unwrap();
    let root = parse_bodystructure(&v).unwrap();
    assert_eq!(root.mime, "multipart/mixed");
    let texts = root.text_parts();
    assert_eq!(texts[0].0, "1.1.1");
    assert_eq!(texts[0].2.as_deref(), Some("iso-8859-1"));
    assert_eq!(texts[0].3, "quoted-printable");
    assert!(texts[1].1, "html");
    assert_eq!(root.text_sizes(), vec![24, 40]);
    let atts: Vec<(String, String, bool)> = root
        .attachments()
        .into_iter()
        .map(|a| (a.id, a.filename, a.inline))
        .collect();
    assert_eq!(
        atts,
        vec![
            ("1.2".to_string(), "untitled.png".to_string(), true),
            ("2".to_string(), "réport.pdf".to_string(), false),
            ("3".to_string(), "untitled.eml".to_string(), false),
        ]
    );
    // The same ids as parsing the whole message.
    let whole: Vec<String> = to_message(MIXED, &meta(&[]))
        .attachments
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(whole, vec!["1.2", "2", "3"]);
    // A single-part message is part 1.
    let single = Tokens::new(b"(\"text\" \"plain\" NIL NIL NIL \"7bit\" 5 1)")
        .value()
        .unwrap();
    assert_eq!(parse_bodystructure(&single).unwrap().text_parts()[0].0, "1");
}

#[test]
fn transfer_encodings_and_charsets() {
    assert_eq!(decode_transfer(b"aGVs\r\nbG8=", "base64"), b"hello");
    // A partial fetch cut mid-quantum still decodes what's whole.
    assert_eq!(decode_transfer(b"aGVsbG8gd29y", "BASE64"), b"hello wor");
    assert_eq!(decode_transfer(b"a=3Db=\r\nc", "quoted-printable"), b"a=bc");
    assert_eq!(decode_transfer(b"=ZZ", "quoted-printable"), b"=ZZ");
    assert_eq!(decode_text(b"Caf\xe9", Some("iso-8859-1")), "Café");
    assert_eq!(
        decode_text("Café".as_bytes(), Some("x-unknown-charset")),
        "Café"
    );
    assert_eq!(decode_rfc2231("utf-8'en'na%C3%AFve.txt"), "naïve.txt");
}
