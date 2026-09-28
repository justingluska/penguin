//! Split Inbox: which conversation lands in which split, paging, counts.

use crate::types::*;
use crate::{Error, Store};

const A: &str = "ada@penguin.example";
const B: &str = "bea@penguin.example";
const BOSS: &str = "maya@acme.example";
const NOW: i64 = 1_760_000_000_000;
const MIN: i64 = 60_000;

fn account(id: &str, added: i64) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        color: "#123456".into(),
        added_at: added,
        ..Account::default()
    }
}

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A, 1)).unwrap();
    s.upsert_account(&account(B, 2)).unwrap();
    s
}

fn addr(email: &str) -> Address {
    Address {
        name: None,
        email: email.into(),
    }
}

fn msg(account: &str, id: &str, thread: &str, date: i64, from: &str, labels: &[&str]) -> Message {
    Message {
        account_id: account.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: addr(from),
        to: vec![addr(account)],
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

fn newsletter(
    account: &str,
    id: &str,
    thread: &str,
    date: i64,
    from: &str,
    labels: &[&str],
) -> Message {
    let mut m = msg(account, id, thread, date, from, labels);
    m.list_unsubscribe = Some("<https://news.example/u>".into());
    m
}

const VIP: &str = "from:maya@acme.example";
const NEWS: &str = "is:newsletter";

fn split(include: Option<&str>, exclude: &[&str]) -> SplitFilter {
    SplitFilter {
        include: include.map(str::to_string),
        exclude: exclude.iter().map(|q| q.to_string()).collect(),
    }
}

fn query(split: SplitFilter) -> ListQuery {
    ListQuery {
        view: MailboxView::Inbox,
        tab: None,
        account_id: None,
        account_ids: None,
        limit: 100,
        before: None,
        unread_only: false,
        split: Some(split),
    }
}

fn ids(s: &Store, q: &ListQuery) -> Vec<String> {
    s.list_threads(q)
        .unwrap()
        .into_iter()
        .map(|t| t.thread_id)
        .collect()
}

/// VIP, then News, then Other.
fn splits_of(s: &Store) -> (Vec<String>, Vec<String>, Vec<String>) {
    (
        ids(s, &query(split(Some(VIP), &[]))),
        ids(s, &query(split(Some(NEWS), &[VIP]))),
        ids(s, &query(split(None, &[VIP, NEWS]))),
    )
}

fn sample() -> Store {
    let s = store();
    s.upsert_messages(&[
        msg(A, "m1", "boss", NOW - MIN, BOSS, &["INBOX", "UNREAD"]),
        newsletter(
            A,
            "m2",
            "digest",
            NOW - 2 * MIN,
            "digest@news.example",
            &["INBOX", "UNREAD"],
        ),
        // A newsletter from the boss: VIP comes first, so it's VIP only.
        newsletter(B, "m3", "boss-news", NOW - 3 * MIN, BOSS, &["INBOX"]),
        msg(
            B,
            "m4",
            "friend",
            NOW - 4 * MIN,
            "sam@friends.example",
            &["INBOX", "UNREAD"],
        ),
        // The boss wrote earlier (archived), a friend replied since: only
        // the inbox message decides, so it's Other.
        msg(A, "m5", "old-boss", NOW - 50 * MIN, BOSS, &[]),
        msg(
            A,
            "m6",
            "old-boss",
            NOW - 5 * MIN,
            "sam@friends.example",
            &["INBOX"],
        ),
        // Archived entirely: in no split.
        msg(A, "m7", "done", NOW - 6 * MIN, BOSS, &[]),
    ])
    .unwrap();
    s
}

#[test]
fn each_conversation_lands_in_the_first_split_that_matches() {
    let s = sample();
    let (vip, news, other) = splits_of(&s);
    assert_eq!(vip, ["boss", "boss-news"]);
    assert_eq!(news, ["digest"]);
    assert_eq!(other, ["friend", "old-boss"]);
    // Together they are exactly the inbox, newest first within each.
    let mut all: Vec<String> = [vip, news, other].concat();
    all.sort();
    let mut inbox = ids(
        &s,
        &ListQuery {
            split: None,
            ..query(SplitFilter::default())
        },
    );
    inbox.sort();
    assert_eq!(all, inbox);
}

#[test]
fn rows_carry_the_inbox_date_and_honour_unread_and_scope() {
    let s = sample();
    let rows = s.list_threads(&query(split(Some(VIP), &[]))).unwrap();
    assert_eq!(rows[0].last_date, NOW - MIN);
    assert!(rows[0].unread);

    let mut q = query(split(Some(VIP), &[]));
    q.unread_only = true;
    assert_eq!(ids(&s, &q), ["boss"]);

    let mut q = query(split(None, &[VIP, NEWS]));
    q.account_ids = Some(vec![B.into()]);
    assert_eq!(ids(&s, &q), ["friend"]);
    q.account_ids = Some(vec![]);
    assert!(ids(&s, &q).is_empty());
}

#[test]
fn a_split_query_that_cannot_be_read_is_an_error_but_an_earlier_one_is_skipped() {
    let s = sample();
    let err = s
        .list_threads(&query(split(Some("date:\"blorp o'clock\""), &[])))
        .unwrap_err();
    assert!(matches!(err, Error::InvalidQuery(_)), "{err:?}");
    // An earlier split that can't be read claims nothing.
    assert_eq!(
        ids(
            &s,
            &query(split(None, &["date:\"blorp o'clock\"", VIP, NEWS]))
        ),
        ["friend", "old-boss"]
    );
}

/// Many conversations, so both the chunked walk (filter-only queries) and
/// the direct read (full-text queries) page across chunks.
#[test]
fn pages_follow_the_inbox_across_chunks() {
    let s = store();
    let mut batch = Vec::new();
    for i in 0..1000 {
        let date = NOW - i * MIN;
        let id = format!("t{i:04}");
        if i % 100 == 7 {
            batch.push(msg(A, &format!("m{i}"), &id, date, BOSS, &["INBOX"]));
        } else if i % 3 == 0 {
            batch.push(newsletter(
                A,
                &format!("m{i}"),
                &id,
                date,
                "digest@news.example",
                &["INBOX"],
            ));
        } else {
            batch.push(msg(
                A,
                &format!("m{i}"),
                &id,
                date,
                "sam@friends.example",
                &["INBOX"],
            ));
        }
    }
    s.upsert_messages(&batch).unwrap();

    let walk = |f: SplitFilter| {
        let mut out = Vec::new();
        let mut before = None;
        loop {
            let mut q = query(f.clone());
            q.limit = 60;
            q.before = before;
            let page = s.list_threads(&q).unwrap();
            let done = page.len() < 60;
            before = page.last().map(|t| t.last_date);
            out.extend(page.into_iter().map(|t| t.thread_id));
            if done {
                break;
            }
        }
        out
    };
    let vip = walk(split(Some(VIP), &[]));
    assert_eq!(vip.len(), 10);
    assert_eq!(vip[0], "t0007");
    assert_eq!(vip[9], "t0907");
    let news = walk(split(Some(NEWS), &[VIP]));
    // Every third conversation, except the VIP ones that fall on a multiple of three.
    let expect = (0..1000).filter(|i| i % 3 == 0 && i % 100 != 7).count();
    assert_eq!(news.len(), expect);
    let other = walk(split(None, &[VIP, NEWS]));
    assert_eq!(vip.len() + news.len() + other.len(), 1000);
    assert!(news.windows(2).all(|w| w[0] < w[1]), "newest first");
}

fn count(total: u32, unread: u32) -> SplitCount {
    SplitCount { total, unread }
}

#[test]
fn counts_each_split_in_order() {
    let s = sample();
    let c = s.split_counts(&[VIP.into(), NEWS.into()], None).unwrap();
    assert_eq!(
        c,
        SplitCounts {
            splits: vec![count(2, 1), count(1, 1), count(2, 1)],
            more: false
        }
    );
    // Order decides: News first claims the boss's newsletter.
    let c = s.split_counts(&[NEWS.into(), VIP.into()], None).unwrap();
    assert_eq!(c.splits, vec![count(2, 1), count(1, 1), count(2, 1)]);
    let c = s
        .split_counts(&[NEWS.into(), VIP.into()], Some(&[A.into()]))
        .unwrap();
    assert_eq!(c.splits, vec![count(1, 1), count(1, 1), count(1, 0)]);
    // An unreadable query counts nothing; its conversations fall through.
    let c = s
        .split_counts(&["date:\"blorp o'clock\"".into(), VIP.into()], None)
        .unwrap();
    assert_eq!(c.splits, vec![count(0, 0), count(2, 1), count(3, 2)]);
}
