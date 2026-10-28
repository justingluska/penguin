//! Cross-device state through the real store: two devices (each a Store)
//! sharing penguin-sync's in-memory server. Each device also sees the same
//! mailbox, applied by hand where a provider sync would apply it.

use penguin_sync::memory::MemoryServer;
use penguin_sync::{sync_once, thread_key, Kind, LocalStore, SnoozeBody, SnoozeState};

use crate::store::LocalWake;
use crate::types::*;
use crate::Store;

const A: &str = "ada@penguin.example";
const HOUR: i64 = 3_600_000;
const T0: i64 = 1_760_000_000_000;

fn account(id: &str) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        color: "#123456".into(),
        added_at: 1,
        ..Account::default()
    }
}

fn device() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A)).unwrap();
    s.set_sync_enabled(true).unwrap();
    s
}

fn msg(id: &str, thread: &str, date: i64, labels: &[&str]) -> Message {
    Message {
        account_id: A.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: Address {
            name: Some("Cy Quill".into()),
            email: "cy@quill.example".into(),
        },
        to: vec![Address {
            name: None,
            email: A.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: format!("Subject {thread}"),
        snippet: format!("snippet {id}"),
        body_text: format!("body {id}"),
        body_html: None,
        label_ids: labels.iter().map(|l| l.to_string()).collect(),
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        sender_authenticated: false,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
    }
}

fn labels(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn sync(s: &Store, server: &MemoryServer, now: i64) {
    server.set_now(now);
    sync_once(s, &mut server.transport(), &|| now).unwrap();
}

/// What the app does to snooze: the record, then the archive.
fn snooze(s: &Store, thread: &str, until: i64, now: i64) {
    s.snooze_threads(&[(A.into(), thread.into())], until, now)
        .unwrap();
    s.modify_thread_labels(A, thread, &[], &labels(&["INBOX"]))
        .unwrap();
}

fn archive(s: &Store, thread: &str) {
    s.modify_thread_labels(A, thread, &[], &labels(&["INBOX"]))
        .unwrap();
}

fn snoozed(s: &Store) -> Vec<(String, i64)> {
    s.list_snoozes(None)
        .unwrap()
        .into_iter()
        .map(|z| (z.thread_id, z.wake_at))
        .collect()
}

fn thread_flags(s: &Store, thread: &str) -> (bool, bool) {
    let t = s
        .list_threads(&ListQuery {
            view: MailboxView::All,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 100,
            before: None,
            unread_only: false,
            split: None,
        })
        .unwrap()
        .into_iter()
        .find(|t| t.thread_id == thread)
        .expect("thread");
    (t.label_ids.iter().any(|l| l == "INBOX"), t.unread)
}

fn record_state(s: &Store, thread: &str) -> Option<SnoozeState> {
    s.sync_records(Kind::Snooze)
        .unwrap()
        .into_iter()
        .find(|r| r.key == thread_key(A, thread))
        .and_then(|r| r.body_as::<SnoozeBody>())
        .map(|b| b.state)
}

/// Both devices have thread t1 in the inbox, read.
fn two_devices() -> (Store, Store, MemoryServer) {
    let (mac, phone) = (device(), device());
    for d in [&mac, &phone] {
        d.upsert_messages(&[msg("m1", "t1", T0, &["INBOX"])])
            .unwrap();
    }
    (mac, phone, MemoryServer::new())
}

#[test]
fn every_install_has_its_own_device_id() {
    let (a, b) = (device(), device());
    assert_ne!(a.sync_device().unwrap(), b.sync_device().unwrap());
    assert_eq!(a.sync_device().unwrap().len(), 16);
}

#[test]
fn a_snooze_made_on_the_mac_shows_on_the_phone_and_waits_for_the_archive() {
    let (mac, phone, server) = two_devices();
    snooze(&mac, "t1", T0 + 5 * HOUR, T0 + 1);
    sync(&mac, &server, T0 + 2);
    sync(&phone, &server, T0 + 3);
    assert_eq!(snoozed(&phone), vec![("t1".to_string(), T0 + 5 * HOUR)]);
    // The phone hasn't seen Gmail's archive yet: something else changing
    // the thread must not count as "back in the inbox".
    phone
        .modify_thread_labels(A, "t1", &labels(&["STARRED"]), &[])
        .unwrap();
    assert_eq!(
        snoozed(&phone).len(),
        1,
        "kept while the archive is pending"
    );
    // The archive arrives through the phone's own Gmail sync.
    archive(&phone, "t1");
    assert_eq!(snoozed(&phone).len(), 1);
    // From now on, back in the inbox means it ended early, as usual.
    phone
        .upsert_messages(&[msg("m2", "t1", T0 + HOUR, &["INBOX", "UNREAD"])])
        .unwrap();
    assert!(snoozed(&phone).is_empty());
    assert_eq!(record_state(&phone, "t1"), Some(SnoozeState::Ended));
    // ...and the Mac hears it (it would also see the reply itself).
    sync(&phone, &server, T0 + HOUR + 1);
    sync(&mac, &server, T0 + HOUR + 2);
    assert!(snoozed(&mac).is_empty());
}

#[test]
fn a_wake_is_committed_once_and_the_other_device_only_follows() {
    let (mac, phone, server) = two_devices();
    snooze(&mac, "t1", T0 + HOUR, T0);
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    archive(&phone, "t1");

    // Wake time: the Mac wakes locally, but has not pulled since.
    assert!(matches!(
        mac.wake_snooze(A, "t1", T0 + HOUR).unwrap(),
        LocalWake::Woke { .. }
    ));
    assert_eq!(thread_flags(&mac, "t1"), (true, true));
    assert!(!mac.snooze_wake_committable(A, "t1").unwrap());
    // A pull after the wake time opens the gate.
    sync(&mac, &server, T0 + HOUR + 1);
    assert!(mac.snooze_wake_committable(A, "t1").unwrap());
    // Pushed to Gmail: the record says woken.
    mac.snooze_pushed(A, "t1").unwrap();
    assert_eq!(record_state(&mac, "t1"), Some(SnoozeState::Woken));
    sync(&mac, &server, T0 + HOUR + 2);

    // The phone, still holding the snooze, takes the wake without owing
    // Gmail anything: it never pushes a second wake.
    sync(&phone, &server, T0 + HOUR + 3);
    assert!(snoozed(&phone).is_empty());
    assert_eq!(thread_flags(&phone, "t1"), (true, true));
    assert!(phone.due_snoozes(T0 + 2 * HOUR).unwrap().is_empty());
    assert!(phone.pending(10).unwrap().is_empty());
    assert_eq!(record_state(&phone, "t1"), Some(SnoozeState::Woken));
}

#[test]
fn a_resnooze_from_the_offline_phone_undoes_the_macs_unpushed_wake() {
    let (mac, phone, server) = two_devices();
    snooze(&mac, "t1", T0 + HOUR, T0);
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    archive(&phone, "t1");
    // The phone (offline) snoozes it again until tomorrow, just before the
    // wake; the Mac wakes it locally at the old time but can't commit yet.
    snooze(&phone, "t1", T0 + 24 * HOUR, T0 + HOUR - 60_000);
    mac.wake_snooze(A, "t1", T0 + HOUR).unwrap();
    assert_eq!(thread_flags(&mac, "t1"), (true, true));
    // The phone gets online first; the Mac's next pull brings the re-snooze.
    sync(&phone, &server, T0 + HOUR + 10);
    sync(&mac, &server, T0 + HOUR + 20);
    assert_eq!(snoozed(&mac), vec![("t1".to_string(), T0 + 24 * HOUR)]);
    // The local wake is undone: archived and read again, as Gmail has it.
    assert_eq!(thread_flags(&mac, "t1"), (false, false));
    assert!(mac.due_snoozes(T0 + 2 * HOUR).unwrap().is_empty());
}

#[test]
fn unsnoozing_on_one_device_ends_it_on_the_other() {
    let (mac, phone, server) = two_devices();
    snooze(&mac, "t1", T0 + HOUR, T0);
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    assert!(phone.unsnooze(A, "t1").unwrap());
    sync(&phone, &server, T0 + 3);
    sync(&mac, &server, T0 + 4);
    assert!(snoozed(&mac).is_empty());
    assert!(mac.due_snoozes(T0 + 2 * HOUR).unwrap().is_empty());
}

#[test]
fn a_thread_gone_from_the_provider_ends_the_snooze_everywhere() {
    let (mac, phone, server) = two_devices();
    snooze(&mac, "t1", T0 + HOUR, T0);
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    archive(&phone, "t1");
    // The phone's wake push got a 404.
    phone.end_snooze(A, "t1").unwrap();
    sync(&phone, &server, T0 + 3);
    sync(&mac, &server, T0 + 4);
    assert!(snoozed(&mac).is_empty());
    assert_eq!(record_state(&mac, "t1"), Some(SnoozeState::Ended));
}

#[test]
fn an_ended_record_does_not_override_a_newer_snooze() {
    let (mac, phone, server) = two_devices();
    snooze(&mac, "t1", T0 + HOUR, T0);
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    archive(&phone, "t1");
    // The Mac snoozes it again (newer generation) while the phone ends the
    // old one: the new snooze stands on both.
    snooze(&mac, "t1", T0 + 3 * HOUR, T0 + 10);
    phone.end_snooze(A, "t1").unwrap();
    sync(&phone, &server, T0 + 20);
    sync(&mac, &server, T0 + 30);
    sync(&phone, &server, T0 + 40);
    assert_eq!(snoozed(&mac), vec![("t1".to_string(), T0 + 3 * HOUR)]);
    assert_eq!(snoozed(&phone), vec![("t1".to_string(), T0 + 3 * HOUR)]);
}

#[test]
fn follow_up_dismissals_sync_both_ways() {
    let (mac, phone, server) = two_devices();
    let mut sent = msg("s1", "t2", T0 - 5 * 24 * HOUR, &["SENT"]);
    sent.from = Address {
        name: None,
        email: A.into(),
    };
    sent.to = vec![Address {
        name: None,
        email: "cy@quill.example".into(),
    }];
    for d in [&mac, &phone] {
        d.upsert_messages(std::slice::from_ref(&sent)).unwrap();
    }
    let follow = |s: &Store| s.list_follow_ups(None, 3, T0, false).unwrap().len();
    assert_eq!((follow(&mac), follow(&phone)), (1, 1));
    mac.dismiss_follow_ups(&[(A.into(), "t2".into())], true)
        .unwrap();
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    assert_eq!(follow(&phone), 0);
    phone
        .dismiss_follow_ups(&[(A.into(), "t2".into())], false)
        .unwrap();
    sync(&phone, &server, T0 + 3);
    sync(&mac, &server, T0 + 4);
    assert_eq!(follow(&mac), 1);
}

#[test]
fn records_wait_for_an_account_this_device_does_not_have_yet() {
    let server = MemoryServer::new();
    let mac = device();
    mac.upsert_messages(&[msg("m1", "t1", T0, &["INBOX"])])
        .unwrap();
    snooze(&mac, "t1", T0 + HOUR, T0);
    sync(&mac, &server, T0 + 1);
    // A phone with no accounts yet keeps the record, unprojected.
    let phone = Store::open_in_memory().unwrap();
    phone.set_sync_enabled(true).unwrap();
    sync(&phone, &server, T0 + 2);
    assert!(snoozed(&phone).is_empty());
    assert_eq!(phone.sync_records(Kind::Snooze).unwrap().len(), 1);
    // Signing in to the account brings the snooze in.
    phone.upsert_account(&account(A)).unwrap();
    assert_eq!(snoozed(&phone), vec![("t1".to_string(), T0 + HOUR)]);
    // Removing an account here keeps its records for the other devices.
    phone.remove_account(A).unwrap();
    assert!(snoozed(&phone).is_empty());
    assert_eq!(phone.sync_records(Kind::Snooze).unwrap().len(), 1);
    assert!(
        phone.pending(10).unwrap().is_empty(),
        "nothing was deleted remotely"
    );
}

#[test]
fn existing_snoozes_and_dismissals_get_records_once() {
    let s = device();
    s.upsert_messages(&[msg("m1", "t1", T0, &["INBOX"])])
        .unwrap();
    // Rows from before sync: written straight to the tables, no records.
    s.write(|tx| {
        tx.execute(
            "INSERT INTO snoozes(account_id, thread_id, wake_at, snoozed_at) VALUES (?1, 't1', ?2, ?3)",
            rusqlite::params![A, T0 + HOUR, T0],
        )?;
        tx.execute(
            "INSERT INTO follow_up_dismissals(account_id, thread_id, sent_at) VALUES (?1, 't9', ?2)",
            rusqlite::params![A, T0],
        )?;
        tx.execute("UPDATE sync_state SET seeded = 0", [])?;
        super::seed_in(tx)?;
        super::seed_in(tx)?; // once only
        Ok(())
    })
    .unwrap();
    let pending = s.pending(10).unwrap();
    assert_eq!(pending.len(), 2);
    let snooze = s.sync_records(Kind::Snooze).unwrap();
    assert_eq!(
        snooze[0].body_as::<SnoozeBody>(),
        Some(SnoozeBody::snoozed(T0 + HOUR, T0))
    );
    // They reach a new device on its first sync.
    let server = MemoryServer::new();
    sync(&s, &server, T0 + 1);
    let phone = device();
    phone
        .upsert_messages(&[msg("m1", "t1", T0, &["SENT"])])
        .unwrap();
    sync(&phone, &server, T0 + 2);
    assert_eq!(snoozed(&phone), vec![("t1".to_string(), T0 + HOUR)]);
}

#[test]
fn sync_off_never_holds_a_wake_back() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A)).unwrap();
    s.upsert_messages(&[msg("m1", "t1", T0, &["INBOX"])])
        .unwrap();
    snooze(&s, "t1", T0 + HOUR, T0);
    s.wake_snooze(A, "t1", T0 + HOUR).unwrap();
    assert!(s.snooze_wake_committable(A, "t1").unwrap());
    assert!(s.sync_pull_fresh(i64::MAX).unwrap());
}

#[test]
fn settings_records_are_kept_for_the_app() {
    let (mac, phone, server) = two_devices();
    for (kind, key, body) in penguin_sync::settings::changes(
        &serde_json::json!({"followUpDays": 3}),
        &serde_json::json!({"followUpDays": 5, "snippets": [{"id": "s1", "body": "hi"}]}),
    ) {
        mac.sync_record_change(kind, &key, body, T0).unwrap();
    }
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    let mut settings = serde_json::json!({"followUpDays": 3});
    for kind in [Kind::Setting, Kind::Snippet] {
        for r in phone.sync_records(kind).unwrap() {
            penguin_sync::settings::apply(&mut settings, r.kind, &r.key, r.body.as_ref());
        }
    }
    assert_eq!(
        settings,
        serde_json::json!({"followUpDays": 5, "snippets": [{"id": "s1", "body": "hi"}]})
    );
}
