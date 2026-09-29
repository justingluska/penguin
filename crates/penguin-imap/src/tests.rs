//! End-to-end tests against the scripted in-process server (testserver.rs):
//! the real session, SMTP, sync engine and provider code over TCP.
//!
//! The same conformance suite also runs against real servers in Docker
//! (`real_server_conformance`, ignored unless PENGUIN_IMAP_TEST_* is set;
//! see docs/PROVIDERS-IMPL.md → IMAP).

use std::sync::Arc;
use std::time::Duration;

use penguin_core::{Account, AccountProvider, Address, AuthMethod, ProviderConfig, Store};
use penguin_provider::conformance::{self, Case};
use penguin_provider::credentials::{MemorySecrets, PasswordCredential, SecretVault};
use penguin_provider::fake::RecordingObserver;
use penguin_provider::window::{OlderMail, WindowPolicy};
use penguin_provider::{Backend, MailProvider};

use crate::account::Ctx;
use crate::mime;
use crate::sync::{AccountSync, Shared};
use crate::testserver::{TestServer, BASIC_CAPS, GENERIC_CAPS, PASS, USER};
use crate::{ImapBackend, ImapProvider};

const DAY: i64 = 86_400_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

pub fn raw(msgid: &str, subject: &str, refs: &[&str], body: &str, attachment: bool) -> Vec<u8> {
    let mut h = format!(
        "From: Bea Lark <bea@lark.example>\r\nTo: Sam Okafor <{USER}>\r\nSubject: {subject}\r\nMessage-ID: <{msgid}>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\nMIME-Version: 1.0\r\n"
    );
    if let Some(last) = refs.last() {
        h.push_str(&format!("In-Reply-To: <{last}>\r\n"));
        h.push_str(&format!(
            "References: {}\r\n",
            refs.iter()
                .map(|r| format!("<{r}>"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    if attachment {
        h.push_str("Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n");
        h.push_str(body);
        h.push_str("\r\n--b1\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQKJcTl8uXr\r\n--b1--\r\n");
    } else {
        h.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
        h.push_str(body);
        h.push_str("\r\n");
    }
    h.into_bytes()
}

pub struct Env {
    pub server: TestServer,
    pub store: Store,
    pub observer: Arc<RecordingObserver>,
    pub backend: ImapBackend,
    pub account: Account,
    pub ctx: Arc<Ctx>,
}

impl Env {
    pub async fn new(caps: &[&str], gmail: bool, host: &str) -> Env {
        let server = TestServer::start(caps, gmail).await;
        let store = Store::open_in_memory().unwrap();
        let account = Account {
            id: USER.into(),
            email: USER.into(),
            provider: AccountProvider::Imap,
            provider_config: ProviderConfig {
                auth: Some(AuthMethod::AppPassword),
                imap: Some(server.imap_settings()),
                smtp: Some(server.smtp_settings()),
                host: Some(host.into()),
                ..ProviderConfig::default()
            },
            ..Account::default()
        };
        store.upsert_account(&account).unwrap();
        let vault: Arc<SecretVault<PasswordCredential>> = Arc::new(SecretVault::new(Box::new(
            Arc::new(MemorySecrets::default()),
        )));
        vault
            .save(
                USER,
                &PasswordCredential {
                    password: PASS.into(),
                    smtp_password: None,
                    saved_at: 1,
                },
            )
            .unwrap();
        let observer = Arc::new(RecordingObserver::default());
        let backend = ImapBackend::with_vault(store.clone(), observer.clone(), vault, false);
        let ctx = backend.ctx(&account).unwrap();
        Env {
            server,
            store,
            observer,
            backend,
            account,
            ctx,
        }
    }

    pub fn provider(&self) -> ImapProvider {
        ImapProvider::new(self.ctx.clone())
    }

    pub fn engine(&self, policy: WindowPolicy) -> AccountSync {
        let shared = Shared::new(USER, self.observer.clone(), policy);
        AccountSync::new(self.ctx.clone(), shared)
    }

    /// Init + fill + older passes to completion.
    pub async fn sync(&self, policy: WindowPolicy) -> AccountSync {
        let mut e = self.engine(policy);
        e.init().await.unwrap();
        while e.fill_step().await.unwrap() {}
        while e.older_step().await.unwrap() {}
        e
    }

    pub fn count(&self) -> u64 {
        self.store.count_messages(Some(USER)).unwrap()
    }

    /// The stored message with this Message-ID header.
    pub fn by_header(&self, msgid: &str) -> Option<penguin_core::Message> {
        let located = crate::db::locations(&self.store, USER, &self.all_ids()).unwrap();
        located
            .keys()
            .filter_map(|id| self.store.get_message(USER, id).unwrap())
            .find(|m| m.message_id_header.as_deref() == Some(msgid))
            .or_else(|| {
                self.all_ids()
                    .into_iter()
                    .filter_map(|id| self.store.get_message(USER, &id).unwrap())
                    .find(|m| m.message_id_header.as_deref() == Some(msgid))
            })
    }

    pub fn all_ids(&self) -> Vec<String> {
        self.store
            .provider_read(|c| {
                let mut stmt = c.prepare("SELECT id FROM messages WHERE account_id = ?1")?;
                let ids = stmt
                    .query_map([USER], |r| r.get::<_, String>(0))?
                    .collect::<penguin_core::rusqlite::Result<Vec<_>>>()?;
                Ok(ids)
            })
            .unwrap()
    }

    pub fn added(&self) -> Vec<String> {
        self.observer
            .added
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(_, ids)| ids.clone())
            .collect()
    }
}

async fn run_conformance(caps: &[&str], gmail: bool, host: &str) {
    let env = Env::new(caps, gmail, host).await;
    // Gmail files the Sent copy itself, as the real one does.
    env.server.state.lock().unwrap().smtp_saves_sent = gmail;
    env.server.deliver(
        "INBOX",
        &raw(
            "q1@lark.example",
            "Quarterly conformance report",
            &[],
            "numbers inside",
            true,
        ),
        &[],
        now_ms() - DAY,
    );
    env.sync(WindowPolicy::EVERYTHING).await;
    let m = env.by_header("q1@lark.example").expect("synced");
    assert!(
        penguin_provider::conformance::check_message(AccountProvider::Imap, USER, &m).is_empty()
    );
    let att = m.attachments.first().cloned().expect("attachment parsed");
    assert_eq!(att.filename, "report.pdf");
    let p = env.provider();
    let case = Case {
        message_id: m.id.clone(),
        thread_id: m.thread_id.clone(),
        attachment: Some(att.clone()),
        from: Address {
            name: Some("Sam Okafor".into()),
            email: USER.into(),
        },
        send_to: Some(Address {
            name: None,
            email: "bea@lark.example".into(),
        }),
    };
    let failures = conformance::run(&p, &env.store, &case).await;
    assert!(failures.is_empty(), "{failures:#?}");
    // Sync never marks mail read behind the user's back.
    {
        let st = env.server.state.lock().unwrap();
        assert!(
            st.non_peek.is_empty(),
            "non-PEEK fetches: {:?}",
            st.non_peek
        );
        // Two sends (send_raw + send_saved_draft), each exactly once.
        assert_eq!(st.smtp_sent.len(), 2);
    }
    // The attachment bytes are the PDF's.
    let bytes = p.get_attachment(&m.id, &att).await.unwrap();
    assert!(bytes.starts_with(b"%PDF-1.4"), "{bytes:?}");
}

#[tokio::test]
async fn conformance_on_a_full_featured_server() {
    run_conformance(GENERIC_CAPS, false, "fastmail").await;
}

#[tokio::test]
async fn conformance_on_a_minimal_server() {
    // No MOVE, UIDPLUS, CONDSTORE, QRESYNC, OBJECTID, LITERAL+, AUTH=PLAIN.
    run_conformance(BASIC_CAPS, false, "imapGeneric").await;
}

#[tokio::test]
async fn conformance_on_gmail_quick_setup() {
    run_conformance(
        &[
            "IMAP4rev1",
            "UIDPLUS",
            "MOVE",
            "IDLE",
            "CONDSTORE",
            "SASL-IR",
            "AUTH=PLAIN",
        ],
        true,
        "gmail",
    )
    .await;
}

fn with_compress(caps: &[&'static str]) -> Vec<&'static str> {
    caps.iter().copied().chain(["COMPRESS=DEFLATE"]).collect()
}

#[tokio::test]
async fn conformance_over_compress_deflate() {
    // RFC 4978: after login every command and response goes through DEFLATE.
    run_conformance(&with_compress(GENERIC_CAPS), false, "fastmail").await;
    run_conformance(
        &with_compress(&[
            "IMAP4rev1",
            "UIDPLUS",
            "MOVE",
            "IDLE",
            "CONDSTORE",
            "SASL-IR",
            "AUTH=PLAIN",
        ]),
        true,
        "gmail",
    )
    .await;
}

#[tokio::test]
async fn compress_deflate_is_used_when_offered_and_shrinks_the_wire() {
    let env = Env::new(&with_compress(GENERIC_CAPS), false, "fastmail").await;
    let para = "The quarterly numbers are in and the team would like to go over them on Thursday. ";
    for i in 0..40 {
        env.server.deliver(
            "INBOX",
            &raw(
                &format!("z{i}@lark.example"),
                &format!("Numbers {i}"),
                &[],
                &para.repeat(40),
                false,
            ),
            &["\\Seen"],
            now_ms() - (40 - i) * 3_600_000,
        );
    }
    let s = env.ctx.open_session().await.unwrap();
    assert!(s.compressed);
    drop(s);
    env.sync(WindowPolicy::EVERYTHING).await;
    assert_eq!(env.count(), 40);
    let m = env.by_header("z7@lark.example").unwrap();
    assert!(m.body_text.contains("quarterly numbers"));
    let st = env.server.state.lock().unwrap();
    assert!(st.commands.iter().any(|c| c == "COMPRESS"));
    let wire = st.wire_out.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        wire * 4 < st.bytes_out,
        "wire {wire} B vs {} B of replies",
        st.bytes_out
    );
}

#[tokio::test]
async fn a_server_without_compress_is_left_alone() {
    let env = Env::new(GENERIC_CAPS, false, "fastmail").await;
    let s = env.ctx.open_session().await.unwrap();
    assert!(!s.compressed);
    assert!(!env
        .server
        .state
        .lock()
        .unwrap()
        .commands
        .iter()
        .any(|c| c == "COMPRESS"));
}

#[tokio::test]
async fn connect_checks_both_servers_and_explains_refusals() {
    let env = Env::new(GENERIC_CAPS, false, "yahoo").await;
    let cfg = env.ctx.cfg.clone();
    assert!(!crate::connect::verify(&cfg, PASS).await.unwrap());
    let e = crate::connect::verify(&cfg, "my-yahoo-password")
        .await
        .unwrap_err();
    match e {
        penguin_provider::Error::InvalidInput(m) => {
            assert!(m.starts_with("Yahoo didn't accept that password"), "{m}");
            assert!(m.contains("Invalid credentials"), "{m}");
            assert!(!m.contains("my-yahoo-password"));
        }
        other => panic!("{other:?}"),
    }
    let mut unreachable = cfg.clone();
    unreachable.imap.port = 1;
    assert!(matches!(
        crate::connect::verify(&unreachable, PASS)
            .await
            .unwrap_err(),
        penguin_provider::Error::Network(_)
    ));
    // Unencrypted is refused for anything but this machine.
    let mut remote = cfg.clone();
    remote.imap.host = "imap.harbor.example".into();
    assert!(matches!(
        crate::connect::verify(&remote, PASS).await.unwrap_err(),
        penguin_provider::Error::InvalidInput(_)
    ));
    let gmail = Env::new(&["IMAP4rev1", "AUTH=PLAIN"], true, "gmail").await;
    assert!(crate::connect::verify(&gmail.ctx.cfg, PASS).await.unwrap());
}

#[tokio::test]
async fn backfill_resumes_after_a_restart_without_duplicates() {
    let env = Env::new(GENERIC_CAPS, false, "fastmail").await;
    // Delivered oldest first, as mail arrives: m119 is the newest.
    for i in 0..120 {
        env.server.deliver(
            "INBOX",
            &raw(
                &format!("m{i}@lark.example"),
                &format!("Note {i}"),
                &[],
                "hi",
                false,
            ),
            &["\\Seen"],
            now_ms() - (120 - i) * 3_600_000,
        );
    }
    let mut first = env.engine(WindowPolicy::EVERYTHING);
    first.init().await.unwrap();
    assert!(first.fill_step().await.unwrap());
    assert_eq!(env.count(), crate::sync::FILL_CHUNK as u64);
    // Newest first: the first chunk holds the newest mail.
    assert!(env.by_header("m119@lark.example").is_some());
    assert!(env.by_header("m0@lark.example").is_none());
    drop(first);
    // "Restart": a new engine resumes from the saved cursor.
    let e = env.sync(WindowPolicy::EVERYTHING).await;
    drop(e);
    assert_eq!(env.count(), 120);
    let cursor = env.store.get_sync_cursor(USER).unwrap();
    assert!(cursor.backfill_done);
    assert_eq!(cursor.window.full_since_ms, Some(0));
    assert!(env.added().is_empty(), "backfill is not new mail");
    assert!(env.by_header("m0@lark.example").is_some());
}

#[tokio::test]
async fn incremental_sync_sees_new_mail_flags_moves_and_deletions() {
    for caps in [GENERIC_CAPS, BASIC_CAPS] {
        let env = Env::new(caps, false, "imapGeneric").await;
        let old = env.server.deliver(
            "INBOX",
            &raw("a@lark.example", "Plans", &[], "first", false),
            &[],
            now_ms() - DAY,
        );
        let mut e = env.sync(WindowPolicy::EVERYTHING).await;
        let a = env.by_header("a@lark.example").unwrap();
        assert_eq!(a.label_ids, vec!["INBOX", "UNREAD"]);

        // New mail (a reply): stored, reported as added, threaded.
        env.server.deliver(
            "INBOX",
            &raw(
                "b@lark.example",
                "Re: Plans",
                &["a@lark.example"],
                "second",
                false,
            ),
            &[],
            now_ms(),
        );
        e.poll(false).await.unwrap();
        let b = env.by_header("b@lark.example").unwrap();
        assert_eq!(b.thread_id, a.thread_id, "reply joins the thread");
        assert_eq!(env.added(), vec![b.id.clone()], "caps {caps:?}");

        // Flags changed elsewhere.
        env.server.set_flags("INBOX", old, &["\\Seen", "\\Flagged"]);
        e.poll(false).await.unwrap();
        assert_eq!(
            env.store
                .get_message(USER, &a.id)
                .unwrap()
                .unwrap()
                .label_ids,
            vec!["INBOX", "STARRED"]
        );

        // Moved to a folder by another client: same id, new label, not "new".
        env.server.move_message("INBOX", old, "Receipts");
        e.poll(false).await.unwrap();
        let moved = env
            .store
            .get_message(USER, &a.id)
            .unwrap()
            .expect("a move is not a deletion");
        assert_eq!(moved.label_ids, vec!["STARRED", "f:Receipts"]);
        assert_eq!(env.added().len(), 1, "a move is not new mail");
        assert_eq!(env.count(), 2);

        // Deleted elsewhere.
        let uid = env.server.messages("Receipts")[0].uid;
        env.server.expunge("Receipts", uid);
        e.poll(true).await.unwrap();
        assert!(
            env.store.get_message(USER, &a.id).unwrap().is_none(),
            "caps {caps:?}"
        );
        assert_eq!(env.count(), 1);
    }
}

#[tokio::test]
async fn a_uidvalidity_reset_remaps_without_duplicates_or_new_mail() {
    for caps in [GENERIC_CAPS, BASIC_CAPS] {
        let env = Env::new(caps, false, "imapGeneric").await;
        for i in 0..5 {
            env.server.deliver(
                "INBOX",
                &raw(&format!("r{i}@lark.example"), "Hello", &[], "x", false),
                &["\\Seen"],
                now_ms() - DAY,
            );
        }
        let mut e = env.sync(WindowPolicy::EVERYTHING).await;
        let before: std::collections::BTreeSet<String> = env.all_ids().into_iter().collect();
        env.server.reset_uidvalidity("INBOX");
        e.poll(true).await.unwrap();
        while e.fill_step().await.unwrap() {}
        let after: std::collections::BTreeSet<String> = env.all_ids().into_iter().collect();
        assert_eq!(before, after, "same messages, same ids");
        assert!(env.added().is_empty());
        // Locations point at the new UIDs: reading still works.
        let p = env.provider();
        let id = after.iter().next().unwrap().clone();
        assert!(p.get_message_raw(&id).await.unwrap().is_some());
        let locs = crate::db::folder_locations(&env.store, USER, "INBOX").unwrap();
        assert_eq!(locs.len(), 5);
        assert!(locs.values().all(|l| l.uidvalidity >= 1100));
    }
}

#[tokio::test]
async fn older_mail_is_headers_only_until_opened() {
    let env = Env::new(GENERIC_CAPS, false, "fastmail").await;
    env.server.deliver(
        "INBOX",
        &raw("new@lark.example", "Recent", &[], "recent body", false),
        &[],
        now_ms() - DAY,
    );
    env.server.deliver(
        "INBOX",
        &raw(
            "old@lark.example",
            "Ancient",
            &[],
            "ancient body text",
            true,
        ),
        &[],
        now_ms() - 400 * DAY,
    );
    let policy = WindowPolicy {
        months: 6,
        older: OlderMail::Headers,
    };
    env.sync(policy).await;
    let recent = env.by_header("new@lark.example").unwrap();
    let old = env.by_header("old@lark.example").unwrap();
    assert_eq!(
        env.store.is_body_pending(USER, &recent.id).unwrap(),
        Some(false)
    );
    assert_eq!(
        env.store.is_body_pending(USER, &old.id).unwrap(),
        Some(true)
    );
    assert_eq!(
        old.attachments.len(),
        1,
        "attachments listed from BODYSTRUCTURE"
    );
    let c = env.store.get_sync_cursor(USER).unwrap();
    assert!(c.backfill_done && c.window.older_done);
    assert_eq!(c.window.older_mode.as_deref(), Some("headers"));
    let threads = env
        .provider()
        .fetch_pending_bodies(std::slice::from_ref(&old.id))
        .await
        .unwrap();
    assert_eq!(threads, vec![old.thread_id.clone()]);
    let full = env.store.get_message(USER, &old.id).unwrap().unwrap();
    assert!(full.body_text.contains("ancient body text"));
    assert_eq!(
        env.store.is_body_pending(USER, &old.id).unwrap(),
        Some(false)
    );
}

/// Spam syncs back 30 days only, even with "everything in full": older
/// spam is never downloaded, not even as headers, while old mail in other
/// folders is. New spam arriving later syncs as usual.
#[tokio::test]
async fn junk_syncs_only_the_spam_window() {
    let policies = [
        WindowPolicy::EVERYTHING,
        WindowPolicy {
            months: 6,
            older: OlderMail::Headers,
        },
    ];
    for policy in policies {
        let env = Env::new(GENERIC_CAPS, false, "fastmail").await;
        let deliver = |folder: &str, id: &str, days: i64| {
            env.server.deliver(
                folder,
                &raw(id, "Prize", &[], "claim your prize", false),
                &[],
                now_ms() - days * DAY,
            );
        };
        deliver("Junk", "fresh-spam@prize.example", 3);
        deliver("Junk", "stale-spam@prize.example", 45);
        deliver("INBOX", "old-mail@lark.example", 400);
        let mut e = env.sync(policy).await;
        let fresh = env.by_header("fresh-spam@prize.example").unwrap();
        assert!(fresh.label_ids.iter().any(|l| l == "SPAM"));
        assert!(
            env.by_header("stale-spam@prize.example").is_none(),
            "{policy:?}"
        );
        assert!(
            env.by_header("old-mail@lark.example").is_some(),
            "{policy:?}"
        );
        let c = env.store.get_sync_cursor(USER).unwrap();
        assert!(c.backfill_done, "{policy:?}");
        // With a window, the older pass finished without Junk.
        assert!(policy.months == 0 || c.window.older_done, "{policy:?}");
        // Spam arriving later syncs like any new mail.
        deliver("Junk", "new-spam@prize.example", 0);
        e.poll(true).await.unwrap();
        assert!(env.by_header("new-spam@prize.example").is_some());
    }
}

#[tokio::test]
async fn sent_copies_are_filed_exactly_once() {
    for saves in [false, true] {
        let env = Env::new(
            GENERIC_CAPS,
            false,
            if saves { "icloud" } else { "fastmail" },
        )
        .await;
        env.server.state.lock().unwrap().smtp_saves_sent = saves;
        env.sync(WindowPolicy::EVERYTHING).await;
        let p = env.provider();
        let msg = b"From: Sam <sam@mail.example>\r\nTo: bea@lark.example\r\nBcc: cy@lark.example\r\nSubject: Hi\r\nMessage-ID: <sent1@mail.example>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\n\r\nhello\r\n";
        let sent = p.send_raw(msg, None).await.unwrap();
        let copies = env.server.messages("Sent");
        assert_eq!(copies.len(), 1, "saves={saves}");
        let st = env.server.state.lock().unwrap();
        let (_, rcpts, data) = st.smtp_sent.last().unwrap();
        assert_eq!(
            rcpts,
            &vec![
                "bea@lark.example".to_string(),
                "cy@lark.example".to_string()
            ]
        );
        assert!(
            !String::from_utf8_lossy(data).contains("cy@lark.example"),
            "Bcc stripped on the wire"
        );
        drop(st);
        let stored = env
            .store
            .get_message(USER, &sent.message_id)
            .unwrap()
            .expect("sent copy stored at once");
        assert_eq!(stored.label_ids, vec!["SENT"]);
    }
}

#[tokio::test]
async fn gmail_mode_uses_labels_thread_ids_and_all_mail() {
    let env = Env::new(
        &[
            "IMAP4rev1",
            "UIDPLUS",
            "MOVE",
            "CONDSTORE",
            "AUTH=PLAIN",
            "SASL-IR",
        ],
        true,
        "gmail",
    )
    .await;
    env.server.deliver(
        "INBOX",
        &raw("g1@lark.example", "Hello", &[], "x", false),
        &[],
        now_ms() - DAY,
    );
    env.server.deliver(
        "INBOX",
        &raw(
            "g2@lark.example",
            "Re: Hello",
            &["g1@lark.example"],
            "y",
            false,
        ),
        &["\\Seen"],
        now_ms(),
    );
    let mut e = env.sync(WindowPolicy::EVERYTHING).await;
    let g1 = env.by_header("g1@lark.example").unwrap();
    let g2 = env.by_header("g2@lark.example").unwrap();
    assert!(g1.id.starts_with("gm:") && g1.thread_id.starts_with("gt:"));
    assert_eq!(g1.thread_id, g2.thread_id);
    assert_eq!(g1.label_ids, vec!["INBOX", "UNREAD"]);
    let p = env.provider();
    // Label as Receipts (Gmail keeps it in the inbox too), then archive.
    p.modify_thread(&g1.thread_id, &["f:Receipts".into()], &[])
        .await
        .unwrap();
    let all = env.server.messages("[Gmail]/All Mail");
    assert!(
        all.iter().all(|m| m.gm_labels.contains("Receipts")),
        "{all:?}"
    );
    p.modify_thread(&g1.thread_id, &[], &["INBOX".into()])
        .await
        .unwrap();
    assert!(env.server.messages("INBOX").is_empty());
    e.poll(true).await.unwrap();
    let now = env.store.get_message(USER, &g1.id).unwrap().unwrap();
    assert_eq!(now.label_ids, vec!["UNREAD", "f:Receipts"]);
    // Trash and back.
    p.trash_thread(&g1.thread_id).await.unwrap();
    assert_eq!(env.server.messages("[Gmail]/Trash").len(), 2);
    p.untrash_thread(&g1.thread_id).await.unwrap();
    assert_eq!(env.server.messages("INBOX").len(), 2);
    assert!(env.server.messages("[Gmail]/Trash").is_empty());
    // Server search runs Gmail syntax in All Mail.
    let q = penguin_core::query::parse("Hello", now_ms());
    let found = p.server_search(&q, 10).await.unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].match_count, 2);
}

#[tokio::test]
async fn a_refused_password_stops_sync_with_needs_reauth() {
    let env = Env::new(GENERIC_CAPS, false, "yahoo").await;
    env.server.state.lock().unwrap().refuse_login = true;
    let h = env.backend.start_sync(&env.account);
    for _ in 0..100 {
        if !h.is_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !h.is_running(),
        "the task ends instead of retrying on a timer"
    );
    let s = env.backend.sync_status(USER).unwrap();
    assert_eq!(s.phase, penguin_core::SyncPhase::NeedsReauth);
    assert!(s.error.unwrap().contains("Invalid credentials"));
}

#[tokio::test]
async fn idle_pushes_new_mail() {
    idle_pushes_new_mail_with(GENERIC_CAPS).await;
}

#[tokio::test]
async fn idle_pushes_new_mail_over_compress_deflate() {
    idle_pushes_new_mail_with(&with_compress(GENERIC_CAPS)).await;
}

async fn idle_pushes_new_mail_with(caps: &[&str]) {
    let env = Env::new(caps, false, "fastmail").await;
    let backend = ImapBackend::with_vault(
        env.store.clone(),
        env.observer.clone(),
        env.backend.vault().clone(),
        true,
    );
    let h = backend.start_sync(&env.account);
    // Wait for the first pass to finish.
    for _ in 0..200 {
        if backend
            .sync_status(USER)
            .is_some_and(|s| s.phase == penguin_core::SyncPhase::Idle)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    env.server.deliver(
        "INBOX",
        &raw("push@lark.example", "Pushed", &[], "x", false),
        &[],
        now_ms(),
    );
    let mut got = false;
    for _ in 0..200 {
        if !env.added().is_empty() {
            got = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    h.stop();
    assert!(
        got,
        "IDLE should poke the sync loop well before the 60 s poll"
    );
}

#[tokio::test]
async fn a_connection_the_server_logged_out_while_idle_reopens_quietly() {
    reaped_connection_reopens_with(GENERIC_CAPS).await;
}

#[tokio::test]
async fn yahoos_plain_bye_on_a_compressed_connection_reopens_quietly() {
    reaped_connection_reopens_with(&with_compress(GENERIC_CAPS)).await;
}

async fn reaped_connection_reopens_with(caps: &[&str]) {
    let env = Env::new(caps, false, "yahoo").await;
    let mut e = env.sync(WindowPolicy::EVERYTHING).await;
    e.stale_after = Duration::ZERO;
    env.server.reap_idle();
    env.server.deliver(
        "INBOX",
        &raw("late@lark.example", "After the logout", &[], "x", false),
        &[],
        now_ms(),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    e.poll(false)
        .await
        .expect("a reaped connection is not a sync error");
    assert!(env.by_header("late@lark.example").is_some());
}

// ---- failure streaks: when "Can't reach …" is shown (penguin-core sync_health.rs) ----

/// Wait (up to 10 s) until `done` holds for the account's latest status.
async fn status_until(
    shared: &Shared,
    done: impl Fn(&penguin_core::SyncStatus) -> bool,
) -> penguin_core::SyncStatus {
    for _ in 0..500 {
        let s = shared.snapshot();
        if done(&s) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("status never settled: {:?}", shared.snapshot());
}

fn statuses_since(env: &Env, from: usize) -> Vec<penguin_core::SyncStatus> {
    env.observer.statuses.lock().unwrap()[from..].to_vec()
}

/// A running engine with a short backoff (20 ms, 40 ms, 80 ms …) that has
/// finished its first sync.
async fn running(env: &Env) -> (Arc<Shared>, tokio::task::JoinHandle<()>) {
    let shared = Shared::new(USER, env.observer.clone(), WindowPolicy::EVERYTHING);
    let mut e = AccountSync::new(env.ctx.clone(), shared.clone());
    e.backoff = Duration::from_millis(20);
    let task = tokio::spawn(e.run());
    shared.poke();
    status_until(&shared, |s| {
        s.phase == penguin_core::SyncPhase::Idle && s.last_synced_at.is_some()
    })
    .await;
    (shared, task)
}

/// The flicker: the connection drops once (Wi-Fi hand-off, the Mac waking,
/// a server reset). Before, that one attempt set the phase to Error, which
/// put up "Can't reach Yahoo right now" until the retry 10 s later took it
/// down again. Now the failure is recorded quietly and the retry closes it.
#[tokio::test]
async fn a_connection_that_drops_once_is_retried_quietly() {
    let env = Env::new(GENERIC_CAPS, false, "yahoo").await;
    let (shared, task) = running(&env).await;
    let from = env.observer.statuses.lock().unwrap().len();
    env.server.deliver(
        "INBOX",
        &raw("blip@lark.example", "After the blip", &[], "x", false),
        &[],
        now_ms(),
    );
    env.server.state.lock().unwrap().drop = Some(("*".into(), 1));
    shared.poke();
    let end = status_until(&shared, |s| s.recovered.is_some()).await;
    task.abort();

    let seen = statuses_since(&env, from);
    let failures: Vec<_> = seen.iter().filter_map(|s| s.failure.clone()).collect();
    assert!(
        !failures.is_empty(),
        "the dropped connection is a failed attempt"
    );
    assert!(
        failures.iter().all(|f| f.count == 1 && !f.alert),
        "{failures:?}"
    );
    assert_eq!(failures[0].kind, penguin_core::SyncErrorKind::Network);
    assert!(end.failure.is_none() && end.error.is_none());
    let r = end.recovered.unwrap();
    assert_eq!((r.failures, r.alerted), (1, false));
    assert!(env.by_header("blip@lark.example").is_some());
}

/// Logging in again is not progress: a server that accepts the login and
/// then fails the same step every time must reach the alert, not reset the
/// count on every reconnect (and flip between "Up to date" and the error).
#[tokio::test]
async fn a_step_that_keeps_failing_after_login_alerts_on_the_third_attempt() {
    let env = Env::new(GENERIC_CAPS, false, "yahoo").await;
    let (shared, task) = running(&env).await;
    let from = env.observer.statuses.lock().unwrap().len();
    env.server.deliver(
        "INBOX",
        &raw(
            "stuck@lark.example",
            "Behind a failing fetch",
            &[],
            "x",
            false,
        ),
        &[],
        now_ms(),
    );
    env.server.state.lock().unwrap().drop = Some(("UID FETCH".into(), 3));
    shared.poke();
    let end = status_until(&shared, |s| s.recovered.is_some()).await;
    task.abort();

    let seen = statuses_since(&env, from);
    let mut steps: Vec<(u32, bool)> = seen
        .iter()
        .filter_map(|s| s.failure.as_ref().map(|f| (f.count, f.alert)))
        .collect();
    steps.dedup();
    assert_eq!(steps, vec![(1, false), (2, false), (3, true)]);
    let first = seen.iter().position(|s| s.failure.is_some()).unwrap();
    let last = seen.iter().rposition(|s| s.failure.is_some()).unwrap();
    assert!(
        seen[first..=last]
            .iter()
            .all(|s| s.phase == penguin_core::SyncPhase::Error),
        "no healthy status between the failures"
    );
    let r = end.recovered.unwrap();
    assert_eq!((r.failures, r.alerted), (3, true));
    assert!(env.by_header("stuck@lark.example").is_some());
}

/// "Retry now" keeps the failure on the status until the retry has
/// synced: before, it reported the account healthy at once, and the
/// alert came back when the retry failed.
#[tokio::test]
async fn retry_now_keeps_the_failure_until_the_retry_syncs() {
    let env = Env::new(GENERIC_CAPS, false, "yahoo").await;
    let h = env.backend.start_sync(&env.account);
    h.poke();
    for _ in 0..500 {
        let s = env.backend.sync_status(USER).unwrap();
        if s.phase == penguin_core::SyncPhase::Idle && s.last_synced_at.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    env.server.deliver(
        "INBOX",
        &raw("retry@lark.example", "After the retry", &[], "x", false),
        &[],
        now_ms(),
    );
    env.server.state.lock().unwrap().drop = Some(("*".into(), 1));
    h.poke();
    for _ in 0..500 {
        if env.backend.sync_status(USER).unwrap().failure.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The engine now waits 10 s before trying again; the user doesn't.
    let failed_at = env
        .backend
        .sync_status(USER)
        .unwrap()
        .failure
        .unwrap()
        .last_at;
    let from = env.observer.statuses.lock().unwrap().len();
    let retried = env.backend.retry_sync(&env.account);
    let right_after = env.observer.statuses.lock().unwrap()[from].clone();
    assert_eq!(right_after.phase, penguin_core::SyncPhase::Error);
    let f = right_after.failure.unwrap();
    assert_eq!(f.count, 1);
    assert!(
        f.next_retry_at.unwrap() < failed_at + 10_000,
        "retrying now, not in 10 s"
    );
    let mut end = None;
    for _ in 0..150 {
        let s = env.backend.sync_status(USER).unwrap();
        if s.recovered.is_some() {
            end = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    retried.stop();
    let end = end.expect("the retry synced well before the 10 s backoff");
    assert!(end.failure.is_none());
    assert!(env.by_header("retry@lark.example").is_some());
}

#[tokio::test]
async fn threads_merge_when_a_missing_link_arrives() {
    let env = Env::new(GENERIC_CAPS, false, "fastmail").await;
    env.server.deliver(
        "INBOX",
        &raw("t1@lark.example", "Trip", &[], "x", false),
        &[],
        now_ms() - 3 * DAY,
    );
    // A reply that lost the root from its References.
    env.server.deliver(
        "INBOX",
        &raw(
            "t3@lark.example",
            "Re: Trip",
            &["t2@lark.example"],
            "z",
            false,
        ),
        &[],
        now_ms() - DAY,
    );
    let mut e = env.sync(WindowPolicy::EVERYTHING).await;
    let t1 = env.by_header("t1@lark.example").unwrap();
    let t3 = env.by_header("t3@lark.example").unwrap();
    assert_ne!(t1.thread_id, t3.thread_id);
    env.server.deliver(
        "INBOX",
        &raw(
            "t2@lark.example",
            "Re: Trip",
            &["t1@lark.example"],
            "y",
            false,
        ),
        &[],
        now_ms(),
    );
    e.poll(false).await.unwrap();
    let t3 = env.store.get_message(USER, &t3.id).unwrap().unwrap();
    assert_eq!(t3.thread_id, t1.thread_id, "merged via set_message_thread");
    let changed: Vec<String> = env
        .observer
        .changed
        .lock()
        .unwrap()
        .iter()
        .flat_map(|(_, t)| t.clone())
        .collect();
    assert!(changed.contains(&t1.thread_id));
}

/// Conformance against a real server. Set PENGUIN_IMAP_TEST_HOST,
/// PENGUIN_IMAP_TEST_PORT, PENGUIN_IMAP_TEST_SMTP_PORT, PENGUIN_IMAP_TEST_USER,
/// PENGUIN_IMAP_TEST_PASS (and optionally PENGUIN_IMAP_TEST_SECURITY=tls|starttls|plain,
/// PENGUIN_IMAP_TEST_SEND_TO). docs/PROVIDERS-IMPL.md → IMAP shows the
/// GreenMail and Dovecot commands. It APPENDs and changes mail: never at a
/// real person's account.
#[tokio::test]
#[ignore]
async fn real_server_conformance() {
    use penguin_core::{MailSecurity, ServerSettings};
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
    let host = var("PENGUIN_IMAP_TEST_HOST");
    let user = var("PENGUIN_IMAP_TEST_USER");
    let pass = var("PENGUIN_IMAP_TEST_PASS");
    let security = match std::env::var("PENGUIN_IMAP_TEST_SECURITY").as_deref() {
        Ok("tls") => MailSecurity::Tls,
        Ok("starttls") => MailSecurity::Starttls,
        _ => MailSecurity::Plain,
    };
    let has_smtp = std::env::var("PENGUIN_IMAP_TEST_SMTP_PORT").is_ok();
    let server = |port: &str| ServerSettings {
        host: host.clone(),
        port: std::env::var(port)
            .unwrap_or_else(|_| var("PENGUIN_IMAP_TEST_PORT"))
            .parse()
            .unwrap(),
        security,
        username: user.clone(),
    };
    let store = Store::open_in_memory().unwrap();
    let email = if user.contains('@') {
        user.clone()
    } else {
        format!("{user}@localhost.example")
    };
    let account = Account {
        id: email.to_lowercase(),
        email: email.clone(),
        provider: AccountProvider::Imap,
        provider_config: ProviderConfig {
            auth: Some(AuthMethod::ImapPassword),
            imap: Some(server("PENGUIN_IMAP_TEST_PORT")),
            smtp: Some(server("PENGUIN_IMAP_TEST_SMTP_PORT")),
            host: Some("imapGeneric".into()),
            ..ProviderConfig::default()
        },
        ..Account::default()
    };
    store.upsert_account(&account).unwrap();
    let vault: Arc<SecretVault<PasswordCredential>> = Arc::new(SecretVault::new(Box::new(
        Arc::new(MemorySecrets::default()),
    )));
    vault
        .save(
            &account.id,
            &PasswordCredential {
                password: pass.clone(),
                smtp_password: None,
                saved_at: 1,
            },
        )
        .unwrap();
    let observer = Arc::new(RecordingObserver::default());
    let backend = ImapBackend::with_vault(store.clone(), observer.clone(), vault, false);
    let ctx = backend.ctx(&account).unwrap();
    if has_smtp {
        crate::connect::verify(&ctx.cfg, &pass)
            .await
            .expect("login + SMTP auth");
    }
    // Seed one inbox message with an attachment (unique Message-ID).
    let msgid = format!("conf{:x}@lark.example", fastrand::u64(..));
    {
        let mut s = ctx.open_session().await.unwrap();
        s.append(
            "INBOX",
            "",
            &raw(
                &msgid,
                "Quarterly conformance report",
                &[],
                "numbers inside",
                true,
            ),
        )
        .await
        .unwrap();
    }
    let mut e = AccountSync::new(
        ctx.clone(),
        Shared::new(&account.id, observer.clone(), WindowPolicy::EVERYTHING),
    );
    e.init().await.unwrap();
    while e.fill_step().await.unwrap() {}
    let m = store
        .provider_read(|c| {
            let mut stmt = c.prepare("SELECT id FROM messages WHERE account_id = ?1")?;
            let ids = stmt
                .query_map([&account.id], |r| r.get::<_, String>(0))?
                .collect::<penguin_core::rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })
        .unwrap()
        .into_iter()
        .filter_map(|id| store.get_message(&account.id, &id).unwrap())
        .find(|m| m.message_id_header.as_deref() == Some(msgid.as_str()))
        .expect("seeded message synced");
    let p = ImapProvider::new(ctx.clone());
    let send_to = std::env::var("PENGUIN_IMAP_TEST_SEND_TO")
        .ok()
        .map(|e| Address {
            name: None,
            email: e,
        });
    let case = Case {
        message_id: m.id.clone(),
        thread_id: m.thread_id.clone(),
        attachment: m.attachments.first().cloned(),
        from: Address {
            name: None,
            email: email.clone(),
        },
        send_to,
    };
    let failures = conformance::run(&p, &store, &case).await;
    assert!(failures.is_empty(), "{failures:#?}");
    let caps = ctx.caps().unwrap();
    eprintln!(
        "real server OK: condstore={} qresync={} objectid={} move={} uidplus={} idle={}",
        caps.condstore(),
        caps.has("QRESYNC"),
        caps.has("OBJECTID"),
        caps.has("MOVE"),
        caps.has("UIDPLUS"),
        caps.has("IDLE")
    );
}

#[tokio::test]
async fn reply_later_folder_is_created_once_and_moves_mail_in_and_out() {
    let env = Env::new(GENERIC_CAPS, false, "imapGeneric").await;
    env.server.deliver(
        "INBOX",
        &raw(
            "rl@lark.example",
            "Can you review?",
            &[],
            "draft attached",
            false,
        ),
        &[],
        now_ms() - DAY,
    );
    env.sync(WindowPolicy::EVERYTHING).await;
    let m = env.by_header("rl@lark.example").unwrap();
    let p = env.provider();

    let label = p.ensure_label("Reply Later").await.unwrap();
    assert_eq!(
        (label.id.as_str(), label.name.as_str()),
        ("f:Reply%20Later", "Reply Later")
    );
    let before = env.server.state.lock().unwrap().mailboxes.len();
    let again = p.ensure_label("reply later").await.unwrap();
    assert_eq!(again.id, label.id, "found, not created twice");
    assert_eq!(env.server.state.lock().unwrap().mailboxes.len(), before);

    // Reply Later = label + archive + read: one move into the folder.
    p.modify_thread(
        &m.thread_id,
        std::slice::from_ref(&label.id),
        &["INBOX".into(), "UNREAD".into()],
    )
    .await
    .unwrap();
    assert!(env.server.messages("INBOX").is_empty());
    let filed = env.server.messages("Reply Later");
    assert_eq!(filed.len(), 1);
    assert!(filed[0].flags.contains("\\Seen"));

    // Replied: the label comes off and the conversation goes to Archive.
    p.modify_thread(&m.thread_id, &[], std::slice::from_ref(&label.id))
        .await
        .unwrap();
    assert!(env.server.messages("Reply Later").is_empty());
    assert_eq!(env.server.messages("Archive").len(), 1);
}

#[tokio::test]
async fn backfill_costs_few_round_trips_per_message() {
    let env = Env::new(GENERIC_CAPS, false, "fastmail").await;
    for i in 0..250 {
        env.server.deliver(
            "INBOX",
            &raw(
                &format!("rt{i}@lark.example"),
                &format!("Note {i}"),
                &[],
                "hi",
                false,
            ),
            &["\\Seen"],
            now_ms() - (250 - i) * 3_600_000,
        );
    }
    let mut e = env.engine(WindowPolicy::EVERYTHING);
    e.init().await.unwrap();
    env.server.state.lock().unwrap().commands.clear();
    while e.fill_step().await.unwrap() {}
    assert_eq!(env.count(), 250);
    let commands = env.server.state.lock().unwrap().commands.clone();
    let count = |c: &str| commands.iter().filter(|x| *x == c).count();
    // One EXAMINE per synced folder (the selection is then reused from
    // chunk to chunk), one SEARCH for INBOX's list, and per 100-message
    // chunk one FETCH for ids plus one for the bodies of small mail.
    assert!(count("EXAMINE") <= 7, "{commands:?}");
    assert_eq!(count("UID SEARCH"), 1, "{commands:?}");
    assert_eq!(count("UID FETCH"), 6, "{commands:?}");
}

#[test]
fn full_downloads_are_grouped_by_count_and_bytes() {
    use crate::account::{full_groups, FULL_CHUNK, FULL_FETCH_BYTES, STRUCTURE_ABOVE};
    use crate::db::{Flags, Location};
    use crate::session::FetchItem;
    let item = |size: u64| {
        (
            Location {
                folder: "INBOX".into(),
                uid: 1,
                uidvalidity: 1,
                message_id: String::new(),
                flags: Flags::default(),
                gm_labels: None,
            },
            FetchItem {
                size: Some(size),
                ..FetchItem::default()
            },
        )
    };
    let lens = |g: Vec<&[(Location, FetchItem)]>| g.iter().map(|g| g.len()).collect::<Vec<_>>();
    // Ordinary mail: FULL_CHUNK per group.
    let small: Vec<_> = (0..FULL_CHUNK * 2 + 20).map(|_| item(30_000)).collect();
    assert_eq!(lens(full_groups(&small)), vec![FULL_CHUNK, FULL_CHUNK, 20]);
    // Big mail splits by bytes; a message over STRUCTURE_ABOVE counts as
    // that much (only its text parts are downloaded).
    let per_group = (FULL_FETCH_BYTES / STRUCTURE_ABOVE) as usize;
    assert!(per_group < FULL_CHUNK);
    let big: Vec<_> = (0..per_group + 1).map(|_| item(STRUCTURE_ABOVE)).collect();
    assert_eq!(lens(full_groups(&big)), vec![per_group, 1]);
    let huge = vec![item(STRUCTURE_ABOVE * 100), item(10)];
    assert_eq!(lens(full_groups(&huge)), vec![2]);
    assert!(full_groups(&[]).is_empty());
    // With its BODYSTRUCTURE, a big message counts its header and text
    // parts: one with a 3 MB PDF and a little text is small…
    let with_structure = |size: u64, text: u64| {
        let mut e = item(size);
        e.1.bodystructure = Some(
            crate::proto::Tokens::new(
                format!(
                    "((\"text\" \"plain\" NIL NIL NIL \"7bit\" {text} 1)(\"application\" \"pdf\" NIL NIL NIL \"base64\" {size} NIL (\"attachment\" (\"filename\" \"a.pdf\")) NIL) \"mixed\")"
                )
                .as_bytes(),
            )
            .value()
            .unwrap(),
        );
        e
    };
    use crate::account::download_bytes;
    assert_eq!(
        download_bytes(&with_structure(3_000_000, 2_000).1),
        2_000 + 16 * 1024
    );
    let pdfs: Vec<_> = (0..FULL_CHUNK)
        .map(|_| with_structure(3_000_000, 2_000))
        .collect();
    assert_eq!(lens(full_groups(&pdfs)), vec![FULL_CHUNK]);
    // …and a 1.5 MB newsletter counts all of its HTML (it comes uncapped).
    assert_eq!(
        download_bytes(&with_structure(1_600_000, 1_500_000).1),
        1_516_384
    );
}

/// A message whose bulk is attachments: alternative text + HTML, an inline
/// image, a PDF and a forwarded message.
fn with_attachments(msgid: &str, pdf_bytes: usize) -> Vec<u8> {
    use base64::Engine;
    let b64 = |bytes: &[u8]| {
        let s = base64::engine::general_purpose::STANDARD.encode(bytes);
        s.as_bytes()
            .chunks(76)
            .map(|c| format!("{}\r\n", std::str::from_utf8(c).unwrap()))
            .collect::<String>()
    };
    let pdf: Vec<u8> = b"%PDF-1.4\n"
        .iter()
        .copied()
        .chain((0..pdf_bytes).map(|i| (i * 7919 % 251) as u8))
        .collect();
    format!(
        "From: Bea Lark <bea@lark.example>\r\nTo: Sam Okafor <{USER}>\r\nSubject: =?UTF-8?Q?Q3_r=C3=A9sum=C3=A9?=\r\nMessage-ID: <{msgid}>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"mix\"\r\n\r\n\
--mix\r\nContent-Type: multipart/related; boundary=\"rel\"\r\n\r\n\
--rel\r\nContent-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n\
--alt\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nNumbers attached, caf=C3=A9 at 10.\r\n\
--alt\r\nContent-Type: text/html; charset=\"iso-8859-1\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\
--alt--\r\n\
--rel\r\nContent-Type: image/png; name=\"logo.png\"\r\nContent-ID: <logo@lark>\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\
--rel--\r\n\
--mix\r\nContent-Type: application/pdf; name=\"q3.pdf\"\r\nContent-Disposition: attachment; filename=\"q3.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\
--mix\r\nContent-Type: message/rfc822\r\nContent-Disposition: attachment\r\n\r\nFrom: Old <old@lark.example>\r\nSubject: Earlier\r\n\r\nforwarded text\r\n\
--mix--\r\n",
        b64(b"<p>Numbers attached, caf\xe9 at 10.</p><img src=\"cid:logo@lark\">"),
        b64(&[0x89, b'P', b'N', b'G', 1, 2, 3, 4, 5, 6, 7, 8]),
        b64(&pdf),
    )
    .into_bytes()
}

#[tokio::test]
async fn big_messages_sync_without_their_attachments() {
    for gmail in [false, true] {
        let env = Env::new(
            GENERIC_CAPS,
            gmail,
            if gmail { "gmail" } else { "fastmail" },
        )
        .await;
        let big = with_attachments("big@lark.example", 400_000);
        let small = with_attachments("small@lark.example", 2_000);
        assert!(big.len() as u64 > crate::account::STRUCTURE_ABOVE);
        assert!((small.len() as u64) < crate::account::STRUCTURE_ABOVE);
        env.server
            .deliver("INBOX", &big, &["\\Seen"], now_ms() - DAY);
        env.server
            .deliver("INBOX", &small, &["\\Seen"], now_ms() - DAY);
        env.server
            .state
            .lock()
            .unwrap()
            .wire_out
            .store(0, std::sync::atomic::Ordering::Relaxed);
        env.sync(WindowPolicy::EVERYTHING).await;
        let wire = env
            .server
            .state
            .lock()
            .unwrap()
            .wire_out
            .load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            wire < big.len() as u64 / 4,
            "the PDF's bytes were downloaded: {wire} B on the wire"
        );
        let got = env.by_header("big@lark.example").expect("big synced");
        let small_got = env.by_header("small@lark.example").expect("small synced");
        // Same message as a whole-message download would give.
        let whole = mime::to_message(
            &big,
            &mime::Meta {
                account_id: USER.into(),
                id: got.id.clone(),
                thread_id: got.thread_id.clone(),
                date_ms: Some(got.date),
                labels: got.label_ids.clone(),
                trusted_authserv: env.ctx.cfg.trusted_authserv(),
            },
        );
        assert_eq!(got.subject, "Q3 résumé");
        assert_eq!(got.subject, whole.subject);
        assert_eq!(got.from, whole.from);
        assert_eq!(got.to, whole.to);
        assert_eq!(got.body_text, whole.body_text);
        assert_eq!(got.body_html, whole.body_html);
        assert!(got.body_html.as_deref().unwrap().contains("café"));
        assert_eq!(got.snippet, whole.snippet);
        assert_eq!(got.message_id_header, whole.message_id_header);
        let key = |a: &penguin_core::AttachmentMeta| {
            (
                a.id.clone(),
                a.filename.clone(),
                a.mime_type.clone(),
                a.content_id.clone(),
                a.inline,
            )
        };
        assert_eq!(
            got.attachments.iter().map(key).collect::<Vec<_>>(),
            whole.attachments.iter().map(key).collect::<Vec<_>>()
        );
        for (a, b) in got.attachments.iter().zip(&whole.attachments) {
            let (a, b) = (a.size as f64, b.size as f64);
            assert!((a - b).abs() <= b * 0.05 + 64.0, "size {a} vs {b}");
        }
        // The small one went whole: same fields.
        assert_eq!(small_got.body_html, got.body_html);
        assert_eq!(
            small_got.attachments.iter().map(key).collect::<Vec<_>>(),
            got.attachments.iter().map(key).collect::<Vec<_>>()
        );
        // And the PDF itself still downloads on demand.
        let pdf = got
            .attachments
            .iter()
            .find(|a| a.filename == "q3.pdf")
            .unwrap()
            .clone();
        let bytes = env.provider().get_attachment(&got.id, &pdf).await.unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert_eq!(bytes.len(), 400_009);
    }
}

#[tokio::test]
async fn long_text_under_two_mb_is_not_cut() {
    // Over STRUCTURE_ABOVE, so it takes the structured path, but under
    // LARGE_MESSAGE: its 1.3 MB of text arrives whole, as it would in a
    // whole-message download.
    let env = Env::new(GENERIC_CAPS, false, "fastmail").await;
    let body = format!(
        "{}THE END",
        "All quiet on the numbers front today.\r\n".repeat(34_000)
    );
    assert!(body.len() as u64 > crate::account::TEXT_PART_CAP);
    let msg = raw("long@lark.example", "Long", &[], &body, true);
    assert!((msg.len() as u64) < crate::account::LARGE_MESSAGE);
    env.server
        .deliver("INBOX", &msg, &["\\Seen"], now_ms() - DAY);
    env.sync(WindowPolicy::EVERYTHING).await;
    let got = env.by_header("long@lark.example").expect("synced");
    assert!(got.body_text.trim_end().ends_with("THE END"));
    assert_eq!(got.attachments.len(), 1);
}

/// CONDSTORE without QRESYNC (Gmail's IMAP offers this pair).
const CONDSTORE_CAPS: &[&str] = &[
    "IMAP4rev1",
    "AUTH=PLAIN",
    "SASL-IR",
    "UIDPLUS",
    "MOVE",
    "IDLE",
    "CONDSTORE",
];

#[tokio::test]
async fn a_quiet_poll_asks_every_folder_in_one_round_trip() {
    for caps in [GENERIC_CAPS, CONDSTORE_CAPS, BASIC_CAPS] {
        let env = Env::new(caps, false, "imapGeneric").await;
        let condstore = caps.contains(&"CONDSTORE");
        let mut uids = Vec::new();
        for i in 0..6 {
            let name = format!("Projects/P{i}");
            env.server.add_folder(&name);
            uids.push(env.server.deliver(
                &name,
                &raw(&format!("p{i}@lark.example"), "Plans", &[], "x", false),
                &["\\Seen"],
                now_ms() - DAY,
            ));
        }
        let mut e = env.sync(WindowPolicy::EVERYTHING).await;
        e.poll(true).await.unwrap();
        env.server.state.lock().unwrap().commands.clear();

        // Nothing changed: one STATUS per folder, sent together; only the
        // selected folder (STATUS isn't for it) and, without CONDSTORE,
        // the primary folder (its flags are scanned every poll) are opened.
        e.poll(true).await.unwrap();
        let cmds = env.server.state.lock().unwrap().commands.clone();
        let opened = cmds
            .iter()
            .filter(|c| *c == "SELECT" || *c == "EXAMINE")
            .count();
        let statuses = cmds.iter().filter(|c| *c == "STATUS").count();
        assert!(statuses >= 12, "caps {caps:?}: {cmds:?}");
        assert!(opened <= 3, "caps {caps:?}: {cmds:?}");
        assert!(
            !cmds.iter().any(|c| c == "UID FETCH"),
            "caps {caps:?}: {cmds:?}"
        );

        // Changes in skipped folders are still seen: new mail, a deletion,
        // and (with CONDSTORE, which shows them in STATUS) flag changes.
        env.server.deliver(
            "Projects/P2",
            &raw("new@lark.example", "News", &[], "y", false),
            &[],
            now_ms(),
        );
        env.server.expunge("Projects/P4", uids[4]);
        env.server
            .set_flags("Projects/P3", uids[3], &["\\Seen", "\\Flagged"]);
        e.poll(true).await.unwrap();
        assert!(env.by_header("new@lark.example").is_some(), "caps {caps:?}");
        assert!(env.by_header("p4@lark.example").is_none(), "caps {caps:?}");
        if condstore {
            let p3 = env.by_header("p3@lark.example").unwrap();
            assert!(
                p3.label_ids.iter().any(|l| l == "STARRED"),
                "caps {caps:?}: {:?}",
                p3.label_ids
            );
        }
    }
}
