//! Sync window against a fake Gmail and an in-memory Store: stage order,
//! resumability, growing/shrinking the window, the body-pending lifecycle,
//! server search and query translation.

use std::sync::{Arc, Mutex};

use penguin_core::{
    Address, Label, Message, Store, SyncPhase, SyncStage, SyncStatus, WindowCursor,
};

use super::super::{AccountSync, GmailApi, Shared, SyncObserver};
use super::*;
use crate::api::{HistoryPage, IdPage, MessageLabels, Profile};

const ACCT: &str = "me@penguin.example";

fn now() -> i64 {
    super::super::now_ms()
}

fn days_ago(d: i64) -> i64 {
    now() - d * DAY_MS
}

fn msg(id: &str, date: i64) -> Message {
    Message {
        account_id: ACCT.into(),
        id: id.into(),
        thread_id: format!("t-{id}"),
        date,
        from: Address {
            name: Some("Cora Vance".into()),
            email: "cora@vance.example".into(),
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
        body_text: format!("body words {id}"),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        sender_authenticated: false,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
    }
}

/// `n` messages, one every `every_days`, newest first.
fn mailbox(n: usize, every_days: i64) -> Vec<Message> {
    (0..n)
        .map(|i| msg(&format!("m{i:03}"), days_ago(i as i64 * every_days) - 1000))
        .collect()
}

#[derive(Default)]
struct FakeState {
    messages: Vec<Message>,
    page_size: usize,
    history_id: u64,
    history: Vec<HistoryPage>,
    /// "list:<q>:<token>", "full:<id>", "hdr:<id>", "search:<q>".
    calls: Vec<String>,
}

#[derive(Clone)]
struct Fake(Arc<Mutex<FakeState>>);

fn parse_bound(q: &str, op: &str) -> Option<i64> {
    q.split_whitespace()
        .find_map(|p| p.strip_prefix(op))
        .and_then(|v| v.parse::<i64>().ok())
        .map(|s| s * 1000)
}

impl Fake {
    fn new(messages: Vec<Message>, page_size: usize) -> Self {
        Fake(Arc::new(Mutex::new(FakeState {
            messages,
            page_size,
            history_id: 100,
            ..Default::default()
        })))
    }
    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.0.lock().unwrap()
    }
    fn matching(&self, q: Option<&str>) -> Vec<Message> {
        let q = q.unwrap_or("");
        let after = parse_bound(q, "after:");
        let before = parse_bound(q, "before:");
        let words: Vec<&str> = q.split_whitespace().filter(|w| !w.contains(':')).collect();
        self.st()
            .messages
            .iter()
            .filter(|m| after.is_none_or(|a| m.date >= a) && before.is_none_or(|b| m.date < b))
            .filter(|m| words.iter().all(|w| m.body_text.contains(w)))
            .cloned()
            .collect()
    }
    fn count(&self, prefix: &str) -> usize {
        self.st()
            .calls
            .iter()
            .filter(|c| c.starts_with(prefix))
            .count()
    }
    fn fetched(&self, prefix: &str) -> Vec<String> {
        self.st()
            .calls
            .iter()
            .filter_map(|c| c.strip_prefix(prefix).map(str::to_string))
            .collect()
    }
}

fn headers_only(m: &Message) -> Message {
    let mut m = m.clone();
    m.body_text.clear();
    m.body_html = None;
    m.attachments.clear();
    m
}

impl GmailApi for Fake {
    async fn profile(&self) -> Result<Profile> {
        let s = self.st();
        Ok(Profile {
            email: ACCT.into(),
            messages_total: s.messages.len() as u64,
            history_id: s.history_id,
        })
    }
    async fn list_message_ids(&self, page_token: Option<&str>, q: Option<&str>) -> Result<IdPage> {
        let all = self.matching(q);
        let mut s = self.st();
        s.calls.push(format!(
            "list:{}:{}",
            q.unwrap_or("-"),
            page_token.unwrap_or("-")
        ));
        let start: usize = page_token
            .map(|t| t.trim_start_matches('p').parse().unwrap())
            .unwrap_or(0);
        let end = (start + s.page_size).min(all.len());
        Ok(IdPage {
            ids: all[start.min(end)..end]
                .iter()
                .map(|m| (m.id.clone(), m.thread_id.clone()))
                .collect(),
            next_page_token: (end < all.len()).then(|| format!("p{end}")),
            result_size_estimate: all.len() as u64,
        })
    }
    async fn get_messages_each(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        let mut s = self.st();
        ids.iter()
            .map(|id| {
                s.calls.push(format!("full:{id}"));
                let m = s.messages.iter().find(|m| &m.id == id).cloned();
                (id.clone(), Ok(m))
            })
            .collect()
    }
    async fn get_headers_each(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        let mut s = self.st();
        ids.iter()
            .map(|id| {
                s.calls.push(format!("hdr:{id}"));
                let m = s.messages.iter().find(|m| &m.id == id).map(headers_only);
                (id.clone(), Ok(m))
            })
            .collect()
    }
    async fn get_message_labels(&self, _ids: &[String]) -> Result<Vec<MessageLabels>> {
        Ok(vec![])
    }
    async fn list_history(&self, _start: u64, _page_token: Option<&str>) -> Result<HistoryPage> {
        let mut s = self.st();
        s.calls.push("history".into());
        let page = if s.history.is_empty() {
            HistoryPage::default()
        } else {
            s.history.remove(0)
        };
        Ok(HistoryPage {
            history_id: s.history_id,
            ..page
        })
    }
    async fn list_labels(&self) -> Result<Vec<Label>> {
        Ok(vec![])
    }
    async fn label_messages_total(&self, _label_id: &str) -> Result<u64> {
        Ok(0)
    }
}

impl WindowApi for Fake {
    async fn search_ids(&self, q: &str, max_results: u32) -> Result<IdPage> {
        let all = self.matching(Some(q));
        self.st().calls.push(format!("search:{q}"));
        Ok(IdPage {
            ids: all
                .iter()
                .take(max_results as usize)
                .map(|m| (m.id.clone(), m.thread_id.clone()))
                .collect(),
            next_page_token: None,
            result_size_estimate: all.len() as u64,
        })
    }
    async fn headers_now(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        GmailApi::get_headers_each(self, ids).await
    }
    async fn full_now(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        GmailApi::get_messages_each(self, ids).await
    }
}

#[derive(Default)]
struct Recorder {
    statuses: Mutex<Vec<SyncStatus>>,
}

impl SyncObserver for Recorder {
    fn status(&self, status: SyncStatus) {
        self.statuses.lock().unwrap().push(status);
    }
    fn mail_changed(&self, _account_id: &str, _thread_ids: Vec<String>) {}
}

struct Harness {
    fake: Fake,
    store: Store,
    shared: Arc<Shared>,
    rec: Arc<Recorder>,
}

impl Harness {
    fn new(messages: Vec<Message>, page_size: usize, policy: WindowPolicy) -> Self {
        let rec = Arc::new(Recorder::default());
        let shared = Shared::new(ACCT, rec.clone());
        *shared.policy.lock().unwrap() = policy;
        Harness {
            fake: Fake::new(messages, page_size),
            store: Store::open_in_memory().unwrap(),
            shared,
            rec,
        }
    }
    fn set_policy(&self, p: WindowPolicy) {
        *self.shared.policy.lock().unwrap() = p;
    }
    fn sync(&self) -> AccountSync<Fake> {
        AccountSync::new(
            ACCT,
            self.fake.clone(),
            self.store.clone(),
            self.shared.clone(),
        )
    }
    fn window(&self) -> WindowCursor {
        self.store.get_sync_cursor(ACCT).unwrap().window
    }
    fn pending(&self, id: &str) -> Option<bool> {
        self.store.is_body_pending(ACCT, id).unwrap()
    }
    fn coverage(&self) -> (u64, u64) {
        self.store
            .body_coverage()
            .unwrap()
            .into_iter()
            .find(|c| c.account_id == ACCT)
            .map_or((0, 0), |c| (c.full, c.headers_only))
    }
    fn last_status(&self) -> SyncStatus {
        self.rec.statuses.lock().unwrap().last().cloned().unwrap()
    }
}

/// One iteration of the run loop, minus the sleep. False = nothing to do.
async fn step(sync: &mut AccountSync<Fake>) -> bool {
    sync.apply_window_policy().await.unwrap();
    if !sync.cursor.backfill_done {
        sync.backfill_page().await.unwrap();
        return true;
    }
    sync.older_page().await.unwrap()
}

async fn run_to_rest(sync: &mut AccountSync<Fake>) {
    for _ in 0..200 {
        if !step(sync).await {
            return;
        }
    }
    panic!("sync never came to rest");
}

fn policy(months: u32, older: OlderMail) -> WindowPolicy {
    WindowPolicy { months, older }
}

// ---------- pure helpers ----------

#[test]
fn window_start_and_range_query() {
    let now = 1_760_000_000_000; // 2025-10-09
    assert_eq!(window_start_ms(now, 0), 0);
    let six = window_start_ms(now, 6);
    assert_eq!(six % DAY_MS, 0, "rounded to UTC midnight");
    let days = (now - six) as f64 / DAY_MS as f64;
    assert!((182.0..184.0).contains(&days), "{days}");
    assert!(window_start_ms(now, 12) < six);
    assert_eq!(range_query(Some(0), None), None);
    assert_eq!(range_query(None, None), None);
    assert_eq!(
        range_query(Some(1_700_000_000_000), Some(1_750_000_000_999)).as_deref(),
        Some("after:1700000000 before:1750000000")
    );
}

#[test]
fn plan_fill_transitions() {
    let t6 = 1_000 * DAY_MS;
    let t12 = t6 - 183 * DAY_MS;
    let mut w = WindowCursor::default();
    let (mut done, mut tok) = (false, None);
    // First fill: [t6, now).
    assert!(plan_fill(&mut w, &mut done, &mut tok, t6));
    assert_eq!(
        (w.fill_after_ms, w.fill_before_ms, done),
        (Some(t6), None, false)
    );
    // In progress; the target sliding a day doesn't restart it.
    tok = Some("p2".into());
    assert!(!plan_fill(&mut w, &mut done, &mut tok, t6 + DAY_MS));
    assert_eq!(tok.as_deref(), Some("p2"));
    // Completed.
    w.full_since_ms = Some(t6);
    w.fill_after_ms = None;
    done = true;
    tok = None;
    assert!(
        !plan_fill(&mut w, &mut done, &mut tok, t6 + 30 * DAY_MS),
        "sliding window needs nothing"
    );
    // Grow to 12 months: fill only the new band.
    assert!(plan_fill(&mut w, &mut done, &mut tok, t12));
    assert_eq!(
        (w.fill_after_ms, w.fill_before_ms, done),
        (Some(t12), Some(t6), false)
    );
    // Shrink back before that band finished: the fill is dropped.
    tok = Some("p9".into());
    assert!(plan_fill(&mut w, &mut done, &mut tok, t6 + DAY_MS));
    assert_eq!((w.fill_after_ms, done, tok), (None, true, None));
    assert_eq!(
        w.full_since_ms,
        Some(t6),
        "shrinking never moves full_since"
    );
}

// ---------- stages ----------

#[tokio::test]
async fn fill_lists_only_the_window_then_older_mail_is_headers_only() {
    // 40 messages, one every 10 days: ~19 inside six months.
    let h = Harness::new(mailbox(40, 10), 5, policy(6, OlderMail::Headers));
    let mut sync = h.sync();
    sync.init().await.unwrap();
    let t6 = window_start_ms(now(), 6);
    let in_window = h
        .fake
        .matching(range_query(Some(t6), None).as_deref())
        .len();
    assert!((17..=20).contains(&in_window), "{in_window}");

    // Stage A: every list call is bounded by after:, every get is full.
    while !sync.cursor.backfill_done {
        assert_eq!(sync.current_stage(), Some(SyncStage::Window));
        step(&mut sync).await;
    }
    assert!(h
        .fake
        .st()
        .calls
        .iter()
        .filter(|c| c.starts_with("list:"))
        .all(|c| c.contains("after:")));
    assert_eq!(h.fake.count("full:"), in_window);
    assert_eq!(h.fake.count("hdr:"), 0);
    assert_eq!(h.window().full_since_ms, Some(t6));
    assert_eq!(h.coverage(), (in_window as u64, 0));

    // Stage B: older mail, headers only, listed with before:<window start>.
    assert_eq!(sync.current_stage(), Some(SyncStage::Older));
    run_to_rest(&mut sync).await;
    assert_eq!(h.fake.count("hdr:"), 40 - in_window);
    assert_eq!(
        h.fake.count("full:"),
        in_window,
        "older mail never fetched in full"
    );
    assert!(h
        .fake
        .st()
        .calls
        .iter()
        .any(|c| c.starts_with(&format!("list:before:{}", t6 / 1000))));
    assert_eq!(h.coverage(), (in_window as u64, (40 - in_window) as u64));
    assert_eq!(h.pending("m039"), Some(true));
    assert_eq!(h.pending("m000"), Some(false));
    let w = h.window();
    assert!(w.older_done);
    assert_eq!(w.older_mode.as_deref(), Some("headers"));
    let last = h.last_status();
    assert_eq!((last.phase, last.stage), (SyncPhase::Idle, None));
    assert!(h
        .rec
        .statuses
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.stage == Some(SyncStage::Older) && s.phase == SyncPhase::Backfilling));
}

#[tokio::test]
async fn new_mail_during_older_stage_arrives_in_full() {
    let h = Harness::new(mailbox(30, 10), 3, policy(3, OlderMail::Headers));
    let mut sync = h.sync();
    sync.init().await.unwrap();
    while !sync.cursor.backfill_done {
        step(&mut sync).await;
    }
    // A new message shows up in history while stage B runs.
    let fresh = msg("fresh", now());
    h.fake.st().messages.insert(0, fresh);
    h.fake.st().history.push(HistoryPage {
        added: vec!["fresh".into()],
        ..Default::default()
    });
    h.shared.poke();
    assert!(step(&mut sync).await, "stage B page ran");
    assert!(
        h.fake.count("history") >= 1,
        "history polled between chunks"
    );
    assert_eq!(h.pending("fresh"), Some(false), "new mail is always full");
    run_to_rest(&mut sync).await;
}

#[tokio::test]
async fn older_none_skips_older_mail_and_older_full_downloads_it() {
    let h = Harness::new(mailbox(20, 20), 4, policy(3, OlderMail::None));
    let mut sync = h.sync();
    sync.init().await.unwrap();
    run_to_rest(&mut sync).await;
    let in_window = h.fake.count("full:");
    assert!(in_window < 20);
    assert_eq!(h.fake.count("hdr:"), 0);
    assert_eq!(h.coverage(), (in_window as u64, 0));
    assert!(!h.fake.st().calls.iter().any(|c| c.contains("before:")));
    assert_eq!(h.last_status().phase, SyncPhase::Idle);

    // Switching to "full" downloads the rest in full.
    h.set_policy(policy(3, OlderMail::Full));
    run_to_rest(&mut sync).await;
    assert_eq!(h.fake.count("full:"), 20);
    assert_eq!(h.coverage(), (20, 0));
}

#[tokio::test]
async fn stages_resume_after_restart() {
    let h = Harness::new(mailbox(40, 10), 4, policy(6, OlderMail::Headers));
    let mut sync = h.sync();
    sync.init().await.unwrap();
    step(&mut sync).await;
    let token =
        crate::sync::GmailCursor::parse(&h.store.get_sync_cursor(ACCT).unwrap().provider_state)
            .unwrap()
            .backfill_page_token;
    assert_eq!(token.as_deref(), Some("p4"));
    drop(sync);

    // Restart mid-fill: continues from the saved page, same query.
    let mut sync = h.sync();
    sync.init().await.unwrap();
    step(&mut sync).await;
    let lists: Vec<String> = h
        .fake
        .st()
        .calls
        .iter()
        .filter(|c| c.starts_with("list:"))
        .cloned()
        .collect();
    assert!(lists[1].ends_with(":p4"), "{lists:?}");
    while !sync.cursor.backfill_done {
        step(&mut sync).await;
    }
    // One stage B page, then restart mid-stage-B.
    step(&mut sync).await;
    let older_token = h.window().older_page_token;
    assert!(older_token.is_some());
    drop(sync);
    let mut sync = h.sync();
    sync.init().await.unwrap();
    step(&mut sync).await;
    let last_list = h
        .fake
        .st()
        .calls
        .iter()
        .rev()
        .find(|c| c.starts_with("list:"))
        .cloned()
        .unwrap();
    assert!(
        last_list.ends_with(&format!(":{}", older_token.unwrap())),
        "{last_list}"
    );
    run_to_rest(&mut sync).await;
    // Nothing was fetched twice.
    let mut all: Vec<String> = h.fake.fetched("full:");
    all.extend(h.fake.fetched("hdr:"));
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n);
    assert_eq!(n, 40);
}

#[tokio::test]
async fn growing_the_window_upgrades_the_new_band_and_shrinking_deletes_nothing() {
    let h = Harness::new(mailbox(40, 10), 5, policy(3, OlderMail::Headers));
    let mut sync = h.sync();
    sync.init().await.unwrap();
    run_to_rest(&mut sync).await;
    let t3 = window_start_ms(now(), 3);
    let (full3, hdr3) = h.coverage();
    assert_eq!(full3 + hdr3, 40);

    // Grow to 12 months: only [t12, t3) is listed and fetched in full.
    h.set_policy(policy(12, OlderMail::Headers));
    let t12 = window_start_ms(now(), 12);
    let before = h.fake.count("full:");
    step(&mut sync).await;
    let last_list = h
        .fake
        .st()
        .calls
        .iter()
        .rev()
        .find(|c| c.starts_with("list:"))
        .cloned()
        .unwrap();
    assert!(
        last_list.starts_with(&format!("list:after:{} before:{}", t12 / 1000, t3 / 1000)),
        "{last_list}"
    );
    run_to_rest(&mut sync).await;
    let band = h
        .fake
        .matching(range_query(Some(t12), Some(t3)).as_deref())
        .len();
    assert_eq!(
        h.fake.count("full:") - before,
        band,
        "the band was upgraded, nothing else"
    );
    assert_eq!(h.window().full_since_ms, Some(t12));
    assert_eq!(h.coverage(), (full3 + band as u64, hdr3 - band as u64));
    assert!(h.window().older_done);
    assert_eq!(h.window().older_before_ms, Some(t12));

    // Shrink to 1 month: nothing is fetched or dropped.
    let calls = h.fake.st().calls.len();
    h.set_policy(policy(1, OlderMail::Headers));
    assert!(!step(&mut sync).await);
    assert_eq!(h.fake.st().calls.len(), calls);
    assert_eq!(h.coverage(), (full3 + band as u64, hdr3 - band as u64));
    assert_eq!(h.window().full_since_ms, Some(t12));
}

#[tokio::test]
async fn legacy_completed_backfill_counts_as_everything() {
    let h = Harness::new(mailbox(10, 60), 5, policy(6, OlderMail::Headers));
    h.store
        .set_sync_cursor(
            ACCT,
            &penguin_core::SyncCursor {
                provider_state: crate::sync::GmailCursor {
                    history_id: Some(100),
                    backfill_page_token: None,
                }
                .to_state(),
                backfill_done: true,
                ..Default::default()
            },
        )
        .unwrap();
    let mut sync = h.sync();
    sync.init().await.unwrap();
    assert!(!step(&mut sync).await);
    assert_eq!(h.window().full_since_ms, Some(0));
    assert_eq!(h.fake.count("list:"), 0);
}

#[tokio::test]
async fn legacy_backfill_in_progress_restarts_as_a_window_fill() {
    let h = Harness::new(mailbox(10, 60), 5, policy(6, OlderMail::Headers));
    h.store
        .set_sync_cursor(
            ACCT,
            &penguin_core::SyncCursor {
                provider_state: crate::sync::GmailCursor {
                    history_id: Some(100),
                    backfill_page_token: Some("p5".into()),
                }
                .to_state(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut sync = h.sync();
    sync.init().await.unwrap();
    step(&mut sync).await;
    let first = h
        .fake
        .st()
        .calls
        .iter()
        .find(|c| c.starts_with("list:"))
        .cloned()
        .unwrap();
    assert!(
        first.starts_with("list:after:") && first.ends_with(":-"),
        "{first}"
    );
}

#[tokio::test]
async fn free_up_space_is_not_undone_by_sync() {
    let h = Harness::new(mailbox(30, 10), 10, policy(0, OlderMail::Headers));
    let mut sync = h.sync();
    sync.init().await.unwrap();
    run_to_rest(&mut sync).await;
    assert_eq!(h.coverage(), (30, 0));
    // Shrink to 3 months and free up space.
    h.set_policy(policy(3, OlderMail::Headers));
    assert!(!step(&mut sync).await, "shrinking alone does nothing");
    let t3 = window_start_ms(now(), 3);
    let dropped = h.store.drop_bodies_before(ACCT, t3).unwrap();
    assert!(dropped > 0);
    assert_eq!(h.coverage(), (30 - dropped, dropped));
    // Nothing re-downloads them while the window stays at 3 months.
    let calls = h.fake.st().calls.len();
    assert!(!step(&mut sync).await);
    assert_eq!(h.fake.st().calls.len(), calls);
}

// ---------- lazy bodies ----------

#[tokio::test]
async fn opening_a_pending_message_fetches_its_body() {
    let fake = Fake::new(mailbox(3, 400), 10);
    let store = Store::open_in_memory().unwrap();
    let old = fake.st().messages[2].clone();
    store.insert_header_messages(&[headers_only(&old)]).unwrap();
    assert_eq!(
        store.pending_message_ids(ACCT, &old.thread_id).unwrap(),
        vec![old.id.clone()]
    );

    let threads = fetch_pending_bodies(&fake, &store, ACCT, std::slice::from_ref(&old.id))
        .await
        .unwrap();
    assert_eq!(threads, vec![old.thread_id.clone()]);
    assert_eq!(store.is_body_pending(ACCT, &old.id).unwrap(), Some(false));
    assert_eq!(
        store.get_message(ACCT, &old.id).unwrap().unwrap().body_text,
        old.body_text
    );
    assert!(store
        .pending_message_ids(ACCT, &old.thread_id)
        .unwrap()
        .is_empty());
    assert_eq!(fake.count("full:"), 1);
}

/// get_thread reads the pending ids and spawns the download; a second
/// get_thread (a re-render, a mail-changed) that read them before the first
/// download landed must not fetch the message again: a refetch costs a full
/// get and hands out new attachment ids.
#[tokio::test]
async fn a_stale_pending_list_does_not_refetch_a_downloaded_body() {
    let fake = Fake::new(mailbox(3, 400), 10);
    let store = Store::open_in_memory().unwrap();
    let old = fake.st().messages[2].clone();
    store.insert_header_messages(&[headers_only(&old)]).unwrap();
    let stale = store.pending_message_ids(ACCT, &old.thread_id).unwrap();

    fetch_pending_bodies(&fake, &store, ACCT, &stale)
        .await
        .unwrap();
    let again = fetch_pending_bodies(&fake, &store, ACCT, &stale)
        .await
        .unwrap();
    assert!(again.is_empty());
    assert_eq!(fake.count("full:"), 1);
    // Not stored at all (deleted meanwhile): nothing to download either.
    let gone = fetch_pending_bodies(&fake, &store, ACCT, &["nope".to_string()])
        .await
        .unwrap();
    assert!(gone.is_empty());
    assert_eq!(fake.count("full:"), 1);
}

// ---------- server search ----------

#[tokio::test]
async fn server_search_stores_unknown_matches_headers_only_and_returns_hits() {
    let mut mail = mailbox(8, 100);
    for m in &mut mail {
        m.body_text = format!("{} quarterly", m.body_text);
    }
    let fake = Fake::new(mail.clone(), 10);
    let store = Store::open_in_memory().unwrap();
    // m000 is stored in full, m001 headers-only; the rest are unknown.
    store.upsert_messages(&[mail[0].clone()]).unwrap();
    store
        .insert_header_messages(&[headers_only(&mail[1])])
        .unwrap();

    let r = search_server(&fake, &store, ACCT, "quarterly", 4)
        .await
        .unwrap();
    assert_eq!(r.estimate, 8);
    assert_eq!(r.fetched, 4, "capped");
    assert_eq!(fake.count("hdr:"), 4);
    assert_eq!(fake.count("full:"), 0);
    // Hits: the 2 already-local + 4 fetched, newest first.
    let ids: Vec<&str> = r.hits.iter().map(|h| h.message_id.as_str()).collect();
    assert_eq!(ids, vec!["m000", "m001", "m002", "m003", "m004", "m005"]);
    assert_eq!(store.is_body_pending(ACCT, "m003").unwrap(), Some(true));
    assert_eq!(store.count_messages(Some(ACCT)).unwrap(), 6);
    // They're local now: local search finds them by subject.
    let local = store
        .search(&penguin_core::SearchRequest {
            query: "subject:m003".into(),
            account_id: None,
            account_ids: None,
            limit: 10,
        })
        .unwrap();
    assert_eq!(local.hits.len(), 1);
    // A repeat search fetches the remainder.
    let r2 = search_server(&fake, &store, ACCT, "quarterly", 4)
        .await
        .unwrap();
    assert_eq!(r2.fetched, 2);
    assert_eq!(r2.hits.len(), 8);
}

#[test]
fn gmail_query_translation() {
    let t = |q: &str| {
        let parsed = penguin_core::query::parse_with_offset(q, 1_760_000_000_000, 0);
        to_gmail_query(&parsed)
    };
    assert_eq!(t("invoice"), ("invoice".into(), false));
    assert_eq!(
        t("\"quarterly report\""),
        ("\"quarterly report\"".into(), false)
    );
    assert_eq!(
        t("from:ana subject:budget has:pdf"),
        ("from:ana subject:budget filename:pdf".into(), false)
    );
    assert_eq!(
        t("is:unread in:inbox -label:work"),
        ("is:unread in:inbox -label:work".into(), false)
    );
    assert_eq!(
        t("in:done is:starred"),
        ("-in:inbox is:starred".into(), false)
    );
    assert_eq!(t("invoice OR receipt"), ("{invoice receipt}".into(), false));
    assert_eq!(
        t("to:\"Ana Ruiz\" filename:q3"),
        ("to:\"Ana Ruiz\" filename:q3".into(), false)
    );
    let (q, approx) = t("before:2024-01-01 after:2023-06-01 tax");
    assert!(!approx);
    assert!(q.starts_with("tax "), "{q}");
    assert!(q.contains(&format!("after:{}", 1_685_577_600)), "{q}");
    assert!(q.contains(&format!("before:{}", 1_704_067_200)), "{q}");
    // account: never reaches Gmail.
    assert_eq!(t("account:me invoice").0, "invoice");

    // Operators Gmail also has.
    assert_eq!(t("from:me to:me"), ("from:me to:me".into(), false));
    assert_eq!(
        t("larger:5M smaller:100k"),
        ("larger:5242880 smaller:102400".into(), false)
    );
    assert_eq!(
        t("with:ana"),
        ("{from:ana to:ana cc:ana bcc:ana}".into(), false)
    );
    assert_eq!(
        t("domain:acme.example"),
        (
            "{from:acme.example to:acme.example cc:acme.example bcc:acme.example}".into(),
            false
        )
    );
    assert_eq!(
        t("-(from:ana OR from:bob) lease"),
        ("-from:ana -from:bob lease".into(), false)
    );
    // Local-only operators are dropped and the query marked approximate
    // (Gmail's results are then broader than the local search).
    for q in [
        "is:new-sender",
        "to:new",
        "is:unanswered",
        "is:awaiting",
        "is:replied",
        "is:reply",
        "is:newsletter",
        "is:snoozed",
        "has:link",
        "has:otp",
        "has:unsubscribe",
        "messages:>5",
        "day:weekend",
    ] {
        let (g, approx) = t(&format!("invoice {q}"));
        assert_eq!(g, "invoice", "{q}");
        assert!(approx, "{q}");
    }
}

#[tokio::test]
async fn window_estimate_uses_one_cheap_list() {
    let fake = Fake::new(mailbox(40, 10), 10);
    let n = window_estimate(&fake, "est@penguin.example", 6, now())
        .await
        .unwrap();
    assert!((17..=20).contains(&n), "{n}");
    // Cached.
    window_estimate(&fake, "est@penguin.example", 6, now())
        .await
        .unwrap();
    assert_eq!(fake.count("search:"), 1);
    let all = window_estimate(&fake, "est@penguin.example", 0, now())
        .await
        .unwrap();
    assert_eq!(all, 40);
}
