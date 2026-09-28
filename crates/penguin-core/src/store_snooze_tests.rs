//! Snooze records: the Snoozed view, waking, early wake and cleanup.

use crate::store::LocalWake;
use crate::types::*;
use crate::Store;

const A: &str = "ada@penguin.example";
const B: &str = "bea@penguin.example";
const HOUR: i64 = 3_600_000;
const T0: i64 = 1_760_000_000_000;

fn account(id: &str) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        display_name: None,
        nickname: None,
        color: "#123456".into(),
        added_at: 1,
        ..Account::default()
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A)).unwrap();
    s.upsert_account(&account(B)).unwrap();
    s
}

fn msg(account: &str, id: &str, thread: &str, date: i64, labels: &[&str]) -> Message {
    Message {
        account_id: account.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: Address {
            name: Some("Cy Quill".into()),
            email: "cy@quill.example".into(),
        },
        to: vec![Address {
            name: None,
            email: account.into(),
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

fn query(view: MailboxView, account: Option<&str>) -> ListQuery {
    ListQuery {
        view,
        tab: None,
        account_id: account.map(Into::into),
        account_ids: None,
        limit: 100,
        before: None,
        unread_only: false,
        split: None,
    }
}

fn ids(s: &Store, q: &ListQuery) -> Vec<String> {
    s.list_threads(q)
        .unwrap()
        .into_iter()
        .map(|t| t.thread_id)
        .collect()
}

fn inbox(s: &Store) -> Vec<String> {
    ids(s, &query(MailboxView::Inbox, None))
}

fn snoozed(s: &Store) -> Vec<String> {
    ids(s, &query(MailboxView::Snoozed, None))
}

/// What the app does: the record first, then the archive.
fn snooze(s: &Store, account: &str, thread: &str, until: i64, now: i64) {
    s.snooze_threads(&[(account.into(), thread.into())], until, now)
        .unwrap();
    s.modify_thread_labels(account, thread, &[], &["INBOX".into()])
        .unwrap();
}

fn labels(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn snoozing_moves_the_thread_from_inbox_to_snoozed() {
    let s = store();
    s.upsert_messages(&[
        msg(A, "m1", "t1", T0, &["INBOX", "UNREAD"]),
        msg(A, "m2", "t2", T0 + 1, &["INBOX"]),
    ])
    .unwrap();
    snooze(&s, A, "t1", T0 + 5 * HOUR, T0 + 2);
    assert_eq!(inbox(&s), vec!["t2"]);
    let listed = s.list_threads(&query(MailboxView::Snoozed, None)).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].thread_id, "t1");
    assert_eq!(listed[0].snoozed_until, Some(T0 + 5 * HOUR));
    assert!(listed[0].unread);
    // Other views say it's snoozed too; unsnoozed threads say nothing.
    let done = s.list_threads(&query(MailboxView::Done, None)).unwrap();
    assert_eq!(done[0].snoozed_until, Some(T0 + 5 * HOUR));
    let all = s.list_threads(&query(MailboxView::All, None)).unwrap();
    assert_eq!(
        all.iter()
            .find(|t| t.thread_id == "t2")
            .unwrap()
            .snoozed_until,
        None
    );
    assert_eq!(
        s.get_snooze(A, "t1").unwrap(),
        Some(Snooze {
            account_id: A.into(),
            thread_id: "t1".into(),
            wake_at: T0 + 5 * HOUR,
            snoozed_at: T0 + 2,
        })
    );
    // Re-snoozing replaces the time.
    snooze(&s, A, "t1", T0 + 9 * HOUR, T0 + 3);
    assert_eq!(s.list_snoozes(None).unwrap()[0].wake_at, T0 + 9 * HOUR);
    assert_eq!(s.next_snooze_due().unwrap(), Some(T0 + 9 * HOUR));
}

#[test]
fn snoozed_view_is_soonest_first_scoped_and_filterable() {
    let s = store();
    s.upsert_messages(&[
        msg(A, "a1", "ta1", T0, &["INBOX"]),
        msg(A, "a2", "ta2", T0 + 1, &["INBOX", "UNREAD"]),
        msg(B, "b1", "tb1", T0 + 2, &["INBOX"]),
    ])
    .unwrap();
    snooze(&s, A, "ta1", T0 + 3 * HOUR, T0);
    snooze(&s, A, "ta2", T0 + 9 * HOUR, T0);
    snooze(&s, B, "tb1", T0 + 1 * HOUR, T0);
    assert_eq!(snoozed(&s), vec!["tb1", "ta1", "ta2"]);
    assert_eq!(
        ids(&s, &query(MailboxView::Snoozed, Some(A))),
        vec!["ta1", "ta2"]
    );
    let mut q = query(MailboxView::Snoozed, None);
    q.account_ids = Some(vec![B.into()]);
    assert_eq!(ids(&s, &q), vec!["tb1"]);
    q.account_ids = Some(vec![]);
    assert!(ids(&s, &q).is_empty());
    let mut unread = query(MailboxView::Snoozed, None);
    unread.unread_only = true;
    assert_eq!(ids(&s, &unread), vec!["ta2"]);
    // One page holds them all: a next page is empty.
    let mut next = query(MailboxView::Snoozed, None);
    next.before = Some(T0);
    assert!(ids(&s, &next).is_empty());
    let scoped: Vec<String> = s
        .list_snoozes(Some(&[A.to_string()]))
        .unwrap()
        .into_iter()
        .map(|z| z.thread_id)
        .collect();
    assert_eq!(scoped, vec!["ta1", "ta2"]);
    assert!(s.list_snoozes(Some(&[])).unwrap().is_empty());
}

#[test]
fn waking_returns_the_thread_unread_at_the_top_of_the_inbox() {
    let s = store();
    s.upsert_messages(&[
        msg(A, "old", "t-old", T0, &["INBOX"]),
        msg(A, "new", "t-new", T0 + HOUR, &["INBOX"]),
    ])
    .unwrap();
    snooze(&s, A, "t-old", T0 + 5 * HOUR, T0 + 2 * HOUR);
    assert!(s.due_snoozes(T0 + 5 * HOUR - 1).unwrap().is_empty());
    let due = s.due_snoozes(T0 + 5 * HOUR).unwrap();
    assert_eq!(due.len(), 1);
    assert!(!due[0].woken);

    let wake_time = T0 + 6 * HOUR;
    let woke = s.wake_snooze(A, "t-old", wake_time).unwrap();
    assert_eq!(
        woke,
        LocalWake::Woke {
            subject: "Subject t-old".into()
        }
    );
    // Above newer mail, unread, out of Snoozed.
    assert_eq!(inbox(&s), vec!["t-old", "t-new"]);
    let top = &s.list_threads(&query(MailboxView::Inbox, None)).unwrap()[0];
    assert!(top.unread);
    assert_eq!(top.last_date, wake_time);
    assert_eq!(top.snoozed_until, None);
    assert!(snoozed(&s).is_empty());
    assert_eq!(s.get_snooze(A, "t-old").unwrap(), None);
    // The Gmail push is still owed; waking again is a no-op.
    let due = s.due_snoozes(wake_time).unwrap();
    assert!(due[0].woken);
    assert_eq!(
        s.wake_snooze(A, "t-old", wake_time).unwrap(),
        LocalWake::NotSnoozed
    );
    s.retry_snooze_push(A, "t-old", wake_time + HOUR).unwrap();
    assert!(s.due_snoozes(wake_time).unwrap().is_empty());
    assert_eq!(s.next_snooze_due().unwrap(), Some(wake_time + HOUR));
    s.snooze_pushed(A, "t-old").unwrap();
    assert!(s.due_snoozes(i64::MAX).unwrap().is_empty());
    assert_eq!(s.next_snooze_due().unwrap(), None);
    // The bump survives other changes while it stays in the inbox...
    s.modify_thread_labels(A, "t-old", &[], &labels(&["UNREAD"]))
        .unwrap();
    assert_eq!(inbox(&s), vec!["t-old", "t-new"]);
    // ...and ends when it leaves: back by date after that.
    s.modify_thread_labels(A, "t-old", &[], &labels(&["INBOX"]))
        .unwrap();
    s.modify_thread_labels(A, "t-old", &labels(&["INBOX"]), &[])
        .unwrap();
    assert_eq!(inbox(&s), vec!["t-new", "t-old"]);
}

#[test]
fn new_mail_in_the_thread_ends_the_snooze_early() {
    let s = store();
    s.upsert_messages(&[msg(A, "m1", "t1", T0, &["INBOX"])])
        .unwrap();
    snooze(&s, A, "t1", T0 + 24 * HOUR, T0 + HOUR);
    // My own reply (SENT, no INBOX) doesn't.
    s.upsert_messages(&[msg(A, "m2", "t1", T0 + 2 * HOUR, &["SENT"])])
        .unwrap();
    assert_eq!(snoozed(&s), vec!["t1"]);
    // A reply arriving in the inbox does.
    s.upsert_messages(&[msg(A, "m3", "t1", T0 + 3 * HOUR, &["INBOX", "UNREAD"])])
        .unwrap();
    assert!(snoozed(&s).is_empty());
    assert_eq!(inbox(&s), vec!["t1"]);
    assert!(s.due_snoozes(i64::MAX).unwrap().is_empty());
    assert_eq!(
        s.wake_snooze(A, "t1", T0 + 25 * HOUR).unwrap(),
        LocalWake::NotSnoozed
    );
}

#[test]
fn moving_to_inbox_trash_or_deleting_ends_the_snooze() {
    let s = store();
    s.upsert_messages(&[
        msg(A, "m1", "t1", T0, &["INBOX"]),
        msg(A, "m2", "t2", T0, &["INBOX"]),
        msg(A, "m3", "t3", T0, &["INBOX"]),
    ])
    .unwrap();
    for t in ["t1", "t2", "t3"] {
        snooze(&s, A, t, T0 + HOUR, T0);
    }
    s.modify_thread_labels(A, "t1", &labels(&["INBOX"]), &[])
        .unwrap();
    s.modify_thread_labels(A, "t2", &labels(&["TRASH"]), &[])
        .unwrap();
    s.delete_messages(A, &["m3".into()]).unwrap();
    assert!(snoozed(&s).is_empty());
    assert!(s.list_snoozes(None).unwrap().is_empty());
    assert!(s.due_snoozes(i64::MAX).unwrap().is_empty());
}

#[test]
fn waking_a_thread_that_is_gone_drops_the_snooze() {
    let s = store();
    s.snooze_threads(&[(A.into(), "never-synced".into())], T0, T0)
        .unwrap();
    assert_eq!(
        s.wake_snooze(A, "never-synced", T0).unwrap(),
        LocalWake::Dropped
    );
    assert!(s.due_snoozes(i64::MAX).unwrap().is_empty());
}

#[test]
fn unsnooze_and_account_removal_drop_records() {
    let s = store();
    s.upsert_messages(&[
        msg(A, "a1", "ta", T0, &["INBOX"]),
        msg(B, "b1", "tb", T0, &["INBOX"]),
    ])
    .unwrap();
    snooze(&s, A, "ta", T0 + HOUR, T0);
    snooze(&s, B, "tb", T0 + HOUR, T0);
    assert!(s.unsnooze(A, "ta").unwrap());
    assert!(!s.unsnooze(A, "ta").unwrap());
    s.remove_account(B).unwrap();
    assert!(s.list_snoozes(None).unwrap().is_empty());
}
