//! Snooze wakes against a fake Gmail, an in-memory store and a fake clock.

use std::sync::Mutex;

use penguin_core::{Account, Address, ListQuery, MailboxView, Message};

use super::*;

const A: &str = "ada@penguin.example";
const B: &str = "bea@penguin.example";
const MIN: i64 = 60_000;

#[derive(Default)]
struct Fake {
    /// Next modify calls fail with this (consumed one per call).
    fail_with: Mutex<Vec<Error>>,
    calls: Mutex<Vec<String>>,
}

impl Fake {
    fn failing(errors: Vec<Error>) -> Fake {
        Fake {
            fail_with: Mutex::new(errors),
            ..Fake::default()
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl SnoozeApi for Fake {
    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        self.calls.lock().unwrap().push(format!(
            "modify {thread_id} +{} -{}",
            add.join(","),
            remove.join(",")
        ));
        let mut fails = self.fail_with.lock().unwrap();
        if fails.is_empty() {
            Ok(())
        } else {
            Err(fails.remove(0))
        }
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    for id in [A, B] {
        s.upsert_account(&Account {
            id: id.into(),
            email: id.into(),
            display_name: None,
            nickname: None,
            color: "#000000".into(),
            added_at: 0,
            ..Account::default()
        })
        .unwrap();
    }
    s
}

fn msg(account: &str, id: &str, thread: &str, date: i64, labels: &[&str]) -> Message {
    Message {
        account_id: account.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: Address {
            name: None,
            email: "cy@quill.example".into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: format!("About {thread}"),
        snippet: String::new(),
        body_text: String::new(),
        body_html: None,
        label_ids: labels.iter().map(|l| l.to_string()).collect(),
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: false,
    }
}

/// The app's snooze: record first, then the archive.
fn snooze(st: &Store, account: &str, thread: &str, until: i64, now: i64) {
    st.snooze_threads(&[(account.into(), thread.into())], until, now)
        .unwrap();
    st.modify_thread_labels(account, thread, &[], &["INBOX".into()])
        .unwrap();
}

fn inbox(st: &Store) -> Vec<(String, bool)> {
    st.list_threads(&ListQuery {
        view: MailboxView::Inbox,
        tab: None,
        account_id: None,
        account_ids: None,
        limit: 50,
        before: None,
        unread_only: false,
        split: None,
    })
    .unwrap()
    .into_iter()
    .map(|t| (t.thread_id, t.unread))
    .collect()
}

#[test]
fn until_must_be_future_and_within_a_year() {
    assert!(check_until(0, 0).is_err());
    assert!(check_until(1, 0).is_ok());
    assert!(check_until(MAX_SNOOZE_AHEAD_MS, 0).is_ok());
    assert!(check_until(MAX_SNOOZE_AHEAD_MS + 1, 0).is_err());
}

#[tokio::test]
async fn wakes_on_time_locally_then_on_gmail() {
    let st = store();
    st.upsert_messages(&[
        msg(A, "m1", "t1", 0, &["INBOX"]),
        msg(A, "m2", "t2", MIN, &["INBOX"]),
    ])
    .unwrap();
    snooze(&st, A, "t1", 60 * MIN, 2 * MIN);
    let c = HashMap::from([(A.to_string(), Fake::default())]);

    assert!(run_due(&st, &c, 60 * MIN - 1).await.unwrap().is_empty());
    assert!(c[A].calls().is_empty());
    assert_eq!(
        due_accounts(&st, 60 * MIN - 1).await.unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        due_accounts(&st, 60 * MIN).await.unwrap(),
        vec![A.to_string()]
    );

    let woke = run_due(&st, &c, 61 * MIN).await.unwrap();
    assert_eq!(
        woke,
        vec![Woke {
            account_id: A.into(),
            thread_id: "t1".into(),
            subject: "About t1".into(),
            due_at: 60 * MIN,
        }]
    );
    assert_eq!(c[A].calls(), vec!["modify t1 +INBOX,UNREAD -"]);
    // Top of the inbox, unread; nothing left to do.
    assert_eq!(inbox(&st), vec![("t1".into(), true), ("t2".into(), false)]);
    assert!(st.due_snoozes(i64::MAX).unwrap().is_empty());
    assert!(run_due(&st, &c, 90 * MIN).await.unwrap().is_empty());
    assert_eq!(c[A].calls().len(), 1);
}

#[tokio::test]
async fn offline_or_signed_out_wakes_locally_and_retries_the_push() {
    let st = store();
    st.upsert_messages(&[
        msg(A, "a1", "ta", 0, &["INBOX"]),
        msg(B, "b1", "tb", 0, &["INBOX"]),
    ])
    .unwrap();
    snooze(&st, A, "ta", 10 * MIN, 0);
    snooze(&st, B, "tb", 10 * MIN, 0);
    // A is offline once; B has no client (signed out).
    let c = HashMap::from([(
        A.to_string(),
        Fake::failing(vec![Error::Network("offline".into())]),
    )]);

    let woke = run_due(&st, &c, 10 * MIN).await.unwrap();
    assert_eq!(woke.len(), 2, "both woke locally");
    assert_eq!(inbox(&st).len(), 2);
    // Each push is owed again in PUSH_RETRY_MS, not before.
    assert!(run_due(&st, &c, 10 * MIN + PUSH_RETRY_MS - 1)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(c[A].calls().len(), 1);
    let again = run_due(&st, &c, 10 * MIN + PUSH_RETRY_MS).await.unwrap();
    assert!(again.is_empty(), "a retried push doesn't wake it twice");
    assert_eq!(c[A].calls().len(), 2);
    // A is done; B still owes its push until it has a client.
    let owed: Vec<String> = st
        .due_snoozes(i64::MAX)
        .unwrap()
        .into_iter()
        .map(|d| d.account_id)
        .collect();
    assert_eq!(owed, vec![B.to_string()]);
}

#[tokio::test]
async fn deleted_on_gmail_drops_the_snooze() {
    let st = store();
    st.upsert_messages(&[msg(A, "m1", "t1", 0, &["INBOX"])])
        .unwrap();
    snooze(&st, A, "t1", MIN, 0);
    let c = HashMap::from([(
        A.to_string(),
        Fake::failing(vec![Error::Http {
            status: 404,
            body: "not found".into(),
        }]),
    )]);
    run_due(&st, &c, MIN).await.unwrap();
    assert!(st.due_snoozes(i64::MAX).unwrap().is_empty());
    assert!(run_due(&st, &c, 10 * MIN).await.unwrap().is_empty());
    assert_eq!(c[A].calls().len(), 1);
}

#[tokio::test]
async fn a_reply_before_wake_time_unsnoozes_without_a_second_wake() {
    let st = store();
    st.upsert_messages(&[msg(A, "m1", "t1", 0, &["INBOX"])])
        .unwrap();
    snooze(&st, A, "t1", 60 * MIN, 0);
    // Sync stores the reply (in the inbox, unread).
    st.upsert_messages(&[msg(A, "m2", "t1", 5 * MIN, &["INBOX", "UNREAD"])])
        .unwrap();
    let c = HashMap::from([(A.to_string(), Fake::default())]);
    assert!(run_due(&st, &c, 60 * MIN).await.unwrap().is_empty());
    assert!(c[A].calls().is_empty());
    assert_eq!(inbox(&st), vec![("t1".into(), true)]);
}

#[tokio::test]
async fn catches_up_on_everything_missed_while_closed() {
    let st = store();
    st.upsert_messages(&[
        msg(A, "m1", "t1", 0, &["INBOX"]),
        msg(A, "m2", "t2", 0, &["INBOX"]),
        msg(A, "m3", "t3", 0, &["INBOX"]),
    ])
    .unwrap();
    snooze(&st, A, "t1", 10 * MIN, 0);
    snooze(&st, A, "t2", 20 * MIN, 0);
    snooze(&st, A, "t3", 24 * 60 * MIN, 0);
    let c = HashMap::from([(A.to_string(), Fake::default())]);
    // First pass after launch, hours later.
    let woke = run_due(&st, &c, 5 * 60 * MIN).await.unwrap();
    let ids: Vec<&str> = woke.iter().map(|w| w.thread_id.as_str()).collect();
    assert_eq!(ids, vec!["t1", "t2"]);
    assert_eq!(st.list_snoozes(None).unwrap().len(), 1);
    assert_eq!(st.next_snooze_due().unwrap(), Some(24 * 60 * MIN));
}
