//! The conformance suite every provider must pass (docs/PROVIDERS-IMPL.md →
//! Test harness). Two layers:
//!
//! - Pure checks on what a provider produces ([`check_message`],
//!   [`check_labels`]): run them on every message your converters build in
//!   unit tests (fixtures with fictional `.example` data).
//! - [`run`]: drives a live [`MailProvider`] through the calls the app makes
//!   (fetch, headers, raw source, attachments, label changes, archive,
//!   report spam / not spam, trash, drafts, send, search) against a
//!   mailbox that holds one known message. Point it at a test server
//!   (Dovecot/GreenMail/Stalwart for IMAP, recorded fixtures for Graph) or
//!   at the [`crate::fake`] provider.
//!   It changes the mailbox (and sends mail when `send_to` is set), so
//!   never at a real person's account.
//!
//! Every failure is a sentence in the returned list; an empty list passes.

use penguin_core::{AccountProvider, Address, AttachmentMeta, Label, Message, Store};

use crate::compose::{build_rfc822, Draft};
use crate::ids::{check_id, system};
use crate::provider::MailProvider;

/// Problems with one message as a provider built it.
pub fn check_message(provider: AccountProvider, account_id: &str, m: &Message) -> Vec<String> {
    let mut out = Vec::new();
    let what = format!("message {}", m.id);
    if m.account_id != account_id {
        out.push(format!(
            "{what}: account_id is {:?}, not {account_id:?}",
            m.account_id
        ));
    }
    if let Some(why) = check_id(&m.id) {
        out.push(format!("{what}: id is {why}"));
    }
    if let Some(why) = check_id(&m.thread_id) {
        out.push(format!("{what}: thread id is {why}"));
    }
    if m.date <= 0 {
        out.push(format!("{what}: date {} is not unix ms", m.date));
    } else if m.date < 100_000_000_000 {
        out.push(format!(
            "{what}: date {} looks like seconds, not ms",
            m.date
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for l in &m.label_ids {
        if !seen.insert(l) {
            out.push(format!("{what}: label {l:?} twice"));
        }
        if let Some(why) = check_id(l) {
            out.push(format!("{what}: label {l:?} is {why}"));
        }
        if l.starts_with('\\') || l.starts_with('$') {
            out.push(format!(
                "{what}: label {l:?} is a raw IMAP flag; map it (\\Seen → no UNREAD, \\Flagged → STARRED) or drop it"
            ));
        }
        if provider != AccountProvider::Gmail
            && l.chars().all(|c| c.is_ascii_uppercase() || c == '_')
            && !system::ALL.contains(&l.as_str())
        {
            out.push(format!(
                "{what}: {l:?} looks like a system label but isn't one of {:?}",
                system::ALL
            ));
        }
    }
    if m.label_ids.iter().any(|l| l == system::DRAFT)
        && m.label_ids.iter().any(|l| l == system::SENT)
    {
        out.push(format!("{what}: both DRAFT and SENT"));
    }
    if m.label_ids.iter().any(|l| l == system::TRASH)
        && m.label_ids.iter().any(|l| l == system::INBOX)
    {
        out.push(format!("{what}: both TRASH and INBOX"));
    }
    for a in &m.attachments {
        if let Some(why) = check_id(&a.id) {
            out.push(format!("{what}: attachment id {:?} is {why}", a.id));
        }
        if a.filename.is_empty() {
            out.push(format!("{what}: attachment {} has no filename", a.id));
        }
        if a.inline && a.content_id.is_none() {
            out.push(format!(
                "{what}: inline attachment {} has no Content-ID",
                a.id
            ));
        }
    }
    if let Some(h) = &m.message_id_header {
        if h.starts_with('<') || h.ends_with('>') {
            out.push(format!(
                "{what}: message_id_header keeps its angle brackets"
            ));
        }
    }
    out
}

/// Problems with a provider's label list.
pub fn check_labels(account_id: &str, labels: &[Label]) -> Vec<String> {
    let mut out = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for l in labels {
        if l.account_id != account_id {
            out.push(format!("label {}: wrong account {:?}", l.id, l.account_id));
        }
        if !ids.insert(&l.id) {
            out.push(format!("label {}: listed twice", l.id));
        }
        if let Some(why) = check_id(&l.id) {
            out.push(format!("label {:?}: id is {why}", l.id));
        }
        match l.kind.as_str() {
            "system" if !system::is_system(&l.id) => {
                out.push(format!("label {}: kind system but not a system id", l.id))
            }
            "user" if system::ALL.contains(&l.id.as_str()) => {
                out.push(format!("label {}: a system id with kind user", l.id))
            }
            "system" | "user" => {}
            other => out.push(format!("label {}: kind {other:?}", l.id)),
        }
        if l.name.trim().is_empty() {
            out.push(format!("label {}: no name", l.id));
        }
    }
    out
}

/// A mailbox fixture for [`run`].
pub struct Case {
    /// A message on the server (in the inbox, not starred), stored locally.
    pub message_id: String,
    pub thread_id: String,
    /// One of its attachments, when it has any.
    pub attachment: Option<AttachmentMeta>,
    /// The account's own address (From of drafts).
    pub from: Address,
    /// Send one real message here (`send_raw`, `send_saved_draft`); None
    /// skips sending.
    pub send_to: Option<Address>,
}

fn labels_of(outcome: &[(String, crate::Result<Option<Message>>)]) -> Option<Vec<String>> {
    match outcome.first() {
        Some((_, Ok(Some(m)))) => Some(m.label_ids.clone()),
        _ => None,
    }
}

/// Drive `p` through every call the app makes; see the module docs.
/// `store` is the store the provider writes to.
pub async fn run(p: &dyn MailProvider, store: &Store, case: &Case) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let kind = p.provider();
    let caps = p.capabilities();
    let acct = p.account_id().to_string();
    let id = case.message_id.clone();

    // Reading.
    let got = p
        .fetch_messages(&[id.clone(), "conformance-missing-7f3a".into()])
        .await;
    match got.as_slice() {
        [(a, Ok(Some(m))), (b, Ok(None))] if *a == id && b == "conformance-missing-7f3a" => {
            out.extend(check_message(kind, &acct, m));
            if m.thread_id != case.thread_id {
                out.push(format!(
                    "fetch_messages: thread {} ≠ the fixture's {}",
                    m.thread_id, case.thread_id
                ));
            }
        }
        other => out.push(format!(
            "fetch_messages: want [Some, None] in input order, got {:?}",
            other
                .iter()
                .map(|(i, r)| format!("{i}: {}", describe(r)))
                .collect::<Vec<_>>()
        )),
    }
    if caps.message_headers {
        match p.get_message_metadata(&id).await {
            Ok(Some(meta)) if !meta.headers.is_empty() => {}
            other => out.push(format!(
                "get_message_metadata: {:?}",
                other.map(|m| m.map(|m| m.headers.len()))
            )),
        }
        match p.get_message_metadata("conformance-missing-7f3a").await {
            Ok(None) => {}
            other => out.push(format!(
                "get_message_metadata(missing) should be Ok(None), got {:?}",
                other.map(|m| m.is_some())
            )),
        }
    }
    if caps.raw_source {
        match p.get_message_raw(&id).await {
            Ok(Some(raw)) if !raw.is_empty() => {}
            other => out.push(format!(
                "get_message_raw: {:?}",
                other.map(|r| r.map(|r| r.len()))
            )),
        }
    }
    if let Some(att) = &case.attachment {
        match p.get_attachment(&id, att).await {
            Ok(bytes) if !bytes.is_empty() => {}
            Ok(_) => out.push("get_attachment: empty".into()),
            Err(e) => out.push(format!("get_attachment: {e}")),
        }
    }

    // Changing: each change must show on the server (fetch_messages).
    let t = case.thread_id.clone();
    // (name, add, remove, the label to check, whether it should be there)
    type Step<'a> = (&'a str, Vec<String>, Vec<String>, &'a str, bool);
    let steps: [Step; 6] = [
        (
            "star",
            vec![system::STARRED.into()],
            vec![],
            system::STARRED,
            true,
        ),
        (
            "unstar",
            vec![],
            vec![system::STARRED.into()],
            system::STARRED,
            false,
        ),
        (
            "mark unread",
            vec![system::UNREAD.into()],
            vec![],
            system::UNREAD,
            true,
        ),
        (
            "mark read",
            vec![],
            vec![system::UNREAD.into()],
            system::UNREAD,
            false,
        ),
        (
            "archive",
            vec![],
            vec![system::INBOX.into()],
            system::INBOX,
            false,
        ),
        (
            "move to inbox",
            vec![system::INBOX.into()],
            vec![],
            system::INBOX,
            true,
        ),
    ];
    for (name, add, remove, label, present) in steps {
        if let Err(e) = p.modify_thread(&t, &add, &remove).await {
            out.push(format!("{name}: {e}"));
            continue;
        }
        match labels_of(&p.fetch_messages(std::slice::from_ref(&id)).await) {
            Some(l) if l.iter().any(|x| x == label) == present => {}
            Some(l) => out.push(format!("{name}: labels now {l:?}")),
            None => out.push(format!("{name}: message gone after the change")),
        }
    }
    // Report spam and Not spam (the app's ThreadAction::ReportSpam /
    // NotSpam): into Spam and out of the inbox, then back. IMAP moves to
    // and from the Junk folder (created when the server has none), Graph
    // to and from junkemail, Gmail relabels.
    // (name, add, remove, labels that must be there, labels that must not)
    type SpamStep<'a> = (&'a str, Vec<String>, Vec<String>, &'a str, &'a str);
    let spam_steps: [SpamStep; 2] = [
        (
            "report spam",
            vec![system::SPAM.into()],
            vec![system::INBOX.into()],
            system::SPAM,
            system::INBOX,
        ),
        (
            "not spam",
            vec![system::INBOX.into()],
            vec![system::SPAM.into()],
            system::INBOX,
            system::SPAM,
        ),
    ];
    for (name, add, remove, there, gone) in spam_steps {
        if let Err(e) = p.modify_thread(&t, &add, &remove).await {
            out.push(format!("{name}: {e}"));
            continue;
        }
        match labels_of(&p.fetch_messages(std::slice::from_ref(&id)).await) {
            Some(l) if l.iter().any(|x| x == there) && !l.iter().any(|x| x == gone) => {}
            Some(l) => out.push(format!("{name}: labels now {l:?}")),
            None => out.push(format!("{name}: message gone after the change")),
        }
    }
    match p.trash_thread(&t).await {
        Ok(()) => match labels_of(&p.fetch_messages(std::slice::from_ref(&id)).await) {
            Some(l)
                if l.iter().any(|x| x == system::TRASH)
                    && !l.iter().any(|x| x == system::INBOX) => {}
            other => out.push(format!("trash: labels now {other:?}")),
        },
        Err(e) => out.push(format!("trash: {e}")),
    }
    match p.untrash_thread(&t).await {
        Ok(()) => match labels_of(&p.fetch_messages(std::slice::from_ref(&id)).await) {
            Some(l)
                if l.iter().any(|x| x == system::INBOX)
                    && !l.iter().any(|x| x == system::TRASH) => {}
            other => out.push(format!("untrash: labels now {other:?}")),
        },
        Err(e) => out.push(format!("untrash: {e}")),
    }
    match p
        .modify_thread("conformance-missing-thread", &[system::STARRED.into()], &[])
        .await
    {
        Err(e) if e.is_not_found() => {}
        other => out.push(format!(
            "modify_thread(missing thread) should be not-found, got {:?}",
            other.map_err(|e| e.to_string())
        )),
    }

    // Drafts.
    if caps.drafts {
        out.extend(drafts(p, store, &acct, case).await);
    }

    // Sending.
    if let Some(to) = &case.send_to {
        let draft = Draft {
            to: vec![to.clone()],
            subject: "Penguin conformance send".into(),
            body_text: "Conformance test message; please disregard.\n".into(),
            ..blank(&acct)
        };
        match build_rfc822(&draft, &case.from, None, &[]) {
            Ok(raw) => match p.send_raw(&raw, None).await {
                Ok(sent) => {
                    for (what, v) in [("message", &sent.message_id), ("thread", &sent.thread_id)] {
                        if let Some(why) = check_id(v) {
                            out.push(format!("send_raw: {what} id {why}"));
                        }
                    }
                }
                Err(e) => out.push(format!("send_raw: {e}")),
            },
            Err(e) => out.push(format!("compose: {e}")),
        }
    }

    // Optional capabilities: when claimed, they work.
    if caps.server_search {
        let q = penguin_core::query::parse("conformance", 0);
        if let Err(e) = p.server_search(&q, 5).await {
            out.push(format!("server_search: {e}"));
        }
    }
    if caps.window_estimate {
        if let Err(e) = p.window_estimate(0, 1_760_000_000_000).await {
            out.push(format!("window_estimate: {e}"));
        }
    }
    out
}

fn blank(account_id: &str) -> Draft {
    Draft {
        request_read_receipt: None,
        account_id: account_id.to_string(),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        subject: String::new(),
        body_text: String::new(),
        body_html: None,
        reply_to_thread_id: None,
        reply_to_message_id: None,
        attachments: vec![],
    }
}

fn describe(r: &crate::Result<Option<Message>>) -> String {
    match r {
        Ok(Some(_)) => "Some".into(),
        Ok(None) => "None".into(),
        Err(e) => format!("Err({e})"),
    }
}

async fn drafts(p: &dyn MailProvider, store: &Store, acct: &str, case: &Case) -> Vec<String> {
    let mut out = Vec::new();
    let mut draft = Draft {
        subject: "Penguin conformance draft".into(),
        body_text: "first version\n".into(),
        ..blank(acct)
    };
    let first = match p.save_draft(&draft, &case.from, None, &[]).await {
        Ok(r) => r,
        Err(e) => return vec![format!("save_draft (create): {e}")],
    };
    for (what, v) in [
        ("draft", &first.draft_id),
        ("message", &first.message_id),
        ("thread", &first.thread_id),
    ] {
        if let Some(why) = check_id(v) {
            out.push(format!("save_draft: {what} id {why}"));
        }
    }
    let mirrored = |mid: String| {
        let (store, acct) = (store.clone(), acct.to_string());
        move || -> (bool, Option<String>) {
            let local = store.get_message(&acct, &mid).ok().flatten();
            let is_draft = local
                .as_ref()
                .is_some_and(|m| m.label_ids.iter().any(|l| l == system::DRAFT));
            let mapped = store.draft_for_message(&acct, &mid).ok().flatten();
            (is_draft, mapped)
        }
    };
    let (is_draft, mapped) = mirrored(first.message_id.clone())();
    if !is_draft {
        out.push("save_draft: no local copy labeled DRAFT".into());
    }
    if mapped.as_deref() != Some(first.draft_id.as_str()) {
        out.push(format!("save_draft: local draft mapping is {mapped:?}"));
    }
    draft.body_text = "second version\n".into();
    match p
        .save_draft(&draft, &case.from, Some(&first.draft_id), &[])
        .await
    {
        Ok(r) if r.draft_id == first.draft_id => {
            let old_gone = first.message_id == r.message_id
                || store
                    .get_message(acct, &first.message_id)
                    .ok()
                    .flatten()
                    .is_none();
            if !old_gone {
                out.push("save_draft (update): the previous local version is still stored".into());
            }
        }
        Ok(r) => out.push(format!(
            "save_draft (update): draft id changed {} → {}",
            first.draft_id, r.draft_id
        )),
        Err(e) => out.push(format!("save_draft (update): {e}")),
    }
    match p.open_draft(Some(&first.draft_id), None).await {
        Ok(Some(o)) if o.draft.body_text.contains("second version") => {}
        Ok(Some(_)) => out.push("open_draft: not the latest content".into()),
        other => out.push(format!(
            "open_draft: {:?}",
            other.map(|o| o.is_some()).map_err(|e| e.to_string())
        )),
    }
    match p.delete_draft(&first.draft_id).await {
        Ok(_) => {
            if let Ok(Some(_)) = p.open_draft(Some(&first.draft_id), None).await {
                out.push("delete_draft: still opens".into());
            }
            if store
                .message_for_draft(acct, &first.draft_id)
                .ok()
                .flatten()
                .is_some()
            {
                out.push("delete_draft: local mapping kept".into());
            }
        }
        Err(e) => out.push(format!("delete_draft: {e}")),
    }
    if let Err(e) = p.delete_draft(&first.draft_id).await {
        out.push(format!("delete_draft twice should be fine: {e}"));
    }
    out.extend(inline_image_draft(p, acct, case).await);
    if let Some(to) = &case.send_to {
        let later = Draft {
            to: vec![to.clone()],
            subject: "Penguin conformance send later".into(),
            body_text: "Conformance test message; please disregard.\n".into(),
            ..blank(acct)
        };
        match p.save_draft(&later, &case.from, None, &[]).await {
            Ok(r) => {
                if let Err(e) = p.send_saved_draft(&r.draft_id).await {
                    out.push(format!("send_saved_draft: {e}"));
                }
                match p.send_saved_draft(&r.draft_id).await {
                    Err(e) if e.is_not_found() => {}
                    other => out.push(format!(
                        "send_saved_draft twice must be not-found (no double send), got {:?}",
                        other.map_err(|e| e.to_string())
                    )),
                }
            }
            Err(e) => out.push(format!("save_draft (send later): {e}")),
        }
    }
    out
}

/// A draft showing a pasted image (`multipart/related` in MIME, an
/// `isInline` attachment with `contentId` on Graph) keeps it across save,
/// reopen and a second save: the HTML still references it, its part comes
/// back as a ref with the same Content-ID and the same bytes, and saving
/// again with that ref doesn't duplicate it.
async fn inline_image_draft(p: &dyn MailProvider, acct: &str, case: &Case) -> Vec<String> {
    use crate::compose::{AttachmentBytes, OutgoingAttachment};
    let mut out = Vec::new();
    let cid = "img-conformance1@penguin";
    let png: Vec<u8> = b"\x89PNG\r\n\x1a\nconformance".to_vec();
    let html = format!(r#"<p>Look: <img src="cid:{cid}" alt="dot.png" width="40"></p>"#);
    let file = OutgoingAttachment::File {
        filename: "dot.png".into(),
        mime_type: "image/png".into(),
        data_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png),
        content_id: Some(cid.into()),
    };
    let bytes = vec![AttachmentBytes {
        filename: "dot.png".into(),
        mime_type: "image/png".into(),
        bytes: png.clone(),
        content_id: Some(cid.into()),
    }];
    let mut draft = Draft {
        subject: "Penguin conformance inline image".into(),
        body_text: "Look: [image: dot.png]\n".into(),
        body_html: Some(html),
        attachments: vec![file],
        ..blank(acct)
    };
    let saved = match p.save_draft(&draft, &case.from, None, &bytes).await {
        Ok(r) => r,
        Err(e) => return vec![format!("save_draft (inline image): {e}")],
    };
    let cids: Vec<Option<&str>> = saved.attachments.iter().map(|a| a.content_id()).collect();
    if cids != vec![Some(cid)] {
        out.push(format!("save_draft (inline image): refs carry {cids:?}"));
    }
    for round in ["reopen", "reopen after a second save"] {
        let opened = match p.open_draft(Some(&saved.draft_id), None).await {
            Ok(Some(o)) => o,
            other => {
                out.push(format!(
                    "open_draft (inline image, {round}): {:?}",
                    other.map(|o| o.is_some()).map_err(|e| e.to_string())
                ));
                break;
            }
        };
        let html = opened.draft.body_html.clone().unwrap_or_default();
        if !html.contains(&format!("src=\"cid:{cid}\"")) {
            out.push(format!(
                "open_draft (inline image, {round}): HTML lost the image: {html}"
            ));
        }
        let images: Vec<&OutgoingAttachment> = opened
            .draft
            .attachments
            .iter()
            .filter(|a| a.content_id() == Some(cid))
            .collect();
        if images.len() != 1 || opened.draft.attachments.len() != 1 {
            out.push(format!(
                "open_draft (inline image, {round}): attachments {:?}",
                opened.draft.attachments
            ));
            break;
        }
        if let OutgoingAttachment::Gmail {
            message_id,
            attachment_id,
            filename,
            mime_type,
            size,
            ..
        } = images[0]
        {
            let meta = penguin_core::AttachmentMeta {
                id: attachment_id.clone(),
                filename: filename.clone(),
                mime_type: mime_type.clone(),
                size: *size,
                content_id: Some(cid.into()),
                inline: true,
            };
            match p.get_attachment(message_id, &meta).await {
                Ok(b) if b == png => {}
                Ok(b) => out.push(format!(
                    "open_draft (inline image, {round}): {} bytes back, not the image",
                    b.len()
                )),
                Err(e) => out.push(format!("get_attachment (inline image, {round}): {e}")),
            }
        }
        if round == "reopen" {
            // Save again pointing at the stored part, as the composer does.
            draft.attachments = opened.draft.attachments.clone();
            draft.body_text = "Look again: [image: dot.png]\n".into();
            if let Err(e) = p
                .save_draft(&draft, &case.from, Some(&saved.draft_id), &bytes)
                .await
            {
                out.push(format!("save_draft (inline image, again): {e}"));
                break;
            }
        }
    }
    if let Err(e) = p.delete_draft(&saved.draft_id).await {
        out.push(format!("delete_draft (inline image): {e}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use penguin_core::{Account, AttachmentMeta, Label};

    use super::*;
    use crate::fake::FakeProvider;

    const A: &str = "ada@fake.example";

    fn message() -> Message {
        Message {
            account_id: A.into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 1_760_000_000_000,
            from: Address {
                name: Some("Cy Quill".into()),
                email: "cy@quill.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Quarterly conformance report".into(),
            snippet: String::new(),
            body_text: "hello".into(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![AttachmentMeta {
                id: "1.2".into(),
                filename: "report.pdf".into(),
                mime_type: "application/pdf".into(),
                size: 3,
                content_id: None,
                inline: false,
            }],
            message_id_header: Some("m1@quill.example".into()),
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    #[tokio::test]
    async fn the_fake_provider_passes() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_account(&Account {
                id: A.into(),
                email: A.into(),
                provider: AccountProvider::Imap,
                ..Account::default()
            })
            .unwrap();
        let p = FakeProvider::new(A, AccountProvider::Imap, store.clone());
        p.seed(message());
        p.seed_attachment("m1", "1.2", b"%PDF");
        store.upsert_messages(&[message()]).unwrap();
        let case = Case {
            message_id: "m1".into(),
            thread_id: "t1".into(),
            attachment: Some(message().attachments[0].clone()),
            from: Address {
                name: None,
                email: A.into(),
            },
            send_to: Some(Address {
                name: None,
                email: "bea@fake.example".into(),
            }),
        };
        let dynp: Arc<dyn MailProvider> = Arc::new(p.clone());
        let failures = run(dynp.as_ref(), &store, &case).await;
        assert!(failures.is_empty(), "{failures:#?}");
        assert!(p.calls().iter().any(|c| c.starts_with("send_saved_draft")));
        // Report spam and Not spam reach the provider as one delta each.
        let calls = p.calls();
        assert!(
            calls.iter().any(|c| c == "modify_thread t1 +SPAM -INBOX"),
            "{calls:#?}"
        );
        assert!(
            calls.iter().any(|c| c == "modify_thread t1 +INBOX -SPAM"),
            "{calls:#?}"
        );
    }

    #[tokio::test]
    async fn a_broken_provider_is_caught() {
        let store = Store::open_in_memory().unwrap();
        let p = FakeProvider::new(A, AccountProvider::Imap, store.clone());
        let mut m = message();
        m.label_ids.push("\\Seen".into());
        p.seed(m);
        p.fail_next("trash_thread", crate::Error::Network("offline".into()));
        let case = Case {
            message_id: "m1".into(),
            thread_id: "t1".into(),
            attachment: None,
            from: Address {
                name: None,
                email: A.into(),
            },
            send_to: None,
        };
        let failures = run(&p, &store, &case).await;
        assert!(
            failures.iter().any(|f| f.contains("raw IMAP flag")),
            "{failures:#?}"
        );
        assert!(
            failures.iter().any(|f| f.starts_with("trash:")),
            "{failures:#?}"
        );
    }

    #[test]
    fn message_checks() {
        let mut m = message();
        assert!(check_message(AccountProvider::Imap, A, &m).is_empty());
        m.date = 1_760_000_000;
        m.label_ids = vec![
            "INBOX".into(),
            "TRASH".into(),
            "My Folder".into(),
            "ARCHIVE".into(),
        ];
        m.message_id_header = Some("<x@y>".into());
        let p = check_message(AccountProvider::Imap, A, &m);
        for want in [
            "seconds",
            "TRASH and INBOX",
            "not printable",
            "ARCHIVE",
            "angle brackets",
        ] {
            assert!(p.iter().any(|x| x.contains(want)), "{want}: {p:#?}");
        }
        // Gmail's own label ids (CATEGORY_*, Label_1) are fine for Gmail.
        m = message();
        m.label_ids = vec!["CATEGORY_UPDATES".into()];
        assert!(check_message(AccountProvider::Gmail, A, &m).is_empty());
    }

    #[test]
    fn label_checks() {
        let label = |id: &str, kind: &str| Label {
            account_id: A.into(),
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
            color: None,
            unread_count: None,
            hidden: false,
        };
        assert!(
            check_labels(A, &[label("INBOX", "system"), label("f:Receipts", "user")]).is_empty()
        );
        let p = check_labels(
            A,
            &[
                label("INBOX", "user"),
                label("f:My Folder", "user"),
                label("x", "folder"),
            ],
        );
        assert_eq!(p.len(), 3, "{p:#?}");
    }
}
