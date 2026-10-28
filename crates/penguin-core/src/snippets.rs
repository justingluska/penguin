//! Filling a snippet in: the variables a snippet body may use, finding
//! snippets for what's typed after ";", the {placeholders} left to fill,
//! and the subject, Cc and Bcc a snippet brings.
//!
//! Variables:
//!   {first_name}     first name of the first To recipient
//!   {last_name}      their last name
//!   {full_name}      their whole name
//!   {company}        their company, from the address's domain (not for
//!                    Gmail, Outlook, iCloud and other personal mail)
//!   {sender_name}    full name of the person whose message you're replying to
//!   {my_name}        your name on the sending account
//!   {my_first_name}  its first word
//!   {date}           today's date ("September 24, 2026")
//!   {cursor}         where the caret lands after inserting
//! Any other {placeholder} stays in the text for you to fill.
//!
//! The same rules as the Mac's composer (apps/desktop/src/features/compose/
//! snippets.ts, which runs them in the web view on every keystroke); the
//! iPhone uses this through penguin-ffi. Tests pin both to the same cases.

use std::collections::HashSet;

use chrono::{Datelike, NaiveDate};

use crate::compose_prefs::Snippet;
use crate::types::Address;

/// The built-in variables and what they mean, for a snippet editor.
pub const SNIPPET_VARIABLES: [(&str, &str); 9] = [
    ("{first_name}", "First name of the first recipient"),
    ("{last_name}", "Last name of the first recipient"),
    ("{full_name}", "Full name of the first recipient"),
    (
        "{company}",
        "The recipient's company, from their email domain",
    ),
    ("{sender_name}", "Name of the person you're replying to"),
    ("{my_name}", "Your name on the sending account"),
    ("{my_first_name}", "Your first name"),
    ("{date}", "Today's date"),
    ("{cursor}", "Where the cursor lands"),
];

/// Who and when a snippet is filled in for.
#[derive(Debug, Clone)]
pub struct SnippetContext {
    /// The message's To, in order (the first one is "the recipient").
    pub to: Vec<Address>,
    /// The sender of the message being answered.
    pub replying_to: Option<Address>,
    /// Your name on the sending account.
    pub my_name: String,
    /// Today, local.
    pub today: NaiveDate,
}

/// A person's name in parts; None when unknown (an address with no name).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameParts {
    pub first: Option<String>,
    pub last: Option<String>,
    pub full: Option<String>,
}

/// "Priya Raman" → Priya / Raman; "Raman, Priya" is read last name first.
pub fn name_parts(a: Option<&Address>) -> NameParts {
    let raw = a
        .and_then(|a| a.name.as_deref())
        .map(|n| n.trim().trim_matches('"').trim())
        .unwrap_or("");
    if raw.is_empty() || raw.contains('@') {
        return NameParts::default();
    }
    let reordered;
    let text = match raw.split_once(',') {
        Some((last, first))
            if !last.trim().is_empty() && !first.trim().is_empty() && !first.contains(',') =>
        {
            reordered = format!("{} {}", first.trim(), last.trim());
            reordered.as_str()
        }
        _ => raw,
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return NameParts::default();
    }
    NameParts {
        first: Some(words[0].to_string()),
        last: (words.len() > 1).then(|| words[words.len() - 1].to_string()),
        full: Some(words.join(" ")),
    }
}

/// Domains of personal mail services: they say nothing about where someone works.
const PERSONAL_DOMAINS: [&str; 40] = [
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "ymail.com",
    "aol.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "proton.me",
    "protonmail.com",
    "pm.me",
    "fastmail.com",
    "gmx.com",
    "gmx.net",
    "gmx.de",
    "web.de",
    "mail.com",
    "zoho.com",
    "yandex.com",
    "yandex.ru",
    "hey.com",
    "tutanota.com",
    "tuta.io",
    "qq.com",
    "163.com",
    "126.com",
    "naver.com",
    "hotmail.co.uk",
    "yahoo.co.uk",
    "outlook.fr",
    "hotmail.fr",
    "yahoo.fr",
    "orange.fr",
    "free.fr",
    "libero.it",
    "t-online.de",
];

/// Second-level labels that belong to the suffix ("co.uk", "com.au").
const SUFFIX_LABELS: [&str; 9] = ["co", "com", "org", "net", "ac", "gov", "edu", "ne", "or"];

/// "priya@mail.northwind.example" → "Northwind"; None for personal mail or no domain.
pub fn company_from_email(email: &str) -> Option<String> {
    let domain = email.split('@').nth(1)?.trim().to_lowercase();
    if domain.is_empty() || PERSONAL_DOMAINS.contains(&domain.as_str()) {
        return None;
    }
    let labels: Vec<&str> = domain.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() < 2 {
        return None;
    }
    let mut i = labels.len() - 2;
    if i > 0 && SUFFIX_LABELS.contains(&labels[i]) && labels[labels.len() - 1].len() == 2 {
        i -= 1;
    }
    let name = labels[i];
    if name.is_empty() || PERSONAL_DOMAINS.contains(&labels[i..].join(".").as_str()) {
        return None;
    }
    let words: Vec<String> = name
        .split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        })
        .collect();
    Some(words.join(" "))
}

/// "September 24, 2026".
pub fn long_date(d: NaiveDate) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    format!("{} {}, {}", MONTHS[d.month0() as usize], d.day(), d.year())
}

/// The value of a built-in variable (None: unknown here, the placeholder stays).
fn value(name: &str, ctx: &SnippetContext) -> Option<String> {
    let to = ctx.to.first();
    let me = ctx.my_name.trim();
    match name {
        "first_name" => name_parts(to).first,
        "last_name" => name_parts(to).last,
        "full_name" => name_parts(to).full,
        "company" => to.and_then(|a| company_from_email(&a.email)),
        "sender_name" => ctx.replying_to.as_ref().map(display_name),
        "my_name" => (!me.is_empty()).then(|| me.to_string()),
        "my_first_name" => me.split_whitespace().next().map(str::to_string),
        "date" => Some(long_date(ctx.today)),
        _ => None,
    }
}

/// A name to show: the name, else the address's local part.
fn display_name(a: &Address) -> String {
    match a.name.as_deref().map(str::trim) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => a.email.split('@').next().unwrap_or_default().to_string(),
    }
}

const VARIABLES: [&str; 8] = [
    "first_name",
    "last_name",
    "full_name",
    "company",
    "sender_name",
    "my_name",
    "my_first_name",
    "date",
];

/// A filled-in snippet and where the caret goes. Offsets count UTF-16
/// code units (what NSString, Swift's `utf16` view and JavaScript count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    pub sel_start: usize,
    pub sel_end: usize,
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Fill the known variables. The caret goes to {cursor} if present (it's
/// removed), else the first remaining {placeholder} is selected, else the
/// end. Unknown values (no recipient yet) stay as placeholders.
pub fn expand_snippet(body: &str, ctx: &SnippetContext) -> Expanded {
    let mut text = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let filled = after.find('}').and_then(|close| {
            let name = &after[..close];
            VARIABLES
                .contains(&name)
                .then(|| value(name, ctx))
                .flatten()
                .map(|v| (v, close))
        });
        match filled {
            Some((v, close)) => {
                text.push_str(&v);
                rest = &after[close + 1..];
            }
            None => {
                text.push('{');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    if let Some(i) = text.find("{cursor}") {
        text.replace_range(i..i + "{cursor}".len(), "");
        let at = utf16_len(&text[..i]);
        return Expanded {
            text,
            sel_start: at,
            sel_end: at,
        };
    }
    if let Some((start, end)) = first_placeholder(&text) {
        return Expanded {
            sel_start: utf16_len(&text[..start]),
            sel_end: utf16_len(&text[..end]),
            text,
        };
    }
    let end = utf16_len(&text);
    Expanded {
        text,
        sel_start: end,
        sel_end: end,
    }
}

/// The first `{…}` with no line break inside (byte range, braces included).
fn first_placeholder(text: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(o) = text[from..].find('{') {
        let open = from + o;
        let inner = &text[open + 1..];
        let close = inner.find(['}', '\n'])?;
        if inner.as_bytes()[close] == b'}' && close > 0 {
            return Some((open, open + 1 + close + 1));
        }
        from = open + 1;
    }
    None
}

/// Snippets for what's typed after ";" (or in the picker): trigger prefix
/// first, then title matches, most used first.
pub fn match_snippets<'a>(list: &'a [Snippet], typed: &str) -> Vec<&'a Snippet> {
    let t = typed.trim().to_lowercase();
    let mut out: Vec<&Snippet> = if t.is_empty() {
        list.iter().collect()
    } else {
        list.iter()
            .filter(|s| {
                s.trigger.starts_with(&t)
                    || (t.chars().count() > 1
                        && (s.title.to_lowercase().contains(&t)
                            || s.body.to_lowercase().contains(&t)))
            })
            .collect()
    };
    if t.is_empty() {
        out.sort_by_key(|s| std::cmp::Reverse(s.uses));
    } else {
        out.sort_by(|a, b| {
            b.trigger
                .starts_with(&t)
                .cmp(&a.trigger.starts_with(&t))
                .then_with(|| {
                    b.title
                        .to_lowercase()
                        .contains(&t)
                        .cmp(&a.title.to_lowercase().contains(&t))
                })
                .then_with(|| b.uses.cmp(&a.uses))
        });
    }
    out
}

/// {placeholders} still in the text when it's about to go (a snippet's
/// {day} nobody filled in). Only the curly-brace form snippets use: a
/// letter, then letters, digits, spaces and `_ . ' -`, at most 40 inside.
pub fn unfilled_placeholders(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(o) = text[from..].find('{') {
        let open = from + o;
        let inner = &text[open + 1..];
        let ok_len = inner
            .char_indices()
            .take_while(|(i, c)| {
                if *i == 0 {
                    c.is_ascii_alphabetic()
                } else {
                    c.is_ascii_alphanumeric() || matches!(c, '_' | ' ' | '.' | '\'' | '-')
                }
            })
            .count();
        if (1..=40).contains(&ok_len) && inner[ok_len..].starts_with('}') {
            let p = text[open..open + 1 + ok_len + 1].to_string();
            if !out.contains(&p) {
                out.push(p);
            }
            from = open + 1 + ok_len + 1;
        } else {
            from = open + 1;
        }
    }
    out
}

/// What a snippet adds besides its text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnippetExtras {
    /// Set only when the message has no subject yet.
    pub subject: Option<String>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
}

/// "Name <a@b.example>" or a bare address; None when it isn't one.
pub fn parse_address(raw: &str) -> Option<Address> {
    let t = raw.trim().trim_end_matches([',', ';']);
    let is_email = |e: &str| {
        let bad = |c: char| c.is_whitespace() || "@<>(),;:\"".contains(c);
        let Some((local, domain)) = e.split_once('@') else {
            return false;
        };
        !local.is_empty()
            && !local.chars().any(bad)
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !domain.chars().any(bad)
    };
    if let (Some(lt), true) = (t.rfind('<'), t.ends_with('>')) {
        let email = t[lt + 1..t.len() - 1].trim();
        let name = t[..lt].trim().trim_matches('"').trim();
        return is_email(email).then(|| Address {
            name: (!name.is_empty()).then(|| name.to_string()),
            email: email.to_string(),
        });
    }
    is_email(t).then(|| Address {
        name: None,
        email: t.to_string(),
    })
}

/// The subject, Cc and Bcc a snippet brings to a message: addresses already
/// on it (in any field) aren't added twice, unreadable ones are skipped.
pub fn snippet_extras(
    s: &Snippet,
    subject: &str,
    to: &[Address],
    cc: &[Address],
    bcc: &[Address],
) -> SnippetExtras {
    let mut have: HashSet<String> = to
        .iter()
        .chain(cc)
        .chain(bcc)
        .map(|a| a.email.to_lowercase())
        .collect();
    let mut take = |list: &[String]| {
        list.iter()
            .filter_map(|raw| parse_address(raw))
            .filter(|a| have.insert(a.email.to_lowercase()))
            .collect::<Vec<_>>()
    };
    let cc = take(&s.cc);
    let bcc = take(&s.bcc);
    let subject = (!s.subject.trim().is_empty() && subject.trim().is_empty())
        .then(|| s.subject.trim().to_string());
    SnippetExtras { subject, cc, bcc }
}

/// The ";trigger" being typed just before the caret, if any: the text after
/// a ";" that starts the text or follows whitespace, with nothing but
/// [a-z0-9_-] between it and the caret. Returns (byte offset of the ";",
/// the trigger typed so far, possibly empty).
pub fn trigger_before_caret(text: &str, caret: usize) -> Option<(usize, String)> {
    let head = text.get(..caret)?;
    let semi = head.rfind(';')?;
    let typed = &head[semi + 1..];
    let ok = typed.len() <= 32
        && typed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let starts_word = head[..semi]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    (ok && starts_word).then(|| (semi, typed.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(name: Option<&str>, email: &str) -> Address {
        Address {
            name: name.map(str::to_string),
            email: email.into(),
        }
    }

    fn ctx() -> SnippetContext {
        SnippetContext {
            to: vec![addr(Some("Priya Raman"), "priya@mail.northwind.example")],
            replying_to: Some(addr(Some("Dana Ortiz"), "dana@harbor.example")),
            my_name: "Sam Okafor".into(),
            today: NaiveDate::from_ymd_opt(2026, 9, 24).unwrap(),
        }
    }

    #[test]
    fn variables_fill_from_the_first_recipient_the_sender_and_me() {
        let r = expand_snippet(
            "{first_name} {last_name} / {full_name} at {company}; {sender_name}; {my_name} ({my_first_name}), {date}",
            &ctx(),
        );
        assert_eq!(
            r.text,
            "Priya Raman / Priya Raman at Northwind; Dana Ortiz; Sam Okafor (Sam), September 24, 2026"
        );
        assert_eq!(r.sel_start, utf16_len(&r.text));
    }

    #[test]
    fn the_caret_lands_on_cursor_else_the_first_unknown_placeholder() {
        let c = expand_snippet("Hi {first_name}, {cursor} thanks", &ctx());
        assert_eq!(c.text, "Hi Priya,  thanks");
        assert_eq!(c.sel_start, 10);
        let mut no_name = ctx();
        no_name.to = vec![addr(None, "ops@northwind.example")];
        let r = expand_snippet("Hi {first_name}, see you {day}", &no_name);
        assert_eq!(r.text, "Hi {first_name}, see you {day}");
        assert_eq!((r.sel_start, r.sel_end), (3, 15));
        // Offsets are UTF-16: an emoji before the caret counts two.
        let e = expand_snippet("👋 {cursor}", &ctx());
        assert_eq!(e.sel_start, 3);
    }

    #[test]
    fn names_split_sensibly_and_company_comes_from_a_work_domain_only() {
        let p = name_parts(Some(&addr(Some("Raman, Priya"), "p@x.example")));
        assert_eq!(
            (p.first.as_deref(), p.last.as_deref(), p.full.as_deref()),
            (Some("Priya"), Some("Raman"), Some("Priya Raman"))
        );
        let c = name_parts(Some(&addr(Some("Cher"), "c@x.example")));
        assert_eq!((c.first.as_deref(), c.last), (Some("Cher"), None));
        assert_eq!(
            name_parts(Some(&addr(Some("priya@x.example"), "priya@x.example"))),
            NameParts::default()
        );
        assert_eq!(
            company_from_email("a@northwind.example").as_deref(),
            Some("Northwind")
        );
        assert_eq!(
            company_from_email("a@mail.blue-harbor.example").as_deref(),
            Some("Blue Harbor")
        );
        assert_eq!(company_from_email("a@acme.co.uk").as_deref(), Some("Acme"));
        assert_eq!(company_from_email("someone@gmail.com"), None);
        assert_eq!(company_from_email("someone@icloud.com"), None);
        assert_eq!(company_from_email("nodomain"), None);
    }

    #[test]
    fn unfilled_placeholders_are_found_once_each() {
        assert_eq!(
            unfilled_placeholders("See you {day}. {day}? And {day before}."),
            ["{day}", "{day before}"]
        );
        assert!(unfilled_placeholders("const a = {};\nfn({ x: 1 })").is_empty());
        assert!(unfilled_placeholders("All done.").is_empty());
    }

    fn snip(id: &str, trigger: &str, title: &str, body: &str, uses: u32) -> Snippet {
        Snippet {
            id: id.into(),
            trigger: trigger.into(),
            title: title.into(),
            body: body.into(),
            uses,
            ..Default::default()
        }
    }

    #[test]
    fn snippets_match_by_trigger_first_then_name_and_text_most_used_first() {
        let list = [
            snip("a", "price", "Pricing", "", 1),
            snip("b", "thx", "Thanks", "pricing attached", 9),
            snip("c", "pr", "PR review", "", 3),
        ];
        let ids = |typed: &str| {
            match_snippets(&list, typed)
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("pr"), ["c", "a", "b"]);
        assert_eq!(ids(""), ["b", "c", "a"]);
        assert_eq!(ids("thanks"), ["b"]);
    }

    #[test]
    fn extras_skip_addresses_already_on_the_message() {
        let s = Snippet {
            subject: " Pricing ".into(),
            cc: vec![
                "Dana <dana@harbor.example>".into(),
                "priya@mail.northwind.example".into(),
                "not an address".into(),
            ],
            bcc: vec![
                "dana@harbor.example".into(),
                "\"Ops\" <ops@x.example>".into(),
            ],
            ..Default::default()
        };
        let to = [addr(None, "Priya@mail.northwind.example")];
        let x = snippet_extras(&s, "", &to, &[], &[]);
        assert_eq!(x.subject.as_deref(), Some("Pricing"));
        assert_eq!(x.cc, [addr(Some("Dana"), "dana@harbor.example")]);
        assert_eq!(x.bcc, [addr(Some("Ops"), "ops@x.example")]);
        assert_eq!(snippet_extras(&s, "Re: hi", &to, &[], &[]).subject, None);
    }

    #[test]
    fn a_trigger_is_a_semicolon_word_before_the_caret() {
        assert_eq!(trigger_before_caret(";th", 3), Some((0, "th".into())));
        assert_eq!(trigger_before_caret("Hi ;", 4), Some((3, String::new())));
        assert_eq!(trigger_before_caret("Hi ;Thx", 7), Some((3, "thx".into())));
        assert_eq!(trigger_before_caret("a;b", 3), None, "inside a word");
        assert_eq!(trigger_before_caret("Hi ;th x", 8), None, "a space ends it");
        assert_eq!(trigger_before_caret("no semicolon", 5), None);
    }
}
