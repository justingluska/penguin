//! Penguin's parsed query → Graph `$search` (KQL over messages).
//!
//! Graph's message search understands free text and the properties
//! `from`, `to`, `cc`, `subject`, `attachment` (file names),
//! `hasattachments`, `received`. It can't be combined with `$filter` or
//! `$orderby`, has no negation Penguin can rely on, and ignores folders
//! and read state, so those parts are dropped: results can be broader than
//! the local query (local search stays primary; this is "also search
//! Outlook").

use penguin_core::query::{Atom, Field, HasKind, ParsedQuery};

use crate::convert::format_date;

/// A KQL value, quoted when it has spaces or syntax characters.
fn value(v: &str) -> String {
    let clean: String = v.chars().filter(|c| *c != '"' && *c != '\\').collect();
    let clean = clean.trim().to_string();
    if clean
        .chars()
        .any(|c| c.is_whitespace() || "():<>=".contains(c))
    {
        format!("\\\"{clean}\\\"")
    } else {
        clean
    }
}

fn term(atom: &Atom) -> Option<String> {
    Some(match atom {
        Atom::Text { text, phrase, .. } => {
            let t = text.trim();
            if t.is_empty() {
                return None;
            }
            if *phrase {
                format!("\\\"{}\\\"", t.replace(['"', '\\'], ""))
            } else {
                value(t)
            }
        }
        Atom::Field {
            field, value: v, ..
        } => {
            let op = match field {
                Field::From => "from",
                Field::To => "to",
                Field::Cc => "cc",
                Field::Subject => "subject",
                // KQL's participants: covers From, To, Cc and Bcc.
                Field::Participant | Field::Domain => "participants",
            };
            format!("{op}:{}", value(v))
        }
        // KQL has NEAR, but $search does not document it: both words.
        Atom::Near { a, b, .. } => format!("{} {}", value(a), value(b)),
        Atom::Filename(f) => format!("attachment:{}", value(f)),
        Atom::Has(HasKind::Attachment) => "hasattachments:true".into(),
        Atom::Has(HasKind::Pdf) => "attachment:pdf".into(),
        Atom::Has(HasKind::Invite) => "attachment:ics".into(),
        // Links, codes and unsubscribe headers are local-only facts.
        Atom::Has(HasKind::Link | HasKind::Otp | HasKind::Unsubscribe) => return None,
        Atom::Has(_) => "hasattachments:true".into(),
        // Folders, labels, read state and the local-only operators (from:me,
        // sizes, thread length, weekday) aren't searchable in KQL here.
        Atom::Label(_)
        | Atom::In(_)
        | Atom::Is(_)
        | Atom::Me(_)
        | Atom::Size { .. }
        | Atom::ThreadLen { .. }
        | Atom::Weekday { .. } => return None,
    })
}

/// The `$search` value (without the outer quotes Graph wants), or None
/// when nothing searchable is left.
pub(crate) fn to_kql(q: &ParsedQuery) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for clause in &q.clauses {
        let terms: Vec<String> = clause
            .terms
            .iter()
            .filter(|t| !t.negated)
            .filter_map(|t| term(&t.atom))
            .collect();
        // Dropping one side of an OR would narrow it: keep only complete ones.
        let complete = terms.len() == clause.terms.len();
        match terms.len() {
            0 => {}
            1 if complete => parts.push(terms.into_iter().next().expect("one")),
            _ if complete => parts.push(format!("({})", terms.join(" OR "))),
            _ => {}
        }
    }
    if parts.is_empty() {
        return None;
    }
    if let Some(after) = q.after {
        parts.push(format!("received>={}", &format_date(after)[..10]));
    }
    if let Some(before) = q.before {
        parts.push(format!("received<{}", &format_date(before)[..10]));
    }
    Some(parts.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kql(s: &str) -> Option<String> {
        to_kql(&penguin_core::query::parse(s, 1_760_000_000_000))
    }

    #[test]
    fn free_text_and_fields() {
        assert_eq!(kql("invoice").as_deref(), Some("invoice"));
        assert_eq!(
            kql("from:cy@quill.example subject:\"q3 plan\" has:attachment").as_deref(),
            Some("from:cy@quill.example AND subject:\\\"q3 plan\\\" AND hasattachments:true")
        );
        assert_eq!(
            kql("\"exact words\"").as_deref(),
            Some("\\\"exact words\\\"")
        );
        assert_eq!(kql("a OR b").as_deref(), Some("(a OR b)"));
    }

    #[test]
    fn unsearchable_parts_are_dropped() {
        assert_eq!(kql("is:unread in:inbox"), None);
        assert_eq!(kql("report -draft").as_deref(), Some("report"));
        assert_eq!(kql(""), None);
        let dated = kql("report after:2025-01-02").unwrap();
        assert!(
            dated.starts_with("report AND received>=2025-01-0"),
            "{dated}"
        );
    }
}
