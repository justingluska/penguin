//! Every way mail leaves, run against the fake provider and checked by
//! parsing the MIME it was handed: forwarded attachments (names, types,
//! bytes), the quoted original's formatting, the text alternative,
//! threading headers, and inline images (pasted and quoted) as
//! multipart/related parts whose Content-IDs the HTML references.

use mail_parser::{MessageParser, MimeHeaders};
use penguin_core::{AccountProvider, Address, AttachmentMeta, Message, Store};
use penguin_provider::compose::{Draft, OutgoingAttachment};
use penguin_provider::fake::FakeProvider;
use penguin_provider::{quote, MailProvider};

use super::*;

const ACCT: &str = "me@penguin.example";
const PDF: &[u8] = b"%PDF-1.4 scope of work v3 \x00\x01\x02\xff";
const XLSX: &[u8] = b"PK\x03\x04 timeline workbook \xfe\xfd";

fn addr(name: Option<&str>, email: &str) -> Address {
    Address {
        name: name.map(str::to_string),
        email: email.to_string(),
    }
}

fn meta(id: &str, filename: &str, mime: &str, bytes: &[u8]) -> AttachmentMeta {
    AttachmentMeta {
        id: id.into(),
        filename: filename.into(),
        mime_type: mime.into(),
        size: bytes.len() as u64,
        content_id: None,
        inline: false,
    }
}

/// The original with two files and a formatted HTML body.
fn original() -> Message {
    Message {
        account_id: ACCT.into(),
        id: "orig".into(),
        thread_id: "t1".into(),
        date: 1_758_000_000_000,
        from: addr(Some("Dana Reyes"), "dana@acme.example"),
        to: vec![addr(None, ACCT)],
        cc: vec![addr(Some("Ops"), "ops@acme.example")],
        bcc: vec![],
        reply_to: vec![],
        subject: "Scope of work v3 (re-timed) + report".into(),
        snippet: "Plan".into(),
        body_text: "Plan\n\nTwo changes: re-timed milestones, report attached.".into(),
        body_html: Some(
            r#"<html><head><style>p{color:red}</style></head><body><h2>Plan</h2><p><b>Two</b> changes:</p><ul><li><i>re-timed</i> milestones</li><li>report <a href="https://docs.acme.example/r">attached</a></li></ul><img src="https://pixel.tracker.example/o.gif"><script>alert(1)</script></body></html>"#.into(),
        ),
        label_ids: vec!["INBOX".into()],
        attachments: vec![
            meta("a1", "Scope of work v3.pdf", "application/pdf", PDF),
            meta(
                "a2",
                "Zeitplan – Überblick.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                XLSX,
            ),
            AttachmentMeta {
                id: "logo".into(),
                filename: "logo.png".into(),
                mime_type: "image/png".into(),
                size: 3,
                content_id: Some("logo@acme.example".into()),
                inline: true,
            },
        ],
        message_id_header: Some("<orig-1@acme.example>".into()),
        in_reply_to: Some("<root@acme.example>".into()),
        references: vec!["<root@acme.example>".into()],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

struct Env {
    store: Store,
    paths: Paths,
    fake: FakeProvider,
    root: std::path::PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn env(tag: &str) -> Env {
    let store = Store::open_in_memory().unwrap();
    store.upsert_messages(&[original()]).unwrap();
    let root = std::env::temp_dir().join(format!(
        "penguin-outgoing-{tag}-{}-{}",
        std::process::id(),
        fastrand_id()
    ));
    let paths = Paths {
        data_dir: root.clone(),
        config_dir: root.clone(),
        cache_dir: root.clone(),
    };
    let fake = FakeProvider::new(ACCT, AccountProvider::Gmail, store.clone());
    fake.seed(original());
    fake.seed_attachment("orig", "a1", PDF);
    fake.seed_attachment("orig", "a2", XLSX);
    Env {
        store,
        paths,
        fake,
        root,
    }
}

fn fastrand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn none() -> OtherAccounts {
    OtherAccounts::new()
}

fn me() -> Address {
    addr(Some("Pat Penguin"), ACCT)
}

fn original_view(m: &Message) -> quote::Original<'_> {
    quote::Original {
        from: &m.from,
        to: &m.to,
        cc: &m.cc,
        subject: &m.subject,
        date: "Tue, Sep 16, 2025 at 5:20 AM",
        html: m.body_html.as_deref(),
        text: &m.body_text,
        cids: None,
    }
}

/// What the composer hands the backend for a forward of `orig` (the same
/// markup `features/compose/quote.ts` builds).
fn forward_draft() -> Draft {
    let m = original();
    let o = original_view(&m);
    Draft {
        request_read_receipt: None,
        account_id: ACCT.into(),
        to: vec![addr(Some("Lee Park"), "lee@partner.example")],
        cc: vec![],
        bcc: vec![],
        subject: "Fwd: Scope of work v3 (re-timed) + report".into(),
        body_text: format!("FYI, see the files.\n\n{}", quote::forward_text(&o)),
        body_html: Some(format!(
            "<div dir=\"auto\"><p>FYI, see the <b>files</b>.</p><br>{}</div>",
            quote::forward_html(&o)
        )),
        reply_to_thread_id: None,
        reply_to_message_id: None,
        attachments: m
            .attachments
            .iter()
            .filter(|a| !a.inline)
            .map(|a| OutgoingAttachment::Gmail {
                message_id: m.id.clone(),
                attachment_id: a.id.clone(),
                filename: a.filename.clone(),
                mime_type: a.mime_type.clone(),
                size: a.size,
                account_id: None,
                content_id: None,
            })
            .collect(),
    }
}

/// (filename, content type, bytes) of each attachment in `raw`.
fn attachments_of(raw: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    let msg = MessageParser::default().parse(raw).expect("parses");
    msg.attachments()
        .map(|p| {
            let ct = p.content_type().expect("content type");
            let ty = format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or(""));
            (
                p.attachment_name().unwrap_or("").to_string(),
                ty,
                p.contents().to_vec(),
            )
        })
        .collect()
}

fn html_and_text(raw: &[u8]) -> (String, String) {
    let msg = MessageParser::default().parse(raw).expect("parses");
    (
        msg.body_html(0).expect("an HTML part").to_string(),
        // (quoted-printable text comes back with the wire's CRLFs)
        msg.body_text(0).expect("a text part").replace("\r\n", "\n"),
    )
}

fn assert_forward(raw: &[u8]) {
    assert_eq!(
        attachments_of(raw),
        vec![
            (
                "Scope of work v3.pdf".to_string(),
                "application/pdf".to_string(),
                PDF.to_vec()
            ),
            (
                "Zeitplan – Überblick.xlsx".to_string(),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
                XLSX.to_vec()
            ),
        ],
        "both files, exact names, types and bytes; the inline logo is not a file"
    );
    let (html, text) = html_and_text(raw);
    for keep in [
        "<p>FYI, see the <b>files</b>.</p>",
        r#"<div class="gmail_quote"><div dir="ltr" class="gmail_attr">---------- Forwarded message ---------<br>From: <strong>Dana Reyes</strong> &lt;dana@acme.example&gt;<br>"#,
        "<h2>Plan</h2><p><b>Two</b> changes:</p><ul><li><i>re-timed</i> milestones</li>",
        r#"<a href="https://docs.acme.example/r">attached</a>"#,
    ] {
        assert!(html.contains(keep), "HTML part lacks {keep}:\n{html}");
    }
    for gone in ["<script", "alert", "pixel.tracker", "<img", "color:red"] {
        assert!(!html.contains(gone), "{gone} in the HTML part:\n{html}");
    }
    assert!(
        text.starts_with("FYI, see the files.\n\n---------- Forwarded message ---------\nFrom: Dana Reyes <dana@acme.example>\n"),
        "{text}"
    );
    assert!(text.contains("Cc: Ops <ops@acme.example>"), "{text}");
    assert!(text.trim_end().ends_with("report attached."), "{text}");
}

#[tokio::test]
async fn forward_sent_directly_carries_the_originals_files_and_formatting() {
    let e = env("direct");
    let sent = send(
        &e.store,
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &forward_draft(),
        None,
    )
    .await
    .unwrap();
    assert!(sent.message_id.starts_with("sent-"));
    let out = e.fake.outgoing();
    assert_eq!(out.len(), 1);
    assert_forward(&out[0]);
    let msg = MessageParser::default().parse(&out[0][..]).unwrap();
    assert!(
        msg.in_reply_to().is_empty(),
        "a forward starts a new thread"
    );
}

#[tokio::test]
async fn forward_through_its_saved_draft_undo_send_path() {
    // The composer autosaves, then (after the undo countdown) sends through
    // the draft with the refs the save returned.
    let e = env("draft");
    let draft = forward_draft();
    let saved = save_draft(&e.paths, &e.fake, &none(), &me(), &draft, None)
        .await
        .unwrap();
    assert_eq!(saved.attachments.len(), 2, "refs re-pointed at the draft");
    let fetched_before = e
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("get_attachment"))
        .count();
    let repointed = Draft {
        attachments: saved.attachments.clone(),
        ..draft
    };
    send(
        &e.store,
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &repointed,
        Some(&saved.draft_id),
    )
    .await
    .unwrap();
    let fetched_after = e
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("get_attachment"))
        .count();
    assert_eq!(
        fetched_before, fetched_after,
        "the send reuses the cached bytes"
    );
    let out = e.fake.outgoing();
    assert_eq!(out.len(), 1);
    assert_forward(&out[0]);
}

#[tokio::test]
async fn forward_scheduled_for_later_sends_the_saved_draft_with_its_files() {
    let e = env("later");
    let saved = save_draft(&e.paths, &e.fake, &none(), &me(), &forward_draft(), None)
        .await
        .unwrap();
    // What the outbox does when the time comes.
    e.fake.send_saved_draft(&saved.draft_id).await.unwrap();
    let out = e.fake.outgoing();
    assert_eq!(out.len(), 1);
    assert_forward(&out[0]);
}

/// Settings → Privacy "Ask for read receipts": the request goes out on a
/// direct send, a send through the draft, and a scheduled send of the
/// saved draft; with the setting off, or a caller that decided, it doesn't.
#[tokio::test]
async fn read_receipt_request_follows_the_setting_on_every_send_path() {
    let asks = |raw: &[u8]| String::from_utf8_lossy(raw).contains("Disposition-Notification-To: ");
    let on = crate::commands::with_receipt_choice(forward_draft(), true);
    let e = env("receipt-direct");
    send(&e.store, &e.paths, &e.fake, &none(), &me(), &on, None).await.unwrap();
    assert!(asks(&e.fake.outgoing()[0]));

    let e = env("receipt-draft");
    let saved = save_draft(&e.paths, &e.fake, &none(), &me(), &on, None).await.unwrap();
    let repointed = Draft {
        attachments: saved.attachments.clone(),
        ..on.clone()
    };
    send(&e.store, &e.paths, &e.fake, &none(), &me(), &repointed, Some(&saved.draft_id)).await.unwrap();
    assert!(asks(&e.fake.outgoing()[0]));

    let e = env("receipt-later");
    let saved = save_draft(&e.paths, &e.fake, &none(), &me(), &on, None).await.unwrap();
    e.fake.send_saved_draft(&saved.draft_id).await.unwrap();
    assert!(asks(&e.fake.outgoing()[0]), "the saved draft carries it");

    let e = env("receipt-off");
    let off = crate::commands::with_receipt_choice(forward_draft(), false);
    send(&e.store, &e.paths, &e.fake, &none(), &me(), &off, None).await.unwrap();
    assert!(!asks(&e.fake.outgoing()[0]));
    // A caller that already decided (a future per-message toggle) wins.
    let mut decided = forward_draft();
    decided.request_read_receipt = Some(false);
    assert_eq!(crate::commands::with_receipt_choice(decided, true).request_read_receipt, Some(false));
}

#[tokio::test]
async fn a_saved_forward_reopens_with_its_files_and_quote() {
    let e = env("reopen");
    let first = save_draft(&e.paths, &e.fake, &none(), &me(), &forward_draft(), None)
        .await
        .unwrap();
    // A second autosave with the re-pointed refs keeps them.
    let second = save_draft(
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &Draft {
            attachments: first.attachments.clone(),
            ..forward_draft()
        },
        Some(&first.draft_id),
    )
    .await
    .unwrap();
    assert_eq!(second.draft_id, first.draft_id);
    let opened = e
        .fake
        .open_draft(Some(&first.draft_id), None)
        .await
        .unwrap()
        .expect("the draft");
    let names: Vec<&str> = opened
        .draft
        .attachments
        .iter()
        .map(|a| a.filename())
        .collect();
    assert_eq!(names, ["Scope of work v3.pdf", "Zeitplan – Überblick.xlsx"]);
    let html = opened.draft.body_html.clone().expect("the HTML part");
    assert!(html.contains(r#"<div class="gmail_quote">"#), "{html}");
    assert!(
        html.contains("<h2>Plan</h2>") && html.contains("<i>re-timed</i>"),
        "{html}"
    );
    assert!(opened
        .draft
        .body_text
        .contains("---------- Forwarded message ---------"));
    // And sending the reopened draft still carries both files.
    send(
        &e.store,
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &opened.draft,
        Some(&first.draft_id),
    )
    .await
    .unwrap();
    assert_eq!(attachments_of(&e.fake.outgoing()[0]).len(), 2);
}

#[tokio::test]
async fn a_file_that_cannot_be_fetched_fails_the_send_by_name() {
    let e = env("missing");
    e.fake.server.lock().unwrap().attachments.clear();
    let err = send(
        &e.store,
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &forward_draft(),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        err.message
            .contains("Couldn't attach “Scope of work v3.pdf”"),
        "{}",
        err.message
    );
    assert!(
        e.fake.outgoing().is_empty(),
        "nothing left without its files"
    );
    let err = save_draft(&e.paths, &e.fake, &none(), &me(), &forward_draft(), None)
        .await
        .unwrap_err();
    assert!(err.message.contains("Couldn't attach"), "{}", err.message);
}

#[tokio::test]
async fn forward_sent_from_another_account_fetches_files_from_the_original_account() {
    let e = env("cross");
    const ALT: &str = "alt@penguin.example";
    let alt = FakeProvider::new(ALT, AccountProvider::Gmail, e.store.clone());
    let mut d = forward_draft();
    d.account_id = ALT.into();
    for a in &mut d.attachments {
        if let OutgoingAttachment::Gmail { account_id, .. } = a {
            *account_id = Some(ACCT.into());
        }
    }
    let from = addr(None, ALT);
    // The original's account isn't connected: a clear error, nothing sent.
    let err = send(&e.store, &e.paths, &alt, &none(), &from, &d, None)
        .await
        .unwrap_err();
    assert!(err.message.contains("isn't connected"), "{}", err.message);
    let mut others = none();
    others.insert(ACCT.into(), std::sync::Arc::new(e.fake.clone()));
    send(&e.store, &e.paths, &alt, &others, &from, &d, None)
        .await
        .unwrap();
    assert!(e.fake.outgoing().is_empty());
    assert_forward(&alt.outgoing()[0]);
}

fn reply_draft() -> Draft {
    let m = original();
    let o = original_view(&m);
    let attribution = "On Tue, Sep 16, 2025 at 5:20 AM, Dana Reyes <dana@acme.example> wrote:";
    Draft {
        request_read_receipt: None,
        account_id: ACCT.into(),
        to: vec![m.from.clone()],
        cc: vec![],
        bcc: vec![],
        subject: "Re: Scope of work v3 (re-timed) + report".into(),
        body_text: format!(
            "Looks good.\n\n{attribution}\n> Plan\n>\n> Two changes: re-timed milestones, report attached."
        ),
        body_html: Some(format!(
            "<div dir=\"auto\"><p>Looks <i>good</i>.</p><br>{}</div>",
            quote::reply_html(attribution, &o)
        )),
        reply_to_thread_id: Some("t1".into()),
        reply_to_message_id: Some("orig".into()),
        attachments: vec![OutgoingAttachment::File {
            filename: "notes.txt".into(),
            mime_type: "text/plain".into(),
            data_base64: "bm90ZXM=".into(),
            content_id: None,
        }],
    }
}

fn assert_reply(raw: &[u8]) {
    let msg = MessageParser::default().parse(raw).unwrap();
    assert_eq!(msg.in_reply_to().as_text(), Some("orig-1@acme.example"));
    let refs: Vec<&str> = msg
        .references()
        .as_text_list()
        .map(|l| l.iter().map(|c| c.as_ref()).collect())
        .or_else(|| msg.references().as_text().map(|t| vec![t]))
        .unwrap_or_default();
    assert_eq!(refs, ["root@acme.example", "orig-1@acme.example"]);
    let (html, text) = html_and_text(raw);
    assert!(
        html.contains(r#"<blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex"><h2>Plan</h2>"#),
        "{html}"
    );
    assert!(html.contains("<p>Looks <i>good</i>.</p>"), "{html}");
    assert!(text.contains("\n> Two changes"), "{text}");
    assert_eq!(
        attachments_of(raw),
        vec![(
            "notes.txt".to_string(),
            "text/plain".to_string(),
            b"notes".to_vec()
        )]
    );
}

#[tokio::test]
async fn reply_threads_under_the_parent_sent_directly_or_through_a_draft() {
    let e = env("reply");
    send(
        &e.store,
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &reply_draft(),
        None,
    )
    .await
    .unwrap();
    let saved = save_draft(&e.paths, &e.fake, &none(), &me(), &reply_draft(), None)
        .await
        .unwrap();
    send(
        &e.store,
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &Draft {
            attachments: saved.attachments.clone(),
            ..reply_draft()
        },
        Some(&saved.draft_id),
    )
    .await
    .unwrap();
    let out = e.fake.outgoing();
    assert_eq!(out.len(), 2);
    assert_reply(&out[0]);
    assert_reply(&out[1]);
}

#[tokio::test]
async fn rule_forward_keeps_html_and_files() {
    let e = env("rule");
    let d = crate::rules::runtime::forward_draft(&original(), "books@ledger.example");
    send(&e.store, &e.paths, &e.fake, &none(), &me(), &d, None)
        .await
        .unwrap();
    let raw = &e.fake.outgoing()[0];
    assert_eq!(attachments_of(raw).len(), 2);
    let (html, text) = html_and_text(raw);
    assert!(html.contains(r#"<div class="gmail_quote">"#), "{html}");
    assert!(
        html.contains("<h2>Plan</h2>") && html.contains("<b>Two</b>"),
        "{html}"
    );
    assert!(!html.contains("script"), "{html}");
    assert!(
        text.starts_with("---------- Forwarded message ---------\n"),
        "{text}"
    );
}

// ---------- quote_sources ----------

fn headers_only(m: &Message) -> Message {
    Message {
        body_text: String::new(),
        body_html: None,
        attachments: vec![],
        ..m.clone()
    }
}

#[tokio::test]
async fn quote_sources_downloads_a_headers_only_original_first() {
    let store = Store::open_in_memory().unwrap();
    let old = Message {
        id: "old".into(),
        ..original()
    };
    store.insert_header_messages(&[headers_only(&old)]).unwrap();
    assert_eq!(store.is_body_pending(ACCT, "old").unwrap(), Some(true));
    let fake = FakeProvider::new(ACCT, AccountProvider::Gmail, store.clone());
    fake.seed(old.clone());

    // No provider (signed out): an error, not an empty list.
    let err = quote_sources(&store, None, ACCT, &["old".into()], true)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::error::ErrorCode::Network);

    // The download fails: the error comes back for the composer to show.
    fake.fail_next(
        "fetch_messages",
        penguin_provider::Error::Network("offline".into()),
    );
    assert!(
        quote_sources(&store, Some(&fake), ACCT, &["old".into()], true)
            .await
            .is_err()
    );

    let (sources, threads) = quote_sources(&store, Some(&fake), ACCT, &["old".into()], true)
        .await
        .unwrap();
    assert_eq!(threads, ["t1"]);
    let s = &sources[0];
    assert_eq!(s.message_id, "old");
    let names: Vec<&str> = s.attachments.iter().map(|a| a.filename.as_str()).collect();
    assert_eq!(
        names,
        [
            "Scope of work v3.pdf",
            "Zeitplan – Überblick.xlsx",
            "logo.png"
        ]
    );
    let html = s.html.as_deref().unwrap();
    assert!(
        html.contains("<h2>Plan</h2>") && !html.contains("script"),
        "{html}"
    );
    assert!(s.text.as_deref().unwrap().starts_with("Plan"));
}

#[tokio::test]
async fn quote_sources_stays_local_for_stored_bodies() {
    let e = env("local");
    let (sources, threads) = quote_sources(&e.store, Some(&e.fake), ACCT, &["orig".into()], false)
        .await
        .unwrap();
    assert!(threads.is_empty());
    assert!(
        e.fake.calls().is_empty(),
        "no provider call: {:?}",
        e.fake.calls()
    );
    assert_eq!(sources[0].attachments.len(), 3);
    assert_eq!(
        (sources[0].html.clone(), sources[0].text.clone()),
        (None, None)
    );
    let err = quote_sources(&e.store, Some(&e.fake), ACCT, &["nope".into()], true)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::error::ErrorCode::NotFound);
}

// ---------- inline images ----------

const LOGO: &[u8] = b"\x89PNG\r\n\x1a\nacme logo";
const SHOT: &[u8] = b"\x89PNG\r\n\x1a\nscreenshot";

/// The original, now with its logo shown inline (and a tracking pixel).
fn original_with_logo() -> Message {
    let mut m = original();
    m.id = "orig-logo".into();
    m.body_html = Some(
        r#"<table><tr><td><img src="cid:logo@acme.example" alt="Acme" width="120"></td></tr></table><h2>Plan</h2><p><b>Two</b> changes.</p><img src="https://pixel.tracker.example/o.gif">"#.into(),
    );
    m
}

fn env_with_logo(tag: &str) -> Env {
    let e = env(tag);
    e.store.upsert_messages(&[original_with_logo()]).unwrap();
    e.fake.seed(original_with_logo());
    e.fake.seed_attachment("orig-logo", "a1", PDF);
    e.fake.seed_attachment("orig-logo", "a2", XLSX);
    e.fake.seed_attachment("orig-logo", "logo", LOGO);
    e
}

/// Content type (+ Content-ID) of every MIME part, depth first, indented.
fn tree(raw: &[u8]) -> Vec<String> {
    fn walk(m: &mail_parser::Message, id: usize, depth: usize, out: &mut Vec<String>) {
        let p = &m.parts[id];
        let ct = p
            .content_type()
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("")))
            .unwrap_or_default();
        let cid = p
            .content_id()
            .map(|c| format!(" <{c}>"))
            .unwrap_or_default();
        out.push(format!("{}{ct}{cid}", "  ".repeat(depth)));
        if let mail_parser::PartType::Multipart(kids) = &p.body {
            for k in kids {
                walk(m, *k as usize, depth + 1, out);
            }
        }
    }
    let m = MessageParser::default().parse(raw).unwrap();
    let mut out = Vec::new();
    walk(&m, 0, 0, &mut out);
    out
}

/// The bytes of the part with Content-ID `cid`.
fn part_bytes(raw: &[u8], cid: &str) -> Option<Vec<u8>> {
    let m = MessageParser::default().parse(raw).unwrap();
    let found = m.parts.iter().find(|p| p.content_id() == Some(cid));
    found.map(|p| p.contents().to_vec())
}

fn content_id(a: &OutgoingAttachment) -> String {
    a.content_id().expect("an inline image").to_string()
}

/// A reply the way the composer builds it from quote_sources: a pasted
/// screenshot in what I wrote, the original (with its logo) quoted, a file.
async fn reply_with_images(e: &Env) -> (Draft, String) {
    let (sources, _) = quote_sources(&e.store, Some(&e.fake), ACCT, &["orig-logo".into()], true)
        .await
        .unwrap();
    let src = &sources[0];
    assert_eq!(src.inline_images.len(), 1, "the logo, not the tracker");
    let logo_cid = content_id(&src.inline_images[0]);
    assert!(logo_cid.starts_with("img-") && logo_cid != "logo@acme.example");
    let quoted = src.html.clone().unwrap();
    assert!(
        quoted.contains(&format!(
            r#"<img src="cid:{logo_cid}" alt="Acme" width="120">"#
        )),
        "{quoted}"
    );
    assert!(
        !quoted.contains("pixel.tracker") && !quoted.contains("logo@acme"),
        "{quoted}"
    );
    let pasted = "img-0000000000000001aaaaaaaa@penguin";
    let draft = Draft {
        request_read_receipt: None,
        account_id: ACCT.into(),
        to: vec![addr(None, "dana@acme.example")],
        cc: vec![],
        bcc: vec![],
        subject: "Re: Scope of work v3".into(),
        body_text: "See the chart: [image: chart.png]\n\nOn …, Dana wrote:\n> Plan".into(),
        body_html: Some(format!(
            r#"<div dir="auto"><p style="margin:0">See the chart: <img src="cid:{pasted}" alt="chart.png" width="600" style="max-width:100%;height:auto"></p><br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">On …, Dana wrote:<br></div><blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex">{quoted}</blockquote></div></div>"#
        )),
        reply_to_thread_id: Some("t1".into()),
        reply_to_message_id: Some("orig-logo".into()),
        attachments: vec![
            OutgoingAttachment::File {
                filename: "chart.png".into(),
                mime_type: "image/png".into(),
                data_base64: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    SHOT,
                ),
                content_id: Some(pasted.into()),
            },
            src.inline_images[0].clone(),
            OutgoingAttachment::File {
                filename: "notes.txt".into(),
                mime_type: "text/plain".into(),
                data_base64: "bm90ZXM=".into(),
                content_id: None,
            },
        ],
    };
    (draft, logo_cid)
}

fn assert_inline_reply(raw: &[u8], logo_cid: &str) {
    let pasted = "img-0000000000000001aaaaaaaa@penguin";
    assert_eq!(
        tree(raw),
        vec![
            "multipart/mixed".to_string(),
            "  multipart/alternative".into(),
            "    text/plain".into(),
            "    multipart/related".into(),
            "      text/html".into(),
            format!("      image/png <{pasted}>"),
            format!("      image/png <{logo_cid}>"),
            "  text/plain".into(),
        ],
        "{}",
        String::from_utf8_lossy(raw)
    );
    assert_eq!(part_bytes(raw, pasted).as_deref(), Some(SHOT));
    assert_eq!(part_bytes(raw, logo_cid).as_deref(), Some(LOGO));
    let (html, _) = html_and_text(raw);
    // Every cid the HTML uses is a part of this message, and nothing remote.
    let used = penguin_render::referenced_cids(&html);
    assert_eq!(
        used,
        vec![pasted.to_string(), logo_cid.to_string()],
        "{html}"
    );
    assert!(
        !html.contains("https://pixel") && !html.contains("<img src=\"http"),
        "{html}"
    );
    assert_eq!(
        attachments_of(raw)
            .into_iter()
            .filter(|(_, ty, _)| ty == "text/plain")
            .count(),
        1,
        "notes.txt stays a file"
    );
}

#[tokio::test]
async fn reply_with_pasted_and_quoted_images_sends_them_as_related_parts() {
    let e = env_with_logo("inline-direct");
    let (draft, logo_cid) = reply_with_images(&e).await;
    send(&e.store, &e.paths, &e.fake, &none(), &me(), &draft, None)
        .await
        .unwrap();
    assert_inline_reply(&e.fake.outgoing()[0], &logo_cid);
}

#[tokio::test]
async fn inline_images_survive_autosave_reopen_and_send_through_the_draft() {
    let e = env_with_logo("inline-draft");
    let (draft, logo_cid) = reply_with_images(&e).await;
    let saved = save_draft(&e.paths, &e.fake, &none(), &me(), &draft, None)
        .await
        .unwrap();
    // Refs come back in the composer's order, the images keeping their ids.
    let ids: Vec<Option<&str>> = saved.attachments.iter().map(|a| a.content_id()).collect();
    assert_eq!(
        ids,
        vec![
            Some("img-0000000000000001aaaaaaaa@penguin"),
            Some(logo_cid.as_str()),
            None
        ]
    );
    // Reopened (a restart): the HTML still shows both, their parts are refs.
    let opened = e
        .fake
        .open_draft(Some(&saved.draft_id), None)
        .await
        .unwrap()
        .unwrap();
    let html = opened.draft.body_html.clone().unwrap();
    assert!(
        html.contains(
            r#"<img src="cid:img-0000000000000001aaaaaaaa@penguin" alt="chart.png" width="600">"#
        ),
        "{html}"
    );
    assert!(
        html.contains(&format!(
            r#"<img src="cid:{logo_cid}" alt="Acme" width="120">"#
        )),
        "{html}"
    );
    assert_eq!(opened.draft.attachments.len(), 3);
    // Autosave again from the reopened draft, then send through it.
    let resaved = save_draft(
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &Draft {
            attachments: opened.draft.attachments.clone(),
            ..draft.clone()
        },
        Some(&saved.draft_id),
    )
    .await
    .unwrap();
    send(
        &e.store,
        &e.paths,
        &e.fake,
        &none(),
        &me(),
        &Draft {
            attachments: resaved.attachments.clone(),
            ..draft
        },
        Some(&saved.draft_id),
    )
    .await
    .unwrap();
    assert_inline_reply(&e.fake.outgoing()[0], &logo_cid);
}

#[tokio::test]
async fn images_over_the_limit_fail_with_a_clear_message() {
    let e = env("inline-big");
    let big = OutgoingAttachment::Gmail {
        message_id: "orig".into(),
        attachment_id: "huge".into(),
        filename: "huge.png".into(),
        mime_type: "image/png".into(),
        size: 26 * 1024 * 1024,
        account_id: None,
        content_id: Some("img-1@penguin".into()),
    };
    let d = Draft {
        body_html: Some(r#"<p><img src="cid:img-1@penguin"></p>"#.into()),
        attachments: vec![big],
        ..reply_draft()
    };
    let err = send(&e.store, &e.paths, &e.fake, &none(), &me(), &d, None)
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        "Attachments and images total 26.0 MB; the limit is 25 MB"
    );
    assert!(e
        .fake
        .calls()
        .iter()
        .all(|c| !c.starts_with("get_attachment")));
}

#[tokio::test]
async fn rule_forward_carries_the_originals_inline_images() {
    let e = env_with_logo("inline-rule");
    let d = crate::rules::runtime::forward_draft(&original_with_logo(), "books@ledger.example");
    send(&e.store, &e.paths, &e.fake, &none(), &me(), &d, None)
        .await
        .unwrap();
    let raw = &e.fake.outgoing()[0];
    let (html, _) = html_and_text(raw);
    let used = penguin_render::referenced_cids(&html);
    assert_eq!(used.len(), 1, "{html}");
    assert_eq!(part_bytes(raw, &used[0]).as_deref(), Some(LOGO));
    assert!(!html.contains("pixel.tracker"), "{html}");
    // The PDF is still a file.
    assert!(attachments_of(raw)
        .iter()
        .any(|(name, _, bytes)| name == "Scope of work v3.pdf" && bytes == PDF));
}
