//! Query-side lexical variants, checked against the full-text index.
//!
//! The index is SQLite FTS5 with `unicode61 remove_diacritics 2`: no
//! stemming, no compound handling, and letters without a decomposition
//! (ø, ß, æ, ł, đ, þ, œ) stay as they are. Rather than re-tokenize every
//! mailbox, search adds, for each plain word of the query, the other forms
//! the mail may use, and keeps only the forms the index actually holds
//! (each check is one seek in the FTS index, no vocabulary scan):
//!
//! - **Inflections**: invoices ↔ invoice, shipped ↔ ship ↔ ships ↔
//!   shipping, results ↔ result, possessives written without the
//!   apostrophe (sofias → sofia). Derivations (renewal ↔ renewing) are left
//!   to search by meaning: they change the word class and often the sense.
//! - **British and American spelling**: colour/color, organise/organize,
//!   centre/center, licence/license, catalogue/catalog, cancelled/canceled,
//!   plus a short list (grey/gray, mum/mom…).
//! - **Compounds and hyphenation**: wi-fi → wifi, pick up ↔ pickup,
//!   oncall → on call, leaserenewal / LeaseRenewal → lease renewal, lea se →
//!   lease.
//! - **Letters the tokenizer doesn't fold**, for a word no mail contains:
//!   odegard → ødegard, hauptstrasse → hauptstraße, mueller → muller.
//! - **Codes and numbers typed in another format**: INV20417 → inv 20417,
//!   2450 → 2 450 (the mail's "$2,450"), 1234.56 → 1 234 56 ("1.234,56"),
//!   $2,450.00 → 2 450, 2.14.3 → v2 14 3, digit groups typed apart or
//!   together (9400 1118 … ↔ 9400111899…), phone numbers with a country
//!   code or a trunk 0 (+1 415 555 0138, 020 7946 0958).
//! - **One typo** in a word no mail contains, corrected to the indexed word
//!   one edit away that occurs with the query's other words (as
//!   `hybrid::respell` does with meaning on; this covers keyword search).
//!
//! The typed form always stays one of the alternatives, so a variant can only
//! add matches. Phrases and exclusions are never touched: quotes and `-`
//! ask for exactly what was typed.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::query::{is_noise_word, Atom, Clause, ParsedQuery, Term};
use crate::text;
use crate::Result;

/// Most lookups one query may spend (each is one FTS seek, ~10–50 µs).
const MAX_LOOKUPS: usize = 400;
/// Messages from which a word counts as common: the way the mail writes it,
/// not a compound to split.
const COMMON_WORD: i64 = 500;
/// Most alternatives kept per word.
const MAX_ALTS: usize = 8;

/// Index lookups with a per-query cache and budget.
struct Index<'c> {
    c: &'c Connection,
    seen: HashMap<String, i64>,
    lookups: usize,
}

impl<'c> Index<'c> {
    fn new(c: &'c Connection) -> Self {
        Index {
            c,
            seen: HashMap::new(),
            lookups: 0,
        }
    }

    /// Documents (capped at `cap`) matching the FTS expression.
    fn count(&mut self, expr: &str, cap: i64) -> Result<i64> {
        let key = format!("{cap}\u{1}{expr}");
        if let Some(n) = self.seen.get(&key) {
            return Ok(*n);
        }
        if self.lookups >= MAX_LOOKUPS {
            return Ok(0);
        }
        self.lookups += 1;
        let n: i64 = self
            .c
            .prepare_cached(
                "SELECT count(*) FROM (SELECT 1 FROM messages_fts WHERE messages_fts MATCH ?1 LIMIT ?2)",
            )?
            .query_row(rusqlite::params![expr, cap], |r| r.get(0))?;
        self.seen.insert(key, n);
        Ok(n)
    }

    /// Does any message contain these words, adjacent and in order?
    fn has(&mut self, words: &[&str]) -> Result<bool> {
        if words.is_empty() || words.iter().any(|w| w.is_empty()) {
            return Ok(false);
        }
        Ok(self.count(&phrase(words), 1)? > 0)
    }

    /// The single words of `cands` some message contains. Most candidates
    /// (inflections, one-edit neighbours) exist nowhere, so one OR query
    /// per batch rules them out before any per-word lookup.
    fn existing(&mut self, cands: &[String]) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for batch in cands.chunks(64) {
            self.split_existing(batch, &mut out)?;
        }
        Ok(out)
    }

    /// Halve a batch while some word in it exists: k existing words among n
    /// cost about k·log2(n) lookups instead of n.
    fn split_existing(&mut self, batch: &[String], out: &mut Vec<String>) -> Result<()> {
        match batch {
            [] => Ok(()),
            [w] => {
                if self.has(&[w])? {
                    out.push(w.clone());
                }
                Ok(())
            }
            _ => {
                let expr = batch.iter().map(|w| phrase(&[w])).collect::<Vec<_>>().join(" OR ");
                if self.count(&expr, 1)? == 0 {
                    return Ok(());
                }
                let (a, b) = batch.split_at(batch.len() / 2);
                self.split_existing(a, out)?;
                self.split_existing(b, out)
            }
        }
    }

    /// A word of some correspondent's name or address (`people_words`).
    fn is_person_word(&mut self, w: &str) -> Result<bool> {
        let key = format!("person\u{1}{w}");
        if let Some(n) = self.seen.get(&key) {
            return Ok(*n > 0);
        }
        self.lookups += 1;
        let n: i64 = self
            .c
            .prepare_cached("SELECT count(*) FROM (SELECT 1 FROM people_words WHERE word = ?1 LIMIT 1)")?
            .query_row([w], |r| r.get(0))?;
        self.seen.insert(key, n);
        Ok(n > 0)
    }

    fn has_prefix(&mut self, w: &str) -> Result<bool> {
        Ok(self.count(&format!("{}*", phrase(&[w])), 1)? > 0)
    }

    fn df(&mut self, words: &[&str]) -> Result<i64> {
        self.count(&phrase(words), 1000)
    }
}

/// An FTS5 phrase of already-folded words.
fn phrase(words: &[&str]) -> String {
    format!("\"{}\"", words.join(" ").replace('"', ""))
}

fn is_letters(w: &str) -> bool {
    !w.is_empty() && w.chars().all(char::is_alphabetic)
}

fn is_digits(w: &str) -> bool {
    !w.is_empty() && w.chars().all(|c| c.is_ascii_digit())
}

/// A single positive plain-word clause: (clause index, typed text, prefix).
fn plain(cl: &Clause) -> Option<(&str, bool)> {
    match cl.terms.as_slice() {
        [Term {
            negated: false,
            atom:
                Atom::Text {
                    text,
                    phrase: false,
                    prefix,
                },
        }] => Some((text.as_str(), *prefix)),
        _ => None,
    }
}

/// Add index-checked variants to the query's plain words
/// (`ParsedQuery::alternatives`), merging typed-apart words and digit groups
/// into one term where the mail writes them together.
pub(crate) fn expand(c: &Connection, q: &mut ParsedQuery) -> Result<()> {
    if !q.clauses.iter().any(|cl| plain(cl).is_some()) {
        return Ok(());
    }
    let mut ix = Index::new(c);
    merge_digit_runs(&mut ix, q)?;
    merge_split_words(&mut ix, q)?;
    // The query's known words, as context for typo corrections.
    let mut context: Vec<String> = Vec::new();
    for cl in &q.clauses {
        if let Some((t, prefix)) = plain(cl) {
            for tok in text::tokens(t) {
                let w = tok.2;
                if !is_noise_word(&w) && !prefix && ix.has(&[&w])? && !context.contains(&w) {
                    context.push(w);
                }
            }
        }
    }
    let words: Vec<(String, bool)> = q
        .clauses
        .iter()
        .filter_map(plain)
        .map(|(t, p)| (t.to_string(), p))
        .collect();
    for (typed, prefix) in words {
        if q.alternatives.contains_key(&typed) {
            continue;
        }
        let mut alts = variants(&mut ix, &typed, prefix, &context)?;
        alts.retain(|a| *a != typed.to_lowercase());
        alts.dedup();
        alts.truncate(MAX_ALTS);
        if !alts.is_empty() {
            q.alternatives.insert(typed, alts);
        }
    }
    Ok(())
}

/// Does the mail hold `w` (a word no message contains) in another form than
/// a typo: joined with a neighbouring word ("sour dough" → sourdough), with
/// letters the tokenizer keeps, split into indexed words, or inflected?
/// Typo correction must leave such words to `expand` ("sour" is one edit
/// from "your").
pub(crate) fn other_form(c: &Connection, w: &str, prev: Option<&str>, next: Option<&str>) -> Result<bool> {
    let mut ix = Index::new(c);
    for joined in [prev.map(|p| format!("{p}{w}")), next.map(|n| format!("{w}{n}"))].into_iter().flatten() {
        if ix.has(&[&joined])? {
            return Ok(true);
        }
    }
    if !ix.existing(&unfolded(w))?.is_empty() {
        return Ok(true);
    }
    if w.is_ascii() && is_letters(w) && !ix.existing(&forms(w))?.is_empty() {
        return Ok(true);
    }
    Ok(segment(&mut ix, w)?.is_some())
}

/// The other forms of one typed word (or hyphenated/dotted token run) that
/// the index holds, each as plain folded text (several words = a phrase).
fn variants(ix: &mut Index, typed: &str, prefix: bool, context: &[String]) -> Result<Vec<String>> {
    let toks: Vec<String> = text::tokens(typed).into_iter().map(|t| t.2).collect();
    let mut out: Vec<String> = Vec::new();
    let push = |v: String, out: &mut Vec<String>| {
        if !out.contains(&v) {
            out.push(v);
        }
    };
    match toks.as_slice() {
        [] => {}
        [w] => {
            let w = w.as_str();
            // A word being typed is a prefix from three letters on, as in
            // `fts_string` (a one-letter prefix query reads every term with it).
            let typing = prefix && w.chars().count() >= crate::search::MIN_PREFIX_CHARS;
            // (An exact hit is a cheap seek; a prefix query merges every term.)
            let known = ix.has(&[w])? || (typing && ix.has_prefix(w)?);
            // Rob for Robert, Bob and back: only names of people you have
            // mail with ("will" is a name nobody here has).
            for v in nicknames(w) {
                if ix.is_person_word(&v)? && ix.has(&[&v])? {
                    push(v, &mut out);
                }
            }
            // English inflections and spellings: ASCII words only.
            if w.is_ascii() && is_letters(w) && w.len() >= 3 && !is_noise_word(w) {
                let cands: Vec<String> = forms(w).into_iter().filter(|v| v != w).collect();
                for v in ix.existing(&cands)? {
                    push(v, &mut out);
                }
            }
            if !known {
                for v in ix.existing(&unfolded(w))? {
                    push(v, &mut out);
                }
                if let Some(seg) = segment(ix, w)? {
                    push(seg.join(" "), &mut out);
                }
                if out.is_empty() && is_letters(w) && w.chars().count() >= 4 {
                    if let Some(fix) = correction(ix, w, context)? {
                        push(fix, &mut out);
                    }
                }
            } else if is_letters(w) && w.chars().count() >= 5 {
                // A compound the mail mostly writes apart ("setup" → "set up").
                if let Some(split) = split_compound(ix, w)? {
                    push(split, &mut out);
                }
            }
        }
        many => {
            let words: Vec<&str> = many.iter().map(String::as_str).collect();
            // wi-fi → wifi, e-mails → emails, pick-up → pickup.
            if words.iter().all(|w| is_letters(w)) {
                let joined = words.concat();
                if ix.has(&[&joined])? {
                    push(joined, &mut out);
                }
            }
            if !ix.has(&words)? {
                for alt in reformatted(ix, &words)? {
                    push(alt, &mut out);
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// inflections and spelling

/// Words that end in -s but aren't plurals (or whose singular is another word).
const NOT_PLURAL: &[&str] = &[
    "news", "series", "species", "means", "physics", "mathematics", "maths", "economics",
    "politics", "lens", "chaos", "canvas", "atlas", "alias", "bias", "gas", "yes", "this",
    "thus", "plus", "bonus", "campus", "census", "status", "virus", "focus", "always",
    "perhaps", "whereas", "towards", "afterwards", "sometimes", "ups", "its", "his", "hers",
    "ours", "yours", "theirs", "was", "has", "does", "is", "us", "christmas", "texas", "paris",
    "james", "chris", "thomas", "lucas", "carlos", "jesus",
];

/// British ↔ American spellings that no rule covers.
const SPELLING_PAIRS: &[(&str, &str)] = &[
    ("grey", "gray"),
    ("greys", "grays"),
    ("mum", "mom"),
    ("programme", "program"),
    ("programmes", "programs"),
    ("tyre", "tire"),
    ("tyres", "tires"),
    ("aluminium", "aluminum"),
    ("jewellery", "jewelry"),
    ("mould", "mold"),
    ("plough", "plow"),
    ("pyjamas", "pajamas"),
    ("cosy", "cozy"),
    ("sceptical", "skeptical"),
    ("kerb", "curb"),
    ("enrol", "enroll"),
    ("fulfil", "fulfill"),
    ("instalment", "installment"),
    ("judgement", "judgment"),
    ("ageing", "aging"),
    ("manoeuvre", "maneuver"),
    ("paediatric", "pediatric"),
    ("paediatrician", "pediatrician"),
    ("anaesthesia", "anesthesia"),
    ("orthopaedic", "orthopedic"),
    ("practise", "practice"),
    ("maths", "math"),
    ("cheque", "check"),
    ("cheques", "checks"),
    ("postcode", "zipcode"),
];

/// English given names and their common short forms: someone who signs
/// "Bob" is Robert Kowalski in the address book. Each group is one person's
/// names; only the forms some mail contains are used.
const NICKNAMES: &[&[&str]] = &[
    &["robert", "rob", "bob", "bobby", "robbie"],
    &["william", "will", "bill", "billy", "liam"],
    &["michael", "mike", "mick", "mikey"],
    &["elizabeth", "liz", "beth", "betty", "lizzie", "eliza"],
    &["katherine", "catherine", "kathryn", "kate", "katie", "kathy", "cathy", "kat"],
    &["james", "jim", "jimmy", "jamie"],
    &["thomas", "tom", "tommy"],
    &["richard", "rick", "dick", "rich", "ricky"],
    &["jennifer", "jen", "jenny"],
    &["jessica", "jess", "jessie"],
    &["christopher", "chris", "topher"],
    &["christine", "christina", "chris", "tina"],
    &["alexander", "alex", "xander", "sasha"],
    &["alexandra", "alex", "alexa", "sasha"],
    &["daniel", "dan", "danny"],
    &["joseph", "joe", "joey"],
    &["margaret", "maggie", "meg", "peggy"],
    &["patricia", "pat", "patty", "trish"],
    &["patrick", "pat", "paddy"],
    &["benjamin", "ben", "benny"],
    &["samuel", "sam", "sammy"],
    &["samantha", "sam", "sammy"],
    &["anthony", "tony"],
    &["edward", "ed", "eddie", "ted", "ned"],
    &["nicholas", "nick", "nicky"],
    &["jonathan", "jon", "jonny"],
    &["matthew", "matt"],
    &["andrew", "andy", "drew"],
    &["steven", "stephen", "steve"],
    &["susan", "sue", "susie"],
    &["deborah", "debbie", "deb"],
    &["rebecca", "becky", "becca"],
    &["victoria", "vicky", "tori"],
    &["theodore", "theo", "ted"],
    &["gregory", "greg"],
    &["timothy", "tim"],
    &["kenneth", "ken", "kenny"],
    &["charles", "charlie", "chuck"],
    &["francisco", "paco", "pancho"],
];

/// A given name's other forms (Rob → Robert, Bob…).
fn nicknames(w: &str) -> Vec<String> {
    let mut out = Vec::new();
    for group in NICKNAMES {
        if group.contains(&w) {
            out.extend(group.iter().filter(|n| **n != w).map(|n| n.to_string()));
        }
    }
    out
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')
}

/// consonant-vowel-consonant ending (ship, plan, stop), where English
/// doubles the last letter before -ed/-ing.
fn cvc(w: &str) -> bool {
    let c: Vec<char> = w.chars().collect();
    let n = c.len();
    n >= 3
        && !is_vowel(c[n - 1])
        && !matches!(c[n - 1], 'w' | 'x' | 'y')
        && is_vowel(c[n - 2])
        && !is_vowel(c[n - 3])
}

/// Base forms `w` may be an inflection of (itself included).
fn bases(w: &str) -> Vec<String> {
    let mut out = vec![w.to_string()];
    let n = w.len();
    let strip = |k: usize| w[..n - k].to_string();
    if n > 4 && w.ends_with("ies") {
        out.push(format!("{}y", strip(3)));
    }
    if n > 3 && w.ends_with("es") {
        let stem = strip(2);
        if ["s", "x", "z", "ch", "sh", "o"].iter().any(|e| stem.ends_with(e)) {
            out.push(stem);
        }
    }
    if n > 3
        && w.ends_with('s')
        && !w.ends_with("ss")
        && !w.ends_with("us")
        && !w.ends_with("is")
        && !NOT_PLURAL.contains(&w)
    {
        out.push(strip(1));
    }
    for suffix in ["ed", "ing"] {
        let k = suffix.len();
        if n > k + 2 && w.ends_with(suffix) {
            let stem = strip(k);
            if suffix == "ed" && stem.ends_with('i') {
                // applied → apply
                out.push(format!("{}y", &stem[..stem.len() - 1]));
            }
            // A base under four letters is more often another word than the
            // stem ("wedding" is not "wed", "added" not "add").
            let ch: Vec<char> = stem.chars().collect();
            if ch.len() >= 5 && ch[ch.len() - 1] == ch[ch.len() - 2] && !is_vowel(ch[ch.len() - 1]) {
                // shipped → ship (but keep "cancell" for the -ll- rule below)
                out.push(ch[..ch.len() - 1].iter().collect());
            }
            if ch.len() >= 3 {
                out.push(format!("{stem}e"));
            }
            if ch.len() >= 4 {
                out.push(stem);
            }
        }
    }
    out
}

/// Inflected forms of a base.
fn inflect(b: &str) -> Vec<String> {
    let mut out = Vec::new();
    if b.ends_with('y') && b.len() > 2 && !b[..b.len() - 1].ends_with(is_vowel) {
        let s = &b[..b.len() - 1];
        out.push(format!("{s}ies"));
        out.push(format!("{s}ied"));
    } else if ["s", "x", "z", "ch", "sh"].iter().any(|e| b.ends_with(e)) {
        out.push(format!("{b}es"));
    } else {
        out.push(format!("{b}s"));
    }
    if let Some(s) = b.strip_suffix('e') {
        out.push(format!("{b}d"));
        out.push(format!("{s}ing"));
    } else {
        out.push(format!("{b}ed"));
        out.push(format!("{b}ing"));
        if cvc(b) && b.len() <= 5 {
            let last = b.chars().last().expect("non-empty");
            out.push(format!("{b}{last}ed"));
            out.push(format!("{b}{last}ing"));
        }
    }
    out
}

/// British ↔ American respellings of `w` (rules, then the list).
fn respellings(w: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rule = |from: &str, to: &str, at_end: bool| {
        if at_end {
            if let Some(stem) = w.strip_suffix(from) {
                if stem.len() >= 2 {
                    out.push(format!("{stem}{to}"));
                }
            }
        } else if let Some(i) = w.chars().next().and_then(|c| w[c.len_utf8()..].find(from).map(|i| i + c.len_utf8())) {
            out.push(format!("{}{to}{}", &w[..i], &w[i + from.len()..]));
        }
    };
    rule("our", "or", false);
    rule("or", "our", false);
    for (uk, us) in [
        ("isation", "ization"),
        ("isations", "izations"),
        ("ise", "ize"),
        ("ised", "ized"),
        ("ises", "izes"),
        ("ising", "izing"),
        ("yse", "yze"),
        ("ysed", "yzed"),
        ("tre", "ter"),
        ("tres", "ters"),
        ("ence", "ense"),
        ("ogue", "og"),
        ("ogues", "ogs"),
    ] {
        rule(uk, us, true);
        rule(us, uk, true);
    }
    // cancelled ↔ canceled, travelling ↔ traveling, labelled ↔ labeled.
    for suffix in ["ed", "ing", "er", "ers"] {
        let Some(stem) = w.strip_suffix(suffix) else { continue };
        if let Some(s) = stem.strip_suffix("ll") {
            if s.len() >= 4 && (s.ends_with('e') || s.ends_with('a')) {
                out.push(format!("{s}l{suffix}"));
            }
        } else if let Some(s) = stem.strip_suffix('l') {
            if s.len() >= 4 && (s.ends_with('e') || s.ends_with('a')) {
                out.push(format!("{s}ll{suffix}"));
            }
        }
    }
    for (a, b) in SPELLING_PAIRS {
        if w == *a {
            out.push(b.to_string());
        } else if w == *b {
            out.push(a.to_string());
        }
    }
    out
}

/// Every candidate form of `w`: its bases, their inflections, and the
/// British/American respellings of all of those.
fn forms(w: &str) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    let add = |v: String, all: &mut Vec<String>| {
        if v.len() >= 2 && !all.contains(&v) {
            all.push(v);
        }
    };
    for b in bases(w) {
        add(b.clone(), &mut all);
        for f in inflect(&b) {
            add(f, &mut all);
        }
    }
    let first = all.clone();
    for f in &first {
        for r in respellings(f) {
            add(r.clone(), &mut all);
            for b in bases(&r) {
                add(b.clone(), &mut all);
                for i in inflect(&b) {
                    add(i, &mut all);
                }
            }
        }
    }
    all.retain(|v| v != w);
    all
}

// ---------------------------------------------------------------------------
// letters the tokenizer keeps

/// For a word no mail contains: the spellings with letters unicode61 doesn't
/// fold (ø, æ, ß, ł, đ, þ, œ) and German transliterations (ue → ü, which the
/// index holds as u).
fn unfolded(w: &str) -> Vec<String> {
    const SUBS: &[(&str, &str)] = &[
        ("o", "ø"),
        ("oe", "ø"),
        ("ae", "æ"),
        ("ss", "ß"),
        ("l", "ł"),
        ("d", "đ"),
        ("th", "þ"),
        ("oe", "œ"),
        ("ue", "u"),
        ("oe", "o"),
        ("ae", "a"),
        ("aa", "a"),
    ];
    let mut out = Vec::new();
    for (from, to) in SUBS {
        if !w.contains(from) {
            continue;
        }
        out.push(w.replace(from, to));
        for (i, _) in w.match_indices(from) {
            let v = format!("{}{to}{}", &w[..i], &w[i + from.len()..]);
            if !out.contains(&v) {
                out.push(v);
            }
        }
    }
    out.retain(|v| v != w);
    out.truncate(24);
    out
}

// ---------------------------------------------------------------------------
// compounds, codes and numbers

/// An unknown word split into indexed words that appear together in the
/// mail: inv20417 → inv 20417, leaserenewal → lease renewal, 2450 → 2 450,
/// 4155550138 → 415 555 0138. Numbers try thousands groups first.
fn segment(ix: &mut Index, w: &str) -> Result<Option<Vec<String>>> {
    let n = w.chars().count();
    if n < 4 || !w.chars().all(char::is_alphanumeric) || n > 40 {
        return Ok(None);
    }
    if is_digits(w) {
        let groups = thousands(w);
        let refs: Vec<&str> = groups.iter().map(String::as_str).collect();
        if groups.len() > 1 && ix.has(&refs)? {
            return Ok(Some(groups));
        }
    }
    let chars: Vec<char> = w.chars().collect();
    let mut path: Vec<String> = Vec::new();
    Ok(dfs(ix, &chars, 0, &mut path)?.then_some(path))
}

/// Depth-first split, longest piece first, each prefix of the split checked
/// as a phrase so dead ends are cut early.
fn dfs(ix: &mut Index, chars: &[char], at: usize, path: &mut Vec<String>) -> Result<bool> {
    if at == chars.len() {
        return Ok(path.len() > 1);
    }
    let max = if at == 0 { chars.len() - 1 } else { chars.len() - at };
    for len in (1..=max).rev() {
        let piece: String = chars[at..at + len].iter().collect();
        // One-letter pieces only inside a code ("1 z"), never a word split.
        if len == 1 && !piece.chars().all(|c| c.is_ascii_digit()) && at > 0 && at + 1 < chars.len() {
            continue;
        }
        path.push(piece);
        let refs: Vec<&str> = path.iter().map(String::as_str).collect();
        if ix.has(&refs)? && dfs(ix, chars, at + len, path)? {
            return Ok(true);
        }
        path.pop();
    }
    Ok(false)
}

/// 1234567 → ["1", "234", "567"].
fn thousands(d: &str) -> Vec<String> {
    let lead = d.len() % 3;
    let mut out = Vec::new();
    if lead > 0 {
        out.push(d[..lead].to_string());
    }
    let mut i = lead;
    while i < d.len() {
        out.push(d[i..i + 3].to_string());
        i += 3;
    }
    out
}

/// A known word some mail writes as two ("handoff" when the mail says
/// "hand off", "setup" for "set up"): the most common split. Messages with
/// the word as typed are still scored on it alone (`Plan::fts_exact`), so
/// the split only brings in the mail that writes it apart.
fn split_compound(ix: &mut Index, w: &str) -> Result<Option<String>> {
    // A word the mail uses all the time is how people write it ("invoice"
    // isn't "in voice"), and checking a phrase of two common words scans
    // every message with both.
    if ix.count(&phrase(&[w]), COMMON_WORD)? >= COMMON_WORD {
        return Ok(None);
    }
    let chars: Vec<char> = w.chars().collect();
    let mut best: Option<(i64, String)> = None;
    for i in 2..chars.len().saturating_sub(1) {
        let a: String = chars[..i].iter().collect();
        let b: String = chars[i..].iter().collect();
        if b.chars().count() < 2 || !ix.has(&[&a])? || !ix.has(&[&b])? {
            continue;
        }
        let n = ix.df(&[&a, &b])?;
        if n > 0 && best.as_ref().is_none_or(|(m, _)| n > *m) {
            best = Some((n, format!("{a} {b}")));
        }
    }
    Ok(best.map(|(_, split)| split))
}

/// Another spelling of a punctuated token run whose phrase no mail
/// contains: cents the mail doesn't print ($2,450.00), thousands written
/// without a separator (1234.56 vs 1.234,56), a version's "v" (2.14.3 vs
/// v2.14.3), and codes split differently.
fn reformatted(ix: &mut Index, words: &[&str]) -> Result<Vec<String>> {
    let mut cands: Vec<Vec<String>> = Vec::new();
    let owned: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    if words.len() >= 2 && matches!(*words.last().expect("non-empty"), "00" | "0") {
        cands.push(owned[..owned.len() - 1].to_vec());
    }
    if is_digits(words[0]) {
        cands.push(std::iter::once(format!("v{}", words[0])).chain(owned[1..].iter().cloned()).collect());
        if words[0].len() > 3 {
            cands.push(thousands(words[0]).into_iter().chain(owned[1..].iter().cloned()).collect());
        }
    }
    // Each unknown piece split into known ones.
    let mut resplit: Vec<String> = Vec::new();
    let mut changed = false;
    for w in words {
        if !ix.has(&[w])? {
            if let Some(seg) = segment(ix, w)? {
                resplit.extend(seg);
                changed = true;
                continue;
            }
        }
        resplit.push(w.to_string());
    }
    if changed {
        cands.push(resplit);
    }
    let mut out = Vec::new();
    for c in cands {
        let refs: Vec<&str> = c.iter().map(String::as_str).collect();
        if !refs.is_empty() && ix.has(&refs)? {
            out.push(c.join(" "));
        }
    }
    Ok(out)
}

/// Typed apart, written together in the mail: two adjacent plain words
/// whose concatenation the index holds ("bike pick up" → pickup, "sour
/// dough" → sourdough, "lea se" → lease) become one term: the two words as
/// a phrase, or the joined word.
fn merge_split_words(ix: &mut Index, q: &mut ParsedQuery) -> Result<()> {
    let mut i = 0;
    while i + 1 < q.clauses.len() {
        let (Some((a, _)), Some((b, bp))) = (plain(&q.clauses[i]), plain(&q.clauses[i + 1])) else {
            i += 1;
            continue;
        };
        let (ta, tb) = (text::tokens(a), text::tokens(b));
        let ok = |t: &[(usize, usize, String)]| t.len() == 1 && is_letters(&t[0].2) && !is_noise_word(&t[0].2);
        if !ok(&ta) || !ok(&tb) {
            i += 1;
            continue;
        }
        let joined = format!("{}{}", ta[0].2, tb[0].2);
        if joined.chars().count() < 4 || !ix.has(&[&joined])? {
            i += 1;
            continue;
        }
        let both = format!("{a} {b}");
        let prefix = bp;
        q.clauses[i] = Clause {
            terms: vec![Term {
                negated: false,
                atom: Atom::Text {
                    text: both.clone(),
                    phrase: false,
                    prefix,
                },
            }],
        };
        q.clauses.remove(i + 1);
        q.alternatives.insert(both, vec![joined]);
        i += 1;
    }
    Ok(())
}

/// Digit groups typed apart that the mail writes another way: joined
/// (a tracking number), without a +country code, or without a trunk 0
/// (phone numbers). The run becomes one term.
fn merge_digit_runs(ix: &mut Index, q: &mut ParsedQuery) -> Result<()> {
    let mut i = 0;
    while i < q.clauses.len() {
        let mut j = i;
        let mut groups: Vec<String> = Vec::new();
        let mut plus = false;
        while j < q.clauses.len() {
            let Some((t, _)) = plain(&q.clauses[j]) else { break };
            let toks = text::tokens(t);
            if toks.len() != 1 || !is_digits(&toks[0].2) {
                break;
            }
            if j == i {
                plus = t.trim_start().starts_with('+');
            }
            groups.push(toks[0].2.clone());
            j += 1;
        }
        if groups.len() < 2 {
            i += 1;
            continue;
        }
        let refs: Vec<&str> = groups.iter().map(String::as_str).collect();
        if ix.has(&refs)? {
            i = j;
            continue;
        }
        let mut cands: Vec<Vec<String>> = vec![vec![groups.concat()]];
        let rest: Vec<String> = if plus && groups[0].len() <= 3 {
            groups[1..].to_vec()
        } else {
            groups.clone()
        };
        if rest.len() < groups.len() {
            cands.push(rest.clone());
        }
        if let Some(first) = rest.first().and_then(|g| g.strip_prefix('0')) {
            if !first.is_empty() {
                let mut v = rest.clone();
                v[0] = first.to_string();
                cands.push(v);
            }
        }
        let mut found = None;
        for c in cands {
            let refs: Vec<&str> = c.iter().map(String::as_str).collect();
            if ix.has(&refs)? {
                found = Some(c.join(" "));
                break;
            }
        }
        if let Some(text) = found {
            q.clauses.splice(
                i..j,
                [Clause {
                    terms: vec![Term {
                        negated: false,
                        atom: Atom::Text {
                            text,
                            phrase: false,
                            prefix: false,
                        },
                    }],
                }],
            );
            i += 1;
        } else {
            i = j;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// typos

/// The indexed word one edit away from `w` (a word no mail contains): one
/// that occurs with the query's other known words wins, then the most
/// frequent (Norvig's corrector, checked in the index).
fn correction(ix: &mut Index, w: &str, context: &[String]) -> Result<Option<String>> {
    let others: Vec<String> = context.iter().filter(|o| *o != w).map(|o| phrase(&[o])).collect();
    let mut best: Option<(bool, i64, String)> = None;
    for cand in ix.existing(&crate::hybrid::edits1(w))? {
        let n = ix.df(&[&cand])?;
        let together = !others.is_empty()
            && ix.count(&format!("{} {}", phrase(&[&cand]), others.join(" ")), 1)? > 0;
        let key = (together, n);
        if best
            .as_ref()
            .is_none_or(|b| key > (b.0, b.1) || (key == (b.0, b.1) && cand < b.2))
        {
            best = Some((together, n, cand));
        }
    }
    Ok(best.map(|b| b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflections_cover_plurals_verbs_and_possessives() {
        let f = forms("invoices");
        assert!(f.contains(&"invoice".to_string()), "{f:?}");
        let f = forms("ships");
        assert!(f.contains(&"shipped".to_string()) && f.contains(&"shipping".to_string()), "{f:?}");
        let f = forms("approve");
        assert!(f.contains(&"approved".to_string()), "{f:?}");
        let f = forms("sofias");
        assert!(f.contains(&"sofia".to_string()), "{f:?}");
        let f = forms("deliveries");
        assert!(f.contains(&"delivery".to_string()), "{f:?}");
        // Not plurals.
        assert!(!forms("news").contains(&"new".to_string()));
        assert!(!forms("status").contains(&"statu".to_string()));
        // Short stems are other words.
        assert!(!forms("wedding").contains(&"wed".to_string()));
        assert!(forms("shipped").contains(&"ship".to_string()));
        // Never slices inside a character.
        let _ = (forms("émails"), respellings("émails"), bases("会議室s"));
    }

    #[test]
    fn british_and_american_spellings() {
        for (a, b) in [
            ("color", "colour"),
            ("favorite", "favourite"),
            ("neighbor", "neighbour"),
            ("organize", "organise"),
            ("center", "centre"),
            ("theater", "theatre"),
            ("license", "licence"),
            ("catalog", "catalogue"),
            ("canceled", "cancelled"),
            ("traveling", "travelling"),
            ("gray", "grey"),
        ] {
            assert!(forms(a).contains(&b.to_string()), "{a} → {b}: {:?}", forms(a));
            assert!(forms(b).contains(&a.to_string()), "{b} → {a}: {:?}", forms(b));
        }
        // "cancel" reaches both spellings of its past tense.
        let f = forms("cancel");
        assert!(f.contains(&"cancelled".to_string()) && f.contains(&"canceled".to_string()), "{f:?}");
        // No -ll- respelling for short stems (filled is not filed).
        assert!(!respellings("filled").contains(&"filed".to_string()));
    }

    #[test]
    fn unfolded_letters() {
        assert!(unfolded("odegard").contains(&"ødegard".to_string()));
        assert!(unfolded("hauptstrasse").contains(&"hauptstraße".to_string()));
        assert!(unfolded("mueller").contains(&"muller".to_string()));
    }

    #[test]
    fn thousands_groups() {
        assert_eq!(thousands("2450"), ["2", "450"]);
        assert_eq!(thousands("1234567"), ["1", "234", "567"]);
        assert_eq!(thousands("123"), ["123"]);
    }
}
