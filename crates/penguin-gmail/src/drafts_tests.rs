//! Draft flows against a fake Gmail and an in-memory store.

use std::sync::Mutex;

use mail_parser::MessageParser;
use penguin_core::{Address, Message, Store};

use super::*;
use crate::compose::OutgoingAttachment;

const ACCT: &str = "ada@penguin.example";

fn addr(email: &str) -> Address {
    Address {
        name: None,
        email: email.into(),
    }
}

fn me() -> Address {
    Address {
        name: Some("Ada Lovelace".into()),
        email: ACCT.into(),
    }
}

/// Fake Gmail: each create/update mints a new message id, like Gmail does.
#[derive(Default)]
struct FakeGmail {
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    next: u32,
    /// draft id → current message id
    drafts: Vec<(String, String)>,
    /// (call, raw MIME) in order
    calls: Vec<(String, Vec<u8>)>,
    remote_messages: Vec<Message>,
}

impl FakeState {
    fn mint(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    /// Like Gmail: the saved draft message exists remotely, with its
    /// attachments under fresh ids.
    fn remember(&mut self, message_id: &str, thread_id: &str, raw: &[u8]) {
        use mail_parser::MimeHeaders;
        let parsed = MessageParser::default().parse(raw).unwrap();
        let attachments = parsed
            .attachments()
            .enumerate()
            .map(|(i, a)| penguin_core::AttachmentMeta {
                id: format!("{message_id}-att{i}"),
                filename: a.attachment_name().unwrap_or_default().to_string(),
                mime_type: a
                    .content_type()
                    .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or_default()))
                    .unwrap_or_default(),
                size: a.contents().len() as u64,
                // Like Gmail's parse (convert.rs): Content-ID kept, inline
                // when the disposition says so.
                content_id: a.content_id().map(str::to_string),
                inline: a
                    .content_disposition()
                    .is_some_and(|d| d.ctype().eq_ignore_ascii_case("inline")),
            })
            .collect();
        let mut m = received(message_id, thread_id, "draft@penguin.example");
        m.label_ids = vec!["DRAFT".into()];
        m.attachments = attachments;
        m.body_html = parsed.body_html(0).map(|h| h.to_string());
        self.remote_messages.push(m);
    }
}

impl DraftApi for FakeGmail {
    async fn create_draft(&self, raw: &[u8], thread_id: Option<&str>) -> Result<DraftRef> {
        let mut s = self.state.lock().unwrap();
        let draft_id = s.mint("r");
        let message_id = s.mint("m");
        s.drafts.push((draft_id.clone(), message_id.clone()));
        s.calls.push(("create".into(), raw.to_vec()));
        s.remember(&message_id, thread_id.unwrap_or("t-new"), raw);
        Ok(DraftRef {
            draft_id,
            message_id,
            thread_id: thread_id.unwrap_or("t-new").into(),
            attachments: vec![],
        })
    }
    async fn update_draft(
        &self,
        draft_id: &str,
        raw: &[u8],
        thread_id: Option<&str>,
    ) -> Result<DraftRef> {
        let mut s = self.state.lock().unwrap();
        if !s.drafts.iter().any(|(d, _)| d == draft_id) {
            return Err(Error::Http {
                status: 404,
                body: "not found".into(),
            });
        }
        let message_id = s.mint("m");
        for d in s.drafts.iter_mut().filter(|(d, _)| d == draft_id) {
            d.1 = message_id.clone();
        }
        s.calls.push(("update".into(), raw.to_vec()));
        s.remember(&message_id, thread_id.unwrap_or("t-new"), raw);
        Ok(DraftRef {
            draft_id: draft_id.into(),
            message_id,
            thread_id: thread_id.unwrap_or("t-new").into(),
            attachments: vec![],
        })
    }
    async fn delete_draft(&self, draft_id: &str) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.drafts.retain(|(d, _)| d != draft_id);
        s.calls.push(("delete".into(), Vec::new()));
        Ok(())
    }
    async fn send_draft(
        &self,
        draft_id: &str,
        raw: &[u8],
        thread_id: Option<&str>,
    ) -> Result<DraftRef> {
        let mut s = self.state.lock().unwrap();
        s.drafts.retain(|(d, _)| d != draft_id);
        let message_id = s.mint("sent");
        s.calls.push(("send".into(), raw.to_vec()));
        Ok(DraftRef {
            draft_id: draft_id.into(),
            message_id,
            thread_id: thread_id.unwrap_or("t-new").into(),
            attachments: vec![],
        })
    }
    async fn list_drafts(&self) -> Result<Vec<DraftRef>> {
        let s = self.state.lock().unwrap();
        Ok(s.drafts
            .iter()
            .map(|(d, m)| DraftRef {
                draft_id: d.clone(),
                message_id: m.clone(),
                thread_id: "t-web".into(),
                attachments: vec![],
            })
            .collect())
    }
    async fn fetch_message(&self, message_id: &str) -> Result<Option<Message>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .remote_messages
            .iter()
            .find(|m| m.id == message_id)
            .cloned())
    }
}

fn received(id: &str, thread: &str, header_id: &str) -> Message {
    Message {
        account_id: ACCT.into(),
        id: id.into(),
        thread_id: thread.into(),
        date: 1_700_000_000_000,
        from: addr("bo@acme.example"),
        to: vec![me()],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Quarterly plan".into(),
        snippet: "Here is the plan".into(),
        body_text: "Here is the plan".into(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: vec![],
        message_id_header: Some(header_id.into()),
        in_reply_to: None,
        references: vec!["root@acme.example".into()],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

fn reply_draft(body: &str) -> Draft {
    Draft {
        request_read_receipt: None,
        account_id: ACCT.into(),
        to: vec![addr("bo@acme.example")],
        cc: vec![],
        bcc: vec![],
        subject: "Re: Quarterly plan".into(),
        body_text: body.into(),
        body_html: None,
        reply_to_thread_id: Some("t1".into()),
        reply_to_message_id: Some("p1".into()),
        attachments: vec![],
    }
}

fn header(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head = &text[..text.find("\r\n\r\n").unwrap_or(text.len())];
    head.lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with(&format!("{}:", name.to_ascii_lowercase()))
        })
        .map(|l| l[name.len() + 1..].trim().to_string())
}

#[tokio::test]
async fn create_update_send_keeps_store_and_mapping_in_step() {
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_messages(&[received("p1", "t1", "parent@acme.example")])
        .unwrap();
    let gmail = FakeGmail::default();

    // create
    let first = save(
        &gmail,
        &store,
        &reply_draft("Sounds good"),
        &me(),
        None,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(first.thread_id, "t1");
    let local = store
        .get_message(ACCT, &first.message_id)
        .unwrap()
        .expect("draft mirrored locally");
    assert_eq!(local.label_ids, vec!["DRAFT"]);
    assert_eq!(local.body_text, "Sounds good");
    assert_eq!(local.in_reply_to.as_deref(), Some("parent@acme.example"));
    assert_eq!(
        store.message_for_draft(ACCT, &first.draft_id).unwrap(),
        Some(first.message_id.clone())
    );

    // update: same draft id, new message id; the old local copy is gone
    let second = save(
        &gmail,
        &store,
        &reply_draft("Sounds good, see you Tuesday"),
        &me(),
        Some(&first.draft_id),
        &[],
    )
    .await
    .unwrap();
    assert_eq!(second.draft_id, first.draft_id);
    assert_ne!(second.message_id, first.message_id);
    assert!(store
        .get_message(ACCT, &first.message_id)
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .get_message(ACCT, &second.message_id)
            .unwrap()
            .unwrap()
            .body_text,
        "Sounds good, see you Tuesday"
    );
    assert_eq!(
        store.draft_for_message(ACCT, &second.message_id).unwrap(),
        Some(first.draft_id.clone())
    );

    // send: local draft and mapping removed; MIME is a threaded reply
    let sent = send(
        &gmail,
        &store,
        &reply_draft("Sounds good, see you Tuesday"),
        &me(),
        &first.draft_id,
        &[],
    )
    .await
    .unwrap();
    assert!(sent.message_id.starts_with("sent"));
    assert!(store
        .get_message(ACCT, &second.message_id)
        .unwrap()
        .is_none());
    assert_eq!(
        store.message_for_draft(ACCT, &first.draft_id).unwrap(),
        None
    );

    let calls = gmail.state.lock().unwrap().calls.clone();
    let names: Vec<&str> = calls.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["create", "update", "send"]);
    let raw = &calls[2].1;
    assert_eq!(
        header(raw, "In-Reply-To").as_deref(),
        Some("<parent@acme.example>")
    );
    let refs = header(raw, "References").unwrap();
    assert!(
        refs.contains("root@acme.example") && refs.contains("parent@acme.example"),
        "{refs}"
    );
    let parsed = MessageParser::default().parse(raw.as_slice()).unwrap();
    assert_eq!(
        parsed.body_text(0).unwrap().trim(),
        "Sounds good, see you Tuesday"
    );
}

#[tokio::test]
async fn draft_without_recipients_saves() {
    let store = Store::open_in_memory().unwrap();
    let gmail = FakeGmail::default();
    let draft = Draft {
        request_read_receipt: None,
        account_id: ACCT.into(),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        subject: String::new(),
        body_text: "note to self".into(),
        body_html: None,
        reply_to_thread_id: None,
        reply_to_message_id: None,
        attachments: vec![],
    };
    let saved = save(&gmail, &store, &draft, &me(), None, &[])
        .await
        .unwrap();
    let raw = gmail.state.lock().unwrap().calls[0].1.clone();
    assert!(header(&raw, "To").is_none());
    assert!(header(&raw, "In-Reply-To").is_none());
    assert!(store
        .get_message(ACCT, &saved.message_id)
        .unwrap()
        .is_some());
    // Sending it still requires a recipient.
    assert!(send(&gmail, &store, &draft, &me(), &saved.draft_id, &[])
        .await
        .is_err());
}

#[tokio::test]
async fn update_of_vanished_draft_recreates_it() {
    let store = Store::open_in_memory().unwrap();
    let gmail = FakeGmail::default();
    store.set_draft(ACCT, "r-gone", "m-gone").unwrap();
    let saved = save(
        &gmail,
        &store,
        &reply_draft("still here"),
        &me(),
        Some("r-gone"),
        &[],
    )
    .await
    .unwrap();
    assert_ne!(saved.draft_id, "r-gone");
    assert_eq!(store.message_for_draft(ACCT, "r-gone").unwrap(), None);
    assert_eq!(
        store.message_for_draft(ACCT, &saved.draft_id).unwrap(),
        Some(saved.message_id)
    );
}

#[tokio::test]
async fn delete_removes_local_copy() {
    let store = Store::open_in_memory().unwrap();
    let gmail = FakeGmail::default();
    let saved = save(&gmail, &store, &reply_draft("draft"), &me(), None, &[])
        .await
        .unwrap();
    let thread = delete(&gmail, &store, ACCT, &saved.draft_id).await.unwrap();
    assert_eq!(thread.as_deref(), Some("t1"));
    assert!(store
        .get_message(ACCT, &saved.message_id)
        .unwrap()
        .is_none());
    assert!(gmail.state.lock().unwrap().drafts.is_empty());
}

#[tokio::test]
async fn open_by_message_id_resolves_unmapped_web_draft() {
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_messages(&[received("p1", "t1", "parent@acme.example")])
        .unwrap();
    // A draft created in Gmail web: synced as a DRAFT message, no mapping yet.
    let mut web = received("m-web", "t1", "draft@penguin.example");
    web.label_ids = vec!["DRAFT".into()];
    web.from = me();
    web.to = vec![addr("bo@acme.example")];
    web.subject = "Re: Quarterly plan".into();
    web.body_text = "typed on the web".into();
    web.body_html = Some("<p>typed on the web<img src=x onerror=alert(1)></p>".into());
    web.in_reply_to = Some("parent@acme.example".into());
    store.upsert_messages(&[web]).unwrap();
    let gmail = FakeGmail::default();
    gmail
        .state
        .lock()
        .unwrap()
        .drafts
        .push(("r-web".into(), "m-web".into()));

    let opened = open(&gmail, &store, ACCT, None, Some("m-web"))
        .await
        .unwrap()
        .expect("draft found");
    assert_eq!(opened.draft_id, "r-web");
    assert_eq!(opened.draft.to, vec![addr("bo@acme.example")]);
    assert_eq!(opened.draft.body_text, "typed on the web");
    assert_eq!(
        opened.draft.body_html.as_deref(),
        Some("<p>typed on the web</p>"),
        "stored HTML goes back to the composer only sanitized"
    );
    assert_eq!(opened.draft.reply_to_message_id.as_deref(), Some("p1"));
    assert_eq!(opened.draft.reply_to_thread_id.as_deref(), Some("t1"));
    // The refresh persisted the mapping.
    assert_eq!(
        store.draft_for_message(ACCT, "m-web").unwrap().as_deref(),
        Some("r-web")
    );

    // Unknown ids resolve to None rather than an error.
    assert!(open(&gmail, &store, ACCT, Some("r-missing"), None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn open_fetches_draft_not_synced_yet() {
    let store = Store::open_in_memory().unwrap();
    let gmail = FakeGmail::default();
    let mut remote = received("m-remote", "t9", "x@penguin.example");
    remote.label_ids = vec!["DRAFT".into()];
    gmail
        .state
        .lock()
        .unwrap()
        .drafts
        .push(("r9".into(), "m-remote".into()));
    gmail.state.lock().unwrap().remote_messages.push(remote);
    let opened = open(&gmail, &store, ACCT, Some("r9"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opened.message_id, "m-remote");
    assert_eq!(opened.draft.reply_to_thread_id, None);
    assert!(store.get_message(ACCT, "m-remote").unwrap().is_some());
}

fn pdf_attachment() -> (OutgoingAttachment, AttachmentBytes) {
    let bytes = b"%PDF-1.4 fake".to_vec();
    (
        OutgoingAttachment::File {
            filename: "Plan — v2.pdf".into(),
            mime_type: "application/pdf".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            content_id: None,
        },
        AttachmentBytes {
            filename: "Plan — v2.pdf".into(),
            mime_type: "application/pdf".into(),
            bytes,
            content_id: None,
        },
    )
}

#[tokio::test]
async fn attachments_are_saved_repointed_reopened_and_sent() {
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_messages(&[received("p1", "t1", "parent@acme.example")])
        .unwrap();
    let gmail = FakeGmail::default();
    let (file, bytes) = pdf_attachment();
    let mut draft = reply_draft("see attached");
    draft.attachments = vec![file];

    // Save: the MIME carries the file, and the returned refs point at the
    // new draft message (not the base64 the composer sent).
    let saved = save(
        &gmail,
        &store,
        &draft,
        &me(),
        None,
        std::slice::from_ref(&bytes),
    )
    .await
    .unwrap();
    let raw = gmail.state.lock().unwrap().calls[0].1.clone();
    let parsed = MessageParser::default().parse(raw.as_slice()).unwrap();
    assert_eq!(
        parsed.attachments().next().unwrap().contents(),
        bytes.bytes.as_slice()
    );
    let expected_ref = OutgoingAttachment::Gmail {
        message_id: saved.message_id.clone(),
        attachment_id: format!("{}-att0", saved.message_id),
        filename: "Plan — v2.pdf".into(),
        mime_type: "application/pdf".into(),
        size: bytes.bytes.len() as u64,
        account_id: None,
        content_id: None,
    };
    assert_eq!(saved.attachments, vec![expected_ref.clone()]);
    // The local mirror lists the attachment too.
    let local = store.get_message(ACCT, &saved.message_id).unwrap().unwrap();
    assert_eq!(local.attachments.len(), 1);

    // Reopen: attachments come back as refs.
    let opened = open(&gmail, &store, ACCT, Some(&saved.draft_id), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opened.draft.attachments, vec![expected_ref.clone()]);

    // Save again with the ref (bytes resolved by the caller): re-pointed again.
    let mut again = reply_draft("see attached (v2)");
    again.attachments = vec![expected_ref];
    let second = save(
        &gmail,
        &store,
        &again,
        &me(),
        Some(&saved.draft_id),
        std::slice::from_ref(&bytes),
    )
    .await
    .unwrap();
    assert_ne!(second.message_id, saved.message_id);
    assert!(
        matches!(&second.attachments[0], OutgoingAttachment::Gmail { message_id, .. } if *message_id == second.message_id)
    );

    // Send carries the bytes; unresolved attachments are an error, not a drop.
    assert!(send(&gmail, &store, &again, &me(), &saved.draft_id, &[])
        .await
        .is_err());
    send(
        &gmail,
        &store,
        &again,
        &me(),
        &saved.draft_id,
        std::slice::from_ref(&bytes),
    )
    .await
    .unwrap();
    let calls = gmail.state.lock().unwrap().calls.clone();
    let sent_raw = &calls.last().unwrap().1;
    let sent = MessageParser::default().parse(sent_raw.as_slice()).unwrap();
    assert_eq!(
        sent.attachments().next().unwrap().contents(),
        bytes.bytes.as_slice()
    );
}

#[tokio::test]
async fn inline_images_survive_save_and_reopen() {
    let store = Store::open_in_memory().unwrap();
    let gmail = FakeGmail::default();
    let (file, pdf) = pdf_attachment();
    let png = b"\x89PNG\r\n\x1a\nchart".to_vec();
    let image = AttachmentBytes {
        filename: "chart.png".into(),
        mime_type: "image/png".into(),
        bytes: png.clone(),
        content_id: Some("img-7@penguin".into()),
    };
    let mut draft = reply_draft("Chart:\n[image: chart.png]");
    draft.reply_to_message_id = None;
    draft.reply_to_thread_id = None;
    draft.body_html = Some(r#"<p>Chart:</p><p><img src="cid:img-7@penguin" alt="chart.png" width="600" style="max-width:100%;height:auto"></p>"#.into());
    draft.attachments = vec![
        OutgoingAttachment::File {
            filename: "chart.png".into(),
            mime_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&png),
            content_id: Some("img-7@penguin".into()),
        },
        file,
    ];
    let saved = save(&gmail, &store, &draft, &me(), None, &[image, pdf])
        .await
        .unwrap();
    // The draft Gmail stores has the image as a related part.
    let raw = gmail.state.lock().unwrap().calls[0].1.clone();
    let text = String::from_utf8_lossy(&raw);
    assert!(
        text.contains("multipart/related") && text.contains("Content-ID: <img-7@penguin>"),
        "{text}"
    );
    // Refs come back in the composer's order: the image by its id, then the file.
    let ids: Vec<(String, Option<String>)> = saved
        .attachments
        .iter()
        .map(|a| match a {
            OutgoingAttachment::Gmail {
                filename,
                content_id,
                ..
            } => (filename.clone(), content_id.clone()),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            ("chart.png".to_string(), Some("img-7@penguin".to_string())),
            ("Plan — v2.pdf".to_string(), None),
        ]
    );
    // Reopened: the HTML still shows it and its part is a ref with the id.
    let opened = open(&gmail, &store, ACCT, Some(&saved.draft_id), None)
        .await
        .unwrap()
        .unwrap();
    let html = opened.draft.body_html.unwrap();
    assert!(
        html.contains(r#"<img src="cid:img-7@penguin" alt="chart.png" width="600">"#),
        "{html}"
    );
    assert_eq!(
        opened
            .draft
            .attachments
            .iter()
            .filter_map(|a| a.content_id())
            .collect::<Vec<_>>(),
        vec!["img-7@penguin"]
    );
    assert_eq!(opened.draft.attachments.len(), 2);
}
