//! Answering: each intent is a handful of exact, indexed queries.
//!
//! - Mail with a target: from-side via `messages_from` (per address),
//!   to/cc-side via the recipients/cc FTS columns (per address, or per
//!   domain for a company), date ranges as rowid ranges.
//! - Aggregates (counts, sums) cover every matching message; nothing is
//!   sampled. Sums list every line they add so they can be checked.
//! - Topic questions reuse `Store::search`, so they rank like search.
//! - Extracted facts (flights, orders, bills…) are answered in facts.rs;
//!   quoted-sentence answers in passages.rs.

#[path = "facts.rs"]
mod facts;
#[path = "passages.rs"]
mod passages;
#[path = "query_exec.rs"]
mod query_exec;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, NaiveDate};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Deserialize;

use super::extract::{self, clip};
use super::intent::{self, Dir, FactKind, First, Focus, Intent, Kind, Question, Which};
use super::resolve::{self, Alias, Target, TargetKind};
use super::*;
use crate::store::{
    Body, F_ATTACH, F_DOC, F_DRAFT, F_IMAGE, F_INBOX, F_NEWSLETTER, F_PDF, F_SENT, F_SHEET, F_SPAM,
    F_TRASH, F_UNREAD, ROWID_SLOTS,
};
use crate::types::{account_scope, SearchRequest};

const HIDDEN: i64 = F_TRASH | F_SPAM | F_DRAFT;
const DAY: i64 = 86_400_000;
/// Most messages a target query returns per side (a very heavy
/// correspondent still answers in milliseconds).
const MAX_ROWS: i64 = 20_000;
/// Most receipts read for a sum.
const MAX_RECEIPTS: usize = 3_000;
/// Items shown under an answer.
const MAX_ITEMS: usize = 8;

const PUBLIC_MAIL: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "yahoo.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "aol.com",
    "proton.me",
    "protonmail.com",
    "hey.com",
    "fastmail.com",
];

// ------------------------------------------------------------- context

struct Cx<'a> {
    store: &'a Store,
    semantic: Option<AskSemantic<'a>>,
    /// Messages the fact scanner hasn't read, once asked.
    pending: std::cell::Cell<Option<i64>>,
    scope: Option<Vec<String>>,
    ask_scope: &'a AskScope,
    now: i64,
    off: i32,
    today: NaiveDate,
    /// [from, to) unix ms of the question's date scope.
    lo: Option<i64>,
    hi: Option<i64>,
    q: Question,
    question: String,
    steps: Vec<String>,
    /// Set when the name asked about fits several people ("Mike"): the
    /// others, for the answer to say so.
    namesakes: Vec<Target>,
}

impl Cx<'_> {
    fn in_scope(&self, account: &str) -> bool {
        self.scope
            .as_ref()
            .is_none_or(|s| s.iter().any(|a| a == account))
    }

    fn range_phrase(&self) -> String {
        match &self.q.range_text {
            None => String::new(),
            Some(t) => {
                let first = t.split_whitespace().next().unwrap_or("");
                if first.chars().next().is_some_and(|c| c.is_ascii_digit())
                    || crate::dates::month_num(first).is_some()
                {
                    format!(" in {t}")
                } else {
                    format!(" {t}")
                }
            }
        }
    }

    /// `after:`/`before:` operators for the question's range.
    fn range_ops(&self) -> String {
        let mut s = String::new();
        if let Some(lo) = self.lo {
            s.push_str(&format!(" after:{}", iso(local_date(lo, self.off))));
        }
        if let Some(hi) = self.hi {
            s.push_str(&format!(" before:{}", iso(local_date(hi, self.off))));
        }
        s
    }

    fn answer(&self, intent: AskIntent) -> AskAnswer {
        AskAnswer {
            intent,
            question: self.question.clone(),
            headline: String::new(),
            detail: None,
            facts: vec![],
            timeline: None,
            items: vec![],
            person: None,
            candidates: vec![],
            confidence: AskConfidence::High,
            steps: vec![],
            search_query: None,
            followups: vec![],
            cards: vec![],
            passages: vec![],
            sum: None,
            coverage: None,
            understood: None,
            groups: vec![],
            result: None,
            took_ms: 0.0,
        }
    }

    fn finish(&mut self, mut a: AskAnswer) -> AskAnswer {
        let mut steps = std::mem::take(&mut self.steps);
        steps.append(&mut a.steps);
        a.steps = steps;
        // "Mike" when you write to two Mikes: say which one this is (the
        // others are "did you mean" chips) rather than pick silently. An
        // answer that already lists everyone the name fits says nothing.
        let namesakes = std::mem::take(&mut self.namesakes);
        if let (false, Some(p)) = (namesakes.is_empty(), &a.person) {
            let listed = namesakes.iter().all(|t| a.headline.contains(&t.label));
            if a.confidence != AskConfidence::None && !listed {
                let others: Vec<&str> = namesakes.iter().map(|t| t.label.as_str()).collect();
                let note = if others.len() == 1 {
                    format!("{} has the same name; this is {}.", others[0], p.label)
                } else {
                    format!("{} also have the name; this is {}.", others.join(", "), p.label)
                };
                a.detail = Some(match a.detail.take() {
                    Some(d) if !d.is_empty() => format!("{d} · {note}"),
                    _ => note,
                });
                if a.confidence == AskConfidence::High {
                    a.confidence = AskConfidence::Medium;
                }
            }
        }
        // A date phrase read from the question is shown as an explicit,
        // removable interpretation (like search's date: chip).
        if let (Some(t), Some(span)) = (&self.q.range_text, self.range_span()) {
            a.facts.insert(
                0,
                self.fact("Dates", format!("\u{201c}{t}\u{201d} = {span}")),
            );
            a.followups.insert(
                0,
                suggest(
                    &format!("Any time (not \u{201c}{t}\u{201d})"),
                    capitalize(&self.q.text),
                ),
            );
        }
        a
    }

    /// "Jan 1, 2026 – Dec 31, 2026" for the question's date scope.
    fn range_span(&self) -> Option<String> {
        if self.lo.is_none() && self.hi.is_none() {
            return None;
        }
        let from = self
            .lo
            .map(|l| fmt_day(l, self.off))
            .unwrap_or_else(|| "the beginning".into());
        let to = self
            .hi
            .map(|h| fmt_day(h - 1, self.off))
            .unwrap_or_else(|| "now".into());
        Some(format!("{from} – {to}"))
    }

    fn fact(&self, label: &str, value: impl Into<String>) -> AskFact {
        AskFact {
            label: label.into(),
            value: value.into(),
            date: None,
            cite: None,
        }
    }

    fn date_fact(&self, label: &str, row: &Row) -> AskFact {
        AskFact {
            label: label.into(),
            value: format!(
                "{} · {}",
                fmt_day(row.date, self.off),
                clip(&row.subject_or_none(), 70)
            ),
            date: Some(row.date),
            cite: Some(row.cite()),
        }
    }
}

// ----------------------------------------------------------------- time

fn local_date(ms: i64, off: i32) -> NaiveDate {
    DateTime::from_timestamp_millis(ms + off as i64 * 1000)
        .unwrap_or_default()
        .date_naive()
}

fn day_ms(d: NaiveDate, off: i32) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp_millis()
        - off as i64 * 1000
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn fmt_date(d: NaiveDate) -> String {
    d.format("%b %-d, %Y").to_string()
}

fn fmt_day(ms: i64, off: i32) -> String {
    fmt_date(local_date(ms, off))
}

fn fmt_month(d: NaiveDate) -> String {
    d.format("%b %Y").to_string()
}

/// "today", "yesterday", "3 days ago", "in 2 weeks", "5 months ago".
fn relative(d: NaiveDate, today: NaiveDate) -> String {
    let days = (d - today).num_days();
    let (n, unit) = match days.abs() {
        0 => return "today".into(),
        1 => {
            return if days > 0 {
                "tomorrow".into()
            } else {
                "yesterday".into()
            }
        }
        n @ 2..=13 => (n, "day"),
        n @ 14..=59 => (n / 7, "week"),
        n @ 60..=729 => (n / 30, "month"),
        n => (n / 365, "year"),
    };
    let s = if n == 1 { "" } else { "s" };
    if days > 0 {
        format!("in {n} {unit}{s}")
    } else {
        format!("{n} {unit}{s} ago")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn money(v: f64, cur: &str) -> String {
    let sym = match cur {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "CAD" => "CA$",
        "AUD" => "A$",
        "PHP" => "₱",
        "MXN" => "MX$",
        "INR" => "₹",
        "JPY" => "¥",
        _ => "",
    };
    let neg = v < 0.0;
    let cents = (v.abs() * 100.0).round() as i64;
    let int = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let body = if cur == "JPY" {
        grouped
    } else {
        format!("{grouped}.{:02}", cents % 100)
    };
    let body = if sym.is_empty() {
        format!("{body} {cur}")
    } else {
        format!("{sym}{body}")
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

// ----------------------------------------------------------------- rows

#[derive(Debug, Clone)]
struct Row {
    rowid: i64,
    id: String,
    account_id: String,
    thread_id: String,
    thread_rowid: i64,
    date: i64,
    flags: i64,
    from_email: String,
    from_name: Option<String>,
    subject: String,
    snippet: String,
    /// Sent by the target.
    from_them: bool,
    /// Sent by you.
    by_me: bool,
}

impl Row {
    fn cite(&self) -> AskCite {
        AskCite {
            account_id: self.account_id.clone(),
            thread_id: self.thread_id.clone(),
            message_id: self.id.clone(),
        }
    }

    fn subject_or_none(&self) -> String {
        if self.subject.trim().is_empty() {
            "(no subject)".into()
        } else {
            self.subject.clone()
        }
    }

    fn item(&self, note: Option<String>) -> AskItem {
        AskItem {
            account_id: self.account_id.clone(),
            thread_id: self.thread_id.clone(),
            message_id: self.id.clone(),
            subject: self.subject_or_none(),
            from: Address {
                name: self.from_name.clone(),
                email: self.from_email.clone(),
            },
            date: self.date,
            snippet: clip(&self.snippet, 180),
            note,
            amount: None,
            sent: self.by_me,
        }
    }
}

const ROW_COLS: &str = "m.rowid, m.id, m.account_id, t.thread_id, m.thread_rowid, m.date, m.flags, m.from_email, m.from_name, m.subject, m.snippet";

fn read_row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    read_row_at(r, 0)
}

/// [`read_row`] for `ROW_COLS` starting at column `o`.
fn read_row_at(r: &rusqlite::Row, o: usize) -> rusqlite::Result<Row> {
    Ok(Row {
        rowid: r.get(o)?,
        id: r.get(o + 1)?,
        account_id: r.get(o + 2)?,
        thread_id: r.get(o + 3)?,
        thread_rowid: r.get(o + 4)?,
        date: r.get(o + 5)?,
        flags: r.get(o + 6)?,
        from_email: r.get::<_, String>(o + 7)?.to_lowercase(),
        from_name: r.get(o + 8)?,
        subject: r.get(o + 9)?,
        snippet: r.get(o + 10)?,
        from_them: false,
        by_me: false,
    })
}

/// FTS5 phrase for an address or domain as the recipients/cc columns
/// tokenize it ("priya@linden.example" → "priya linden example").
fn fts_phrase(s: &str) -> Option<String> {
    let toks: Vec<String> = crate::text::tokens(s).into_iter().map(|t| t.2).collect();
    (!toks.is_empty()).then(|| format!("\"{}\"", toks.join(" ")))
}

/// The target as a full-text condition on the address columns: its
/// addresses, and a company's domains. Every message `target_rows` returns
/// matches it (mail from them has the address in `sender`; mail to them was
/// found by these phrases), so it can narrow a full-text scan to their mail.
/// None when an address has nothing to search for.
fn target_fts(t: &Target) -> Option<String> {
    let domains = if t.kind == TargetKind::Company {
        &t.domains[..]
    } else {
        &[]
    };
    let keys = t
        .emails
        .iter()
        .chain(domains)
        .map(|k| fts_phrase(k))
        .collect::<Option<Vec<String>>>()?;
    (!keys.is_empty()).then(|| format!("{{sender recipients cc}} : ({})", keys.join(" OR ")))
}

fn own_addresses(c: &Connection) -> Result<HashSet<String>> {
    Ok(c.prepare_cached("SELECT lower(email) FROM accounts")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn rowid_bounds(lo: Option<i64>, hi: Option<i64>) -> (i64, i64) {
    (
        lo.map_or(i64::MIN, |l| l.saturating_mul(ROWID_SLOTS)),
        hi.map_or(i64::MAX, |h| h.saturating_mul(ROWID_SLOTS)),
    )
}

/// Every message between you and the target (either direction, plus
/// messages they were copied on), oldest first.
fn target_rows(
    cx: &Cx,
    c: &Connection,
    t: &Target,
    lo: Option<i64>,
    hi: Option<i64>,
) -> Result<Vec<Row>> {
    let own = own_addresses(c)?;
    let (rlo, rhi) = rowid_bounds(lo, hi);
    let mut by_rowid: HashMap<i64, Row> = HashMap::new();
    let emails: HashSet<&str> = t.emails.iter().map(String::as_str).collect();

    let mut from = c.prepare_cached(&format!(
        "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
         WHERE m.from_email = ?1 COLLATE NOCASE AND m.rowid >= ?2 AND m.rowid < ?3 AND m.flags & ?4 = 0
         ORDER BY m.date DESC LIMIT ?5"
    ))?;
    for e in &t.emails {
        for r in from.query_map(params![e, rlo, rhi, HIDDEN, MAX_ROWS], read_row)? {
            let mut r = r?;
            r.from_them = true;
            by_rowid.insert(r.rowid, r);
        }
    }

    // To/cc side: one phrase per address, or the domain for a company.
    let phrases: Vec<String> = if t.kind == TargetKind::Company {
        t.domains.iter().filter_map(|d| fts_phrase(d)).collect()
    } else {
        t.emails.iter().filter_map(|e| fts_phrase(e)).collect()
    };
    if !phrases.is_empty() {
        let expr = format!("{{recipients cc}} : ({})", phrases.join(" OR "));
        let mut to = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM messages_fts f JOIN messages m ON m.rowid = f.rowid JOIN threads t ON t.rowid = m.thread_rowid
             WHERE messages_fts MATCH ?1 AND f.rowid >= ?2 AND f.rowid < ?3 AND m.flags & ?4 = 0
             ORDER BY f.rowid DESC LIMIT ?5"
        ))?;
        for r in to.query_map(params![expr, rlo, rhi, HIDDEN, MAX_ROWS], read_row)? {
            let r = r?;
            by_rowid.entry(r.rowid).or_insert(r);
        }
    }
    let mut rows: Vec<Row> = by_rowid
        .into_values()
        .filter(|r| cx.in_scope(&r.account_id))
        .map(|mut r| {
            r.by_me = r.flags & F_SENT != 0 || own.contains(&r.from_email);
            r.from_them = r.from_them || emails.contains(r.from_email.as_str());
            if r.by_me {
                r.from_them = false;
            }
            r
        })
        .collect();
    rows.sort_by_key(|r| r.rowid);
    Ok(rows)
}

fn list_emails(e: &[String]) -> String {
    match e.len() {
        0 => "nobody".into(),
        1..=3 => e.join(", "),
        n => format!("{}, … ({n} addresses)", e[..3].join(", ")),
    }
}

/// A Penguin search query showing mail with the target.
fn target_query(t: &Target, dir: Dir) -> String {
    let keys: Vec<String> = if t.kind == TargetKind::Company {
        t.domains.clone()
    } else {
        t.emails.iter().take(4).cloned().collect()
    };
    let one = |op: &str| {
        keys.iter()
            .map(|k| format!("{op}:{k}"))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    match dir {
        Dir::FromThem => one("from"),
        Dir::ToThem => one("to"),
        Dir::Any => format!("{} OR {}", one("from"), one("to")),
    }
}

fn person_of(t: &Target) -> AskPerson {
    AskPerson {
        label: t.label.clone(),
        name: t.name.clone(),
        emails: t.emails.clone(),
        domain: t.domains.first().cloned().or_else(|| {
            t.emails
                .first()
                .map(|e| e.rsplit('@').next().unwrap_or("").to_string())
        }),
        company: t.kind == TargetKind::Company,
    }
}

fn load_text(c: &Connection, rowid: i64) -> Result<String> {
    let body: Option<Body> = c
        .prepare_cached("SELECT body_text FROM message_bodies WHERE rowid = ?1")?
        .query_row([rowid], |r| r.get(0))
        .optional()?;
    Ok(body.map(|b| b.0).unwrap_or_default())
}

#[derive(Deserialize, Default)]
struct Recipients {
    #[serde(default)]
    to: Vec<Address>,
    #[serde(default)]
    cc: Vec<Address>,
}

fn recipients(c: &Connection, rowid: i64) -> Result<Vec<String>> {
    let extra: Option<String> = c
        .prepare_cached("SELECT extra FROM message_bodies WHERE rowid = ?1")?
        .query_row([rowid], |r| r.get(0))
        .optional()?;
    let r: Recipients = extra
        .and_then(|e| serde_json::from_str(&e).ok())
        .unwrap_or_default();
    Ok(r.to
        .into_iter()
        .chain(r.cc)
        .map(|a| a.email.to_lowercase())
        .collect())
}

// ---------------------------------------------------------- resolution

enum Resolved {
    Found(Target, Vec<AskSuggestion>),
    Answer(Box<AskAnswer>),
}

fn aliases(c: &Connection, scope: &AskScope) -> Result<Vec<Alias>> {
    let mut out = Vec::new();
    let accounts: Vec<(String, Option<String>)> = c
        .prepare_cached("SELECT lower(email), nickname FROM accounts")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let domain = |e: &str| e.rsplit('@').next().unwrap_or("").to_string();
    let company = |d: &String| !PUBLIC_MAIL.contains(&d.as_str());
    for (email, nick) in &accounts {
        if let Some(n) = nick.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            let d = domain(email);
            if company(&d) {
                out.push(Alias {
                    name: n.to_lowercase(),
                    domains: vec![d],
                });
            }
        }
    }
    for a in &scope.aliases {
        let domains: Vec<String> = a
            .account_ids
            .iter()
            .map(|e| domain(&e.to_lowercase()))
            .filter(company)
            .collect();
        if !domains.is_empty() && !a.name.trim().is_empty() {
            out.push(Alias {
                name: a.name.trim().to_lowercase(),
                domains,
            });
        }
    }
    Ok(out)
}

fn resolve_who(cx: &mut Cx, who: &str, intent: AskIntent) -> Result<Resolved> {
    if intent::is_pronoun(who) {
        let known = match &cx.ask_scope.person {
            Some(p) if !p.is_empty() => cx.store.read(|c| resolve::known(c, p, None))?,
            _ => None,
        };
        return Ok(match known {
            Some(t) => {
                cx.steps.push(format!(
                    "\"{who}\" = {} <{}> from the previous answer",
                    t.label,
                    list_emails(&t.emails)
                ));
                Resolved::Found(t, vec![])
            }
            None => {
                let mut a = cx.answer(intent);
                a.headline = format!("Who do you mean by \u{201c}{who}\u{201d}?");
                a.detail = Some("Ask about someone by name first, then follow up with \u{201c}he\u{201d} or \u{201c}they\u{201d}.".into());
                a.confidence = AskConfidence::None;
                Resolved::Answer(Box::new(cx.finish(a)))
            }
        });
    }
    let scope = cx.ask_scope.clone();
    let res = cx.store.read(|c| {
        let own = own_addresses(c)?;
        let al = aliases(c, &scope)?;
        resolve::resolve(c, who, &own, &al)
    })?;
    let Some(target) = res.target else {
        let mut a = cx.answer(intent);
        a.headline = format!("I don't know anyone matching \u{201c}{who}\u{201d}");
        a.detail = Some(
            "No sender or recipient in your local mail matches that name, address or company."
                .into(),
        );
        a.confidence = AskConfidence::None;
        a.search_query = Some(who.to_string());
        a.steps.push(format!("Looked up \u{201c}{who}\u{201d} in people (names, addresses, domains, acronyms, account names): no match"));
        a.followups.push(AskSuggestion {
            label: format!("Search for \u{201c}{who}\u{201d}"),
            question: who.to_string(),
        });
        return Ok(Resolved::Answer(Box::new(cx.finish(a))));
    };
    cx.steps.push(format!("Resolved {}", target.how));
    if !res.namesakes.is_empty() {
        cx.steps.push(format!(
            "\u{201c}{who}\u{201d} also fits {}",
            res.namesakes.iter().map(|t| format!("{} <{}>", t.label, list_emails(&t.emails))).collect::<Vec<_>>().join(", ")
        ));
    }
    cx.namesakes = res.namesakes.clone();
    let mut others: Vec<&Target> = res.alternatives.iter().collect();
    for n in &res.namesakes {
        if !others.iter().any(|a| a.emails.first() == n.emails.first()) {
            others.push(n);
        }
    }
    let candidates = others
        .into_iter()
        .map(|alt| {
            let key = if alt.kind == TargetKind::Company {
                alt.domains.first().cloned().unwrap_or_default()
            } else {
                alt.emails.first().cloned().unwrap_or_default()
            };
            let label = if alt.kind == TargetKind::Company {
                alt.label.clone()
            } else {
                format!("{} <{key}>", alt.label)
            };
            AskSuggestion {
                label,
                question: replace_who(&cx.q.text, who, &key, cx.q.range_text.as_deref()),
            }
        })
        .collect();
    Ok(Resolved::Found(target, candidates))
}

fn replace_who(text: &str, who: &str, with: &str, range: Option<&str>) -> String {
    let mut q = text.replacen(who, with, 1);
    if let Some(r) = range {
        q.push(' ');
        q.push_str(r);
    }
    let mut c = q.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => q,
    }
}

fn suggest(label: &str, question: String) -> AskSuggestion {
    AskSuggestion {
        label: label.into(),
        question,
    }
}

fn person_followups(t: &Target, skip: AskIntent) -> Vec<AskSuggestion> {
    let who = match t.kind {
        TargetKind::Person => t.emails.first().cloned().unwrap_or_default(),
        TargetKind::Company => t.domains.first().cloned().unwrap_or_default(),
    };
    let short = t
        .name
        .as_deref()
        .and_then(|n| n.split_whitespace().next())
        .unwrap_or(&t.label)
        .to_string();
    let all = [
        (
            AskIntent::LastContact,
            format!("Last email with {short}"),
            format!("When did I last email {who}"),
        ),
        (
            AskIntent::Relationship,
            "Relationship timeline".to_string(),
            format!("How long have I known {who}"),
        ),
        (
            AskIntent::LatestItem,
            format!("Latest attachment from {short}"),
            format!("Latest attachment from {who}"),
        ),
        (
            AskIntent::WaitingOn,
            format!("Waiting on {short}?"),
            format!("What am I waiting on from {who}"),
        ),
        (
            AskIntent::Count,
            "Emails this year".to_string(),
            format!("How many emails with {who} this year"),
        ),
        (
            AskIntent::WhoIs,
            format!("About {short}"),
            format!("Who is {who}"),
        ),
    ];
    all.into_iter()
        .filter(|(i, _, _)| *i != skip)
        .take(4)
        .map(|(_, l, q)| suggest(&l, q))
        .collect()
}

// --------------------------------------------------------------- answer

pub(super) fn answer<'a>(
    store: &'a Store,
    question: &str,
    scope: &'a AskScope,
    now: i64,
    off: i32,
    semantic: Option<AskSemantic<'a>>,
) -> Result<AskAnswer> {
    let today = local_date(now, off);
    let q = intent::parse(question, today);
    let lo = q.range.and_then(|r| r.from).map(|d| day_ms(d, off));
    let hi = q.range.and_then(|r| r.to).map(|d| day_ms(d, off));
    let mut cx = Cx {
        store,
        semantic,
        pending: std::cell::Cell::new(None),
        scope: account_scope(None, scope.account_ids.as_deref()),
        ask_scope: scope,
        now,
        off,
        today,
        lo,
        hi,
        q: q.clone(),
        question: question.trim().to_string(),
        steps: vec![],
        namesakes: vec![],
    };
    if let (Some(t), Some(span)) = (&q.range_text, cx.range_span()) {
        cx.steps.push(format!("Read \u{201c}{t}\u{201d} as {span}"));
    }
    // Questions no template reads exactly (or reads only as a topic) are
    // tried as a structured query: counts, sums, averages, extremes,
    // groups, comparisons and lists over facts and mail.
    // The fact templates stay the fast path for one thing (the next flight,
    // where a parcel is); counts, sums, lists, groups and comparisons of
    // those things go to the query layer, which covers every one.
    let open = matches!(
        q.intent,
        Intent::Unknown | Intent::Passage { .. } | Intent::When { .. } | Intent::Find { .. }
    );
    // A sum template ("how much did I spend at X") is a fact question too:
    // when the grammar reads all of it ("total cost of my Streamly receipts
    // this year"), the query layer adds up every matching fact; the
    // template's merchant slot can't tell "cost of my streamly" from a name.
    let fact_template = matches!(
        q.intent,
        Intent::Flight { .. }
            | Intent::Stay { .. }
            | Intent::Package { .. }
            | Intent::Orders { .. }
            | Intent::Bills { .. }
            | Intent::Booking { .. }
            | Intent::CountFacts { .. }
            | Intent::Spend { .. }
    );
    if open || fact_template {
        let parsed = super::qparse::parse(question, today).filter(|p| p.unread.is_empty());
        // "What did I order from Paperleaf" stays the template's latest
        // order; "what did I order from Paperleaf this year" lists them all.
        let aggregate = |q: &AskQuery| {
            !matches!(q.op, QueryOp::First | QueryOp::Last | QueryOp::Next | QueryOp::List)
                || (q.op == QueryOp::List && q.timeframe.is_some())
                || q.group_by.is_some()
                || !q.compare.is_empty()
        };
        // A spending template about a category ("how much did I spend on
        // hotels") keeps its own answer; one about a merchant goes through
        // the query, whose reading ("total cost of my Streamly receipts")
        // names the merchant where the template's slot can't.
        let spend_ok = |q: &AskQuery| {
            !matches!(cx.q.intent, Intent::Spend { .. }) || q.merchant.is_some() || q.group_by.is_some() || !q.compare.is_empty()
        };
        if let Some(p) = parsed.filter(|p| open || (aggregate(&p.query) && spend_ok(&p.query))) {
            let (lo, hi, saved_q, saved_steps) = (cx.lo, cx.hi, cx.q.clone(), cx.steps.clone());
            if let Some(a) = query_exec::run_query(&mut cx, &p.query, QuerySource::Grammar)? {
                return Ok(a);
            }
            cx.lo = lo;
            cx.hi = hi;
            cx.q = saved_q;
            cx.steps = saved_steps;
        }
    }
    let fact_question = matches!(
        q.intent,
        Intent::Flight { .. }
            | Intent::Stay { .. }
            | Intent::Package { .. }
            | Intent::Orders { .. }
            | Intent::Bills { .. }
            | Intent::Booking { .. }
            | Intent::ContactInfo { .. }
    );
    // A spending question whose merchant wasn't found may still read as a
    // query; it doesn't fall back to quoted sentences ("I don't know anyone
    // matching …" is the honest answer to a sum).
    let spend_question = matches!(q.intent, Intent::Spend { .. });
    let a = match q.intent.clone() {
        Intent::LastContact { who, dir } => contact(&mut cx, &who, dir, true, First::Email),
        Intent::FirstContact { who, dir, first } => contact(&mut cx, &who, dir, false, first),
        Intent::Relationship { who, focus } => relationship(&mut cx, &who, focus),
        Intent::LatestItem { who, kind } => latest_item(&mut cx, who.as_deref(), kind),
        Intent::Count { who, dir, kind } => count(&mut cx, &who, dir, kind),
        Intent::CountTopic { topic } => count_topic(&mut cx, &topic),
        Intent::Spend { merchant } => spend(&mut cx, &merchant),
        Intent::WhoAbout { topic } => who_about(&mut cx, &topic),
        Intent::TopSenders => top_senders(&mut cx),
        Intent::WhoIs { who } => who_is(&mut cx, &who),
        Intent::WaitingOn { who } => waiting_on(&mut cx, who.as_deref()),
        Intent::OweReplies { who } => owe_replies(&mut cx, who.as_deref()),
        Intent::When { topic } => when(&mut cx, &topic),
        Intent::Flight {
            place,
            which,
            field,
        } => facts::flight(&mut cx, place.as_deref(), which, field),
        Intent::Stay { place, which } => facts::stay(&mut cx, place.as_deref(), which),
        Intent::Package { what } => facts::package(&mut cx, what.as_deref()),
        Intent::Orders { merchant } => facts::orders(&mut cx, merchant.as_deref()),
        Intent::Bills { what } => facts::bills(&mut cx, what.as_deref()),
        Intent::Booking { what } => facts::booking(&mut cx, what.as_deref()),
        Intent::Code { service } => facts::code(&mut cx, service.as_deref()),
        Intent::Subscriptions => facts::subscriptions(&mut cx),
        Intent::ContactInfo { who, field } => facts::contact_info(&mut cx, &who, field),
        Intent::CountFacts { kind, who } => facts::count_facts(&mut cx, kind, who.as_deref()),
        Intent::Said { who, topic } => passages::said(&mut cx, who.as_deref(), &topic),
        Intent::DidReply { who, topic } => passages::did_reply(&mut cx, &who, &topic),
        Intent::Find { who, topic } => passages::find_thing(&mut cx, who.as_deref(), &topic),
        Intent::Passage { topic } => passages::topic(&mut cx, AskIntent::Passage, &topic),
        Intent::Unknown => unknown(&mut cx),
    }?;
    // "Who is the adjuster on my insurance claim" names a role, not a
    // person in the address book: find the sentence that says.
    if let Intent::WhoIs { who } = &q.intent {
        if a.confidence == AskConfidence::None && who.split_whitespace().count() >= 3 {
            let text = cx.q.text.clone();
            let p = passages::topic(&mut cx, AskIntent::Passage, &text)?;
            if !p.passages.is_empty() {
                return Ok(p);
            }
        }
    }
    // Nothing extracted matches ("how much is the new rent?" when the rent
    // is only in a landlord's email): answer from the text instead, and
    // keep the fact answer if the text has nothing either.
    if (fact_question || spend_question) && a.confidence == AskConfidence::None {
        // First the question read as a query, which may name the thing
        // another way ("how much is my Streamly subscription": no Streamly
        // bill, but its latest charge).
        if let Some(p) = super::qparse::parse(question, today).filter(|p| p.unread.is_empty()) {
            let (lo, hi, saved_q, saved_steps) = (cx.lo, cx.hi, cx.q.clone(), cx.steps.clone());
            if let Some(qa) = query_exec::run_query(&mut cx, &p.query, QuerySource::Grammar)? {
                let found = !(qa.items.is_empty() && qa.cards.is_empty() && qa.groups.is_empty());
                if found && qa.confidence != AskConfidence::None {
                    return Ok(qa);
                }
            }
            cx.lo = lo;
            cx.hi = hi;
            cx.q = saved_q;
            cx.steps = saved_steps;
        }
    }
    if fact_question && a.confidence == AskConfidence::None {
        let text = cx.q.text.clone();
        let mut p = passages::topic(&mut cx, AskIntent::Passage, &text)?;
        if !p.passages.is_empty() {
            p.steps.insert(
                0,
                format!(
                    "No extracted booking, order or bill matched ({})",
                    a.headline
                ),
            );
            return Ok(p);
        }
    }
    Ok(a)
}

/// The user's local date at `now`.
pub(super) fn today_of(now: i64, off: i32) -> NaiveDate {
    local_date(now, off)
}

/// Answer a query written by the model or the user.
pub(super) fn answer_query(
    store: &Store,
    question: &str,
    query: &AskQuery,
    source: QuerySource,
    scope: &AskScope,
    now: i64,
    off: i32,
) -> Result<AskAnswer> {
    let today = local_date(now, off);
    let q = Question {
        intent: Intent::Unknown,
        range: None,
        range_text: None,
        text: question.trim().to_string(),
    };
    let mut cx = Cx {
        store,
        semantic: None,
        pending: std::cell::Cell::new(None),
        scope: account_scope(None, scope.account_ids.as_deref()),
        ask_scope: scope,
        now,
        off,
        today,
        lo: None,
        hi: None,
        q,
        question: question.trim().to_string(),
        steps: vec![],
        namesakes: vec![],
    };
    cx.steps.push(match source {
        QuerySource::Model => "Read the question with Apple Intelligence (on this Mac), then checked its reading against the query schema and your mail".to_string(),
        QuerySource::Edited => "Answered your edited reading of the question".to_string(),
        QuerySource::Grammar => "Read the question with Penguin's grammar".to_string(),
    });
    match query_exec::run_query(&mut cx, query, source)? {
        Some(a) => Ok(a),
        None => {
            let mut a = cx.answer(AskIntent::Query);
            a.headline = "I couldn't answer that reading of the question".into();
            a.confidence = AskConfidence::None;
            Ok(cx.finish(a))
        }
    }
}

fn nothing_with(cx: &mut Cx, intent: AskIntent, t: &Target, what: &str) -> AskAnswer {
    let mut a = cx.answer(intent);
    a.headline = format!(
        "No {what} with {}{} in your local mail",
        t.label,
        cx.range_phrase()
    );
    a.confidence = AskConfidence::None;
    a.person = Some(person_of(t));
    a.search_query = Some(format!("{}{}", target_query(t, Dir::Any), cx.range_ops()));
    a.detail = Some("Older mail may not be downloaded yet; \u{201c}Also search Gmail\u{201d} in search checks the server.".into());
    cx.finish(a)
}

// ---------------------------------------------------- last/first contact

/// "1 year 7 months", "3 months", "2 weeks": how long from `a` to `b`.
fn duration_words(a: NaiveDate, b: NaiveDate) -> String {
    let mut months = (b.year() - a.year()) * 12 + b.month() as i32 - a.month() as i32;
    if b.day() < a.day() {
        months -= 1;
    }
    let (y, m) = (months / 12, months % 12);
    match (y, m) {
        (0, 0) => {
            let days = (b - a).num_days().max(0) as usize;
            if days >= 14 {
                plural(days / 7, "week", "weeks")
            } else {
                plural(days, "day", "days")
            }
        }
        (0, m) => plural(m as usize, "month", "months"),
        (y, 0) => plural(y as usize, "year", "years"),
        (y, m) => format!("{} {}", plural(y as usize, "year", "years"), plural(m as usize, "month", "months")),
    }
}

/// Last or first email with someone. A first-contact question that asks
/// when a relationship began ("when did I hire Julia") or how long you've
/// known someone is answered from the same first email, in either
/// direction, and says that's what it is: mail shows when you started
/// writing, not a hire date.
fn contact(cx: &mut Cx, who: &str, dir: Dir, last: bool, first: First) -> Result<AskAnswer> {
    let intent = if last {
        AskIntent::LastContact
    } else {
        AskIntent::FirstContact
    };
    let (t, candidates) = match resolve_who(cx, who, intent)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    let (lo, hi) = (cx.lo, cx.hi);
    let rows = cx.store.read(|c| target_rows(cx, c, &t, lo, hi))?;
    cx.steps.push(format!(
        "Found {} with {} (from them, from you, or copied){}",
        plural(rows.len(), "message", "messages"),
        t.label,
        cx.range_phrase()
    ));
    let direct: Vec<&Row> = rows.iter().filter(|r| r.from_them || r.by_me).collect();
    if direct.is_empty() {
        let mut a = nothing_with(cx, intent, &t, "emails");
        a.candidates = candidates;
        return Ok(a);
    }
    let pick = |f: &dyn Fn(&Row) -> bool| -> Option<&Row> {
        if last {
            direct.iter().rev().copied().find(|r| f(r))
        } else {
            direct.iter().copied().find(|r| f(r))
        }
    };
    let from_them = pick(&|r| r.from_them);
    let from_me = pick(&|r| r.by_me);
    let either = if last {
        direct.last().copied()
    } else {
        direct.first().copied()
    };
    let chosen = match dir {
        Dir::FromThem => from_them.or(either),
        Dir::ToThem => from_me.or(either),
        Dir::Any => either,
    }
    .expect("non-empty");
    let when = local_date(chosen.date, cx.off);
    let rel = relative(when, cx.today);
    let name = &t.label;
    let mut a = cx.answer(intent);
    let subject = clip(&chosen.subject_or_none(), 90);
    let who_wrote = if chosen.by_me { "you wrote it" } else { "they wrote it" };
    a.headline = match (last, chosen.by_me) {
        (true, true) => format!("You last emailed {name} on {} ({rel})", fmt_date(when)),
        (true, false) => format!("{name} last emailed you on {} ({rel})", fmt_date(when)),
        // Either direction: the first email between you.
        (false, _) if dir == Dir::Any && first == First::Known => {
            format!("You've known {name} since {} ({})", fmt_month(when), duration_words(when, cx.today))
        }
        (false, _) if dir == Dir::Any => {
            format!("Your first email with {name}: {}, \u{201c}{subject}\u{201d}", fmt_date(when))
        }
        (false, true) => format!("You first emailed {name} on {} ({rel})", fmt_date(when)),
        (false, false) => format!("{name} first emailed you on {} ({rel})", fmt_date(when)),
    };
    if !last && dir == Dir::Any {
        a.detail = Some(match first {
            First::Known => format!("From your first email with them: {}, \u{201c}{subject}\u{201d} ({who_wrote})", fmt_date(when)),
            First::Start => format!("Your mail can't show when that started; this is the earliest email between you ({who_wrote}, {rel})."),
            First::Email => format!("{} ({who_wrote})", capitalize(&rel)),
        });
    } else if dir == Dir::FromThem && !chosen.from_them {
        a.detail = Some(format!(
            "{name} hasn't written to you{}; this is the {} message you sent.",
            cx.range_phrase(),
            if last { "latest" } else { "first" }
        ));
    } else if dir == Dir::ToThem && !chosen.by_me {
        a.detail = Some(format!(
            "You haven't written to {name}{}; this is the {} message from them.",
            cx.range_phrase(),
            if last { "latest" } else { "first" }
        ));
    } else {
        a.detail = Some(format!(
            "\u{201c}{}\u{201d}",
            clip(&chosen.subject_or_none(), 90)
        ));
    }
    let first = direct.first().copied().expect("non-empty");
    let latest = direct.last().copied().expect("non-empty");
    if let Some(r) = from_them {
        a.facts.push(cx.date_fact(
            if last {
                "Last from them"
            } else {
                "First from them"
            },
            r,
        ));
    }
    if let Some(r) = from_me {
        a.facts.push(cx.date_fact(
            if last {
                "Last from you"
            } else {
                "First from you"
            },
            r,
        ));
    }
    if last {
        a.facts.push(cx.date_fact("First contact", first));
    } else {
        a.facts.push(cx.date_fact("Latest contact", latest));
    }
    let n_them = direct.iter().filter(|r| r.from_them).count();
    let n_me = direct.len() - n_them;
    a.facts
        .push(cx.fact("Messages", format!("{n_them} from them · {n_me} from you")));
    let mut items: Vec<&Row> = if last {
        direct.iter().rev().copied().take(5).collect()
    } else {
        direct.iter().copied().take(5).collect()
    };
    items.dedup_by_key(|r| r.rowid);
    a.items = items.into_iter().map(|r| r.item(None)).collect();
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.confidence = if t.loose || !a.candidates.is_empty() {
        AskConfidence::Medium
    } else {
        AskConfidence::High
    };
    a.search_query = Some(format!("{}{}", target_query(&t, Dir::Any), cx.range_ops()));
    a.followups = person_followups(&t, intent);
    let mut result = AskResult::new(AskResultKind::Item);
    result.date = Some(iso(when));
    result.count = Some(direct.len() as u64);
    a.result = Some(result);
    Ok(cx.finish(a))
}

// --------------------------------------------------------- relationship

/// Phrases that mark the end of a working relationship, with a weight
/// (2 = explicit, 1 = suggestive).
const END_PHRASES: &[(&str, i64)] = &[
    ("leaving off", 2),
    ("leave off", 2),
    ("parting ways", 2),
    ("part ways", 2),
    ("end our engagement", 2),
    ("ending our engagement", 2),
    ("end the engagement", 2),
    ("ending the engagement", 2),
    ("end our contract", 2),
    ("terminate our", 2),
    ("terminating our", 2),
    ("termination", 2),
    ("no longer need", 2),
    ("different direction", 2),
    ("in house", 2),
    ("wind down", 2),
    ("winding down", 2),
    ("let you go", 2),
    ("not renew", 2),
    ("not be renewing", 2),
    ("won t be renewing", 2),
    ("cancel our", 2),
    ("cancelling our", 2),
    ("canceling our", 2),
    ("last day", 2),
    ("offboarding", 2),
    ("final invoice", 1),
    ("hand off", 1),
    ("handoff", 1),
    ("handing off", 1),
    ("handover", 1),
    ("transition plan", 1),
    ("pause our", 1),
    ("pausing our", 1),
    ("moving on", 1),
];

struct Span<'r> {
    first: &'r Row,
    last: &'r Row,
    /// First/last message of the longest stretch of regular mail.
    regular: Option<(&'r Row, &'r Row)>,
    /// Handoff message and the sentence that marks it.
    handoff: Option<(&'r Row, String)>,
    unit: String,
    buckets: Vec<AskBucket>,
}

fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).expect("day 1")
}

fn months_between(a: NaiveDate, b: NaiveDate) -> i32 {
    (b.year() - a.year()) * 12 + b.month() as i32 - a.month() as i32
}

fn buckets(rows: &[&Row], from: NaiveDate, to: NaiveDate, off: i32) -> (String, Vec<AskBucket>) {
    let yearly = months_between(from, to) > 48;
    let mut out: Vec<AskBucket> = Vec::new();
    let mut d = if yearly {
        NaiveDate::from_ymd_opt(from.year(), 1, 1).expect("jan 1")
    } else {
        month_start(from)
    };
    while d <= to {
        out.push(AskBucket {
            start: day_ms(d, off),
            label: if yearly {
                d.year().to_string()
            } else {
                fmt_month(d)
            },
            from_them: 0,
            from_me: 0,
        });
        d = if yearly {
            NaiveDate::from_ymd_opt(d.year() + 1, 1, 1).expect("jan 1")
        } else {
            crate::dates::add_months(d, 1).expect("month")
        };
    }
    for r in rows {
        let ld = local_date(r.date, off);
        let i = if yearly {
            ld.year() - from.year()
        } else {
            months_between(month_start(from), ld)
        };
        if let Some(b) = usize::try_from(i).ok().and_then(|i| out.get_mut(i)) {
            if r.by_me {
                b.from_me += 1;
            } else {
                b.from_them += 1;
            }
        }
    }
    ((if yearly { "year" } else { "month" }).into(), out)
}

/// `who`: `target_fts` for the target, to restrict the end-phrase scan to
/// mail with them.
fn analyze<'r>(
    cx: &Cx,
    c: &Connection,
    rows: &'r [&'r Row],
    who: Option<&str>,
) -> Result<Span<'r>> {
    let first = rows[0];
    let last = rows[rows.len() - 1];
    let from = local_date(first.date, cx.off);
    let to = local_date(last.date, cx.off).max(if cx.now - last.date < 60 * DAY {
        cx.today
    } else {
        from
    });
    // Cadence is judged per month, even when the sparkline is yearly.
    let mut monthly: Vec<u32> = vec![0; (months_between(from, to) + 1).max(1) as usize];
    for r in rows {
        let i = months_between(month_start(from), local_date(r.date, cx.off));
        if let Some(x) = usize::try_from(i).ok().and_then(|i| monthly.get_mut(i)) {
            *x += 1;
        }
    }
    // Longest run of active months (≥2 messages), allowing one quiet month.
    let active: Vec<bool> = monthly.iter().map(|n| *n >= 2).collect();
    let mut best: Option<(usize, usize, u32)> = None; // (start, end, total)
    let mut i = 0;
    while i < active.len() {
        if !active[i] {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        let mut j = i + 1;
        while j < active.len() {
            if active[j] {
                end = j;
                j += 1;
            } else if j + 1 < active.len() && active[j + 1] {
                j += 1;
            } else {
                break;
            }
        }
        let total: u32 = monthly[start..=end].iter().sum();
        if end - start >= 2 && best.is_none_or(|b| total > b.2) {
            best = Some((start, end, total));
        }
        i = end + 1;
    }
    let regular = best.and_then(|(s, e, _)| {
        let m0 = crate::dates::add_months(month_start(from), s as u32)?;
        let m1 = crate::dates::add_months(month_start(from), e as u32 + 1)?;
        let in_run: Vec<&Row> = rows
            .iter()
            .copied()
            .filter(|r| (m0..m1).contains(&local_date(r.date, cx.off)))
            .collect();
        Some((*in_run.first()?, *in_run.last()?))
    });

    // Handoff: an end phrase in a message from either side, near the end
    // of the regular stretch (or anywhere after its start). Restricted to
    // mail with the target in the index itself: over years of a busy
    // mailbox, the phrases alone ("last day", "moving on") match hundreds
    // of messages (70 ms at 300k), and only theirs count.
    let set: HashMap<i64, &Row> = rows.iter().map(|r| (r.rowid, *r)).collect();
    let phrases = format!(
        "{{subject body}} : ({})",
        END_PHRASES
            .iter()
            .map(|(p, _)| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(" OR ")
    );
    let expr = match who {
        Some(who) => format!("({phrases}) AND ({who})"),
        None => phrases,
    };
    let mut stmt = c.prepare_cached("SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1 AND rowid >= ?2 AND rowid <= ?3")?;
    let hits: Vec<i64> = stmt
        .query_map(params![expr, first.rowid, last.rowid], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let anchor_end = regular.map_or(last.date, |r| r.1.date);
    let anchor_start = regular.map_or(first.date, |r| r.0.date);
    let mut handoff: Option<(i64, &Row, String)> = None;
    for id in hits {
        let Some(r) = set.get(&id).copied() else {
            continue;
        };
        if r.date < anchor_start {
            continue;
        }
        // Token match, like FTS ("we're" = "we re", "in-house" = "in house").
        let body = crate::text::split_quoted(&load_text(c, r.rowid)?).0;
        let found = END_PHRASES.iter().find_map(|(p, w)| {
            let quote =
                extract::quote_around(&body, p).or_else(|| extract::quote_around(&r.subject, p))?;
            Some((quote, *w))
        });
        let Some((quote, w)) = found else { continue };
        let days = (r.date - anchor_end).abs() / DAY;
        let score = w * 60 - days;
        if handoff.as_ref().is_none_or(|h| score > h.0) {
            handoff = Some((score, r, quote));
        }
    }
    let (unit, b) = buckets(rows, from, to, cx.off);
    Ok(Span {
        first,
        last,
        regular,
        handoff: handoff.map(|(_, r, q)| (r, q)),
        unit,
        buckets: b,
    })
}

fn relationship(cx: &mut Cx, who: &str, focus: Focus) -> Result<AskAnswer> {
    let (t, candidates) = match resolve_who(cx, who, AskIntent::Relationship)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    let (lo, hi) = (cx.lo, cx.hi);
    let rows = cx.store.read(|c| target_rows(cx, c, &t, lo, hi))?;
    let direct: Vec<&Row> = rows.iter().filter(|r| r.from_them || r.by_me).collect();
    cx.steps.push(format!(
        "Found {} between you and {}",
        plural(direct.len(), "message", "messages"),
        t.label
    ));
    if direct.is_empty() {
        let mut a = nothing_with(cx, AskIntent::Relationship, &t, "emails");
        a.candidates = candidates;
        return Ok(a);
    }
    let who = target_fts(&t);
    let span = cx.store.read(|c| analyze(cx, c, &direct, who.as_deref()))?;
    cx.steps.push("Monthly activity: a month with ≥2 messages is active; the longest active stretch (one quiet month allowed) is the regular period".into());
    cx.steps.push(format!("Scanned subject and body for {} end-of-engagement phrases (\u{201c}leaving off\u{201d}, \u{201c}parting ways\u{201d}, \u{201c}final invoice\u{201d}, …)", END_PHRASES.len()));

    let name = t.label.clone();
    let mut a = cx.answer(AskIntent::Relationship);
    let ongoing = cx.now - span.last.date < 45 * DAY;
    let reg_end = span
        .handoff
        .as_ref()
        .map(|h| h.0)
        .or(span.regular.map(|r| r.1));
    let ended = match (&span.handoff, span.regular) {
        (Some(_), _) => true,
        (None, Some((_, e))) => cx.now - e.date > 60 * DAY,
        (None, None) => !ongoing,
    };
    let start_row = span.regular.map_or(span.first, |r| r.0);
    let start_m = fmt_month(local_date(start_row.date, cx.off));
    a.headline = match focus {
        Focus::Span => match (span.regular, reg_end, ended) {
            (Some(_), Some(end), true) => {
                format!(
                    "You worked with {name} from {start_m} to {}",
                    fmt_month(local_date(end.date, cx.off))
                )
            }
            (Some(_), _, false) => {
                format!("You've worked with {name} since {start_m} (still active)")
            }
            (Some(_), _, _) => format!("Regular mail with {name} started in {start_m}"),
            (None, _, _) => format!(
                "You've emailed with {name} since {} — {} in all, never on a regular cadence",
                fmt_month(local_date(span.first.date, cx.off)),
                plural(direct.len(), "message", "messages")
            ),
        },
        Focus::End => match (&span.handoff, span.regular) {
            (Some((r, _)), _) => format!(
                "It looks like it ended on {}: \u{201c}{}\u{201d}",
                fmt_day(r.date, cx.off),
                clip(&r.subject_or_none(), 60)
            ),
            (None, Some((_, e))) if ended => format!(
                "Regular mail with {name} stopped after {}",
                fmt_day(e.date, cx.off)
            ),
            _ => format!(
                "It doesn't look like it has ended: you last emailed with {name} {}",
                relative(local_date(span.last.date, cx.off), cx.today)
            ),
        },
    };
    if let Some((_, quote)) = &span.handoff {
        a.detail = Some(format!("\u{201c}{quote}\u{201d}"));
    }
    let mut markers = vec![AskMarker {
        date: span.first.date,
        label: "First contact".into(),
        cite: Some(span.first.cite()),
    }];
    a.facts.push(cx.date_fact("First contact", span.first));
    if let Some((s, e)) = span.regular {
        a.facts.push(cx.date_fact("Regular from", s));
        if s.rowid != span.first.rowid {
            markers.push(AskMarker {
                date: s.date,
                label: "Regular from".into(),
                cite: Some(s.cite()),
            });
        }
        if span.handoff.is_none() && ended {
            a.facts.push(cx.date_fact("Regular until", e));
            markers.push(AskMarker {
                date: e.date,
                label: "Regular until".into(),
                cite: Some(e.cite()),
            });
        }
    }
    if let Some((r, _)) = &span.handoff {
        a.facts.push(cx.date_fact("Handoff", r));
        markers.push(AskMarker {
            date: r.date,
            label: "Handoff".into(),
            cite: Some(r.cite()),
        });
        let after: Vec<&&Row> = direct.iter().filter(|x| x.date > r.date).collect();
        if !after.is_empty() {
            a.facts.push(cx.fact(
                "Since then",
                format!(
                    "{} · last {}",
                    plural(after.len(), "message", "messages"),
                    fmt_day(span.last.date, cx.off)
                ),
            ));
        }
    }
    a.facts.push(cx.date_fact("Last contact", span.last));
    if span.last.rowid != span.first.rowid {
        markers.push(AskMarker {
            date: span.last.date,
            label: "Last contact".into(),
            cite: Some(span.last.cite()),
        });
    }
    let n_them = direct.iter().filter(|r| r.from_them).count();
    a.facts.push(cx.fact(
        "Messages",
        format!("{n_them} from them · {} from you", direct.len() - n_them),
    ));
    a.timeline = Some(AskTimeline {
        unit: span.unit.clone(),
        buckets: span.buckets.clone(),
        markers,
    });

    let mut items: Vec<AskItem> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |r: &Row, note: &str| {
        if seen.insert(r.rowid) {
            items.push(r.item(Some(note.into())));
        }
    };
    if let Some((r, _)) = &span.handoff {
        push(r, "Handoff");
    }
    if let Some((s, e)) = span.regular {
        push(s, "Regular from");
        push(e, "Last in regular period");
    }
    push(span.last, "Last contact");
    push(span.first, "First contact");
    a.items = items;
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.confidence = if t.loose && !a.candidates.is_empty() {
        AskConfidence::Low
    } else {
        AskConfidence::Medium
    };
    a.search_query = Some(format!("{}{}", target_query(&t, Dir::Any), cx.range_ops()));
    a.followups = person_followups(&t, AskIntent::Relationship);
    if focus != Focus::End {
        a.followups.insert(
            0,
            suggest("When did it end?", "When did they let us go".into()),
        );
    }
    Ok(cx.finish(a))
}

// ----------------------------------------------------------- latest item

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Any => "email",
        Kind::Invoice => "invoice",
        Kind::Receipt => "receipt",
        Kind::Contract => "contract",
        Kind::Pdf => "PDF",
        Kind::Attachment => "attachment",
        Kind::Spreadsheet => "spreadsheet",
        Kind::Image => "image",
        Kind::Doc => "document",
    }
}

fn kind_plural(k: Kind) -> &'static str {
    match k {
        Kind::Any => "emails",
        Kind::Invoice => "invoices",
        Kind::Receipt => "receipts",
        Kind::Contract => "contracts",
        Kind::Pdf => "PDFs",
        Kind::Attachment => "attachments",
        Kind::Spreadsheet => "spreadsheets",
        Kind::Image => "images",
        Kind::Doc => "documents",
    }
}

fn kind_flag(k: Kind) -> Option<i64> {
    match k {
        Kind::Pdf => Some(F_PDF),
        Kind::Attachment => Some(F_ATTACH),
        Kind::Spreadsheet => Some(F_SHEET),
        Kind::Image => Some(F_IMAGE),
        Kind::Doc => Some(F_DOC),
        _ => None,
    }
}

/// FTS expression for keyword kinds (subject, body, attachment names).
fn kind_fts(k: Kind) -> Option<&'static str> {
    match k {
        Kind::Invoice => Some("{subject body filenames} : (invoice* OR \"amount due\" OR \"payment due\" OR \"billing statement\")"),
        Kind::Receipt => Some("{subject body filenames} : (receipt* OR \"order confirmation\" OR \"your order\" OR \"payment received\" OR \"thanks for your payment\" OR \"thank you for your payment\")"),
        Kind::Contract => Some("{subject body filenames} : (contract* OR agreement* OR \"statement of work\" OR sow OR docusign OR nda OR msa OR proposal*)"),
        _ => None,
    }
}

fn kind_query(k: Kind) -> &'static str {
    match k {
        Kind::Any => "",
        Kind::Invoice => " invoice",
        Kind::Receipt => " receipt",
        Kind::Contract => " contract",
        Kind::Pdf => " has:pdf",
        Kind::Attachment => " has:attachment",
        Kind::Spreadsheet => " has:spreadsheet",
        Kind::Image => " has:image",
        Kind::Doc => " has:doc",
    }
}

/// Filter rows to a kind (flags, or an FTS keyword match over their span).
fn filter_kind<'r>(c: &Connection, rows: Vec<&'r Row>, k: Kind) -> Result<Vec<&'r Row>> {
    if let Some(f) = kind_flag(k) {
        return Ok(rows.into_iter().filter(|r| r.flags & f != 0).collect());
    }
    let Some(expr) = kind_fts(k) else {
        return Ok(rows);
    };
    let (Some(lo), Some(hi)) = (
        rows.iter().map(|r| r.rowid).min(),
        rows.iter().map(|r| r.rowid).max(),
    ) else {
        return Ok(rows);
    };
    let hits: HashSet<i64> = c
        .prepare_cached("SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1 AND rowid >= ?2 AND rowid <= ?3")?
        .query_map(params![expr, lo, hi], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter(|r| hits.contains(&r.rowid))
        .collect())
}

fn attachment_names(c: &Connection, rowid: i64) -> Result<Vec<String>> {
    Ok(c.prepare_cached(
        "SELECT filename FROM attachments WHERE message_rowid = ?1 AND inline = 0 ORDER BY ord",
    )?
    .query_map([rowid], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

fn latest_item(cx: &mut Cx, who: Option<&str>, kind: Kind) -> Result<AskAnswer> {
    let (lo, hi) = (cx.lo, cx.hi);
    let (target, candidates, rows): (Option<Target>, Vec<AskSuggestion>, Vec<Row>) = match who {
        Some(w) => {
            let (t, cands) = match resolve_who(cx, w, AskIntent::LatestItem)? {
                Resolved::Found(t, c) => (t, c),
                Resolved::Answer(a) => return Ok(*a),
            };
            let rows = cx.store.read(|c| target_rows(cx, c, &t, lo, hi))?;
            let rows: Vec<Row> = rows.into_iter().filter(|r| r.from_them).collect();
            (Some(t), cands, rows)
        }
        None => {
            // Anyone: newest non-sent messages of that kind.
            let rows = cx.store.read(|c| {
                let (rlo, rhi) = rowid_bounds(lo, hi);
                let own = own_addresses(c)?;
                let mut out = Vec::new();
                if let Some(expr) = kind_fts(kind) {
                    let mut stmt = c.prepare_cached(&format!(
                        "SELECT {ROW_COLS} FROM messages_fts f JOIN messages m ON m.rowid = f.rowid JOIN threads t ON t.rowid = m.thread_rowid
                         WHERE messages_fts MATCH ?1 AND f.rowid >= ?2 AND f.rowid < ?3 AND m.flags & ?4 = 0 ORDER BY f.rowid DESC LIMIT 200"
                    ))?;
                    for r in stmt.query_map(params![expr, rlo, rhi, HIDDEN | F_SENT], read_row)? {
                        out.push(r?);
                    }
                } else {
                    // A flag kind, or any message: 0 & 0 = 0 matches everything.
                    let f = kind_flag(kind).unwrap_or(0);
                    let mut stmt = c.prepare_cached(&format!(
                        "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
                         WHERE m.rowid >= ?1 AND m.rowid < ?2 AND m.flags & ?3 = ?3 AND m.flags & ?4 = 0 ORDER BY m.rowid DESC LIMIT 200"
                    ))?;
                    for r in stmt.query_map(params![rlo, rhi, f, HIDDEN | F_SENT], read_row)? {
                        out.push(r?);
                    }
                }
                Ok(out.into_iter().filter(|r| !own.contains(&r.from_email)).map(|mut r| {
                    r.from_them = true;
                    r
                }).collect::<Vec<_>>())
            })?;
            let rows: Vec<Row> = rows
                .into_iter()
                .filter(|r| cx.in_scope(&r.account_id))
                .collect();
            (None, vec![], rows)
        }
    };
    let what = kind_name(kind);
    let label = target
        .as_ref()
        .map_or_else(|| "anyone".to_string(), |t| t.label.clone());
    let refs: Vec<&Row> = rows.iter().collect();
    let mut matching = cx.store.read(|c| filter_kind(c, refs, kind))?;
    matching.sort_by_key(|r| std::cmp::Reverse(r.rowid));
    cx.steps.push(format!(
        "{} from {label}{}, {} matching \u{201c}{what}\u{201d}{}",
        plural(rows.len(), "message", "messages"),
        cx.range_phrase(),
        matching.len(),
        match (kind_flag(kind), kind_fts(kind)) {
            (Some(_), _) => " (attachment type)",
            (None, Some(_)) => " (subject, body or file name)",
            _ => "",
        }
    ));
    let mut a = cx.answer(AskIntent::LatestItem);
    a.search_query = Some(
        format!(
            "{}{}{}",
            target
                .as_ref()
                .map_or(String::new(), |t| target_query(t, Dir::FromThem)),
            kind_query(kind),
            cx.range_ops()
        )
        .trim()
        .to_string(),
    );
    a.person = target.as_ref().map(person_of);
    a.candidates = candidates;
    let Some(top) = matching.first().copied() else {
        a.headline = format!(
            "No {what} from {label}{} in your local mail",
            cx.range_phrase()
        );
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    };
    let names = cx.store.read(|c| attachment_names(c, top.rowid))?;
    let from = top
        .from_name
        .clone()
        .unwrap_or_else(|| top.from_email.clone());
    a.headline = if target.is_some() {
        format!(
            "The latest {what} from {label} is \u{201c}{}\u{201d} ({})",
            clip(&top.subject_or_none(), 70),
            fmt_day(top.date, cx.off)
        )
    } else {
        format!(
            "The latest {what} is \u{201c}{}\u{201d} from {from} ({})",
            clip(&top.subject_or_none(), 70),
            fmt_day(top.date, cx.off)
        )
    };
    if !names.is_empty() {
        a.detail = Some(format!("Attached: {}", names.join(", ")));
    }
    a.facts.push(cx.date_fact(&format!("Latest {what}"), top));
    a.facts.push(cx.fact(
        &format!("{} in total", capitalize(kind_plural(kind))),
        matching.len().to_string(),
    ));
    if let Some(first) = matching.last() {
        if first.rowid != top.rowid {
            a.facts.push(cx.date_fact(&format!("First {what}"), first));
        }
    }
    let mut items = Vec::new();
    for r in matching.iter().take(MAX_ITEMS) {
        let names = cx.store.read(|c| attachment_names(c, r.rowid))?;
        items.push(r.item((!names.is_empty()).then(|| clip(&names.join(", "), 80))));
    }
    a.items = items;
    a.confidence = if kind_fts(kind).is_some() || target.as_ref().is_some_and(|t| t.loose) {
        AskConfidence::Medium
    } else {
        AskConfidence::High
    };
    if let Some(t) = &target {
        a.followups = person_followups(t, AskIntent::LatestItem);
        if kind != Kind::Any {
            let key = t
                .emails
                .first()
                .or(t.domains.first())
                .cloned()
                .unwrap_or_default();
            a.followups.insert(
                0,
                suggest(
                    &format!("How many {}?", kind_plural(kind)),
                    format!("How many {} from {key}", kind_plural(kind)),
                ),
            );
        }
    }
    Ok(cx.finish(a))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------- counts

fn count(cx: &mut Cx, who: &str, dir: Dir, kind: Kind) -> Result<AskAnswer> {
    let (t, candidates) = match resolve_who(cx, who, AskIntent::Count)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    let (lo, hi) = (cx.lo, cx.hi);
    let rows = cx.store.read(|c| target_rows(cx, c, &t, lo, hi))?;
    let side: Vec<&Row> = rows
        .iter()
        .filter(|r| match dir {
            Dir::FromThem => r.from_them,
            Dir::ToThem => r.by_me,
            Dir::Any => r.from_them || r.by_me,
        })
        .collect();
    let side = cx.store.read(|c| filter_kind(c, side, kind))?;
    let n = side.len();
    let threads: HashSet<i64> = side.iter().map(|r| r.thread_rowid).collect();
    let name = t.label.clone();
    let range = cx.range_phrase();
    let noun = |n: usize| {
        if n == 1 {
            kind_name(kind).to_string()
        } else {
            kind_plural(kind).to_string()
        }
    };
    cx.steps.push(format!(
        "Counted every stored message {}{}{} — exact, not estimated",
        match dir {
            Dir::FromThem => format!("from {name}"),
            Dir::ToThem => format!("you sent to {name}"),
            Dir::Any => format!("between you and {name}"),
        },
        if kind == Kind::Any {
            String::new()
        } else {
            format!(" matching \u{201c}{}\u{201d}", kind_name(kind))
        },
        range
    ));
    let mut a = cx.answer(AskIntent::Count);
    a.headline = match dir {
        Dir::FromThem => format!("{name} sent you {n} {}{range}", noun(n)),
        Dir::ToThem => format!("You sent {name} {n} {}{range}", noun(n)),
        Dir::Any => format!("{n} {} between you and {name}{range}", noun(n)),
    };
    if n > 0 {
        a.detail = Some(format!("In {}", plural(threads.len(), "thread", "threads")));
    }
    if dir == Dir::Any && n > 0 {
        let them = side.iter().filter(|r| r.from_them).count();
        a.facts.push(cx.fact("From them", them.to_string()));
        a.facts.push(cx.fact("From you", (n - them).to_string()));
    }
    if let (Some(f), Some(l)) = (side.first(), side.last()) {
        a.facts.push(cx.date_fact("First", f));
        if l.rowid != f.rowid {
            a.facts.push(cx.date_fact("Latest", l));
        }
        let from = cx
            .lo
            .map_or_else(|| local_date(f.date, cx.off), |x| local_date(x, cx.off));
        let to = cx
            .hi
            .map_or(cx.today, |x| local_date(x - 1, cx.off))
            .min(cx.today);
        let (unit, b) = buckets(&side, from, to.max(from), cx.off);
        if b.len() > 1 {
            // A few busiest periods as facts; the sparkline has the rest.
            let mut busiest: Vec<&AskBucket> =
                b.iter().filter(|x| x.from_them + x.from_me > 0).collect();
            busiest.sort_by_key(|x| std::cmp::Reverse(x.from_them + x.from_me));
            if let Some(top) = busiest.first() {
                a.facts.push(cx.fact(
                    &format!("Busiest {unit}"),
                    format!("{} ({})", top.label, top.from_them + top.from_me),
                ));
            }
            a.timeline = Some(AskTimeline {
                unit,
                buckets: b,
                markers: vec![],
            });
        }
    }
    a.items = side
        .iter()
        .rev()
        .take(MAX_ITEMS)
        .map(|r| r.item(None))
        .collect();
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.confidence = if t.loose || kind_fts(kind).is_some() {
        AskConfidence::Medium
    } else {
        AskConfidence::High
    };
    a.search_query = Some(format!(
        "{}{}{}",
        target_query(&t, dir),
        kind_query(kind),
        cx.range_ops()
    ));
    a.followups = person_followups(&t, AskIntent::Count);
    Ok(cx.finish(a))
}

/// FTS terms for a topic: every word, quoted (stopwords dropped).
fn topic_words(topic: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "my", "our", "your", "their", "his", "her", "this", "that", "next",
        "upcoming", "about", "of", "for", "to", "in", "on", "with", "and",
    ];
    crate::text::tokens(topic)
        .into_iter()
        .map(|t| t.2)
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect()
}

fn count_topic(cx: &mut Cx, topic: &str) -> Result<AskAnswer> {
    let words = topic_words(topic);
    let mut a = cx.answer(AskIntent::Count);
    let clean = words.join(" ");
    a.search_query = Some(format!("{clean}{}", cx.range_ops()));
    if words.is_empty() {
        a.headline = "What topic should I count?".into();
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    }
    let expr = words
        .iter()
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" AND ");
    let (rlo, rhi) = rowid_bounds(cx.lo, cx.hi);
    let scope = cx.scope.clone();
    let (n, threads) = cx.store.read(|c| {
        let mut sql = String::from(
            "SELECT count(*), count(DISTINCT m.thread_rowid) FROM messages_fts f JOIN messages m ON m.rowid = f.rowid
             WHERE messages_fts MATCH ? AND f.rowid >= ? AND f.rowid < ? AND m.flags & ? = 0",
        );
        let mut ps: Vec<Value> = vec![Value::Text(expr.clone()), Value::Integer(rlo), Value::Integer(rhi), Value::Integer(HIDDEN)];
        if let Some(s) = &scope {
            sql.push_str(&format!(" AND m.account_id IN ({})", vec!["?"; s.len()].join(",")));
            ps.extend(s.iter().map(|x| Value::Text(x.clone())));
        }
        Ok(c.prepare(&sql)?.query_row(params_from_iter(ps), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?)
    })?;
    cx.steps.push(format!(
        "Counted every stored message containing {}{} — exact",
        words
            .iter()
            .map(|w| format!("\u{201c}{w}\u{201d}"))
            .collect::<Vec<_>>()
            .join(" and "),
        cx.range_phrase()
    ));
    a.headline = format!(
        "{} mention \u{201c}{clean}\u{201d}{}",
        plural(n as usize, "email", "emails"),
        cx.range_phrase()
    );
    a.detail = Some(format!(
        "In {}",
        plural(threads as usize, "thread", "threads")
    ));
    a.items = search_items(cx, &format!("{clean}{}", cx.range_ops()), MAX_ITEMS)?;
    a.confidence = AskConfidence::High;
    a.followups.push(suggest(
        "Who wrote about it?",
        format!("Who emailed me about {clean}"),
    ));
    Ok(cx.finish(a))
}

// ----------------------------------------------------------------- spend

/// Spending categories answered from bookings rather than one merchant.
fn spend_category(merchant: &str) -> Option<(FactKind, &'static str)> {
    match merchant {
        "flights" | "flight" | "airfare" | "air travel" | "plane tickets" | "airline tickets"
        | "airlines" | "vuelos" | "pasajes" => Some((FactKind::Flight, "flights")),
        "hotels" | "hotel" | "lodging" | "hotel stays" | "accommodation" | "accommodations"
        | "stays" | "hoteles" | "alojamiento" => Some((FactKind::Stay, "hotels")),
        "tickets" | "events" | "concerts" | "shows" | "entradas" => {
            Some((FactKind::Booking, "tickets"))
        }
        _ => None,
    }
}

/// One email's contribution to a spend sum.
struct Added<'r> {
    row: &'r Row,
    value: f64,
    currency: String,
    line: String,
    reference: Option<String>,
}

fn spend(cx: &mut Cx, merchant: &str) -> Result<AskAnswer> {
    if let Some((kind, label)) = spend_category(merchant.trim()) {
        return facts::spend_category(cx, kind, label);
    }
    let (t, candidates) = match resolve_who(cx, merchant, AskIntent::Spend)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    // Receipts, promos and refunds come from different addresses of one
    // company: sum the domain, not one sender.
    let t = match cx.store.read(|c| {
        let own = own_addresses(c)?;
        resolve::widen(c, &t, merchant, &own, PUBLIC_MAIL)
    })? {
        Some(w) => {
            cx.steps.push(format!(
                "Widened to every sender at {}",
                w.domains.join(", ")
            ));
            w
        }
        None => t,
    };
    // Another address at the same company is already in the sum.
    let candidates: Vec<AskSuggestion> = candidates
        .into_iter()
        .filter(|c| {
            let at = c.label.rsplit('@').next().unwrap_or("").trim_end_matches('>');
            !t.domains.iter().any(|d| at == d || at.ends_with(&format!(".{d}")))
        })
        .collect();
    let (lo, hi) = (cx.lo, cx.hi);
    let rows = cx.store.read(|c| target_rows(cx, c, &t, lo, hi))?;
    let from: Vec<&Row> = rows
        .iter()
        .filter(|r| r.from_them)
        .rev()
        .take(MAX_RECEIPTS)
        .collect();
    let mut found: Vec<Added> = Vec::new();
    let mut unpaid: Vec<Added> = Vec::new();
    let mut skipped = 0usize;
    let mut pending = 0usize;
    let mut from_facts = 0usize;
    cx.store.read(|c| {
        let ids: Vec<i64> = from.iter().map(|r| r.rowid).collect();
        let mut spent = facts::spent_many(c, &ids)?;
        for r in &from {
            // The extracted amount first (receipts, orders, bookings, paid
            // bills, schema.org prices), else the receipt's total line.
            if let Some(s) = spent.remove(&r.rowid) {
                let added = Added {
                    row: r,
                    value: s.value,
                    currency: s.currency,
                    line: s.source,
                    reference: s.reference,
                };
                from_facts += 1;
                if s.unpaid {
                    unpaid.push(added);
                } else {
                    found.push(added);
                }
                continue;
            }
            let body = load_text(c, r.rowid)?;
            let text = if body.trim().is_empty() {
                pending += 1;
                r.snippet.clone()
            } else {
                crate::text::split_quoted(&body).0
            };
            match extract::receipt_total(&text) {
                Some(mut total) => {
                    let s = r.subject.to_lowercase();
                    if (s.contains("refund") || s.contains("credit")) && total.amount.value > 0.0 {
                        total.amount.value = -total.amount.value;
                    }
                    found.push(Added {
                        row: r,
                        value: total.amount.value,
                        currency: total.amount.currency.to_string(),
                        line: total.line,
                        reference: None,
                    });
                }
                None => skipped += 1,
            }
        }
        Ok(())
    })?;
    cx.steps.push(format!(
        "Read {} from {}{}: the amount extracted from each receipt, order or booking ({}), else its \u{201c}Total\u{201d} line (grand/order total, amount charged or paid, else total)",
        plural(from.len(), "message", "messages"),
        t.label,
        cx.range_phrase(),
        plural(from_facts, "from extracted facts", "from extracted facts")
    ));
    // One order counts once: its confirmation and receipt (or shipping
    // notice) carry the same order number. Refunds count separately.
    let mut duplicates = 0usize;
    {
        let mut seen: HashSet<(String, bool)> = HashSet::new();
        // Oldest first, so the first email about an order is the one kept.
        found.sort_by_key(|x| x.row.rowid);
        found.retain(|x| match &x.reference {
            Some(r) if seen.contains(&(r.clone(), x.value < 0.0)) => {
                duplicates += 1;
                false
            }
            Some(r) => {
                seen.insert((r.clone(), x.value < 0.0));
                true
            }
            None => true,
        });
        found.sort_by_key(|x| std::cmp::Reverse(x.row.rowid));
    }
    if duplicates > 0 {
        cx.steps.push(format!(
            "Counted each order once: left out {} repeating an order number already added",
            plural(duplicates, "email", "emails")
        ));
    }
    // Only invoices, none marked paid: sum what was billed, and say so.
    let billed_only = found.is_empty() && !unpaid.is_empty();
    let unpaid_count = unpaid.len();
    if billed_only {
        found = unpaid;
        cx.steps.push(
            "No receipts, only invoices: summed the amounts billed (not confirmed paid)".into(),
        );
    } else if unpaid_count > 0 {
        cx.steps.push(format!(
            "Left out {} still due (not money spent yet)",
            plural(unpaid_count, "invoice", "invoices")
        ));
    }
    let mut sums: Vec<(String, f64, usize)> = Vec::new();
    for x in &found {
        match sums.iter_mut().find(|s| s.0 == x.currency) {
            Some(s) => {
                s.1 += x.value;
                s.2 += 1;
            }
            None => sums.push((x.currency.clone(), x.value, 1)),
        }
    }
    sums.sort_by_key(|s| std::cmp::Reverse(s.2));
    let mut a = cx.answer(AskIntent::Spend);
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.search_query = Some(format!(
        "{}{}",
        target_query(&t, Dir::FromThem),
        cx.range_ops()
    ));
    if found.is_empty() {
        a.headline = if from.is_empty() {
            format!(
                "No mail from {}{} in your local mail",
                t.label,
                cx.range_phrase()
            )
        } else {
            format!(
                "None of the {} from {}{} has a total I can read",
                plural(from.len(), "email", "emails"),
                t.label,
                cx.range_phrase()
            )
        };
        a.confidence = AskConfidence::None;
        a.items = from.iter().take(MAX_ITEMS).map(|r| r.item(None)).collect();
        return Ok(cx.finish(a));
    }
    let total_text = sums
        .iter()
        .map(|(c, v, _)| money(*v, c))
        .collect::<Vec<_>>()
        .join(" + ");
    // "Uber Receipts" → "Uber".
    let short = crate::structured::clean_org(
        &t.name
            .clone()
            .unwrap_or_else(|| t.label.split(" (").next().unwrap_or(&t.label).to_string()),
    );
    let noun = if billed_only {
        ("invoice", "invoices")
    } else {
        ("receipt", "receipts")
    };
    a.headline = if billed_only {
        format!(
            "{short} billed you {total_text}{} across {}",
            cx.range_phrase(),
            plural(found.len(), noun.0, noun.1)
        )
    } else {
        format!(
            "{total_text} across {} {short} {}{}",
            found.len(),
            if found.len() == 1 { noun.0 } else { noun.1 },
            cx.range_phrase()
        )
    };
    a.detail = Some(
        if billed_only {
            "Summed from the amount due on each invoice below; none is marked paid in your mail."
        } else {
            "Summed from each email below (open the list to check every line); nothing is estimated."
        }
        .into(),
    );
    a.sum = Some(AskSum {
        totals: sums
            .iter()
            .map(|(c, v, n)| AskTotal {
                value: *v,
                currency: c.clone(),
                count: *n,
            })
            .collect(),
        basis: if billed_only {
            "The amount due on each invoice".into()
        } else {
            "The total on each receipt, order or booking".into()
        },
        duplicates,
        skipped,
        unpaid: if billed_only { 0 } else { unpaid_count },
    });
    for (cur, v, n) in &sums {
        if sums.len() > 1 {
            a.facts.push(cx.fact(
                &format!("Total {cur}"),
                format!(
                    "{} over {}",
                    money(*v, cur),
                    plural(*n, "receipt", "receipts")
                ),
            ));
        }
    }
    if let Some(x) = found.iter().max_by(|a, b| a.value.total_cmp(&b.value)) {
        let mut f = cx.date_fact("Largest", x.row);
        f.value = format!(
            "{} · {}",
            money(x.value, &x.currency),
            fmt_day(x.row.date, cx.off)
        );
        a.facts.push(f);
    }
    if sums.len() == 1 && found.len() > 1 {
        a.facts
            .push(cx.fact("Average", money(sums[0].1 / found.len() as f64, &sums[0].0)));
    }
    // Emails without an amount are in `sum.skipped`, shown with the total.
    if pending > 0 {
        a.facts.push(cx.fact(
            "Headers only",
            format!(
                "{} not downloaded in full; read from the preview",
                plural(pending, "email", "emails")
            ),
        ));
    }
    // Every line that was added, newest first, so the sum can be audited.
    a.items = found
        .iter()
        .map(|x| {
            let mut it = x.row.item(Some(x.line.clone()));
            it.amount = Some(AskAmount {
                value: x.value,
                currency: x.currency.clone(),
                source: x.line.clone(),
            });
            it
        })
        .collect();
    let dates: Vec<&Row> = found.iter().map(|x| x.row).collect();
    let from_d = cx.lo.map_or_else(
        || local_date(dates.iter().map(|r| r.date).min().unwrap_or(cx.now), cx.off),
        |x| local_date(x, cx.off),
    );
    let to_d = cx
        .hi
        .map_or(cx.today, |x| local_date(x - 1, cx.off))
        .min(cx.today);
    let (unit, b) = buckets(&dates, from_d, to_d.max(from_d), cx.off);
    if b.len() > 1 {
        a.timeline = Some(AskTimeline {
            unit,
            buckets: b,
            markers: vec![],
        });
    }
    a.coverage = cx.coverage("receipts")?;
    a.confidence = if skipped == 0 && !t.loose && !billed_only && a.coverage.is_none() {
        AskConfidence::High
    } else {
        AskConfidence::Medium
    };
    let key = t
        .domains
        .first()
        .or(t.emails.first())
        .cloned()
        .unwrap_or_default();
    a.followups = vec![
        suggest(
            "This year",
            format!("How much did I spend on {key} this year"),
        ),
        suggest(
            "Last year",
            format!("How much did I spend on {key} last year"),
        ),
        suggest("Latest receipt", format!("Latest receipt from {key}")),
    ];
    Ok(cx.finish(a))
}

// ------------------------------------------------------------ who / top

fn strip_marks(html: &str) -> String {
    let s = html.replace("<mark>", "").replace("</mark>", "");
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn search(cx: &Cx, query: &str, limit: u32) -> Result<Vec<crate::types::SearchHit>> {
    let resp = cx.store.search(&SearchRequest {
        query: query.to_string(),
        account_id: None,
        account_ids: cx.scope.clone(),
        limit,
    })?;
    Ok(resp.hits)
}

fn search_items(cx: &mut Cx, query: &str, n: usize) -> Result<Vec<AskItem>> {
    let hits = search(cx, query, n as u32)?;
    cx.steps.push(format!(
        "Searched \u{201c}{}\u{201d}: {}",
        query.trim(),
        plural(hits.len(), "thread", "threads")
    ));
    Ok(hits
        .into_iter()
        .map(|h| AskItem {
            account_id: h.account_id,
            thread_id: h.thread_id,
            message_id: h.message_id,
            subject: h.subject,
            from: h.from,
            date: h.date,
            snippet: clip(&strip_marks(&h.snippet_html), 180),
            note: None,
            amount: None,
            sent: h.label_ids.iter().any(|l| l == "SENT"),
        })
        .collect())
}

fn who_about(cx: &mut Cx, topic: &str) -> Result<AskAnswer> {
    let words = topic_words(topic);
    let clean = if words.is_empty() {
        topic.to_string()
    } else {
        words.join(" ")
    };
    let query = format!("{clean}{}", cx.range_ops());
    let hits = search(cx, &query, 100)?;
    cx.steps.push(format!(
        "Searched \u{201c}{query}\u{201d}: {}",
        plural(hits.len(), "thread", "threads")
    ));
    let own: HashSet<String> = cx.store.read(own_addresses)?;
    let mut people: Vec<(Address, usize, &crate::types::SearchHit)> = Vec::new();
    for h in &hits {
        let e = h.from.email.to_lowercase();
        if own.contains(&e) {
            continue;
        }
        match people
            .iter_mut()
            .find(|p| p.0.email.eq_ignore_ascii_case(&e))
        {
            Some(p) => {
                p.1 += 1;
                if h.date > p.2.date {
                    p.2 = h;
                }
            }
            None => people.push((h.from.clone(), 1, h)),
        }
    }
    people.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.date.cmp(&a.2.date)));
    let mut a = cx.answer(AskIntent::WhoAbout);
    a.search_query = Some(query.clone());
    if people.is_empty() {
        a.headline = format!(
            "Nobody emailed you about \u{201c}{clean}\u{201d}{}",
            cx.range_phrase()
        );
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    }
    let names: Vec<String> = people
        .iter()
        .take(3)
        .map(|p| {
            p.0.name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| p.0.email.clone())
        })
        .collect();
    let list = match names.len() {
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => format!(
            "{}, {} and {}",
            names[0],
            names[1],
            if people.len() > 3 {
                format!("{} more", people.len() - 2)
            } else {
                names[2].clone()
            }
        ),
    };
    a.headline = if people.len() == 1 {
        format!(
            "{list} emailed you about \u{201c}{clean}\u{201d}{}",
            cx.range_phrase()
        )
    } else {
        format!(
            "{} people emailed you about \u{201c}{clean}\u{201d}{}: {list}",
            people.len(),
            cx.range_phrase()
        )
    };
    for (addr, n, h) in people.iter().take(8) {
        a.facts.push(AskFact {
            label: addr
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| addr.email.clone()),
            value: format!(
                "{} · latest {}",
                plural(*n, "thread", "threads"),
                fmt_day(h.date, cx.off)
            ),
            date: Some(h.date),
            cite: Some(AskCite {
                account_id: h.account_id.clone(),
                thread_id: h.thread_id.clone(),
                message_id: h.message_id.clone(),
            }),
        });
    }
    a.items = hits
        .iter()
        .filter(|h| !own.contains(&h.from.email.to_lowercase()))
        .take(MAX_ITEMS)
        .map(|h| AskItem {
            account_id: h.account_id.clone(),
            thread_id: h.thread_id.clone(),
            message_id: h.message_id.clone(),
            subject: h.subject.clone(),
            from: h.from.clone(),
            date: h.date,
            snippet: clip(&strip_marks(&h.snippet_html), 180),
            note: None,
            amount: None,
            sent: false,
        })
        .collect();
    a.confidence = AskConfidence::Medium;
    if let Some(p) = people.first() {
        a.followups.push(suggest(
            &format!("Who is {}?", names[0]),
            format!("Who is {}", p.0.email),
        ));
    }
    a.followups.push(suggest(
        "How many emails?",
        format!("How many emails about {clean}"),
    ));
    a.followups
        .push(suggest("When is it?", format!("When is {clean}")));
    Ok(cx.finish(a))
}

fn top_senders(cx: &mut Cx) -> Result<AskAnswer> {
    let lo = cx.lo.unwrap_or(cx.now - 365 * DAY);
    let hi = cx.hi;
    let (rlo, rhi) = rowid_bounds(Some(lo), hi);
    let scope = cx.scope.clone();
    let rows: Vec<(String, Option<String>, i64, i64)> = cx.store.read(|c| {
        let own = own_addresses(c)?;
        let mut sql = String::from(
            "SELECT lower(from_email), max(from_name), count(*), max(rowid) FROM messages
             WHERE rowid >= ? AND rowid < ? AND flags & ? = 0",
        );
        let mut ps: Vec<Value> = vec![
            Value::Integer(rlo),
            Value::Integer(rhi),
            Value::Integer(HIDDEN | F_SENT | F_NEWSLETTER),
        ];
        if let Some(s) = &scope {
            sql.push_str(&format!(
                " AND account_id IN ({})",
                vec!["?"; s.len()].join(",")
            ));
            ps.extend(s.iter().map(|x| Value::Text(x.clone())));
        }
        sql.push_str(" GROUP BY 1 ORDER BY 3 DESC LIMIT 40");
        let out: Vec<(String, Option<String>, i64, i64)> = c
            .prepare(&sql)?
            .query_map(params_from_iter(ps), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(out
            .into_iter()
            .filter(|r| !own.contains(&r.0) && !automated(&r.0))
            .take(10)
            .collect())
    })?;
    let period = if cx.q.range_text.is_some() {
        cx.range_phrase()
    } else {
        " in the past year".to_string()
    };
    cx.steps.push(format!("Counted messages per sender{period}, excluding newsletters, automated senders and your own mail"));
    let mut a = cx.answer(AskIntent::TopSenders);
    let Some(top) = rows.first() else {
        a.headline = format!("No personal mail{period}");
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    };
    let name = |r: &(String, Option<String>, i64, i64)| {
        r.1.clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| r.0.clone())
    };
    a.headline = format!(
        "{} emails you the most: {}{period}",
        name(top),
        plural(top.2 as usize, "message", "messages")
    );
    for r in &rows {
        a.facts.push(cx.fact(&name(r), r.2.to_string()));
    }
    let latest: Vec<i64> = rows.iter().take(MAX_ITEMS).map(|r| r.3).collect();
    let items = cx.store.read(|c| {
        let mut out = Vec::new();
        let mut stmt = c.prepare_cached(&format!("SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid WHERE m.rowid = ?1"))?;
        for id in &latest {
            if let Some(r) = stmt.query_row([id], read_row).optional()? {
                out.push(r.item(Some("Latest from them".into())));
            }
        }
        Ok(out)
    })?;
    a.items = items;
    a.followups = rows
        .iter()
        .take(3)
        .map(|r| suggest(&format!("About {}", name(r)), format!("Who is {}", r.0)))
        .collect();
    Ok(cx.finish(a))
}

/// Senders that are machines, not people (for "who" and open-loop answers).
pub(crate) fn automated(email: &str) -> bool {
    let local = email.split('@').next().unwrap_or("");
    [
        "noreply",
        "no-reply",
        "no_reply",
        "donotreply",
        "do-not-reply",
        "notification",
        "notifications",
        "mailer-daemon",
        "postmaster",
        "bounce",
        "alerts",
        "alert",
        "newsletter",
        "news",
        "updates",
        "digest",
        "automated",
        "receipts",
        "billing",
        "calendar-notification",
        "support",
    ]
    .iter()
    .any(|p| {
        local == *p
            || local.starts_with(&format!("{p}-"))
            || local.starts_with(&format!("{p}+"))
            || local.starts_with(&format!("{p}."))
            || local.contains("noreply")
            || local.contains("no-reply")
    })
}

fn who_is(cx: &mut Cx, who: &str) -> Result<AskAnswer> {
    let (t, candidates) = match resolve_who(cx, who, AskIntent::WhoIs)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    let rows = cx.store.read(|c| target_rows(cx, c, &t, None, None))?;
    let direct: Vec<&Row> = rows.iter().filter(|r| r.from_them || r.by_me).collect();
    let mut a = cx.answer(AskIntent::WhoIs);
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.search_query = Some(target_query(&t, Dir::Any));
    a.followups = person_followups(&t, AskIntent::WhoIs);
    let domain = person_of(&t).domain.unwrap_or_default();
    match t.kind {
        TargetKind::Person => {
            let email = t.emails.first().cloned().unwrap_or_default();
            a.headline = match &t.name {
                Some(n) => format!("{n} <{email}>"),
                None => email.clone(),
            };
            a.facts.push(cx.fact("Email", t.emails.join(", ")));
            if !PUBLIC_MAIL.contains(&domain.as_str()) && !domain.is_empty() {
                a.facts.push(cx.fact("Company domain", domain.clone()));
            }
        }
        TargetKind::Company => {
            let mut counts: HashMap<&str, (usize, Option<&str>)> = HashMap::new();
            for r in &direct {
                if r.from_them {
                    let e = counts.entry(r.from_email.as_str()).or_insert((0, None));
                    e.0 += 1;
                    if e.1.is_none() {
                        e.1 = r.from_name.as_deref();
                    }
                }
            }
            let mut people: Vec<(&str, (usize, Option<&str>))> = counts.into_iter().collect();
            people.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
            a.headline = format!(
                "{}: {} you've heard from",
                t.label,
                plural(people.len().max(t.emails.len().min(1)), "person", "people")
            );
            for (e, (n, name)) in people.iter().take(6) {
                a.facts.push(cx.fact(
                    name.unwrap_or(e),
                    format!("{e} · {}", plural(*n, "message", "messages")),
                ));
            }
        }
    }
    if let (Some(f), Some(l)) = (direct.first(), direct.last()) {
        let them = direct.iter().filter(|r| r.from_them).count();
        a.detail = Some(format!(
            "{} since {}; last {} ({})",
            plural(direct.len(), "message", "messages"),
            fmt_month(local_date(f.date, cx.off)),
            fmt_day(l.date, cx.off),
            relative(local_date(l.date, cx.off), cx.today)
        ));
        a.facts.push(cx.date_fact("First contact", f));
        a.facts.push(cx.date_fact("Last contact", l));
        a.facts.push(cx.fact(
            "Messages",
            format!("{them} from them · {} from you", direct.len() - them),
        ));
        let mut per_account: Vec<(String, usize)> = Vec::new();
        for r in &direct {
            match per_account.iter_mut().find(|x| x.0 == r.account_id) {
                Some(x) => x.1 += 1,
                None => per_account.push((r.account_id.clone(), 1)),
            }
        }
        per_account.sort_by_key(|x| std::cmp::Reverse(x.1));
        a.facts.push(
            cx.fact(
                "In accounts",
                per_account
                    .iter()
                    .map(|(a, n)| format!("{a} ({n})"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        );
        a.items = direct.iter().rev().take(5).map(|r| r.item(None)).collect();
        if t.kind == TargetKind::Person {
            let contact = facts::contact_facts(cx, &t)?;
            let mut phone = false;
            let mut address = false;
            for h in &contact {
                let crate::structured::Extracted::Contact(c) = &h.f.fact else {
                    continue;
                };
                if let (false, Some(p)) = (phone, c.phones.first()) {
                    let mut f = cx.date_fact("Phone", &h.row);
                    f.value = format!("{p} · signature, {}", fmt_day(h.row.date, cx.off));
                    a.facts.push(f);
                    phone = true;
                }
                if let (false, Some(ad)) = (address, c.addresses.first()) {
                    let mut f = cx.date_fact("Address", &h.row);
                    f.value = ad.clone();
                    a.facts.push(f);
                    address = true;
                }
            }
        }
        a.confidence = if t.loose {
            AskConfidence::Medium
        } else {
            AskConfidence::High
        };
    } else {
        a.detail = Some("Known address, but no mail with them in your local mail.".into());
        a.confidence = AskConfidence::Low;
    }
    Ok(cx.finish(a))
}

// ----------------------------------------------------------- open loops

/// Does authored text ask for a response?
fn asks(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains('?')
        || [
            "let me know",
            "please confirm",
            "please advise",
            "please review",
            "please send",
            "can you",
            "could you",
            "would you",
            "your thoughts",
            "thoughts on",
            "get back to me",
            "waiting on",
            "waiting for",
            "look forward to hearing",
        ]
        .iter()
        .any(|p| t.contains(p))
}

/// The newest visible message of a thread: (rowid, flags, from).
fn thread_latest(c: &Connection, thread_rowid: i64) -> Result<Option<(i64, i64, String)>> {
    Ok(c.prepare_cached("SELECT rowid, flags, lower(from_email) FROM messages WHERE thread_rowid = ?1 AND flags & ?2 = 0 ORDER BY rowid DESC LIMIT 1")?
        .query_row(params![thread_rowid, HIDDEN], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?)
}

fn waiting_on(cx: &mut Cx, who: Option<&str>) -> Result<AskAnswer> {
    let (target, candidates) = match who {
        Some(w) => match resolve_who(cx, w, AskIntent::WaitingOn)? {
            Resolved::Found(t, c) => (Some(t), c),
            Resolved::Answer(a) => return Ok(*a),
        },
        None => (None, vec![]),
    };
    let lo = cx.lo.unwrap_or(cx.now - 60 * DAY);
    let hi = cx.hi.unwrap_or(cx.now - 2 * DAY).min(cx.now - 2 * DAY);
    let (rlo, rhi) = rowid_bounds(Some(lo), Some(hi));
    let wanted: Option<HashSet<String>> =
        target.as_ref().map(|t| t.emails.iter().cloned().collect());
    let wanted_domains: Vec<String> = target
        .as_ref()
        .filter(|t| t.kind == TargetKind::Company)
        .map(|t| t.domains.clone())
        .unwrap_or_default();
    let waiting: Vec<(Row, Vec<String>)> = cx.store.read(|c| {
        let own = own_addresses(c)?;
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
             WHERE m.rowid >= ?1 AND m.rowid < ?2 AND m.flags & ?3 != 0 AND m.flags & ?4 = 0 ORDER BY m.rowid DESC LIMIT 3000"
        ))?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for r in stmt.query_map(params![rlo, rhi, F_SENT, HIDDEN], read_row)? {
            let mut r = r?;
            if !seen.insert(r.thread_rowid) || !cx.in_scope(&r.account_id) {
                continue;
            }
            let Some((latest, flags, from)) = thread_latest(c, r.thread_rowid)? else { continue };
            if latest != r.rowid && flags & F_SENT == 0 && !own.contains(&from) {
                continue; // someone replied
            }
            let to: Vec<String> = recipients(c, r.rowid)?.into_iter().filter(|e| !own.contains(e)).collect();
            if to.is_empty() || to.iter().all(|e| automated(e)) {
                continue;
            }
            // A closing "thanks, looks good" isn't waiting on anything:
            // count threads you started, or messages that ask something.
            let started: Option<i64> = c.prepare_cached("SELECT min(rowid) FROM messages WHERE thread_rowid = ?1")?.query_row([r.thread_rowid], |x| x.get(0))?;
            if started != Some(r.rowid) && !asks(&crate::text::split_quoted(&load_text(c, r.rowid)?).0) {
                continue;
            }
            if let Some(w) = &wanted {
                let hit = to.iter().any(|e| w.contains(e) || wanted_domains.iter().any(|d| e.ends_with(&format!("@{d}")) || e.ends_with(&format!(".{d}"))));
                if !hit {
                    continue;
                }
            }
            r.by_me = true;
            out.push((r, to));
        }
        Ok(out)
    })?;
    cx.steps.push(format!(
        "Threads where your message is the latest, sent {} – {} ago",
        relative(local_date(hi, cx.off), cx.today).replace(" ago", ""),
        relative(local_date(lo, cx.off), cx.today).replace(" ago", "")
    ));
    let mut a = cx.answer(AskIntent::WaitingOn);
    a.person = target.as_ref().map(person_of);
    a.candidates = candidates;
    let label = target.as_ref().map(|t| t.label.clone());
    if waiting.is_empty() {
        a.headline = match &label {
            Some(l) => {
                format!("Nothing pending: {l} has answered everything you sent in the last 60 days")
            }
            None => "Nothing pending: every email you sent in the last 60 days got a reply".into(),
        };
        a.confidence = AskConfidence::High;
        return Ok(cx.finish(a));
    }
    let mut w = waiting;
    w.sort_by_key(|(r, _)| r.date);
    let oldest = &w[0].0;
    a.headline = match &label {
        Some(l) => format!(
            "You're waiting on {l} in {}",
            plural(w.len(), "thread", "threads")
        ),
        None => format!(
            "You're waiting on replies in {}",
            plural(w.len(), "thread", "threads")
        ),
    };
    a.detail = Some(format!(
        "Oldest: \u{201c}{}\u{201d}, sent {}",
        clip(&oldest.subject_or_none(), 60),
        relative(local_date(oldest.date, cx.off), cx.today)
    ));
    let names: HashMap<String, Option<String>> = cx.store.read(|c| {
        let mut out = HashMap::new();
        let mut stmt = c.prepare_cached("SELECT name FROM people WHERE email = ?1")?;
        for (_, to) in &w {
            for e in to {
                if !out.contains_key(e) {
                    out.insert(
                        e.clone(),
                        stmt.query_row([e], |r| r.get::<_, Option<String>>(0))
                            .optional()?
                            .flatten(),
                    );
                }
            }
        }
        Ok(out)
    })?;
    a.items = w
        .iter()
        .take(20)
        .map(|(r, to)| {
            let who = to
                .iter()
                .map(|e| names.get(e).cloned().flatten().unwrap_or_else(|| e.clone()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            r.item(Some(format!(
                "Waiting {} · to {who}",
                relative(local_date(r.date, cx.off), cx.today).replace(" ago", "")
            )))
        })
        .collect();
    a.confidence = AskConfidence::High;
    a.search_query = Some(format!(
        "in:sent{}",
        target.as_ref().map_or(String::new(), |t| format!(
            " {}",
            target_query(t, Dir::ToThem)
        ))
    ));
    a.followups.push(suggest(
        "What do I owe replies to?",
        "What do I owe replies to?".into(),
    ));
    Ok(cx.finish(a))
}

fn owe_replies(cx: &mut Cx, who: Option<&str>) -> Result<AskAnswer> {
    let (target, candidates) = match who {
        Some(w) => match resolve_who(cx, w, AskIntent::OweReplies)? {
            Resolved::Found(t, c) => (Some(t), c),
            Resolved::Answer(a) => return Ok(*a),
        },
        None => (None, vec![]),
    };
    let lo = cx.lo.unwrap_or(cx.now - 30 * DAY);
    let (rlo, rhi) = rowid_bounds(Some(lo), cx.hi);
    let wanted: Option<HashSet<String>> =
        target.as_ref().map(|t| t.emails.iter().cloned().collect());
    let owed: Vec<Row> = cx.store.read(|c| {
        let own = own_addresses(c)?;
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
             WHERE m.rowid >= ?1 AND m.rowid < ?2 AND m.flags & ?3 = 0 ORDER BY m.rowid DESC LIMIT 5000"
        ))?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for r in stmt.query_map(params![rlo, rhi, HIDDEN | F_SENT | F_NEWSLETTER], read_row)? {
            let r = r?;
            if !seen.insert(r.thread_rowid) || !cx.in_scope(&r.account_id) || own.contains(&r.from_email) || automated(&r.from_email) {
                continue;
            }
            if let Some(w) = &wanted {
                if !w.contains(&r.from_email) {
                    continue;
                }
            }
            // Archived = handled; a later message (yours) = answered.
            if r.flags & F_INBOX == 0 {
                continue;
            }
            let Some((latest, _, _)) = thread_latest(c, r.thread_rowid)? else { continue };
            if latest != r.rowid {
                continue;
            }
            // Addressed to you, not a list you're on.
            if !recipients(c, r.rowid)?.iter().any(|e| own.contains(e)) {
                continue;
            }
            out.push(r);
        }
        Ok(out)
    })?;
    cx.steps.push(format!(
        "Inbox threads since {} where the latest message is from a person (not automated or a newsletter), addressed to you, with no reply from you after it",
        fmt_day(lo, cx.off)
    ));
    let mut a = cx.answer(AskIntent::OweReplies);
    a.person = target.as_ref().map(person_of);
    a.candidates = candidates;
    a.search_query = Some(format!(
        "in:inbox{}",
        target.as_ref().map_or(String::new(), |t| format!(
            " {}",
            target_query(t, Dir::FromThem)
        ))
    ));
    if owed.is_empty() {
        a.headline = match &target {
            Some(t) => format!("You don't owe {} a reply", t.label),
            None => "You're all caught up: no one is waiting on a reply".into(),
        };
        return Ok(cx.finish(a));
    }
    let unread = owed.iter().filter(|r| r.flags & F_UNREAD != 0).count();
    a.headline = match &target {
        Some(t) => format!(
            "You owe {} a reply in {}",
            t.label,
            plural(owed.len(), "thread", "threads")
        ),
        None => format!(
            "You owe replies in {}",
            plural(owed.len(), "thread", "threads")
        ),
    };
    let mut names: Vec<String> = Vec::new();
    for r in &owed {
        let n = r
            .from_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| r.from_email.clone());
        if !names.contains(&n) {
            names.push(n);
        }
    }
    a.detail = Some(format!(
        "From {}{}",
        names.iter().take(4).cloned().collect::<Vec<_>>().join(", "),
        if unread > 0 {
            format!(" · {unread} unread")
        } else {
            String::new()
        }
    ));
    a.items = owed
        .iter()
        .take(20)
        .map(|r| {
            let age = relative(local_date(r.date, cx.off), cx.today);
            r.item(Some(format!(
                "{}{age}",
                if r.flags & F_UNREAD != 0 {
                    "Unread · "
                } else {
                    ""
                }
            )))
        })
        .collect();
    a.followups.push(suggest(
        "What am I waiting on?",
        "What am I waiting on?".into(),
    ));
    Ok(cx.finish(a))
}

// ------------------------------------------------------------------ when

fn when(cx: &mut Cx, topic: &str) -> Result<AskAnswer> {
    let words = topic_words(topic);
    // A booking, trip or bill the question names: its date is exact.
    if let Some(a) = facts::when_from_facts(cx, &words)? {
        return Ok(a);
    }
    let clean = if words.is_empty() {
        topic.to_string()
    } else {
        words.join(" ")
    };
    let query = format!("{clean}{}", cx.range_ops());
    let hits = search(cx, &query, 12)?;
    cx.steps.push(format!(
        "Searched \u{201c}{query}\u{201d}: {}",
        plural(hits.len(), "thread", "threads")
    ));
    let mut a = cx.answer(AskIntent::When);
    a.search_query = Some(query.clone());
    if hits.is_empty() {
        a.headline = format!("I couldn't find any email about \u{201c}{clean}\u{201d}");
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    }
    // Stems for matching the topic inside a sentence ("renewal" ~ "renew").
    let stems: Vec<String> = words.iter().map(|w| w.chars().take(5).collect()).collect();
    struct Cand {
        score: f64,
        m: extract::DateMention,
        hit: usize,
        cite: AskCite,
        from: Address,
        msg_date: i64,
    }
    let mut cands: Vec<Cand> = Vec::new();
    // The newest few messages of each top thread: a short reply ("see you
    // tonight") often matches best while the date is in the message before.
    let read: Vec<(usize, Row, String)> = cx.store.read(|c| {
        let mut out = Vec::new();
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
             WHERE t.account_id = ?1 AND t.thread_id = ?2 AND m.flags & ?3 = 0 ORDER BY m.rowid DESC LIMIT 6"
        ))?;
        for (i, h) in hits.iter().enumerate().take(6) {
            for r in stmt.query_map(params![h.account_id, h.thread_id, HIDDEN], read_row)? {
                let r = r?;
                let body = load_text(c, r.rowid)?;
                let text = format!("{}.\n{}", r.subject, crate::text::split_quoted(&body).0);
                out.push((i, r, text));
            }
        }
        Ok(out)
    })?;
    for (i, r, text) in &read {
        let anchor = local_date(r.date, cx.off);
        for m in extract::date_mentions(text, anchor) {
            let s = m.sentence.to_lowercase();
            let topical = stems.iter().filter(|st| s.contains(st.as_str())).count() as f64;
            let mut score = topical * 3.0 - *i as f64 * 0.5;
            if m.explicit {
                score += 2.0;
            }
            if m.date >= anchor {
                score += 1.0;
            }
            if m.time.is_some() {
                score += 0.5;
            }
            // Newer mail is more likely current.
            score += 1.0 / (1.0 + ((cx.now - r.date) / DAY) as f64 / 30.0);
            cands.push(Cand {
                score,
                m,
                hit: *i,
                cite: r.cite(),
                from: Address {
                    name: r.from_name.clone(),
                    email: r.from_email.clone(),
                },
                msg_date: r.date,
            });
        }
    }
    cx.steps.push(format!("Read {} in the top threads and extracted the dates written in them (\u{201c}Sep 24\u{201d}, \u{201c}1/31/2027\u{201d}, \u{201c}on Thursday\u{201d}, relative to each email's date)", plural(read.len(), "message", "messages")));
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));
    let item_for = |i: usize, note: Option<String>| -> AskItem {
        let h = &hits[i];
        AskItem {
            account_id: h.account_id.clone(),
            thread_id: h.thread_id.clone(),
            message_id: h.message_id.clone(),
            subject: h.subject.clone(),
            from: h.from.clone(),
            date: h.date,
            snippet: clip(&strip_marks(&h.snippet_html), 180),
            note,
            amount: None,
            sent: h.label_ids.iter().any(|l| l == "SENT"),
        }
    };
    let Some(best) = cands.first() else {
        a.headline = format!(
            "I found {} about \u{201c}{clean}\u{201d}, but none mention a date",
            plural(hits.len(), "email", "emails")
        );
        a.confidence = AskConfidence::Low;
        a.items = (0..hits.len().min(MAX_ITEMS))
            .map(|i| item_for(i, None))
            .collect();
        return Ok(cx.finish(a));
    };
    let day = best.m.date.format("%a, %b %-d, %Y").to_string();
    let time = best
        .m
        .time
        .as_ref()
        .map(|t| format!(" at {t}"))
        .unwrap_or_default();
    a.headline = format!(
        "{}: {day}{time} ({})",
        capitalize(&clean),
        relative(best.m.date, cx.today)
    );
    let from = best
        .from
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| best.from.email.clone());
    a.detail = Some(format!(
        "\u{201c}{}\u{201d} — {from}, {}",
        best.m.sentence,
        fmt_day(best.msg_date, cx.off)
    ));
    let mut shown: Vec<NaiveDate> = Vec::new();
    for cnd in &cands {
        if shown.contains(&cnd.m.date) || shown.len() >= 5 {
            continue;
        }
        shown.push(cnd.m.date);
        a.facts.push(AskFact {
            label: cnd.m.date.format("%a, %b %-d, %Y").to_string(),
            value: clip(&cnd.m.sentence, 120),
            date: Some(day_ms(cnd.m.date, cx.off)),
            cite: Some(cnd.cite.clone()),
        });
    }
    let mut order: Vec<usize> = vec![best.hit];
    for cnd in &cands {
        if !order.contains(&cnd.hit) {
            order.push(cnd.hit);
        }
    }
    for i in 0..hits.len() {
        if !order.contains(&i) {
            order.push(i);
        }
    }
    a.items = order
        .into_iter()
        .take(MAX_ITEMS)
        .map(|i| match cands.iter().find(|c| c.hit == i) {
            Some(cnd) => {
                let mut it = item_for(i, Some(cnd.m.sentence.clone()));
                it.message_id = cnd.cite.message_id.clone();
                it
            }
            None => item_for(i, None),
        })
        .collect();
    let topical = stems
        .iter()
        .any(|st| best.m.sentence.to_lowercase().contains(st.as_str()));
    a.confidence = if topical && best.m.explicit {
        AskConfidence::Medium
    } else {
        AskConfidence::Low
    };
    a.followups.push(suggest(
        "Who emailed about it?",
        format!("Who emailed me about {clean}"),
    ));
    Ok(cx.finish(a))
}

// --------------------------------------------------------------- unknown

fn unknown(cx: &mut Cx) -> Result<AskAnswer> {
    // Any question: the best sentences in the mail it's about, quoted.
    let text = cx.q.text.clone();
    let mut a = passages::topic(cx, AskIntent::Unknown, &text)?;
    if a.passages.is_empty() && a.items.is_empty() {
        a.headline = "I can't answer that one directly".into();
        a.followups = example_questions()
            .into_iter()
            .take(4)
            .map(|q| suggest(q, q.to_string()))
            .collect();
    }
    Ok(a)
}
