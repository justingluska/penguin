//! Graded relevance judgments as rules over conversation facts.
//!
//! A query's judgments are a list of (condition, grade) rules; a
//! conversation's grade is the highest grade of any rule it satisfies, 0
//! otherwise. Grades follow the TREC Deep Learning track's four-level scale
//! (Craswell et al., TREC 2019), passage-task wording, since an email either
//! is the thing or answers the question:
//!
//! - 3 perfectly relevant: the item itself / contains the exact answer
//! - 2 highly relevant: squarely about the need, answer partial or one of
//!   several equally good items
//! - 1 related: on topic but does not satisfy the need
//! - 0 irrelevant
//!
//! Because the rules run over every conversation's facts, a judgment is
//! complete for the concept (all Lisbon flights, all electricity bills), not
//! only for the one conversation the query was written about.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::corpus::{Corpus, ThreadFacts};
use crate::window::Window;

#[derive(Debug, Clone)]
pub enum Cond {
    Tag(&'static str),
    All(Vec<Cond>),
    Any(Vec<Cond>),
    Not(Box<Cond>),
    /// First message inside the window.
    In(Window),
    /// Some message inside the window (first or last).
    Touches(Window),
    /// An attachment name contains this (lowercase).
    File(&'static str),
    /// The subject contains this (case-insensitive).
    Subject(&'static str),
}

impl Cond {
    pub fn eval(&self, f: &ThreadFacts, now: i64) -> bool {
        match self {
            Cond::Tag(t) => f.has(t),
            Cond::All(cs) => cs.iter().all(|c| c.eval(f, now)),
            Cond::Any(cs) => cs.iter().any(|c| c.eval(f, now)),
            Cond::Not(c) => !c.eval(f, now),
            Cond::In(w) => w.contains(now, f.first),
            Cond::Touches(w) => w.contains(now, f.first) || w.contains(now, f.last),
            Cond::File(s) => f
                .tags
                .iter()
                .any(|t| t.strip_prefix("file:").is_some_and(|n| n.contains(s))),
            Cond::Subject(s) => f.subject.to_lowercase().contains(&s.to_lowercase()),
        }
    }

    /// Every literal tag the condition mentions (for the typo check).
    pub fn tags(&self, out: &mut Vec<&'static str>) {
        match self {
            Cond::Tag(t) => out.push(t),
            Cond::All(cs) | Cond::Any(cs) => cs.iter().for_each(|c| c.tags(out)),
            Cond::Not(c) => c.tags(out),
            _ => {}
        }
    }
}

pub fn t(tag: &'static str) -> Cond {
    Cond::Tag(tag)
}
pub fn all<const N: usize>(cs: [Cond; N]) -> Cond {
    Cond::All(cs.into())
}
pub fn any<const N: usize>(cs: [Cond; N]) -> Cond {
    Cond::Any(cs.into())
}
pub fn not(c: Cond) -> Cond {
    Cond::Not(Box::new(c))
}
pub fn within(w: Window) -> Cond {
    Cond::In(w)
}
pub fn touches(w: Window) -> Cond {
    Cond::Touches(w)
}
pub fn file(s: &'static str) -> Cond {
    Cond::File(s)
}
pub fn subj(s: &'static str) -> Cond {
    Cond::Subject(s)
}

/// Query categories: every query has exactly one, and metrics are reported
/// per category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// Operators (`from:`, `has:pdf`, `date:`…), usually with words.
    Operator,
    /// A person's or company's name or address.
    Name,
    /// Exact codes and numbers: booking codes, order, tracking and invoice
    /// numbers, amounts, phone numbers, verification codes.
    Identifier,
    /// A natural description that shares words with the target.
    Natural,
    /// Vocabulary mismatch: the target doesn't use the query's words.
    Paraphrase,
    /// A question, as typed into Ask.
    Question,
    /// Typos and misspellings.
    Misspelling,
    /// Spanish, or across Spanish and English.
    Spanish,
    /// Time-scoped ("last spring", "last month"), in words or `date:`.
    Time,
    // The edge-case categories (docs/SEARCH-CASES.md): each exercises one
    // branch of the taxonomy of what people type into mail search.
    /// Boolean syntax: OR precedence, grouping, negation, quotes, prefixes,
    /// Gmail braces, KQL `NOT`, malformed input.
    Syntax,
    /// Labels (spaces, nesting, Gmail's dashed form), folders, `is:`,
    /// categories, accounts, Trash and Spam.
    Place,
    /// `has:` kinds, `filename:` (extensions, spaces, underscores),
    /// `larger:`/`smaller:`/`size:`.
    Attachment,
    /// Date operators in many formats, relative dates, boundaries at local
    /// midnight, date phrases in words.
    Date,
    /// Names: diacritics, CJK, apostrophes, hyphens, nicknames, display name
    /// vs address, domains and subdomains, plus-addressing.
    People,
    /// Identifiers typed in another format than the mail's: amounts,
    /// currencies, phone numbers, spaced or joined codes, URLs.
    Format,
    /// Plurals, inflections, British vs American spelling, compounds and
    /// hyphenation, possessives.
    Morphology,
    /// Very short or long queries, pasted subjects with Re:/Fwd:,
    /// punctuation, capitals, unbalanced quotes, transposed letters.
    Messy,
    /// Operators mixed with natural language, and conflicting filters that
    /// must return nothing (facet `expect-empty`).
    Mixed,
    /// Descriptive recall ("that email where…"), questions and cross-language
    /// queries over the edge-case mail.
    Recall,
}

impl Category {
    pub const ALL: [Category; 19] = [
        Category::Operator,
        Category::Name,
        Category::Identifier,
        Category::Natural,
        Category::Paraphrase,
        Category::Question,
        Category::Misspelling,
        Category::Spanish,
        Category::Time,
        Category::Syntax,
        Category::Place,
        Category::Attachment,
        Category::Date,
        Category::People,
        Category::Format,
        Category::Morphology,
        Category::Messy,
        Category::Mixed,
        Category::Recall,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Category::Operator => "operator",
            Category::Name => "name",
            Category::Identifier => "identifier",
            Category::Natural => "natural",
            Category::Paraphrase => "paraphrase",
            Category::Question => "question",
            Category::Misspelling => "misspelling",
            Category::Spanish => "spanish",
            Category::Time => "time",
            Category::Syntax => "syntax",
            Category::Place => "place",
            Category::Attachment => "attachment",
            Category::Date => "date",
            Category::People => "people",
            Category::Format => "format",
            Category::Morphology => "morphology",
            Category::Messy => "messy",
            Category::Mixed => "mixed",
            Category::Recall => "recall",
        }
    }
}

/// Facet of a query whose right answer is nothing at all (conflicting or
/// impossible filters): it scores 1 on every metric when the ranking is
/// empty and 0 otherwise, and has no judgment rules.
pub const EXPECT_EMPTY: &str = "expect-empty";

/// One judged query.
pub struct QuerySpec {
    pub id: &'static str,
    /// The query as typed. Owned: date-format queries are written for the
    /// day the corpus is generated (`after:2026/08/14`).
    pub text: String,
    pub category: Category,
    /// Extra facets: `known-item`, `cross-lingual`, `broad`, `operator`,
    /// `question`, `recency`.
    pub facets: &'static [&'static str],
    pub rules: Vec<(Cond, u8)>,
}

/// Problems with a query set: duplicate ids, queries with nothing relevant
/// (grade ≥ 2) to find, and rule tags that are on no conversation (typos).
pub fn validate(corpus: &Corpus, queries: &[QuerySpec]) -> Vec<String> {
    let qrels = qrels(corpus, queries);
    let known: std::collections::HashSet<&str> = corpus
        .threads
        .iter()
        .flat_map(|t| t.tags.iter().map(String::as_str))
        .collect();
    let mut ids = std::collections::HashSet::new();
    let mut problems = Vec::new();
    for q in queries {
        if !ids.insert(q.id) {
            problems.push(format!("duplicate id {}", q.id));
        }
        if q.facets.contains(&EXPECT_EMPTY) {
            if !q.rules.is_empty() {
                problems.push(format!("{}: expect-empty with judgment rules", q.id));
            }
        } else if !qrels[q.id].values().any(|g| *g >= 2) {
            problems.push(format!(
                "{}: nothing relevant (grade ≥ 2) in the corpus",
                q.id
            ));
        }
        let mut tags = Vec::new();
        for (c, _) in &q.rules {
            c.tags(&mut tags);
        }
        for t in tags {
            if !known.contains(t) {
                problems.push(format!("{}: tag {t:?} is on no conversation", q.id));
            }
        }
    }
    problems
}

/// Grade every conversation for every query: `qrels[query_id][doc_id]`,
/// graded 1–3 (0s are omitted, as in TREC qrels).
pub fn qrels(corpus: &Corpus, queries: &[QuerySpec]) -> BTreeMap<String, BTreeMap<String, u8>> {
    let mut out = BTreeMap::new();
    for q in queries {
        let mut m = BTreeMap::new();
        for f in &corpus.threads {
            let g = q
                .rules
                .iter()
                .filter(|(c, _)| c.eval(f, corpus.now))
                .map(|(_, g)| *g)
                .max()
                .unwrap_or(0);
            if g > 0 {
                m.insert(f.doc_id(), g);
            }
        }
        out.insert(q.id.to_string(), m);
    }
    out
}
