use super::*;

fn untagged(s: &[u8]) -> Untagged {
    match parse_response(s).unwrap() {
        Response::Untagged(u) => u,
        other => panic!("not untagged: {other:?}"),
    }
}

#[test]
fn fetch_with_literals_sections_and_extensions() {
    let raw = b"* 12 FETCH (UID 4827 FLAGS (\\Seen $Forwarded) INTERNALDATE \" 7-Feb-2026 09:10:11 +0100\" RFC822.SIZE 311 MODSEQ (91) EMAILID (M6d99ac32) X-GM-MSGID 1278455344230334865 X-GM-THRID 1278455344230334865 X-GM-LABELS (\\Inbox \"Two words\" &AOk-t&AOk-) BODY[HEADER.FIELDS (MESSAGE-ID DATE)] {21}\r\nMessage-ID: <a@b>\r\n\r\n)\r\n";
    let Untagged::Fetch { seq, items } = untagged(raw) else {
        panic!()
    };
    assert_eq!(seq, 12);
    let get = |name: &str| {
        items
            .chunks(2)
            .find(|p| p[0].is_atom(name))
            .map(|p| p[1].clone())
            .unwrap()
    };
    assert_eq!(get("UID").as_u64(), Some(4827));
    assert_eq!(get("FLAGS").as_list().unwrap().len(), 2);
    assert_eq!(get("RFC822.SIZE").as_u64(), Some(311));
    assert_eq!(get("MODSEQ").as_list().unwrap()[0].as_u64(), Some(91));
    assert_eq!(
        get("EMAILID").as_list().unwrap()[0].as_atom(),
        Some("M6d99ac32")
    );
    assert_eq!(get("X-GM-MSGID").as_u64(), Some(1278455344230334865));
    let labels = get("X-GM-LABELS");
    let labels = labels.as_list().unwrap();
    assert_eq!(labels[0].as_atom(), Some("\\Inbox"));
    assert_eq!(labels[1].as_text().as_deref(), Some("Two words"));
    assert_eq!(utf7::decode(labels[2].as_bytes().unwrap()), "été");
    assert_eq!(
        get("BODY[HEADER.FIELDS (MESSAGE-ID DATE)]").as_bytes(),
        Some(&b"Message-ID: <a@b>\r\n\r\n"[..])
    );
}

#[test]
fn status_codes_and_text() {
    match parse_response(b"* OK [UIDVALIDITY 3857529045] UIDs valid\r\n").unwrap() {
        Response::Untagged(Untagged::Status(s)) => {
            assert_eq!(s.kind, StatusKind::Ok);
            assert_eq!(s.code_num("UIDVALIDITY"), Some(3857529045));
            assert_eq!(s.text, "UIDs valid");
        }
        other => panic!("{other:?}"),
    }
    match parse_response(b"a7 NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n").unwrap()
    {
        Response::Tagged { tag, status } => {
            assert_eq!(tag, "a7");
            assert_eq!(status.kind, StatusKind::No);
            assert!(status.code_is("AUTHENTICATIONFAILED"));
            assert_eq!(status.text, "Invalid credentials (Failure)");
        }
        other => panic!("{other:?}"),
    }
    match parse_response(b"a8 OK [COPYUID 38505 304,319:320 3956:3958] Done\r\n").unwrap() {
        Response::Tagged { status, .. } => {
            let (code, args) = status.code.unwrap();
            assert_eq!(code, "COPYUID");
            assert_eq!(args[1].as_atom(), Some("304,319:320"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        parse_response(b"+ idling\r\n").unwrap(),
        Response::Continue("idling".into())
    );
    // A bracket that isn't a code stays text.
    match parse_response(b"* BYE [UNAVAILABLE] try later\r\n").unwrap() {
        Response::Untagged(Untagged::Status(s)) => {
            assert_eq!(s.kind, StatusKind::Bye);
            assert!(s.code_is("UNAVAILABLE"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn list_search_status_vanished() {
    assert_eq!(
        untagged(b"* LIST (\\HasNoChildren \\Sent) \"/\" \"[Gmail]/Sent Mail\"\r\n"),
        Untagged::List {
            attrs: vec!["\\HasNoChildren".into(), "\\Sent".into()],
            delimiter: Some("/".into()),
            name: b"[Gmail]/Sent Mail".to_vec(),
        }
    );
    assert_eq!(
        untagged(b"* LIST (\\Noselect) NIL {5}\r\nfolde\r\n"),
        Untagged::List {
            attrs: vec!["\\Noselect".into()],
            delimiter: None,
            name: b"folde".to_vec(),
        }
    );
    assert_eq!(
        untagged(b"* SEARCH 2 84 882 (MODSEQ 917162500)\r\n"),
        Untagged::Search(vec![2, 84, 882])
    );
    assert_eq!(untagged(b"* SEARCH\r\n"), Untagged::Search(vec![]));
    match untagged(b"* STATUS INBOX (MESSAGES 231 UIDNEXT 44292)\r\n") {
        Untagged::MailboxStatus { name, items } => {
            assert_eq!(name, b"INBOX");
            assert_eq!(items[3].as_u64(), Some(44292));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        untagged(b"* VANISHED (EARLIER) 300:310,405\r\n"),
        Untagged::Vanished {
            earlier: true,
            uids: "300:310,405".into()
        }
    );
    assert_eq!(untagged(b"* 3 EXPUNGE\r\n"), Untagged::Expunge(3));
    assert_eq!(untagged(b"* 18 EXISTS\r\n"), Untagged::Exists(18));
    assert_eq!(
        untagged(b"* CAPABILITY IMAP4rev1 IDLE X-GM-EXT-1 AUTH=PLAIN\r\n"),
        Untagged::Capability(vec![
            "IMAP4rev1".into(),
            "IDLE".into(),
            "X-GM-EXT-1".into(),
            "AUTH=PLAIN".into()
        ])
    );
}

#[test]
fn nested_lists_nil_and_escapes() {
    let mut t = Tokens::new(b"((\"a\\\"b\" NIL) () \"x\\\\y\")");
    let v = t.value().unwrap();
    let l = v.as_list().unwrap();
    assert_eq!(l[0].as_list().unwrap()[0].as_bytes(), Some(&b"a\"b"[..]));
    assert!(l[0].as_list().unwrap()[1].is_nil());
    assert_eq!(l[1].as_list().unwrap().len(), 0);
    assert_eq!(l[2].as_bytes(), Some(&b"x\\y"[..]));
    assert!(parse_response(b"* 1 FETCH (UID 1\r\n").is_err());
    assert!(Tokens::new(b"{5}\r\nab").value().is_err());
}

#[test]
fn literal_markers() {
    assert_eq!(trailing_literal(b"* 1 FETCH (BODY[] {42}"), Some(42));
    assert_eq!(trailing_literal(b"a APPEND x {42+}"), Some(42));
    assert_eq!(trailing_literal(b"* OK done"), None);
    assert_eq!(trailing_literal(b"* OK {x}"), None);
}

#[tokio::test]
async fn reads_whole_responses_with_literals() {
    let data: &[u8] = b"* 1 FETCH (BODY[] {5}\r\nhello UID 7)\r\n* 2 EXISTS\r\nA1 OK done\r\n";
    let mut r = tokio::io::BufReader::new(data);
    let first = read_response(&mut r).await.unwrap();
    assert_eq!(first, b"* 1 FETCH (BODY[] {5}\r\nhello UID 7)\r\n");
    assert_eq!(read_response(&mut r).await.unwrap(), b"* 2 EXISTS\r\n");
    assert_eq!(read_response(&mut r).await.unwrap(), b"A1 OK done\r\n");
    assert!(read_response(&mut r).await.is_err());
}

#[test]
fn quoting() {
    assert!(quotable(b"INBOX"));
    assert!(!quotable("Entwürfe".as_bytes()));
    assert!(!quotable(b"a\r\nb"));
    assert_eq!(quote(br#"a"b\c"#), br#""a\"b\\c""#.to_vec());
}
