//! A deterministic, realistic synthetic mailbox for judging search.
//!
//! Three accounts (work, personal, a freelance studio) with roughly 20,000
//! messages in 6,000–7,000 conversations: work threads with replies and
//! quoted history, newsletters and promotions, receipts (some carrying
//! schema.org JSON-LD), flights and hotels, shipping updates, bills and
//! invoices, calendar invitations, verification codes, social notifications,
//! personal mail, and mail in Spanish. Everyone in it is fictional and every
//! domain ends in `.example`.
//!
//! Every conversation carries [`ThreadFacts`]: machine-readable tags (its
//! kind, the people, merchants, cities, order numbers and topics in it). The
//! query set judges relevance with rules over those tags, so judgments are
//! complete for the concepts a query asks about, not just for one planted
//! message (see docs/SEARCH-EVAL.md, "Judgments").
//!
//! Background mail comes from procedural families (`families.rs`); the
//! hand-written conversations that paraphrase, natural-language and Ask
//! queries point at live in `planted.rs`.

pub mod edge;
mod families;
mod planted;
pub mod world;

use std::collections::BTreeSet;

use chrono::{Local, TimeZone};
use penguin_core::{Account, Address, AttachmentMeta, Label, Message, Store};
use serde::Serialize;

use crate::rng::Rng;
use crate::window::{day_anchor, DAY};

pub use world::{ACCOUNTS, ME_NAME};

/// What the judgments know about one conversation.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadFacts {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
    /// `key:value` tags, e.g. `kind:receipt`, `merchant:dishdash`,
    /// `person:priya`, `city:lisbon`, `plant:water-heater`.
    pub tags: BTreeSet<String>,
    /// First and last message dates (unix ms).
    pub first: i64,
    pub last: i64,
}

impl ThreadFacts {
    pub fn has(&self, tag: &str) -> bool {
        self.tags.contains(tag)
    }
    /// Stable document id used in qrels and runs.
    pub fn doc_id(&self) -> String {
        doc_id(&self.account_id, &self.thread_id)
    }
}

pub fn doc_id(account_id: &str, thread_id: &str) -> String {
    format!("{account_id}/{thread_id}")
}

/// The generated mailbox.
pub struct Corpus {
    pub now: i64,
    pub seed: u64,
    pub messages: Vec<Message>,
    pub threads: Vec<ThreadFacts>,
    user_labels: Vec<(usize, String)>,
}

/// An attachment to add: filename, MIME type, size in bytes.
#[derive(Clone)]
pub struct Att(pub String, pub &'static str, pub u64);

impl Att {
    pub fn pdf(name: impl Into<String>, size: u64) -> Att {
        Att(name.into(), "application/pdf", size)
    }
    pub fn jpg(name: impl Into<String>, size: u64) -> Att {
        Att(name.into(), "image/jpeg", size)
    }
    pub fn xlsx(name: impl Into<String>, size: u64) -> Att {
        Att(
            name.into(),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            size,
        )
    }
    pub fn docx(name: impl Into<String>, size: u64) -> Att {
        Att(
            name.into(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            size,
        )
    }
    pub fn ics() -> Att {
        Att("invite.ics".into(), "text/calendar", 2_400)
    }
}

/// One message of a conversation being built.
#[derive(Clone)]
pub struct Msg {
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: i64,
    pub body: String,
    pub html: Option<String>,
    pub atts: Vec<Att>,
    pub labels: Vec<String>,
    pub list_unsubscribe: bool,
    /// Subject for this message only (a reply's "Re: …" is automatic).
    pub subject: Option<String>,
    /// Quote the previous message below this one's body (a reply).
    pub quote: bool,
}

impl Msg {
    pub fn new(from: Address, to: Vec<Address>, date: i64, body: impl Into<String>) -> Msg {
        Msg {
            from,
            to,
            cc: Vec::new(),
            date,
            body: body.into(),
            html: None,
            atts: Vec::new(),
            labels: Vec::new(),
            list_unsubscribe: false,
            subject: None,
            quote: false,
        }
    }
    pub fn reply(mut self) -> Msg {
        self.quote = true;
        self
    }
    pub fn att(mut self, a: Att) -> Msg {
        self.atts.push(a);
        self
    }
    pub fn html(mut self, h: String) -> Msg {
        self.html = Some(h);
        self
    }
    pub fn label(mut self, l: &str) -> Msg {
        self.labels.push(l.to_string());
        self
    }
    pub fn bulk(mut self) -> Msg {
        self.list_unsubscribe = true;
        self
    }
    pub fn cc(mut self, a: Address) -> Msg {
        self.cc.push(a);
        self
    }
    pub fn subject(mut self, s: impl Into<String>) -> Msg {
        self.subject = Some(s.into());
        self
    }
}

/// A conversation being built.
pub struct Thread {
    pub account: usize,
    pub id: String,
    pub subject: String,
    pub tags: Vec<String>,
    pub msgs: Vec<Msg>,
}

impl Thread {
    pub fn new(account: usize, id: impl Into<String>, subject: impl Into<String>) -> Thread {
        Thread {
            account,
            id: id.into(),
            subject: subject.into(),
            tags: Vec::new(),
            msgs: Vec::new(),
        }
    }
    pub fn tag(mut self, t: impl Into<String>) -> Thread {
        self.tags.push(t.into());
        self
    }
    pub fn tags(mut self, ts: &[&str]) -> Thread {
        self.tags.extend(ts.iter().map(|t| t.to_string()));
        self
    }
    pub fn msg(mut self, m: Msg) -> Thread {
        self.msgs.push(m);
        self
    }
    /// Attach a file to the last message.
    /// Add a label to the first message.
    pub fn label_first(mut self, l: &str) -> Thread {
        if let Some(m) = self.msgs.first_mut() {
            m.labels.push(l.to_string());
        }
        self
    }
    pub fn att_last(mut self, a: Att) -> Thread {
        if let Some(m) = self.msgs.last_mut() {
            m.atts.push(a);
        }
        self
    }
}

/// FNV-1a, for stable content hashes.
pub fn fnv(mut h: u64, s: &str) -> u64 {
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h ^ 0xff
}

/// `Name <email>` → Address.
pub fn addr(name: &str, email: &str) -> Address {
    Address {
        name: if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        },
        email: email.to_string(),
    }
}

/// The mailbox owner's address in account `i`.
pub fn me(i: usize) -> Address {
    addr(ME_NAME, ACCOUNTS[i].0)
}

/// "On Tue, Mar 3, 2026 at 10:14 AM Priya Shah <priya.shah@…> wrote:"
pub fn attribution(from: &Address, date: i64) -> String {
    let t = Local.timestamp_millis_opt(date).single().unwrap();
    let who = match &from.name {
        Some(n) => format!("{n} <{}>", from.email),
        None => format!("<{}>", from.email),
    };
    format!(
        "On {} {who} wrote:",
        t.format("%a, %b %-d, %Y at %-I:%M %p")
    )
}

/// Format a unix-ms time in local time.
pub fn fmt_local(ms: i64, fmt: &str) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap()
        .format(fmt)
        .to_string()
}

fn quote_block(body: &str) -> String {
    body.lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Plain text → a simple HTML version (for mail that has both parts).
pub fn to_html(text: &str) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let paras: Vec<String> = text
        .split("\n\n")
        .map(|p| format!("<p>{}</p>", esc(p).replace('\n', "<br>")))
        .collect();
    format!(
        "<html><body><div style=\"font-family:Arial,sans-serif\">{}</div></body></html>",
        paras.join("")
    )
}

impl Corpus {
    /// Generate the whole mailbox for `seed`, with dates relative to the
    /// local day containing `now` (runs on the same day are identical).
    pub fn generate(seed: u64, now: i64) -> Corpus {
        let mut c = Corpus {
            now: day_anchor(now),
            seed,
            messages: Vec::new(),
            threads: Vec::new(),
            user_labels: Vec::new(),
        };
        planted::add(&mut c);
        families::add(&mut c);
        edge::add(&mut c);
        c.messages
            .sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
        c
    }

    /// A time `days` days before now, at `hour`:`min` local.
    pub fn days_ago(&self, days: i64, hour: i64, min: i64) -> i64 {
        self.now - 12 * 3_600_000 - days * DAY + hour * 3_600_000 + min * 60_000
    }

    /// Random time between `lo` and `hi` days ago, in waking hours.
    pub fn rand_time(&self, rng: &mut Rng, lo: i64, hi: i64) -> i64 {
        let d = rng.range(lo, hi);
        self.days_ago(d, rng.range(7, 21), rng.range(0, 59))
    }

    pub fn user_label(&mut self, account: usize, name: &str) -> String {
        let id = format!("Label_{}", name.replace(['/', ' '], "_"));
        if !self
            .user_labels
            .iter()
            .any(|(a, n)| *a == account && n == name)
        {
            self.user_labels.push((account, name.to_string()));
        }
        id
    }

    /// Apply the user label `name` to every message of every conversation
    /// in `account` that `pick` selects (as a person filing mail would), and
    /// record it in the facts as `ulabel:<id>`. Returns how many
    /// conversations got it.
    pub fn label_threads(
        &mut self,
        account: usize,
        name: &str,
        pick: impl Fn(&ThreadFacts) -> bool,
    ) -> usize {
        let id = self.user_label(account, name);
        let acct = ACCOUNTS[account].0;
        let mut chosen = BTreeSet::new();
        for t in self.threads.iter_mut() {
            if t.account_id == acct && pick(t) {
                t.tags.insert(format!("ulabel:{id}"));
                chosen.insert(t.thread_id.clone());
            }
        }
        for m in self.messages.iter_mut() {
            if m.account_id == acct && chosen.contains(&m.thread_id) && !m.label_ids.contains(&id) {
                m.label_ids.push(id.clone());
                m.label_ids.sort();
            }
        }
        chosen.len()
    }

    /// Turn a built conversation into stored messages and facts.
    pub fn push(&mut self, t: Thread) {
        assert!(!t.msgs.is_empty(), "thread {} has no messages", t.id);
        let (acct_email, _) = ACCOUNTS[t.account];
        let me_lc = acct_email.to_lowercase();
        let mut tags: BTreeSet<String> = t.tags.iter().cloned().collect();
        tags.insert(format!("account:{}", ACCOUNTS[t.account].1));
        let mut prev: Option<(Address, i64, String)> = None;
        let mut refs: Vec<String> = Vec::new();
        let (mut first, mut last) = (i64::MAX, i64::MIN);
        for (i, m) in t.msgs.iter().enumerate() {
            let id = if t.msgs.len() == 1 {
                format!("{}-m", t.id)
            } else {
                format!("{}-m{i}", t.id)
            };
            let date = m.date.min(self.now - 60_000);
            first = first.min(date);
            last = last.max(date);
            let sent = m.from.email.to_lowercase() == me_lc;
            let subject = match &m.subject {
                Some(s) => s.clone(),
                None if i == 0 => t.subject.clone(),
                None => format!("Re: {}", t.subject),
            };
            let mut body = m.body.clone();
            if m.quote {
                if let Some((pf, pd, pb)) = &prev {
                    body = format!(
                        "{}\n\n{}\n{}",
                        body.trim_end(),
                        attribution(pf, *pd),
                        quote_block(pb)
                    );
                }
            }
            let mut labels: Vec<String> = m.labels.clone();
            if sent {
                labels.push("SENT".into());
            } else {
                let age = self.now - date;
                // Recent mail sits in the inbox; older mail is mostly archived.
                let h = (date / 1000) as u64 ^ (i as u64 * 7919);
                if age < 14 * DAY || h.is_multiple_of(9) {
                    labels.push("INBOX".into());
                }
                if age < 5 * DAY && h.is_multiple_of(3) {
                    labels.push("UNREAD".into());
                }
            }
            // Trash and Spam are out of the inbox.
            if labels.iter().any(|l| l == "TRASH" || l == "SPAM") {
                labels.retain(|l| l != "INBOX");
            }
            // Gmail files received mail with no other category under
            // Primary (CATEGORY_PERSONAL); sent mail has no category.
            if !sent && !labels.iter().any(|l| l.starts_with("CATEGORY_")) {
                labels.push("CATEGORY_PERSONAL".into());
            }
            labels.sort();
            labels.dedup();
            // Facts for judging status, place and size filters. Message-level
            // combinations (`inbox-from:x`) keep a thread-level rule exact
            // for `in:inbox from:x`.
            let from_lc = m.from.email.to_lowercase();
            for l in &labels {
                match l.as_str() {
                    "INBOX" => {
                        tags.insert("in:inbox".into());
                        tags.insert(format!("inbox-from:{from_lc}"));
                    }
                    "UNREAD" => {
                        tags.insert("is:unread".into());
                        tags.insert(format!("unread-from:{from_lc}"));
                    }
                    "STARRED" => {
                        tags.insert("is:starred".into());
                    }
                    "TRASH" => {
                        tags.insert("in:trash".into());
                    }
                    "SPAM" => {
                        tags.insert("in:spam".into());
                    }
                    l if l.starts_with("CATEGORY_") => {
                        tags.insert(format!("category:{}", l[9..].to_lowercase()));
                    }
                    l if l.starts_with("Label_") => {
                        tags.insert(format!("ulabel:{l}"));
                    }
                    _ => {}
                }
            }
            if !labels.iter().any(|l| l == "INBOX") && !sent {
                tags.insert("archived".into());
            }
            if sent {
                for a in m.to.iter().chain(&m.cc) {
                    tags.insert(format!("sent-to:{}", a.email.to_lowercase()));
                }
            }
            let att_total: u64 = m.atts.iter().map(|a| a.2).sum();
            for (mb, tag) in [(1, "larger:1m"), (5, "larger:5m"), (10, "larger:10m"), (25, "larger:25m")] {
                if att_total >= mb << 20 {
                    tags.insert(tag.into());
                }
            }
            let msg_id_header = format!(
                "<{id}@mail.{}>",
                ACCOUNTS[t.account].0.split('@').nth(1).unwrap()
            );
            let snippet: String = m
                .body
                .split_whitespace()
                .take(24)
                .collect::<Vec<_>>()
                .join(" ");
            self.messages.push(Message {
                account_id: acct_email.to_string(),
                id: id.clone(),
                thread_id: t.id.clone(),
                date,
                from: m.from.clone(),
                to: m.to.clone(),
                cc: m.cc.clone(),
                bcc: Vec::new(),
                reply_to: Vec::new(),
                subject,
                snippet,
                body_text: body.clone(),
                body_html: m.html.clone(),
                label_ids: labels,
                attachments: m
                    .atts
                    .iter()
                    .enumerate()
                    .map(|(k, a)| AttachmentMeta {
                        id: format!("{id}-a{k}"),
                        filename: a.0.clone(),
                        mime_type: a.1.to_string(),
                        size: a.2,
                        content_id: None,
                        inline: false,
                    })
                    .collect(),
                message_id_header: Some(msg_id_header.clone()),
                in_reply_to: refs.last().cloned(),
                references: refs.clone(),
                list_unsubscribe: m.list_unsubscribe.then(|| {
                    format!(
                        "<https://{}/unsubscribe?u=1>",
                        m.from.email.split('@').nth(1).unwrap_or("x.example")
                    )
                }),
                list_unsubscribe_post: m.list_unsubscribe.then_some(true),
                sender_authenticated: true,
            });
            refs.push(msg_id_header);
            if !sent {
                tags.insert(format!("from:{}", m.from.email.to_lowercase()));
            }
            for a in m.to.iter().chain(&m.cc) {
                if a.email.to_lowercase() != me_lc {
                    tags.insert(format!("to:{}", a.email.to_lowercase()));
                }
            }
            if !m.atts.is_empty() {
                tags.insert("has:attachment".into());
            }
            for a in &m.atts {
                tags.insert(format!("file:{}", a.0.to_lowercase()));
            }
            if m.atts.iter().any(|a| a.1 == "application/pdf") {
                tags.insert("has:pdf".into());
                // Message-level fact, for judging `from:x has:pdf` exactly.
                tags.insert(format!("pdf-from:{}", m.from.email.to_lowercase()));
            }
            if sent {
                tags.insert("has:sent".into());
            }
            prev = Some((m.from.clone(), date, body));
        }
        self.threads.push(ThreadFacts {
            account_id: acct_email.to_string(),
            thread_id: t.id,
            subject: t.subject,
            tags,
            first,
            last,
        });
    }

    /// Load the mailbox into a fresh store at `path` through the public API.
    pub fn load_into(&self, store: &Store) -> penguin_core::Result<()> {
        const SYSTEM: &[&str] = &[
            "INBOX",
            "SENT",
            "DRAFT",
            "TRASH",
            "SPAM",
            "STARRED",
            "IMPORTANT",
            "UNREAD",
            "CATEGORY_PERSONAL",
            "CATEGORY_PROMOTIONS",
            "CATEGORY_UPDATES",
            "CATEGORY_SOCIAL",
            "CATEGORY_FORUMS",
        ];
        for (i, (email, nick)) in ACCOUNTS.iter().enumerate() {
            store.upsert_account(&Account {
                id: email.to_string(),
                email: email.to_string(),
                display_name: Some(ME_NAME.into()),
                nickname: Some(nick.to_string()),
                color: "#4F7CFF".into(),
                added_at: i as i64,
                ..Account::default()
            })?;
            let mut labels: Vec<Label> = SYSTEM
                .iter()
                .map(|id| Label {
                    account_id: email.to_string(),
                    id: id.to_string(),
                    name: id.to_string(),
                    kind: "system".into(),
                    color: None,
                    unread_count: None,
                    hidden: false,
                })
                .collect();
            for (a, name) in &self.user_labels {
                if *a == i {
                    labels.push(Label {
                        account_id: email.to_string(),
                        id: format!("Label_{}", name.replace(['/', ' '], "_")),
                        name: name.clone(),
                        kind: "user".into(),
                        color: None,
                        unread_count: None,
                        hidden: false,
                    });
                }
            }
            store.replace_labels(email, &labels)?;
        }
        for chunk in self.messages.chunks(500) {
            store.upsert_messages(chunk)?;
        }
        store.optimize()
    }

    /// A hash of the mailbox's structure (seed, conversation and message ids,
    /// how many facts each conversation has), so a diff can tell whether two
    /// runs used the same
    /// generator. Dates and the date words in bodies are left out: they move
    /// with the day the corpus is generated, while every age relative to
    /// "now" stays the same.
    pub fn fingerprint(&self) -> String {
        let mut h = fnv(0xcbf2_9ce4_8422_2325, &self.seed.to_string());
        for t in &self.threads {
            h = fnv(h, &t.thread_id);
            // Tag values can embed dates (bill months, invoice years); their
            // number can't.
            h = fnv(h, &t.tags.len().to_string());
        }
        for m in &self.messages {
            h = fnv(h, &m.id);
        }
        format!("{h:016x}")
    }
}
