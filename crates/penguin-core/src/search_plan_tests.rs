//! Search plans: the faster plans (set filters tested on the FTS row,
//! the is:unread prefilter, filter-first scans) return exactly what the
//! plain plans return.

use super::*;
use crate::search::PLAIN_PLANS;

const WORDS: &[&str] = &[
    "invoice",
    "meeting",
    "budget",
    "lease",
    "report",
    "the",
    "quarterly",
    "renewal",
    "pickup",
    "dinner",
    "draft",
    "review",
    "https",
    "tax",
];

/// A few hundred messages over two accounts with a spread of labels,
/// unread, sent, attachments and dates, from a fixed seed.
fn corpus() -> Store {
    let s = store_with_accounts(&[A, B]);
    for acct in [A, B] {
        let labels: Vec<Label> = (1..=3)
            .map(|i| Label {
                account_id: acct.into(),
                id: format!("Label_{i}"),
                name: format!("Project {i}"),
                kind: "user".into(),
                color: None,
                unread_count: None,
                hidden: false,
            })
            .collect();
        s.replace_labels(acct, &labels).unwrap();
    }
    let mut seed = 99u64;
    let mut rnd = move |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let t0 = now() - 900 * DAY;
    let mut batch = Vec::new();
    for i in 0..600u64 {
        let acct = if rnd(3) == 0 { B } else { A };
        let thread = format!("t{}", i - rnd(3).min(i));
        let date = t0 + (i as i64) * DAY + rnd(80_000_000) as i64;
        let words: Vec<&str> = (0..6)
            .map(|_| WORDS[rnd(WORDS.len() as u64) as usize])
            .collect();
        let mut labels = vec!["INBOX"];
        if rnd(12) == 0 {
            labels.push("UNREAD");
        }
        if rnd(20) == 0 {
            labels.push("Label_1");
        }
        if rnd(7) == 0 {
            labels.push("Label_2");
        }
        if rnd(40) == 0 {
            labels.push("Label_3");
        }
        let sent = rnd(3) == 0;
        let mut m = M::new(acct, &format!("m{i}"), &thread, date)
            .subject(&format!("{} {}", words[0], words[1]))
            .body(&words.join(" "));
        m = if sent {
            m.from("Me", acct)
                .to(&[("", &format!("p{}@corp{}.example", rnd(15), rnd(4)))])
                .labels(&["SENT"])
        } else {
            m.from("", &format!("p{}@corp{}.example", rnd(15), rnd(4)))
                .labels(&labels)
        };
        if rnd(6) == 0 {
            m = m.attach(&format!("report-{}.pdf", rnd(50)), "application/pdf");
        }
        batch.push(m.done());
        if batch.len() == 50 {
            s.upsert_messages(&batch).unwrap();
            batch.clear();
        }
    }
    s.upsert_messages(&batch).unwrap();
    s
}

type Outcome = (Vec<(String, String, u32)>, Vec<String>);

fn outcome(s: &Store, q: &str) -> Outcome {
    let r = search(s, q);
    (
        r.hits
            .iter()
            .map(|h| (h.thread_id.clone(), h.message_id.clone(), h.match_count))
            .collect(),
        r.attachments.iter().map(|a| a.message_id.clone()).collect(),
    )
}

fn plain<T>(f: impl FnOnce() -> T) -> T {
    PLAIN_PLANS.set(true);
    let out = f();
    PLAIN_PLANS.set(false);
    out
}

const QUERIES: &[&str] = &[
    "invoice",
    "the",
    "label:Label_1",
    "label:Label_1 meeting",
    "label:Label_2 the",
    "label:Label_3 budget OR lease",
    "-label:Label_2 invoice",
    "(label:Label_1 OR label:Label_3) report",
    "is:unread",
    "is:unread invoice",
    "is:unread label:Label_2 the",
    "-is:unread budget",
    "is:unread OR label:Label_1 tax",
    "is:new-sender invoice",
    "is:new-sender",
    "to:new budget",
    "filename:report meeting",
    "filename:rep",
    "has:pdf review",
    "has:link invoice",
    "-meeting has:pdf",
    "in:sent budget",
    "is:replied invoice",
    "account:home invoice",
    "account:home is:unread the",
    "invoice older_than:1y",
    "\"quarterly report\" label:Label_2",
];

#[test]
fn faster_plans_return_the_same_results() {
    let s = corpus();
    let mut engaged = 0;
    for q in QUERIES {
        let fast = outcome(&s, q);
        let slow = plain(|| outcome(&s, q));
        assert_eq!(fast, slow, "{q}");
        engaged += usize::from(!fast.0.is_empty());
        // Rules use the same plans.
        let fast = s.match_query(q, None, None, 1000).unwrap();
        let slow = plain(|| s.match_query(q, None, None, 1000).unwrap());
        let key =
            |v: Vec<QueryMatch>| -> Vec<String> { v.into_iter().map(|m| m.message_id).collect() };
        assert_eq!(key(fast), key(slow), "rule {q}");
    }
    // The corpus must exercise the filters for this to mean anything.
    assert!(
        engaged >= QUERIES.len() - 2,
        "only {engaged} queries matched"
    );
}

/// A phrase with more matches than search scores is ranked unscored (see
/// `rank_fts`); with filters on it, the faster plans must still return
/// what the plain plans return, on both sides of that threshold.
#[test]
fn common_phrases_with_filters_match_plain_plans() {
    let s = store_with_accounts(&[A, B]);
    for acct in [A, B] {
        s.replace_labels(
            acct,
            &[Label {
                account_id: acct.into(),
                id: "Label_1".into(),
                name: "Project 1".into(),
                kind: "user".into(),
                color: None,
                unread_count: None,
                hidden: false,
            }],
        )
        .unwrap();
    }
    let t0 = now() - 2000 * DAY;
    let n = crate::search::MAX_CANDIDATES as i64 + 400;
    let mut batch = Vec::new();
    for i in 0..n {
        let acct = if i % 3 == 0 { B } else { A };
        let mut labels = vec!["INBOX"];
        if i % 11 == 0 {
            labels.push("UNREAD");
        }
        if i % 5 == 0 {
            labels.push("Label_1");
        }
        if i % 97 == 0 {
            labels.push("TRASH");
        }
        let body = if i % 2 == 0 {
            "went to the park"
        } else {
            "back to the end to a"
        };
        batch.push(
            M::new(
                acct,
                &format!("m{i}"),
                &format!("t{}", i / 2),
                t0 + i * 3_600_000,
            )
            .body(&format!("{body} n{i}"))
            .labels(&labels)
            .done(),
        );
        if batch.len() == 500 {
            s.upsert_messages(&batch).unwrap();
            batch.clear();
        }
    }
    s.upsert_messages(&batch).unwrap();
    for q in [
        "\"to the\"",
        "\"to the\" is:unread",
        "\"to the\" label:Label_1",
        "\"to the\" account:home",
        "\"to the\" older_than:1y",
        "\"to the\" -n10",
        "\"to a\"",
        "\"to a\" label:Label_1",
        "\"went to\" in:anywhere",
    ] {
        let fast = outcome(&s, q);
        let slow = plain(|| outcome(&s, q));
        assert!(!fast.0.is_empty(), "{q}");
        assert_eq!(fast, slow, "{q}");
    }
}
