//! Store + search integration tests (in-memory and file-backed).

use std::collections::HashSet;

use crate::types::*;
use crate::Store;

const DAY: i64 = 86_400_000;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        },
        email: email.to_string(),
    }
}

struct M {
    m: Message,
}

impl M {
    fn new(account: &str, id: &str, thread: &str, date: i64) -> M {
        M {
            m: Message {
                account_id: account.into(),
                id: id.into(),
                thread_id: thread.into(),
                date,
                from: addr("Ana Ruiz", "ana@ruiz.example"),
                to: vec![addr("Alex", account)],
                cc: vec![],
                bcc: vec![],
                reply_to: vec![],
                subject: format!("Subject {id}"),
                snippet: format!("snippet {id}"),
                body_text: format!("body of {id}"),
                body_html: Some(format!("<p>body of {id}</p>")),
                label_ids: vec!["INBOX".into()],
                attachments: vec![],
                message_id_header: Some(format!("<{id}@mail.example>")),
                in_reply_to: None,
                references: vec![],
                sender_authenticated: false,
                list_unsubscribe: None,
                list_unsubscribe_post: None,
            },
        }
    }
    fn from(mut self, name: &str, email: &str) -> M {
        self.m.from = addr(name, email);
        self
    }
    fn to(mut self, to: &[(&str, &str)]) -> M {
        self.m.to = to.iter().map(|(n, e)| addr(n, e)).collect();
        self
    }
    fn subject(mut self, s: &str) -> M {
        self.m.subject = s.into();
        self
    }
    fn body(mut self, s: &str) -> M {
        self.m.body_text = s.into();
        self
    }
    fn labels(mut self, l: &[&str]) -> M {
        self.m.label_ids = l.iter().map(|s| s.to_string()).collect();
        self
    }
    fn attach(mut self, filename: &str, mime: &str) -> M {
        let n = self.m.attachments.len();
        self.m.attachments.push(AttachmentMeta {
            id: format!("att{n}"),
            filename: filename.into(),
            mime_type: mime.into(),
            size: 1000,
            content_id: None,
            inline: false,
        });
        self
    }
    fn inline_image(mut self) -> M {
        self.m.attachments.push(AttachmentMeta {
            id: "logo".into(),
            filename: "logo.png".into(),
            mime_type: "image/png".into(),
            size: 10,
            content_id: Some("logo@x".into()),
            inline: true,
        });
        self
    }
    fn unsubscribe(mut self) -> M {
        self.m.list_unsubscribe = Some("<mailto:unsub@news.example>".into());
        self
    }
    fn done(self) -> Message {
        self.m
    }
}

fn account(id: &str) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        display_name: Some(format!("Alex {id}")),
        nickname: None,
        color: "#123456".into(),
        added_at: 1,
        ..Account::default()
    }
}

fn store_with_accounts(ids: &[&str]) -> Store {
    let s = Store::open_in_memory().unwrap();
    for id in ids {
        s.upsert_account(&account(id)).unwrap();
    }
    s
}

fn search(s: &Store, q: &str) -> SearchResponse {
    s.search(&SearchRequest {
        query: q.into(),
        account_id: None,
        account_ids: None,
        limit: 50,
    })
    .unwrap()
}

fn hit_ids(r: &SearchResponse) -> Vec<String> {
    r.hits.iter().map(|h| h.message_id.clone()).collect()
}

fn list(s: &Store, view: MailboxView, tab: Option<InboxTab>, account: Option<&str>) -> Vec<String> {
    s.list_threads(&ListQuery {
        view,
        tab,
        account_id: account.map(Into::into),
        account_ids: None,
        limit: 100,
        before: None,
        unread_only: false,
        split: None,
    })
    .unwrap()
    .into_iter()
    .map(|t| t.thread_id)
    .collect()
}

const A: &str = "me@work.example";
const B: &str = "me@home.example";

#[test]
fn roundtrip_get_message() {
    let s = store_with_accounts(&[A]);
    let mut m = M::new(A, "m1", "t1", now())
        .attach("Lease.pdf", "application/pdf")
        .inline_image()
        .unsubscribe()
        .done();
    m.cc = vec![addr("Bo", "bo@x.example")];
    m.bcc = vec![addr("", "hidden@x.example")];
    m.reply_to = vec![addr("Ana", "reply@x.example")];
    m.references = vec!["<a@x>".into(), "<b@x>".into()];
    m.in_reply_to = Some("<b@x>".into());
    s.upsert_messages(std::slice::from_ref(&m)).unwrap();
    assert_eq!(s.get_message(A, "m1").unwrap().unwrap(), m);
    assert_eq!(s.get_message(A, "nope").unwrap(), None);
    assert_eq!(s.count_messages(None).unwrap(), 1);
    assert_eq!(s.count_messages(Some(A)).unwrap(), 1);
    assert_eq!(s.count_messages(Some(B)).unwrap(), 0);
    let known = s.known_message_ids(A, &["m1".into(), "m2".into()]).unwrap();
    assert_eq!(known, HashSet::from(["m1".to_string()]));
}

#[test]
fn upsert_replaces_fts_rows() {
    let s = store_with_accounts(&[A]);
    s.upsert_messages(&[M::new(A, "m1", "t1", now()).body("the zebra report").done()])
        .unwrap();
    assert_eq!(hit_ids(&search(&s, "zebra")), vec!["m1"]);
    s.upsert_messages(&[M::new(A, "m1", "t1", now())
        .body("the giraffe report")
        .done()])
        .unwrap();
    assert!(search(&s, "zebra").hits.is_empty());
    assert_eq!(hit_ids(&search(&s, "giraffe")), vec!["m1"]);
    assert_eq!(s.count_messages(None).unwrap(), 1);
    // Date change moves the rowid; still exactly one row and one FTS doc.
    s.upsert_messages(&[M::new(A, "m1", "t1", now() - 5 * DAY)
        .body("the giraffe report")
        .done()])
        .unwrap();
    assert_eq!(hit_ids(&search(&s, "giraffe")), vec!["m1"]);
    assert_eq!(search(&s, "giraffe").hits[0].match_count, 1);
    assert_eq!(s.count_messages(None).unwrap(), 1);
}

/// Gmail hands out a new attachmentId on every messages.get. Re-storing a
/// message (its body downloaded on open, a window fill) must not change the
/// ids the UI, search results and the attachment cache already hold, or
/// Download/Preview on them fail with "attachment not found".
#[test]
fn refetch_keeps_attachment_ids() {
    let s = store_with_accounts(&["a@x.example"]);
    let first = M::new("a@x.example", "m1", "t1", now() - 200 * DAY)
        .attach("Q3 report.pdf", "application/pdf")
        .attach("notes.txt", "text/plain")
        .attach("notes.txt", "text/plain")
        .done();
    s.upsert_messages(&[first.clone()]).unwrap();

    // The same message, fetched again: every part has a fresh id, and one
    // part is new (never happens for a real message; shows it isn't matched).
    let mut again = first.clone();
    for (i, a) in again.attachments.iter_mut().enumerate() {
        a.id = format!("fresh{i}");
    }
    again.attachments.push(AttachmentMeta {
        id: "fresh3".into(),
        filename: "extra.csv".into(),
        mime_type: "text/csv".into(),
        size: 5,
        content_id: None,
        inline: false,
    });
    s.upsert_messages(&[again]).unwrap();

    let stored = s.get_message("a@x.example", "m1").unwrap().unwrap();
    let ids: Vec<&str> = stored.attachments.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["att0", "att1", "att2", "fresh3"]);
    // The person card lists the same ids.
    let card = s.person_summary("ana@ruiz.example").unwrap();
    assert!(card
        .recent_attachments
        .iter()
        .any(|h| h.message_id == "m1" && h.attachment.id == "att0"));

    // Headers-only (body pending) upgraded to full: nothing stored to keep.
    let mut pending = M::new("a@x.example", "m2", "t2", now() - 300 * DAY).done();
    pending.body_text.clear();
    s.insert_header_messages(&[pending]).unwrap();
    let full = M::new("a@x.example", "m2", "t2", now() - 300 * DAY)
        .attach("deck.pdf", "application/pdf")
        .done();
    s.upsert_messages(&[full]).unwrap();
    let stored = s.get_message("a@x.example", "m2").unwrap().unwrap();
    assert_eq!(stored.attachments[0].id, "att0");

    // Parts addressed by partId are stable already and are never swapped.
    let mut inlined = M::new("a@x.example", "m3", "t3", now()).done();
    inlined.attachments.push(AttachmentMeta {
        id: "part:4".into(),
        filename: "invite.ics".into(),
        mime_type: "text/calendar".into(),
        size: 30,
        content_id: None,
        inline: false,
    });
    s.upsert_messages(&[inlined.clone()]).unwrap();
    inlined.attachments[0].id = "ANGj_now_out_of_line".into();
    s.upsert_messages(&[inlined]).unwrap();
    let stored = s.get_message("a@x.example", "m3").unwrap().unwrap();
    assert_eq!(stored.attachments[0].id, "ANGj_now_out_of_line");
}

#[test]
fn delete_removes_everything() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "m1", "t1", t - 2000)
            .body("walrus")
            .attach("walrus-plan.pdf", "application/pdf")
            .done(),
        M::new(A, "m2", "t1", t - 1000).body("walrus again").done(),
    ])
    .unwrap();
    assert_eq!(search(&s, "walrus").hits[0].match_count, 2);
    s.delete_messages(A, &["m1".into(), "missing".into()])
        .unwrap();
    let r = search(&s, "walrus");
    assert_eq!(hit_ids(&r), vec!["m2"]);
    assert_eq!(r.hits[0].match_count, 1);
    assert!(r.attachments.is_empty());
    assert!(search(&s, "filename:plan").hits.is_empty());
    assert_eq!(s.count_messages(None).unwrap(), 1);
    s.delete_messages(A, &["m2".into()]).unwrap();
    assert!(list(&s, MailboxView::Inbox, None, None).is_empty());
    assert_eq!(s.get_thread(A, "t1").unwrap(), None);
}

#[test]
fn thread_aggregation() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "m2", "t1", t - 1000)
            .from("Bo Chen", "bo@x.example")
            .subject("Re: Offsite")
            .labels(&["INBOX", "UNREAD"])
            .done(),
        M::new(A, "m1", "t1", t - 3000)
            .from("Ana Ruiz", "ana@x.example")
            .subject("Offsite")
            .labels(&["INBOX", "Label_7"])
            .attach("agenda.pdf", "application/pdf")
            .done(),
        M::new(A, "m3", "t1", t - 2000)
            .from("Ana Ruiz", "ana@x.example")
            .subject("Re: Offsite")
            .labels(&["SENT"])
            .done(),
    ])
    .unwrap();
    let threads = s
        .list_threads(&ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 10,
            before: None,
            unread_only: false,
            split: None,
        })
        .unwrap();
    assert_eq!(threads.len(), 1);
    let th = &threads[0];
    assert_eq!(th.subject, "Offsite");
    assert_eq!(th.snippet, "snippet m2");
    assert_eq!(th.message_count, 3);
    assert!(th.unread && !th.starred && th.has_attachments);
    assert_eq!(
        th.participants
            .iter()
            .map(|p| p.email.as_str())
            .collect::<Vec<_>>(),
        vec!["bo@x.example", "ana@x.example"]
    );
    let labels: HashSet<_> = th.label_ids.iter().cloned().collect();
    assert_eq!(
        labels,
        HashSet::from([
            "INBOX".to_string(),
            "UNREAD".into(),
            "Label_7".into(),
            "SENT".into()
        ])
    );
    assert_eq!(th.last_date, t - 1000);
    // Sent view's last_date is the latest SENT message.
    let sent = s
        .list_threads(&ListQuery {
            view: MailboxView::Sent,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 10,
            before: None,
            unread_only: false,
            split: None,
        })
        .unwrap();
    assert_eq!(sent[0].last_date, t - 2000);
    let detail = s.get_thread(A, "t1").unwrap().unwrap();
    assert_eq!(
        detail
            .messages
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["m1", "m3", "m2"]
    );
    // Marking read on the thread clears unread in lists and label counts.
    s.replace_labels(
        A,
        &[Label {
            account_id: A.into(),
            id: "INBOX".into(),
            name: "INBOX".into(),
            kind: "system".into(),
            color: None,
            unread_count: Some(99),
            hidden: false,
        }],
    )
    .unwrap();
    assert_eq!(s.list_labels(Some(A)).unwrap()[0].unread_count, Some(1));
    s.modify_thread_labels(A, "t1", &[], &["UNREAD".into()])
        .unwrap();
    assert!(
        !s.list_threads(&ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 10,
            before: None,
            unread_only: false,
            split: None,
        })
        .unwrap()[0]
            .unread
    );
    assert_eq!(s.list_labels(Some(A)).unwrap()[0].unread_count, Some(0));
}

#[test]
fn views_and_tabs() {
    let s = store_with_accounts(&[A, B]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "a1", "imp", t - 1)
            .labels(&["INBOX", "IMPORTANT"])
            .done(),
        M::new(A, "a2", "news", t - 2)
            .labels(&["INBOX", "IMPORTANT"])
            .unsubscribe()
            .done(),
        M::new(A, "a3", "promo", t - 3)
            .labels(&["INBOX", "CATEGORY_PROMOTIONS"])
            .done(),
        M::new(A, "a4", "other", t - 4).labels(&["INBOX"]).done(),
        M::new(A, "a5", "archived", t - 5)
            .labels(&["Label_1"])
            .done(),
        M::new(A, "a6", "starred", t - 6)
            .labels(&["STARRED"])
            .done(),
        M::new(A, "a7", "sent", t - 7).labels(&["SENT"]).done(),
        M::new(A, "a8", "draft", t - 8).labels(&["DRAFT"]).done(),
        M::new(A, "a9", "trash", t - 9)
            .labels(&["TRASH", "Label_1"])
            .done(),
        M::new(A, "a10", "spam", t - 10).labels(&["SPAM"]).done(),
        M::new(B, "b1", "home", t - 11).labels(&["INBOX"]).done(),
    ])
    .unwrap();
    use MailboxView::*;
    assert_eq!(
        list(&s, Inbox, None, None),
        vec!["imp", "news", "promo", "other", "home"]
    );
    assert_eq!(
        list(&s, Inbox, Some(InboxTab::All), Some(A)),
        vec!["imp", "news", "promo", "other"]
    );
    assert_eq!(
        list(&s, Inbox, Some(InboxTab::Important), None),
        vec!["imp"]
    );
    assert_eq!(
        list(&s, Inbox, Some(InboxTab::Newsletters), None),
        vec!["news", "promo"]
    );
    assert_eq!(
        list(&s, Inbox, Some(InboxTab::Other), None),
        vec!["other", "home"]
    );
    assert_eq!(
        list(&s, Inbox, Some(InboxTab::Other), Some(B)),
        vec!["home"]
    );
    assert_eq!(list(&s, Starred, None, None), vec!["starred"]);
    assert_eq!(list(&s, Sent, None, None), vec!["sent"]);
    assert_eq!(list(&s, Drafts, None, None), vec!["draft"]);
    assert_eq!(
        list(&s, Done, None, None),
        vec!["archived", "starred", "sent"]
    );
    assert_eq!(list(&s, Trash, None, None), vec!["trash"]);
    assert_eq!(list(&s, Spam, None, None), vec!["spam"]);
    assert_eq!(
        list(&s, All, None, Some(A)),
        vec!["imp", "news", "promo", "other", "archived", "starred", "sent", "draft"]
    );
    // Trashed messages leave label views.
    assert_eq!(
        list(&s, Label("Label_1".into()), None, None),
        vec!["archived"]
    );

    // Archive: thread moves from Inbox to Done.
    s.modify_thread_labels(A, "other", &[], &["INBOX".into()])
        .unwrap();
    assert!(!list(&s, Inbox, None, None).contains(&"other".to_string()));
    assert_eq!(
        list(&s, Done, None, None),
        vec!["other", "archived", "starred", "sent"]
    );
    // Move back.
    s.modify_message_labels(A, &["a4".into()], &["INBOX".into()], &[])
        .unwrap();
    assert!(list(&s, Inbox, None, None).contains(&"other".to_string()));
    // Trash via labels.
    s.modify_thread_labels(A, "imp", &["TRASH".into()], &["INBOX".into()])
        .unwrap();
    assert_eq!(list(&s, Trash, None, None), vec!["imp", "trash"]);
    assert!(!list(&s, All, None, None).contains(&"imp".to_string()));
}

#[test]
fn pagination_cursor() {
    let s = store_with_accounts(&[A]);
    let t = now();
    let msgs: Vec<Message> = (0..25)
        .map(|i| M::new(A, &format!("m{i}"), &format!("t{i}"), t - i * 1000).done())
        .collect();
    s.upsert_messages(&msgs).unwrap();
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let page = s
            .list_threads(&ListQuery {
                view: MailboxView::Inbox,
                tab: None,
                account_id: None,
                account_ids: None,
                limit: 10,
                before,
                unread_only: false,
                split: None,
            })
            .unwrap();
        if page.is_empty() {
            break;
        }
        before = Some(page.last().unwrap().last_date);
        seen.extend(page.into_iter().map(|t| t.thread_id));
    }
    assert_eq!(seen, (0..25).map(|i| format!("t{i}")).collect::<Vec<_>>());
}

#[test]
fn search_operators() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.replace_labels(
        A,
        &[Label {
            account_id: A.into(),
            id: "Label_9".into(),
            name: "Clients/Acme Corp".into(),
            kind: "user".into(),
            color: None,
            unread_count: None,
            hidden: false,
        }],
    )
    .unwrap();
    s.upsert_messages(&[
        M::new(A, "lease", "t1", t - 100 * DAY)
            .from("Mike Delgado", "mike@delgado.example")
            .subject("Lease renewal for 2026")
            .body("Attached is the lease renewal. Early termination clause on page 3.")
            .attach("Lease_Renewal_INV-2041.pdf", "application/pdf")
            .done(),
        M::new(A, "budget", "t2", t - 2 * DAY)
            .from("Priya Nair", "priya@nair.example")
            .to(&[("Mike Delgado", "mike@delgado.example")])
            .subject("Q3 budget")
            .body("Numbers in the sheet")
            .attach(
                "q3-budget.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            )
            .labels(&["INBOX", "UNREAD", "Label_9"])
            .done(),
        M::new(A, "photo", "t3", t - 400 * DAY)
            .from("Bo Chen", "bo@chen.example")
            .subject("Photos from the offsite")
            .body("see pics")
            .attach("IMG_2041.jpg", "image/jpeg")
            .labels(&["STARRED"])
            .done(),
        M::new(A, "news", "t4", t - DAY)
            .from("Weekly", "news@weekly.example")
            .subject("This week in lease law")
            .body("renewal season")
            .inline_image()
            .unsubscribe()
            .done(),
    ])
    .unwrap();

    let ids = |q: &str| {
        let mut v = hit_ids(&search(&s, q));
        v.sort();
        v
    };
    assert_eq!(ids("lease"), vec!["lease", "news"]);
    assert_eq!(ids("from:mike"), vec!["lease"]);
    assert_eq!(ids("from:mike@delgado.example"), vec!["lease"]);
    assert_eq!(ids("from:delgado.example"), vec!["lease"]);
    assert_eq!(ids("to:mike"), vec!["budget"]);
    assert_eq!(ids("subject:budget"), vec!["budget"]);
    assert_eq!(ids("subject:\"lease renewal\""), vec!["lease"]);
    assert_eq!(ids("has:pdf"), vec!["lease"]);
    assert_eq!(ids("has:spreadsheet"), vec!["budget"]);
    assert_eq!(ids("has:image"), vec!["photo"]); // the newsletter's inline logo does not count
    assert_eq!(ids("has:attachment"), vec!["budget", "lease", "photo"]);
    assert_eq!(ids("filename:2041"), vec!["lease", "photo"]);
    assert_eq!(ids("filename:q3"), vec!["budget"]);
    // Words of a name, whatever joins them in the file.
    assert_eq!(ids("filename:\"q3 budget\""), vec!["budget"]);
    assert_eq!(ids("filename:\"lease 2041\""), vec!["lease"]);
    assert!(ids("filename:\"q3 lease\"").is_empty());
    assert_eq!(ids("filename:renewal_inv"), vec!["lease"]);
    assert_eq!(ids("label:acme-corp"), vec!["budget"]);
    assert_eq!(ids("label:clients-acme-corp"), vec!["budget"]);
    assert_eq!(ids("label:\"Clients/Acme Corp\""), vec!["budget"]);
    // A word of the name, and the name typed without quotes.
    assert_eq!(ids("label:acme"), vec!["budget"]);
    assert_eq!(ids("label:clients/acme corp"), vec!["budget"]);
    assert_eq!(ids("label:clients/acme corp numbers"), vec!["budget"]);
    let chip = search(&s, "label:clients/acme corp numbers").chips;
    assert_eq!(chip[0].raw, "label:clients/acme corp");
    assert!(ids("label:nonexistent").is_empty());
    assert_eq!(ids("is:unread"), vec!["budget"]);
    assert_eq!(ids("is:starred"), vec!["photo"]);
    assert_eq!(ids("in:inbox"), vec!["budget", "lease", "news"]);
    assert_eq!(ids("-in:inbox"), vec!["photo"]);
    assert_eq!(ids("lease -law"), vec!["lease"]);
    assert_eq!(ids("lease -from:weekly"), vec!["lease"]);
    assert!(ids("-lease -budget -photos").is_empty());
    assert_eq!(ids("-lease -budget"), vec!["photo"]);
    assert_eq!(ids("lease OR budget"), vec!["budget", "lease", "news"]);
    assert_eq!(ids("has:pdf OR has:image"), vec!["lease", "photo"]);
    assert_eq!(ids("\"early termination\""), vec!["lease"]);
    assert!(ids("\"termination early\"").is_empty());
    assert_eq!(ids("renew"), vec!["lease", "news"]); // as-you-type prefix
    assert!(ids("renew ").is_empty()); // completed word: exact
    assert_eq!(ids("inv-2041"), vec!["lease"]);
    assert_eq!(ids("newer_than:7d"), vec!["budget", "news"]);
    assert_eq!(ids("older_than:1y"), vec!["photo"]);
    assert_eq!(ids("lease newer_than:30d"), vec!["news"]);
    assert!(ids("before:2000-01-01").is_empty());
    assert_eq!(ids("mike has:pdf"), vec!["lease"]);
    assert_eq!(ids("café"), Vec::<String>::new());
}

#[test]
fn search_scope_rules() {
    let s = store_with_accounts(&[A, B]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "inbox", "t1", t - 1).body("platypus").done(),
        M::new(A, "trash", "t2", t - 2)
            .body("platypus")
            .labels(&["TRASH"])
            .done(),
        M::new(A, "spam", "t3", t - 3)
            .body("platypus")
            .labels(&["SPAM"])
            .done(),
        M::new(B, "home", "t4", t - 4).body("platypus").done(),
    ])
    .unwrap();
    let ids = |q: &str, acct: Option<&str>| {
        let mut v = hit_ids(
            &s.search(&SearchRequest {
                query: q.into(),
                account_id: acct.map(Into::into),
                account_ids: None,
                limit: 50,
            })
            .unwrap(),
        );
        v.sort();
        v
    };
    assert_eq!(ids("platypus", None), vec!["home", "inbox"]);
    assert_eq!(ids("platypus in:trash", None), vec!["trash"]);
    assert_eq!(ids("platypus in:spam", None), vec!["spam"]);
    assert_eq!(
        ids("platypus in:anywhere", None),
        vec!["home", "inbox", "spam", "trash"]
    );
    assert_eq!(ids("platypus", Some(A)), vec!["inbox"]);
    assert_eq!(ids("platypus account:home", None), vec!["home"]);
    assert_eq!(ids("platypus account:me@work", None), vec!["inbox"]);
    assert!(ids("platypus account:home", Some(A)).is_empty());
    // account: also matches the user's nickname for the account.
    s.update_account(A, Some(Some("Day Job")), None).unwrap();
    assert_eq!(ids("platypus account:\"day job\"", None), vec!["inbox"]);
    assert_eq!(ids("platypus account:day", None), vec!["inbox"]);
    assert!(ids("platypus account:nobody", None).is_empty());
    // Filter-only queries obey scope too.
    assert_eq!(ids("in:anywhere", Some(A)), vec!["inbox", "spam", "trash"]);
    assert_eq!(ids("is:unread", None), Vec::<String>::new());
}

#[test]
fn snippets_are_escaped_and_highlighted() {
    let s = store_with_accounts(&[A]);
    s.upsert_messages(&[M::new(A, "x", "t1", now())
        .subject("<img src=x onerror=alert(1)>")
        .body("Hello <script>alert('pwn')</script> the invoice & \"receipt\" <b>INV-7</b> is here.\n\nOn Mon, Jan 5, 2026 at 9:00 AM Bo <bo@x.example> wrote:\n> old invoice")
        .done()])
    .unwrap();
    let r = search(&s, "invoice");
    let snip = &r.hits[0].snippet_html;
    assert!(snip.contains("<mark>invoice</mark>"), "{snip}");
    let stripped = snip.replace("<mark>", "").replace("</mark>", "");
    assert!(!stripped.contains('<') && !stripped.contains('>'), "{snip}");
    assert!(
        snip.contains("alert(&#39;pwn&#39;)&lt;/script&gt;"),
        "{snip}"
    );
    assert!(snip.contains("&amp;") && snip.contains("&quot;receipt&quot;"));
    // The authored body is preferred over the quoted "old invoice".
    assert!(!snip.contains("old"));
    // Prefix highlight while typing.
    let r = search(&s, "invo");
    assert!(r.hits[0].snippet_html.contains("<mark>invoice</mark>"));
    // Match only in the quoted part: that part is shown.
    let r = search(&s, "old");
    assert!(
        r.hits[0].snippet_html.contains("<mark>old</mark>"),
        "{}",
        r.hits[0].snippet_html
    );
    // Hostile query text never reaches the HTML unescaped.
    let r = search(&s, "<script>");
    for h in &r.hits {
        assert!(!h
            .snippet_html
            .replace("<mark>", "")
            .replace("</mark>", "")
            .contains('<'));
    }
    // Match only in an attachment name: say so.
    s.upsert_messages(&[M::new(A, "f", "t3", now())
        .body("see attached")
        .attach("Budget<Q3>.xlsx", "text/csv")
        .done()])
        .unwrap();
    let r = search(&s, "budget");
    assert_eq!(
        r.hits[0].snippet_html,
        "Attachment: <mark>Budget</mark>&lt;Q3&gt;.xlsx"
    );
    // Filter-only: escaped plain snippet.
    s.upsert_messages(&[M::new(A, "y", "t2", now()).body("x").done()])
        .unwrap();
    let mut m = s.get_message(A, "y").unwrap().unwrap();
    m.snippet = "<b>bold</b> & co".into();
    s.upsert_messages(&[m]).unwrap();
    let r = search(&s, "in:inbox");
    let y = r.hits.iter().find(|h| h.message_id == "y").unwrap();
    assert_eq!(y.snippet_html, "&lt;b&gt;bold&lt;/b&gt; &amp; co");
}

#[test]
fn hostile_queries_never_error() {
    let s = store_with_accounts(&[A]);
    s.upsert_messages(&[M::new(A, "x", "t1", now()).body("alpha beta gamma").done()])
        .unwrap();
    for q in [
        "\"",
        "\"\"",
        "before:275000-01-01 alpha",
        "after:99999999/12/31",
        "older_than:999999999y alpha",
        "newer_than:999999999y",
        "alpha\"",
        "a\"b",
        "*",
        "alpha*",
        "^alpha",
        "NEAR(alpha beta)",
        "alpha AND",
        "AND",
        "NOT",
        "alpha NOT beta",
        "{subject}: alpha",
        "subject:",
        "(alpha",
        "alpha)",
        "-",
        "--alpha",
        "-\"",
        "'; DROP TABLE messages; --",
        "%",
        "_",
        "from:%",
        "filename:%_",
        "label:'",
        "\u{0}",
        "a\u{0}b",
        "🙂",
        "é",
        "ab",
        "a",
        "OR OR OR",
        "alpha OR",
        "会議室s émails ÆØÅ 🎉🎉",
        "ødegård+tag straße 1.234,56 +44 020",
        "{alpha NOT (beta AROUND 3",
        "from:\"",
        "before:",
        "account:",
        "has:",
        "in:",
        "is:",
    ] {
        let r = s.search(&SearchRequest {
            query: q.into(),
            account_id: None,
            account_ids: None,
            limit: 50,
        });
        assert!(r.is_ok(), "query {q:?} failed: {r:?}");
    }
    // Uppercase NOT excludes (Outlook, Apple Mail): x has both words.
    assert!(search(&s, "alpha NOT beta").hits.is_empty());
    // Uppercase AND is Gmail's (optional) conjunction, not a word.
    assert_eq!(hit_ids(&search(&s, "alpha AND")), vec!["x"]);
    assert_eq!(hit_ids(&search(&s, "alpha AND beta")), vec!["x"]);
    // (A word one edit from an indexed one would be respelled: "zeta" → "beta".)
    assert!(search(&s, "alpha AND qwxyz").hits.is_empty());
}

#[test]
fn ranking_prefers_subject_recency_and_known_people() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "body_old", "t1", t - 900 * DAY).body("the quarterly report is attached").done(),
        M::new(A, "subject_new", "t2", t - DAY).subject("Quarterly report").body("see attached").done(),
        M::new(A, "quoted", "t3", t - DAY).body("thanks\n\nOn Mon, Jan 5, 2026 at 9:00 AM Bo <bo@x.example> wrote:\n> the quarterly report").done(),
    ])
    .unwrap();
    let r = search(&s, "quarterly report");
    assert_eq!(r.hits[0].message_id, "subject_new");
    assert_eq!(r.hits.last().unwrap().message_id, "quoted");

    // A bare name boosts that sender without filtering others out.
    s.upsert_messages(&[
        M::new(A, "from_mike", "t4", t - 300 * DAY)
            .from("Mike Delgado", "mike@d.example")
            .body("contract draft")
            .done(),
        M::new(A, "about_contract", "t5", t - 2 * DAY)
            .from("Ana Ruiz", "ana@r.example")
            .body("contract draft")
            .done(),
    ])
    .unwrap();
    let r = search(&s, "mike contract");
    assert_eq!(hit_ids(&r), vec!["from_mike"]); // AND semantics: only the mail mentioning mike (sender field)
    let r = search(&s, "contract draft");
    assert_eq!(r.hits.len(), 2);
    assert_eq!(r.people.len(), 0);
    let r = search(&s, "mike");
    assert_eq!(r.people[0].address.email, "mike@d.example");
    let r = search(&s, "from:mike");
    assert_eq!(r.chips[0].label, "From Mike Delgado");
}

#[test]
fn people_and_sent_boost() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "s1", "t1", t - DAY)
            .from("Me", A)
            .to(&[("Sam Ortiz", "sam@ortiz.example")])
            .labels(&["SENT"])
            .body("hello")
            .done(),
        M::new(A, "s2", "t2", t - DAY)
            .from("Me", A)
            .to(&[("Sam Ortiz", "sam@ortiz.example")])
            .labels(&["SENT"])
            .body("hello")
            .done(),
        M::new(A, "r1", "t3", t - 10 * DAY)
            .from("Sam Ortiz", "sam@ortiz.example")
            .body("widget order")
            .done(),
        M::new(A, "r2", "t4", t - 10 * DAY)
            .from("Samantha Lee", "samantha@lee.example")
            .body("widget order")
            .done(),
    ])
    .unwrap();
    let r = search(&s, "sam");
    let emails: Vec<_> = r.people.iter().map(|p| p.address.email.as_str()).collect();
    assert_eq!(emails, vec!["sam@ortiz.example", "samantha@lee.example"]);
    assert_eq!(r.people[0].message_count, 3);
    assert!(!emails.contains(&A)); // never yourself
    let r = search(&s, "widget order");
    assert_eq!(
        r.hits[0].message_id, "r1",
        "sender you write to ranks first"
    );
    // Deleting the sent mail removes the boost source.
    s.delete_messages(A, &["s1".into(), "s2".into()]).unwrap();
    let r = search(&s, "sam ortiz");
    assert_eq!(r.people[0].message_count, 1);
}

/// People are found by the words of their current name only
/// (`people_words` follows `search_key`): a newer message under another
/// name replaces the old words, an older one doesn't.
#[test]
fn people_found_by_their_current_name() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[M::new(A, "r1", "t1", t - 10 * DAY)
        .from("Robin Vale", "rv@quill.example")
        .body("hi")
        .done()])
        .unwrap();
    let names = |q: &str| -> Vec<String> {
        search(&s, q)
            .people
            .iter()
            .map(|p| p.address.email.clone())
            .collect()
    };
    assert_eq!(names("vale"), vec!["rv@quill.example"]);
    // An older message with another name doesn't rename.
    s.upsert_messages(&[M::new(A, "r0", "t0", t - 20 * DAY)
        .from("Bobby Tables", "rv@quill.example")
        .body("hi")
        .done()])
        .unwrap();
    assert!(names("tables").is_empty());
    assert_eq!(names("robin"), vec!["rv@quill.example"]);
    // A newer one does: the old words go, the new ones match by prefix.
    s.upsert_messages(&[M::new(A, "r2", "t2", t - DAY)
        .from("Robin Castellanos", "rv@quill.example")
        .body("hi")
        .done()])
        .unwrap();
    assert!(names("vale").is_empty());
    assert_eq!(names("castel"), vec!["rv@quill.example"]);
    assert_eq!(names("quill"), vec!["rv@quill.example"]);
}

#[test]
fn attachments_panel() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "m1", "t1", t - DAY)
            .body("see attached")
            .attach("Lease_2026.pdf", "application/pdf")
            .attach("floorplan.png", "image/png")
            .done(),
        M::new(A, "m2", "t2", t - 2 * DAY)
            .subject("lease question")
            .body("no files")
            .done(),
        M::new(A, "m3", "t3", t - 3 * DAY)
            .subject("photos")
            .body("lease photos")
            .attach("unit.jpg", "image/jpeg")
            .inline_image()
            .done(),
    ])
    .unwrap();
    let r = search(&s, "lease");
    let files: Vec<_> = r
        .attachments
        .iter()
        .map(|a| a.attachment.filename.as_str())
        .collect();
    assert_eq!(files[0], "Lease_2026.pdf", "{files:?}");
    assert!(files.contains(&"unit.jpg"));
    assert!(!files.contains(&"logo.png"));
    let r = search(&s, "lease has:image");
    let files: Vec<_> = r
        .attachments
        .iter()
        .map(|a| a.attachment.filename.as_str())
        .collect();
    assert_eq!(files, vec!["floorplan.png", "unit.jpg"]);
    assert_eq!(r.attachments[0].message_id, "m1");
}

#[test]
fn remove_account_wipes_data() {
    let s = store_with_accounts(&[A, B]);
    s.upsert_messages(&[
        M::new(A, "a", "t1", now())
            .from("Zed", "zed@x.example")
            .body("kiwi")
            .done(),
        M::new(B, "b", "t2", now())
            .from("Zed", "zed@x.example")
            .body("kiwi")
            .done(),
    ])
    .unwrap();
    s.set_sync_cursor(
        A,
        &SyncCursor {
            provider_state: r#"{"historyId":5}"#.into(),
            backfill_done: true,
            failed_message_ids: vec![],
            window: Default::default(),
        },
    )
    .unwrap();
    s.remove_account(A).unwrap();
    assert_eq!(hit_ids(&search(&s, "kiwi")), vec!["b"]);
    assert_eq!(s.count_messages(None).unwrap(), 1);
    assert_eq!(s.list_accounts().unwrap().len(), 1);
    assert_eq!(s.get_sync_cursor(A).unwrap(), SyncCursor::default());
    assert_eq!(search(&s, "zed").people[0].message_count, 1);
    assert!(list(&s, MailboxView::Inbox, None, Some(A)).is_empty());
}

#[test]
fn file_backed_cursor_persistence_and_readers() {
    let dir = std::env::temp_dir().join(format!(
        "penguin-core-test-{}-{}",
        std::process::id(),
        now()
    ));
    let path = dir.join("mail.db");
    {
        let s = Store::open(&path).unwrap();
        s.upsert_account(&account(A)).unwrap();
        let cur = SyncCursor {
            provider_state: r#"{"historyId":12345,"backfillPageToken":"tok"}"#.into(),
            backfill_done: false,
            failed_message_ids: vec!["bad1".into(), "bad2".into()],
            window: WindowCursor {
                full_since_ms: Some(1_700_000_000_000),
                fill_page_token: Some("fill-tok".into()),
                older_mode: Some("headers".into()),
                ..Default::default()
            },
        };
        s.set_sync_cursor(A, &cur).unwrap();
        assert_eq!(s.get_sync_cursor(A).unwrap(), cur);
        s.upsert_messages(&[M::new(A, "m1", "t1", now())
            .body("persistent narwhal")
            .done()])
            .unwrap();
        // Reader connections (separate from the writer) see committed data.
        assert_eq!(hit_ids(&search(&s, "narwhal")), vec!["m1"]);
        // Concurrent readers from several threads.
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let s = s.clone();
                std::thread::spawn(move || search(&s, "narwhal").hits.len())
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), 1);
        }
    }
    let s = Store::open(&path).unwrap();
    // The provider's state comes back byte for byte.
    assert_eq!(
        s.get_sync_cursor(A).unwrap().provider_state,
        r#"{"historyId":12345,"backfillPageToken":"tok"}"#
    );
    assert_eq!(
        s.get_sync_cursor(A).unwrap().failed_message_ids,
        vec!["bad1", "bad2"]
    );
    assert_eq!(hit_ids(&search(&s, "narwhal")), vec!["m1"]);
    s.optimize().unwrap();
    assert_eq!(hit_ids(&search(&s, "narwhal")), vec!["m1"]);
    drop(s);
    std::fs::remove_dir_all(&dir).ok();
}

/// `optimize` merges the full-text index in short steps: a write issued
/// while it runs (sync of another account, an archive) goes through within
/// a step instead of waiting for the whole merge, and the index still ends
/// as one segment with the same search results. The steps here are a
/// quarter of the app's, so this small index still takes several of them
/// on a fast machine; the step latency itself is in the benchmark
/// (`examples/bench.rs`, index maintenance).
#[test]
fn optimize_lets_other_writes_through() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let dir = std::env::temp_dir().join(format!(
        "penguin-core-optimize-{}-{}",
        std::process::id(),
        now()
    ));
    let path = dir.join("mail.db");
    let s = Store::open(&path).unwrap();
    s.upsert_account(&account(A)).unwrap();
    // Wordy mail in many small transactions: each commit writes a new
    // index segment, so there is a real merge to do.
    let mut seed = 7u64;
    let mut word = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        format!("w{}", (seed >> 33) % 40_000)
    };
    let t0 = now() - 400 * DAY;
    for chunk in 0..300 {
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                let n = chunk * 10 + i;
                let body: Vec<String> = (0..150).map(|_| word()).collect();
                M::new(A, &format!("m{n}"), &format!("t{n}"), t0 + n * 60_000)
                    .body(&format!("{} narwhal{n}", body.join(" ")))
                    .done()
            })
            .collect();
        s.upsert_messages(&msgs).unwrap();
    }
    let segments = fts_segments;
    assert!(segments(&s) > 1);
    let before = hit_ids(&search(&s, "narwhal1234"));
    assert_eq!(before, vec!["m1234"]);

    let done = Arc::new(AtomicBool::new(false));
    let maintenance = {
        let (s, done) = (s.clone(), done.clone());
        std::thread::spawn(move || {
            let t = Instant::now();
            let steps = s.merge_fts("messages_fts", 64).unwrap();
            done.store(true, Ordering::Release);
            (t.elapsed(), steps)
        })
    };
    let (mut writes, mut worst) = (0, Duration::ZERO);
    while !done.load(Ordering::Acquire) {
        let star = writes % 2 == 0;
        let (add, remove) = if star {
            (vec!["STARRED".to_string()], vec![])
        } else {
            (vec![], vec!["STARRED".to_string()])
        };
        let t = Instant::now();
        s.modify_thread_labels(A, "t5", &add, &remove).unwrap();
        worst = worst.max(t.elapsed());
        writes += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
    let (took, steps) = maintenance.join().unwrap();
    // The merge must have overlapped the writes for this to test anything.
    assert!(
        steps >= 3 && writes >= 3,
        "the merge took {took:?} in {steps} steps, too short to overlap writes"
    );
    assert!(
        worst < Duration::from_millis(250),
        "a write waited {worst:?} while the merge ran ({took:?}, {steps} steps, {writes} writes)"
    );
    assert_eq!(segments(&s), 1);
    assert_eq!(hit_ids(&search(&s, "narwhal1234")), before);
    // The whole of optimize (both indexes, planner statistics) with the
    // app's step size: nothing left to merge.
    s.optimize().unwrap();
    assert_eq!(segments(&s), 1);
    drop(s);
    std::fs::remove_dir_all(&dir).ok();
}

/// Ingest settings: FTS5 merges at 8 segments a level (a migration), and
/// the writer checkpoints only as a backstop (a background thread does it,
/// see below).
#[test]
fn ingest_settings() {
    let s = store_with_accounts(&[A]);
    let (automerge, checkpoint): (i64, i64) = s
        .read(|c| {
            Ok((
                c.query_row(
                    "SELECT v FROM messages_fts_config WHERE k = 'automerge'",
                    [],
                    |r| r.get(0),
                )?,
                c.query_row("PRAGMA wal_autocheckpoint", [], |r| r.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!((automerge, checkpoint), (8, 16_384));
}

/// The WAL is checkpointed by a background thread, not by the writes, and
/// once everything is copied the next write starts the WAL over (else it
/// would grow with every write).
#[test]
fn wal_is_checkpointed_in_the_background() {
    use std::time::{Duration, Instant};
    let dir = std::env::temp_dir().join(format!(
        "penguin-core-checkpoint-{}-{}",
        std::process::id(),
        now()
    ));
    let path = dir.join("mail.db");
    let s = Store::open(&path).unwrap();
    s.upsert_account(&account(A)).unwrap();
    let t0 = now() - 100 * DAY;
    for chunk in 0..20 {
        let msgs: Vec<Message> = (0..20)
            .map(|i| {
                let n = chunk * 20 + i;
                M::new(A, &format!("m{n}"), &format!("t{n}"), t0 + n * 60_000)
                    .body(&format!("wal churn {n} {}", "lorem ipsum ".repeat(50)))
                    .done()
            })
            .collect();
        s.upsert_messages(&msgs).unwrap();
    }
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let (runs, idle) = s.checkpoint_progress();
        if runs > 0 && idle {
            break;
        }
        assert!(Instant::now() < until, "no background checkpoint");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Everything is copied: this write starts the WAL over, so it is all
    // the WAL holds (reported by a checkpoint on another connection).
    s.upsert_messages(&[M::new(A, "last", "tl", t0).done()])
        .unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    let frames: i64 = c
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| r.get(1))
        .unwrap();
    assert!(frames < 50, "the WAL didn't start over: {frames} frames");
    drop(c);
    assert_eq!(search(&s, "churn").hits.len(), 50);
    drop(s);
    std::fs::remove_dir_all(&dir).ok();
}

fn fts_segments(s: &Store) -> i64 {
    s.read(|c| {
        Ok(c.query_row(
            "SELECT count(DISTINCT segid) FROM messages_fts_idx",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

/// A batch is written oldest first whatever order it arrives in: FTS5
/// starts a new segment whenever a rowid is lower than the previous one,
/// so a newest-first batch (Gmail's order) used to leave one segment per
/// message. Re-storing a batch (bodies filled in later) likewise.
#[test]
fn a_newest_first_batch_is_one_index_segment() {
    let s = store_with_accounts(&[A]);
    let t0 = now() - 30 * DAY;
    let batch: Vec<Message> = (0..40)
        .rev()
        .map(|i| {
            M::new(A, &format!("m{i}"), &format!("t{}", i % 7), t0 + i * 60_000)
                .body(&format!("quokka note {i}"))
                .attach(&format!("scan{i}.pdf"), "application/pdf")
                .done()
        })
        .collect();
    s.upsert_messages(&batch).unwrap();
    assert_eq!(fts_segments(&s), 1);
    assert_eq!(s.count_messages(None).unwrap(), 40);
    let r = search(&s, "quokka");
    assert_eq!(r.hits.len(), 7);
    // The newest message of each thread leads it.
    assert_eq!(hit_ids(&r)[0], "m39");
    let ids_before: Vec<String> = s
        .get_message(A, "m3")
        .unwrap()
        .unwrap()
        .attachments
        .iter()
        .map(|a| a.id.clone())
        .collect();

    // Stored again, as when bodies are fetched after headers: deletes then
    // inserts, one new segment; attachment ids kept, nothing counted twice.
    let mut again = batch.clone();
    for m in &mut again {
        m.body_text = format!("{} wombat", m.body_text);
        for a in &mut m.attachments {
            a.id = format!("fresh-{}", a.id);
        }
    }
    s.upsert_messages(&again).unwrap();
    assert!(fts_segments(&s) <= 2, "{} segments", fts_segments(&s));
    assert_eq!(s.count_messages(None).unwrap(), 40);
    assert_eq!(search(&s, "wombat").hits.len(), 7);
    let ids_after: Vec<String> = s
        .get_message(A, "m3")
        .unwrap()
        .unwrap()
        .attachments
        .iter()
        .map(|a| a.id.clone())
        .collect();
    assert_eq!(ids_after, ids_before);

    // A batch naming a message twice keeps its order: the last copy wins.
    s.upsert_messages(&[
        M::new(A, "dup", "td", t0 + 5 * DAY)
            .subject("first copy")
            .done(),
        M::new(A, "dup", "td", t0).subject("second copy").done(),
    ])
    .unwrap();
    let dup = s.get_message(A, "dup").unwrap().unwrap();
    assert_eq!((dup.subject.as_str(), dup.date), ("second copy", t0));
    assert_eq!(s.count_messages(None).unwrap(), 41);
}

#[test]
fn same_millisecond_messages_get_distinct_rowids() {
    let s = store_with_accounts(&[A]);
    let t = now();
    let msgs: Vec<Message> = (0..50)
        .map(|i| {
            M::new(A, &format!("m{i}"), "t1", t)
                .body("same time lynx")
                .done()
        })
        .collect();
    s.upsert_messages(&msgs).unwrap();
    assert_eq!(s.count_messages(None).unwrap(), 50);
    assert_eq!(search(&s, "lynx").hits[0].match_count, 50);
}

#[test]
fn date_filters_use_message_dates() {
    let s = store_with_accounts(&[A]);
    let t = now();
    s.upsert_messages(&[
        M::new(A, "recent", "t1", t - 2 * DAY).body("ocelot").done(),
        M::new(A, "old", "t2", t - 60 * DAY).body("ocelot").done(),
    ])
    .unwrap();
    assert_eq!(hit_ids(&search(&s, "ocelot newer_than:7d")), vec!["recent"]);
    assert_eq!(hit_ids(&search(&s, "ocelot older_than:30d")), vec!["old"]);
    assert_eq!(hit_ids(&search(&s, "newer_than:7d")), vec!["recent"]);
    assert!(search(&s, "ocelot after:2030-01-01").hits.is_empty());
    assert!(search(&s, "ocelot after:2026-06-01 before:2026-01-01")
        .hits
        .is_empty());
}

#[test]
fn empty_query_returns_count_only() {
    let s = store_with_accounts(&[A]);
    s.upsert_messages(&[M::new(A, "m", "t", now()).done()])
        .unwrap();
    let r = search(&s, "   ");
    assert!(r.hits.is_empty());
    assert_eq!(r.indexed_messages, 1);
}

#[test]
fn bodies_are_compressed_and_legacy_text_rows_still_read() {
    let s = store_with_accounts(&[A]);
    let html = format!(
        "<html><body>{}</body></html>",
        "<p>Quarterly numbers attached.</p>".repeat(500)
    );
    let mut m = M::new(A, "big", "t1", now())
        .body(&"Quarterly numbers attached. ".repeat(500))
        .done();
    m.body_html = Some(html.clone());
    s.upsert_messages(std::slice::from_ref(&m)).unwrap();
    let (text_len, html_len, text_type): (i64, i64, String) = s
        .read(|c| Ok(c.query_row("SELECT length(body_text), length(body_html), typeof(body_html) FROM message_bodies", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?))
        .unwrap();
    assert_eq!(text_type, "blob");
    assert!(
        (html_len as usize) < html.len() / 10,
        "html stored as {html_len} bytes"
    );
    assert!((text_len as usize) < m.body_text.len() / 10);
    assert_eq!(s.get_message(A, "big").unwrap().unwrap(), m);
    assert!(search(&s, "quarterly").hits[0]
        .snippet_html
        .contains("<mark>Quarterly</mark>"));
    // A row written as plain TEXT (pre-compression layout) decodes too.
    s.read(|c| Ok(c.execute("UPDATE message_bodies SET body_text = 'legacy plain body', body_html = '<p>legacy</p>'", [])?)).unwrap();
    let got = s.get_message(A, "big").unwrap().unwrap();
    assert_eq!(got.body_text, "legacy plain body");
    assert_eq!(got.body_html.as_deref(), Some("<p>legacy</p>"));
}

#[test]
fn sender_authenticated_round_trips_and_survives_label_changes() {
    let s = store_with_accounts(&["a@x.example"]);
    let mut authed = M::new("a@x.example", "m1", "t1", now())
        .labels(&["INBOX", "UNREAD"])
        .done();
    authed.sender_authenticated = true;
    let plain = M::new("a@x.example", "m2", "t2", now())
        .labels(&["INBOX"])
        .done();
    s.upsert_messages(&[authed, plain]).unwrap();
    assert!(
        s.get_message("a@x.example", "m1")
            .unwrap()
            .unwrap()
            .sender_authenticated
    );
    assert!(
        !s.get_message("a@x.example", "m2")
            .unwrap()
            .unwrap()
            .sender_authenticated
    );

    // Label deltas (history sync, optimistic actions) keep the bit.
    s.modify_message_labels(
        "a@x.example",
        &["m1".into()],
        &["STARRED".into()],
        &["UNREAD".into()],
    )
    .unwrap();
    s.modify_thread_labels("a@x.example", "t1", &[], &["INBOX".into()])
        .unwrap();
    let m1 = s.get_message("a@x.example", "m1").unwrap().unwrap();
    assert!(m1.sender_authenticated);
    assert!(m1.is_starred() && !m1.is_unread());
}

// ---------- account sets (profiles) ----------

const C: &str = "me@side.example";

fn list_in(
    s: &Store,
    account: Option<&str>,
    ids: Option<&[&str]>,
    limit: u32,
    before: Option<i64>,
) -> Vec<ThreadSummary> {
    s.list_threads(&ListQuery {
        view: MailboxView::Inbox,
        tab: None,
        account_id: account.map(Into::into),
        account_ids: ids.map(|v| v.iter().map(|a| a.to_string()).collect()),
        limit,
        before,
        unread_only: false,
        split: None,
    })
    .unwrap()
}

/// Interleaved threads across three accounts: thread i belongs to account i % 3.
fn three_account_store() -> Store {
    let s = store_with_accounts(&[A, B, C]);
    let base = now() - 100 * DAY;
    let msgs: Vec<Message> = (0..30)
        .map(|i| {
            let acct = [A, B, C][i % 3];
            M::new(
                acct,
                &format!("m{i}"),
                &format!("t{i}"),
                base + i as i64 * DAY,
            )
            .subject(&format!("quarterly report {i}"))
            .labels(if i % 2 == 0 {
                &["INBOX", "UNREAD"]
            } else {
                &["INBOX"]
            })
            .done()
        })
        .collect();
    s.upsert_messages(&msgs).unwrap();
    s
}

#[test]
fn account_scope_intersects_and_dedupes() {
    let ids: Vec<AccountId> = vec![A.into(), B.into(), A.into()];
    assert_eq!(account_scope(None, None), None);
    assert_eq!(account_scope(Some(A), None), Some(vec![A.to_string()]));
    assert_eq!(
        account_scope(None, Some(&ids)),
        Some(vec![A.to_string(), B.to_string()])
    );
    assert_eq!(
        account_scope(Some(B), Some(&ids)),
        Some(vec![B.to_string()])
    );
    assert_eq!(account_scope(Some(C), Some(&ids)), Some(vec![]));
    assert_eq!(account_scope(None, Some(&[])), Some(vec![]));
}

#[test]
fn list_threads_over_an_account_set_merges_newest_first() {
    let s = three_account_store();
    let got = list_in(&s, None, Some(&[A, C]), 100, None);
    let want: Vec<String> = (0..30)
        .rev()
        .filter(|i| i % 3 != 1)
        .map(|i| format!("t{i}"))
        .collect();
    assert_eq!(
        got.iter().map(|t| t.thread_id.clone()).collect::<Vec<_>>(),
        want
    );
    assert!(got.iter().all(|t| t.account_id != B));
    assert!(got.windows(2).all(|w| w[0].last_date > w[1].last_date));

    // The limit applies to the merged list, and paging with `before` continues it.
    let page1 = list_in(&s, None, Some(&[A, C]), 5, None);
    assert_eq!(
        page1
            .iter()
            .map(|t| t.thread_id.clone())
            .collect::<Vec<_>>(),
        want[..5]
    );
    let page2 = list_in(&s, None, Some(&[A, C]), 5, Some(page1[4].last_date));
    assert_eq!(
        page2
            .iter()
            .map(|t| t.thread_id.clone())
            .collect::<Vec<_>>(),
        want[5..10]
    );

    // A one-account set equals account_id; the whole set equals unified.
    let only_b: Vec<_> = list_in(&s, None, Some(&[B]), 100, None)
        .into_iter()
        .map(|t| t.thread_id)
        .collect();
    assert_eq!(only_b, list(&s, MailboxView::Inbox, None, Some(B)));
    let all: Vec<_> = list_in(&s, None, Some(&[A, B, C]), 100, None)
        .into_iter()
        .map(|t| t.thread_id)
        .collect();
    assert_eq!(all, list(&s, MailboxView::Inbox, None, None));
}

#[test]
fn list_threads_intersects_account_id_with_account_ids() {
    let s = three_account_store();
    let a_in_set: Vec<_> = list_in(&s, Some(A), Some(&[A, B]), 100, None)
        .into_iter()
        .map(|t| t.thread_id)
        .collect();
    assert_eq!(a_in_set, list(&s, MailboxView::Inbox, None, Some(A)));
    assert!(list_in(&s, Some(C), Some(&[A, B]), 100, None).is_empty());
    assert!(list_in(&s, None, Some(&[]), 100, None).is_empty());
    // Unknown accounts simply contribute nothing.
    assert_eq!(
        list_in(&s, None, Some(&[A, "gone@x.example"]), 100, None).len(),
        10
    );
}

#[test]
fn list_threads_account_set_uses_the_per_account_index() {
    let s = three_account_store();
    // Build the same statement list_threads runs and check SQLite's plan: an
    // index range per account, never a scan of thread_views.
    let plan: Vec<String> = s
        .read(|c| {
            let mut stmt = c.prepare(
                "EXPLAIN QUERY PLAN WITH top(thread_rowid, last_date) AS (
                   SELECT * FROM (SELECT thread_rowid, last_date FROM thread_views INDEXED BY thread_views_acct
                     WHERE account_id = ?4 AND view = ?1 AND last_date < ?2 ORDER BY last_date DESC LIMIT ?3)
                   UNION ALL
                   SELECT * FROM (SELECT thread_rowid, last_date FROM thread_views INDEXED BY thread_views_acct
                     WHERE account_id = ?5 AND view = ?1 AND last_date < ?2 ORDER BY last_date DESC LIMIT ?3))
                 SELECT t.thread_id, top.last_date FROM top JOIN threads t ON t.rowid = top.thread_rowid
                 ORDER BY top.last_date DESC LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(rusqlite::params!["inbox", i64::MAX, 10, A, C], |r| r.get::<_, String>(3))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    let text = plan.join("\n");
    assert!(text.contains("thread_views_acct"), "{text}");
    assert!(!text.contains("SCAN thread_views"), "{text}");
}

fn unread_in(
    s: &Store,
    account: Option<&str>,
    ids: Option<&[&str]>,
    limit: u32,
    before: Option<i64>,
) -> Vec<String> {
    s.list_threads(&ListQuery {
        view: MailboxView::Inbox,
        tab: None,
        account_id: account.map(Into::into),
        account_ids: ids.map(|v| v.iter().map(|a| a.to_string()).collect()),
        limit,
        before,
        unread_only: true,
        split: None,
    })
    .unwrap()
    .into_iter()
    .map(|t| t.thread_id)
    .collect()
}

#[test]
fn list_threads_unread_only_in_every_scope_shape() {
    let s = three_account_store();
    // Even-numbered threads are unread; thread i belongs to account i % 3.
    let want = |keep: &dyn Fn(usize) -> bool| -> Vec<String> {
        (0..30)
            .rev()
            .filter(|&i| i % 2 == 0 && keep(i))
            .map(|i| format!("t{i}"))
            .collect()
    };
    // Unified, one account, and an account set each run on their partial
    // index (INDEXED BY fails the query if the index can't serve it).
    assert_eq!(unread_in(&s, None, None, 100, None), want(&|_| true));
    assert_eq!(
        unread_in(&s, Some(B), None, 100, None),
        want(&|i| i % 3 == 1)
    );
    assert_eq!(
        unread_in(&s, None, Some(&[A, C]), 100, None),
        want(&|i| i % 3 != 1)
    );
    assert!(unread_in(&s, Some(C), Some(&[A, B]), 100, None).is_empty());

    // Paging with `before` continues the unread list.
    let all = want(&|_| true);
    let page1 = s
        .list_threads(&ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: None,
            account_ids: None,
            limit: 4,
            before: None,
            unread_only: true,
            split: None,
        })
        .unwrap();
    assert_eq!(
        page1
            .iter()
            .map(|t| t.thread_id.clone())
            .collect::<Vec<_>>(),
        all[..4]
    );
    assert!(page1.iter().all(|t| t.unread));
    assert_eq!(
        unread_in(&s, None, None, 4, Some(page1[3].last_date)),
        all[4..8]
    );

    // Reading a thread takes it out of the unread list; marking it unread brings it back.
    s.modify_thread_labels(A, "t0", &[], &["UNREAD".into()])
        .unwrap();
    assert!(!unread_in(&s, None, None, 100, None).contains(&"t0".to_string()));
    s.modify_thread_labels(A, "t0", &["UNREAD".into()], &[])
        .unwrap();
    assert!(unread_in(&s, None, None, 100, None).contains(&"t0".to_string()));
}

#[test]
fn labels_filter_by_account_set() {
    let s = three_account_store();
    for (acct, id) in [(A, "L_a"), (B, "L_b"), (C, "L_c")] {
        s.replace_labels(
            acct,
            &[
                Label {
                    account_id: acct.into(),
                    id: "INBOX".into(),
                    name: "INBOX".into(),
                    kind: "system".into(),
                    color: None,
                    unread_count: None,
                    hidden: false,
                },
                Label {
                    account_id: acct.into(),
                    id: id.into(),
                    name: id.into(),
                    kind: "user".into(),
                    color: None,
                    unread_count: None,
                    hidden: false,
                },
            ],
        )
        .unwrap();
    }
    let set: Vec<AccountId> = vec![A.into(), C.into()];
    let labels = s.list_labels_in(Some(&set)).unwrap();
    assert_eq!(labels.len(), 4);
    assert!(labels.iter().all(|l| l.account_id != B));
    let inbox_unread: u32 = labels
        .iter()
        .filter(|l| l.id == "INBOX")
        .filter_map(|l| l.unread_count)
        .sum();
    // Even-numbered threads are unread: i in 0..30, i % 3 != 1, i even → 10.
    assert_eq!(
        inbox_unread,
        (0..30).filter(|i| i % 3 != 1 && i % 2 == 0).count() as u32
    );
    assert!(s.list_labels_in(Some(&[])).unwrap().is_empty());
    assert_eq!(s.list_labels_in(None).unwrap().len(), 6);
    assert_eq!(s.list_labels(Some(B)).unwrap().len(), 2);
}

#[test]
fn search_respects_account_set_and_account_operator() {
    let s = three_account_store();
    let run = |q: &str, ids: Option<&[&str]>, one: Option<&str>| -> HashSet<String> {
        s.search(&SearchRequest {
            query: q.into(),
            account_id: one.map(Into::into),
            account_ids: ids.map(|v| v.iter().map(|a| a.to_string()).collect()),
            limit: 100,
        })
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.account_id)
        .collect()
    };
    assert_eq!(run("quarterly", None, None).len(), 3);
    assert_eq!(
        run("quarterly", Some(&[A, C]), None),
        HashSet::from([A.to_string(), C.to_string()])
    );
    // account: narrows within the set, and can't escape it.
    assert_eq!(
        run("quarterly account:me@side", Some(&[A, C]), None),
        HashSet::from([C.to_string()])
    );
    assert!(run("quarterly account:me@home", Some(&[A, C]), None).is_empty());
    // account_id intersects too.
    assert_eq!(
        run("quarterly", Some(&[A, C]), Some(A)),
        HashSet::from([A.to_string()])
    );
    assert!(run("quarterly", Some(&[A, C]), Some(B)).is_empty());
    // Filter-only queries (no FTS) are scoped as well.
    assert_eq!(
        run("is:unread", Some(&[B]), None),
        HashSet::from([B.to_string()])
    );
}

#[test]
fn more_than_1024_messages_in_one_millisecond_all_store() {
    // Bulk imports can stamp thousands of messages with the same internalDate.
    let s = store_with_accounts(&["a@x.example"]);
    let date = 1_758_600_000_000;
    let batch: Vec<Message> = (0..1_100)
        .map(|i| {
            M::new("a@x.example", &format!("m{i}"), &format!("t{i}"), date)
                .labels(&["INBOX"])
                .done()
        })
        .collect();
    for chunk in batch.chunks(300) {
        s.upsert_messages(chunk).unwrap();
    }
    assert_eq!(s.count_messages(Some("a@x.example")).unwrap(), 1_100);
    let m = s.get_message("a@x.example", "m1099").unwrap().unwrap();
    assert_eq!(
        m.date, date,
        "the real date is kept even when the rowid spills over"
    );
    // Re-upserting one of the spilled messages keeps it single.
    s.upsert_messages(&batch[1_050..1_051]).unwrap();
    assert_eq!(s.count_messages(Some("a@x.example")).unwrap(), 1_100);
}

#[test]
fn extreme_dates_do_not_overflow_rowids() {
    let s = store_with_accounts(&["a@x.example"]);
    let msgs: Vec<Message> = [i64::MAX, i64::MAX - 1, i64::MIN, -1, 0]
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            M::new("a@x.example", &format!("d{i}"), &format!("t{i}"), d)
                .labels(&["INBOX"])
                .attach("report.pdf", "application/pdf")
                .done()
        })
        .collect();
    s.upsert_messages(&msgs).unwrap();
    assert_eq!(s.count_messages(Some("a@x.example")).unwrap(), 5);
    assert_eq!(
        s.get_message("a@x.example", "d0").unwrap().unwrap().date,
        i64::MAX
    );
}

#[test]
fn account_nickname_and_color_survive_sign_in_again() {
    let s = store_with_accounts(&["a@x.example"]);
    let a = s
        .update_account("a@x.example", Some(Some("Alex Work")), Some("#4F7CFF"))
        .unwrap();
    assert_eq!(
        (a.nickname.as_deref(), a.color.as_str()),
        (Some("Alex Work"), "#4F7CFF")
    );
    // A fresh sign-in upserts the account again (with no nickname): keep it.
    s.upsert_account(&Account {
        color: "#4F7CFF".into(),
        ..account("a@x.example")
    })
    .unwrap();
    assert_eq!(
        s.list_accounts().unwrap()[0].nickname.as_deref(),
        Some("Alex Work")
    );
    // Color alone leaves the nickname; Some(None) clears it.
    let a = s
        .update_account("a@x.example", None, Some("#2FA37A"))
        .unwrap();
    assert_eq!(
        (a.nickname.as_deref(), a.color.as_str()),
        (Some("Alex Work"), "#2FA37A")
    );
    let a = s.update_account("a@x.example", Some(None), None).unwrap();
    assert_eq!(a.nickname, None);
    assert!(matches!(
        s.update_account("nobody@x.example", Some(Some("x")), None),
        Err(crate::Error::NotFound(_))
    ));
}

// ---------- outbox ----------

fn scheduled(id: &str, account: &str, draft: &str, send_at: i64) -> ScheduledSend {
    ScheduledSend {
        id: id.into(),
        account_id: account.into(),
        draft_id: draft.into(),
        send_at,
        created_at: 1,
        remind_after_ms: None,
        attempts: 0,
        last_error: None,
    }
}

fn reminder(id: &str, account: &str, thread: &str, sent_at: i64, remind_at: i64) -> Reminder {
    Reminder {
        id: id.into(),
        account_id: account.into(),
        thread_id: thread.into(),
        sent_message_id: Some(format!("{thread}-sent")),
        sent_at,
        remind_at,
        created_at: 1,
    }
}

#[test]
fn outbox_due_ordering_and_updates() {
    let s = store_with_accounts(&["a@x.example", "b@x.example"]);
    s.upsert_scheduled_send(&scheduled("s3", "a@x.example", "d3", 300))
        .unwrap();
    s.upsert_scheduled_send(&scheduled("s1", "b@x.example", "d1", 100))
        .unwrap();
    s.upsert_scheduled_send(&scheduled("s2", "a@x.example", "d2", 200))
        .unwrap();
    s.upsert_reminder(&reminder("r1", "a@x.example", "t1", 0, 150))
        .unwrap();

    let due: Vec<String> = s
        .due_scheduled_sends(200)
        .unwrap()
        .into_iter()
        .map(|x| x.id)
        .collect();
    assert_eq!(due, vec!["s1", "s2"]);
    assert_eq!(s.next_outbox_due().unwrap(), Some(100));
    assert_eq!(
        s.list_scheduled_sends(Some("a@x.example")).unwrap().len(),
        2
    );
    assert_eq!(
        s.scheduled_sends_for_draft("a@x.example", "d3").unwrap()[0].id,
        "s3"
    );
    assert_eq!(s.due_reminders(149).unwrap().len(), 0);
    assert_eq!(s.due_reminders(150).unwrap()[0].thread_id, "t1");

    // Retry bookkeeping round-trips.
    let mut retry = scheduled("s1", "b@x.example", "d1", 900);
    retry.attempts = 2;
    retry.last_error = Some("network".into());
    retry.remind_after_ms = Some(86_400_000);
    s.upsert_scheduled_send(&retry).unwrap();
    assert_eq!(
        s.get_scheduled_send("b@x.example", "s1").unwrap(),
        Some(retry)
    );

    assert!(s.delete_scheduled_send("a@x.example", "s2").unwrap());
    assert!(!s.delete_scheduled_send("a@x.example", "s2").unwrap());
    assert_eq!(
        s.list_reminders(None).unwrap()[0]
            .sent_message_id
            .as_deref(),
        Some("t1-sent")
    );
    assert!(s.delete_reminder_by_id("r1").unwrap());
    assert!(!s.delete_reminder_by_id("r1").unwrap());
    s.upsert_scheduled_send(&scheduled("s4", "a@x.example", "d3", 400))
        .unwrap();
    assert_eq!(
        s.delete_scheduled_sends_for_draft("a@x.example", "d3")
            .unwrap(),
        2
    );
    s.upsert_scheduled_send(&scheduled("s5", "a@x.example", "d5", 300))
        .unwrap();
    assert!(s.delete_scheduled_send_by_id("s5").unwrap());
    assert_eq!(s.next_outbox_due().unwrap(), Some(900));
    assert!(s.list_reminders(None).unwrap().is_empty());
}

#[test]
fn removing_an_account_drops_its_outbox() {
    let s = store_with_accounts(&["a@x.example", "b@x.example"]);
    s.upsert_scheduled_send(&scheduled("s1", "a@x.example", "d1", 100))
        .unwrap();
    s.upsert_scheduled_send(&scheduled("s2", "b@x.example", "d2", 100))
        .unwrap();
    s.upsert_reminder(&reminder("r1", "a@x.example", "t1", 0, 100))
        .unwrap();
    s.remove_account("a@x.example").unwrap();
    let left: Vec<String> = s
        .list_scheduled_sends(None)
        .unwrap()
        .into_iter()
        .map(|x| x.id)
        .collect();
    assert_eq!(left, vec!["s2"]);
    assert!(s.list_reminders(None).unwrap().is_empty());
}

#[test]
fn has_reply_after_ignores_own_drafts_and_spam() {
    let a = "me@x.example";
    let s = store_with_accounts(&[a]);
    let sent = 1_000_000;
    s.upsert_messages(&[
        // My message that started the wait.
        M::new(a, "m0", "t", sent)
            .labels(&["SENT"])
            .from("Me", a)
            .done(),
        // Earlier reply from someone else: doesn't count.
        M::new(a, "m1", "t", sent - 10).labels(&["INBOX"]).done(),
        // My follow-up from a send-as alias: SENT, different From.
        M::new(a, "m2", "t", sent + 10)
            .labels(&["SENT"])
            .from("Me", "alias@other.example")
            .done(),
        // A draft reply.
        M::new(a, "m3", "t", sent + 20)
            .labels(&["DRAFT"])
            .from("Me", a)
            .done(),
        // An auto-reply bot that went to spam.
        M::new(a, "m4", "t", sent + 30).labels(&["SPAM"]).done(),
    ])
    .unwrap();
    assert!(!s.has_reply_after(a, "t", sent).unwrap());
    s.upsert_messages(&[M::new(a, "m5", "t", sent + 40)
        .labels(&["INBOX", "UNREAD"])
        .done()])
        .unwrap();
    assert!(s.has_reply_after(a, "t", sent).unwrap());
    assert!(
        !s.has_reply_after(a, "t", sent + 40).unwrap(),
        "strictly after"
    );
    assert!(!s.has_reply_after(a, "other-thread", 0).unwrap());
}

/// Rule conditions (`match_query`) use the search language unchanged: for
/// any query, it matches exactly the messages search finds (one message
/// per thread here, so search's best-per-thread hits are all of them).
#[test]
fn rule_conditions_match_like_search() {
    let s = store_with_accounts(&["me@a.example", "me@b.example"]);
    let t = now() - DAY;
    s.upsert_messages(&[
        M::new("me@a.example", "m1", "t1", t)
            .from("Uber Receipts", "receipts@uber.example")
            .subject("Your Tuesday trip")
            .attach("receipt.pdf", "application/pdf")
            .done(),
        M::new("me@a.example", "m2", "t2", t + 1)
            .from("Uber", "noreply@uber.example")
            .subject("Promo: 50% off")
            .done(),
        M::new("me@b.example", "m3", "t3", t + 2)
            .from("Ana Ruiz", "ana@ruiz.example")
            .subject("Invoice for March")
            .body("invoice attached, thanks")
            .labels(&["INBOX", "UNREAD"])
            .done(),
        M::new("me@b.example", "m4", "t4", t + 3)
            .subject("old spam")
            .labels(&["SPAM"])
            .done(),
    ])
    .unwrap();
    for q in [
        "from:uber.example",
        "from:uber.example has:pdf",
        "from:uber.example -promo",
        "invoice",
        "is:unread",
        "subject:invoice OR subject:trip",
        "in:spam",
        "old spam",
        "account:me@b.example",
    ] {
        let from_search: HashSet<String> = hit_ids(&search(&s, q)).into_iter().collect();
        let from_rule: HashSet<String> = s
            .match_query(q, None, None, 1000)
            .unwrap()
            .into_iter()
            .map(|m| m.message_id)
            .collect();
        assert_eq!(from_rule, from_search, "query {q:?}");
        let (n, capped) = s.count_query_matches(q, None, 1000).unwrap();
        assert_eq!(n as usize, from_rule.len(), "count for {q:?}");
        assert!(!capped);
    }

    // Restricting to just-arrived ids (FTS-driven and filter-only plans).
    let only = vec![("me@a.example".to_string(), "m2".to_string())];
    let hits = s
        .match_query("from:uber.example", None, Some(&only), 10)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, "m2");
    assert_eq!(hits[0].subject, "Promo: 50% off");
    assert!(s
        .match_query("has:pdf", None, Some(&only), 10)
        .unwrap()
        .is_empty());
    assert!(s.match_query("x", None, Some(&[]), 10).unwrap().is_empty());

    // Account scope intersects with account: operators.
    let scope = vec!["me@b.example".to_string()];
    assert!(s
        .match_query("from:uber.example", Some(&scope), None, 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        s.count_query_matches("from:uber.example", None, 1).unwrap(),
        (1, true)
    );
    assert!(s.match_query("   ", None, None, 10).is_err());
    // A typo'd date would constrain nothing: rejected rather than matching all.
    assert!(s
        .count_query_matches(r#"from:uber.example date:"last wek""#, None, 10)
        .is_err());
    assert!(s.match_query("type:event standup", None, None, 10).is_err());
}

fn user_label(account: &str, id: &str, name: &str) -> Label {
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

#[test]
fn label_hidden_round_trips_and_upsert_patches_one_label() {
    let s = store_with_accounts(&[A]);
    let mut hidden = user_label(A, "Label_2", "Receipts");
    hidden.hidden = true;
    s.replace_labels(A, &[user_label(A, "Label_1", "Clients"), hidden])
        .unwrap();
    let got = s.list_labels(Some(A)).unwrap();
    let by_id = |id: &str| got.iter().find(|l| l.id == id).unwrap().clone();
    assert!(!by_id("Label_1").hidden);
    assert!(by_id("Label_2").hidden);

    let mut patched = user_label(A, "Label_1", "Clients/Active");
    patched.color = Some("#16a766".into());
    patched.hidden = true;
    s.upsert_label(&patched).unwrap();
    let got = s.list_labels(Some(A)).unwrap();
    assert_eq!(got.len(), 2);
    let l = got.iter().find(|l| l.id == "Label_1").unwrap();
    assert_eq!(l.name, "Clients/Active");
    assert_eq!(l.color.as_deref(), Some("#16a766"));
    assert!(l.hidden);
}

#[test]
fn delete_label_strips_it_from_messages_and_threads_of_that_account_only() {
    let s = store_with_accounts(&[A, B]);
    s.replace_labels(A, &[user_label(A, "Label_1", "Clients")])
        .unwrap();
    s.replace_labels(B, &[user_label(B, "Label_1", "Clients")])
        .unwrap();
    s.upsert_messages(&[
        M::new(A, "m1", "t1", now() - 3000)
            .labels(&["INBOX", "Label_1"])
            .done(),
        M::new(A, "m2", "t1", now() - 2000)
            .labels(&["INBOX"])
            .done(),
        M::new(A, "m3", "t2", now() - 1000)
            .labels(&["INBOX"])
            .done(),
        M::new(B, "m1", "t1", now())
            .labels(&["INBOX", "Label_1"])
            .done(),
    ])
    .unwrap();
    let label_view = || MailboxView::Label("Label_1".into());
    assert_eq!(list(&s, label_view(), None, Some(A)), vec!["t1"]);

    let touched = s.delete_label(A, "Label_1").unwrap();
    assert_eq!(touched, vec!["t1".to_string()]);
    assert!(s.list_labels(Some(A)).unwrap().is_empty());
    assert!(list(&s, label_view(), None, Some(A)).is_empty());
    let t1 = s.get_thread(A, "t1").unwrap().unwrap();
    assert!(!t1.label_ids.contains(&"Label_1".to_string()));
    assert!(t1
        .messages
        .iter()
        .all(|m| !m.label_ids.contains(&"Label_1".to_string())));
    // Still in the inbox; the other account keeps its own Label_1.
    assert_eq!(list(&s, MailboxView::Inbox, None, Some(A)).len(), 2);
    assert_eq!(list(&s, label_view(), None, Some(B)), vec!["t1"]);
    assert_eq!(s.list_labels(Some(B)).unwrap().len(), 1);
}

#[path = "search_ops_tests.rs"]
mod search_ops;

#[path = "search_plan_tests.rs"]
mod search_plan;
