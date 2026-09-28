//! The provider and the sync engine against the in-memory Graph
//! (`fake_graph.rs`): the conformance suite, backfill/resume, incremental
//! changes, moves, 410 recovery, window changes, throttling, token expiry,
//! drafts with attachments. Fictional `.example` data only.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use penguin_core::{Account, AccountProvider, Address, Store, SyncPhase, SyncStatus};
use penguin_provider::compose::{AttachmentBytes, Draft, OutgoingAttachment};
use penguin_provider::conformance::{self, Case};
use penguin_provider::credentials::{MemorySecrets, MicrosoftCredential, SecretVault};
use penguin_provider::window::{OlderMail, WindowPolicy, DAY_MS};
use penguin_provider::{Backend, Error, MailProvider, SyncObserver};

use crate::auth::Auth;
use crate::fake_graph::{endpoints, FAtt, FMsg, FakeGraph, ME};
use crate::sync::{label_diff, AccountSync};
use crate::GraphBackend;

#[derive(Default)]
struct Recorder {
    statuses: Mutex<Vec<SyncStatus>>,
    changed: Mutex<Vec<String>>,
    added: Mutex<Vec<String>>,
    labels_added: Mutex<Vec<(String, Vec<String>)>>,
}

impl SyncObserver for Recorder {
    fn status(&self, s: SyncStatus) {
        self.statuses.lock().unwrap().push(s);
    }
    fn mail_changed(&self, _: &str, threads: Vec<String>) {
        self.changed.lock().unwrap().extend(threads);
    }
    fn messages_added(&self, _: &str, ids: Vec<String>) {
        self.added.lock().unwrap().extend(ids);
    }
    fn labels_added(&self, _: &str, changes: Vec<(String, Vec<String>)>) {
        self.labels_added.lock().unwrap().extend(changes);
    }
}

struct Harness {
    fake: Arc<FakeGraph>,
    store: Store,
    backend: GraphBackend,
    rec: Arc<Recorder>,
    account: Account,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn harness_with(policy: WindowPolicy) -> Harness {
    let fake = FakeGraph::new();
    let mem = Arc::new(MemorySecrets::default());
    let vault: SecretVault<MicrosoftCredential> = SecretVault::new(Box::new(mem));
    vault
        .save(
            ME,
            &MicrosoftCredential {
                refresh_token: "rt-0".into(),
                client_id: "1b2c3d4e-0000-1111-2222-333344445555".into(),
                scopes: vec![],
                tenant_id: None,
                object_id: None,
                obtained_at: 0,
            },
        )
        .unwrap();
    let auth = Arc::new(Auth::new(fake.clone(), endpoints(), Arc::new(vault)));
    let store = Store::open_in_memory().unwrap();
    let account = Account {
        id: ME.into(),
        email: ME.into(),
        provider: AccountProvider::Microsoft,
        ..Account::default()
    };
    store.upsert_account(&account).unwrap();
    let rec = Arc::new(Recorder::default());
    let backend = GraphBackend::with_parts(store.clone(), rec.clone(), auth, fake.clone()).unwrap();
    backend.set_window_policy(policy);
    Harness {
        fake,
        store,
        backend,
        rec,
        account,
    }
}

fn harness() -> Harness {
    harness_with(WindowPolicy {
        months: 6,
        older: OlderMail::Headers,
    })
}

fn msg(subject: &str, days_ago: i64) -> FMsg {
    FMsg {
        received_ms: now_ms() - days_ago * DAY_MS - 1000,
        subject: subject.into(),
        body_html: format!("<p>{subject}: details inside.</p>"),
        from: ("Cy Quill".into(), "cy@quill.example".into()),
        to: vec![("Sam Rivers".into(), ME.into())],
        is_read: true,
        ..FMsg::default()
    }
}

impl Harness {
    fn provider(&self) -> Arc<dyn MailProvider> {
        self.backend.client(&self.account).unwrap()
    }

    fn sync(&self) -> AccountSync {
        self.backend.account_sync(ME)
    }

    fn local(&self, id: &str) -> Option<penguin_core::Message> {
        self.store.get_message(ME, id).unwrap()
    }

    fn labels(&self, id: &str) -> Vec<String> {
        self.local(id).map(|m| m.label_ids).unwrap_or_default()
    }

    fn count(&self) -> u64 {
        self.store.count_messages(Some(ME)).unwrap()
    }
}

async fn synced(h: &Harness) -> AccountSync {
    let mut s = h.sync();
    s.init().await.unwrap();
    s.run_until_idle().await.unwrap();
    s
}

async fn poll(s: &mut AccountSync) {
    s.poke();
    s.run_until_idle().await.unwrap();
}

#[tokio::test]
async fn conformance_suite_passes_against_the_fake_graph() {
    let h = harness();
    let mut m = msg("Quarterly conformance report", 2);
    m.is_read = false;
    // The logo is inline because the body draws it.
    m.body_html
        .push_str(r#"<img src="cid:logo@quill.example" alt="Quill">"#);
    m.attachments = vec![
        FAtt {
            id: "ATT-pdf".into(),
            name: "report.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF-1.4 fake".to_vec(),
            ..FAtt::default()
        },
        FAtt {
            id: "ATT-logo".into(),
            name: "logo.png".into(),
            content_type: "image/png".into(),
            bytes: vec![137, 80, 78, 71],
            inline: true,
            content_id: Some("logo@quill.example".into()),
        },
    ];
    let id = h.fake.add("inbox", m);
    synced(&h).await;
    let local = h.local(&id).expect("synced");
    let attachment = local.attachments.iter().find(|a| !a.inline).cloned();
    assert!(attachment.is_some());
    let inline = local
        .attachments
        .iter()
        .find(|a| a.inline)
        .expect("inline image");
    assert_eq!(inline.content_id.as_deref(), Some("logo@quill.example"));
    let case = Case {
        message_id: id.clone(),
        thread_id: local.thread_id.clone(),
        attachment,
        from: Address {
            name: Some("Sam Rivers".into()),
            email: ME.into(),
        },
        send_to: Some(Address {
            name: None,
            email: "bea@quill.example".into(),
        }),
    };
    let p = h.provider();
    let failures = conformance::run(p.as_ref(), &h.store, &case).await;
    assert!(failures.is_empty(), "{failures:#?}");
    let s = h.fake.lock();
    assert_eq!(s.sent.len(), 2, "send_raw and send later each sent once");
    assert_eq!(
        s.missing_immutable, 0,
        "every Graph request prefers immutable ids"
    );
}

#[tokio::test]
async fn backfill_files_folders_flags_and_categories_and_honors_the_window() {
    let h = harness();
    let mut unread = msg("Unread flagged", 1);
    unread.is_read = false;
    unread.flagged = true;
    unread.categories = vec!["Red category".into(), "Travel plans".into()];
    let a = h.fake.add("inbox", unread);
    let old = h.fake.add("inbox", msg("From two years ago", 700));
    let sent = h.fake.add("sentitems", msg("My reply", 3));
    let archived = h.fake.add("archive", msg("Archived", 4));
    let receipt = h.fake.add("FLD-receipts", msg("Receipt", 5));
    let spam = h.fake.add("junkemail", msg("Spam", 6));
    let trash = h.fake.add("deleteditems", msg("Deleted", 7));
    let outbox = h.fake.add("outbox", msg("Queued", 0));
    let s = synced(&h).await;

    assert_eq!(h.count(), 7, "outbox isn't synced");
    assert!(h.local(&outbox).is_none());
    assert_eq!(
        h.labels(&a),
        vec![
            "INBOX",
            "UNREAD",
            "STARRED",
            "c:Red%20category",
            "c:Travel%20plans"
        ]
    );
    assert_eq!(h.labels(&sent), vec!["SENT"]);
    assert!(h.labels(&archived).is_empty());
    assert_eq!(h.labels(&receipt), vec!["f:FLD-receipts"]);
    assert_eq!(h.labels(&spam), vec!["SPAM"]);
    assert_eq!(h.labels(&trash), vec!["TRASH"]);
    for id in [&a, &old, &sent, &archived, &receipt, &spam, &trash] {
        let m = h.local(id).unwrap();
        let problems = conformance::check_message(AccountProvider::Microsoft, ME, &m);
        assert!(problems.is_empty(), "{problems:?}");
    }
    // Outside the 6-month window: headers only.
    assert_eq!(h.store.is_body_pending(ME, &old).unwrap(), Some(true));
    assert_eq!(h.store.is_body_pending(ME, &a).unwrap(), Some(false));
    assert!(h.local(&a).unwrap().body_text.contains("details inside"));

    let labels = h.store.list_labels(Some(ME)).unwrap();
    assert!(conformance::check_labels(ME, &labels).is_empty());
    let receipts = labels.iter().find(|l| l.id == "f:FLD-receipts").unwrap();
    assert_eq!(receipts.name, "Inbox/Receipts");
    assert!(labels
        .iter()
        .any(|l| l.id == "c:Travel%20plans" && l.name == "Travel plans"));
    assert!(labels.iter().any(|l| l.id == "f:FLD-clients"));

    let c = s.cursor();
    assert!(c.backfill_done);
    let since = c.window.full_since_ms.expect("window recorded");
    assert!(since > now_ms() - 200 * DAY_MS && since < now_ms() - 150 * DAY_MS);
    assert_eq!(c.window.older_mode.as_deref(), Some("headers"));
    let g = s.graph_cursor();
    for f in [
        "FLD-inbox",
        "FLD-sent",
        "FLD-drafts",
        "FLD-trash",
        "FLD-junk",
        "FLD-archive",
        "FLD-receipts",
        "FLD-clients",
    ] {
        assert!(g.folders[f].delta_link.is_some(), "{f}");
    }
    assert!(!g.folders.contains_key("FLD-outbox"));
    assert!(
        h.rec.added.lock().unwrap().is_empty(),
        "backfill isn't new mail"
    );
    let last = h.rec.statuses.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.phase, SyncPhase::Idle);
    assert_eq!(h.fake.lock().missing_immutable, 0);
    // The opened thread downloads the pending body on demand.
    let threads = h
        .provider()
        .fetch_pending_bodies(&[old.clone()])
        .await
        .unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(h.store.is_body_pending(ME, &old).unwrap(), Some(false));
}

#[tokio::test]
async fn backfill_resumes_after_a_restart_without_refetching() {
    let h = harness();
    for i in 0..230 {
        h.fake.add("inbox", msg(&format!("Newsletter {i}"), i % 60));
    }
    let mut s = h.sync();
    s.init().await.unwrap();
    // One delta page of the inbox (the first step is the empty poll).
    s.step().await.unwrap();
    s.step().await.unwrap();
    assert!(s.graph_cursor().folders["FLD-inbox"].next.is_some());
    assert_eq!(h.count(), 100);
    drop(s);

    let mut again = h.sync();
    again.init().await.unwrap();
    again.run_until_idle().await.unwrap();
    assert_eq!(h.count(), 230);
    // Bodies came a page at a time (one listing per delta page, by date),
    // and the restart didn't list the first page again.
    let requests = h.fake.lock().requests.clone();
    let one_by_one = requests
        .iter()
        .filter(|r| r.starts_with("GET /me/messages/") && r.matches('/').count() == 3)
        .count();
    let listings = requests
        .iter()
        .filter(|r| r.starts_with("GET /me/mailFolders/") && r.ends_with("/messages"))
        .count();
    assert_eq!(one_by_one, 0, "{requests:#?}");
    assert_eq!(listings, 3, "{requests:#?}");
}

#[tokio::test]
async fn incremental_sync_picks_up_adds_changes_moves_and_deletes() {
    let h = harness();
    let keep = h.fake.add("inbox", msg("Read me later", 1));
    let flag = h.fake.add("inbox", msg("Flag me", 2));
    let moved = h.fake.add("inbox", msg("Move me", 3));
    let deleted = h.fake.add("inbox", msg("Delete me", 4));
    let to_clients = h.fake.add("inbox", msg("File me", 5));
    let mut s = synced(&h).await;
    h.rec.changed.lock().unwrap().clear();

    let new = h.fake.add("inbox", msg("Brand new", 0));
    h.fake.with(|st| {
        st.update(&keep, |m| m.is_read = false);
        st.update(&flag, |m| {
            m.flagged = true;
            m.categories = vec!["Red category".into()];
        });
        st.move_message(&moved, "archive");
        st.delete(&deleted);
        st.move_message(&to_clients, "FLD-clients");
    });
    poll(&mut s).await;

    assert_eq!(
        h.rec.added.lock().unwrap().clone(),
        vec![new.clone()],
        "only new mail"
    );
    assert_eq!(h.labels(&new), vec!["INBOX"]);
    assert_eq!(h.labels(&keep), vec!["INBOX", "UNREAD"]);
    assert_eq!(
        h.labels(&flag),
        vec!["INBOX", "STARRED", "c:Red%20category"]
    );
    assert!(
        h.labels(&moved).is_empty(),
        "archived keeps its id, loses INBOX"
    );
    assert!(h.local(&deleted).is_none());
    assert_eq!(h.labels(&to_clients), vec!["f:FLD-clients"]);
    let la = h.rec.labels_added.lock().unwrap().clone();
    assert!(la
        .iter()
        .any(|(id, l)| *id == flag && l.contains(&"STARRED".to_string())));
    assert!(!h.rec.changed.lock().unwrap().is_empty());
    assert_eq!(h.count(), 5);
    // Moved messages were looked up, not re-downloaded.
    assert_eq!(h.fake.lock().missing_immutable, 0);
}

#[tokio::test]
async fn a_move_between_folders_keeps_the_message_whichever_folder_reports_first() {
    let h = harness();
    let a = h.fake.add("FLD-clients", msg("Client brief", 1));
    let b = h.fake.add("inbox", msg("Inbox note", 1));
    let mut s = synced(&h).await;
    // Clients → Inbox: the inbox (polled first) sees the add first.
    // Inbox → Clients: the inbox sees the removal first.
    h.fake.with(|st| {
        st.move_message(&a, "inbox");
        st.move_message(&b, "FLD-clients");
    });
    poll(&mut s).await;
    assert_eq!(h.labels(&a), vec!["INBOX"]);
    assert_eq!(h.labels(&b), vec!["f:FLD-clients"]);
    assert!(
        h.rec.added.lock().unwrap().is_empty(),
        "a move is not new mail"
    );
    assert_eq!(h.count(), 2);
}

#[tokio::test]
async fn an_expired_delta_link_relists_the_folder_without_duplicates_or_loss() {
    let h = harness();
    let stays = h.fake.add("inbox", msg("Stays", 1));
    let goes = h.fake.add("inbox", msg("Goes away", 2));
    let mut s = synced(&h).await;
    let new = h.fake.add("inbox", msg("Arrived meanwhile", 0));
    h.fake.with(|st| {
        st.delete(&goes);
        st.expire_delta_links("inbox");
    });
    poll(&mut s).await;
    assert!(h.local(&stays).is_some());
    assert!(h.local(&goes).is_none(), "deleted during the gap");
    assert!(h.local(&new).is_some());
    assert_eq!(h.count(), 2);
    assert!(s.graph_cursor().folders["FLD-inbox"].delta_link.is_some());
}

#[tokio::test]
async fn plain_delta_when_graph_refuses_ordering() {
    let h = harness();
    h.fake.lock().refuse_ordered_delta = true;
    let id = h.fake.add("inbox", msg("Hello", 1));
    let s = synced(&h).await;
    assert!(h.local(&id).is_some());
    assert!(s.graph_cursor().folders["FLD-inbox"].plain);
}

#[tokio::test]
async fn growing_the_window_downloads_bodies_and_turning_headers_on_lists_older_mail() {
    let h = harness_with(WindowPolicy {
        months: 1,
        older: OlderMail::None,
    });
    let recent = h.fake.add("inbox", msg("Recent", 5));
    let middle = h.fake.add("inbox", msg("Two months ago", 60));
    let ancient = h.fake.add("archive", msg("Ancient", 900));
    let mut s = synced(&h).await;
    assert!(h.local(&recent).is_some());
    assert!(
        h.local(&middle).is_none() && h.local(&ancient).is_none(),
        "older mail: none"
    );

    h.backend.set_window_policy(WindowPolicy {
        months: 6,
        older: OlderMail::Headers,
    });
    s.run_until_idle().await.unwrap();
    assert_eq!(
        h.store.is_body_pending(ME, &middle).unwrap(),
        Some(false),
        "now in the window"
    );
    assert_eq!(
        h.store.is_body_pending(ME, &ancient).unwrap(),
        Some(true),
        "headers only"
    );
    let w = &s.cursor().window;
    assert_eq!(w.older_mode.as_deref(), Some("headers"));
    assert!(w.full_since_ms.unwrap() < now_ms() - 150 * DAY_MS);

    h.backend.set_window_policy(WindowPolicy {
        months: 6,
        older: OlderMail::Full,
    });
    s.run_until_idle().await.unwrap();
    assert_eq!(h.store.is_body_pending(ME, &ancient).unwrap(), Some(false));
    assert_eq!(s.cursor().window.full_since_ms, Some(0));
    assert_eq!(h.count(), 3);
}

#[tokio::test]
async fn a_folder_deleted_into_deleted_items_becomes_trash() {
    let h = harness();
    let id = h.fake.add("FLD-clients", msg("Client brief", 1));
    let mut s = synced(&h).await;
    assert_eq!(h.labels(&id), vec!["f:FLD-clients"]);
    h.fake.with(|st| {
        st.move_folder("FLD-clients", "FLD-trash");
    });
    s.refresh_folders().await.unwrap();
    assert_eq!(h.labels(&id), vec!["TRASH"]);
}

#[tokio::test(start_paused = true)]
async fn throttling_waits_for_retry_after_then_surfaces_rate_limited() {
    let h = harness();
    let id = h.fake.add("inbox", msg("Hello", 1));
    h.fake.fail("/me/messages/", 429, Some(3), 2);
    let p = h.provider();
    let started = tokio::time::Instant::now();
    let got = p.fetch_messages(std::slice::from_ref(&id)).await;
    assert!(
        matches!(&got[0].1, Ok(Some(_))),
        "{:?}",
        got[0].1.as_ref().err()
    );
    assert!(
        started.elapsed() >= Duration::from_secs(6),
        "honored Retry-After twice"
    );

    h.fake.fail("/me/messages/", 503, None, 1);
    assert!(matches!(
        &p.fetch_messages(std::slice::from_ref(&id)).await[0].1,
        Ok(Some(_))
    ));

    h.fake.fail("/me/messages/", 429, Some(1), 50);
    let got = p.fetch_messages(std::slice::from_ref(&id)).await;
    assert!(
        matches!(&got[0].1, Err(Error::RateLimited)),
        "{:?}",
        got[0].1.as_ref().err()
    );
    // A send isn't retried on 503 (it may have gone out).
    h.fake.lock().failures.clear();
    h.fake.fail("/me/messages", 503, None, 1);
    let draft = Draft {
        to: vec![Address {
            name: None,
            email: "bea@quill.example".into(),
        }],
        subject: "Hi".into(),
        body_text: "Hello\n".into(),
        ..blank()
    };
    let raw = penguin_provider::compose::build_rfc822(
        &draft,
        &Address {
            name: None,
            email: ME.into(),
        },
        None,
        &[],
    )
    .unwrap();
    assert!(matches!(
        p.send_raw(&raw, None).await,
        Err(Error::Http { status: 503, .. })
    ));
}

#[tokio::test]
async fn a_revoked_refresh_token_stops_sync_with_needs_reauth() {
    let h = harness();
    h.fake.add("inbox", msg("Hello", 1));
    h.fake.with(|st| st.reject_refresh = true);
    let mut s = h.sync();
    assert!(matches!(s.init().await, Err(Error::NeedsReauth(_))));
    // Through the backend: the task ends in NeedsReauth and stays stopped.
    let handle = h.backend.start_sync(&h.account);
    for _ in 0..200 {
        if !handle.is_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!handle.is_running());
    let status = h.backend.sync_status(ME).unwrap();
    assert_eq!(status.phase, SyncPhase::NeedsReauth);
    assert!(status.error.unwrap().contains("AADSTS70008"));
}

#[tokio::test]
async fn a_token_graph_rejects_is_refreshed_once() {
    let h = harness();
    let id = h.fake.add("inbox", msg("Hello", 1));
    let p = h.provider();
    assert!(matches!(
        &p.fetch_messages(std::slice::from_ref(&id)).await[0].1,
        Ok(Some(_))
    ));
    let calls = h.fake.lock().token_calls;
    // The server forgets the token (revoked session): one refresh, then OK.
    h.fake.with(|st| st.access_tokens.clear());
    assert!(matches!(
        &p.fetch_messages(std::slice::from_ref(&id)).await[0].1,
        Ok(Some(_))
    ));
    assert_eq!(h.fake.lock().token_calls, calls + 1);
}

fn blank() -> Draft {
    Draft {
        request_read_receipt: None,
        account_id: ME.into(),
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

#[test]
fn read_receipt_request_maps_to_graph() {
    let body = |r: Option<bool>| {
        crate::drafts::draft_body(&Draft {
            request_read_receipt: r,
            ..blank()
        })["isReadReceiptRequested"]
            .clone()
    };
    assert_eq!(body(None), serde_json::json!(false));
    assert_eq!(body(Some(false)), serde_json::json!(false));
    assert_eq!(body(Some(true)), serde_json::json!(true));
}

#[tokio::test]
async fn label_deltas_become_graph_operations() {
    let h = harness();
    let a = h.fake.add("inbox", msg("Thread start", 2));
    let conv = h.fake.lock().messages[&a].conversation.clone();
    let mut reply = msg("Re: Thread start", 1);
    reply.conversation = conv.clone();
    reply.from = ("Sam Rivers".into(), ME.into());
    let r = h.fake.add("sentitems", reply);
    synced(&h).await;
    let p = h.provider();
    let server = |id: &str| h.fake.lock().messages[id].clone();

    // The app applies the change locally first (optimistic), then calls.
    let apply = |add: &[&str], remove: &[&str]| {
        let add: Vec<String> = add.iter().map(|s| s.to_string()).collect();
        let remove: Vec<String> = remove.iter().map(|s| s.to_string()).collect();
        h.store
            .modify_thread_labels(ME, &conv, &add, &remove)
            .unwrap();
        (add, remove)
    };
    let (add, remove) = apply(&["UNREAD", "STARRED", "c:Red%20category"], &[]);
    p.modify_thread(&conv, &add, &remove).await.unwrap();
    assert!(!server(&a).is_read && server(&a).flagged);
    assert_eq!(server(&a).categories, vec!["Red category"]);
    assert_eq!(server(&r).categories, vec!["Red category"]);

    let (add, remove) = apply(&[], &["c:Red%20category", "STARRED"]);
    p.modify_thread(&conv, &add, &remove).await.unwrap();
    assert!(server(&a).categories.is_empty() && !server(&a).flagged);

    // File into a folder: the inbox message moves; the sent one stays.
    let (add, remove) = apply(&["f:FLD-clients"], &["INBOX"]);
    p.modify_thread(&conv, &add, &remove).await.unwrap();
    assert_eq!(server(&a).folder, "FLD-clients");
    assert_eq!(server(&r).folder, "FLD-sent");
    assert_eq!(
        h.labels(&a)
            .iter()
            .filter(|l| l.as_str() == "INBOX")
            .count(),
        0
    );
    assert!(h.labels(&a).contains(&"f:FLD-clients".to_string()));

    // Removing the folder label archives.
    let (add, remove) = apply(&[], &["f:FLD-clients"]);
    p.modify_thread(&conv, &add, &remove).await.unwrap();
    assert_eq!(server(&a).folder, "FLD-archive");

    // Spam and back.
    let (add, remove) = apply(&["SPAM"], &[]);
    p.modify_thread(&conv, &add, &remove).await.unwrap();
    assert_eq!(server(&a).folder, "FLD-junk");
    assert_eq!(server(&r).folder, "FLD-sent");
    let (add, remove) = apply(&[], &["SPAM"]);
    p.modify_thread(&conv, &add, &remove).await.unwrap();
    assert_eq!(server(&a).folder, "FLD-inbox");

    // Trash takes the whole thread; untrash puts my own mail back in Sent.
    p.trash_thread(&conv).await.unwrap();
    assert_eq!(server(&a).folder, "FLD-trash");
    assert_eq!(server(&r).folder, "FLD-trash");
    assert!(
        h.labels(&r).contains(&"TRASH".to_string()) && !h.labels(&r).contains(&"SENT".to_string())
    );
    p.untrash_thread(&conv).await.unwrap();
    assert_eq!(server(&a).folder, "FLD-inbox");
    assert_eq!(server(&r).folder, "FLD-sent");

    // A folder that doesn't exist, a thread that doesn't: not found.
    assert!(p
        .modify_thread(&conv, &["f:FLD-gone".into()], &[])
        .await
        .unwrap_err()
        .is_not_found());
    assert!(p
        .modify_thread("CONV-none", &["STARRED".into()], &[])
        .await
        .unwrap_err()
        .is_not_found());
}

#[tokio::test]
async fn archive_is_created_when_the_mailbox_has_none() {
    let h = harness();
    h.fake.with(|st| st.remove_folder("FLD-archive"));
    let a = h.fake.add("inbox", msg("Archive me", 1));
    synced(&h).await;
    let conv = h.fake.lock().messages[&a].conversation.clone();
    h.provider()
        .modify_thread(&conv, &[], &["INBOX".into()])
        .await
        .unwrap();
    let folder = h.fake.lock().messages[&a].folder.clone();
    assert!(folder.starts_with("FLD-new"), "{folder}");
    assert!(h.labels(&a).is_empty(), "archived");
}

#[tokio::test]
async fn drafts_keep_their_id_sync_attachments_and_thread_replies() {
    let h = harness();
    let parent = h.fake.add("inbox", msg("Can you send the deck?", 1));
    synced(&h).await;
    let p = h.provider();
    let from = Address {
        name: Some("Sam Rivers".into()),
        email: ME.into(),
    };
    let small = AttachmentBytes {
        filename: "notes.txt".into(),
        mime_type: "text/plain".into(),
        bytes: b"notes".to_vec(),
        content_id: None,
    };
    let big = AttachmentBytes {
        filename: "deck.pdf".into(),
        mime_type: "application/pdf".into(),
        bytes: vec![7u8; 3 * 1024 * 1024 + 10],
        content_id: None,
    };
    let file = |a: &AttachmentBytes| OutgoingAttachment::File {
        filename: a.filename.clone(),
        mime_type: a.mime_type.clone(),
        data_base64: String::new(),
        content_id: None,
    };
    let mut draft = Draft {
        to: vec![Address {
            name: None,
            email: "cy@quill.example".into(),
        }],
        subject: "RE: Can you send the deck?".into(),
        body_text: "Here it is.\n".into(),
        reply_to_message_id: Some(parent.clone()),
        attachments: vec![file(&small), file(&big)],
        ..blank()
    };
    let saved = p
        .save_draft(&draft, &from, None, &[small.clone(), big.clone()])
        .await
        .unwrap();
    let parent_conv = h.fake.lock().messages[&parent].conversation.clone();
    assert_eq!(
        saved.thread_id, parent_conv,
        "createReply threads the draft"
    );
    assert_eq!(saved.message_id, saved.draft_id);
    assert_eq!(saved.attachments.len(), 2);
    {
        let s = h.fake.lock();
        let d = &s.messages[&saved.draft_id];
        assert_eq!(d.attachments.len(), 2);
        assert_eq!(
            d.attachments[1].bytes.len(),
            big.bytes.len(),
            "upload session"
        );
    }
    let local = h.local(&saved.draft_id).unwrap();
    assert_eq!(local.label_ids, vec!["DRAFT"]);
    assert_eq!(
        local.in_reply_to.as_deref(),
        h.local(&parent).unwrap().message_id_header.as_deref()
    );

    // Keep the first attachment (a ref to the draft), drop the big one.
    draft.attachments = vec![saved.attachments[0].clone()];
    draft.body_text = "Here are the notes.\n".into();
    let again = p
        .save_draft(&draft, &from, Some(&saved.draft_id), &[small.clone()])
        .await
        .unwrap();
    assert_eq!(again.draft_id, saved.draft_id, "stable draft id");
    let names: Vec<String> = h.fake.lock().messages[&saved.draft_id]
        .attachments
        .iter()
        .map(|a| a.name.clone())
        .collect();
    assert_eq!(names, vec!["notes.txt"]);
    let opened = p
        .open_draft(Some(&saved.draft_id), None)
        .await
        .unwrap()
        .unwrap();
    assert!(opened.draft.body_text.contains("notes"));
    assert_eq!(
        opened.draft.reply_to_message_id.as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(opened.draft.attachments.len(), 1);

    // Send: gone from Drafts (server and local), in Sent under a new id.
    let sent = p
        .send_draft(&draft, &from, &saved.draft_id, &[small])
        .await
        .unwrap();
    assert_eq!(sent.thread_id, parent_conv);
    assert!(h.local(&saved.draft_id).is_none());
    assert!(h
        .store
        .message_for_draft(ME, &saved.draft_id)
        .unwrap()
        .is_none());
    assert!(!h.fake.lock().messages.contains_key(&saved.draft_id));
    assert_eq!(h.fake.lock().sent.len(), 1);
    // The id no longer names a draft: deleting it must not touch the sent mail.
    p.delete_draft(&saved.draft_id).await.unwrap();
    assert_eq!(h.fake.lock().sent.len(), 1);
}

#[tokio::test]
async fn inline_images_upload_with_content_id_and_are_kept_across_saves() {
    let h = harness();
    synced(&h).await;
    let p = h.provider();
    let from = Address {
        name: None,
        email: ME.into(),
    };
    // One small image (inline in the request) and one over 3 MB (upload
    // session), both shown by the HTML, plus one the HTML doesn't show.
    let image = |name: &str, cid: &str, len: usize| AttachmentBytes {
        filename: name.into(),
        mime_type: "image/png".into(),
        bytes: vec![9u8; len],
        content_id: Some(cid.into()),
    };
    let small = image("small.png", "img-a@penguin", 10);
    let big = image("big.png", "img-b@penguin", 3 * 1024 * 1024 + 10);
    let spare = image("spare.png", "img-c@penguin", 20);
    let file = |a: &AttachmentBytes| OutgoingAttachment::File {
        filename: a.filename.clone(),
        mime_type: a.mime_type.clone(),
        data_base64: String::new(),
        content_id: a.content_id.clone(),
    };
    let mut draft = Draft {
        subject: "Screenshots".into(),
        body_text: "[image: small.png] [image: big.png]\n".into(),
        body_html: Some(
            r#"<p><img src="cid:img-a@penguin" alt="small.png"> <img src="cid:img-b@penguin" alt="big.png"> <img src="https://t.example/p.gif"></p>"#.into(),
        ),
        attachments: vec![file(&small), file(&big), file(&spare)],
        ..blank()
    };
    let saved = p
        .save_draft(
            &draft,
            &from,
            None,
            &[small.clone(), big.clone(), spare.clone()],
        )
        .await
        .unwrap();
    {
        let s = h.fake.lock();
        let d = &s.messages[&saved.draft_id];
        let got: Vec<(String, bool, Option<String>)> = d
            .attachments
            .iter()
            .map(|a| (a.name.clone(), a.inline, a.content_id.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("small.png".into(), true, Some("img-a@penguin".into())),
                ("big.png".into(), true, Some("img-b@penguin".into())),
                // Not shown: an ordinary attachment.
                ("spare.png".into(), false, None),
            ]
        );
        assert!(
            d.body_html.contains(r#"src="cid:img-a@penguin""#),
            "{}",
            d.body_html
        );
        assert!(!d.body_html.contains("t.example"), "{}", d.body_html);
    }
    let cids: Vec<Option<&str>> = saved.attachments.iter().map(|a| a.content_id()).collect();
    assert_eq!(
        cids,
        vec![Some("img-a@penguin"), Some("img-b@penguin"), None]
    );

    // Reopen: both images come back as refs with their ids; the HTML shows them.
    let opened = p
        .open_draft(Some(&saved.draft_id), None)
        .await
        .unwrap()
        .unwrap();
    let html = opened.draft.body_html.clone().unwrap();
    assert!(
        html.contains(r#"<img src="cid:img-b@penguin" alt="big.png">"#),
        "{html}"
    );
    let ids: Vec<Option<&str>> = opened
        .draft
        .attachments
        .iter()
        .map(|a| a.content_id())
        .collect();
    assert_eq!(
        ids,
        vec![Some("img-a@penguin"), Some("img-b@penguin"), None]
    );

    // Save again with the refs, the spare one dropped: nothing re-uploaded.
    let before: Vec<String> = h.fake.lock().messages[&saved.draft_id]
        .attachments
        .iter()
        .map(|a| a.id.clone())
        .collect();
    draft.attachments = opened.draft.attachments[..2].to_vec();
    let again = p
        .save_draft(&draft, &from, Some(&saved.draft_id), &[small, big])
        .await
        .unwrap();
    let after: Vec<String> = h.fake.lock().messages[&saved.draft_id]
        .attachments
        .iter()
        .map(|a| a.id.clone())
        .collect();
    assert_eq!(after, before[..2].to_vec());
    assert_eq!(again.attachments.len(), 2);
}

#[tokio::test]
async fn details_source_attachments_search_estimates_and_photo() {
    let h = harness();
    let mut m = msg("Invoice 7 from Quill", 1);
    m.headers = vec![
        ("Received".into(), "from mx.quill.example".into()),
        (
            "List-Unsubscribe".into(),
            "<https://quill.example/u>".into(),
        ),
    ];
    m.attachments = vec![FAtt {
        id: "ATT-x".into(),
        name: "invoice.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: b"%PDF invoice".to_vec(),
        ..FAtt::default()
    }];
    let id = h.fake.add("inbox", m);
    let old = h.fake.add("archive", msg("Old invoice 3", 400));
    let p = h.provider();
    let meta = p.get_message_metadata(&id).await.unwrap().unwrap();
    assert_eq!(meta.headers[0].0, "Received");
    assert!(meta.size_estimate.unwrap() > 2048);
    assert!(p
        .get_message_metadata("AAMkADfake999999-Aa_=")
        .await
        .unwrap()
        .is_none());
    let raw = p.get_message_raw(&id).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("Subject: Invoice 7 from Quill"));
    let full = p
        .fetch_messages(std::slice::from_ref(&id))
        .await
        .remove(0)
        .1
        .unwrap()
        .unwrap();
    assert_eq!(
        full.list_unsubscribe.as_deref(),
        Some("<https://quill.example/u>")
    );
    assert_eq!(full.list_unsubscribe_post, Some(false));
    let bytes = p.get_attachment(&id, &full.attachments[0]).await.unwrap();
    assert_eq!(bytes, b"%PDF invoice");
    let mut gone = full.attachments[0].clone();
    gone.id = "ATT-nope".into();
    assert!(p
        .get_attachment(&id, &gone)
        .await
        .unwrap_err()
        .is_not_found());

    // Server search stores unknown matches headers-only and returns hits.
    let q = penguin_core::query::parse("invoice", now_ms());
    let found = p.server_search(&q, 50).await.unwrap();
    assert_eq!(found.hits.len(), 2);
    assert_eq!(found.fetched, 2);
    assert_eq!(found.hits[0].message_id, id, "newest first");
    assert_eq!(h.store.is_body_pending(ME, &old).unwrap(), Some(true));
    assert!(p
        .server_search(&penguin_core::query::parse("is:unread", 0), 50)
        .await
        .unwrap()
        .hits
        .is_empty());

    assert_eq!(p.window_estimate(0, now_ms()).await.unwrap(), 2);
    assert_eq!(p.window_estimate(6, now_ms()).await.unwrap(), 1);
    assert_eq!(p.profile_photo().await.unwrap(), None);
    h.fake.lock().photo = Some(vec![1, 2, 3]);
    assert_eq!(p.profile_photo().await.unwrap(), Some(vec![1, 2, 3]));
}

#[test]
fn label_diffs_only_touch_what_the_change_reports() {
    let local: Vec<String> = ["INBOX", "UNREAD", "STARRED", "c:Red"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    // A partial entry (no isRead, no flag, no categories) moved to archive.
    let partial = crate::wire::WireMessage::default();
    let (add, remove) = label_diff(&local, &partial, &[]);
    assert!(add.is_empty());
    assert_eq!(remove, vec!["INBOX"]);
    let full = crate::wire::WireMessage {
        is_read: Some(true),
        flag: Some(crate::wire::Flag {
            flag_status: "notFlagged".into(),
        }),
        categories: Some(vec![]),
        ..Default::default()
    };
    let (add, remove) = label_diff(&local, &full, &["f:X".to_string()]);
    assert_eq!(add, vec!["f:X"]);
    assert_eq!(remove, vec!["INBOX", "UNREAD", "STARRED", "c:Red"]);
}

#[tokio::test]
async fn invitations_are_answered_on_the_linked_event() {
    let h = harness();
    let invite = h.fake.add("inbox", msg("Invitation: Plan review", 1));
    let plain = h.fake.add("inbox", msg("Lunch?", 1));
    h.fake
        .lock()
        .meeting_events
        .insert(invite.clone(), "EV-plan-review".into());
    let p = h.provider();
    let answer = penguin_provider::InvitationAnswer {
        response: "declined".into(),
        comment: Some("Out that week".into()),
        proposal: Some((1_790_000_000_000, 1_790_003_600_000)),
    };
    p.respond_to_invitation(&invite, &answer).await.unwrap();
    {
        let s = h.fake.lock();
        let (ev, action, body) = s.meeting_actions.last().unwrap().clone();
        assert_eq!(ev, "EV-plan-review");
        assert_eq!(action, "decline");
        assert_eq!(body["comment"], "Out that week");
        assert_eq!(body["sendResponse"], true);
        assert_eq!(body["proposedNewTime"]["start"]["timeZone"], "UTC");
    }
    // A message that isn't an invitation: nothing to answer.
    let err = p.respond_to_invitation(&plain, &answer).await.unwrap_err();
    assert!(err.is_not_found(), "{err:?}");
    assert_eq!(h.fake.lock().meeting_actions.len(), 1);
}

/// A mailbox with inline images, attachments, categories and headers.
fn rich_mailbox(h: &Harness) -> Vec<String> {
    (0..40)
        .map(|i| {
            let mut m = msg(&format!("Weekly numbers {i}"), i);
            // Distinct seconds, as real receivedDateTimes are.
            m.received_ms -= i * 7_000;
            m.is_read = i % 3 != 0;
            m.flagged = i % 5 == 0;
            m.categories = if i % 4 == 0 {
                vec!["Red category".into()]
            } else {
                vec![]
            };
            m.headers = vec![(
                "List-Unsubscribe".into(),
                format!("<mailto:leave{i}@quill.example>"),
            )];
            m.body_html = format!("<p>Numbers {i}</p><img src=\"cid:chart{i}@quill.example\">");
            m.attachments = vec![
                FAtt {
                    id: format!("ATT-chart{i}"),
                    name: "chart.png".into(),
                    content_type: "image/png".into(),
                    bytes: vec![137, 80, 78, 71, i as u8],
                    inline: true,
                    content_id: Some(format!("chart{i}@quill.example")),
                },
                FAtt {
                    id: format!("ATT-sheet{i}"),
                    name: "numbers.xlsx".into(),
                    content_type: "application/vnd.ms-excel".into(),
                    bytes: vec![1; 300 + i as usize],
                    ..FAtt::default()
                },
            ];
            h.fake.add("inbox", m)
        })
        .collect()
}

#[tokio::test]
async fn backfill_lists_a_pages_bodies_in_one_request_and_stores_the_same_messages() {
    // Ordered delta: bodies by date range. Plain delta: one GET each (the
    // reference). Same mailbox (same ids), so the stored messages must match.
    let ranged = harness();
    let one_by_one = harness();
    one_by_one.fake.lock().refuse_ordered_delta = true;
    let ids = rich_mailbox(&ranged);
    assert_eq!(rich_mailbox(&one_by_one), ids);
    synced(&ranged).await;
    synced(&one_by_one).await;
    for id in &ids {
        let a = ranged.local(id).expect("ranged stored it");
        let b = one_by_one.local(id).expect("one by one stored it");
        assert_eq!(a, b, "{id}");
        assert!(a.body_html.as_deref().unwrap().contains("Numbers"));
        assert_eq!(
            a.attachments
                .iter()
                .find(|x| x.inline)
                .and_then(|x| x.content_id.clone()),
            Some(format!(
                "chart{}@quill.example",
                ids.iter().position(|i| i == id).unwrap()
            ))
        );
    }
    let count = |h: &Harness, f: &dyn Fn(&str) -> bool| {
        h.fake.lock().requests.iter().filter(|r| f(r)).count()
    };
    let gets = |r: &str| r.starts_with("GET /me/messages/") && r.matches('/').count() == 3;
    let lists = |r: &str| r.starts_with("GET /me/mailFolders/") && r.ends_with("/messages");
    assert_eq!(count(&ranged, &gets), 0);
    assert_eq!(count(&ranged, &lists), 1);
    assert_eq!(count(&one_by_one, &gets), 40);
    assert_eq!(count(&one_by_one, &lists), 0);
}

#[tokio::test]
async fn a_range_listing_that_misses_messages_falls_back_to_one_get_each() {
    let h = harness();
    let ids = rich_mailbox(&h);
    // The listing is refused once: every body still arrives, one by one.
    h.fake
        .fail("/me/mailFolders/FLD-inbox/messages$", 400, None, 1);
    synced(&h).await;
    for id in &ids {
        assert!(h.local(id).is_some(), "{id}");
    }
    assert_eq!(h.count(), 40);
}
