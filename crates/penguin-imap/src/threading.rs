//! Conversation threading for IMAP servers without thread ids ("JWZ-lite").
//!
//! A message joins the thread of any Message-ID it names (References,
//! In-Reply-To) or that names it. When it links two known threads, they
//! merge: every message moves to the root-most thread
//! (`Store::set_message_thread`). A new conversation's id is
//! `ids::imap_thread_id(root Message-ID)`, so a reply that arrives before
//! its parent still lands in the thread the parent will join. Replies that
//! lost their headers ("Re: …" without References) join the thread with the
//! same base subject only within [`SUBJECT_WINDOW_MS`]; nothing else merges
//! on subject. Gmail (X-GM-THRID) never comes here.

use std::collections::HashSet;

use penguin_core::{Result, Store};
use penguin_provider::ids::{imap_thread_id, normalize_message_id};
use sha2::{Digest, Sha256};

use crate::db;

/// Subject-only merging looks this far back (and forward).
pub const SUBJECT_WINDOW_MS: i64 = 7 * 24 * 3600 * 1000;

/// What threading needs from one message.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Penguin message id (fallback thread root when there are no ids).
    pub message_id: String,
    /// Message-ID header (bare).
    pub own: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub subject: String,
    pub date_ms: i64,
}

const REPLY_PREFIXES: &[&str] = &["re", "aw", "sv", "vs", "antw", "odp", "ref", "rif"];
const FORWARD_PREFIXES: &[&str] = &["fw", "fwd", "wg", "tr", "rv", "enc", "doorst"];

/// Strip one `Re:` / `Fwd:` / `Re[2]:` prefix; returns (rest, was reply).
fn strip_prefix(s: &str) -> Option<(&str, bool)> {
    let t = s.trim_start();
    let colon = t.find(':')?;
    let head = t[..colon].trim();
    let word = head
        .split('[')
        .next()
        .unwrap_or(head)
        .trim()
        .to_ascii_lowercase();
    if head.contains('[') && !head.ends_with(']') {
        return None;
    }
    if REPLY_PREFIXES.contains(&word.as_str()) {
        Some((&t[colon + 1..], true))
    } else if FORWARD_PREFIXES.contains(&word.as_str()) {
        Some((&t[colon + 1..], false))
    } else {
        None
    }
}

/// (base subject lowercased with whitespace collapsed, has a reply prefix).
pub fn base_subject(subject: &str) -> (String, bool) {
    let mut rest = subject;
    let mut reply = false;
    while let Some((r, is_reply)) = strip_prefix(rest) {
        reply |= is_reply;
        rest = r;
    }
    let base = rest
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (base, reply)
}

fn subject_key(base: &str) -> String {
    let digest = Sha256::digest(base.as_bytes());
    digest[..12].iter().map(|b| format!("{b:02x}")).collect()
}

/// Pick the message's thread, record its Message-IDs, and merge threads it
/// links. Returns (thread id, threads whose contents changed by a merge).
pub fn assign(store: &Store, account: &str, input: &Input) -> Result<(String, Vec<String>)> {
    // Root-most first: References in order, then In-Reply-To, then its own.
    let mut chain: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for id in input
        .references
        .iter()
        .chain(input.in_reply_to.iter())
        .chain(input.own.iter())
    {
        let n = normalize_message_id(id);
        if !n.is_empty() && seen.insert(n.clone()) {
            chain.push(n);
        }
    }
    let known = db::threads_of(store, account, &chain)?;
    let mut threads: Vec<String> = Vec::new();
    for id in &chain {
        if let Some(t) = known.get(id) {
            if !threads.contains(t) {
                threads.push(t.clone());
            }
        }
    }
    let (base, reply) = base_subject(&input.subject);
    let has_refs = !input.references.is_empty() || input.in_reply_to.is_some();
    let key = (base.chars().count() >= 3).then(|| subject_key(&base));
    let chosen = match threads.first() {
        Some(t) => t.clone(),
        None => {
            let by_subject = match (&key, reply && !has_refs) {
                (Some(k), true) => {
                    db::subject_thread(store, account, k, input.date_ms - SUBJECT_WINDOW_MS)?
                }
                _ => None,
            };
            match by_subject {
                Some(t) => t,
                None => match chain.first() {
                    Some(root) => imap_thread_id(root),
                    None => imap_thread_id(&input.message_id),
                },
            }
        }
    };
    let merged: Vec<String> = threads.into_iter().filter(|t| *t != chosen).collect();
    let mut changed = Vec::new();
    for t in &merged {
        let ids: Vec<String> = store
            .get_thread(account, t)?
            .map(|d| d.messages.into_iter().map(|m| m.id).collect())
            .unwrap_or_default();
        changed.extend(store.set_message_thread(account, &ids, &chosen)?);
    }
    db::set_threads(store, account, &chain, &chosen, &merged)?;
    if let Some(k) = &key {
        db::put_subject(store, account, k, &chosen, input.date_ms)?;
    }
    changed.sort();
    changed.dedup();
    Ok((chosen, changed))
}

#[cfg(test)]
mod tests {
    use penguin_core::{Account, Address, Message};

    use super::*;

    const A: &str = "sam@mail.example";

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&Account {
            id: A.into(),
            email: A.into(),
            ..Default::default()
        })
        .unwrap();
        db::migrate(&s).unwrap();
        s
    }

    fn input(
        id: &str,
        own: &str,
        irt: Option<&str>,
        refs: &[&str],
        subject: &str,
        date: i64,
    ) -> Input {
        Input {
            message_id: id.into(),
            own: Some(own.into()),
            in_reply_to: irt.map(str::to_string),
            references: refs.iter().map(|s| s.to_string()).collect(),
            subject: subject.into(),
            date_ms: date,
        }
    }

    fn stored(s: &Store, id: &str, thread: &str) {
        s.upsert_messages(&[Message {
            account_id: A.into(),
            id: id.into(),
            thread_id: thread.into(),
            date: 1_760_000_000_000,
            from: Address {
                name: None,
                email: "bea@mail.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Plans".into(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }])
        .unwrap();
    }

    #[test]
    fn subjects() {
        assert_eq!(
            base_subject("Re: Fwd: RE[2]:  Lunch   plans"),
            ("lunch plans".into(), true)
        );
        assert_eq!(base_subject("Fwd: Lunch"), ("lunch".into(), false));
        assert_eq!(
            base_subject("Lunch: Tuesday"),
            ("lunch: tuesday".into(), false)
        );
        assert_eq!(base_subject("AW: Termin"), ("termin".into(), true));
    }

    #[test]
    fn replies_join_their_parent_in_any_order() {
        let s = store();
        // The reply arrives first: its thread is rooted at the parent's id.
        let (t_reply, _) = assign(
            &s,
            A,
            &input(
                "h:2",
                "b@x.example",
                Some("a@x.example"),
                &["a@x.example"],
                "Re: Plans",
                2,
            ),
        )
        .unwrap();
        let (t_parent, _) =
            assign(&s, A, &input("h:1", "a@x.example", None, &[], "Plans", 1)).unwrap();
        assert_eq!(t_reply, t_parent);
        assert_eq!(t_parent, imap_thread_id("<a@x.example>"));
        // Unrelated mail with the same subject but no reply prefix stays apart.
        let (t_other, _) =
            assign(&s, A, &input("h:3", "c@x.example", None, &[], "Plans", 3)).unwrap();
        assert_ne!(t_other, t_parent);
    }

    #[test]
    fn a_message_linking_two_threads_merges_them() {
        let s = store();
        let (t1, _) = assign(&s, A, &input("h:1", "a@x.example", None, &[], "One", 1)).unwrap();
        stored(&s, "h:1", &t1);
        // A reply whose References lost the root, seen before the middle one.
        let (t2, _) = assign(
            &s,
            A,
            &input(
                "h:3",
                "c@x.example",
                Some("b@x.example"),
                &["b@x.example"],
                "Re: One",
                3,
            ),
        )
        .unwrap();
        stored(&s, "h:3", &t2);
        assert_ne!(t1, t2);
        // The middle message names both: everything becomes one thread.
        let (t, changed) = assign(
            &s,
            A,
            &input(
                "h:2",
                "b@x.example",
                Some("a@x.example"),
                &["a@x.example"],
                "Re: One",
                2,
            ),
        )
        .unwrap();
        assert_eq!(t, t1);
        assert!(changed.contains(&t2), "{changed:?}");
        assert_eq!(s.get_message(A, "h:3").unwrap().unwrap().thread_id, t1);
        assert!(s.get_thread(A, &t2).unwrap().is_none());
        // Later mail naming the merged thread's ids lands in the survivor.
        let (t4, _) = assign(
            &s,
            A,
            &input("h:4", "d@x.example", Some("c@x.example"), &[], "Re: One", 4),
        )
        .unwrap();
        assert_eq!(t4, t1);
    }

    #[test]
    fn headerless_replies_merge_on_subject_only_within_the_window() {
        let s = store();
        let day = 24 * 3600 * 1000;
        let (t1, _) = assign(
            &s,
            A,
            &input("h:1", "a@x.example", None, &[], "Budget review", 10 * day),
        )
        .unwrap();
        let (t2, _) = assign(
            &s,
            A,
            &input(
                "h:2",
                "b@x.example",
                None,
                &[],
                "RE: Budget  review",
                12 * day,
            ),
        )
        .unwrap();
        assert_eq!(t1, t2);
        let (t3, _) = assign(
            &s,
            A,
            &input(
                "h:3",
                "c@x.example",
                None,
                &[],
                "Re: Budget review",
                40 * day,
            ),
        )
        .unwrap();
        assert_ne!(t3, t1);
    }

    #[test]
    fn mail_without_any_message_id_gets_its_own_thread() {
        let s = store();
        let mut i = input("h:9", "", None, &[], "Hello", 1);
        i.own = None;
        let (t, _) = assign(&s, A, &i).unwrap();
        assert_eq!(t, imap_thread_id("h:9"));
    }
}
