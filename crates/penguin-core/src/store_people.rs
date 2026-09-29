//! One correspondent at a glance, for the person card: how much mail there
//! is with them, when it started and last happened, which of your accounts
//! it lives in, the latest threads and attachments, and other addresses
//! filed under the same name. Totals come from `people` (kept
//! incrementally); everything else is indexed (messages_from, the
//! recipients/cc FTS columns as a candidate filter confirmed against the
//! stored address lists, attachments_msg) and bounded, so a heavy
//! correspondent costs about the same as a rare one. Spam is left out.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, params_from_iter, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{has_word, word_range, Store, F_SPAM};
use crate::types::{Address, AttachmentHit, AttachmentMeta};
use crate::Result;

/// Most recent messages considered on each side (from them / to them) when
/// picking the latest threads and attachments.
const RECENT_WINDOW: usize = 60;
const RECENT_THREADS: usize = 5;
const RECENT_ATTACHMENTS: usize = 5;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonSummary {
    /// Lowercased.
    pub email: String,
    /// Best display name seen for the address.
    pub name: Option<String>,
    /// Other addresses stored under the same name.
    pub other_addresses: Vec<Address>,
    pub domain: String,
    /// Unix ms of the first / last message from or to them.
    pub first_contact: Option<i64>,
    pub last_contact: Option<i64>,
    /// Messages they sent you.
    pub messages_from: u32,
    /// Messages you sent them (to, cc or bcc).
    pub messages_to: u32,
    /// Your accounts that have mail with them, most messages first.
    pub accounts: Vec<PersonAccount>,
    pub recent_threads: Vec<PersonThread>,
    pub recent_attachments: Vec<AttachmentHit>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonAccount {
    pub account_id: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonThread {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
    pub date: i64,
}

/// FTS5 query for an address in the recipients (to+bcc) and cc columns:
/// a column filter plus a quoted phrase, quoted the way search.rs does it.
/// It's a candidate filter only (the tokenizer reads "sam@ortiz.example"
/// and "sam.ortiz@example…" alike), so every hit is confirmed against the
/// stored to/cc/bcc before it counts.
fn recipient_query(email: &str) -> Option<String> {
    if !email.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    let mut q = String::from("{recipients cc} : \"");
    for ch in email.chars() {
        match ch {
            '"' => q.push_str("\"\""),
            c if c.is_control() => q.push(' '),
            c => q.push(c),
        }
    }
    q.push('"');
    Some(q)
}

/// The address lists stored with each body (message_bodies.extra, JSON).
#[derive(Deserialize, Default)]
struct Recipients {
    #[serde(default)]
    to: Vec<Address>,
    #[serde(default)]
    cc: Vec<Address>,
    #[serde(default)]
    bcc: Vec<Address>,
}

/// Candidates checked per direction when confirming "to them" hits.
const CANDIDATES: i64 = 200;
/// Oldest candidates checked for the first-contact date.
const OLDEST_CANDIDATES: i64 = 20;

struct Hit {
    rowid: i64,
    thread_rowid: i64,
    account_id: String,
    date: i64,
}

impl Store {
    /// Recipients for the composer's To/Cc/Bcc as you type: everyone in the
    /// people index (every address in your mail, not only recent threads)
    /// with a name or address word starting with each typed word. People
    /// you've written to come first (most often, then most recently), then
    /// people who wrote to you; no-reply and notification addresses are
    /// left out. Local and indexed (people_words ranges), so it can run on
    /// every keystroke.
    pub fn suggest_recipients(&self, query: &str, limit: usize) -> Result<Vec<Address>> {
        let toks: Vec<String> = crate::text::tokens(query).into_iter().map(|(_, _, t)| t).collect();
        if toks.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut conds = Vec::new();
        let mut vals = Vec::new();
        for t in toks.iter().take(4) {
            conds.push(has_word(vals.len() + 1));
            vals.extend(word_range(t, true));
        }
        // Over-fetch so the automated addresses dropped below don't starve the list.
        vals.push(rusqlite::types::Value::Integer((limit * 4 + 8) as i64));
        let sql = format!(
            "SELECT email, name FROM people WHERE {} \
             ORDER BY (sent_to_count > 0) DESC, sent_to_count DESC, from_count DESC, last_date DESC LIMIT ?{}",
            conds.join(" AND "),
            vals.len()
        );
        self.read(|c| {
            let rows = c
                .prepare_cached(&sql)?
                .query_map(params_from_iter(vals.iter()), |r| {
                    Ok(Address { email: r.get(0)?, name: r.get(1)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows
                .into_iter()
                .filter(|a| !crate::store::is_automated_address(&a.email))
                .take(limit)
                .collect())
        })
    }

    pub fn person_summary(&self, email: &str) -> Result<PersonSummary> {
        let email = email.trim().to_lowercase();
        let domain = email.rsplit('@').next().unwrap_or("").to_string();
        let query = recipient_query(&email);
        self.read(|c| {
            // Totals come from `people`, which the store keeps incrementally.
            let person: Option<(Option<String>, u32, u32)> = c
                .prepare_cached("SELECT name, from_count, sent_to_count FROM people WHERE email = ?1")?
                .query_row(params![email], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            let (name, messages_from, messages_to) = match person {
                Some((n, f, t)) => (n.filter(|n| !n.trim().is_empty()), f, t),
                None => (None, 0, 0),
            };

            // From them: per-account counts and first/last dates (messages_from index).
            let mut per_account: HashMap<String, u32> = HashMap::new();
            let mut first: Option<i64> = None;
            let mut last: Option<i64> = None;
            fn widen(first: &mut Option<i64>, last: &mut Option<i64>, d: i64) {
                *first = Some(first.map_or(d, |f| f.min(d)));
                *last = Some(last.map_or(d, |l| l.max(d)));
            }
            {
                let mut stmt = c.prepare_cached(
                    "SELECT account_id, count(*), min(date), max(date) FROM messages
                     WHERE from_email = ?1 COLLATE NOCASE AND flags & ?2 = 0 GROUP BY account_id",
                )?;
                let rows = stmt.query_map(params![email, F_SPAM], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
                })?;
                for row in rows {
                    let (acct, n, lo, hi) = row?;
                    *per_account.entry(acct).or_default() += n;
                    widen(&mut first, &mut last, lo);
                    widen(&mut first, &mut last, hi);
                }
            }

            let mut recent: Vec<Hit> = Vec::new();
            {
                let mut stmt = c.prepare_cached(
                    "SELECT rowid, thread_rowid, account_id, date FROM messages
                     WHERE from_email = ?1 COLLATE NOCASE AND flags & ?2 = 0
                     ORDER BY date DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![email, F_SPAM, RECENT_WINDOW as i64], |r| {
                    Ok(Hit { rowid: r.get(0)?, thread_rowid: r.get(1)?, account_id: r.get(2)?, date: r.get(3)? })
                })?;
                for row in rows {
                    recent.push(row?);
                }
            }

            // To / cc them: FTS candidates (rowids are date-ordered, so the
            // LIMIT stops early), each confirmed against the stored lists.
            if let Some(q) = &query {
                let confirm = |c: &rusqlite::Connection, order: &str, limit: i64| -> Result<Vec<Hit>> {
                    let sql = format!(
                        "SELECT m.rowid, m.thread_rowid, m.account_id, m.date, b.extra
                         FROM messages_fts f
                         JOIN messages m ON m.rowid = f.rowid
                         JOIN message_bodies b ON b.rowid = m.rowid
                         WHERE messages_fts MATCH ?1 AND m.flags & ?2 = 0
                         ORDER BY f.rowid {order} LIMIT ?3"
                    );
                    let mut stmt = c.prepare_cached(&sql)?;
                    let rows = stmt.query_map(params![q, F_SPAM, limit], |r| {
                        Ok((Hit { rowid: r.get(0)?, thread_rowid: r.get(1)?, account_id: r.get(2)?, date: r.get(3)? }, r.get::<_, String>(4)?))
                    })?;
                    let mut out = Vec::new();
                    for row in rows {
                        let (hit, extra) = row?;
                        let r: Recipients = serde_json::from_str(&extra).unwrap_or_default();
                        if r.to.iter().chain(&r.cc).chain(&r.bcc).any(|a| a.email.eq_ignore_ascii_case(&email)) {
                            out.push(hit);
                        }
                    }
                    Ok(out)
                };
                let newest = confirm(c, "DESC", CANDIDATES)?;
                for h in &newest {
                    *per_account.entry(h.account_id.clone()).or_default() += 1;
                    widen(&mut first, &mut last, h.date);
                }
                if let Some(oldest) = confirm(c, "ASC", OLDEST_CANDIDATES)?.first() {
                    widen(&mut first, &mut last, oldest.date);
                }
                recent.extend(newest.into_iter().take(RECENT_WINDOW));
            }
            let mut accounts: Vec<PersonAccount> =
                per_account.into_iter().map(|(account_id, count)| PersonAccount { account_id, count }).collect();
            accounts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.account_id.cmp(&b.account_id)));

            recent.sort_by_key(|h| std::cmp::Reverse(h.rowid));
            recent.dedup_by_key(|h| h.rowid);

            let mut seen_threads = HashSet::new();
            let mut recent_threads = Vec::new();
            {
                let mut stmt = c.prepare_cached(
                    "SELECT account_id, thread_id, subject, last_date FROM threads WHERE rowid = ?1",
                )?;
                for h in &recent {
                    if recent_threads.len() == RECENT_THREADS {
                        break;
                    }
                    if !seen_threads.insert(h.thread_rowid) {
                        continue;
                    }
                    if let Some(t) = stmt
                        .query_row(params![h.thread_rowid], |r| {
                            Ok(PersonThread { account_id: r.get(0)?, thread_id: r.get(1)?, subject: r.get(2)?, date: r.get(3)? })
                        })
                        .optional()?
                    {
                        recent_threads.push(t);
                    }
                }
            }

            let mut recent_attachments = Vec::new();
            if !recent.is_empty() {
                let ids: Vec<i64> = recent.iter().map(|h| h.rowid).collect();
                let marks = vec!["?"; ids.len()].join(",");
                let sql = format!(
                    "SELECT m.account_id, t.thread_id, m.id, m.from_name, m.from_email, m.date,
                            a.att_id, a.filename, a.mime_type, a.size, a.content_id
                     FROM attachments a
                     JOIN messages m ON m.rowid = a.message_rowid
                     JOIN threads t ON t.rowid = m.thread_rowid
                     WHERE a.message_rowid IN ({marks}) AND a.inline = 0
                     ORDER BY m.rowid DESC, a.ord LIMIT {RECENT_ATTACHMENTS}"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt.query_map(params_from_iter(ids.iter()), |r| {
                    Ok(AttachmentHit {
                        account_id: r.get(0)?,
                        thread_id: r.get(1)?,
                        message_id: r.get(2)?,
                        from: Address { name: r.get(3)?, email: r.get(4)? },
                        date: r.get(5)?,
                        attachment: AttachmentMeta {
                            id: r.get(6)?,
                            filename: r.get(7)?,
                            mime_type: r.get(8)?,
                            size: r.get::<_, i64>(9)? as u64,
                            content_id: r.get(10)?,
                            inline: false,
                        },
                    })
                })?;
                for row in rows {
                    recent_attachments.push(row?);
                }
            }

            // Same name, other addresses (people is one row per correspondent).
            let mut other_addresses = Vec::new();
            if let Some(n) = &name {
                let mut stmt = c.prepare_cached(
                    "SELECT email, name FROM people WHERE name = ?1 COLLATE NOCASE AND email != ?2
                     ORDER BY last_date DESC LIMIT 5",
                )?;
                for row in stmt.query_map(params![n, email], |r| Ok(Address { email: r.get(0)?, name: r.get(1)? }))? {
                    other_addresses.push(row?);
                }
            }

            Ok(PersonSummary {
                email: email.clone(),
                name,
                other_addresses,
                domain: domain.clone(),
                first_contact: first,
                last_contact: last,
                messages_from,
                messages_to,
                accounts,
                recent_threads,
                recent_attachments,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{Address, AttachmentMeta, Message};
    use crate::Store;

    fn addr(name: &str, email: &str) -> Address {
        Address {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    fn msg(
        acct: &str,
        id: &str,
        thread: &str,
        from: Address,
        to: Vec<Address>,
        date: i64,
        sent: bool,
    ) -> Message {
        Message {
            account_id: acct.into(),
            id: id.into(),
            thread_id: thread.into(),
            date,
            from,
            to,
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: format!("subject {thread}"),
            snippet: String::new(),
            body_text: "hello".into(),
            body_html: None,
            label_ids: if sent {
                vec!["SENT".into()]
            } else {
                vec!["INBOX".into()]
            },
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    #[test]
    fn summarizes_mail_both_ways_across_accounts() {
        let store = Store::open_in_memory().unwrap();
        let priya = addr("Priya Natarajan", "priya@linden.example");
        let me_a = addr("Sam", "sam@a.example");
        let me_b = addr("Sam", "sam@b.example");
        let mut with_file = msg(
            "a",
            "m2",
            "t1",
            priya.clone(),
            vec![me_a.clone()],
            2_000,
            false,
        );
        with_file.attachments = vec![AttachmentMeta {
            id: "att1".into(),
            filename: "deck.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 10,
            content_id: None,
            inline: false,
        }];
        store
            .upsert_messages(&[
                msg(
                    "a",
                    "m1",
                    "t1",
                    me_a.clone(),
                    vec![priya.clone()],
                    1_000,
                    true,
                ),
                with_file,
                msg(
                    "b",
                    "m3",
                    "t2",
                    priya.clone(),
                    vec![me_b.clone()],
                    3_000,
                    false,
                ),
                msg(
                    "a",
                    "m4",
                    "t3",
                    addr("Other", "other@x.example"),
                    vec![me_a.clone()],
                    4_000,
                    false,
                ),
                msg(
                    "a",
                    "m5",
                    "t4",
                    addr("Priya Natarajan", "priya.n@home.example"),
                    vec![me_a.clone()],
                    500,
                    false,
                ),
            ])
            .unwrap();
        let s = store.person_summary("Priya@Linden.example").unwrap();
        assert_eq!(s.email, "priya@linden.example");
        assert_eq!(s.name.as_deref(), Some("Priya Natarajan"));
        assert_eq!(s.domain, "linden.example");
        assert_eq!((s.messages_from, s.messages_to), (2, 1));
        assert_eq!(
            (s.first_contact, s.last_contact),
            (Some(1_000), Some(3_000))
        );
        assert_eq!(
            s.accounts
                .iter()
                .map(|a| (a.account_id.as_str(), a.count))
                .collect::<Vec<_>>(),
            vec![("a", 2), ("b", 1)]
        );
        assert_eq!(
            s.recent_threads
                .iter()
                .map(|t| t.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["t2", "t1"]
        );
        assert_eq!(s.recent_attachments.len(), 1);
        assert_eq!(s.recent_attachments[0].attachment.filename, "deck.pdf");
        assert_eq!(
            s.other_addresses
                .iter()
                .map(|a| a.email.as_str())
                .collect::<Vec<_>>(),
            vec!["priya.n@home.example"]
        );
    }

    /// cargo test -p penguin-core --release person_summary_bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn person_summary_bench() {
        let store = Store::open_in_memory().unwrap();
        let me = addr("Sam", "sam@a.example");
        let n = 300_000;
        let batch: Vec<Message> = (0..n)
            .map(|i| {
                let other = addr("Person", &format!("p{}@corp{}.example", i % 5_000, i % 50));
                let sent = i % 4 == 0;
                let (from, to) = if sent {
                    (me.clone(), vec![other])
                } else {
                    (other, vec![me.clone()])
                };
                msg(
                    "a",
                    &format!("m{i}"),
                    &format!("t{}", i / 3),
                    from,
                    to,
                    1_000_000 + i as i64 * 1_000,
                    sent,
                )
            })
            .collect();
        for chunk in batch.chunks(5_000) {
            store.upsert_messages(chunk).unwrap();
        }
        let t = std::time::Instant::now();
        let runs = 20;
        for k in 0..runs {
            let s = store
                .person_summary(&format!("p{}@corp{}.example", k * 7, (k * 7) % 50))
                .unwrap();
            assert!(s.messages_from + s.messages_to > 0);
        }
        eprintln!(
            "person_summary: {:.2} ms avg over {n} messages",
            t.elapsed().as_secs_f64() * 1000.0 / runs as f64
        );
    }

    /// Against the bench mailbox (bench_search example):
    ///   PENGUIN_BENCH_DB=target/…/bench-db/bench-300000.db \
    ///   cargo test -p penguin-core --release person_summary_bench_db -- --ignored --nocapture
    #[test]
    #[ignore]
    fn person_summary_bench_db() {
        let Ok(path) = std::env::var("PENGUIN_BENCH_DB") else {
            return;
        };
        let store = Store::open(std::path::Path::new(&path)).unwrap();
        let picks: Vec<(String, u32)> = store
            .read(|c| {
                let mut out = Vec::new();
                for sql in [
                    "SELECT email, from_count + sent_to_count FROM people ORDER BY from_count DESC LIMIT 1",
                    "SELECT email, from_count + sent_to_count FROM people ORDER BY sent_to_count DESC LIMIT 1",
                    "SELECT email, from_count + sent_to_count FROM people WHERE from_count + sent_to_count BETWEEN 1 AND 3 LIMIT 1",
                ] {
                    out.push(c.query_row(sql, [], |r| Ok((r.get(0)?, r.get(1)?)))?);
                }
                Ok(out)
            })
            .unwrap();
        for (email, n) in picks {
            store.person_summary(&email).unwrap(); // warm
            let mut ms: Vec<f64> = (0..10)
                .map(|_| {
                    let t = std::time::Instant::now();
                    store.person_summary(&email).unwrap();
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            ms.sort_by(|a, b| a.total_cmp(b));
            eprintln!(
                "{email} ({n} msgs): p50 {:.2} ms, max {:.2} ms",
                ms[5], ms[9]
            );
        }
    }

    #[test]
    fn from_lookups_use_the_index() {
        let store = Store::open_in_memory().unwrap();
        let plan: Vec<String> = store
            .read(|c| {
                let mut stmt = c.prepare(
                    "EXPLAIN QUERY PLAN SELECT account_id, count(*), min(date), max(date) FROM messages
                     WHERE from_email = ?1 COLLATE NOCASE GROUP BY account_id",
                )?;
                let rows = stmt.query_map(["a@b.example"], |r| r.get::<_, String>(3))?;
                Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert!(plan.iter().any(|p| p.contains("messages_from")), "{plan:?}");
    }

    #[test]
    fn recipient_matches_are_confirmed_exactly() {
        let store = Store::open_in_memory().unwrap();
        let me = addr("Sam", "sam@a.example");
        // Tokenizes like "sam@ortiz.example" but is someone else.
        let lookalike = addr("Sam Ortiz", "sam.ortiz@example.com");
        store
            .upsert_messages(&[
                msg("a", "m1", "t1", me.clone(), vec![lookalike], 1_000, true),
                msg(
                    "a",
                    "m2",
                    "t2",
                    me.clone(),
                    vec![addr("S", "sam@ortiz.example")],
                    2_000,
                    true,
                ),
            ])
            .unwrap();
        let s = store.person_summary("sam@ortiz.example").unwrap();
        assert_eq!(
            s.recent_threads
                .iter()
                .map(|t| t.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["t2"]
        );
        assert_eq!(s.first_contact, Some(2_000));
    }

    #[test]
    fn unknown_address_is_empty_not_an_error() {
        let store = Store::open_in_memory().unwrap();
        let s = store.person_summary("nobody@x.example").unwrap();
        assert_eq!(
            (s.messages_from, s.messages_to, s.first_contact),
            (0, 0, None)
        );
        assert!(s.accounts.is_empty() && s.recent_threads.is_empty());
    }

    #[test]
    fn suggest_recipients_prefers_people_you_wrote_to_and_skips_robots() {
        let s = Store::open_in_memory().unwrap();
        let me = addr("Sam Okafor", "sam@northwind.example");
        let priya_r = addr("Priya Raman", "priya.raman@linden.example");
        let priya_n = addr("Priya Natarajan", "priya@harbor.example");
        let robot = addr("Priya bot", "no-reply@priya-alerts.example");
        s.upsert_messages(&[
            // You wrote to Priya Raman once, long ago: no recent inbox thread.
            msg("a", "m1", "t1", me.clone(), vec![priya_r.clone()], 1_600_000_000_000, true),
            // Priya Natarajan wrote to you twice, recently.
            msg("a", "m2", "t2", priya_n.clone(), vec![me.clone()], 1_790_000_000_000, false),
            msg("a", "m3", "t3", priya_n.clone(), vec![me.clone()], 1_790_000_100_000, false),
            msg("a", "m4", "t4", robot.clone(), vec![me.clone()], 1_790_000_200_000, false),
        ])
        .unwrap();
        let got = |q: &str| s.suggest_recipients(q, 6).unwrap().into_iter().map(|a| a.email).collect::<Vec<_>>();
        assert_eq!(got("pri"), ["priya.raman@linden.example", "priya@harbor.example"]);
        assert_eq!(got("priya nat"), ["priya@harbor.example"]);
        assert_eq!(got("linden"), ["priya.raman@linden.example"]);
        assert!(got("zzz").is_empty());
        assert!(got("  ").is_empty());
    }
}
