//! Who does a phrase mean? "mike from kettle on the knoll", "mike kotk",
//! "fernwood", "priya@linden.example", "uber" → one person or one company,
//! resolved against the people table (name and address tokens), email
//! domains, acronyms of domains ("kotk" = kettleontheknoll) and account or
//! profile names. Deterministic; ties become alternatives the UI shows as
//! "did you mean" chips rather than a silent guess.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};

use crate::text;
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetKind {
    Person,
    Company,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub kind: TargetKind,
    /// Display: the person's name, or the company as typed ("Kettle On The Knoll").
    pub label: String,
    pub name: Option<String>,
    /// Lowercased addresses, most mail first.
    pub emails: Vec<String>,
    /// Company targets: the domains all mail with them lives under.
    pub domains: Vec<String>,
    /// How the phrase was matched, for "how I got this".
    pub how: String,
    /// Matched loosely (acronym, partial name): worth a confidence notch.
    pub loose: bool,
    /// sent_to × 3 + from, used to rank.
    weight: i64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Resolution {
    pub target: Option<Target>,
    /// Other plausible matches, best first.
    pub alternatives: Vec<Target>,
    /// Other people the name fits just as well ("Mike": Mike Chen and Mike
    /// Delgado), however little mail you have with them, so an answer can
    /// say the name is ambiguous instead of silently picking one.
    pub namesakes: Vec<Target>,
}

/// A name standing for a set of domains (account nickname, profile name).
#[derive(Debug, Clone)]
pub(crate) struct Alias {
    pub name: String,
    pub domains: Vec<String>,
}

/// Words that join a name to a company, or say nothing about who.
const STOP: &[&str] = &[
    "from", "at", "of", "the", "with", "on", "in", "and", "my", "our", "guy", "person", "team",
    "folks", "people", "contact", "company", "client", "@", "'s", "a", "an", "mr", "mrs", "ms",
    "dr", "inc", "llc", "ltd", "co",
];

/// Most candidate rows fetched per lookup.
const CANDIDATES: i64 = 400;

#[derive(Debug, Clone)]
struct Person {
    email: String,
    name: Option<String>,
    weight: i64,
}

fn domain_of(email: &str) -> &str {
    email.rsplit_once('@').map_or("", |(_, d)| d)
}

/// The domain labels that name the organization: "mail.uber.com" → ["mail", "uber"],
/// "kettleontheknoll.example" → ["kettleontheknoll"].
fn org_labels(domain: &str) -> Vec<&str> {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() <= 1 {
        return parts;
    }
    let mut keep = parts.len() - 1;
    // co.uk, com.au, com.mx: drop the second-level too.
    if parts.len() >= 3
        && matches!(
            parts[parts.len() - 2],
            "co" | "com" | "org" | "net" | "gov" | "ac" | "edu"
        )
        && parts[parts.len() - 1].len() == 2
    {
        keep -= 1;
    }
    parts[..keep].to_vec()
}

/// The registrable domain ("mail.uber.com" → "uber.com"), used to group a company.
pub(crate) fn base_domain(domain: &str) -> String {
    let labels = org_labels(domain);
    match labels.last() {
        Some(l) => {
            let at = domain.find(l).unwrap_or(0);
            domain[at..].to_string()
        }
        None => domain.to_string(),
    }
}

/// Can `label` be cut into pieces (each ≥2 letters, except a trailing
/// single letter) whose initials spell `acr`? "kettleontheknoll" / "kotk".
fn acronym_of(label: &str, acr: &str) -> bool {
    fn go(l: &[u8], a: &[u8]) -> bool {
        match (l.first(), a.first()) {
            (None, None) => true,
            (_, None) | (None, _) => false,
            (Some(c), Some(x)) if c != x => false,
            _ if a.len() == 1 => l.len() <= 14,
            _ => (2..=l.len().min(14)).any(|k| l.get(k) == Some(&a[1]) && go(&l[k..], &a[1..])),
        }
    }
    acr.len() >= 3
        && acr.len() <= 6
        && label.len() >= acr.len() * 2
        && go(label.as_bytes(), acr.as_bytes())
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Default, Clone, Copy)]
struct Cover {
    ok: bool,
    /// Some word matched the person's name or mailbox (not just the domain).
    named: bool,
    /// Some word only matched loosely (acronym, substring of a domain).
    loose: bool,
    /// Some span matched the domain/alias (a company qualifier).
    org: bool,
    /// Every name word matched a whole token ("42" = "42", not "4200").
    exact: bool,
}

/// Does every meaningful word of the phrase describe `p`?
fn cover(words: &[String], p: &Person, aliases: &[Alias]) -> Cover {
    let domain = domain_of(&p.email);
    let labels = org_labels(domain);
    let local = p.email.split('@').next().unwrap_or("");
    let mut name_toks: Vec<String> = text::tokens(p.name.as_deref().unwrap_or(""))
        .into_iter()
        .map(|t| t.2)
        .collect();
    name_toks.extend(text::tokens(local).into_iter().map(|t| t.2));
    let mut out = Cover {
        ok: true,
        exact: true,
        ..Cover::default()
    };
    let mut i = 0;
    let mut matched_any = false;
    'words: while i < words.len() {
        // Longest span first: "kettle on the knoll" → "kettleontheknoll".
        for j in (i + 1..=words.len()).rev() {
            let span = &words[i..j];
            let joined = span.join(" ");
            if aliases.iter().any(|a| {
                a.name == joined
                    && a.domains
                        .iter()
                        .any(|d| domain == d || domain.ends_with(&format!(".{d}")))
            }) {
                out.org = true;
                matched_any = true;
                i = j;
                continue 'words;
            }
            if j - i >= 2 {
                let concat: String = span.concat();
                if concat.len() >= 5 && labels.iter().any(|l| l.contains(&concat)) {
                    out.org = true;
                    matched_any = true;
                    i = j;
                    continue 'words;
                }
            }
        }
        let w = &words[i];
        if STOP.contains(&w.as_str()) {
            i += 1;
            continue;
        }
        if name_toks.iter().any(|t| t.starts_with(w.as_str())) {
            out.named = true;
            out.exact &= name_toks.iter().any(|t| t == w);
        } else if labels
            .iter()
            .any(|l| l.starts_with(w.as_str()) || (w.len() >= 4 && l.contains(w.as_str())))
        {
            out.org = true;
        } else if labels.iter().any(|l| acronym_of(l, w)) {
            out.org = true;
            out.loose = true;
        } else {
            out.ok = false;
            return out;
        }
        matched_any = true;
        i += 1;
    }
    out.ok = matched_any;
    out
}

fn like_sub(w: &str) -> String {
    let e = w
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%@%{e}%")
}

fn fetch(c: &Connection, sql: &str, arg: &str, into: &mut HashMap<String, Person>) -> Result<()> {
    fetch_with(c, sql, params![arg, CANDIDATES], into)
}

/// People with a word starting with `w` (`prefix_sql`).
fn fetch_word(
    c: &Connection,
    sql: &str,
    w: &str,
    into: &mut HashMap<String, Person>,
) -> Result<()> {
    let [lo, hi] = crate::store::word_range(w, true);
    fetch_with(c, sql, params![lo, CANDIDATES, hi], into)
}

fn fetch_with(
    c: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
    into: &mut HashMap<String, Person>,
) -> Result<()> {
    let mut stmt = c.prepare_cached(sql)?;
    let rows = stmt.query_map(args, |r| {
        Ok(Person {
            email: r.get::<_, String>(0)?.to_lowercase(),
            name: r.get(1)?,
            weight: r.get(2)?,
        })
    })?;
    for p in rows {
        let p = p?;
        into.entry(p.email.clone()).or_insert(p);
    }
    Ok(())
}

const SELECT: &str = "SELECT email, name, sent_to_count * 3 + from_count FROM people WHERE from_count + sent_to_count > 0";

/// Resolve `phrase`, which may list alternatives ("x / y", "x or y"); the
/// best match of each part is merged into one target.
pub(crate) fn resolve(
    c: &Connection,
    phrase: &str,
    own: &HashSet<String>,
    aliases: &[Alias],
) -> Result<Resolution> {
    let parts: Vec<String> = phrase
        .replace(" or ", " / ")
        .split('/')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() <= 1 {
        return resolve_one(c, phrase.trim(), own, aliases);
    }
    let mut merged: Option<Target> = None;
    let mut alternatives = Vec::new();
    for part in &parts {
        let r = resolve_one(c, part, own, aliases)?;
        alternatives.extend(r.alternatives);
        let Some(mut t) = r.target else { continue };
        // "Mike from KOTK / Fernwood": if the second part is a company whose
        // only addresses are the same person, it's one person at two places.
        if let (Some(m), TargetKind::Company) = (&merged, t.kind) {
            if let Some(n) = &m.name {
                let same = t.emails.len() <= 3
                    && t.emails.iter().all(|e| {
                        c.prepare_cached("SELECT name FROM people WHERE email = ?1")
                            .and_then(|mut s| {
                                s.query_row(params![e], |r| r.get::<_, Option<String>>(0))
                            })
                            .ok()
                            .flatten()
                            .is_some_and(|x| x.eq_ignore_ascii_case(n))
                    });
                if same {
                    t.kind = TargetKind::Person;
                    t.name = m.name.clone();
                    t.label = m.label.clone();
                }
            }
        }
        merged = Some(match merged {
            None => t,
            Some(mut m) => {
                for e in t.emails {
                    if !m.emails.contains(&e) {
                        m.emails.push(e);
                    }
                }
                for d in t.domains {
                    if !m.domains.contains(&d) {
                        m.domains.push(d);
                    }
                }
                if m.name.is_some() && m.name == t.name {
                    // Same person at two addresses.
                } else if !m.label.split(" / ").any(|l| l == t.label) {
                    m.label = format!("{} / {}", m.label, t.label);
                }
                m.how = format!("{}; {}", m.how, t.how);
                m.loose |= t.loose;
                m.weight += t.weight;
                m
            }
        });
    }
    if let Some(m) = &merged {
        alternatives.retain(|a| !a.emails.iter().all(|e| m.emails.contains(e)));
    }
    Ok(Resolution {
        target: merged,
        alternatives,
        namesakes: vec![],
    })
}

fn resolve_one(
    c: &Connection,
    phrase: &str,
    own: &HashSet<String>,
    aliases: &[Alias],
) -> Result<Resolution> {
    let phrase = phrase
        .trim()
        .trim_matches(|ch: char| matches!(ch, '<' | '>' | '"' | '\''));
    // A literal address.
    if phrase.contains('@') && !phrase.contains(' ') {
        let email = phrase.to_lowercase();
        let row: Option<(Option<String>, i64)> = c
            .prepare_cached(
                "SELECT name, sent_to_count * 3 + from_count FROM people WHERE email = ?1",
            )?
            .query_row(params![email], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        let (name, weight) = row.unwrap_or((None, 0));
        let label = name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| email.clone());
        return Ok(Resolution {
            target: Some(Target {
                kind: TargetKind::Person,
                label,
                name,
                emails: vec![email.clone()],
                domains: vec![],
                how: format!("\"{phrase}\" is an address"),
                loose: false,
                weight,
            }),
            alternatives: vec![],
            namesakes: vec![],
        });
    }
    // "@linden.example" / "linden.example": a whole domain.
    let bare = phrase.trim_start_matches('@');
    if bare.contains('.') && !bare.contains(' ') && bare.split('.').all(|p| !p.is_empty()) {
        let domain = bare.to_lowercase();
        let mut people = HashMap::new();
        fetch(c, &format!("{SELECT} AND (email LIKE '%@' || ?1 OR email LIKE '%.' || ?1) ORDER BY 3 DESC LIMIT ?2"), &domain, &mut people)?;
        people.retain(|e, _| !own.contains(e));
        if !people.is_empty() {
            return Ok(Resolution {
                target: Some(company(
                    &domain,
                    &domain,
                    people.into_values().collect(),
                    false,
                    format!("\"{phrase}\" is a domain"),
                )),
                alternatives: vec![],
                namesakes: vec![],
            });
        }
    }

    let words: Vec<String> = phrase
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect();
    let tokens: Vec<String> = words
        .iter()
        .flat_map(|w| text::tokens(w).into_iter().map(|t| t.2))
        .collect();
    let meaningful: Vec<&String> = tokens
        .iter()
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect();
    if meaningful.is_empty() {
        return Ok(Resolution::default());
    }

    // Candidates: name/address token prefixes, domain substrings, joined
    // spans ("kettle on the knoll"), and alias domains.
    let mut people: HashMap<String, Person> = HashMap::new();
    // Word prefixes as an index range over `people_words` (?1..?3).
    let prefix_sql = format!(
        "{SELECT} AND email IN (SELECT email FROM people_words WHERE word BETWEEN ?1 AND ?3) ORDER BY 3 DESC LIMIT ?2"
    );
    let domain_sql = format!("{SELECT} AND email LIKE ?1 ESCAPE '\\' ORDER BY 3 DESC LIMIT ?2");
    for w in &meaningful {
        fetch_word(c, &prefix_sql, w, &mut people)?;
        if w.len() >= 4 {
            fetch(c, &domain_sql, &like_sub(w), &mut people)?;
        }
    }
    for i in 0..tokens.len() {
        for j in i + 2..=tokens.len() {
            let concat = tokens[i..j].concat();
            if concat.len() >= 5 {
                fetch(c, &domain_sql, &like_sub(&concat), &mut people)?;
            }
        }
    }
    for a in aliases {
        if phrase.to_lowercase().contains(&a.name) {
            for d in &a.domains {
                fetch(c, &format!("{SELECT} AND (email LIKE '%@' || ?1 OR email LIKE '%.' || ?1) ORDER BY 3 DESC LIMIT ?2"), d, &mut people)?;
            }
        }
    }
    people.retain(|e, _| !own.contains(e));

    let mut full: Vec<(Person, Cover)> = people
        .values()
        .map(|p| (p.clone(), cover(&tokens, p, aliases)))
        .filter(|(_, cv)| cv.ok)
        .collect();

    // Nothing? Try acronyms against every domain we know ("kotk").
    if full.is_empty() {
        let short: Vec<&String> = meaningful
            .iter()
            .copied()
            .filter(|w| (3..=6).contains(&w.len()))
            .collect();
        if !short.is_empty() {
            let mut stmt = c.prepare_cached(SELECT)?;
            let rows = stmt.query_map([], |r| {
                Ok(Person {
                    email: r.get::<_, String>(0)?.to_lowercase(),
                    name: r.get(1)?,
                    weight: r.get(2)?,
                })
            })?;
            for p in rows {
                let p = p?;
                if own.contains(&p.email) {
                    continue;
                }
                let labels = org_labels(domain_of(&p.email));
                if short
                    .iter()
                    .any(|w| labels.iter().any(|l| acronym_of(l, w)))
                {
                    let cv = cover(&tokens, &p, aliases);
                    if cv.ok {
                        full.push((p, cv));
                    }
                }
            }
        }
    }
    if full.is_empty() {
        return Ok(Resolution::default());
    }

    let named: Vec<&(Person, Cover)> = full.iter().filter(|(_, cv)| cv.named).collect();
    if !named.is_empty() {
        let mut ranked: Vec<&(Person, Cover)> = named;
        ranked.sort_by(|a, b| {
            b.1.exact
                .cmp(&a.1.exact)
                .then(b.0.weight.cmp(&a.0.weight))
                .then_with(|| a.0.email.cmp(&b.0.email))
        });
        let qualified = ranked.iter().any(|(_, cv)| cv.org);
        let top = &ranked[0].0;
        let mut target = person(top, ranked[0].1, phrase);
        // The same full name at other addresses (unless a company narrowed it).
        if let Some(n) = top
            .name
            .as_deref()
            .filter(|n| n.split_whitespace().count() >= 2)
        {
            for (p, _) in ranked.iter().skip(1) {
                if p.name.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(n))
                    && !target.emails.contains(&p.email)
                {
                    target.emails.push(p.email.clone());
                    target.weight += p.weight;
                }
            }
            if !qualified {
                let mut stmt = c.prepare_cached("SELECT email FROM people WHERE name = ?1 COLLATE NOCASE ORDER BY last_date DESC LIMIT 5")?;
                for e in stmt.query_map(params![n], |r| r.get::<_, String>(0))? {
                    let e = e?.to_lowercase();
                    if !own.contains(&e) && !target.emails.contains(&e) {
                        target.emails.push(e);
                    }
                }
            }
        }
        let alternatives: Vec<Target> = ranked
            .iter()
            .skip(1)
            .filter(|(p, _)| !target.emails.contains(&p.email))
            .filter(|(p, _)| p.weight * 10 >= top.weight.max(1))
            .take(4)
            .map(|(p, cv)| person(p, *cv, phrase))
            .collect();
        // A close second with a different name makes the pick uncertain.
        if alternatives
            .first()
            .is_some_and(|a| a.weight * 3 >= top.weight)
        {
            target.loose = true;
        }
        // Everyone else whose full name has the typed words as whole words
        // ("mike" → Mike Chen, Mike Delgado), one per name.
        let mut names: Vec<String> = top.name.iter().map(|n| n.to_lowercase()).collect();
        let mut namesakes = Vec::new();
        for (p, cv) in ranked.iter().skip(1) {
            let Some(n) = p.name.as_deref().filter(|n| n.split_whitespace().count() >= 2) else {
                continue;
            };
            if !cv.exact || target.emails.contains(&p.email) || names.contains(&n.to_lowercase()) {
                continue;
            }
            names.push(n.to_lowercase());
            namesakes.push(person(p, *cv, phrase));
            if namesakes.len() == 4 {
                break;
            }
        }
        return Ok(Resolution {
            target: Some(target),
            alternatives,
            namesakes,
        });
    }

    // Only the company matched: group by registrable domain.
    let mut by_domain: HashMap<String, Vec<Person>> = HashMap::new();
    let mut loose_domains: HashSet<String> = HashSet::new();
    for (p, cv) in &full {
        let d = base_domain(domain_of(&p.email));
        if cv.loose {
            loose_domains.insert(d.clone());
        }
        by_domain.entry(d).or_default().push(p.clone());
    }
    let mut groups: Vec<(String, Vec<Person>)> = by_domain.into_iter().collect();
    groups.sort_by(|a, b| {
        let wa: i64 = a.1.iter().map(|p| p.weight).sum();
        let wb: i64 = b.1.iter().map(|p| p.weight).sum();
        wb.cmp(&wa).then_with(|| a.0.cmp(&b.0))
    });
    let label = title_case(phrase);
    let mut out: Vec<Target> = groups
        .into_iter()
        .take(5)
        .map(|(d, ps)| {
            let loose = loose_domains.contains(&d);
            let how = format!(
                "\"{phrase}\" matched the domain {d}{}",
                if loose { " (as an acronym)" } else { "" }
            );
            company(&d, &label, ps, loose, how)
        })
        .collect();
    // All of the company's addresses, not just the ones the phrase fetched.
    let mut target = out.remove(0);
    let mut stmt = c.prepare_cached(&format!(
        "{SELECT} AND (email LIKE '%@' || ?1 OR email LIKE '%.' || ?1) ORDER BY 3 DESC LIMIT ?2"
    ))?;
    for p in stmt.query_map(params![target.domains[0], CANDIDATES], |r| {
        Ok((r.get::<_, String>(0)?.to_lowercase(), r.get::<_, i64>(2)?))
    })? {
        let (e, _) = p?;
        if !own.contains(&e) && !target.emails.contains(&e) {
            target.emails.push(e);
        }
    }
    if out.first().is_some_and(|a| a.weight * 3 >= target.weight) {
        target.loose = true;
    }
    Ok(Resolution {
        target: Some(target),
        alternatives: out,
        namesakes: vec![],
    })
}

fn person(p: &Person, cv: Cover, phrase: &str) -> Target {
    let name = p.name.clone().filter(|n| !n.trim().is_empty());
    let label = name.clone().unwrap_or_else(|| p.email.clone());
    let how = if cv.org {
        format!(
            "\"{phrase}\" matched {label} <{}> by name and company{}",
            p.email,
            if cv.loose {
                " (company as an acronym)"
            } else {
                ""
            }
        )
    } else {
        format!("\"{phrase}\" matched {label} <{}>", p.email)
    };
    Target {
        kind: TargetKind::Person,
        label,
        name,
        emails: vec![p.email.clone()],
        domains: vec![],
        how,
        loose: cv.loose,
        weight: p.weight,
    }
}

fn company(domain: &str, label: &str, mut people: Vec<Person>, loose: bool, how: String) -> Target {
    people.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.email.cmp(&b.email)));
    let weight = people.iter().map(|p| p.weight).sum();
    Target {
        kind: TargetKind::Company,
        label: if label.contains('.') {
            label.to_string()
        } else {
            format!("{label} ({domain})")
        },
        name: None,
        emails: people.into_iter().map(|p| p.email).collect(),
        domains: vec![domain.to_string()],
        how,
        loose,
        weight,
    }
}

/// A person at a company domain → the whole company (every address at
/// that domain). None when it's already a company or a public mail domain.
pub(crate) fn widen(
    c: &Connection,
    t: &Target,
    phrase: &str,
    own: &HashSet<String>,
    public: &[&str],
) -> Result<Option<Target>> {
    if t.kind == TargetKind::Company {
        return Ok(None);
    }
    let domains: HashSet<String> = t.emails.iter().map(|e| base_domain(domain_of(e))).collect();
    if domains.len() != 1 {
        return Ok(None);
    }
    let d = domains.into_iter().next().unwrap_or_default();
    if d.is_empty() || public.contains(&d.as_str()) {
        return Ok(None);
    }
    let mut people = HashMap::new();
    fetch(c, &format!("{SELECT} AND (email LIKE '%@' || ?1 OR email LIKE '%.' || ?1) ORDER BY 3 DESC LIMIT ?2"), &d, &mut people)?;
    people.retain(|e, _| !own.contains(e));
    if people.len() <= 1 {
        return Ok(None);
    }
    let how = format!("{}; widened to the domain {d}", t.how);
    Ok(Some(company(
        &d,
        &title_case(phrase),
        people.into_values().collect(),
        t.loose,
        how,
    )))
}

/// A target for addresses we already know (pronouns: the previous answer's person).
pub(crate) fn known(
    c: &Connection,
    emails: &[String],
    label: Option<&str>,
) -> Result<Option<Target>> {
    let emails: Vec<String> = emails
        .iter()
        .map(|e| e.trim().to_lowercase())
        .filter(|e| e.contains('@'))
        .collect();
    let Some(first) = emails.first() else {
        return Ok(None);
    };
    let name: Option<String> = c
        .prepare_cached("SELECT name FROM people WHERE email = ?1")?
        .query_row(params![first], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .filter(|n| !n.trim().is_empty());
    let label = label
        .map(String::from)
        .or_else(|| name.clone())
        .unwrap_or_else(|| first.clone());
    Ok(Some(Target {
        kind: TargetKind::Person,
        label,
        name,
        emails: emails.clone(),
        domains: vec![],
        how: "the person from the previous answer".into(),
        loose: false,
        weight: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acronyms() {
        assert!(acronym_of("kettleontheknoll", "kotk"));
        assert!(!acronym_of("lindenpartners", "lp")); // too short
        assert!(acronym_of("northwindtraders", "nwt"));
        assert!(!acronym_of("kettleontheknoll", "kotx"));
        assert!(!acronym_of("uber", "ubr"));
    }

    #[test]
    fn domains() {
        assert_eq!(org_labels("mail.uber.com"), vec!["mail", "uber"]);
        assert_eq!(
            org_labels("kettleontheknoll.example"),
            vec!["kettleontheknoll"]
        );
        assert_eq!(org_labels("shop.example.co.uk"), vec!["shop", "example"]);
        assert_eq!(base_domain("mail.uber.com"), "uber.com");
        assert_eq!(base_domain("uber.com"), "uber.com");
        assert_eq!(base_domain("shop.example.co.uk"), "example.co.uk");
    }
}
