//! Penguin's search language (`penguin_core::query::ParsedQuery`) as IMAP
//! `UID SEARCH` criteria, or as Gmail syntax for `X-GM-RAW` on Gmail.
//!
//! IMAP search is weaker than Gmail's: `has:pdf`, `filename:` and
//! `is:important` only approximate, and body search depends on the server.
//! Local search stays primary; this is "also search the server".

use penguin_core::query::{Atom, Field, Folder as QFolder, HasKind, ParsedQuery, State};

use crate::folders::Role;
use crate::proto::Arg;
use crate::session::search_date;

/// Which folders a search looks in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scope {
    /// Only folders with these roles (`in:sent`); empty = the default set
    /// (inbox, sent, archive, user folders).
    pub roles: Vec<Role>,
    /// `in:done`: everything but the inbox.
    pub not_inbox: bool,
    /// `label:x`: user folders by name (case-insensitive).
    pub folder_names: Vec<String>,
    pub include_trash: bool,
    pub include_spam: bool,
}

/// UID SEARCH criteria for `q`, the folders to search, and whether the
/// translation dropped something (results may be broader).
pub fn imap_criteria(q: &ParsedQuery) -> (Vec<Arg>, Scope, bool) {
    let mut out: Vec<Arg> = Vec::new();
    let mut approximate = false;
    let mut scope = Scope {
        include_trash: q.include_trash,
        include_spam: q.include_spam,
        ..Scope::default()
    };
    for clause in &q.clauses {
        // Folder atoms change the scope (single-term clauses only).
        if let [t] = clause.terms.as_slice() {
            if !t.negated {
                match &t.atom {
                    Atom::In(f) => {
                        match f {
                            QFolder::Inbox => scope.roles.push(Role::Inbox),
                            QFolder::Sent => scope.roles.push(Role::Sent),
                            QFolder::Drafts => scope.roles.push(Role::Drafts),
                            QFolder::Trash => {
                                scope.roles.push(Role::Trash);
                                scope.include_trash = true;
                            }
                            QFolder::Spam => {
                                scope.roles.push(Role::Junk);
                                scope.include_spam = true;
                            }
                            QFolder::Anywhere => {
                                scope.include_trash = true;
                                scope.include_spam = true;
                            }
                            QFolder::Done => scope.not_inbox = true,
                            QFolder::Starred => out.push(Arg::raw("FLAGGED")),
                            QFolder::Important => approximate = true,
                        }
                        continue;
                    }
                    Atom::Label(name) => {
                        scope.folder_names.push(name.clone());
                        continue;
                    }
                    _ => {}
                }
            }
        }
        let mut keys: Vec<Vec<Arg>> = Vec::new();
        for t in &clause.terms {
            match key(&t.atom) {
                Some((mut k, approx)) => {
                    approximate |= approx;
                    if t.negated {
                        k.insert(0, Arg::raw("NOT"));
                    }
                    keys.push(k);
                }
                None => approximate = true,
            }
        }
        if keys.len() != clause.terms.len() && clause.terms.len() > 1 {
            // Dropping one side of an OR would narrow it: drop the clause.
            approximate = true;
            continue;
        }
        out.extend(or_all(keys));
    }
    if let Some(after) = q.after {
        out.push(Arg::raw(format!("SINCE {}", search_date(after))));
    }
    if let Some(before) = q.before {
        // BEFORE is by whole days; round an exclusive bound up.
        let day = 86_400_000;
        let b = if before % day == 0 {
            before
        } else {
            before - before.rem_euclid(day) + day
        };
        out.push(Arg::raw(format!("BEFORE {}", search_date(b))));
    }
    if out.is_empty() {
        out.push(Arg::raw("ALL"));
    }
    (out, scope, approximate)
}

fn or_all(mut keys: Vec<Vec<Arg>>) -> Vec<Arg> {
    match keys.len() {
        0 => Vec::new(),
        1 => keys.pop().expect("one"),
        _ => {
            let first = keys.remove(0);
            let mut out = vec![Arg::raw("OR")];
            out.extend(wrap(first));
            out.extend(wrap(or_all(keys)));
            out
        }
    }
}

/// A multi-part key as one parenthesized key (for OR operands).
fn wrap(k: Vec<Arg>) -> Vec<Arg> {
    if k.len() <= 2 {
        return k;
    }
    let mut out = vec![Arg::raw("(")];
    out.extend(k);
    out.push(Arg::raw(")"));
    out
}

fn text(s: &str) -> Arg {
    Arg::string(s.as_bytes())
}

/// (criteria, approximate) for one atom; None when IMAP can't express it.
fn key(atom: &Atom) -> Option<(Vec<Arg>, bool)> {
    Some(match atom {
        Atom::Text { text: t, .. } => (vec![Arg::raw("TEXT"), text(t)], false),
        // IMAP has no proximity: both words (approximate).
        Atom::Near { a, b, .. } => (vec![Arg::raw("TEXT"), text(a), Arg::raw("TEXT"), text(b)], true),
        Atom::Field { field, value, .. } => match field {
            Field::From => (vec![Arg::raw("FROM"), text(value)], false),
            Field::Cc => (vec![Arg::raw("CC"), text(value)], false),
            Field::Subject => (vec![Arg::raw("SUBJECT"), text(value)], false),
            Field::To => (
                vec![
                    Arg::raw("OR"),
                    Arg::raw("TO"),
                    text(value),
                    Arg::raw("OR"),
                    Arg::raw("CC"),
                    text(value),
                    Arg::raw("BCC"),
                    text(value),
                ],
                false,
            ),
            // with:/domain: — the address (or domain text) in any header.
            Field::Participant | Field::Domain => (
                vec![
                    Arg::raw("OR"),
                    Arg::raw("FROM"),
                    text(value),
                    Arg::raw("OR"),
                    Arg::raw("TO"),
                    text(value),
                    Arg::raw("OR"),
                    Arg::raw("CC"),
                    text(value),
                    Arg::raw("BCC"),
                    text(value),
                ],
                false,
            ),
        },
        // IMAP's LARGER/SMALLER are the whole message's size, not the
        // attachments', so they only approximate.
        Atom::Size { min, max } => match (min, max) {
            (Some(lo), None) => (vec![Arg::raw("LARGER"), Arg::raw(&lo.to_string())], true),
            (None, Some(hi)) => (vec![Arg::raw("SMALLER"), Arg::raw(&hi.to_string())], true),
            (Some(lo), Some(hi)) => (
                vec![
                    Arg::raw("LARGER"),
                    Arg::raw(&lo.to_string()),
                    Arg::raw("SMALLER"),
                    Arg::raw(&hi.to_string()),
                ],
                true,
            ),
            (None, None) => return None,
        },
        // Local-only: from:me/to:me need your addresses, the rest need the
        // whole mailbox (first contacts, replies) or local state.
        Atom::Me(_) | Atom::ThreadLen { .. } | Atom::Weekday { .. } => return None,
        Atom::Filename(f) => (vec![Arg::raw("TEXT"), text(f)], true),
        Atom::Has(kind) => match kind {
            HasKind::Attachment => (
                vec![
                    Arg::raw("HEADER"),
                    text("Content-Type"),
                    text("multipart/mixed"),
                ],
                true,
            ),
            HasKind::Pdf => (vec![Arg::raw("TEXT"), text(".pdf")], true),
            HasKind::Invite => (vec![Arg::raw("TEXT"), text("text/calendar")], true),
            _ => return None,
        },
        Atom::Is(state) => match state {
            State::Unread => (vec![Arg::raw("UNSEEN")], false),
            State::Read => (vec![Arg::raw("SEEN")], false),
            State::Starred => (vec![Arg::raw("FLAGGED")], false),
            State::Unstarred => (vec![Arg::raw("UNFLAGGED")], false),
            State::Important
            | State::Snoozed
            | State::Newsletter
            | State::NewSender
            | State::FirstOutbound
            | State::KnownSender
            | State::Unanswered
            | State::Replied
            | State::Awaiting
            | State::Reply => return None,
        },
        // Folder and label atoms inside ORs or negated: not expressible.
        Atom::In(_) | Atom::Label(_) => return None,
    })
}

// ---------- Gmail (X-GM-RAW) ----------

/// `q` in Gmail's search syntax (for X-GM-RAW in All Mail): the same
/// translation the Gmail API provider uses (penguin-core's `query_gmail`).
pub fn gmail_query(q: &ParsedQuery) -> String {
    penguin_core::query_gmail::to_gmail_query(q).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(args: &[Arg]) -> String {
        args.iter()
            .map(|a| match a {
                Arg::Raw(s) => s.clone(),
                Arg::Str(s) => format!("\"{}\"", String::from_utf8_lossy(s)),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn q(s: &str) -> ParsedQuery {
        penguin_core::query::parse_with_offset(s, 1_760_000_000_000, 0)
    }

    #[test]
    fn fields_flags_and_dates() {
        let (c, scope, approx) = imap_criteria(&q(
            "from:ada subject:\"quarterly report\" is:unread invoice",
        ));
        let w = wire(&c);
        assert!(w.contains("FROM \"ada\""), "{w}");
        assert!(w.contains("SUBJECT \"quarterly report\""), "{w}");
        assert!(w.contains("UNSEEN"), "{w}");
        assert!(w.contains("TEXT \"invoice\""), "{w}");
        assert_eq!(scope, Scope::default());
        assert!(!approx);
        let (c, _, _) = imap_criteria(&q("after:2026-01-05 before:2026-02-01"));
        let w = wire(&c);
        assert!(w.contains("SINCE 5-Jan-2026"), "{w}");
        assert!(w.contains("BEFORE 1-Feb-2026"), "{w}");
    }

    #[test]
    fn ors_negations_and_scopes() {
        let (c, _, _) = imap_criteria(&q("from:ada OR from:bea -is:starred"));
        let w = wire(&c);
        assert!(w.starts_with("OR FROM \"ada\" FROM \"bea\""), "{w}");
        assert!(w.contains("NOT FLAGGED"), "{w}");
        let (c, scope, _) = imap_criteria(&q("in:sent"));
        assert_eq!(wire(&c), "ALL");
        assert_eq!(scope.roles, vec![Role::Sent]);
        let (_, scope, _) = imap_criteria(&q("label:Receipts in:anywhere"));
        assert_eq!(scope.folder_names, vec!["Receipts".to_string()]);
        assert!(scope.include_trash && scope.include_spam);
        let (_, _, approx) = imap_criteria(&q("has:spreadsheet"));
        assert!(approx);
    }

    #[test]
    fn gmail_syntax() {
        assert_eq!(
            gmail_query(&q("from:ada has:attachment -in:inbox")),
            "from:ada has:attachment --in:inbox".replace("--in:inbox", "-in:inbox")
        );
        assert_eq!(gmail_query(&q("\"exact words\"")), "\"exact words\"");
    }
}
