//! Headers-only (body pending) lifecycle and "free up space".

use crate::types::*;
use crate::Store;

const A: &str = "ada@penguin.example";
const DAY: i64 = 86_400_000;
const T0: i64 = 1_760_000_000_000;

fn msg(id: &str, thread: &str, date: i64, body: &str) -> Message {
    Message {
        account_id: A.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: Address {
            name: Some("Bea Quill".into()),
            email: "bea@quill.example".into(),
        },
        to: vec![Address {
            name: None,
            email: A.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: format!("Harbor report {id}"),
        snippet: format!("lighthouse snippet {id}"),
        body_text: body.into(),
        body_html: (!body.is_empty()).then(|| format!("<p>{body}</p>")),
        label_ids: vec!["INBOX".into(), "UNREAD".into()],
        attachments: vec![AttachmentMeta {
            id: "att0".into(),
            filename: "tides.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 10,
            content_id: None,
            inline: false,
        }],
        message_id_header: Some(format!("<{id}@quill.example>")),
        in_reply_to: None,
        references: vec![],
        sender_authenticated: false,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
    }
}

fn headers(id: &str, thread: &str, date: i64) -> Message {
    let mut m = msg(id, thread, date, "");
    m.attachments.clear();
    m
}

fn search_ids(s: &Store, q: &str) -> Vec<String> {
    let mut ids: Vec<String> = s
        .search(&SearchRequest {
            query: q.into(),
            account_id: None,
            account_ids: None,
            limit: 50,
        })
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.message_id)
        .collect();
    ids.sort();
    ids
}

fn coverage(s: &Store) -> (u64, u64) {
    let c = s.body_coverage().unwrap();
    let row = c
        .iter()
        .find(|c| c.account_id == A)
        .cloned()
        .unwrap_or_default();
    (row.full, row.headers_only)
}

#[test]
fn headers_only_messages_are_searchable_by_subject_sender_and_snippet() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(
        s.insert_header_messages(&[headers("h1", "t1", T0)])
            .unwrap(),
        1
    );
    assert_eq!(search_ids(&s, "harbor"), vec!["h1"]);
    assert_eq!(search_ids(&s, "from:bea"), vec!["h1"]);
    assert_eq!(search_ids(&s, "lighthouse"), vec!["h1"]);
    assert_eq!(s.is_body_pending(A, "h1").unwrap(), Some(true));
    assert_eq!(s.pending_message_ids(A, "t1").unwrap(), vec!["h1"]);
    assert_eq!(coverage(&s), (0, 1));
    assert_eq!(s.count_messages(Some(A)).unwrap(), 1);
    // It lists like any other thread.
    let q = ListQuery {
        view: MailboxView::Inbox,
        tab: None,
        account_id: None,
        account_ids: None,
        limit: 10,
        before: None,
        unread_only: false,
        split: None,
    };
    assert_eq!(s.list_threads(&q).unwrap().len(), 1);
}

#[test]
fn full_upsert_replaces_pending_and_reindexes_the_body() {
    let s = Store::open_in_memory().unwrap();
    s.insert_header_messages(&[headers("h1", "t1", T0)])
        .unwrap();
    assert!(search_ids(&s, "narwhal").is_empty());
    assert_eq!(
        s.ids_needing_body(A, &["h1".into(), "new".into()]).unwrap(),
        vec!["h1", "new"]
    );

    s.upsert_messages(&[msg("h1", "t1", T0, "the narwhal migration")])
        .unwrap();
    assert_eq!(search_ids(&s, "narwhal"), vec!["h1"]);
    assert_eq!(s.is_body_pending(A, "h1").unwrap(), Some(false));
    assert!(s.pending_message_ids(A, "t1").unwrap().is_empty());
    assert_eq!(
        s.ids_needing_body(A, &["h1".into()]).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(coverage(&s), (1, 0));
    assert_eq!(s.count_messages(Some(A)).unwrap(), 1);
}

#[test]
fn header_insert_never_downgrades_a_full_message() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_messages(&[msg("m1", "t1", T0, "orca body")])
        .unwrap();
    assert_eq!(
        s.insert_header_messages(&[headers("m1", "t1", T0)])
            .unwrap(),
        0
    );
    assert_eq!(s.is_body_pending(A, "m1").unwrap(), Some(false));
    assert_eq!(search_ids(&s, "orca"), vec!["m1"]);
    assert_eq!(s.count_messages(Some(A)).unwrap(), 1);
}

#[test]
fn label_changes_keep_the_pending_flag() {
    let s = Store::open_in_memory().unwrap();
    s.insert_header_messages(&[headers("h1", "t1", T0)])
        .unwrap();
    s.modify_message_labels(A, &["h1".into()], &["STARRED".into()], &["UNREAD".into()])
        .unwrap();
    assert_eq!(s.is_body_pending(A, "h1").unwrap(), Some(true));
    let m = s.get_message(A, "h1").unwrap().unwrap();
    assert!(m.is_starred() && !m.is_unread());
}

#[test]
fn free_up_space_drops_old_bodies_and_keeps_headers() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_messages(&[
        msg("old", "t1", T0 - 400 * DAY, "ancient walrus"),
        msg("new", "t2", T0 - DAY, "fresh walrus"),
    ])
    .unwrap();
    let mut draft = msg("draft", "t3", T0 - 500 * DAY, "unsent walrus");
    draft.label_ids = vec!["DRAFT".into()];
    s.upsert_messages(&[draft]).unwrap();
    let cutoff = T0 - 180 * DAY;
    assert_eq!(s.count_full_before(A, cutoff).unwrap(), 1);
    let (n, bytes) = s.full_bodies_before(A, cutoff).unwrap();
    assert_eq!(n, 1);
    assert!(bytes > 0);

    assert_eq!(s.drop_bodies_before(A, cutoff).unwrap(), 1);
    let old = s.get_message(A, "old").unwrap().unwrap();
    assert_eq!(old.body_text, "");
    assert_eq!(old.body_html, None);
    assert_eq!(old.subject, "Harbor report old");
    assert_eq!(old.attachments.len(), 1, "attachment metadata stays");
    assert_eq!(s.is_body_pending(A, "old").unwrap(), Some(true));
    // Body words are gone from the index; headers, snippet, filenames stay.
    assert_eq!(search_ids(&s, "walrus"), vec!["draft", "new"]);
    assert_eq!(search_ids(&s, "lighthouse old"), vec!["old"]);
    assert_eq!(
        search_ids(&s, "filename:tides"),
        vec!["draft", "new", "old"]
    );
    assert_eq!(coverage(&s), (2, 1));
    // Idempotent, and vacuum works after it.
    assert_eq!(s.drop_bodies_before(A, cutoff).unwrap(), 0);
    assert_eq!(s.full_bodies_before(A, cutoff).unwrap().0, 0);
    s.vacuum().unwrap();
    // Opening it later restores the body.
    s.upsert_messages(&[msg("old", "t1", T0 - 400 * DAY, "ancient walrus")])
        .unwrap();
    assert_eq!(search_ids(&s, "ancient"), vec!["old"]);
}

#[test]
fn window_cursor_round_trips() {
    let s = Store::open_in_memory().unwrap();
    let cur = SyncCursor {
        provider_state: r#"{"historyId":9}"#.into(),
        window: WindowCursor {
            full_since_ms: Some(T0),
            fill_after_ms: Some(T0 - DAY),
            fill_before_ms: Some(T0),
            fill_page_token: Some("p1".into()),
            older_before_ms: Some(T0 - DAY),
            older_page_token: None,
            older_done: true,
            older_mode: Some("headers".into()),
        },
        ..Default::default()
    };
    s.set_sync_cursor(A, &cur).unwrap();
    assert_eq!(s.get_sync_cursor(A).unwrap(), cur);
}
