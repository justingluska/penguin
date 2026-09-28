//! Outbox against a fake Gmail, an in-memory store and a fake clock.

use std::collections::VecDeque;
use std::sync::Mutex;

use penguin_core::{Account, Address, Message};

use super::*;

const A: &str = "me@penguin.example";
const MIN: i64 = 60_000;

#[derive(Default)]
struct Fake {
    /// Scripted results for drafts.send, consumed in order (default: success).
    send_results: Mutex<VecDeque<Result<()>>>,
    modify_fails: Mutex<bool>,
    calls: Mutex<Vec<String>>,
}

impl Fake {
    fn script(&self, results: Vec<Result<()>>) {
        *self.send_results.lock().unwrap() = results.into();
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl OutboxApi for Fake {
    async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef> {
        self.calls.lock().unwrap().push(format!("send {draft_id}"));
        self.send_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))?;
        Ok(SentRef {
            message_id: format!("sent-{draft_id}"),
            thread_id: format!("t-{draft_id}"),
        })
    }
    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        _remove: &[String],
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("modify {thread_id} +{}", add.join(",")));
        if *self.modify_fails.lock().unwrap() {
            return Err(Error::Network("offline".into()));
        }
        Ok(())
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&Account {
        id: A.into(),
        email: A.into(),
        display_name: None,
        nickname: None,
        color: "#000000".into(),
        added_at: 0,
        ..Account::default()
    })
    .unwrap();
    s
}

fn clients(fake: Fake) -> HashMap<String, Fake> {
    HashMap::from([(A.to_string(), fake)])
}

fn msg(id: &str, thread: &str, date: i64, labels: &[&str], from: &str) -> Message {
    Message {
        account_id: A.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: Address {
            name: None,
            email: from.into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Proposal".into(),
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

#[tokio::test]
async fn sends_when_due_cleans_up_the_draft_and_arms_a_reminder() {
    let st = store();
    st.upsert_messages(&[msg("dm1", "t-d1", 0, &["DRAFT"], A)])
        .unwrap();
    st.set_draft(A, "d1", "dm1").unwrap();
    let s = schedule_send(&st, A, "d1", 10 * MIN, Some(60 * MIN), 0)
        .await
        .unwrap();
    let c = clients(Fake::default());

    // Not yet due.
    assert!(run_due(&st, &c, 10 * MIN - 1).await.unwrap().is_empty());
    assert!(c[A].calls().is_empty());

    let events = run_due(&st, &c, 10 * MIN).await.unwrap();
    assert_eq!(
        events,
        vec![OutboxEvent::Sent {
            account_id: A.into(),
            schedule_id: s.id,
            draft_id: "d1".into(),
            message_id: "sent-d1".into(),
            thread_id: "t-d1".into(),
            scheduled_for: 10 * MIN,
        }]
    );
    assert!(st.list_scheduled_sends(None).unwrap().is_empty());
    assert!(
        st.get_message(A, "dm1").unwrap().is_none(),
        "local draft copy removed"
    );
    assert_eq!(st.message_for_draft(A, "d1").unwrap(), None);
    let reminders = st.list_reminders(None).unwrap();
    assert_eq!(reminders.len(), 1);
    assert_eq!(
        (
            reminders[0].thread_id.as_str(),
            reminders[0].sent_at,
            reminders[0].remind_at
        ),
        ("t-d1", 10 * MIN, 70 * MIN)
    );
}

#[tokio::test]
async fn transient_failures_retry_with_backoff_and_report_once() {
    let st = store();
    schedule_send(&st, A, "d1", 0, None, 0).await.unwrap();
    let fake = Fake::default();
    fake.script(vec![
        Err(Error::Network("offline".into())),
        Err(Error::Http {
            status: 503,
            body: String::new(),
        }),
    ]);
    let c = clients(fake);

    let first = run_due(&st, &c, 0).await.unwrap();
    assert!(matches!(&first[..], [OutboxEvent::SendFailed { retry_at: Some(t), .. }] if *t == MIN));
    // Second failure: quiet, longer delay.
    assert!(run_due(&st, &c, MIN).await.unwrap().is_empty());
    let s = &st.list_scheduled_sends(None).unwrap()[0];
    assert_eq!((s.attempts, s.send_at), (2, MIN + 2 * MIN));
    assert!(s.last_error.is_some());
    // Third attempt succeeds.
    let done = run_due(&st, &c, 3 * MIN).await.unwrap();
    assert!(matches!(&done[..], [OutboxEvent::Sent { .. }]));
    assert_eq!(c[A].calls().len(), 3);
}

#[tokio::test]
async fn gone_draft_cancels_and_rejected_draft_fails_permanently() {
    let st = store();
    schedule_send(&st, A, "gone", 0, None, 0).await.unwrap();
    schedule_send(&st, A, "bad", 1, None, 0).await.unwrap();
    let fake = Fake::default();
    fake.script(vec![
        Err(Error::Http {
            status: 404,
            body: String::new(),
        }),
        Err(Error::Http {
            status: 400,
            body: "Invalid To header".into(),
        }),
    ]);
    let c = clients(fake);
    let events = run_due(&st, &c, 1).await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(events
        .iter()
        .any(|e| matches!(e, OutboxEvent::SendCancelled { draft_id, .. } if draft_id == "gone")));
    assert!(events.iter().any(|e| matches!(e, OutboxEvent::SendFailed { draft_id, retry_at: None, .. } if draft_id == "bad")));
    assert!(st.list_scheduled_sends(None).unwrap().is_empty());
}

#[tokio::test]
async fn overdue_sends_catch_up_in_order_at_launch() {
    let st = store();
    schedule_send(&st, A, "d2", 2 * MIN, None, 0).await.unwrap();
    schedule_send(&st, A, "d1", MIN, None, 0).await.unwrap();
    // The app was closed past both times: one pass sends both, soonest first.
    let c = clients(Fake::default());
    let events = run_due(&st, &c, 60 * MIN).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(c[A].calls(), vec!["send d1", "send d2"]);
}

#[tokio::test]
async fn disconnected_accounts_keep_their_schedule() {
    let st = store();
    schedule_send(&st, A, "d1", MIN, None, 0).await.unwrap();
    // Account not connected (signed out, or services not up yet at launch).
    let none: HashMap<String, Fake> = HashMap::new();
    let events = run_due(&st, &none, 60 * MIN).await.unwrap();
    assert!(
        matches!(&events[..], [OutboxEvent::SendFailed { retry_at: Some(t), .. }] if *t == 61 * MIN)
    );
    assert_eq!(
        st.list_scheduled_sends(None).unwrap().len(),
        1,
        "nothing dropped"
    );
    let c = clients(Fake::default());
    assert!(matches!(
        &run_due(&st, &c, 61 * MIN).await.unwrap()[..],
        [OutboxEvent::Sent { .. }]
    ));
}

#[tokio::test]
async fn rescheduling_a_draft_replaces_its_schedule() {
    let st = store();
    schedule_send(&st, A, "d1", 10 * MIN, None, 0)
        .await
        .unwrap();
    let s = schedule_send(&st, A, "d1", 20 * MIN, None, 0)
        .await
        .unwrap();
    assert_eq!(st.list_scheduled_sends(None).unwrap(), vec![s]);
    assert!(schedule_send(&st, A, "d1", 0, Some(0), 0).await.is_err());
}

#[tokio::test]
async fn reminder_resurfaces_only_without_a_reply() {
    let st = store();
    st.upsert_messages(&[
        msg("m1", "quiet", 0, &["SENT"], A),
        msg("m2", "answered", 0, &["SENT"], A),
        msg("m3", "answered", 5 * MIN, &["INBOX"], "bo@acme.example"),
    ])
    .unwrap();
    set_reminder(&st, A, "quiet", Some("m1"), 30 * MIN, 0)
        .await
        .unwrap();
    set_reminder(&st, A, "answered", Some("m2"), 30 * MIN, 0)
        .await
        .unwrap();
    let c = clients(Fake::default());

    let events = run_due(&st, &c, 30 * MIN).await.unwrap();
    assert!(
        matches!(&events[..], [OutboxEvent::ReminderFired { thread_id, .. }] if thread_id == "quiet")
    );
    assert_eq!(c[A].calls(), vec!["modify quiet +INBOX,UNREAD"]);
    let m = st.get_message(A, "m1").unwrap().unwrap();
    assert!(m.label_ids.contains(&"INBOX".to_string()) && m.is_unread());
    assert!(st.list_reminders(None).unwrap().is_empty(), "both resolved");
}

#[tokio::test]
async fn reminder_retries_when_gmail_is_unreachable() {
    let st = store();
    st.upsert_messages(&[msg("m1", "quiet", 0, &["SENT"], A)])
        .unwrap();
    set_reminder(&st, A, "quiet", None, 10 * MIN, 0)
        .await
        .unwrap();
    let fake = Fake::default();
    *fake.modify_fails.lock().unwrap() = true;
    let c = clients(fake);
    assert!(run_due(&st, &c, 10 * MIN).await.unwrap().is_empty());
    let r = &st.list_reminders(None).unwrap()[0];
    assert_eq!(r.remind_at, 15 * MIN);
    assert!(
        !st.get_message(A, "m1").unwrap().unwrap().is_unread(),
        "local state only changes once Gmail agrees"
    );

    *c[A].modify_fails.lock().unwrap() = false;
    assert_eq!(run_due(&st, &c, 15 * MIN).await.unwrap().len(), 1);
    // A second reminder on the same thread replaces the first.
    set_reminder(&st, A, "quiet", None, 99 * MIN, 20 * MIN)
        .await
        .unwrap();
    set_reminder(&st, A, "quiet", None, 98 * MIN, 20 * MIN)
        .await
        .unwrap();
    assert_eq!(st.list_reminders(None).unwrap().len(), 1);
    assert_eq!(
        due_accounts(&st, 98 * MIN).await.unwrap(),
        vec![A.to_string()]
    );
}

#[test]
fn events_serialize_for_the_ui() {
    let e = OutboxEvent::SendFailed {
        account_id: A.into(),
        schedule_id: "s".into(),
        draft_id: "d".into(),
        error: "offline".into(),
        retry_at: Some(5),
    };
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({"kind": "sendFailed", "accountId": A, "scheduleId": "s", "draftId": "d", "error": "offline", "retryAt": 5})
    );
}

#[tokio::test]
async fn schedule_times_are_validated_and_reminders_date_from_the_sent_message() {
    let st = store();
    let now = 1_000 * MIN;
    assert!(
        schedule_send(&st, A, "d", now - 2 * MIN, None, now)
            .await
            .is_err(),
        "past"
    );
    assert!(
        schedule_send(&st, A, "d", now - 30_000, None, now)
            .await
            .is_ok(),
        "clock skew is fine"
    );
    assert!(
        schedule_send(&st, A, "d", now + 400 * 24 * 60 * MIN, None, now)
            .await
            .is_err(),
        "> a year"
    );

    st.upsert_messages(&[msg("s1", "t", 900 * MIN, &["SENT"], A)])
        .unwrap();
    let r = set_reminder(&st, A, "t", Some("s1"), now + MIN, now)
        .await
        .unwrap();
    assert_eq!(
        (r.sent_at, r.sent_message_id.as_deref()),
        (900 * MIN, Some("s1"))
    );
    let unknown = set_reminder(&st, A, "t", Some("not-synced-yet"), now + MIN, now)
        .await
        .unwrap();
    assert_eq!(unknown.sent_at, now);
    assert_eq!(st.list_reminders(None).unwrap(), vec![unknown]);
}
