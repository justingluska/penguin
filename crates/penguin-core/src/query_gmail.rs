//! Penguin's search language (`query::ParsedQuery`) in Gmail's search syntax:
//! the Gmail API's `q` and Gmail IMAP's `X-GM-RAW`. Pure string translation,
//! shared by penguin-gmail and penguin-imap so the two can't drift apart.

use crate::query::{Atom, Field, Folder, HasKind, Me, ParsedQuery, State as IsState};

/// Gmail `q` for messages dated in `[after_ms, before_ms)`: epoch seconds,
/// which Gmail accepts in after:/before: (dates alone are read in Pacific
/// time). None when unbounded.
pub fn range_query(after_ms: Option<i64>, before_ms: Option<i64>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(a) = after_ms.filter(|a| *a > 0) {
        parts.push(format!("after:{}", a.div_euclid(1000)));
    }
    if let Some(b) = before_ms {
        parts.push(format!("before:{}", b.div_euclid(1000)));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Gmail search syntax for a parsed Penguin query, plus whether anything
/// had to be dropped (then Gmail's results are broader than the local
/// query). `account:` is handled by the caller, not Gmail. Local-only
/// operators (is:new-sender, is:first-outbound, is:unanswered, is:replied,
/// is:awaiting, is:reply, is:newsletter, is:snoozed, has:link, has:otp,
/// has:unsubscribe, messages:, day:) have no Gmail equivalent and are dropped.
pub fn to_gmail_query(q: &ParsedQuery) -> (String, bool) {
    let mut parts: Vec<String> = Vec::new();
    let mut approximate = false;
    for clause in &q.clauses {
        let mut terms: Vec<String> = Vec::new();
        for t in &clause.terms {
            match term_to_gmail(&t.atom) {
                Some(s) => terms.push(if t.negated { format!("-{s}") } else { s }),
                None => approximate = true,
            }
        }
        match terms.len() {
            0 => {}
            1 => parts.push(terms.pop().expect("one")),
            _ if terms.len() == clause.terms.len() => {
                parts.push(format!("{{{}}}", terms.join(" ")))
            }
            // Dropping one side of an OR would narrow it; drop the clause.
            _ => approximate = true,
        }
    }
    if let Some(r) = range_query(q.after, q.before) {
        parts.push(r);
    }
    if (q.include_trash || q.include_spam) && !parts.iter().any(|p| p.starts_with("in:")) {
        parts.push("in:anywhere".into());
    }
    (parts.join(" "), approximate)
}

/// A value quoted for Gmail when it has spaces or syntax characters.
fn quote(v: &str) -> String {
    let clean: String = v.chars().filter(|c| *c != '"').collect();
    if clean
        .chars()
        .any(|c| c.is_whitespace() || "(){}:-".contains(c))
    {
        format!("\"{clean}\"")
    } else {
        clean
    }
}

fn term_to_gmail(atom: &Atom) -> Option<String> {
    Some(match atom {
        Atom::Text { text, phrase, .. } => {
            if *phrase {
                format!("\"{}\"", text.replace('"', ""))
            } else {
                quote(text)
            }
        }
        Atom::Field { field, value, .. } => {
            let op = match field {
                Field::From => "from",
                Field::To => "to",
                Field::Cc => "cc",
                Field::Subject => "subject",
                // Gmail matches a domain given to from:/to:/cc:.
                Field::Participant | Field::Domain => {
                    let v = quote(value);
                    return Some(format!("{{from:{v} to:{v} cc:{v} bcc:{v}}}"));
                }
            };
            format!("{op}:{}", quote(value))
        }
        Atom::Near { a, b, distance } => format!("{} AROUND {distance} {}", quote(a), quote(b)),
        Atom::Me(Me::From) => "from:me".into(),
        Atom::Me(Me::To) => "to:me".into(),
        Atom::Size { min, max } => match (min, max) {
            (Some(lo), None) => format!("larger:{lo}"),
            (None, Some(hi)) => format!("smaller:{hi}"),
            (Some(lo), Some(hi)) => format!("larger:{lo} smaller:{hi}"),
            (None, None) => return None,
        },
        Atom::ThreadLen { .. } | Atom::Weekday { .. } => return None,
        Atom::Filename(f) => format!("filename:{}", quote(f)),
        Atom::Has(kind) => match kind {
            HasKind::Attachment => "has:attachment".into(),
            HasKind::Pdf => "filename:pdf".into(),
            HasKind::Image => {
                "{filename:jpg filename:jpeg filename:png filename:gif filename:heic filename:webp}"
                    .into()
            }
            HasKind::Doc => {
                "{filename:doc filename:docx filename:pages filename:rtf has:document}".into()
            }
            HasKind::Spreadsheet => {
                "{filename:xls filename:xlsx filename:csv filename:numbers has:spreadsheet}".into()
            }
            HasKind::Presentation => {
                "{filename:ppt filename:pptx filename:key has:presentation}".into()
            }
            HasKind::Invite => "filename:ics".into(),
            HasKind::Link | HasKind::Otp | HasKind::Unsubscribe => return None,
        },
        Atom::Label(l) => format!("label:{}", quote(&l.replace(' ', "-"))),
        Atom::In(folder) => match folder {
            Folder::Inbox => "in:inbox".into(),
            Folder::Sent => "in:sent".into(),
            Folder::Drafts => "in:drafts".into(),
            Folder::Trash => "in:trash".into(),
            Folder::Spam => "in:spam".into(),
            Folder::Anywhere => "in:anywhere".into(),
            Folder::Done => "-in:inbox".into(),
            Folder::Starred => "is:starred".into(),
            Folder::Important => "is:important".into(),
        },
        Atom::Is(state) => match state {
            IsState::Unread => "is:unread".into(),
            IsState::Read => "is:read".into(),
            IsState::Starred => "is:starred".into(),
            IsState::Unstarred => "-is:starred".into(),
            IsState::Important => "is:important".into(),
            IsState::Snoozed
            | IsState::Newsletter
            | IsState::NewSender
            | IsState::FirstOutbound
            | IsState::KnownSender
            | IsState::Unanswered
            | IsState::Replied
            | IsState::Awaiting
            | IsState::Reply => return None,
        },
    })
}
