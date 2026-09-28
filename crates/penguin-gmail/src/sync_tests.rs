//! Sync logic against a fake Gmail and an in-memory Store.

use super::*;
use penguin_core::Address;

const ACCT: &str = "me@penguin.example";

fn msg(id: &str, thread: &str, date: i64, labels: &[&str]) -> Message {
    Message {
        account_id: ACCT.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: Address {
            name: Some("Sam Sender".into()),
            email: "sam@acme.example".into(),
        },
        to: vec![Address {
            name: None,
            email: ACCT.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: format!("Subject {id}"),
        snippet: format!("snippet {id}"),
        body_text: format!("body of {id}"),
        body_html: None,
        label_ids: labels.iter().map(|s| s.to_string()).collect(),
        attachments: vec![],
        message_id_header: Some(format!("{id}@acme.example")),
        in_reply_to: None,
        references: vec![],
        sender_authenticated: false,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
    }
}

fn label(id: &str) -> Label {
    Label {
        account_id: ACCT.into(),
        id: id.into(),
        name: id.into(),
        kind: "system".into(),
        color: None,
        unread_count: None,
        hidden: false,
    }
}

#[derive(Default)]
struct FakeState {
    /// Server mailbox, newest first.
    messages: Vec<Message>,
    history_id: u64,
    /// Pages served by list_history (token "h<i>").
    history: Vec<HistoryPage>,
    history_expired: bool,
    page_size: usize,
    fail_next_get: bool,
    reject_page_tokens: bool,
    needs_reauth: bool,
    keychain_denied: bool,
    labels: Vec<Label>,
    calls: Vec<String>,
    fetched: Vec<String>,
    /// Per-message HTTP status to fail with (a "bad" message).
    bad: HashMap<String, u16>,
    /// Fetch attempts per message id.
    attempts: HashMap<String, u32>,
}

#[derive(Clone)]
struct Fake(Arc<Mutex<FakeState>>);

impl Fake {
    fn new(messages: Vec<Message>, page_size: usize) -> Self {
        Fake(Arc::new(Mutex::new(FakeState {
            messages,
            history_id: 100,
            page_size,
            labels: ["INBOX", "UNREAD", "STARRED", "TRASH"]
                .into_iter()
                .map(label)
                .collect(),
            ..Default::default()
        })))
    }
    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.0.lock().unwrap()
    }
}

impl GmailApi for Fake {
    async fn profile(&self) -> Result<Profile> {
        let mut s = self.st();
        s.calls.push("profile".into());
        if s.needs_reauth {
            return Err(Error::NeedsReauth("invalid_grant".into()));
        }
        if s.keychain_denied {
            return Err(Error::Keychain("User canceled the operation.".into()));
        }
        Ok(Profile {
            email: ACCT.into(),
            messages_total: s.messages.len() as u64,
            history_id: s.history_id,
        })
    }

    async fn list_message_ids(&self, page_token: Option<&str>, q: Option<&str>) -> Result<IdPage> {
        let mut s = self.st();
        s.calls.push(format!(
            "list:{}:{}",
            page_token.unwrap_or("-"),
            q.unwrap_or("-")
        ));
        if page_token.is_some() && s.reject_page_tokens {
            return Err(Error::Http {
                status: 400,
                body: "Invalid pageToken".into(),
            });
        }
        let start: usize = page_token
            .map(|t| t.trim_start_matches('p').parse().unwrap())
            .unwrap_or(0);
        let end = (start + s.page_size).min(s.messages.len());
        Ok(IdPage {
            ids: s.messages[start..end]
                .iter()
                .map(|m| (m.id.clone(), m.thread_id.clone()))
                .collect(),
            next_page_token: (end < s.messages.len()).then(|| format!("p{end}")),
            result_size_estimate: s.messages.len() as u64,
        })
    }

    async fn get_messages_each(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        let mut s = self.st();
        // Transport-level failure: hits every id, not any one message.
        let network_down = std::mem::take(&mut s.fail_next_get);
        ids.iter()
            .map(|id| {
                *s.attempts.entry(id.clone()).or_default() += 1;
                let r = if network_down {
                    Err(Error::Network("connection reset".into()))
                } else if let Some(&status) = s.bad.get(id) {
                    Err(Error::Http {
                        status,
                        body: "backendError".into(),
                    })
                } else {
                    s.fetched.push(id.clone());
                    Ok(s.messages.iter().find(|m| &m.id == id).cloned())
                };
                (id.clone(), r)
            })
            .collect()
    }

    async fn get_message_labels(&self, ids: &[String]) -> Result<Vec<MessageLabels>> {
        let s = self.st();
        Ok(ids
            .iter()
            .filter_map(|id| s.messages.iter().find(|m| &m.id == id))
            .map(|m| MessageLabels {
                id: m.id.clone(),
                thread_id: m.thread_id.clone(),
                label_ids: m.label_ids.clone(),
            })
            .collect())
    }

    async fn list_history(&self, start: u64, page_token: Option<&str>) -> Result<HistoryPage> {
        let mut s = self.st();
        s.calls
            .push(format!("history:{start}:{}", page_token.unwrap_or("-")));
        if s.history_expired {
            return Err(Error::HistoryExpired);
        }
        let i: usize = page_token
            .map(|t| t.trim_start_matches('h').parse().unwrap())
            .unwrap_or(0);
        match s.history.get(i) {
            None => Ok(HistoryPage {
                history_id: s.history_id,
                ..Default::default()
            }),
            Some(p) => {
                let mut p = p.clone();
                p.history_id = s.history_id;
                p.next_page_token = (i + 1 < s.history.len()).then(|| format!("h{}", i + 1));
                Ok(p)
            }
        }
    }

    async fn list_labels(&self) -> Result<Vec<Label>> {
        let mut s = self.st();
        s.calls.push("labels".into());
        Ok(s.labels.clone())
    }

    async fn label_messages_total(&self, label_id: &str) -> Result<u64> {
        let s = self.st();
        Ok(s.messages
            .iter()
            .filter(|m| m.label_ids.iter().any(|l| l == label_id))
            .count() as u64)
    }
}

#[derive(Default)]
struct Recorder {
    statuses: Mutex<Vec<SyncStatus>>,
    changed: Mutex<Vec<String>>,
    /// messages_added / labels_added (the rules engine's triggers).
    added: Mutex<Vec<String>>,
    labels_added: Mutex<Vec<(String, Vec<String>)>>,
}

impl SyncObserver for Recorder {
    fn status(&self, status: SyncStatus) {
        self.statuses.lock().unwrap().push(status);
    }
    fn mail_changed(&self, _account_id: &str, thread_ids: Vec<String>) {
        self.changed.lock().unwrap().extend(thread_ids);
    }
    fn messages_added(&self, _account_id: &str, message_ids: Vec<String>) {
        self.added.lock().unwrap().extend(message_ids);
    }
    fn labels_added(&self, _account_id: &str, changes: Vec<(String, Vec<String>)>) {
        self.labels_added.lock().unwrap().extend(changes);
    }
}

struct Harness {
    fake: Fake,
    store: Store,
    rec: Arc<Recorder>,
    shared: Arc<Shared>,
}

impl Harness {
    fn new(messages: Vec<Message>, page_size: usize) -> Self {
        let rec = Arc::new(Recorder::default());
        Harness {
            fake: Fake::new(messages, page_size),
            store: Store::open_in_memory().unwrap(),
            shared: Shared::new(ACCT, rec.clone()),
            rec,
        }
    }
    /// A fresh AccountSync, as after an app restart.
    fn sync(&self) -> AccountSync<Fake> {
        AccountSync::new(
            ACCT,
            self.fake.clone(),
            self.store.clone(),
            self.shared.clone(),
        )
    }
    fn cursor(&self) -> SyncCursor {
        self.store.get_sync_cursor(ACCT).unwrap()
    }
    /// Gmail's part of the saved cursor (`provider_state`).
    fn gmail(&self) -> GmailCursor {
        GmailCursor::parse(&self.cursor().provider_state).unwrap()
    }
    fn count(&self) -> u64 {
        self.store.count_messages(Some(ACCT)).unwrap()
    }
    fn labels_of(&self, id: &str) -> Vec<String> {
        let mut l = self.store.get_message(ACCT, id).unwrap().unwrap().label_ids;
        l.sort();
        l
    }
}

/// m7 (newest) … m1 (oldest), one thread each.
fn mailbox(n: usize) -> Vec<Message> {
    (1..=n)
        .rev()
        .map(|i| {
            msg(
                &format!("m{i}"),
                &format!("t{i}"),
                i as i64 * 1000,
                &["INBOX"],
            )
        })
        .collect()
}

#[tokio::test]
async fn backfill_records_history_first_skips_known_and_resumes() {
    let h = Harness::new(mailbox(7), 3);
    let mut sync = h.sync();
    sync.init().await.unwrap();

    // History id recorded before any listing happened.
    assert_eq!(h.gmail().history_id, Some(100));
    assert!(!h.fake.st().calls.iter().any(|c| c.starts_with("list:")));

    sync.backfill_page().await.unwrap();
    assert_eq!(h.count(), 3);
    assert_eq!(h.gmail().backfill_page_token.as_deref(), Some("p3"));

    // m4 arrives locally some other way (e.g. history); backfill must skip it.
    h.store
        .upsert_messages(&[msg("m4", "t4", 4000, &["INBOX"])])
        .unwrap();

    // Restart; the next page's fetch fails → cursor must not advance.
    drop(sync);
    h.fake.st().fail_next_get = true;
    let mut sync = h.sync();
    sync.init().await.unwrap();
    assert!(sync.backfill_page().await.is_err());
    assert_eq!(h.gmail().backfill_page_token.as_deref(), Some("p3"));
    assert_eq!(h.count(), 4);

    sync.backfill_page().await.unwrap();
    assert_eq!(h.gmail().backfill_page_token.as_deref(), Some("p6"));
    sync.backfill_page().await.unwrap();
    let c = h.cursor();
    assert!(c.backfill_done);
    assert_eq!(h.gmail().backfill_page_token, None);
    assert_eq!(h.gmail().history_id, Some(100));
    assert_eq!(h.count(), 7);

    let fetched = h.fake.st().fetched.clone();
    assert!(
        !fetched.contains(&"m4".to_string()),
        "known id refetched: {fetched:?}"
    );
    for id in ["m7", "m6", "m5"] {
        assert_eq!(
            fetched.iter().filter(|f| *f == id).count(),
            1,
            "{id} fetched more than once"
        );
    }
    let statuses = h.rec.statuses.lock().unwrap();
    assert!(statuses.iter().any(|s| s.phase == SyncPhase::Backfilling));
    let last = statuses.last().unwrap();
    assert_eq!(
        (last.phase, last.indexed, last.total_estimate),
        (SyncPhase::Idle, 7, Some(7))
    );
    assert!(h.rec.changed.lock().unwrap().contains(&"t7".to_string()));
    // Backfill is never "new mail" for rules.
    assert!(h.rec.added.lock().unwrap().is_empty());
}

#[tokio::test]
async fn incremental_applies_adds_deletes_and_label_changes() {
    let h = Harness::new(mailbox(2), 10);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    sync.backfill_page().await.unwrap();
    assert!(h.cursor().backfill_done);
    h.store
        .modify_message_labels(ACCT, &["m2".into()], &["UNREAD".into()], &[])
        .unwrap();

    {
        let mut s = h.fake.st();
        s.messages
            .insert(0, msg("m3", "t3", 3000, &["INBOX", "UNREAD", "Label_9"]));
        s.messages.retain(|m| m.id != "m1");
        s.history_id = 150;
        s.labels.push(label("Label_9"));
        s.history = vec![
            HistoryPage {
                added: vec!["m3".into()],
                message_threads: HashMap::from([("m3".into(), "t3".into())]),
                ..Default::default()
            },
            HistoryPage {
                deleted: vec!["m1".into()],
                labels_removed: vec![("m2".into(), vec!["UNREAD".into()])],
                labels_added: vec![("m2".into(), vec!["STARRED".into()])],
                message_threads: HashMap::from([
                    ("m1".into(), "t1".into()),
                    ("m2".into(), "t2".into()),
                ]),
                ..Default::default()
            },
        ];
    }
    h.rec.changed.lock().unwrap().clear();
    let label_calls_before = h.fake.st().calls.iter().filter(|c| *c == "labels").count();

    sync.poll_history().await.unwrap();

    assert!(h.store.get_message(ACCT, "m1").unwrap().is_none());
    assert!(h.store.get_message(ACCT, "m3").unwrap().is_some());
    assert_eq!(h.labels_of("m2"), vec!["INBOX", "STARRED"]);
    assert_eq!(h.gmail().history_id, Some(150));
    let mut changed = h.rec.changed.lock().unwrap().clone();
    changed.sort();
    changed.dedup();
    assert_eq!(changed, vec!["t1", "t2", "t3"]);
    // Rules triggers: only history's new message, and labels added (not removed).
    assert_eq!(*h.rec.added.lock().unwrap(), vec!["m3"]);
    assert_eq!(
        *h.rec.labels_added.lock().unwrap(),
        vec![("m2".to_string(), vec!["STARRED".to_string()])]
    );
    // Unknown label on new mail → label list refreshed.
    assert!(h.fake.st().calls.iter().filter(|c| *c == "labels").count() > label_calls_before);
    // Both history pages requested from the same start id.
    let calls = h.fake.st().calls.clone();
    assert!(
        calls.contains(&"history:100:-".to_string())
            && calls.contains(&"history:100:h1".to_string())
    );
}

#[tokio::test]
async fn history_expired_reanchors_and_reconciles_without_wiping() {
    let h = Harness::new(mailbox(3), 10);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    sync.backfill_page().await.unwrap();
    // A local message the server no longer lists: must survive.
    h.store
        .upsert_messages(&[msg("old", "t-old", 10, &["INBOX"])])
        .unwrap();

    {
        let mut s = h.fake.st();
        s.history_expired = true;
        s.history_id = 999;
        s.messages
            .insert(0, msg("m4", "t4", 4000, &["INBOX", "UNREAD"]));
        let m2 = s.messages.iter_mut().find(|m| m.id == "m2").unwrap();
        m2.label_ids = vec!["TRASH".into()];
    }
    sync.poll_history().await.unwrap();

    assert_eq!(h.gmail().history_id, Some(999));
    assert!(h.store.get_message(ACCT, "m4").unwrap().is_some());
    assert_eq!(h.labels_of("m2"), vec!["TRASH"]);
    assert!(
        h.store.get_message(ACCT, "old").unwrap().is_some(),
        "local data wiped"
    );
    assert_eq!(h.count(), 5);
    let calls = h.fake.st().calls.clone();
    assert!(
        calls.contains(&"list:-:in:anywhere".to_string()),
        "{calls:?}"
    );
    // Re-anchor (profile) happened before the reconcile listing.
    let expired_at = calls.iter().position(|c| c == "history:100:-").unwrap();
    let profile_at = calls
        .iter()
        .skip(expired_at)
        .position(|c| c == "profile")
        .unwrap()
        + expired_at;
    let list_at = calls
        .iter()
        .position(|c| c == "list:-:in:anywhere")
        .unwrap();
    assert!(profile_at < list_at);
    let last = h.rec.statuses.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.phase, SyncPhase::Idle);
    assert!(last.last_synced_at.is_some());
}

#[tokio::test]
async fn stale_page_token_restarts_listing() {
    let h = Harness::new(mailbox(5), 2);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    sync.backfill_page().await.unwrap();
    assert_eq!(h.gmail().backfill_page_token.as_deref(), Some("p2"));

    h.fake.st().reject_page_tokens = true;
    sync.backfill_page().await.unwrap();
    assert_eq!(h.gmail().backfill_page_token, None);
    assert!(!h.cursor().backfill_done);
    h.fake.st().reject_page_tokens = false;

    while !h.cursor().backfill_done {
        sync.backfill_page().await.unwrap();
    }
    assert_eq!(h.count(), 5);
    let fetched = h.fake.st().fetched.clone();
    assert_eq!(fetched.iter().filter(|f| *f == "m5").count(), 1);
}

#[tokio::test]
async fn needs_reauth_stops_the_account() {
    let h = Harness::new(mailbox(1), 10);
    h.fake.st().needs_reauth = true;
    // run() must return (not loop) and leave the account in NeedsReauth.
    tokio::time::timeout(Duration::from_secs(5), h.sync().run())
        .await
        .expect("run returned");
    let last = h.shared.snapshot();
    assert_eq!(last.phase, SyncPhase::NeedsReauth);
    assert!(last.error.is_some());
}

#[tokio::test(start_paused = true)]
async fn transient_errors_back_off_then_recover() {
    let h = Harness::new(mailbox(3), 10);
    h.fake.st().fail_next_get = true;
    let task = tokio::spawn(h.sync().run());
    // Let the first attempt fail and the retry (after backoff) succeed.
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if h.shared.snapshot().phase == SyncPhase::Idle && h.count() == 3 {
            break;
        }
    }
    task.abort();
    assert_eq!(h.count(), 3);
    let statuses = h.rec.statuses.lock().unwrap();
    assert!(statuses.iter().any(|s| s.phase == SyncPhase::Error));
    assert_eq!(statuses.last().unwrap().error, None);
}

fn parked(h: &Harness) -> Vec<String> {
    h.cursor().failed_message_ids
}

#[tokio::test(start_paused = true)]
async fn bad_message_is_parked_and_backfill_continues_then_retry_succeeds() {
    let h = Harness::new(mailbox(6), 3);
    h.fake.st().bad.insert("m5".into(), 500);
    let mut sync = h.sync();
    sync.failed_retry_interval = Duration::ZERO;
    sync.init().await.unwrap();

    // Page 1 = m6, m5, m4: m5 fails every attempt and is parked.
    sync.backfill_page().await.unwrap();
    assert_eq!(h.count(), 2);
    assert_eq!(h.gmail().backfill_page_token.as_deref(), Some("p3"));
    assert_eq!(parked(&h), vec!["m5"]);
    assert_eq!(h.fake.st().attempts["m5"], MESSAGE_ATTEMPTS);
    let s = h.shared.snapshot();
    assert_eq!(s.phase, SyncPhase::Backfilling);
    assert_eq!(
        s.error.as_deref(),
        Some("1 message couldn't be downloaded yet; retrying")
    );

    // The rest of the mailbox is unaffected, and backfill doesn't re-try m5.
    sync.backfill_page().await.unwrap();
    assert!(h.cursor().backfill_done);
    assert_eq!(h.count(), 5);
    assert_eq!(h.fake.st().attempts["m5"], MESSAGE_ATTEMPTS);
    assert_eq!(h.shared.snapshot().phase, SyncPhase::Idle);

    // Still broken on the next cycle: stays parked, account stays healthy.
    sync.poll_history().await.unwrap();
    assert_eq!(parked(&h), vec!["m5"]);
    assert_eq!(h.fake.st().attempts["m5"], MESSAGE_ATTEMPTS + 1);
    let s = h.shared.snapshot();
    assert_eq!(s.phase, SyncPhase::Idle);
    assert!(s.error.is_some());

    // Gmail recovers: the next cycle downloads it and clears the note.
    h.fake.st().bad.clear();
    h.rec.changed.lock().unwrap().clear();
    sync.poll_history().await.unwrap();
    assert!(h.store.get_message(ACCT, "m5").unwrap().is_some());
    assert!(parked(&h).is_empty());
    assert_eq!(h.count(), 6);
    let s = h.shared.snapshot();
    assert_eq!((s.phase, s.error, s.indexed), (SyncPhase::Idle, None, 6));
    assert!(h.rec.changed.lock().unwrap().contains(&"t5".to_string()));
}

#[tokio::test(start_paused = true)]
async fn parked_message_that_disappears_is_dropped() {
    let h = Harness::new(mailbox(3), 10);
    h.fake.st().bad.insert("m2".into(), 400);
    let mut sync = h.sync();
    sync.failed_retry_interval = Duration::ZERO;
    sync.init().await.unwrap();
    sync.backfill_page().await.unwrap();
    assert_eq!(parked(&h), vec!["m2"]);

    // Deleted on the server → 404 on retry → no longer tracked.
    {
        let mut s = h.fake.st();
        s.bad.clear();
        s.messages.retain(|m| m.id != "m2");
    }
    sync.poll_history().await.unwrap();
    assert!(parked(&h).is_empty());
    assert_eq!(h.shared.snapshot().error, None);
}

#[tokio::test(start_paused = true)]
async fn retries_are_spaced_by_the_retry_interval() {
    let h = Harness::new(mailbox(2), 10);
    h.fake.st().bad.insert("m1".into(), 500);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    sync.backfill_page().await.unwrap();
    assert_eq!(parked(&h), vec!["m1"]);

    sync.poll_history().await.unwrap();
    sync.poll_history().await.unwrap();
    // One retry pass, then nothing until the interval has passed.
    assert_eq!(h.fake.st().attempts["m1"], MESSAGE_ATTEMPTS + 1);
    tokio::time::advance(FAILED_RETRY_INTERVAL).await;
    sync.poll_history().await.unwrap();
    assert_eq!(h.fake.st().attempts["m1"], MESSAGE_ATTEMPTS + 2);
}

#[tokio::test(start_paused = true)]
async fn whole_batch_failing_is_treated_as_gmail_trouble_not_bad_messages() {
    let h = Harness::new(mailbox(4), 10);
    for id in ["m1", "m2", "m3", "m4"] {
        h.fake.st().bad.insert(id.into(), 503);
    }
    let mut sync = h.sync();
    sync.init().await.unwrap();
    assert!(sync.backfill_page().await.is_err());
    assert!(parked(&h).is_empty());
    assert!(!h.cursor().backfill_done);
    assert_eq!(h.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn bad_message_from_history_is_parked_and_history_advances() {
    let h = Harness::new(mailbox(1), 10);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    sync.backfill_page().await.unwrap();
    {
        let mut s = h.fake.st();
        s.messages.insert(0, msg("m9", "t9", 9000, &["INBOX"]));
        s.messages.insert(0, msg("m10", "t10", 10000, &["INBOX"]));
        s.bad.insert("m9".into(), 400);
        s.history_id = 200;
        s.history = vec![HistoryPage {
            added: vec!["m10".into(), "m9".into()],
            ..Default::default()
        }];
    }
    sync.poll_history().await.unwrap();
    let c = h.cursor();
    assert_eq!(h.gmail().history_id, Some(200));
    assert_eq!(c.failed_message_ids, vec!["m9"]);
    assert!(h.store.get_message(ACCT, "m10").unwrap().is_some());
    let s = h.shared.snapshot();
    assert_eq!(s.phase, SyncPhase::Idle);
    assert!(s.error.unwrap().starts_with("1 message"));
}

#[test]
fn eta_from_rate() {
    assert_eq!(eta_secs(Some(10_000), 4_000, Some(1_200.0)), Some(300));
    assert_eq!(eta_secs(Some(10_000), 12_000, Some(1_200.0)), Some(0));
    assert_eq!(eta_secs(None, 0, Some(1_200.0)), None);
    assert_eq!(eta_secs(Some(10), 0, Some(0.0)), None);
    assert_eq!(eta_secs(Some(10), 0, None), None);
}

#[test]
fn rate_estimator_waits_then_smooths() {
    let t0 = Instant::now();
    let at = |secs: u64| t0 + Duration::from_secs(secs);
    let mut r = RateEstimator::new(t0);
    // Nothing for the first 15 s.
    assert_eq!(r.record(at(5), 100), None);
    assert_eq!(r.record(at(10), 100), None);
    // 300 stored over 15 s = 1,200/min.
    assert_eq!(r.record(at(15), 100), Some(1_200.0));
    // Steady 100 per 5 s keeps it at 1,200/min.
    let mut last = None;
    for i in 4..=12 {
        last = r.record(at(i * 5), 100);
    }
    assert!((last.unwrap() - 1_200.0).abs() < 1e-6, "{last:?}");
    // A sudden stall-then-burst chunk moves it only partway (EMA)...
    let spiky = r.record(at(61), 400).unwrap();
    assert!(spiky > 1_200.0 && spiky < 1_500.0, "{spiky}");
    // ...and old samples age out of the 60 s window.
    let after_gap = r.record(at(200), 0).unwrap();
    assert!(after_gap < spiky);
}

#[tokio::test(start_paused = true)]
async fn backfill_reports_rate_and_eta_then_clears_them() {
    let h = Harness::new(mailbox(6), 3);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    tokio::time::advance(Duration::from_secs(30)).await;
    sync.backfill_page().await.unwrap();
    let s = h.shared.snapshot();
    // 3 messages over 30 s of measurement = 6/min; 3 left → 30 s.
    assert_eq!(s.rate_per_min, Some(6.0));
    assert_eq!(s.eta_secs, Some(30));
    sync.backfill_page().await.unwrap();
    let s = h.shared.snapshot();
    assert_eq!(
        (s.phase, s.rate_per_min, s.eta_secs),
        (SyncPhase::Idle, None, None)
    );
}

#[tokio::test]
async fn total_estimate_excludes_spam_and_trash() {
    let mut messages = mailbox(4);
    messages.push(msg("s1", "ts1", 1, &["SPAM"]));
    messages.push(msg("x1", "tx1", 2, &["TRASH"]));
    messages.push(msg("x2", "tx2", 3, &["TRASH"]));
    let h = Harness::new(messages, 10);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    // Profile says 7; backfill can only ever reach the 4 non-spam/trash.
    assert_eq!(h.shared.snapshot().total_estimate, Some(4));
}

#[tokio::test(start_paused = true)]
async fn keychain_denial_stops_instead_of_reprompting() {
    let h = Harness::new(mailbox(1), 10);
    h.fake.st().keychain_denied = true;
    tokio::time::timeout(Duration::from_secs(3600), h.sync().run())
        .await
        .expect("run returned");
    let s = h.shared.snapshot();
    assert_eq!(s.phase, SyncPhase::NeedsReauth);
    assert!(s.error.unwrap().contains("Keychain"));
    // One attempt, no timed retries (each would be another Keychain prompt).
    assert_eq!(
        h.fake.st().calls.iter().filter(|c| *c == "profile").count(),
        1
    );
}
