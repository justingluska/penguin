//! Search query language. OWNER: store/search agent. Reference: docs/SEARCH.md.
//!
//! Supported (Gmail-compatible where possible):
//!   people   from: to: cc: bcc: (to:/bcc: = any recipient), with:/participant:
//!            (sender or any recipient), domain:, from:me, to:me,
//!            from:new (= is:new-sender), to:new (= is:first-outbound)
//!   content  subject: filename: "quoted phrases"
//!   has:     attachment pdf image doc spreadsheet presentation invite
//!            link otp unsubscribe
//!   is:      unread read starred unstarred important snoozed newsletter
//!            new-sender (the first mail you ever got from that address)
//!            first-outbound (the first mail you ever sent to an address)
//!            unanswered (received, no reply from you after it)
//!            replied, awaiting (you sent the last message), reply
//!            (not the first message of its thread); is:sent/draft/inbox…
//!            are folder aliases
//!   where    in:/folder: inbox|sent|drafts|trash|spam|anywhere|done|starred|
//!            important|snoozed, label:, category:, account:
//!   size     larger:/smaller: (5M, 200K, bytes), size:>5M, messages:>5
//!            (thread length), day:monday|weekend|weekday
//!   calendar has:invite; type:event | is:event | in:calendar (events only;
//!            dates apply to the event's start)
//!   dates    before: after: since: until: on: (YYYY-MM-DD, YYYY/MM/DD,
//!            YYYY-MM, YYYY, M/D/YYYY, or any date: expression),
//!            older_than:/newer_than: (3d, 2w, 3m, 1y),
//!            date:<expression>, a bare word or a quoted phrase (grammar in dates.rs):
//!     date:february (the most recent one), date:"feb 2025", date:"feb 10",
//!     date:"late february", date:"the week of feb 10", date:tuesday,
//!     date:today, date:yesterday, date:"last week", date:"last spring",
//!     date:"2 weeks ago", date:"since march", date:"between jan and march",
//!     date:"jan 5 to jan 20", date:aug1..aug15, date:2026-02, date:2026-02-10,
//!     date:2025. An unreadable value becomes an `error` chip that constrains nothing.
//!   logic    -exclusion, OR (binds tighter than the implicit AND), AND
//!            (optional), ( ) grouping and -( ) negated groups.
//! Free text is never a date: "february", "last spring" or "yesterday" typed
//! without `date:` are searched as words.
//!
//! The parser never fails: anything it does not recognize stays plain text.
//! Every recognized piece becomes a [`SearchChip`] whose `raw` is the exact
//! source substring, so the UI can remove a chip by editing the text.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, TimeZone};

use crate::dates::{self, year_num, ymd};
use crate::types::SearchChip;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    From,
    /// To, Cc and Bcc (Gmail semantics).
    To,
    Cc,
    Subject,
    /// The sender or any recipient (`with:`, `participant:`).
    Participant,
    /// An address domain anywhere in From/To/Cc/Bcc (`domain:`).
    Domain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HasKind {
    Attachment,
    Pdf,
    Image,
    Doc,
    Spreadsheet,
    Presentation,
    /// A calendar invitation: a text/calendar part or an .ics file.
    Invite,
    /// A web link in the message's own text.
    Link,
    /// A detected verification code or sign-in link (see otp.rs).
    Otp,
    /// A List-Unsubscribe header.
    Unsubscribe,
}

impl HasKind {
    /// True for the kinds that are attachments (drive the attachment panel).
    pub fn is_attachment(self) -> bool {
        !matches!(self, HasKind::Link | HasKind::Otp | HasKind::Unsubscribe)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Spam,
    Anywhere,
    /// Archived: not in the inbox.
    Done,
    Starred,
    Important,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unread,
    Read,
    Starred,
    Unstarred,
    Important,
    /// In a snoozed conversation (Penguin's local snooze).
    Snoozed,
    /// Bulk mail: a List-Unsubscribe header or a promotions/updates/forums category.
    Newsletter,
    /// The first message ever received from its sender's address.
    NewSender,
    /// A sent message that was the first one ever sent to one of its recipients.
    FirstOutbound,
    /// Received from an address you have written to (the "People you know"
    /// view, as an operator: Split Inbox and rules use it to keep people apart
    /// from bulk and cold mail).
    KnownSender,
    /// Received (not bulk), and nothing was sent in its thread after it.
    Unanswered,
    /// Received, and you sent something in its thread after it.
    Replied,
    /// Sent, and nothing arrived (or was sent) in its thread after it.
    Awaiting,
    /// Not the first message of its thread.
    Reply,
}

/// `from:me` / `to:me`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Me {
    /// Sent (or drafted) by you.
    From,
    /// One of your addresses is a recipient.
    To,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Atom {
    /// Free text. `phrase` = quoted; `prefix` = last unfinished word (as-you-type).
    Text {
        text: String,
        phrase: bool,
        prefix: bool,
    },
    Field {
        field: Field,
        value: String,
        prefix: bool,
    },
    /// `a AROUND n b` (Gmail): both, at most `distance` words apart.
    Near {
        a: String,
        b: String,
        distance: u32,
    },
    Filename(String),
    Has(HasKind),
    /// User label by name or id; resolved by the store per account.
    Label(String),
    In(Folder),
    Is(State),
    Me(Me),
    /// Total size of the attached files, bytes: `min <= size < max`.
    Size {
        min: Option<u64>,
        max: Option<u64>,
    },
    /// Messages in the conversation, inclusive bounds (`messages:>5`).
    ThreadLen {
        min: u32,
        max: Option<u32>,
    },
    /// Day of the week the message arrived: bit 0 = Sunday … bit 6 = Saturday.
    /// `utc_offset_secs` None = the machine's local time (DST-aware).
    Weekday {
        days: u8,
        utc_offset_secs: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub negated: bool,
    pub atom: Atom,
}

/// A disjunction of terms (`a OR b`). Most clauses hold a single term.
#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    pub terms: Vec<Term>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedQuery {
    pub chips: Vec<SearchChip>,
    /// Conjunction of clauses.
    pub clauses: Vec<Clause>,
    /// Inclusive lower bound, unix ms.
    pub after: Option<i64>,
    /// Exclusive upper bound, unix ms.
    pub before: Option<i64>,
    /// `account:` values (lowercased); OR-ed together.
    pub accounts: Vec<String>,
    pub include_trash: bool,
    pub include_spam: bool,
    /// `type:event` / `is:event` / `in:calendar`: calendar events only, no mail.
    pub events_only: bool,
    /// Other forms of a plain word the mail uses, keyed by the word's typed
    /// text: inflections, British/American spellings, compounds, codes in
    /// another format (lexicon.rs). Each is folded text (several words = a
    /// phrase), matched as an alternative to the typed form. Filled by search
    /// from the index; empty after parsing.
    pub alternatives: std::collections::BTreeMap<String, Vec<String>>,
}

impl ParsedQuery {
    /// Positive free-text terms, for highlighting and person resolution.
    pub fn text_terms(&self) -> impl Iterator<Item = (&str, bool, bool)> {
        self.clauses
            .iter()
            .flat_map(|c| c.terms.iter())
            .filter_map(|t| match &t.atom {
                Atom::Text {
                    text,
                    phrase,
                    prefix,
                } if !t.negated => Some((text.as_str(), *phrase, *prefix)),
                _ => None,
            })
    }

    /// True when the query restricts nothing (only whitespace or noise).
    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty()
            && self.after.is_none()
            && self.before.is_none()
            && self.accounts.is_empty()
    }

    /// True when the date constraints can never match.
    pub fn empty_range(&self) -> bool {
        matches!((self.after, self.before), (Some(a), Some(b)) if a >= b)
    }
}

/// Parse with the machine's local timezone (for date phrases).
pub fn parse(input: &str, now_ms: i64) -> ParsedQuery {
    Parser {
        input,
        now_ms,
        tz: Tz::Local,
    }
    .run()
}

/// Parse with a fixed UTC offset; deterministic, used by tests.
pub fn parse_with_offset(input: &str, now_ms: i64, utc_offset_secs: i32) -> ParsedQuery {
    Parser {
        input,
        now_ms,
        tz: Tz::Fixed(utc_offset_secs),
    }
    .run()
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Tz {
    Local,
    Fixed(i32),
}

impl Tz {
    fn local_now(self, now_ms: i64) -> NaiveDateTime {
        let utc = chrono::DateTime::from_timestamp_millis(now_ms).unwrap_or_default();
        match self {
            Tz::Local => utc.with_timezone(&chrono::Local).naive_local(),
            Tz::Fixed(off) => utc.naive_utc() + Duration::seconds(off as i64),
        }
    }

    /// Unix ms of local midnight at the start of `d`.
    fn midnight_ms(self, d: NaiveDate) -> i64 {
        let naive = d.and_hms_opt(0, 0, 0).expect("midnight exists");
        match self {
            Tz::Local => chrono::Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|t| t.timestamp_millis())
                // Midnight skipped by a DST jump: fall back to 1 AM.
                .unwrap_or_else(|| {
                    chrono::Local
                        .from_local_datetime(&(naive + Duration::hours(1)))
                        .earliest()
                        .map_or(naive.and_utc().timestamp_millis(), |t| t.timestamp_millis())
                }),
            Tz::Fixed(off) => naive.and_utc().timestamp_millis() - off as i64 * 1000,
        }
    }
}

struct Parser<'a> {
    input: &'a str,
    now_ms: i64,
    tz: Tz,
}

enum Item {
    Term(Term),
    Or,
    /// Something recognized that is not a boolean term (dates, account:,
    /// pending ops, `AND`). Breaks an OR chain.
    Global,
    /// `(` or `-(`; `{` or `-{` (Gmail: any of the terms inside) with `any`.
    Open {
        negated: bool,
        any: bool,
    },
    Close,
    /// `AROUND n` between two terms (folded into `Atom::Near` by `near`).
    Around(u32),
}

/// Word distance for `a AROUND b` without a number (FTS5's NEAR default).
const AROUND_DEFAULT: u32 = 10;

/// Quotes a phrase may open with: straight, and the typographic ones a
/// paste or macOS smart quotes produce (“…”, „…“).
fn is_open_quote(c: char) -> bool {
    matches!(c, '"' | '\u{201c}' | '\u{201e}' | '\u{201d}')
}

fn is_close_quote(c: char) -> bool {
    matches!(c, '"' | '\u{201d}' | '\u{201c}')
}

fn is_quote(c: char) -> bool {
    is_open_quote(c) || is_close_quote(c)
}

/// `key:(…)` at `at`: the operator key and the byte offsets of the parens,
/// when the key takes a value per term and the group closes.
fn operator_group(s: &str, at: usize) -> Option<(String, usize, usize)> {
    let rest = &s[at..];
    let colon = rest.find(':')?;
    let key = rest[..colon].to_ascii_lowercase();
    if !rest[colon + 1..].starts_with('(')
        || !matches!(
            key.as_str(),
            "from" | "to" | "cc" | "bcc" | "with" | "participant" | "participants" | "domain"
                | "subject" | "filename" | "label" | "has" | "is" | "in" | "category"
                | "deliveredto" | "attachment"
        )
    {
        return None;
    }
    let open = at + colon + 1;
    let mut depth = 0usize;
    let mut quoted = false;
    for (k, ch) in s[open..].char_indices() {
        match ch {
            c if is_quote(c) => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    return Some((key, open, open + k));
                }
            }
            _ => {}
        }
    }
    None
}

/// The terms of an operator group: whitespace-separated, quotes kept
/// together, a leading `-` negating one.
fn group_pieces(inner: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i..].chars().next().expect("in bounds");
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let start = i;
        let mut quoted = false;
        while i < inner.len() {
            let ch = inner[i..].chars().next().expect("in bounds");
            if ch.is_whitespace() && !quoted {
                break;
            }
            if is_quote(ch) {
                quoted = !quoted;
            }
            i += ch.len_utf8();
        }
        let piece = &inner[start..i];
        match piece.strip_prefix('-') {
            Some(p) if !p.is_empty() => out.push((true, p)),
            _ => out.push((false, piece)),
        }
    }
    out
}

/// `a AROUND n b` → one `Near` term; an `AROUND` without a free-text term
/// on each side is dropped (the terms stay, ANDed).
fn near(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    let mut it = items.into_iter().peekable();
    while let Some(item) = it.next() {
        let Item::Around(distance) = item else {
            out.push(item);
            continue;
        };
        let left = match out.last() {
            Some(Item::Term(Term { negated: false, atom: Atom::Text { text, .. } })) => Some(text.clone()),
            _ => None,
        };
        let right = match it.peek() {
            Some(Item::Term(Term { negated: false, atom: Atom::Text { text, .. } })) => Some(text.clone()),
            _ => None,
        };
        if let (Some(a), Some(b)) = (left, right) {
            it.next();
            out.pop();
            out.push(Item::Term(Term {
                negated: false,
                atom: Atom::Near { a, b, distance },
            }));
        }
    }
    out
}

/// Boolean structure before it is flattened into clauses.
enum Expr {
    Lit(Term),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

/// Parenthesized queries are flattened into AND-of-OR clauses; a query whose
/// expansion would need more clauses (or terms per clause) than this is
/// reported as an error instead.
const MAX_CNF: usize = 64;

/// Label of the chip for grouping that would expand too far.
pub const GROUP_ERROR: &str = "Too many combinations in ( ); simplify the grouping";

/// A date constraint: [after, before).
#[derive(Debug, Clone, Copy)]
struct Range {
    after: Option<i64>,
    before: Option<i64>,
}

impl Parser<'_> {
    fn run(&self) -> ParsedQuery {
        let s = self.input;
        let mut q = ParsedQuery::default();
        let mut items: Vec<Item> = Vec::new();
        let mut depth = 0usize;
        // An uppercase `NOT` (Outlook/KQL, Apple Mail) negates what follows.
        let mut not_at: Option<usize> = None;
        let mut i = 0;
        while i < s.len() {
            let c = s[i..].chars().next().expect("in bounds");
            if c.is_whitespace() {
                i += c.len_utf8();
                continue;
            }
            // Grouping: `(` / `-(` open an AND group, Gmail's `{` / `-{` an OR
            // group; `)` or `}` closes one that is open.
            let dash_group = c == '-' && (s[i + 1..].starts_with('(') || s[i + 1..].starts_with('{'));
            if c == '(' || c == '{' || dash_group {
                let brace = if dash_group { s[i + 1..].starts_with('{') } else { c == '{' };
                let negated = dash_group ^ not_at.take().is_some();
                items.push(Item::Open { negated, any: brace });
                depth += 1;
                i += if dash_group { 2 } else { 1 };
                continue;
            }
            if (c == ')' || c == '}') && depth > 0 {
                items.push(Item::Close);
                depth -= 1;
                i += 1;
                continue;
            }
            let start = not_at.unwrap_or(i);
            let dash = c == '-'
                && s[i + 1..]
                    .chars()
                    .next()
                    .is_some_and(|n| !n.is_whitespace());
            let body_start = if dash { i + 1 } else { i };
            let negated = dash ^ not_at.is_some();
            let first = s[body_start..].chars().next();
            if first.is_some_and(is_open_quote) {
                let open = first.map_or(1, char::len_utf8);
                let close = s[body_start + open..].find(is_close_quote).map(|p| body_start + open + p);
                let end = close.map_or(s.len(), |p| p + s[p..].chars().next().map_or(1, char::len_utf8));
                let content = &s[body_start + open..close.unwrap_or(s.len())];
                not_at = None;
                if has_tokens(content) {
                    let label = format!("\u{201c}{}\u{201d}", content.trim());
                    let (kind, label) = if negated {
                        ("exclude", format!("Excluding {label}"))
                    } else {
                        ("text", label)
                    };
                    q.chips.push(chip(kind, label, &s[start..end]));
                    items.push(Item::Term(Term {
                        negated,
                        atom: Atom::Text {
                            text: content.to_string(),
                            phrase: true,
                            prefix: false,
                        },
                    }));
                }
                i = end;
                continue;
            }
            // `subject:(dinner movie)`, `from:(ana OR bob)`: the operator
            // applies to every term of the group (Gmail).
            if let Some((key, open, close)) = operator_group(s, body_start) {
                not_at = None;
                let raw = &s[start..=close];
                items.push(Item::Open { negated, any: false });
                for (piece_neg, piece) in group_pieces(&s[open + 1..close]) {
                    match piece {
                        "OR" => items.push(Item::Or),
                        "AND" => items.push(Item::Global),
                        _ => {
                            let body = format!("{key}:{piece}");
                            match self.operator(&body, raw, piece_neg, &mut q) {
                                Some(item) => items.push(item),
                                None if has_tokens(piece) => items.push(Item::Term(Term {
                                    negated: piece_neg,
                                    atom: Atom::Text {
                                        text: piece.trim_matches(is_quote).to_string(),
                                        phrase: false,
                                        prefix: false,
                                    },
                                })),
                                None => {}
                            }
                        }
                    }
                }
                items.push(Item::Close);
                i = close + 1;
                continue;
            }
            let mut j = body_start;
            while j < s.len() {
                let ch = s[j..].chars().next().expect("in bounds");
                if ch.is_whitespace() || ((ch == ')' || ch == '}') && depth > 0) {
                    break;
                }
                if is_open_quote(ch) {
                    let after = j + ch.len_utf8();
                    j = s[after..].find(is_close_quote).map_or(s.len(), |p| {
                        let k = after + p;
                        k + s[k..].chars().next().map_or(1, char::len_utf8)
                    });
                    continue;
                }
                j += ch.len_utf8();
            }
            let raw = &s[start..j];
            let body = &s[body_start..j];
            i = j;
            if let Some(item) = self.operator(body, raw, negated, &mut q) {
                not_at = None;
                items.push(item);
                continue;
            }
            if !dash {
                match body {
                    "OR" => {
                        items.push(Item::Or);
                        continue;
                    }
                    "AND" => {
                        items.push(Item::Global);
                        continue;
                    }
                    "NOT" => {
                        not_at = if not_at.is_some() { None } else { Some(start) };
                        continue;
                    }
                    // Gmail's `a AROUND 5 b`: the two neighbouring terms within
                    // that many words (the number is optional).
                    "AROUND" | "NEAR" => {
                        let rest = s[j..].trim_start();
                        let n: String = rest.chars().take_while(char::is_ascii_digit).collect();
                        let after = rest[n.len()..].chars().next();
                        let distance = if !n.is_empty() && after.is_none_or(char::is_whitespace) {
                            i = s.len() - rest.len() + n.len();
                            n.parse().unwrap_or(AROUND_DEFAULT).min(1000)
                        } else {
                            AROUND_DEFAULT
                        };
                        items.push(Item::Around(distance));
                        continue;
                    }
                    _ => {}
                }
            }
            not_at = None;
            if !has_tokens(body) {
                continue;
            }
            // A trailing `*` asks for a prefix match (`invoic*`), as in
            // Outlook, Fastmail, notmuch and mu.
            let (text, wildcard) = match body.strip_suffix('*') {
                Some(t) if has_tokens(t) => (t.trim_end_matches('*'), true),
                _ => (body, false),
            };
            if negated {
                q.chips.push(chip(
                    "exclude",
                    format!("Excluding \u{201c}{text}\u{201d}"),
                    raw,
                ));
            }
            items.push(Item::Term(Term {
                negated,
                atom: Atom::Text {
                    text: text.to_string(),
                    phrase: false,
                    prefix: wildcard,
                },
            }));
        }
        let mut items = near(items);

        // As-you-type: the final unfinished word (or unquoted operator value) is a prefix.
        let ends_open = s
            .chars()
            .last()
            .is_some_and(|c| !c.is_whitespace() && !is_close_quote(c) && c != ')' && c != '}');
        if ends_open {
            if let Some(Item::Term(t)) = items.last_mut() {
                if !t.negated {
                    match &mut t.atom {
                        Atom::Text {
                            phrase: false,
                            prefix,
                            ..
                        } => *prefix = true,
                        // A complete address is never a fragment being typed.
                        Atom::Field { prefix, value, .. } if !is_complete_email(value) => {
                            *prefix = true
                        }
                        _ => {}
                    }
                }
            }
        }

        // Structure: `a OR b` binds adjacent terms, ( ) groups, then the
        // whole thing is flattened into a conjunction of OR clauses.
        let mut items = items.into_iter().peekable();
        let tree = Expr::And(sequence(&mut items));
        match cnf(tree, false) {
            Some(clauses) => {
                q.clauses = clauses
                    .into_iter()
                    .filter(|terms| !terms.is_empty())
                    .map(|terms| Clause { terms })
                    .collect()
            }
            None => q
                .chips
                .push(chip("error", GROUP_ERROR.to_string(), s.trim())),
        }
        drop_noise_words(&mut q);
        for t in q.clauses.iter().flat_map(|c| c.terms.iter()) {
            if t.negated {
                continue;
            }
            match t.atom {
                Atom::In(Folder::Trash) => q.include_trash = true,
                Atom::In(Folder::Spam) => q.include_spam = true,
                Atom::In(Folder::Anywhere) => {
                    q.include_trash = true;
                    q.include_spam = true;
                }
                _ => {}
            }
        }
        q
    }

    /// Recognize `key:value`. Returns None when `body` is not a known operator
    /// with a valid value (it then stays plain text).
    fn operator(&self, body: &str, raw: &str, negated: bool, q: &mut ParsedQuery) -> Option<Item> {
        let colon = body.find(':')?;
        if colon == 0 {
            return None;
        }
        let key = body[..colon].to_ascii_lowercase();
        let raw_value = &body[colon + 1..];
        let value = raw_value.trim_matches(is_quote).trim();
        if !OPERATOR_KEYS.contains(&key.as_str()) {
            return None;
        }
        if value.is_empty() {
            // "from:" while typing: recognized, but constrains nothing yet.
            return Some(Item::Global);
        }
        // Other mail apps' spellings of the same operators: Gmail's
        // deliveredto: (the recipient), Outlook/KQL's participants:,
        // attachment:/attachmentnames: (a file name) and hasattachment(s):yes.
        let key = match key.as_str() {
            "deliveredto" => "to".to_string(),
            "participants" => "with".to_string(),
            "attachment" | "attachmentnames" => "filename".to_string(),
            "hasattachment" | "hasattachments" => {
                return match value.to_ascii_lowercase().as_str() {
                    "yes" | "true" => self.operator("has:attachment", raw, negated, q),
                    "no" | "false" => self.operator("has:attachment", raw, !negated, q),
                    _ => None,
                };
            }
            _ => key,
        };
        // `subject:set*`: an explicit prefix.
        let (value, wildcard) = match value.strip_suffix('*') {
            Some(v) if has_tokens(v) && !matches!(key.as_str(), "filename" | "label") => (v.trim_end_matches('*'), true),
            _ => (value, false),
        };
        let lv = value.to_lowercase();
        let term = |atom: Atom, kind: &str, label: String, q: &mut ParsedQuery| -> Option<Item> {
            let (kind, label) = if negated {
                ("exclude", format!("Not {}", lower_first(&label)))
            } else {
                (kind, label)
            };
            q.chips.push(chip(kind, label, raw));
            Some(Item::Term(Term { negated, atom }))
        };
        match key.as_str() {
            "from" | "to" | "cc" | "bcc" | "subject" | "with" | "participant" => {
                // from:me / to:me, and from:new / to:new (first contact).
                match (key.as_str(), lv.as_str()) {
                    ("from", "me") => return term(Atom::Me(Me::From), "from", "From me".into(), q),
                    ("to" | "cc" | "bcc", "me") => {
                        return term(Atom::Me(Me::To), "to", "To me".into(), q)
                    }
                    ("from", "new") => {
                        return term(
                            Atom::Is(State::NewSender),
                            "is",
                            state_label(State::NewSender).into(),
                            q,
                        )
                    }
                    ("to", "new") => {
                        return term(
                            Atom::Is(State::FirstOutbound),
                            "is",
                            state_label(State::FirstOutbound).into(),
                            q,
                        )
                    }
                    ("from", "known") => {
                        return term(
                            Atom::Is(State::KnownSender),
                            "is",
                            state_label(State::KnownSender).into(),
                            q,
                        )
                    }
                    _ => {}
                }
                let (field, kind, label) = match key.as_str() {
                    "from" => (Field::From, "from", format!("From {value}")),
                    "to" | "bcc" => (Field::To, "to", format!("To {value}")),
                    "cc" => (Field::Cc, "cc", format!("Cc {value}")),
                    "with" | "participant" => (Field::Participant, "with", format!("With {value}")),
                    _ => (
                        Field::Subject,
                        "subject",
                        format!("Subject \u{201c}{value}\u{201d}"),
                    ),
                };
                if !has_tokens(value) {
                    return Some(Item::Global);
                }
                term(
                    Atom::Field {
                        field,
                        value: value.to_string(),
                        prefix: wildcard,
                    },
                    kind,
                    label,
                    q,
                )
            }
            "domain" => {
                let d = lv.trim_start_matches('@');
                if !has_tokens(d) {
                    return Some(Item::Global);
                }
                term(
                    Atom::Field {
                        field: Field::Domain,
                        value: d.to_string(),
                        prefix: wildcard,
                    },
                    "domain",
                    format!("Domain {d}"),
                    q,
                )
            }
            "filename" => term(
                Atom::Filename(value.to_string()),
                "filename",
                format!("Filename {value}"),
                q,
            ),
            "has" => {
                let k = has_kind(&lv)?;
                term(Atom::Has(k), "has", has_label(k).to_string(), q)
            }
            "in" | "is" | "type" if is_event_kind(&lv, &key) => {
                if negated {
                    return None;
                }
                q.events_only = true;
                q.chips.push(chip("type", "Calendar events".into(), raw));
                Some(Item::Global)
            }
            "in" | "folder" => {
                if matches!(lv.as_str(), "snoozed" | "snooze") {
                    return term(
                        Atom::Is(State::Snoozed),
                        "is",
                        state_label(State::Snoozed).into(),
                        q,
                    );
                }
                let (f, label) = folder(&lv)?;
                term(Atom::In(f), "in", label.to_string(), q)
            }
            "is" => {
                if let Some(st) = state(&lv) {
                    return term(Atom::Is(st), "is", state_label(st).to_string(), q);
                }
                // is:sent, is:draft, is:inbox… behave like in:.
                let (f, label) =
                    folder(&lv).filter(|(f, _)| !matches!(f, Folder::Anywhere | Folder::Done))?;
                term(Atom::In(f), "in", label.to_string(), q)
            }
            "label" => {
                // System labels typed as label:inbox behave like in:inbox.
                if let Some((f, label)) =
                    folder(&lv).filter(|(f, _)| *f != Folder::Anywhere && *f != Folder::Done)
                {
                    return term(Atom::In(f), "in", label.to_string(), q);
                }
                if lv == "unread" {
                    return term(Atom::Is(State::Unread), "is", "Unread".into(), q);
                }
                term(
                    Atom::Label(value.to_string()),
                    "label",
                    format!("Label {value}"),
                    q,
                )
            }
            "category" => {
                let cat = match lv.as_str() {
                    "primary" | "personal" => "personal",
                    "promotions" | "promotion" | "promo" | "promos" => "promotions",
                    "social" => "social",
                    "updates" | "update" => "updates",
                    "forums" | "forum" => "forums",
                    _ => return None,
                };
                let mut c = cat.chars();
                let title = c.next().map_or(String::new(), |f| {
                    f.to_uppercase().collect::<String>() + c.as_str()
                });
                term(
                    Atom::Label(cat.to_string()),
                    "label",
                    format!("Category {title}"),
                    q,
                )
            }
            "larger" | "smaller" | "size" => {
                let (atom, label) = size_value(&key, &lv)?;
                term(atom, "size", label, q)
            }
            "messages" => {
                let (min, max) = thread_len(&lv)?;
                term(
                    Atom::ThreadLen { min, max },
                    "messages",
                    thread_len_label(min, max),
                    q,
                )
            }
            "day" => {
                let (days, label) = weekdays(&lv)?;
                let utc_offset_secs = match self.tz {
                    Tz::Local => None,
                    Tz::Fixed(off) => Some(off),
                };
                term(
                    Atom::Weekday {
                        days,
                        utc_offset_secs,
                    },
                    "day",
                    label,
                    q,
                )
            }
            "before" | "after" | "since" | "until" => {
                if negated {
                    return None;
                }
                let (range, label) = self.bound(&key, &lv)?;
                let kind = if key == "before" || key == "until" {
                    "before"
                } else {
                    "after"
                };
                q.chips.push(chip(kind, label, raw));
                apply_range(q, range);
                Some(Item::Global)
            }
            "older_than" | "newer_than" | "older" | "newer" => {
                if negated {
                    return None;
                }
                let (n, unit) = parse_relative(&lv)?;
                let cutoff = self.shift_now(n, unit)?;
                let span = describe_span(n, unit);
                let (range, kind, label) = if key.starts_with("older") {
                    (
                        Range {
                            after: None,
                            before: Some(cutoff),
                        },
                        "before",
                        format!("Older than {span}"),
                    )
                } else {
                    (
                        Range {
                            after: Some(cutoff),
                            before: None,
                        },
                        "after",
                        format!("Newer than {span}"),
                    )
                };
                q.chips.push(chip(kind, label, raw));
                apply_range(q, range);
                Some(Item::Global)
            }
            "date" | "on" => {
                // An unreadable date is shown as an error chip (and constrains
                // nothing) rather than silently searched as text.
                match self.date_operator(value).filter(|_| !negated) {
                    Some((range, label)) => {
                        q.chips.push(chip("date", label, raw));
                        apply_range(q, range);
                    }
                    None => q.chips.push(chip("error", DATE_ERROR.to_string(), raw)),
                }
                Some(Item::Global)
            }
            "account" => {
                if negated {
                    return None;
                }
                q.accounts.push(lv);
                q.chips
                    .push(chip("account", format!("Account {value}"), raw));
                Some(Item::Global)
            }
            _ => None,
        }
    }

    /// before:/after:/since:/until: with a numeric date or any date expression.
    /// before: = before the start of it, after:/since: = from its start
    /// (inclusive, like Gmail's after:), until: = up to its end (inclusive).
    fn bound(&self, key: &str, lv: &str) -> Option<(Range, String)> {
        // Unix seconds (Gmail: `after:1388552400`), an exact instant.
        if lv.len() >= 9 && lv.len() <= 11 && lv.bytes().all(|b| b.is_ascii_digit()) {
            let ms = lv.parse::<i64>().ok()?.checked_mul(1000)?;
            let at = chrono::DateTime::from_timestamp_millis(ms)?;
            let local = match self.tz {
                Tz::Local => at.with_timezone(&chrono::Local).naive_local(),
                Tz::Fixed(off) => at.naive_utc() + Duration::seconds(off as i64),
            };
            let when = format!("{} {}", fmt_date(local.date()), local.format("%-I:%M %p"));
            return Some(match key {
                "before" | "until" => (
                    Range {
                        after: None,
                        before: Some(ms),
                    },
                    format!("Before {when}"),
                ),
                _ => (
                    Range {
                        after: Some(ms),
                        before: None,
                    },
                    format!("{} {when}", if key == "since" { "Since" } else { "After" }),
                ),
            });
        }
        let (start, end) = match parse_date_value(lv) {
            Some(d) => {
                let next = if lv.split(['-', '/', '.']).count() == 3 {
                    d + Duration::days(1)
                } else if lv.split(['-', '/', '.']).count() == 2 {
                    d.checked_add_months(Months::new(1))?
                } else {
                    d.checked_add_months(Months::new(12))?
                };
                (d, Some(next))
            }
            None => {
                let words = date_words(lv);
                let words: Vec<&str> = words.split_whitespace().collect();
                let today = self.tz.local_now(self.now_ms).date();
                let (n, r) = dates::parse(&words, today)?;
                if n != words.len() {
                    return None;
                }
                (r.from?, r.to)
            }
        };
        Some(match key {
            "before" => (
                Range {
                    after: None,
                    before: Some(self.tz.midnight_ms(start)),
                },
                format!("Before {}", fmt_date(start)),
            ),
            "until" => {
                let end = end?;
                (
                    Range {
                        after: None,
                        before: Some(self.tz.midnight_ms(end)),
                    },
                    format!("Until {}", fmt_date(end - Duration::days(1))),
                )
            }
            _ => (
                Range {
                    after: Some(self.tz.midnight_ms(start)),
                    before: None,
                },
                format!(
                    "{} {}",
                    if key == "since" { "Since" } else { "After" },
                    fmt_date(start)
                ),
            ),
        })
    }

    /// now minus `n` units, in unix ms (calendar-aware for months/years).
    fn shift_now(&self, n: u32, unit: char) -> Option<i64> {
        let now = chrono::DateTime::from_timestamp_millis(self.now_ms)?.naive_utc();
        let t = match unit {
            'h' => now - Duration::hours(n as i64),
            'd' => now - Duration::days(n as i64),
            'w' => now - Duration::weeks(n as i64),
            'm' => now.checked_sub_months(Months::new(n))?,
            'y' => now.checked_sub_months(Months::new(n.checked_mul(12)?))?,
            _ => return None,
        };
        Some(t.and_utc().timestamp_millis())
    }

    /// `date:<words>`: any date expression, which must use every word.
    fn date_operator(&self, value: &str) -> Option<(Range, String)> {
        let lower = date_words(&value.to_lowercase());
        let words: Vec<&str> = lower.split_whitespace().collect();
        let today = self.tz.local_now(self.now_ms).date();
        let (n, range) = dates::parse(&words, today)?;
        if n != words.len() {
            return None;
        }
        Some((self.to_range(range), date_label(&words.join(" "), range)))
    }

    fn to_range(&self, r: dates::DateRange) -> Range {
        Range {
            after: r.from.map(|d| self.tz.midnight_ms(d)),
            before: r.to.map(|d| self.tz.midnight_ms(d)),
        }
    }
}

/// True when `phrase` (typed without `date:`) would read as a date inside
/// `date:`: every word forms one date expression ("february", "last spring",
/// "feb 10", "since march"). Used for the "Use 'february' as a date?" hint.
pub fn is_date_like(phrase: &str, now_ms: i64) -> bool {
    let lower = phrase.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let today = Tz::Local.local_now(now_ms).date();
    !words.is_empty() && dates::parse(&words, today).is_some_and(|(n, _)| n == words.len())
}

/// A suggestion to turn a date-like run of plain words into `date:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateHint {
    /// Exact substring of the query to replace.
    pub raw: String,
    /// Replacement, e.g. `date:february` or `date:"last spring"`.
    pub rewrite: String,
}

/// The first (longest) run of up to six plain words in `input` that reads as
/// a date. Operators, quoted text and exclusions are skipped, and so are
/// things that are usually not meant as dates: "may" alone (a verb) and
/// numbers alone ("2025" is as likely an invoice number).
pub fn date_hint(input: &str, now_ms: i64) -> Option<DateHint> {
    let today = Tz::Local.local_now(now_ms).date();
    date_hint_at(input, today)
}

fn date_hint_at(input: &str, today: NaiveDate) -> Option<DateHint> {
    // Plain words with byte ranges; anything else breaks a run.
    let mut words: Vec<Option<(usize, usize, String)>> = Vec::new();
    let mut i = 0;
    for part in input.split_whitespace() {
        let start = input[i..].find(part).map(|p| p + i)?;
        let end = start + part.len();
        i = end;
        let plain =
            !part.contains(':') && !part.contains('"') && !part.starts_with('-') && part != "OR";
        words.push(plain.then(|| (start, end, part.to_lowercase())));
    }
    for s0 in 0..words.len() {
        let run: Vec<&(usize, usize, String)> = words[s0..]
            .iter()
            .take(6)
            .map_while(|w| w.as_ref())
            .collect();
        for len in (1..=run.len()).rev() {
            let ws: Vec<&str> = run[..len].iter().map(|w| w.2.as_str()).collect();
            if len == 1 && (ws[0] == "may" || ws[0].chars().any(|c| c.is_ascii_digit())) {
                continue;
            }
            if dates::parse(&ws, today).is_some_and(|(n, _)| n == len) {
                let raw = input[run[0].0..run[len - 1].1].to_string();
                let value = ws.join(" ");
                let rewrite = if len == 1 {
                    format!("date:{value}")
                } else {
                    format!("date:\"{value}\"")
                };
                return Some(DateHint { raw, rewrite });
            }
        }
    }
    None
}

/// Label of the chip for a `date:` value we couldn't read.
pub const DATE_ERROR: &str = "Couldn't read that date";

/// "Last spring · Mar 1 – May 31, 2026", "Since feb · since Feb 1, 2026".
fn date_label(text: &str, r: dates::DateRange) -> String {
    let mut c = text.chars();
    let title = c
        .next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default();
    match (r.from, r.to) {
        (Some(a), Some(b)) => format!("{title} \u{b7} {}", fmt_span(a, b)),
        (Some(a), None) => format!("{title} \u{b7} since {}", fmt_date(a)),
        (None, Some(b)) => format!("{title} \u{b7} before {}", fmt_date(b)),
        (None, None) => title,
    }
}

/// Words that carry no identifying signal in an email search ("the pdf mike
/// sent about the lease"). Completed noise words are not required when the
/// query has other content; alone (or while still being typed) they stay.
const NOISE: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "of",
    "to",
    "in",
    "on",
    "at",
    "for",
    "from",
    "with",
    "about",
    "by",
    "that",
    "this",
    "these",
    "those",
    "is",
    "was",
    "were",
    "be",
    "been",
    "it",
    "its",
    "my",
    "me",
    "i",
    "we",
    "our",
    "us",
    "you",
    "your",
    "he",
    "she",
    "they",
    "them",
    "his",
    "her",
    "their",
    "some",
    "any",
    "sent",
    "send",
    "email",
    "emails",
    "mail",
    "message",
    "messages",
    "regarding",
    "re",
    "fw",
    "fwd",
    "who",
    "what",
    "when",
    "where",
    "which",
    "did",
    "do",
    "does",
    "had",
    "has",
    "have",
    "got",
    "get",
    "there",
    "where's",
    "whats",
];

pub fn is_noise_word(w: &str) -> bool {
    NOISE.contains(&w.to_lowercase().as_str())
}

/// Reply and forward prefixes as mail clients write them, in several
/// languages (RE, FW, FWD; German AW/WG, Nordic SV/VS, Spanish RV, French
/// TR, Portuguese RES/ENC, Dutch ANTW, Polish ODP).
const SUBJECT_PREFIXES: &[&str] = &[
    "re", "fw", "fwd", "aw", "wg", "sv", "vs", "rv", "tr", "res", "enc", "antw", "odp",
];

/// Parts of a pasted subject line or link that aren't what the mail says:
/// "RE:", "Fwd:", "AW:", a gateway tag like "[External]", a bare "https://".
fn is_subject_decoration(text: &str) -> bool {
    let t = text.trim();
    if t.starts_with('[') && t.ends_with(']') {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    if let Some(p) = lower.strip_suffix(':') {
        if SUBJECT_PREFIXES.contains(&p) {
            return true;
        }
    }
    matches!(
        lower.trim_end_matches('/'),
        "http:" | "https:" | "mailto:" | "www." | "http" | "https"
    )
}

fn drop_noise_words(q: &mut ParsedQuery) {
    // A noise word still being typed counts too: "what did grace send"
    // shouldn't require "send*" (the next keystroke would narrow it anyway).
    let is_noise = |c: &Clause| matches!(c.terms.as_slice(), [Term { negated: false, atom: Atom::Text { text, phrase: false, .. } }] if is_noise_word(text) || is_subject_decoration(text));
    let content = q.clauses.iter().filter(|c| !is_noise(c)).count();
    let other = q.after.is_some() || q.before.is_some() || !q.accounts.is_empty();
    if content > 0 || other {
        q.clauses.retain(|c| !is_noise(c));
    }
}

/// `type:event`, `is:event`, `in:calendar` and their plurals.
fn is_event_kind(value: &str, key: &str) -> bool {
    match key {
        "in" => value == "calendar",
        _ => matches!(
            value,
            "event" | "events" | "meeting" | "meetings" | "calendar"
        ),
    }
}

/// Every operator key the parser recognizes (aliases included). Keep in
/// sync with the UI's `features/search/query.ts` and docs/SEARCH.md.
pub const OPERATOR_KEYS: &[&str] = &[
    "from",
    "to",
    "cc",
    "bcc",
    "with",
    "participant",
    "domain",
    "subject",
    "has",
    "in",
    "folder",
    "is",
    "label",
    "category",
    "filename",
    "before",
    "after",
    "since",
    "until",
    "on",
    "older_than",
    "newer_than",
    "older",
    "newer",
    "account",
    "date",
    "type",
    "larger",
    "smaller",
    "size",
    "messages",
    "day",
    "deliveredto",
    "participants",
    "attachment",
    "attachmentnames",
    "hasattachment",
    "hasattachments",
];

fn has_kind(v: &str) -> Option<HasKind> {
    Some(match v {
        "attachment" | "attachments" | "file" | "files" => HasKind::Attachment,
        "pdf" | "pdfs" => HasKind::Pdf,
        "image" | "images" | "photo" | "photos" | "picture" | "pictures" => HasKind::Image,
        "doc" | "docs" | "document" | "documents" | "word" => HasKind::Doc,
        "spreadsheet" | "spreadsheets" | "sheet" | "sheets" | "excel" | "xls" => {
            HasKind::Spreadsheet
        }
        "presentation" | "presentations" | "slides" | "ppt" | "powerpoint" => HasKind::Presentation,
        "invite" | "invites" | "invitation" | "invitations" | "ics" => HasKind::Invite,
        "link" | "links" | "url" | "urls" => HasKind::Link,
        "otp" | "code" | "codes" | "2fa" | "verification" => HasKind::Otp,
        "unsubscribe" | "list-unsubscribe" => HasKind::Unsubscribe,
        _ => return None,
    })
}

fn has_label(k: HasKind) -> &'static str {
    match k {
        HasKind::Attachment => "Has attachment",
        HasKind::Pdf => "Has PDF",
        HasKind::Image => "Has image",
        HasKind::Doc => "Has document",
        HasKind::Spreadsheet => "Has spreadsheet",
        HasKind::Presentation => "Has presentation",
        HasKind::Invite => "Has invitation",
        HasKind::Link => "Has link",
        HasKind::Otp => "Has verification code",
        HasKind::Unsubscribe => "Has unsubscribe link",
    }
}

fn state(v: &str) -> Option<State> {
    let v = v.replace('_', "-");
    Some(match v.as_str() {
        "unread" => State::Unread,
        "read" => State::Read,
        "starred" => State::Starred,
        "unstarred" => State::Unstarred,
        "important" => State::Important,
        "snoozed" => State::Snoozed,
        "newsletter" | "newsletters" | "bulk" | "list" | "mailing-list" => State::Newsletter,
        "new-sender" | "newsender" | "new-senders" | "first-contact" => State::NewSender,
        "first-outbound" | "first-sent" | "new-recipient" | "new-recipients" => {
            State::FirstOutbound
        }
        "known-sender" | "known" | "known-senders" | "people-you-know" => State::KnownSender,
        "unanswered" | "unreplied" | "needs-reply" => State::Unanswered,
        "replied" | "answered" => State::Replied,
        "awaiting" | "awaiting-reply" | "waiting" | "no-reply" => State::Awaiting,
        "reply" | "replies" => State::Reply,
        _ => return None,
    })
}

fn state_label(st: State) -> &'static str {
    match st {
        State::Unread => "Unread",
        State::Read => "Read",
        State::Starred => "Starred",
        State::Unstarred => "Not starred",
        State::Important => "Important",
        State::Snoozed => "Snoozed",
        State::Newsletter => "Newsletter",
        State::NewSender => "New sender",
        State::FirstOutbound => "First email to them",
        State::KnownSender => "Someone you've written to",
        State::Unanswered => "Unanswered",
        State::Replied => "Replied",
        State::Awaiting => "Awaiting reply",
        State::Reply => "A reply",
    }
}

/// `5M`, `200k`, `1.5mb`, `300000` (bytes, like Gmail).
fn parse_bytes(v: &str) -> Option<u64> {
    let v = v.trim();
    let split = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    let (num, unit) = v.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mult: f64 = match unit.trim() {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = n * mult;
    (0.0..1e15).contains(&bytes).then_some(bytes as u64)
}

fn fmt_bytes(b: u64) -> String {
    let (n, unit) = if b >= 1 << 30 {
        (b as f64 / (1u64 << 30) as f64, "GB")
    } else if b >= 1 << 20 {
        (b as f64 / (1u64 << 20) as f64, "MB")
    } else if b >= 1 << 10 {
        (b as f64 / 1024.0, "KB")
    } else {
        return format!("{b} bytes");
    };
    if n.fract() == 0.0 {
        format!("{n:.0} {unit}")
    } else {
        format!("{n:.1} {unit}")
    }
}

/// larger:5M, smaller:100k, size:5M (= larger), size:>5M, size:<1M.
fn size_value(key: &str, v: &str) -> Option<(Atom, String)> {
    let (larger, v) = match key {
        "larger" => (true, v),
        "smaller" => (false, v),
        _ => match v.strip_prefix('<') {
            Some(rest) => (false, rest),
            None => (true, v.strip_prefix('>').unwrap_or(v)),
        },
    };
    let v = v.strip_prefix('=').unwrap_or(v);
    let b = parse_bytes(v)?;
    Some(if larger {
        (
            Atom::Size {
                min: Some(b),
                max: None,
            },
            format!("Larger than {}", fmt_bytes(b)),
        )
    } else {
        (
            Atom::Size {
                min: None,
                max: Some(b),
            },
            format!("Smaller than {}", fmt_bytes(b)),
        )
    })
}

/// messages:>5, >=5, <3, <=3, 5, 5+, 2..4 → inclusive (min, max).
fn thread_len(v: &str) -> Option<(u32, Option<u32>)> {
    let num = |s: &str| s.trim().parse::<u32>().ok().filter(|n| *n <= 100_000);
    if let Some((a, b)) = v.split_once("..") {
        let (a, b) = (num(a)?, num(b)?);
        return (a <= b).then_some((a, Some(b)));
    }
    if let Some(r) = v.strip_prefix(">=") {
        return Some((num(r)?, None));
    }
    if let Some(r) = v.strip_prefix("<=") {
        return Some((0, Some(num(r)?)));
    }
    if let Some(r) = v.strip_prefix('>') {
        return Some((num(r)?.checked_add(1)?, None));
    }
    if let Some(r) = v.strip_prefix('<') {
        return Some((0, Some(num(r)?.checked_sub(1)?)));
    }
    if let Some(r) = v.strip_suffix('+') {
        return Some((num(r)?, None));
    }
    let n = num(v.strip_prefix('=').unwrap_or(v))?;
    Some((n, Some(n)))
}

fn thread_len_label(min: u32, max: Option<u32>) -> String {
    match max {
        None => format!("Threads of {min}+ messages"),
        Some(m) if m == min => format!(
            "Threads of {min} message{}",
            if min == 1 { "" } else { "s" }
        ),
        Some(m) if min <= 1 => format!("Threads of up to {m} messages"),
        Some(m) => format!("Threads of {min}\u{2013}{m} messages"),
    }
}

/// day:monday, day:mon,wed, day:weekend, day:weekday → (bitmask, label).
fn weekdays(v: &str) -> Option<(u8, String)> {
    const NAMES: [&str; 7] = [
        "Sundays",
        "Mondays",
        "Tuesdays",
        "Wednesdays",
        "Thursdays",
        "Fridays",
        "Saturdays",
    ];
    match v {
        "weekend" | "weekends" => return Some((0b100_0001, "On weekends".into())),
        "weekday" | "weekdays" | "workday" | "workdays" => {
            return Some((0b011_1110, "On weekdays".into()))
        }
        _ => {}
    }
    let mut mask = 0u8;
    let mut names = Vec::new();
    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let p = part.trim_end_matches('s');
        let d = match p {
            "sun" | "sunday" => 0,
            "mon" | "monday" => 1,
            "tue" | "tues" | "tuesday" => 2,
            "wed" | "weds" | "wednesday" => 3,
            "thu" | "thur" | "thurs" | "thursday" => 4,
            "fri" | "friday" => 5,
            "sat" | "saturday" => 6,
            _ => return None,
        };
        if mask & (1 << d) == 0 {
            names.push(NAMES[d]);
        }
        mask |= 1 << d;
    }
    (mask != 0).then(|| (mask, format!("On {}", names.join(", "))))
}

/// A `date:` value as words for dates.rs: `aug1..aug15` → `aug 1 to aug 15`
/// (`..` is a range, a letter followed by a digit is two words).
fn date_words(v: &str) -> String {
    let v = v.replace("..", " to ");
    let mut out = String::with_capacity(v.len() + 4);
    let mut prev: Option<char> = None;
    for c in v.chars() {
        if c.is_ascii_digit() && prev.is_some_and(|p| p.is_alphabetic()) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

type Items = std::iter::Peekable<std::vec::IntoIter<Item>>;

/// An implicit-AND sequence, up to a `)` (consumed) or the end. `a OR b`
/// joins the adjacent units; anything global (a date, account:, AND)
/// breaks an OR chain.
fn sequence(items: &mut Items) -> Vec<Expr> {
    let mut out: Vec<Expr> = Vec::new();
    let mut last_was_unit = false;
    let mut pending_or = false;
    while let Some(item) = items.next() {
        let unit = match item {
            Item::Close => break,
            Item::Global => {
                last_was_unit = false;
                pending_or = false;
                continue;
            }
            Item::Or => {
                pending_or = last_was_unit;
                continue;
            }
            Item::Term(t) => Expr::Lit(t),
            Item::Around(_) => continue,
            Item::Open { negated, any } => {
                let inner = sequence(items);
                if inner.is_empty() {
                    continue;
                }
                let group = if any { Expr::Or(inner) } else { Expr::And(inner) };
                if negated {
                    Expr::Not(Box::new(group))
                } else {
                    group
                }
            }
        };
        if pending_or && last_was_unit {
            match out.last_mut() {
                Some(Expr::Or(alts)) => alts.push(unit),
                Some(last) => {
                    let prev = std::mem::replace(last, Expr::And(Vec::new()));
                    *last = Expr::Or(vec![prev, unit]);
                }
                None => out.push(unit),
            }
        } else {
            out.push(unit);
        }
        pending_or = false;
        last_was_unit = true;
    }
    out
}

/// Conjunctive normal form (a list of OR clauses), with `negate` pushed down
/// to the terms (De Morgan). None when the expansion exceeds `MAX_CNF`
/// clauses. An empty sub-expression (only dates or noise) is left out of a
/// disjunction rather than making it always true.
fn cnf(e: Expr, negate: bool) -> Option<Vec<Vec<Term>>> {
    match e {
        Expr::Lit(mut t) => {
            t.negated ^= negate;
            Some(vec![vec![t]])
        }
        Expr::Not(x) => cnf(*x, !negate),
        Expr::And(xs) if !negate => conjunction(xs, negate),
        Expr::Or(xs) if negate => conjunction(xs, negate),
        Expr::And(xs) | Expr::Or(xs) => {
            let mut acc: Vec<Vec<Term>> = Vec::new();
            for x in xs {
                let cx = cnf(x, negate)?;
                if cx.is_empty() {
                    continue;
                }
                if acc.is_empty() {
                    acc = cx;
                    continue;
                }
                if acc.len() * cx.len() > MAX_CNF {
                    return None;
                }
                let mut next = Vec::with_capacity(acc.len() * cx.len());
                for a in &acc {
                    for b in &cx {
                        let mut c = a.clone();
                        c.extend(b.iter().cloned());
                        next.push(c);
                    }
                }
                acc = next;
            }
            Some(acc)
        }
    }
}

fn conjunction(xs: Vec<Expr>, negate: bool) -> Option<Vec<Vec<Term>>> {
    let mut out = Vec::new();
    for x in xs {
        out.extend(cnf(x, negate)?);
    }
    Some(out)
}

fn chip(kind: &str, label: String, raw: &str) -> SearchChip {
    SearchChip {
        kind: kind.to_string(),
        label,
        raw: raw.to_string(),
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn apply_range(q: &mut ParsedQuery, r: Range) {
    if let Some(a) = r.after {
        q.after = Some(q.after.map_or(a, |x| x.max(a)));
    }
    if let Some(b) = r.before {
        q.before = Some(q.before.map_or(b, |x| x.min(b)));
    }
}

fn is_complete_email(v: &str) -> bool {
    v.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
    })
}

/// True when `s` contains at least one indexable token (FTS would otherwise
/// see an empty phrase).
pub fn has_tokens(s: &str) -> bool {
    s.chars().any(|c| c.is_alphanumeric())
}

fn folder(v: &str) -> Option<(Folder, &'static str)> {
    Some(match v {
        "inbox" => (Folder::Inbox, "In Inbox"),
        "sent" => (Folder::Sent, "In Sent"),
        "draft" | "drafts" => (Folder::Drafts, "In Drafts"),
        "trash" | "bin" => (Folder::Trash, "In Trash"),
        "spam" | "junk" => (Folder::Spam, "In Spam"),
        "anywhere" | "all" => (Folder::Anywhere, "Anywhere, incl. Trash & Spam"),
        "done" | "archive" | "archived" => (Folder::Done, "Done"),
        "starred" => (Folder::Starred, "Starred"),
        "important" => (Folder::Important, "Important"),
        _ => return None,
    })
}

fn parse_relative(v: &str) -> Option<(u32, char)> {
    let unit = v.chars().last()?;
    let n: u32 = v[..v.len() - unit.len_utf8()].parse().ok()?;
    if n == 0 || n > 10_000 || !matches!(unit, 'h' | 'd' | 'w' | 'm' | 'y') {
        return None;
    }
    Some((n, unit))
}

fn describe_span(n: u32, unit: char) -> String {
    let word = match unit {
        'h' => "hour",
        'd' => "day",
        'w' => "week",
        'm' => "month",
        _ => "year",
    };
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Dates for before:/after: — YYYY-MM-DD, YYYY/MM/DD, YYYY-MM, YYYY, M/D/YYYY, or a month name.
fn parse_date_value(v: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = v.split(['-', '/', '.']).collect();
    let num = |s: &str| s.parse::<u32>().ok();
    match parts.as_slice() {
        [y] if y.len() == 4 => ymd(year_num(y)?, 1, 1),
        [y, m] if y.len() == 4 => ymd(year_num(y)?, num(m)?, 1),
        [y, m, d] if y.len() == 4 => ymd(year_num(y)?, num(m)?, num(d)?),
        [a, b, y] if y.len() == 4 || (y.len() == 2 && a.len() <= 2) => {
            dates::numeric_day(v.contains('.'), num(a)?, num(b)?, y)
        }
        _ => None,
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn fmt_date(d: NaiveDate) -> String {
    format!("{} {}, {}", MONTHS[d.month0() as usize], d.day(), d.year())
}

/// Human span for [a, b): "Mar 1 – May 31, 2026".
fn fmt_span(a: NaiveDate, b: NaiveDate) -> String {
    let last = b - Duration::days(1);
    if last <= a {
        return fmt_date(a);
    }
    let mon = |d: NaiveDate| MONTHS[d.month0() as usize];
    if a.year() != last.year() {
        format!("{} \u{2013} {}", fmt_date(a), fmt_date(last))
    } else if a.month() == last.month() {
        format!(
            "{} {} \u{2013} {}, {}",
            mon(a),
            a.day(),
            last.day(),
            a.year()
        )
    } else {
        format!("{} {} \u{2013} {}", mon(a), a.day(), fmt_date(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Wed 2026-09-23 12:00:00 UTC.
    const NOW: i64 = 1_790_164_800_000;

    fn p(s: &str) -> ParsedQuery {
        parse_with_offset(s, NOW, 0)
    }

    fn day(y: i32, m: u32, d: u32) -> i64 {
        ymd(y, m, d)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    fn only_term(q: &ParsedQuery) -> &Term {
        assert_eq!(q.clauses.len(), 1, "{q:?}");
        assert_eq!(q.clauses[0].terms.len(), 1);
        &q.clauses[0].terms[0]
    }

    #[test]
    fn now_is_what_we_think() {
        assert_eq!(
            chrono::DateTime::from_timestamp_millis(NOW)
                .unwrap()
                .to_rfc3339(),
            "2026-09-23T12:00:00+00:00"
        );
    }

    #[test]
    fn plain_words_and_prefix() {
        let q = p("lease renew");
        assert_eq!(q.clauses.len(), 2);
        assert!(q.chips.is_empty());
        assert_eq!(
            q.clauses[0].terms[0].atom,
            Atom::Text {
                text: "lease".into(),
                phrase: false,
                prefix: false
            }
        );
        assert_eq!(
            q.clauses[1].terms[0].atom,
            Atom::Text {
                text: "renew".into(),
                phrase: false,
                prefix: true
            }
        );
        let q = p("lease renew ");
        assert_eq!(
            q.clauses[1].terms[0].atom,
            Atom::Text {
                text: "renew".into(),
                phrase: false,
                prefix: false
            }
        );
    }

    #[test]
    fn quoted_phrase_and_unterminated() {
        let q = p("\"lease renewal\" x");
        assert_eq!(
            q.clauses[0].terms[0].atom,
            Atom::Text {
                text: "lease renewal".into(),
                phrase: true,
                prefix: false
            }
        );
        assert_eq!(q.chips[0].kind, "text");
        assert_eq!(q.chips[0].raw, "\"lease renewal\"");
        let q = p("\"lease ren");
        assert_eq!(
            only_term(&q).atom,
            Atom::Text {
                text: "lease ren".into(),
                phrase: true,
                prefix: false
            }
        );
        assert_eq!(q.chips[0].raw, "\"lease ren");
    }

    #[test]
    fn exclusion() {
        let q = p("invoice -draft -\"old stuff\" -from:bob");
        assert_eq!(q.clauses.len(), 4);
        assert!(q.clauses[1].terms[0].negated);
        assert!(q.clauses[2].terms[0].negated);
        assert_eq!(
            q.clauses[3].terms[0].atom,
            Atom::Field {
                field: Field::From,
                value: "bob".into(),
                prefix: false
            }
        );
        let kinds: Vec<_> = q
            .chips
            .iter()
            .map(|c| (c.kind.as_str(), c.raw.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("exclude", "-draft"),
                ("exclude", "-\"old stuff\""),
                ("exclude", "-from:bob")
            ]
        );
        assert_eq!(q.chips[2].label, "Not from bob");
        // A lone dash is not an exclusion.
        assert!(p("x - y").clauses.len() == 2);
    }

    #[test]
    fn or_groups() {
        let q = p("from:ana OR from:bob invoice");
        assert_eq!(q.clauses.len(), 2);
        assert_eq!(q.clauses[0].terms.len(), 2);
        let q = p("pdf OR docx OR xlsx");
        assert_eq!(q.clauses.len(), 1);
        assert_eq!(q.clauses[0].terms.len(), 3);
        // Dangling / leading OR is ignored.
        assert_eq!(p("OR x OR").clauses.len(), 1);
        // lowercase "or" is a (noise) word, not the operator.
        let q = p("x or y");
        assert_eq!(q.clauses.len(), 2);
        assert!(q.clauses.iter().all(|c| c.terms.len() == 1));
    }

    #[test]
    fn field_operators() {
        let q = p("from:mike@acme.example to:ana cc:\"Ops Team\" bcc:x subject:\"q3 plan\" filename:budget");
        let atoms: Vec<_> = q.clauses.iter().map(|c| c.terms[0].atom.clone()).collect();
        assert_eq!(
            atoms[0],
            Atom::Field {
                field: Field::From,
                value: "mike@acme.example".into(),
                prefix: false
            }
        );
        assert_eq!(
            atoms[1],
            Atom::Field {
                field: Field::To,
                value: "ana".into(),
                prefix: false
            }
        );
        assert_eq!(
            atoms[2],
            Atom::Field {
                field: Field::Cc,
                value: "Ops Team".into(),
                prefix: false
            }
        );
        assert_eq!(
            atoms[3],
            Atom::Field {
                field: Field::To,
                value: "x".into(),
                prefix: false
            }
        );
        assert_eq!(
            atoms[4],
            Atom::Field {
                field: Field::Subject,
                value: "q3 plan".into(),
                prefix: false
            }
        );
        assert_eq!(atoms[5], Atom::Filename("budget".into()));
        let raws: Vec<_> = q.chips.iter().map(|c| c.raw.as_str()).collect();
        assert_eq!(
            raws,
            vec![
                "from:mike@acme.example",
                "to:ana",
                "cc:\"Ops Team\"",
                "bcc:x",
                "subject:\"q3 plan\"",
                "filename:budget"
            ]
        );
        assert_eq!(q.chips[0].label, "From mike@acme.example");
        assert_eq!(q.chips[4].label, "Subject \u{201c}q3 plan\u{201d}");
    }

    #[test]
    fn operator_value_prefix_while_typing() {
        let q = p("from:mik");
        assert_eq!(
            only_term(&q).atom,
            Atom::Field {
                field: Field::From,
                value: "mik".into(),
                prefix: true
            }
        );
        let q = p("from:mike@acme.example");
        assert_eq!(
            only_term(&q).atom,
            Atom::Field {
                field: Field::From,
                value: "mike@acme.example".into(),
                prefix: false
            }
        );
        let q = p("from:mike@acme");
        assert_eq!(
            only_term(&q).atom,
            Atom::Field {
                field: Field::From,
                value: "mike@acme".into(),
                prefix: true
            }
        );
        let q = p("from:");
        assert!(q.clauses.is_empty());
        assert!(q.chips.is_empty());
        let q = p("FROM:Ana ");
        assert_eq!(
            only_term(&q).atom,
            Atom::Field {
                field: Field::From,
                value: "Ana".into(),
                prefix: false
            }
        );
    }

    #[test]
    fn calendar_operators() {
        for q in [
            "type:event",
            "type:events",
            "is:event",
            "in:calendar",
            "type:meeting",
        ] {
            let r = p(q);
            assert!(r.events_only, "{q}");
            assert!(r.clauses.is_empty(), "{q}");
            assert_eq!(r.chips.len(), 1, "{q}");
            assert_eq!(r.chips[0].kind, "type");
            assert_eq!(r.chips[0].raw, q);
        }
        // Unknown values and negation stay plain text.
        for q in ["type:banana", "-type:event", "in:calendars"] {
            let r = p(q);
            assert!(!r.events_only, "{q}");
            assert!(r.chips.iter().all(|c| c.kind != "type"), "{q}");
        }
        // is:unread still works next to it.
        let r = p("is:event is:unread");
        assert!(r.events_only);
        assert_eq!(r.clauses[0].terms[0].atom, Atom::Is(State::Unread));

        for q in ["has:invite", "has:invitations", "has:ics"] {
            let r = p(q);
            assert_eq!(
                r.clauses[0].terms[0].atom,
                Atom::Has(HasKind::Invite),
                "{q}"
            );
            assert_eq!(r.chips[0].label, "Has invitation");
        }
        let r = p("-has:invite");
        assert!(r.clauses[0].terms[0].negated);
    }

    #[test]
    fn has_in_is_label() {
        let q = p("has:attachment has:pdf has:image has:doc has:spreadsheet has:presentation");
        let has: Vec<_> = q.clauses.iter().map(|c| c.terms[0].atom.clone()).collect();
        assert_eq!(
            has,
            vec![
                Atom::Has(HasKind::Attachment),
                Atom::Has(HasKind::Pdf),
                Atom::Has(HasKind::Image),
                Atom::Has(HasKind::Doc),
                Atom::Has(HasKind::Spreadsheet),
                Atom::Has(HasKind::Presentation)
            ]
        );
        assert_eq!(q.chips[1].label, "Has PDF");
        for (v, f) in [
            ("inbox", Folder::Inbox),
            ("sent", Folder::Sent),
            ("drafts", Folder::Drafts),
            ("trash", Folder::Trash),
            ("spam", Folder::Spam),
            ("anywhere", Folder::Anywhere),
            ("done", Folder::Done),
            ("starred", Folder::Starred),
        ] {
            assert_eq!(only_term(&p(&format!("in:{v}"))).atom, Atom::In(f), "{v}");
        }
        for (v, s) in [
            ("unread", State::Unread),
            ("read", State::Read),
            ("starred", State::Starred),
            ("important", State::Important),
        ] {
            assert_eq!(only_term(&p(&format!("is:{v}"))).atom, Atom::Is(s), "{v}");
        }
        assert_eq!(
            only_term(&p("label:Clients")).atom,
            Atom::Label("Clients".into())
        );
        assert_eq!(
            only_term(&p("label:\"Q3 Deals\"")).atom,
            Atom::Label("Q3 Deals".into())
        );
        assert_eq!(only_term(&p("label:inbox")).atom, Atom::In(Folder::Inbox));
        // Unknown values stay text.
        assert!(matches!(
            only_term(&p("has:unicorn")).atom,
            Atom::Text { .. }
        ));
        assert!(matches!(only_term(&p("is:shiny")).atom, Atom::Text { .. }));
        // Unknown operators stay text.
        assert!(matches!(only_term(&p("ref:1234")).atom, Atom::Text { .. }));
    }

    #[test]
    fn scope_flags() {
        assert!(!p("x").include_trash);
        let q = p("x in:trash");
        assert!(q.include_trash && !q.include_spam);
        let q = p("x in:anywhere");
        assert!(q.include_trash && q.include_spam);
        assert!(!p("-in:trash").include_trash);
    }

    #[test]
    fn before_after_formats() {
        let q = p("before:2026-03-01 after:2025/12/15");
        assert_eq!(q.before, Some(day(2026, 3, 1)));
        assert_eq!(q.after, Some(day(2025, 12, 15)));
        assert_eq!(q.chips[0].label, "Before Mar 1, 2026");
        assert_eq!(q.chips[1].label, "After Dec 15, 2025");
        assert_eq!(p("before:2025").before, Some(day(2025, 1, 1)));
        assert_eq!(p("after:2025-06").after, Some(day(2025, 6, 1)));
        assert_eq!(p("before:3/15/2026").before, Some(day(2026, 3, 15)));
        // Invalid dates stay text.
        assert!(p("before:banana").before.is_none());
        assert!(p("before:2026-13-01").before.is_none());
        // Timezone: local midnight in UTC-7.
        let q = parse_with_offset("after:2026-03-01", NOW, -7 * 3600);
        assert_eq!(q.after, Some(day(2026, 3, 1) + 7 * 3600 * 1000));
        // Intersection.
        let q = p("after:2026-01-01 after:2026-02-01 before:2026-06-01 before:2026-05-01");
        assert_eq!(
            (q.after, q.before),
            (Some(day(2026, 2, 1)), Some(day(2026, 5, 1)))
        );
        assert!(p("after:2026-06-01 before:2026-01-01").empty_range());
    }

    #[test]
    fn older_newer() {
        let q = p("older_than:3d");
        assert_eq!(q.before, Some(NOW - 3 * 86_400_000));
        assert_eq!(q.chips[0].label, "Older than 3 days");
        let q = p("newer_than:1y");
        assert_eq!(q.after, Some(day(2025, 9, 23) + 12 * 3_600_000));
        let q = p("newer_than:2m");
        assert_eq!(q.after, Some(day(2026, 7, 23) + 12 * 3_600_000));
        assert_eq!(p("newer_than:2w").after, Some(NOW - 14 * 86_400_000));
        assert!(p("newer_than:abc").after.is_none());
    }

    /// `date:<s>`: bare when a single token, quoted otherwise. Strings that
    /// already contain `date:` pass through unchanged.
    fn dq(s: &str) -> String {
        if s.contains("date:") {
            s.to_string()
        } else if s.contains(char::is_whitespace) {
            format!("date:\"{s}\"")
        } else {
            format!("date:{s}")
        }
    }

    /// Assert `date:<s>` is an error chip that constrains nothing.
    fn bad_date(s: &str) {
        let q = p(&dq(s));
        assert!(q.after.is_none() && q.before.is_none(), "{s}");
        assert_eq!(q.chips.len(), 1, "{s}");
        assert_eq!(q.chips[0].kind, "error", "{s}");
        assert_eq!(q.chips[0].label, DATE_ERROR);
        assert_eq!(q.chips[0].raw, dq(s));
    }

    fn range(s: &str) -> (Option<i64>, Option<i64>, String, String) {
        let q = p(&dq(s));
        let c = q.chips.first().cloned().unwrap_or(SearchChip {
            kind: String::new(),
            label: String::new(),
            raw: String::new(),
        });
        (q.after, q.before, c.raw, c.label)
    }

    #[test]
    fn date_values_simple() {
        assert_eq!(range("today").0, Some(day(2026, 9, 23)));
        assert_eq!(range("today").1, Some(day(2026, 9, 24)));
        assert_eq!(range("yesterday").0, Some(day(2026, 9, 22)));
        assert_eq!(range("yesterday").1, Some(day(2026, 9, 23)));
        // 2026-09-23 is a Wednesday; week starts Monday 09-21.
        assert_eq!(range("this week").0, Some(day(2026, 9, 21)));
        assert_eq!(range("this week").1, Some(day(2026, 9, 28)));
        assert_eq!(range("last week").0, Some(day(2026, 9, 14)));
        assert_eq!(range("last week").1, Some(day(2026, 9, 21)));
        assert_eq!(
            range("past week"),
            (
                Some(day(2026, 9, 16)),
                None,
                "date:\"past week\"".into(),
                "Past week \u{b7} since Sep 16, 2026".into()
            )
        );
        assert_eq!(range("this month").0, Some(day(2026, 9, 1)));
        assert_eq!(range("this month").1, Some(day(2026, 10, 1)));
        assert_eq!(range("last month").0, Some(day(2026, 8, 1)));
        assert_eq!(range("last month").1, Some(day(2026, 9, 1)));
        assert_eq!(range("past month").0, Some(day(2026, 8, 23)));
        assert_eq!(range("this year").0, Some(day(2026, 1, 1)));
        assert_eq!(range("this year").1, Some(day(2027, 1, 1)));
        assert_eq!(range("last year").0, Some(day(2025, 1, 1)));
        assert_eq!(range("last year").1, Some(day(2026, 1, 1)));
        assert_eq!(range("past year").0, Some(day(2025, 9, 23)));
        assert_eq!(range("past 12 months").0, Some(day(2025, 9, 23)));
    }

    #[test]
    fn date_values_seasons() {
        let (a, b, raw, label) = range("last spring");
        assert_eq!((a, b), (Some(day(2026, 3, 1)), Some(day(2026, 6, 1))));
        assert_eq!(raw, "date:\"last spring\"");
        assert_eq!(label, "Last spring \u{b7} Mar 1 \u{2013} May 31, 2026");
        // Today is in early fall: last summer is Jun-Aug 2026, last fall is 2025.
        assert_eq!(range("last summer").0, Some(day(2026, 6, 1)));
        assert_eq!(range("last fall").0, Some(day(2025, 9, 1)));
        assert_eq!(range("last autumn").1, Some(day(2025, 12, 1)));
        assert_eq!(range("last winter").0, Some(day(2025, 12, 1)));
        assert_eq!(range("last winter").1, Some(day(2026, 3, 1)));
        assert_eq!(range("this fall").0, Some(day(2026, 9, 1)));
        assert_eq!(range("this summer").0, Some(day(2026, 6, 1)));
        assert_eq!(range("this winter").0, Some(day(2026, 12, 1)));
        // In January, "this winter" is the one in progress.
        let jan = day(2026, 1, 15);
        let q = parse_with_offset("date:\"this winter\"", jan, 0);
        assert_eq!(q.after, Some(day(2025, 12, 1)));
        let q = parse_with_offset("date:\"last winter\"", jan, 0);
        assert_eq!(q.after, Some(day(2024, 12, 1)));
    }

    #[test]
    fn date_values_months_and_years() {
        assert_eq!(range("in march").0, Some(day(2026, 3, 1)));
        assert_eq!(range("in march").1, Some(day(2026, 4, 1)));
        assert_eq!(range("in December").0, Some(day(2025, 12, 1)));
        assert_eq!(range("in september").0, Some(day(2026, 9, 1)));
        assert_eq!(range("last september").0, Some(day(2025, 9, 1)));
        // "last <month>" is the occurrence before the most recent one.
        assert_eq!(range("last march").0, Some(day(2025, 3, 1)));
        assert_eq!(range("march 2025").0, Some(day(2025, 3, 1)));
        assert_eq!(range("Mar 2025").1, Some(day(2025, 4, 1)));
        assert_eq!(range("in 2024").0, Some(day(2024, 1, 1)));
        assert_eq!(range("in 2024").1, Some(day(2025, 1, 1)));
        // Inside date:, a bare year is the whole year.
        assert_eq!(range("2024").0, Some(day(2024, 1, 1)));
        assert!(p("2024").after.is_none());
        assert_eq!(range("march").0, Some(day(2026, 3, 1)));
        assert!(p("in the office").after.is_none());
    }

    fn span_of(s: &str) -> (Option<i64>, Option<i64>) {
        let q = p(&dq(s));
        (q.after, q.before)
    }

    fn date_chip(s: &str) -> SearchChip {
        p(&dq(s))
            .chips
            .into_iter()
            .find(|c| c.kind == "date")
            .unwrap_or_else(|| panic!("no date chip for {s:?}"))
    }

    // NOW is Wed 2026-09-23.
    #[test]
    fn bare_month_names() {
        assert_eq!(
            span_of("february"),
            (Some(day(2026, 2, 1)), Some(day(2026, 3, 1)))
        );
        let c = date_chip("february");
        assert_eq!(c.raw, "date:february");
        assert_eq!(c.label, "February \u{b7} Feb 1 \u{2013} 28, 2026");
        // Most recent past occurrence: October hasn't come yet this year.
        assert_eq!(
            span_of("october"),
            (Some(day(2025, 10, 1)), Some(day(2025, 11, 1)))
        );
        assert_eq!(span_of("September").0, Some(day(2026, 9, 1)));
        for (w, m) in [
            ("jan", 1),
            ("feb", 2),
            ("mar", 3),
            ("apr", 4),
            ("jun", 6),
            ("jul", 7),
            ("aug", 8),
            ("sep", 9),
            ("sept", 9),
        ] {
            assert_eq!(span_of(w).0, Some(day(2026, m, 1)), "{w}");
        }
        assert_eq!(span_of("nov").0, Some(day(2025, 11, 1)));
        assert_eq!(span_of("dec").0, Some(day(2025, 12, 1)));
        // Mixed with words: date: is a chip, the rest stays text.
        let q = p("invoice date:february");
        assert_eq!(
            q.text_terms().map(|t| t.0).collect::<Vec<_>>(),
            vec!["invoice"]
        );
        assert!(q.after.is_some());
    }

    #[test]
    fn bare_date_words_are_text() {
        for s in [
            "february",
            "feb",
            "tuesday",
            "today",
            "yesterday",
            "last week",
            "last spring",
            "this year",
            "feb 10",
            "late february",
            "since march",
            "in may",
            "2 weeks ago",
            "the week of feb 10",
            "between jan and march",
            "2026-02-10",
            "2025",
        ] {
            let q = p(s);
            assert!(q.after.is_none() && q.before.is_none(), "{s}");
            assert!(q.chips.is_empty(), "{s}: {:?}", q.chips);
        }
        // "lease last spring" searches those three words.
        let q = p("lease last spring ");
        assert_eq!(
            q.text_terms().map(|t| t.0).collect::<Vec<_>>(),
            vec!["lease", "last", "spring"]
        );
        // February is searchable as a word.
        let q = p("february ");
        assert_eq!(
            q.text_terms().collect::<Vec<_>>(),
            vec![("february", false, false)]
        );
    }

    #[test]
    fn date_like_hints() {
        let today = ymd(2026, 9, 23).unwrap();
        let hint = |s: &str| date_hint_at(s, today).map(|h| (h.raw, h.rewrite));
        assert_eq!(
            hint("invoice february"),
            Some(("february".into(), "date:february".into()))
        );
        assert_eq!(
            hint("lease last spring"),
            Some(("last spring".into(), "date:\"last spring\"".into()))
        );
        assert_eq!(
            hint("receipts since  March"),
            Some(("since  March".into(), "date:\"since march\"".into()))
        );
        assert_eq!(
            hint("the week of feb 10 receipts"),
            Some((
                "the week of feb 10".into(),
                "date:\"the week of feb 10\"".into()
            ))
        );
        assert_eq!(
            hint("tuesday standup"),
            Some(("tuesday".into(), "date:tuesday".into()))
        );
        assert_eq!(
            hint("meeting may 10"),
            Some(("may 10".into(), "date:\"may 10\"".into()))
        );
        // Not suggested: "may" alone, bare numbers, operators, quotes, words.
        for s in [
            "we may need it",
            "invoice 2025",
            "INV-20417",
            "date:february",
            "\"february\"",
            "-february",
            "lease renewal",
            "",
        ] {
            assert_eq!(hint(s), None, "{s}");
        }
        assert!(is_date_like("Last Spring", NOW));
        assert!(is_date_like("may", NOW));
        assert!(!is_date_like("february banana", NOW));
        assert!(!is_date_like("", NOW));
    }

    #[test]
    fn quoting_keeps_month_as_text() {
        let q = p("\"february\"");
        assert!(q.after.is_none() && q.before.is_none());
        assert_eq!(
            q.text_terms().collect::<Vec<_>>(),
            vec![("february", true, false)]
        );
        assert!(p("\"last week\"").after.is_none());
    }

    #[test]
    fn may_inside_date_values() {
        // Alone, or used as a verb, "may" is text.
        let q = p("may");
        assert!(q.after.is_none());
        assert_eq!(q.text_terms().count(), 1);
        assert!(p("we may need the contract").after.is_none());
        assert!(p("May").after.is_none());
        // In a date context it's the month.
        assert_eq!(
            span_of("in may"),
            (Some(day(2026, 5, 1)), Some(day(2026, 6, 1)))
        );
        assert_eq!(
            span_of("may 2025"),
            (Some(day(2025, 5, 1)), Some(day(2025, 6, 1)))
        );
        assert_eq!(
            span_of("date:may"),
            (Some(day(2026, 5, 1)), Some(day(2026, 6, 1)))
        );
        assert_eq!(span_of("since may"), (Some(day(2026, 5, 1)), None));
        assert_eq!(
            span_of("early may"),
            (Some(day(2026, 5, 1)), Some(day(2026, 5, 11)))
        );
        assert_eq!(
            span_of("may 10"),
            (Some(day(2026, 5, 10)), Some(day(2026, 5, 11)))
        );
        assert_eq!(span_of("last may").0, Some(day(2025, 5, 1)));
        assert_eq!(
            span_of("between may and july"),
            (Some(day(2026, 5, 1)), Some(day(2026, 8, 1)))
        );
    }

    #[test]
    fn month_with_year_and_day() {
        assert_eq!(
            span_of("feb 2025"),
            (Some(day(2025, 2, 1)), Some(day(2025, 3, 1)))
        );
        assert_eq!(
            span_of("february '25"),
            (Some(day(2025, 2, 1)), Some(day(2025, 3, 1)))
        );
        assert_eq!(span_of("february \u{2019}25").0, Some(day(2025, 2, 1)));
        assert_eq!(date_chip("february '25").raw, "date:\"february '25\"");
        assert_eq!(
            span_of("feb 10"),
            (Some(day(2026, 2, 10)), Some(day(2026, 2, 11)))
        );
        assert_eq!(
            span_of("feb 10th"),
            (Some(day(2026, 2, 10)), Some(day(2026, 2, 11)))
        );
        assert_eq!(
            span_of("feb 10, 2025"),
            (Some(day(2025, 2, 10)), Some(day(2025, 2, 11)))
        );
        // A day later in the year than today resolves to last year.
        assert_eq!(span_of("dec 25").0, Some(day(2025, 12, 25)));
        assert_eq!(date_chip("feb 10").label, "Feb 10 \u{b7} Feb 10, 2026");
    }

    #[test]
    fn early_mid_late() {
        assert_eq!(
            span_of("early february"),
            (Some(day(2026, 2, 1)), Some(day(2026, 2, 11)))
        );
        assert_eq!(
            span_of("mid february"),
            (Some(day(2026, 2, 11)), Some(day(2026, 2, 21)))
        );
        assert_eq!(
            span_of("late february"),
            (Some(day(2026, 2, 21)), Some(day(2026, 3, 1)))
        );
        assert_eq!(
            span_of("late jan 2025"),
            (Some(day(2025, 1, 21)), Some(day(2025, 2, 1)))
        );
        assert_eq!(
            span_of("end of march"),
            (Some(day(2026, 3, 21)), Some(day(2026, 4, 1)))
        );
        assert_eq!(
            date_chip("late february").label,
            "Late february \u{b7} Feb 21 \u{2013} 28, 2026"
        );
        // Not a date: stays text.
        bad_date("late fee");
    }

    #[test]
    fn week_of() {
        // Feb 10 2026 is a Tuesday: Mon Feb 9 – Sun Feb 15.
        assert_eq!(
            span_of("the week of feb 10"),
            (Some(day(2026, 2, 9)), Some(day(2026, 2, 16)))
        );
        assert_eq!(
            date_chip("receipts date:\"the week of feb 10\"").raw,
            "date:\"the week of feb 10\""
        );
        assert_eq!(span_of("week of march 3").0, Some(day(2026, 3, 2)));
        bad_date("the week of hell");
    }

    #[test]
    fn since_before_after_between() {
        assert_eq!(span_of("since february"), (Some(day(2026, 2, 1)), None));
        assert_eq!(
            date_chip("since february").label,
            "Since february \u{b7} since Feb 1, 2026"
        );
        assert_eq!(span_of("before march"), (None, Some(day(2026, 3, 1))));
        assert_eq!(
            date_chip("before march").label,
            "Before march \u{b7} before Mar 1, 2026"
        );
        assert_eq!(span_of("after march"), (Some(day(2026, 4, 1)), None));
        assert_eq!(span_of("since last week"), (Some(day(2026, 9, 14)), None));
        assert_eq!(
            span_of("between jan and march"),
            (Some(day(2026, 1, 1)), Some(day(2026, 4, 1)))
        );
        // The start resolves relative to the end, across the year boundary.
        assert_eq!(
            span_of("between nov and feb"),
            (Some(day(2025, 11, 1)), Some(day(2026, 3, 1)))
        );
        assert_eq!(
            span_of("jan to mar"),
            (Some(day(2026, 1, 1)), Some(day(2026, 4, 1)))
        );
        // Keywords without a date after them stay text.
        for s in [
            "before the meeting",
            "since forever",
            "between us and them",
            "from mike",
        ] {
            bad_date(s);
        }
    }

    #[test]
    fn last_month_name_is_the_one_before() {
        assert_eq!(
            span_of("last february"),
            (Some(day(2025, 2, 1)), Some(day(2025, 3, 1)))
        );
        // Current month: most recent is this September, so "last" is 2025's.
        assert_eq!(span_of("last september").0, Some(day(2025, 9, 1)));
        assert_eq!(span_of("last october").0, Some(day(2024, 10, 1)));
    }

    #[test]
    fn weekdays() {
        // NOW is Wednesday Sep 23.
        assert_eq!(
            span_of("tuesday"),
            (Some(day(2026, 9, 22)), Some(day(2026, 9, 23)))
        );
        assert_eq!(span_of("monday").0, Some(day(2026, 9, 21)));
        assert_eq!(span_of("thursday").0, Some(day(2026, 9, 17)));
        // Today's weekday is today; "last" means the one before.
        assert_eq!(span_of("wednesday").0, Some(day(2026, 9, 23)));
        assert_eq!(span_of("last wednesday").0, Some(day(2026, 9, 16)));
        assert_eq!(span_of("last friday").0, Some(day(2026, 9, 18)));
        assert_eq!(date_chip("tuesday").label, "Tuesday \u{b7} Sep 22, 2026");
    }

    #[test]
    fn date_operator() {
        assert_eq!(
            span_of("date:february"),
            (Some(day(2026, 2, 1)), Some(day(2026, 3, 1)))
        );
        let c = date_chip("invoice date:\"last week\"");
        assert_eq!(c.raw, "date:\"last week\"");
        assert_eq!(c.label, "Last week \u{b7} Sep 14 \u{2013} 20, 2026");
        assert_eq!(
            span_of("date:\"last week\""),
            (Some(day(2026, 9, 14)), Some(day(2026, 9, 21)))
        );
        assert_eq!(
            span_of("date:2026-02"),
            (Some(day(2026, 2, 1)), Some(day(2026, 3, 1)))
        );
        assert_eq!(
            span_of("date:2026-02-10"),
            (Some(day(2026, 2, 10)), Some(day(2026, 2, 11)))
        );
        assert_eq!(
            span_of("date:2025"),
            (Some(day(2025, 1, 1)), Some(day(2026, 1, 1)))
        );
        assert_eq!(
            span_of("date:\"feb 10\""),
            (Some(day(2026, 2, 10)), Some(day(2026, 2, 11)))
        );
        assert_eq!(
            span_of("date:\"jan 5 to jan 20\""),
            (Some(day(2026, 1, 5)), Some(day(2026, 1, 21)))
        );
        assert_eq!(
            span_of("date:\"dec 20 - jan 5\""),
            (Some(day(2025, 12, 20)), Some(day(2026, 1, 6)))
        );
        assert_eq!(
            span_of("date:\"since march\""),
            (Some(day(2026, 3, 1)), None)
        );
        assert_eq!(span_of("date:today").0, Some(day(2026, 9, 23)));
        // Unparseable, partially parseable or negated: an error chip that
        // constrains nothing (never silently searched as text).
        for s in ["date:banana", "date:\"february banana\"", "-date:may"] {
            let q = p(s);
            assert!(q.after.is_none() && q.before.is_none(), "{s}");
            assert_eq!(q.chips.len(), 1, "{s}");
            assert_eq!(
                (q.chips[0].kind.as_str(), q.chips[0].label.as_str()),
                ("error", DATE_ERROR),
                "{s}"
            );
            assert_eq!(q.chips[0].raw, s);
            assert!(q.clauses.is_empty(), "{s}");
        }
        // The rest of the query still works next to a bad date.
        let q = p("invoice date:banana");
        assert_eq!(
            q.text_terms().map(|t| t.0).collect::<Vec<_>>(),
            vec!["invoice"]
        );
        // "date:" while typing constrains nothing.
        assert!(p("date:").chips.is_empty());
        // ISO dates in free text stay text (they look like identifiers).
        assert!(p("2026-02-10").after.is_none());
    }

    #[test]
    fn date_values_relative() {
        assert_eq!(range("3 days ago").0, Some(day(2026, 9, 20)));
        assert_eq!(range("3 days ago").1, Some(day(2026, 9, 21)));
        assert_eq!(range("2 weeks ago").0, Some(day(2026, 9, 6)));
        assert_eq!(range("2 weeks ago").1, Some(day(2026, 9, 13)));
        assert_eq!(range("2 months ago").0, Some(day(2026, 7, 1)));
        assert_eq!(range("2 months ago").1, Some(day(2026, 8, 1)));
        assert_eq!(range("1 year ago").0, Some(day(2025, 1, 1)));
        assert_eq!(range("last 3 days").0, Some(day(2026, 9, 20)));
        assert_eq!(range("past 2 weeks").0, Some(day(2026, 9, 9)));
        assert_eq!(range("last 6 months").0, Some(day(2026, 3, 23)));
    }

    #[test]
    fn date_chip_raw_is_exact_substring() {
        let input = "lease  date:\"Last Spring\"  from:mike";
        let q = p(input);
        let date = q.chips.iter().find(|c| c.kind == "date").unwrap();
        assert_eq!(date.raw, "date:\"Last Spring\"");
        assert_eq!(date.label, "Last spring \u{b7} Mar 1 \u{2013} May 31, 2026");
        assert!(input.contains(&date.raw));
        // Text around the date phrase remains text.
        assert_eq!(
            q.text_terms().map(|t| t.0).collect::<Vec<_>>(),
            vec!["lease"]
        );
    }

    #[test]
    fn combined_real_queries() {
        let q = p("the pdf mike sent about the lease date:\"last spring\" has:pdf");
        assert!(q.after.is_some() && q.before.is_some());
        assert!(q
            .clauses
            .iter()
            .any(|c| c.terms[0].atom == Atom::Has(HasKind::Pdf)));
        let q = p("account:work is:unread newer_than:7d");
        assert_eq!(q.accounts, vec!["work"]);
        assert_eq!(q.chips.len(), 3);
        for c in &q.chips {
            assert!("account:work is:unread newer_than:7d".contains(&c.raw));
        }
    }

    #[test]
    fn noise_words_are_optional_with_content() {
        let q = p("the pdf mike sent about the lease date:\"last spring\"");
        let words: Vec<_> = q.text_terms().map(|t| t.0).collect();
        assert_eq!(words, vec!["pdf", "mike", "lease"]);
        assert!(q.after.is_some());
        // Alone, or with only noise, they stay.
        assert_eq!(p("the").text_terms().count(), 1);
        assert_eq!(p("to the ").text_terms().count(), 2);
        // Phrases are never dropped.
        assert_eq!(p("lease \"to the\"").text_terms().count(), 2);
        // A noise word at the end is a finished word far more often than the
        // start of another ("what did grace send"), so it is optional too;
        // the next keystroke ("lease thea") narrows as usual.
        assert_eq!(
            p("lease the").text_terms().map(|t| t.0).collect::<Vec<_>>(),
            vec!["lease"]
        );
        assert_eq!(p("lease thea").text_terms().count(), 2);
        // Exclusions stay.
        assert!(p("lease -the").clauses.iter().any(|c| c.terms[0].negated));
        // Date phrase alone counts as content.
        assert!(p("the date:\"last week\"").text_terms().next().is_none());
    }

    #[test]
    fn noise_is_ignored_safely() {
        for s in [
            "",
            "   ",
            "---",
            "\"\"",
            "\"",
            "-",
            "OR",
            ":",
            "a:",
            "*",
            "(a OR b)",
            "NEAR(a b)",
            "a AND",
            "\u{0}",
            "é",
            "🙂",
        ] {
            let q = p(s);
            for c in &q.chips {
                assert!(s.contains(&c.raw));
            }
        }
        assert!(p("---").clauses.is_empty());
        assert!(p("\"\"").clauses.is_empty());
    }

    fn atoms(q: &ParsedQuery) -> Vec<Vec<(bool, Atom)>> {
        q.clauses
            .iter()
            .map(|c| {
                c.terms
                    .iter()
                    .map(|t| (t.negated, t.atom.clone()))
                    .collect()
            })
            .collect()
    }

    fn text(s: &str) -> Atom {
        Atom::Text {
            text: s.into(),
            phrase: false,
            prefix: false,
        }
    }

    #[test]
    fn people_operators() {
        let q = p("from:me");
        assert_eq!(only_term(&q).atom, Atom::Me(Me::From));
        assert_eq!(q.chips[0].label, "From me");
        for s in ["to:me", "cc:me", "bcc:ME"] {
            assert_eq!(only_term(&p(s)).atom, Atom::Me(Me::To), "{s}");
        }
        assert_eq!(only_term(&p("from:new")).atom, Atom::Is(State::NewSender));
        assert_eq!(p("from:new").chips[0].label, "New sender");
        assert_eq!(only_term(&p("to:new")).atom, Atom::Is(State::FirstOutbound));
        assert_eq!(
            only_term(&p("with:ana ")).atom,
            Atom::Field {
                field: Field::Participant,
                value: "ana".into(),
                prefix: false
            }
        );
        assert_eq!(p("participant:ana").chips[0].label, "With ana");
        let q = p("domain:@Acme.example ");
        assert_eq!(
            only_term(&q).atom,
            Atom::Field {
                field: Field::Domain,
                value: "acme.example".into(),
                prefix: false
            }
        );
        assert_eq!(q.chips[0].label, "Domain acme.example");
        assert_eq!(p("-from:me").chips[0].label, "Not from me");
        assert!(only_term(&p("-from:me")).negated);
    }

    #[test]
    fn state_and_has_values() {
        for (v, st) in [
            ("snoozed", State::Snoozed),
            ("newsletter", State::Newsletter),
            ("bulk", State::Newsletter),
            ("new-sender", State::NewSender),
            ("new_sender", State::NewSender),
            ("first-outbound", State::FirstOutbound),
            ("new-recipient", State::FirstOutbound),
            ("unanswered", State::Unanswered),
            ("needs-reply", State::Unanswered),
            ("replied", State::Replied),
            ("awaiting", State::Awaiting),
            ("awaiting-reply", State::Awaiting),
            ("reply", State::Reply),
        ] {
            assert_eq!(only_term(&p(&format!("is:{v}"))).atom, Atom::Is(st), "{v}");
        }
        assert_eq!(p("is:unanswered").chips[0].label, "Unanswered");
        assert_eq!(p("is:first-outbound").chips[0].label, "First email to them");
        // Folder aliases.
        assert_eq!(only_term(&p("is:sent")).atom, Atom::In(Folder::Sent));
        assert_eq!(only_term(&p("is:draft")).atom, Atom::In(Folder::Drafts));
        assert_eq!(only_term(&p("folder:inbox")).atom, Atom::In(Folder::Inbox));
        assert_eq!(only_term(&p("in:snoozed")).atom, Atom::Is(State::Snoozed));
        assert!(matches!(
            only_term(&p("is:anywhere")).atom,
            Atom::Text { .. }
        ));
        for (v, k) in [
            ("link", HasKind::Link),
            ("links", HasKind::Link),
            ("otp", HasKind::Otp),
            ("code", HasKind::Otp),
            ("unsubscribe", HasKind::Unsubscribe),
        ] {
            assert_eq!(only_term(&p(&format!("has:{v}"))).atom, Atom::Has(k), "{v}");
        }
        assert!(!HasKind::Link.is_attachment());
        assert!(HasKind::Pdf.is_attachment());
        let q = p("category:promotions");
        assert_eq!(only_term(&q).atom, Atom::Label("promotions".into()));
        assert_eq!(q.chips[0].label, "Category Promotions");
        assert_eq!(
            only_term(&p("category:primary")).atom,
            Atom::Label("personal".into())
        );
        assert!(matches!(
            only_term(&p("category:banana")).atom,
            Atom::Text { .. }
        ));
    }

    #[test]
    fn size_operators() {
        let mb = 1024 * 1024;
        let q = p("larger:5M");
        assert_eq!(
            only_term(&q).atom,
            Atom::Size {
                min: Some(5 * mb),
                max: None
            }
        );
        assert_eq!(q.chips[0].label, "Larger than 5 MB");
        assert_eq!(q.chips[0].kind, "size");
        let q = p("smaller:100k");
        assert_eq!(
            only_term(&q).atom,
            Atom::Size {
                min: None,
                max: Some(100 * 1024)
            }
        );
        assert_eq!(q.chips[0].label, "Smaller than 100 KB");
        assert_eq!(
            only_term(&p("size:<1.5mb")).atom,
            Atom::Size {
                min: None,
                max: Some(mb + mb / 2)
            }
        );
        assert_eq!(
            only_term(&p("size:2000000")).atom,
            Atom::Size {
                min: Some(2_000_000),
                max: None
            }
        );
        assert_eq!(p("size:>10MB").chips[0].label, "Larger than 10 MB");
        assert_eq!(p("larger:1500").chips[0].label, "Larger than 1.5 KB");
        for s in ["larger:banana", "size:5Q", "smaller:-1"] {
            assert!(matches!(only_term(&p(s)).atom, Atom::Text { .. }), "{s}");
        }
    }

    #[test]
    fn thread_length_and_weekday() {
        for (v, min, max) in [
            (">5", 6, None),
            (">=5", 5, None),
            ("5+", 5, None),
            ("<3", 0, Some(2)),
            ("<=3", 0, Some(3)),
            ("3", 3, Some(3)),
            ("2..4", 2, Some(4)),
        ] {
            assert_eq!(
                only_term(&p(&format!("messages:{v}"))).atom,
                Atom::ThreadLen { min, max },
                "{v}"
            );
        }
        assert_eq!(p("messages:>5").chips[0].label, "Threads of 6+ messages");
        assert_eq!(
            p("messages:2..4").chips[0].label,
            "Threads of 2\u{2013}4 messages"
        );
        assert_eq!(p("messages:1").chips[0].label, "Threads of 1 message");
        for s in ["messages:many", "messages:4..2", "messages:<0"] {
            assert!(matches!(only_term(&p(s)).atom, Atom::Text { .. }), "{s}");
        }
        let q = p("day:weekend");
        assert_eq!(
            only_term(&q).atom,
            Atom::Weekday {
                days: 0b100_0001,
                utc_offset_secs: Some(0)
            }
        );
        assert_eq!(q.chips[0].label, "On weekends");
        let q = p("day:mon,Wednesday");
        assert_eq!(
            only_term(&q).atom,
            Atom::Weekday {
                days: 0b000_1010,
                utc_offset_secs: Some(0)
            }
        );
        assert_eq!(q.chips[0].label, "On Mondays, Wednesdays");
        assert_eq!(p("day:fridays").chips[0].label, "On Fridays");
        assert!(matches!(
            only_term(&p("day:someday")).atom,
            Atom::Text { .. }
        ));
        // The machine's timezone is resolved by SQLite, DST-aware.
        let q = parse("day:monday", NOW);
        assert_eq!(
            only_term(&q).atom,
            Atom::Weekday {
                days: 0b10,
                utc_offset_secs: None
            }
        );
    }

    fn raw_span(s: &str) -> (Option<i64>, Option<i64>) {
        let q = p(s);
        (q.after, q.before)
    }

    #[test]
    fn date_ranges_and_more_date_operators() {
        // NOW is Wed 2026-09-23.
        assert_eq!(
            span_of("date:aug1..aug15"),
            (Some(day(2026, 8, 1)), Some(day(2026, 8, 16)))
        );
        assert_eq!(
            span_of("date:2026-08-01..2026-08-15"),
            (Some(day(2026, 8, 1)), Some(day(2026, 8, 16)))
        );
        assert_eq!(
            span_of("date:\"august 27\""),
            (Some(day(2026, 8, 27)), Some(day(2026, 8, 28)))
        );
        assert_eq!(
            span_of("date:aug27"),
            (Some(day(2026, 8, 27)), Some(day(2026, 8, 28)))
        );
        assert_eq!(
            raw_span("on:2026-08-27"),
            (Some(day(2026, 8, 27)), Some(day(2026, 8, 28)))
        );
        assert_eq!(p("on:\"aug 27\"").chips[0].raw, "on:\"aug 27\"");
        assert_eq!(
            raw_span("before:\"aug 27\""),
            (None, Some(day(2026, 8, 27)))
        );
        assert_eq!(p("before:\"aug 27\"").chips[0].label, "Before Aug 27, 2026");
        assert_eq!(raw_span("after:yesterday"), (Some(day(2026, 9, 22)), None));
        assert_eq!(raw_span("after:\"last week\"").0, Some(day(2026, 9, 14)));
        assert_eq!(raw_span("since:2026-08"), (Some(day(2026, 8, 1)), None));
        assert_eq!(p("since:2026-08").chips[0].label, "Since Aug 1, 2026");
        assert_eq!(raw_span("until:2026-08-27"), (None, Some(day(2026, 8, 28))));
        assert_eq!(p("until:2026-08-27").chips[0].label, "Until Aug 27, 2026");
        assert_eq!(raw_span("until:2026-08"), (None, Some(day(2026, 9, 1))));
        assert_eq!(raw_span("until:march"), (None, Some(day(2026, 4, 1))));
        // Open-ended expressions work for before/after but not until.
        assert_eq!(raw_span("after:\"since march\"").0, Some(day(2026, 3, 1)));
        assert!(p("until:\"since march\"").before.is_none());
    }

    #[test]
    fn grouping_with_parentheses() {
        let q = p("(from:ana OR from:bob) lease ");
        let a = atoms(&q);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].len(), 2);
        assert_eq!(a[1], vec![(false, text("lease"))]);
        let raws: Vec<_> = q.chips.iter().map(|c| c.raw.as_str()).collect();
        assert_eq!(raws, vec!["from:ana", "from:bob"]);

        // Negated group: De Morgan.
        assert_eq!(
            atoms(&p("-(alpha OR beta) ")),
            vec![vec![(true, text("alpha"))], vec![(true, text("beta"))]]
        );
        assert_eq!(
            atoms(&p("-(alpha beta) ")),
            vec![vec![(true, text("alpha")), (true, text("beta"))]]
        );
        // Distribution: (a b) OR c = (a OR c) (b OR c).
        assert_eq!(
            atoms(&p("(alpha beta) OR gamma ")),
            vec![
                vec![(false, text("alpha")), (false, text("gamma"))],
                vec![(false, text("beta")), (false, text("gamma"))]
            ]
        );
        // Nested, and double negation.
        assert_eq!(atoms(&p("-(-alpha) ")), vec![vec![(false, text("alpha"))]]);
        assert_eq!(atoms(&p("((alpha OR beta) gamma) delta ")).len(), 3);
        // Unbalanced: a missing ")" closes at the end; a stray ")" is text.
        assert_eq!(atoms(&p("(alpha OR beta")).len(), 1);
        assert_eq!(atoms(&p("(alpha OR beta"))[0].len(), 2);
        assert_eq!(atoms(&p("alpha) ")), vec![vec![(false, text("alpha)"))]]);
        // Empty groups and date-only groups constrain nothing extra.
        assert!(p("()").clauses.is_empty());
        let q = p("(date:today) OR alpha ");
        assert!(q.after.is_some());
        assert_eq!(atoms(&q), vec![vec![(false, text("alpha"))]]);
        // As-you-type prefix inside a group, not after ")".
        assert_eq!(
            atoms(&p("(from:mik"))[0][0].1,
            Atom::Field {
                field: Field::From,
                value: "mik".into(),
                prefix: true
            }
        );
        assert_eq!(atoms(&p("(lease)")), vec![vec![(false, text("lease"))]]);
        // Too many combinations: an error chip, and nothing constrained.
        let big = (0..8)
            .map(|i| format!("(a{i} b{i})"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let q = p(&big);
        assert!(q.clauses.is_empty());
        assert_eq!(q.chips.last().unwrap().kind, "error");
        assert_eq!(q.chips.last().unwrap().label, GROUP_ERROR);
        // A long flat OR chain is fine.
        let chain = (0..100)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        assert_eq!(p(&chain).clauses[0].terms.len(), 100);
    }

    #[test]
    fn and_keyword() {
        assert_eq!(
            atoms(&p("alpha AND beta ")),
            vec![vec![(false, text("alpha"))], vec![(false, text("beta"))]]
        );
        // "and" in lowercase is a (noise) word.
        assert_eq!(p("alpha and beta ").clauses.len(), 2);
    }

    #[test]
    fn every_operator_key_is_recognized() {
        for key in OPERATOR_KEYS {
            let q = p(&format!("{key}:"));
            assert!(q.clauses.is_empty(), "{key}");
        }
    }

    fn window_of(q: &str) -> (Option<i64>, Option<i64>) {
        let q = p(q);
        assert!(!q.chips.iter().any(|c| c.kind == "error"), "{:?}", q.chips);
        (q.after, q.before)
    }

    #[test]
    fn more_date_forms() {
        // NOW is Wed 2026-09-23.
        let one = |y, m, d: u32| (Some(day(y, m, d)), Some(day(y, m, d + 1)));
        // Day first, spelled or unambiguous, and dotted (European).
        assert_eq!(window_of("date:\"18 august 2026\""), one(2026, 8, 18));
        assert_eq!(window_of("date:\"18th of august\""), one(2026, 8, 18));
        assert_eq!(window_of("on:18/08/2026"), one(2026, 8, 18));
        assert_eq!(window_of("on:18.08.2026"), one(2026, 8, 18));
        assert_eq!(window_of("on:05.03.2026"), one(2026, 3, 5));
        assert_eq!(window_of("on:05/03/2026"), one(2026, 5, 3));
        // Abbreviations with a period, two-digit years.
        assert_eq!(window_of("date:\"aug. 18\""), one(2026, 8, 18));
        assert_eq!(window_of("date:8/18/26"), one(2026, 8, 18));
        assert_eq!(window_of("after:8/18/26").0, Some(day(2026, 8, 18)));
        // Numbers in words.
        assert_eq!(window_of("date:\"two weeks ago\""), window_of("date:\"2 weeks ago\""));
        assert_eq!(window_of("date:\"a couple of weeks ago\""), window_of("date:\"2 weeks ago\""));
        assert_eq!(window_of("date:\"a week ago\""), window_of("date:\"1 week ago\""));
        assert_eq!(window_of("date:\"last three days\""), window_of("date:\"last 3 days\""));
        // Weekends (Monday-first weeks) and quarters.
        assert_eq!(window_of("date:\"last weekend\""), (Some(day(2026, 9, 19)), Some(day(2026, 9, 21))));
        assert_eq!(window_of("date:\"this weekend\""), (Some(day(2026, 9, 26)), Some(day(2026, 9, 28))));
        // "weekend" alone stays a word ("weekend plans").
        assert!(!is_date_like("weekend", NOW));
        assert_eq!(window_of("date:\"last quarter\""), (Some(day(2026, 4, 1)), Some(day(2026, 7, 1))));
        assert_eq!(window_of("date:\"this quarter\""), (Some(day(2026, 7, 1)), Some(day(2026, 10, 1))));
        assert_eq!(window_of("date:q2"), (Some(day(2026, 4, 1)), Some(day(2026, 7, 1))));
        assert_eq!(window_of("date:\"in q2\""), window_of("date:q2"));
        assert_eq!(window_of("date:q4"), (Some(day(2025, 10, 1)), Some(day(2026, 1, 1))));
        assert_eq!(window_of("date:\"q2 2025\""), (Some(day(2025, 4, 1)), Some(day(2025, 7, 1))));
        // Unix seconds: an exact instant (Gmail).
        assert_eq!(window_of("after:1790000000"), (Some(1_790_000_000_000), None));
        assert_eq!(window_of("before:1790000000"), (None, Some(1_790_000_000_000)));
        // "a" alone is still not a date.
        assert!(!is_date_like("a", NOW));
    }

    #[test]
    fn pasted_subject_decoration_is_optional() {
        let words = |s: &str| p(s).text_terms().map(|t| t.0.to_string()).collect::<Vec<_>>();
        assert_eq!(words("RE: FW: Past due: invoice "), vec!["Past", "due:", "invoice"]);
        assert_eq!(words("AW: Nordlys pilot "), vec!["Nordlys", "pilot"]);
        assert_eq!(words("RV: presupuesto "), vec!["presupuesto"]);
        assert_eq!(words("[External] Past due "), vec!["Past", "due"]);
        assert_eq!(words("lease renewal https:// "), vec!["lease", "renewal"]);
        // Alone they are what there is to search for; "tr" without a colon is a word.
        assert_eq!(words("[External] "), vec!["[External]"]);
        assert_eq!(words("tr lease "), vec!["tr", "lease"]);
    }

    fn field(field: Field, v: &str) -> Atom {
        Atom::Field {
            field,
            value: v.into(),
            prefix: false,
        }
    }

    #[test]
    fn typographic_quotes_make_a_phrase() {
        for s in ["\u{201c}early termination\u{201d} ", "\u{201e}early termination\u{201c} "] {
            let q = p(s);
            assert_eq!(
                only_term(&q).atom,
                Atom::Text {
                    text: "early termination".into(),
                    phrase: true,
                    prefix: false
                },
                "{s}"
            );
            assert_eq!(q.chips[0].raw, s.trim());
        }
        let q = p("subject:\u{201c}q3 plan\u{201d} ");
        assert_eq!(only_term(&q).atom, field(Field::Subject, "q3 plan"));
    }

    #[test]
    fn gmail_braces_are_an_or_group() {
        let q = p("{from:marco from:julia} invoice ");
        assert_eq!(
            atoms(&q),
            vec![
                vec![(false, field(Field::From, "marco")), (false, field(Field::From, "julia"))],
                vec![(false, text("invoice"))],
            ]
        );
        // Negated: none of them.
        let q = p("invoice -{crestline bianchi} ");
        assert_eq!(
            atoms(&q),
            vec![
                vec![(false, text("invoice"))],
                vec![(true, text("crestline"))],
                vec![(true, text("bianchi"))],
            ]
        );
    }

    #[test]
    fn uppercase_not_negates_the_next_term_or_group() {
        let q = p("invoice NOT crestline ");
        assert_eq!(atoms(&q), vec![vec![(false, text("invoice"))], vec![(true, text("crestline"))]]);
        let q = p("invoice NOT (crestline OR bianchi) ");
        assert_eq!(
            atoms(&q),
            vec![
                vec![(false, text("invoice"))],
                vec![(true, text("crestline"))],
                vec![(true, text("bianchi"))],
            ]
        );
        let q = p("NOT from:theo ");
        assert!(matches!(only_term(&q).atom, Atom::Field { .. }));
        assert!(only_term(&q).negated);
        // Lowercase "not" is a word.
        assert_eq!(p("not crestline ").clauses.len(), 2);
    }

    #[test]
    fn operator_applies_to_a_group() {
        let q = p("subject:(lease renewal) ");
        assert_eq!(
            atoms(&q),
            vec![
                vec![(false, field(Field::Subject, "lease"))],
                vec![(false, field(Field::Subject, "renewal"))]
            ]
        );
        let q = p("from:(marco OR julia) invoice ");
        assert_eq!(
            atoms(&q),
            vec![
                vec![(false, field(Field::From, "marco")), (false, field(Field::From, "julia"))],
                vec![(false, text("invoice"))],
            ]
        );
        assert!(q
            .chips
            .iter()
            .filter(|c| c.kind == "from")
            .all(|c| c.raw == "from:(marco OR julia)"));
        // Unclosed: plain words.
        assert_eq!(p("subject:(lease renewal").clauses.len(), 2);
    }

    #[test]
    fn trailing_star_is_a_prefix() {
        let q = p("invoic* crestline ");
        assert_eq!(
            q.clauses[0].terms[0].atom,
            Atom::Text {
                text: "invoic".into(),
                phrase: false,
                prefix: true
            }
        );
        assert_eq!(
            only_term(&p("subject:renew* ")).atom,
            Atom::Field {
                field: Field::Subject,
                value: "renew".into(),
                prefix: true
            }
        );
        // `*` alone is nothing.
        assert!(p("* ").clauses.is_empty());
    }

    #[test]
    fn around_joins_two_terms() {
        assert_eq!(
            only_term(&p("rent AROUND 5 june ")).atom,
            Atom::Near {
                a: "rent".into(),
                b: "june".into(),
                distance: 5
            }
        );
        assert_eq!(
            only_term(&p("rent AROUND june ")).atom,
            Atom::Near {
                a: "rent".into(),
                b: "june".into(),
                distance: AROUND_DEFAULT
            }
        );
        // Nothing on one side: the words stay.
        assert_eq!(atoms(&p("AROUND 5 june ")), vec![vec![(false, text("june"))]]);
    }

    #[test]
    fn other_apps_operator_spellings() {
        assert_eq!(
            only_term(&p("deliveredto:me+shop@mail.example ")).atom,
            field(Field::To, "me+shop@mail.example")
        );
        assert_eq!(only_term(&p("hasattachment:yes ")).atom, Atom::Has(HasKind::Attachment));
        assert!(only_term(&p("hasattachments:false ")).negated);
        assert_eq!(only_term(&p("attachment:roadmap ")).atom, Atom::Filename("roadmap".into()));
        assert_eq!(only_term(&p("participants:ana ")).atom, field(Field::Participant, "ana"));
    }
}
