//! Reply Later and Follow up views.

use super::is_automated_address;
use crate::types::*;
use crate::Store;

const A: &str = "ada@penguin.example";
const B: &str = "bea@penguin.example";
const DAY: i64 = 86_400_000;
const NOW: i64 = 1_760_000_000_000;

fn account(id: &str, added: i64) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        display_name: None,
        nickname: None,
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

/// A message received by `account` from `from`.
fn received(
    account: &str,
    id: &str,
    thread: &str,
    date: i64,
    from: &str,
    labels: &[&str],
) -> Message {
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

/// A message `account` sent to `to`.
fn sent(account: &str, id: &str, thread: &str, date: i64, to: &[&str]) -> Message {
    let mut m = received(account, id, thread, date, account, &["SENT"]);
    m.to = to.iter().map(|e| addr(e)).collect();
    m
}

fn label(account: &str, id: &str, name: &str) -> Label {
    Label {
        account_id: account.into(),
        id: id.into(),
        name: name.into(),
        kind: "user".into(),
        color: None,
        unread_count: None,
        hidden: false,
    }
}

fn query(view: MailboxView) -> ListQuery {
    ListQuery {
        view,
        tab: None,
        account_id: None,
        account_ids: None,
        limit: 100,
        before: None,
        unread_only: false,
        split: None,
    }
}

fn follow(s: &Store, days: u32) -> Vec<String> {
    s.list_follow_ups(None, days, NOW, false)
        .unwrap()
        .into_iter()
        .map(|t| t.thread_id)
        .collect()
}

#[test]
fn reply_later_merges_each_accounts_label() {
    let s = store();
    s.upsert_label(&label(A, "Label_7", "Reply Later")).unwrap();
    s.upsert_label(&label(B, "f:Reply%20Later", "reply later"))
        .unwrap();
    s.upsert_label(&label(B, "Label_x", "Receipts")).unwrap();
    s.upsert_messages(&[
        received(
            A,
            "a1",
            "ta",
            NOW - 3 * DAY,
            "cy@quill.example",
            &["Label_7"],
        ),
        received(
            B,
            "b1",
            "tb",
            NOW - DAY,
            "cy@quill.example",
            &["f:Reply%20Later", "UNREAD"],
        ),
        received(
            B,
            "b2",
            "tc",
            NOW,
            "cy@quill.example",
            &["Label_x", "INBOX"],
        ),
    ])
    .unwrap();
    let got: Vec<String> = s
        .list_threads(&query(MailboxView::ReplyLater))
        .unwrap()
        .into_iter()
        .map(|t| t.thread_id)
        .collect();
    assert_eq!(got, vec!["tb", "ta"]);
    assert_eq!(
        s.reply_later_label(B).unwrap().as_deref(),
        Some("f:Reply%20Later")
    );

    let mut q = query(MailboxView::ReplyLater);
    q.account_ids = Some(vec![A.into()]);
    assert_eq!(s.list_threads(&q).unwrap().len(), 1);
    q.account_ids = None;
    q.unread_only = true;
    assert_eq!(s.list_threads(&q).unwrap()[0].thread_id, "tb");
    q.unread_only = false;
    q.before = Some(NOW - 2 * DAY);
    assert_eq!(s.list_threads(&q).unwrap()[0].thread_id, "ta");

    let counts = s.triage_counts(None, 3, NOW).unwrap();
    assert_eq!(
        counts.iter().map(|c| c.reply_later).collect::<Vec<_>>(),
        vec![1, 1]
    );
}

#[test]
fn reply_later_without_a_label_is_empty() {
    let s = store();
    s.upsert_messages(&[received(A, "a1", "ta", NOW, "cy@quill.example", &["INBOX"])])
        .unwrap();
    assert!(s
        .list_threads(&query(MailboxView::ReplyLater))
        .unwrap()
        .is_empty());
    assert_eq!(s.reply_later_label(A).unwrap(), None);
}

#[test]
fn follow_up_lists_unanswered_sent_mail_oldest_first() {
    let s = store();
    s.upsert_messages(&[
        // Waiting 5 days (you started it).
        sent(A, "w1", "wait5", NOW - 5 * DAY, &["cy@quill.example"]),
        // Waiting 10 days, a reply to them.
        received(
            A,
            "w2a",
            "wait10",
            NOW - 12 * DAY,
            "dee@quill.example",
            &["INBOX"],
        ),
        sent(A, "w2b", "wait10", NOW - 10 * DAY, &["dee@quill.example"]),
        // Too recent for 3 days.
        sent(A, "w3", "fresh", NOW - DAY, &["cy@quill.example"]),
        // Too old for the lookback.
        sent(A, "w4", "ancient", NOW - 90 * DAY, &["cy@quill.example"]),
        // They answered.
        sent(A, "r1", "answered", NOW - 6 * DAY, &["cy@quill.example"]),
        received(
            A,
            "r2",
            "answered",
            NOW - 5 * DAY,
            "cy@quill.example",
            &["INBOX"],
        ),
        // A draft after your message doesn't count as an answer.
        sent(B, "d1", "drafted", NOW - 4 * DAY, &["eve@quill.example"]),
        received(B, "d2", "drafted", NOW - 3 * DAY, B, &["DRAFT"]),
    ])
    .unwrap();
    assert_eq!(follow(&s, 3), vec!["wait10", "wait5", "drafted"]);
    assert_eq!(follow(&s, 7), vec!["wait10"]);
    // last_date is when the awaited message went out.
    let rows = s.list_follow_ups(None, 3, NOW, false).unwrap();
    assert_eq!(rows[0].last_date, NOW - 10 * DAY);
    let counts = s.triage_counts(None, 3, NOW).unwrap();
    assert_eq!(
        counts
            .iter()
            .map(|c| (c.account_id.as_str(), c.follow_up))
            .collect::<Vec<_>>(),
        vec![(A, 2), (B, 1)]
    );
    assert_eq!(
        s.list_follow_ups(Some(&[B.into()]), 3, NOW, false)
            .unwrap()
            .len(),
        1
    );
    assert!(s
        .list_follow_ups(Some(&[]), 3, NOW, false)
        .unwrap()
        .is_empty());
    // One page: a `before` page is empty, so the list never pages forever.
    let mut q = query(MailboxView::FollowUp);
    q.before = Some(NOW);
    assert!(s.list_threads(&q).unwrap().is_empty());
}

#[test]
fn follow_up_skips_machines_self_notes_rsvps_and_bulk_threads() {
    let s = store();
    let mut rsvp = sent(A, "c1", "rsvp", NOW - 5 * DAY, &["cy@quill.example"]);
    rsvp.attachments = vec![AttachmentMeta {
        id: "att1".into(),
        filename: "invite.ics".into(),
        mime_type: "text/calendar".into(),
        size: 100,
        content_id: None,
        inline: false,
    }];
    s.upsert_messages(&[
        sent(
            A,
            "n1",
            "noreply",
            NOW - 5 * DAY,
            &["no-reply@shop.example"],
        ),
        sent(A, "n2", "self", NOW - 5 * DAY, &[A, B]),
        sent(
            A,
            "n3",
            "mixed",
            NOW - 5 * DAY,
            &["notifications@tracker.example", "cy@quill.example"],
        ),
        rsvp,
        received(
            A,
            "b1",
            "bulk",
            NOW - 6 * DAY,
            "hello@letters.example",
            &["CATEGORY_PROMOTIONS"],
        ),
        sent(A, "b2", "bulk", NOW - 5 * DAY, &["hello@letters.example"]),
    ])
    .unwrap();
    assert_eq!(follow(&s, 3), vec!["mixed"]);
}

#[test]
fn follow_up_dismiss_holds_until_something_newer_is_sent() {
    let s = store();
    s.upsert_messages(&[
        sent(A, "m1", "t1", NOW - 8 * DAY, &["cy@quill.example"]),
        sent(A, "m2", "t2", NOW - 6 * DAY, &["dee@quill.example"]),
    ])
    .unwrap();
    s.dismiss_follow_ups(&[(A.into(), "t1".into())], true)
        .unwrap();
    assert_eq!(follow(&s, 3), vec!["t2"]);
    // Undo.
    s.dismiss_follow_ups(&[(A.into(), "t1".into())], false)
        .unwrap();
    assert_eq!(follow(&s, 3), vec!["t1", "t2"]);
    // Dismissed, then they reply and you nudge again: it's back once due.
    s.dismiss_follow_ups(&[(A.into(), "t1".into())], true)
        .unwrap();
    s.upsert_messages(&[
        received(A, "m3", "t1", NOW - 7 * DAY, "cy@quill.example", &["INBOX"]),
        sent(A, "m4", "t1", NOW - 4 * DAY, &["cy@quill.example"]),
    ])
    .unwrap();
    assert_eq!(follow(&s, 3), vec!["t2", "t1"]);
    // Removing the account drops its dismissals.
    s.dismiss_follow_ups(&[(A.into(), "t2".into())], true)
        .unwrap();
    s.remove_account(A).unwrap();
    assert!(follow(&s, 3).is_empty());
}

/// Replies seconds after your message, or in the same millisecond, are
/// answers too: the thread row can't prove those (see `follow_ups`), so the
/// messages decide. Trashed replies don't count, and a thread whose newest
/// live message is yours waits even with an older reply in Spam.
#[test]
fn follow_up_reads_messages_when_the_thread_row_cant_tell() {
    let s = store();
    let t = NOW - 5 * DAY;
    s.upsert_messages(&[
        sent(A, "q1", "quick", t, &["cy@quill.example"]),
        received(A, "q2", "quick", t + 60_000, "cy@quill.example", &["INBOX"]),
        sent(A, "s1", "same-ms", t, &["cy@quill.example"]),
        received(A, "s2", "same-ms", t, "cy@quill.example", &["INBOX"]),
        sent(A, "x1", "trashed-reply", t, &["cy@quill.example"]),
        received(
            A,
            "x2",
            "trashed-reply",
            t + DAY,
            "cy@quill.example",
            &["TRASH"],
        ),
    ])
    .unwrap();
    // "same-ms": both share a millisecond, and the later rowid (the reply,
    // stored second) is the latest.
    assert_eq!(follow(&s, 3), vec!["trashed-reply"]);
    assert_eq!(s.triage_counts(None, 3, NOW).unwrap()[0].follow_up, 1);
}

/// The one-pass Follow up agrees with the per-thread queries it replaced
/// (the latest live message by rowid is yours and in the window; not a
/// reply into bulk mail) over random threads: replies before and after,
/// in the same millisecond, trashed, spam, drafts, newsletters.
#[test]
fn follow_up_matches_the_per_thread_rules() {
    use rusqlite::params;
    let s = store();
    let mut seed: u64 = 0x5eed;
    let mut rnd = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut msgs = Vec::new();
    for t in 0..400 {
        let thread = format!("t{t}");
        let base = NOW - (1 + rnd(70) as i64) * DAY;
        for i in 0..1 + rnd(5) {
            let id = format!("{thread}-{i}");
            // Dates close together, often the same millisecond.
            let date = base + [0, 0, 1, 60_000, DAY, 3 * DAY][rnd(6) as usize];
            let mut m = if rnd(2) == 0 {
                sent(A, &id, &thread, date, &["cy@quill.example"])
            } else {
                received(A, &id, &thread, date, "cy@quill.example", &["INBOX"])
            };
            match rnd(8) {
                0 => m.label_ids.push("TRASH".into()),
                1 => m.label_ids.push("SPAM".into()),
                2 => m.label_ids = vec!["DRAFT".into()],
                3 => m.label_ids.push("CATEGORY_PROMOTIONS".into()),
                _ => {}
            }
            msgs.push(m);
        }
    }
    // Arrival order shuffled, so rowids within a millisecond vary.
    for i in (1..msgs.len()).rev() {
        msgs.swap(i, rnd(i as u64 + 1) as usize);
    }
    for chunk in msgs.chunks(37) {
        s.upsert_messages(chunk).unwrap();
    }
    for days in [1, 3, 14] {
        let hi = NOW - days as i64 * DAY;
        let lo = NOW - FOLLOW_UP_LOOKBACK_DAYS as i64 * DAY;
        let mut want: Vec<String> = s
            .read(|c| {
                let threads: Vec<(i64, String)> = c
                    .prepare(
                        "SELECT v.thread_rowid, t.thread_id FROM thread_views v JOIN threads t ON t.rowid = v.thread_rowid
                         WHERE v.view = 'SENT' AND v.last_date >= ?1 AND v.last_date < ?2",
                    )?
                    .query_map(params![lo, hi], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut out = Vec::new();
                for (trow, thread_id) in threads {
                    let latest: Option<(i64, i64)> = c
                        .query_row(
                            "SELECT flags, date FROM messages WHERE thread_rowid = ?1 AND flags & ?2 = 0
                             ORDER BY rowid DESC LIMIT 1",
                            params![trow, super::NOT_LIVE],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .ok();
                    let Some((flags, at)) = latest else { continue };
                    if flags & super::F_SENT == 0 || at >= hi || at < lo {
                        continue;
                    }
                    let (n, bulk): (i64, i64) = c.query_row(
                        "SELECT count(*), coalesce(sum(flags & ?2 != 0), 0) FROM messages
                         WHERE thread_rowid = ?1 AND flags & ?3 = 0",
                        params![trow, super::F_NEWSLETTER, super::F_SENT | super::NOT_LIVE],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    if n > 0 && bulk == n {
                        continue;
                    }
                    out.push(thread_id);
                }
                Ok(out)
            })
            .unwrap();
        let mut got = follow(&s, days);
        want.sort();
        got.sort();
        assert!(want.len() > 10, "a meaningful sample: {}", want.len());
        assert_eq!(got, want, "days = {days}");
    }
}

#[test]
fn follow_up_hides_snoozed_threads() {
    let s = store();
    s.upsert_messages(&[sent(A, "m1", "t1", NOW - 8 * DAY, &["cy@quill.example"])])
        .unwrap();
    s.snooze_threads(&[(A.into(), "t1".into())], NOW + DAY, NOW)
        .unwrap();
    assert!(follow(&s, 3).is_empty());
}

#[test]
fn automated_addresses() {
    for e in [
        "noreply@shop.example",
        "no-reply@shop.example",
        "do_not_reply@bank.example",
        "DoNotReply@bank.example",
        "notifications@github.example",
        "notifications+abc@tracker.example",
        "mailer-daemon@mx.example",
        "bounces+123@list.example",
        "newsletter@letters.example",
        "calendar-notification@google.com",
        "abc123@group.calendar.google.com",
    ] {
        assert!(is_automated_address(e), "{e}");
    }
    for e in [
        "ana@harbor.example",
        "support@vendor.example",
        "newsroom@paper.example",
        "noah@reply.example",
        "billing@vendor.example",
    ] {
        assert!(!is_automated_address(e), "{e}");
    }
}
