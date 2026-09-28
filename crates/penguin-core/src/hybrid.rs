//! Hybrid search: the keyword index (FTS5 bm25) and search by meaning
//! (embeddings, `penguin-semantic`) fused into one ranking.
//!
//! Design and sources: docs/SEARCH-RANKING.md. In short:
//!
//! 0. **Rewrite** plain words (`rewrite`): a date typed as words becomes a
//!    soft ranking window; a word no message contains is respelled from
//!    the index.
//! 1. **Route.** Each query gets a semantic weight α from its shape (`route`):
//!    pure filters, quoted phrases and boolean text never use meaning (they
//!    run exactly as before); codes, numbers, single words and names are
//!    keyword-led (α 0.25); two plain words are balanced (α 0.5); descriptive
//!    text and questions are meaning-led (α 0.7).
//! 2. **Retrieve.** The keyword side is the existing bm25 candidate walk. The
//!    meaning side embeds the free text (operators removed) and asks the
//!    vector index for the nearest chunks, with every filter of the query
//!    applied (accounts and dates inside the index search; the rest by a
//!    pre-filtered id set when it is small, otherwise by checking each
//!    candidate against the same SQL the keyword side uses).
//! 3. **Fuse.** Convex combination per message:
//!    `(1-α)·bm25/bm25_max + α·norm(cosine)` (Bruch et al., TOIS 2023),
//!    plus the query-independent priors search already uses (recency,
//!    starred, unread, correspondents, a typed sender name), then a
//!    rerank of the top conversations with the signals that need the thread
//!    (you took part, exact subject or sender name, codes, bulk mail).
//! 4. **Passages.** A conversation found by meaning carries the best
//!    passage of its best chunk (`passage`).
//!
//! With no index, an index still being built, or a route without meaning,
//! nothing here runs and search is exactly the keyword search.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use penguin_semantic::{Embedder, SearchFilter, VectorIndex};
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};

use crate::query::{self, ParsedQuery};
use crate::search::{
    group_scored, preds_sql, prior, rowid_range_sql, sort_groups, Group, Highlighter, KwCands,
    Plan, Stages,
};
use crate::store::{self, F_IMPORTANT, F_NEWSLETTER, F_SENT};
use crate::text;
use crate::types::SemanticStatus;
use crate::Result;

/// The model and index search by meaning uses, as the app holds them.
#[derive(Clone)]
pub struct SemanticHandles {
    pub embedder: Arc<dyn Embedder>,
    pub index: Arc<dyn VectorIndex>,
    /// Share of mail embedded so far (0–1); None = unknown, taken as done.
    pub progress: Option<f32>,
}

impl SemanticHandles {
    pub fn status(&self) -> (SemanticStatus, Option<f32>) {
        match self.progress {
            Some(p) if p < 1.0 => (SemanticStatus::Indexing, Some(p.clamp(0.0, 1.0))),
            _ => (SemanticStatus::Ready, None),
        }
    }
}

/// How the meaning side's cosine similarities are put on the keyword side's
/// [0, 1] scale before the convex combination.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SemNorm {
    /// (s − s_k) / (s_1 − s_k) over the retrieved list: the best chunk is 1,
    /// the k-th is 0. Model-agnostic (no assumption about a model's cosine
    /// range), at the price of always calling the best chunk a 1.
    MinMax,
    /// (s + 1) / (s_1 + 1): theoretical min-max with cosine's minimum −1
    /// (Bruch et al.'s TM2C2).
    TheoreticalMinMax,
}

/// How the two sides are combined.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fusion {
    /// (1−α)·keyword + α·meaning, both normalized to [0, 1].
    Convex(SemNorm),
    /// Weighted reciprocal rank fusion, (1−α)/(k+rank_kw) + α/(k+rank_sem),
    /// scaled so rank 1 on both sides is 1 (Cormack et al., SIGIR 2009).
    Rrf { k: f64 },
}

/// Every tunable of hybrid ranking. `Default` is what the app ships; the
/// values and how they were chosen are in docs/SEARCH-RANKING.md.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridParams {
    pub fusion: Fusion,
    /// α for keyword-led, balanced and meaning-led queries.
    pub alpha_keyword: f64,
    pub alpha_balanced: f64,
    pub alpha_semantic: f64,
    /// Chunks asked of the vector index (×`post_filter_factor` when filters
    /// are checked after the search).
    pub k: usize,
    pub post_filter_factor: usize,
    /// Meaning-only candidates below this normalized similarity are dropped.
    pub sem_floor: f64,
    /// Normalized similarity from which a conversation counts as "matched by
    /// meaning" (the tag, and the passage).
    pub meaning_floor: f64,
    /// Keyword-led queries skip the meaning side when the keyword side found
    /// at least this many conversations (0 = never skip).
    pub keyword_enough: usize,
    /// Rerank signals (added to the fused score of the top conversations).
    pub participated: f64,
    pub important: f64,
    pub newsletter_penalty: f64,
    pub code_boost: f64,
    /// Keyword matches of a query whose words (all of them, in some form)
    /// occur together in at most `rare_words_max` conversations: a rare
    /// co-occurrence is the known item, so meaning neighbours don't bury it.
    pub rare_words_boost: f64,
    pub rare_words_max: usize,
    pub subject_boost: f64,
    pub sender_boost: f64,
    /// Conversations reranked with the thread-level signals.
    pub rerank_depth: usize,
    /// Boost for mail inside a date typed as words ("last spring").
    pub date_boost: f64,
    /// A query meaning-led only by its length (no question or cue) whose
    /// words all occur in some mail gets the balanced α instead.
    pub keyword_found_balances: bool,
}

impl Default for HybridParams {
    fn default() -> Self {
        HybridParams {
            fusion: Fusion::Convex(SemNorm::MinMax),
            alpha_keyword: 0.25,
            alpha_balanced: 0.5,
            alpha_semantic: 0.7,
            k: 100,
            post_filter_factor: 4,
            sem_floor: 0.2,
            meaning_floor: 0.5,
            keyword_enough: 20,
            participated: 0.05,
            important: 0.03,
            newsletter_penalty: 0.15,
            code_boost: 0.15,
            rare_words_boost: 0.15,
            rare_words_max: 3,
            subject_boost: 0.10,
            sender_boost: 0.30,
            rerank_depth: 200,
            date_boost: 0.30,
            keyword_found_balances: true,
        }
    }
}

// ---------------------------------------------------------------------------
// routing

/// How much a query leans on meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    /// Filters only, quoted phrases, boolean text: keyword search only.
    Off,
    /// Codes, numbers, single words, names.
    Keyword,
    /// Two or so plain words.
    Balanced,
    /// Descriptive text, questions.
    Semantic,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub kind: RouteKind,
    /// Weight of meaning in the fusion, 0–1.
    pub alpha: f64,
    /// The text embedded for the meaning side.
    pub text: Option<String>,
    /// The query holds a code, number or address: exact matches win.
    pub has_code: bool,
    /// A question or descriptive cue (not just word count) made it meaning-led.
    pub question: bool,
    /// Content words of the query (noise words dropped), folded.
    pub words: Vec<String>,
}

impl Route {
    /// No meaning, but fused ranking still runs for a soft date (a name
    /// or a code plus "last month").
    pub(crate) fn dates_only(words: Vec<String>) -> Route {
        Route {
            kind: RouteKind::Keyword,
            alpha: 0.0,
            text: None,
            has_code: false,
            question: false,
            words,
        }
    }

    fn off() -> Route {
        Route {
            kind: RouteKind::Off,
            alpha: 0.0,
            text: None,
            has_code: false,
            question: false,
            words: Vec::new(),
        }
    }
}

/// Openers and phrases of descriptive, remembered-content queries ("that
/// thing about…", "when is…"). Matched on the raw query, lowercased.
const QUESTION_OPENERS: &[&str] = &[
    "what", "when", "where", "who", "whom", "which", "why", "how", "did", "does", "do", "is",
    "was", "were", "are", "can", "could", "should", "has", "have", "any", "find", "show",
];
const DESCRIPTIVE_CUES: &[&str] = &[
    "thing about",
    "something about",
    "stuff about",
    "anything about",
    "the one about",
    "the email about",
    "the mail about",
    "the message about",
    "email about",
    "mail about",
    "that email",
    "that mail",
    "that message",
    "the one where",
    "the email where",
    "remind me",
    "looking for",
    "info on",
    "details on",
    "details about",
];

/// Pick the route for a parsed query. `raw` is the text as typed.
pub(crate) fn route(
    c: &Connection,
    q: &ParsedQuery,
    plan: &Plan,
    raw: &str,
    p: &HybridParams,
) -> Result<Route> {
    // Exact syntax means exact results: a quoted phrase or text inside OR /
    // groups is answered by the keyword index alone, as before.
    if plan.boolean_text || q.text_terms().any(|(_, phrase, _)| phrase) {
        return Ok(Route::off());
    }
    let Some(text) = semantic_text(q, raw) else {
        return Ok(Route::off());
    };
    let words: Vec<String> = text::tokens(&text)
        .into_iter()
        .map(|t| t.2)
        .filter(|w| !query::is_noise_word(w))
        .collect();
    if words.is_empty() {
        return Ok(Route::off());
    }
    let has_code = text.split_whitespace().any(is_code);
    let lower = raw.trim().to_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    let question = lower.ends_with('?')
        || QUESTION_OPENERS.contains(&first)
        || DESCRIPTIVE_CUES.iter().any(|cue| lower.contains(cue));
    let kind = if has_code {
        RouteKind::Keyword
    } else if question {
        RouteKind::Semantic
    } else if words.len() <= 3 && all_names(c, &words)? {
        // A name is an identity, not a meaning: embeddings of names are
        // noise (dense retrievers fail on entities, Sciavolino et al. 2021),
        // and the keyword side already boosts that person's mail.
        return Ok(Route {
            words,
            ..Route::off()
        });
    } else if words.len() == 1 {
        RouteKind::Keyword
    } else if words.len() >= 3 {
        RouteKind::Semantic
    } else {
        RouteKind::Balanced
    };
    let alpha = match kind {
        RouteKind::Off => 0.0,
        RouteKind::Keyword => p.alpha_keyword,
        RouteKind::Balanced => p.alpha_balanced,
        RouteKind::Semantic => p.alpha_semantic,
    };
    Ok(Route {
        kind,
        alpha,
        text: Some(text),
        has_code,
        question,
        words,
    })
}

/// Order numbers, invoice ids, amounts, addresses: text a model can't
/// match by meaning but the keyword index matches exactly.
fn is_code(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric());
    w.chars().any(|c| c.is_ascii_digit()) || word.contains('@') || word.contains('#')
}

/// Every word names someone you correspond with ("mike", "ana ruiz").
fn all_names(c: &Connection, words: &[String]) -> Result<bool> {
    let mut stmt = c.prepare_cached(&format!(
        "SELECT count(*) FROM (SELECT 1 FROM people WHERE from_count + sent_to_count > 0 \
         AND {} LIMIT 1)",
        store::has_word(1)
    ))?;
    for w in words {
        let n: i64 = stmt.query_row(params_from_iter(store::word_range(w, false)), |r| r.get(0))?;
        if n == 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The text embedded for the meaning side: the query as typed minus every
/// operator, exclusion, date and grouping (whatever the parser turned into a
/// chip), keeping natural words in order ("that thing about the lease").
/// A last word still being typed (under 3 characters) is left out while
/// other words carry the meaning. None when no free text remains.
pub fn semantic_text(q: &ParsedQuery, raw: &str) -> Option<String> {
    q.text_terms().next()?;
    let mut rest = raw.to_string();
    for chip in &q.chips {
        if chip.kind != "text" && !chip.raw.is_empty() {
            if let Some(i) = rest.find(&chip.raw) {
                rest.replace_range(i..i + chip.raw.len(), " ");
            }
        }
    }
    let mut words: Vec<&str> = rest
        .split_whitespace()
        .map(|w| w.trim_matches(|c| matches!(c, '(' | ')' | '{' | '}')))
        .filter(|w| !w.is_empty() && *w != "OR" && *w != "AND" && !w.starts_with('-'))
        .collect();
    let typing = !raw.ends_with(char::is_whitespace);
    if typing && words.len() > 1 {
        if let Some(last) = words.last() {
            if text::tokens(last).iter().map(|t| t.2.chars().count()).sum::<usize>()
                < crate::search::MIN_PREFIX_CHARS
            {
                words.pop();
            }
        }
    }
    let text = words.join(" ");
    text::tokens(&text).first()?;
    Some(text)
}

// ---------------------------------------------------------------------------
// query rewrites (hybrid only)

/// A date window written as plain words ("dentist last spring"): it ranks
/// mail in the window first instead of filtering. `date:` stays a filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoftDate {
    pub after: Option<i64>,
    pub before: Option<i64>,
}

/// The query search actually runs when meaning is on.
#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    /// What the keyword side and the filters run.
    pub raw: String,
    /// What the model embeds: the typed words minus the date words, with
    /// only the corrections the query's other words confirm. A word missing
    /// from the mailbox is often a real word the mail doesn't use ("plane"
    /// where the mail says "flight"), exactly what meaning is for, and
    /// correcting it to a nearby indexed word ("place") would change the
    /// meaning; a correction that co-occurs with the other words is safe.
    pub embed_raw: String,
    pub soft_date: Option<SoftDate>,
    /// (typed, replaced by) for each misspelled word.
    pub respelled: Vec<(String, String)>,
}

/// Two rewrites of plain words, never of operators, quotes or boolean text:
///
/// - **Dates in words.** Free text is never a date filter (SEARCH.md), so
///   "dentist last spring" used to require the words "last" and "spring".
///   Here the date words (`query::date_hint`, the same grammar as `date:`)
///   leave the text and become a soft window (`SoftDate`): mail inside it
///   gets a boost, mail near it part of one. Shortwave does the same with a
///   Gaussian around the extracted range.
/// - **Misspellings.** A word no message contains makes the all-words
///   keyword match empty. On the keyword side such a word (4+ letters) is
///   replaced by the indexed word one edit away (Damerau–Levenshtein
///   distance 1) that the most messages contain. The model still embeds
///   the word as typed (`embed_raw`).
pub fn rewrite(c: &Connection, raw: &str, now: i64) -> Result<Rewrite> {
    let mut out = Rewrite {
        raw: raw.to_string(),
        embed_raw: raw.to_string(),
        soft_date: None,
        respelled: Vec::new(),
    };
    let q = query::parse(raw, now);
    let exact = q.clauses.iter().any(|cl| {
        cl.terms.len() > 1 && cl.terms.iter().any(|t| matches!(t.atom, query::Atom::Text { .. }))
    }) || q.text_terms().any(|(_, phrase, _)| phrase);
    if exact || q.text_terms().next().is_none() {
        return Ok(out);
    }
    if let Some(h) = query::date_hint(raw, now) {
        let rest = raw.replacen(&h.raw, " ", 1);
        let has_words = query::parse(&rest, now)
            .text_terms()
            .any(|(t, _, _)| text::tokens(t).iter().any(|w| !query::is_noise_word(&w.2)));
        let window = query::parse(&h.rewrite, now);
        if has_words && (window.after.is_some() || window.before.is_some()) {
            out.raw = rest.split_whitespace().collect::<Vec<_>>().join(" ");
            if raw.ends_with(char::is_whitespace) {
                out.raw.push(' ');
            }
            out.soft_date = Some(SoftDate {
                after: window.after,
                before: window.before,
            });
        }
    }
    out.embed_raw = out.raw.clone();
    respell(c, &mut out, now)?;
    Ok(out)
}

fn respell(c: &Connection, rw: &mut Rewrite, now: i64) -> Result<()> {
    let q = query::parse(&rw.raw, now);
    let typing = !rw.raw.ends_with(char::is_whitespace);
    let words: Vec<(String, bool)> = q
        .text_terms()
        .filter(|(_, phrase, _)| !phrase)
        .flat_map(|(t, _, prefix)| {
            let toks = text::tokens(t);
            let n = toks.len();
            toks.into_iter()
                .enumerate()
                .map(move |(i, tok)| (tok.2, prefix && typing && i + 1 == n))
        })
        .filter(|(w, _)| {
            w.chars().count() >= 4
                && w.chars().all(char::is_alphabetic)
                && !query::is_noise_word(w)
        })
        .collect();
    if words.is_empty() {
        return Ok(());
    }
    // Every plain word in order, for neighbours.
    let seq: Vec<String> = q
        .text_terms()
        .filter(|(_, phrase, _)| !phrase)
        .flat_map(|(t, _, _)| text::tokens(t).into_iter().map(|t| t.2))
        .collect();
    // Term lookups in the full-text index: a seek per term, no scan.
    let mut exists = c.prepare_cached(
        "SELECT count(*) FROM (SELECT 1 FROM messages_fts WHERE messages_fts MATCH ?1 LIMIT 1)",
    )?;
    let mut docs = c.prepare_cached(
        "SELECT count(*) FROM (SELECT 1 FROM messages_fts WHERE messages_fts MATCH ?1 LIMIT 1000)",
    )?;
    // The query's words that the index has, as context for corrections.
    let mut all_words: Vec<String> = Vec::new();
    for (t, phrase, _) in q.text_terms() {
        if phrase {
            continue;
        }
        for tok in text::tokens(t) {
            let w = tok.2;
            if w.chars().count() >= 2
                && !query::is_noise_word(&w)
                && !all_words.contains(&w)
                && exists.query_row([fts_term(&w, false)], |r| r.get::<_, i64>(0))? > 0
            {
                all_words.push(w);
            }
        }
    }
    for (w, prefix) in words {
        // A word being typed matches as a prefix; a finished one exactly.
        let n: i64 = exists.query_row([fts_term(&w, prefix)], |r| r.get(0))?;
        if n > 0 {
            continue;
        }
        // Another form the mail uses ("sour dough" is sourdough, not "your
        // rough"): the keyword side's lexicon finds it (lexicon.rs).
        let at = seq.iter().position(|x| *x == w);
        let prev = at.and_then(|i| i.checked_sub(1)).map(|i| seq[i].as_str());
        let next = at.and_then(|i| seq.get(i + 1)).map(String::as_str);
        if crate::lexicon::other_form(c, &w, prev, next)? {
            continue;
        }
        // Candidates: every word one edit away (Norvig's corrector, a few
        // hundred), kept when the index has it. Most misspellings are one
        // insertion, deletion, substitution or swap (Damerau, CACM 1964:
        // ~80%). A survivor that occurs together with the query's other
        // words wins ("recipt deskcraft": "receipt" is in the Deskcraft
        // mail, "recipe" isn't), then the more frequent (a unigram prior).
        let others: Vec<String> = all_words
            .iter()
            .filter(|o| **o != w)
            .map(|o| fts_term(o, false))
            .collect();
        let mut best: Option<(bool, i64, String)> = None;
        for cand in edits1(&w) {
            if exists.query_row([fts_term(&cand, false)], |r| r.get::<_, i64>(0))? == 0 {
                continue;
            }
            let n: i64 = docs.query_row([fts_term(&cand, false)], |r| r.get(0))?;
            let together = !others.is_empty()
                && exists.query_row([format!("{} {}", fts_term(&cand, false), others.join(" "))], |r| {
                    r.get::<_, i64>(0)
                })? > 0;
            let key = (together, n);
            if best
                .as_ref()
                .is_none_or(|b| key > (b.0, b.1) || (key == (b.0, b.1) && cand < b.2))
            {
                best = Some((together, n, cand));
            }
        }
        if let Some((together, _, term)) = best {
            // Confirmed by the other words: the model gets the correction
            // too ("recipt" embeds near "recipe"). Unconfirmed, it gets the
            // word as typed: it may be a real word the mail doesn't use.
            if together {
                if let Some(pos) = find_word(&rw.embed_raw, &w) {
                    rw.embed_raw.replace_range(pos..pos + w.len(), &term);
                }
            }
            if let Some(pos) = find_word(&rw.raw, &w) {
                rw.raw.replace_range(pos..pos + w.len(), &term);
                rw.respelled.push((w, term));
            }
        }
    }
    Ok(())
}

/// A single folded word as an FTS5 query: quoted, `*` for a prefix.
fn fts_term(w: &str, prefix: bool) -> String {
    format!("\"{}\"{}", w.replace('"', ""), if prefix { "*" } else { "" })
}

/// Every string one deletion, adjacent swap, substitution or insertion away
/// from `w` (letters a–z plus the word's own letters), without `w` itself.
pub(crate) fn edits1(w: &str) -> Vec<String> {
    let chars: Vec<char> = w.chars().collect();
    let mut alphabet: Vec<char> = ('a'..='z').collect();
    for &c in &chars {
        if !alphabet.contains(&c) {
            alphabet.push(c);
        }
    }
    let n = chars.len();
    let mut out: HashSet<String> = HashSet::with_capacity(n * (2 * alphabet.len() + 2) + alphabet.len());
    let s = |v: &[char]| v.iter().collect::<String>();
    for i in 0..n {
        let mut d = chars.clone();
        d.remove(i);
        out.insert(s(&d));
        if i + 1 < n {
            let mut t = chars.clone();
            t.swap(i, i + 1);
            out.insert(s(&t));
        }
        for &a in &alphabet {
            let mut r = chars.clone();
            r[i] = a;
            out.insert(s(&r));
        }
    }
    for i in 0..=n {
        for &a in &alphabet {
            let mut ins = chars.clone();
            ins.insert(i, a);
            out.insert(s(&ins));
        }
    }
    out.remove(w);
    let mut v: Vec<String> = out.into_iter().filter(|c| c.chars().count() >= 3).collect();
    v.sort();
    v
}

/// Byte offset of `word` as a whole word in `s`, case-insensitively (ASCII
/// folding; the respelled words are the tokenizer's folded form).
fn find_word(s: &str, word: &str) -> Option<usize> {
    let lower = s.to_lowercase();
    if lower.len() != s.len() {
        return None;
    }
    let mut from = 0;
    while let Some(i) = lower[from..].find(word).map(|i| i + from) {
        let before = lower[..i].chars().next_back().is_none_or(|ch| !ch.is_alphanumeric());
        let after = lower[i + word.len()..].chars().next().is_none_or(|ch| !ch.is_alphanumeric());
        if before && after {
            return Some(i);
        }
        from = i + word.len();
    }
    None
}

/// How well a message date fits a soft window: `boost` inside it, falling
/// off as a Gaussian outside (σ = half the window, at least a week).
fn date_fit(date: i64, w: SoftDate, boost: f64) -> f64 {
    const WEEK: f64 = 7.0 * 86_400_000.0;
    let dist = match (w.after, w.before) {
        (Some(a), _) if date < a => (a - date) as f64,
        (_, Some(b)) if date >= b => (date - b) as f64,
        _ => return boost,
    };
    let sigma = match (w.after, w.before) {
        (Some(a), Some(b)) => ((b - a) as f64 / 2.0).max(WEEK),
        _ => 4.0 * WEEK,
    };
    boost * (-(dist / sigma).powi(2) / 2.0).exp()
}

// ---------------------------------------------------------------------------
// fusion

/// Everything `fuse` reads from the search in progress.
pub(crate) struct Inputs<'a> {
    pub plan: &'a Plan,
    pub q: &'a ParsedQuery,
    pub route: &'a Route,
    pub kw: &'a KwCands,
    pub accounts: Option<&'a [String]>,
    pub now: i64,
    pub sent_to: &'a HashMap<String, u32>,
    pub boost: &'a HashSet<String>,
    pub params: &'a HybridParams,
    /// A date typed as words, as a ranking window (`rewrite`).
    pub soft_date: Option<SoftDate>,
}

/// How each conversation on the page matched.
#[derive(Default)]
pub(crate) struct Matches {
    /// Keyword-only search with free text: every hit matched by words.
    pub all_words: bool,
    threads: HashMap<i64, ThreadMatch>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ThreadMatch {
    pub words: bool,
    pub meaning: bool,
    /// The message and chunk of its best passage by meaning.
    pub passage_from: Option<(i64, u32)>,
}

impl ThreadMatch {
    pub fn labels(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.words {
            out.push("words".to_string());
        }
        if self.meaning {
            out.push("meaning".to_string());
        }
        out
    }
}

impl Matches {
    pub fn thread(&self, thread_rowid: i64) -> ThreadMatch {
        if self.all_words {
            return ThreadMatch {
                words: true,
                ..ThreadMatch::default()
            };
        }
        self.threads.get(&thread_rowid).copied().unwrap_or_default()
    }
}

/// A message the meaning side found and the query's filters accept.
struct SemMsg {
    thread: i64,
    flags: i64,
    from_email: String,
    /// Best chunk similarity of the message, and that chunk.
    sim: f32,
    chunk: u32,
}

/// Fuse the keyword candidates with search by meaning. None when the route
/// or the query leaves meaning out (the caller then ranks by keywords).
pub(crate) fn fuse(
    c: &Connection,
    inp: &Inputs,
    sem: &SemanticHandles,
    stages: &mut Stages,
) -> Result<Option<(Vec<Group>, Matches)>> {
    let route = inp.route;
    let p = inp.params;
    if route.kind == RouteKind::Off {
        return Ok(None);
    }
    // Keyword-led and already plenty: skip the model (latency), the keyword
    // results are what the user asked for.
    if route.kind == RouteKind::Keyword && p.keyword_enough > 0 && inp.soft_date.is_none() {
        let threads: HashSet<i64> = inp.kw.cands.values().map(|c| c.thread).collect();
        if threads.len() >= p.keyword_enough {
            return Ok(None);
        }
    }
    // A soft date alone (a name plus "last month") ranks without meaning.
    let text = route.text.as_deref().filter(|_| route.alpha > 0.0);
    let sem_msgs = match text {
        Some(text) => {
            let qvec = match embed_query(sem.embedder.as_ref(), text) {
                Ok(v) => v,
                Err(e) => {
                    // Never the query text: only that it failed.
                    tracing::warn!(error = %e, "query embedding failed; keyword results only");
                    return Ok(None);
                }
            };
            stages.mark("embed");
            let m = match semantic_side(c, inp, sem.index.as_ref(), &qvec) {
                Ok(m) => m,
                Err(SideError::Index(e)) => {
                    tracing::warn!(error = %e, "vector search failed; keyword results only");
                    return Ok(None);
                }
                Err(SideError::Db(e)) => return Err(e),
            };
            stages.mark("vector");
            m
        }
        None if inp.soft_date.is_some() => HashMap::new(),
        None => return Ok(None),
    };
    let date_fit_of = |rowid: i64| {
        inp.soft_date
            .map_or(0.0, |w| date_fit(rowid / store::ROWID_SLOTS, w, p.date_boost))
    };

    // Normalize both sides to [0, 1].
    // Meaning-led by word count alone, but every word is in some mail: the
    // words are what the user remembers, so balance the two sides.
    let alpha = if p.keyword_found_balances
        && route.kind == RouteKind::Semantic
        && !route.question
        && !inp.kw.cands.is_empty()
    {
        p.alpha_balanced
    } else {
        route.alpha
    };
    let sem_scores = normalize_sem(&sem_msgs, p.fusion);
    let kw_rank = ranks(inp.kw.cands.iter().map(|(&r, c)| (r, c.rel)));
    let sem_rank = ranks(sem_msgs.iter().map(|(&r, m)| (r, m.sim as f64)));

    // All the words together in a handful of conversations: the known item.
    let kw_threads: HashSet<i64> = inp.kw.cands.values().map(|c| c.thread).collect();
    let rare = (1..=p.rare_words_max).contains(&kw_threads.len()) && !route.has_code;
    let mut scored: Vec<(i64, i64, f64)> = Vec::with_capacity(inp.kw.cands.len() + sem_msgs.len());
    let mut threads: HashMap<i64, ThreadMatch> = HashMap::new();
    let mut flags_of: HashMap<i64, i64> = HashMap::new();
    for (&rowid, cand) in &inp.kw.cands {
        let kw = cand.rel * inp.kw.norm;
        let s = sem_scores.get(&rowid).copied();
        let rel = match p.fusion {
            Fusion::Convex(_) => (1.0 - alpha) * kw + alpha * s.unwrap_or(0.0),
            Fusion::Rrf { k } => rrf(k, alpha, kw_rank.get(&rowid), sem_rank.get(&rowid)),
        };
        let mut score = rel + cand.extra + date_fit_of(rowid);
        if route.has_code {
            score += p.code_boost;
        }
        if rare {
            score += p.rare_words_boost;
        }
        if cand.flags & F_NEWSLETTER != 0 && s.is_some_and(|s| s >= p.meaning_floor) {
            score -= p.newsletter_penalty;
        }
        scored.push((rowid, cand.thread, score));
        flags_of.insert(rowid, cand.flags);
        let t = threads.entry(cand.thread).or_default();
        t.words = true;
        note_meaning(t, rowid, s, sem_msgs.get(&rowid), p, &sem_scores);
    }
    for (&rowid, m) in &sem_msgs {
        if inp.kw.cands.contains_key(&rowid) {
            continue;
        }
        let s = sem_scores.get(&rowid).copied().unwrap_or(0.0);
        if s < p.sem_floor {
            continue;
        }
        let rel = match p.fusion {
            Fusion::Convex(_) => alpha * s,
            Fusion::Rrf { k } => rrf(k, alpha, None, sem_rank.get(&rowid)),
        };
        let mut score = rel
            + prior(inp.now, rowid, m.flags, &m.from_email, inp.sent_to, inp.boost)
            + date_fit_of(rowid);
        if m.flags & F_NEWSLETTER != 0 {
            score -= p.newsletter_penalty;
        }
        scored.push((rowid, m.thread, score));
        flags_of.insert(rowid, m.flags);
        let t = threads.entry(m.thread).or_default();
        note_meaning(t, rowid, Some(s), Some(m), p, &sem_scores);
    }
    let mut groups = group_scored(scored.into_iter());
    rerank(c, inp, &mut groups, &flags_of)?;
    stages.mark("fuse");
    Ok(Some((groups, Matches { all_words: false, threads })))
}

/// Record a meaning match for the thread, keeping its best passage.
fn note_meaning(
    t: &mut ThreadMatch,
    rowid: i64,
    s: Option<f64>,
    m: Option<&SemMsg>,
    p: &HybridParams,
    sem_scores: &HashMap<i64, f64>,
) {
    let (Some(s), Some(m)) = (s, m) else {
        return;
    };
    if s < p.meaning_floor {
        return;
    }
    t.meaning = true;
    let better = match t.passage_from {
        Some((r, _)) => s > sem_scores.get(&r).copied().unwrap_or(0.0),
        None => true,
    };
    if better {
        t.passage_from = Some((rowid, m.chunk));
    }
}

fn rrf(k: f64, alpha: f64, kw: Option<&usize>, sem: Option<&usize>) -> f64 {
    let part = |w: f64, r: Option<&usize>| r.map_or(0.0, |&r| w * (k + 1.0) / (k + r as f64));
    part(1.0 - alpha, kw) + part(alpha, sem)
}

/// 1-based rank of each key by descending value.
fn ranks(items: impl Iterator<Item = (i64, f64)>) -> HashMap<i64, usize> {
    let mut v: Vec<(i64, f64)> = items.collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    v.into_iter().enumerate().map(|(i, (r, _))| (r, i + 1)).collect()
}

/// Meaning scores on [0, 1] (see `SemNorm`). The list's lowest similarity
/// stands in for "unrelated mail" for this query.
fn normalize_sem(msgs: &HashMap<i64, SemMsg>, fusion: Fusion) -> HashMap<i64, f64> {
    let top = msgs.values().map(|m| m.sim as f64).fold(f64::MIN, f64::max);
    let low = msgs.values().map(|m| m.sim as f64).fold(f64::MAX, f64::min);
    msgs.iter()
        .map(|(&r, m)| {
            let s = m.sim as f64;
            let v = match fusion {
                Fusion::Convex(SemNorm::TheoreticalMinMax) => (s + 1.0) / (top + 1.0),
                // RRF uses ranks for fusion; the min-max value only gates
                // the floors.
                Fusion::Convex(SemNorm::MinMax) | Fusion::Rrf { .. } => {
                    if top - low > 1e-6 {
                        (s - low) / (top - low)
                    } else {
                        1.0
                    }
                }
            };
            (r, v.clamp(0.0, 1.0))
        })
        .collect()
}

/// Thread-level signals for the top conversations: you took part in the
/// thread, Gmail marked it important, the subject holds the query's words,
/// the sender's name is the query. Bulk mail found only by meaning was
/// already penalized per message.
fn rerank(
    c: &Connection,
    inp: &Inputs,
    groups: &mut [Group],
    flags_of: &HashMap<i64, i64>,
) -> Result<()> {
    let p = inp.params;
    let depth = p.rerank_depth.min(groups.len());
    if depth == 0 {
        return Ok(());
    }
    let words = &inp.route.words;
    let mut stmt = c.prepare_cached(
        "SELECT t.flags, m.subject, m.from_name FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
         WHERE m.rowid = ?1",
    )?;
    for g in groups[..depth].iter_mut() {
        let (tflags, subject, from_name): (i64, String, Option<String>) = stmt
            .query_row([g.best_rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        if tflags & F_SENT != 0 {
            g.best_score += p.participated;
        }
        if flags_of.get(&g.best_rowid).is_some_and(|f| f & F_IMPORTANT != 0) {
            g.best_score += p.important;
        }
        if words.len() >= 2 && contains_run(&subject, words) {
            g.best_score += p.subject_boost;
        }
        if words.len() >= 2 {
            if let Some(name) = from_name.as_deref() {
                let name: HashSet<String> = text::tokens(name).into_iter().map(|t| t.2).collect();
                if words.iter().all(|w| name.contains(w)) {
                    g.best_score += p.sender_boost;
                }
            }
        }
    }
    sort_groups(&mut groups[..]);
    Ok(())
}

/// `words` appear in `hay` as consecutive tokens, in order.
fn contains_run(hay: &str, words: &[String]) -> bool {
    let toks: Vec<String> = text::tokens(hay)
        .into_iter()
        .map(|t| t.2)
        .filter(|w| !query::is_noise_word(w))
        .collect();
    toks.windows(words.len()).any(|w| w == words)
}

// ---------------------------------------------------------------------------
// the meaning side

enum SideError {
    Index(penguin_semantic::SemanticError),
    Db(crate::Error),
}

impl From<crate::Error> for SideError {
    fn from(e: crate::Error) -> Self {
        SideError::Db(e)
    }
}

impl From<rusqlite::Error> for SideError {
    fn from(e: rusqlite::Error) -> Self {
        SideError::Db(e.into())
    }
}

/// Pre-filtered id sets are built up to this many messages; a broader
/// filter is checked on the candidates after the vector search instead.
const PREFILTER_MAX: usize = 20_000;

/// Accepted (account id → message ids).
type IdSet = HashMap<String, HashSet<String>>;

fn semantic_side(
    c: &Connection,
    inp: &Inputs,
    index: &dyn VectorIndex,
    qvec: &[f32],
) -> std::result::Result<HashMap<i64, SemMsg>, SideError> {
    let p = inp.params;
    let allowed = if inp.plan.narrowing && prefilter_worthwhile(inp.plan) {
        allowed_ids(c, inp.plan)?
    } else {
        None
    };
    let k = if inp.plan.narrowing && allowed.is_none() {
        p.k * p.post_filter_factor
    } else {
        p.k
    };
    // Structured, so an index that can applies them inside its scan, before
    // the nearest-neighbour cut (penguin_semantic::SemanticIndex does).
    let filter = SearchFilter {
        account_ids: inp.accounts.map(|a| a.to_vec()),
        after: inp.q.after,
        before: inp.q.before,
        messages: allowed,
    };
    let hits = index
        .search_where(qvec, k, &filter)
        .map_err(SideError::Index)?;
    // Best chunk per message.
    let mut best: HashMap<(String, String), (f32, u32)> = HashMap::new();
    for h in hits {
        let key = (h.chunk.account_id, h.chunk.message_id);
        let e = best.entry(key).or_insert((f32::MIN, h.chunk.chunk));
        if h.score > e.0 {
            *e = (h.score, h.chunk.chunk);
        }
    }
    if best.is_empty() {
        return Ok(HashMap::new());
    }
    // Store rows by the UNIQUE(account_id, id) index, one lookup each. (A
    // row-value `IN (VALUES …)` next to a rowid range or a flag test made
    // SQLite scan the table instead: 150–500 ms at 300k messages.)
    let mut look =
        c.prepare_cached("SELECT rowid FROM messages WHERE account_id = ?1 AND id = ?2")?;
    let mut by_rowid: HashMap<i64, (f32, u32)> = HashMap::with_capacity(best.len());
    for ((a, m), &v) in &best {
        let mut rows = look.query([a, m])?;
        if let Some(r) = rows.next()? {
            by_rowid.insert(r.get(0)?, v);
        }
    }
    // The query's filters, checked on each candidate with the same SQL the
    // keyword side uses (Trash/Spam, labels, states, fields, exclusions…).
    let rowids: Vec<i64> = by_rowid.keys().copied().collect();
    let mut out = HashMap::new();
    for chunk in rowids.chunks(400) {
        let mut params: Vec<Value> = chunk.iter().map(|&r| Value::Integer(r)).collect();
        let range = rowid_range_sql("m.rowid", inp.plan, &mut params);
        let preds = preds_sql(inp.plan, &mut params, "m.rowid");
        let fts = fts_filter_sql(inp.plan, &mut params);
        let sql = format!(
            "SELECT m.rowid, m.thread_rowid, m.flags, m.from_email FROM messages m
             WHERE m.rowid IN ({}){range}{preds}{fts}",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut stmt = c.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        while let Some(r) = rows.next()? {
            let rowid: i64 = r.get(0)?;
            let Some(&(sim, ch)) = by_rowid.get(&rowid) else {
                continue;
            };
            out.insert(
                r.get::<_, i64>(0)?,
                SemMsg {
                    thread: r.get(1)?,
                    flags: r.get(2)?,
                    from_email: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    sim,
                    chunk: ch,
                },
            );
        }
    }
    Ok(out)
}

/// ` AND` the FTS filters (from:, subject:, has:link…) and NOT the
/// exclusions, checked per row.
fn fts_filter_sql(plan: &Plan, params: &mut Vec<Value>) -> String {
    let mut s = String::new();
    if !plan.fts_filters.is_empty() {
        params.push(Value::Text(plan.fts_filters.join(" AND ")));
        s.push_str(
            " AND EXISTS (SELECT 1 FROM messages_fts WHERE messages_fts MATCH ? AND messages_fts.rowid = m.rowid)",
        );
    }
    if !plan.fts_neg.is_empty() {
        params.push(Value::Text(plan.fts_neg.join(" OR ")));
        s.push_str(
            " AND NOT EXISTS (SELECT 1 FROM messages_fts WHERE messages_fts MATCH ? AND messages_fts.rowid = m.rowid)",
        );
    }
    s
}

/// A filter whose accepted set an index can produce quickly: an FTS field
/// (from:mike), a rowid set (label:, is:new-sender, filename:, is:unread's
/// partial index). Flag tests alone (has:pdf, in:sent) would be a scan of
/// the mailbox; those are checked after the vector search.
fn prefilter_worthwhile(plan: &Plan) -> bool {
    !plan.fts_filters.is_empty() || plan.preds.iter().any(|p| p.contains(crate::search::ROWID))
}

/// Every message the plan accepts ignoring free text, when there are at
/// most `PREFILTER_MAX`; None when there are more.
fn allowed_ids(c: &Connection, plan: &Plan) -> Result<Option<IdSet>> {
    let mut params = Vec::new();
    let sql = if plan.fts_filters.is_empty() {
        let range = rowid_range_sql("m.rowid", plan, &mut params);
        let preds = preds_sql(plan, &mut params, "m.rowid");
        let neg = neg_sql(plan, &mut params);
        format!("SELECT m.account_id, m.id FROM messages m WHERE 1{range}{preds}{neg}")
    } else {
        let mut expr = plan.fts_filters.join(" AND ");
        if !plan.fts_neg.is_empty() {
            expr = format!("({expr}) NOT ({})", plan.fts_neg.join(" OR "));
        }
        params.push(Value::Text(expr));
        let range = rowid_range_sql("messages_fts.rowid", plan, &mut params);
        let preds = preds_sql(plan, &mut params, "m.rowid");
        format!(
            "SELECT m.account_id, m.id FROM messages_fts CROSS JOIN messages m ON m.rowid = messages_fts.rowid
             WHERE messages_fts MATCH ?{range}{preds}"
        )
    };
    let mut stmt = c.prepare_cached(&format!("{sql} LIMIT {}", PREFILTER_MAX + 1))?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out: IdSet = HashMap::new();
    let mut n = 0;
    while let Some(r) = rows.next()? {
        n += 1;
        if n > PREFILTER_MAX {
            return Ok(None);
        }
        out.entry(r.get(0)?).or_default().insert(r.get(1)?);
    }
    Ok(Some(out))
}

fn neg_sql(plan: &Plan, params: &mut Vec<Value>) -> String {
    if plan.fts_neg.is_empty() {
        return String::new();
    }
    params.push(Value::Text(plan.fts_neg.join(" OR ")));
    " AND NOT EXISTS (SELECT 1 FROM messages_fts WHERE messages_fts MATCH ? AND messages_fts.rowid = m.rowid)".into()
}

/// Query vectors of recent searches: typing "flight to lisbon", then
/// deleting a word, asks for texts it has seen. Keyed by model id + text.
const QUERY_CACHE: usize = 64;

/// (model id, query text, vector), least recently used first.
type QueryCache = Mutex<Vec<(String, String, Arc<Vec<f32>>)>>;

fn query_cache() -> &'static QueryCache {
    static CACHE: OnceLock<QueryCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn embed_query(e: &dyn Embedder, text: &str) -> penguin_semantic::Result<Arc<Vec<f32>>> {
    let model = e.model_id();
    {
        let mut cache = query_cache().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(i) = cache.iter().position(|(m, t, _)| m == model && t == text) {
            let hit = cache.remove(i);
            let v = hit.2.clone();
            cache.push(hit);
            return Ok(v);
        }
    }
    let v = Arc::new(e.embed_query(text)?);
    let mut cache = query_cache().lock().unwrap_or_else(|p| p.into_inner());
    if cache.len() >= QUERY_CACHE {
        cache.remove(0);
    }
    cache.push((model.to_string(), text.to_string(), v.clone()));
    Ok(v)
}

// ---------------------------------------------------------------------------
// passages

/// Longest passage shown, in characters.
pub const PASSAGE_CHARS: usize = 240;

/// The passage of chunk `chunk` of a message (cut the way the indexer cut
/// it) that best matches the query's words: the sentence with the most of
/// them (whole words, else a shared 5-letter stem), and what follows it, up
/// to `PASSAGE_CHARS`. With no word in common, the chunk's opening: a
/// meaning match need not share words. Plain text; None when the message
/// has no body text beyond its subject.
pub(crate) fn passage(subject: &str, body: &str, chunk: u32, hl: &Highlighter) -> Option<String> {
    let authored = text::strip_quoted(body);
    let chunks = penguin_semantic::chunk_message(subject, &authored);
    let mut src = chunks
        .get(chunk as usize)
        .or(chunks.last())?
        .as_str();
    if !subject.trim().is_empty() {
        // Every chunk starts with the subject line, which the row already
        // shows (penguin_semantic::chunk).
        src = src.split_once('\n').map_or("", |(_, rest)| rest);
    }
    let src = src.trim();
    if src.is_empty() {
        return None;
    }
    let sentences = sentences(src);
    let stems: Vec<&str> = hl
        .terms
        .iter()
        .filter(|(t, _)| t.chars().count() >= 5)
        .map(|(t, _)| &t[..t.char_indices().nth(5).map_or(t.len(), |(i, _)| i)])
        .collect();
    let mut best = (0.0f64, 0usize);
    for (i, &(a, b)) in sentences.iter().enumerate() {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut stem_hits = 0.0;
        for t in text::tokens(&src[a..b]) {
            if let Some(k) = hl.term_of(&t.2) {
                seen.insert(k);
            } else if stems.iter().any(|s| t.2.starts_with(s)) {
                stem_hits += 0.5;
            }
        }
        let score = seen.len() as f64 + f64::min(stem_hits, 1.0);
        if score > best.0 {
            best = (score, i);
        }
    }
    let start = sentences[best.1].0;
    let mut out = String::new();
    if start > 0 {
        out.push('\u{2026}');
    }
    let rest = &src[start..];
    let room = PASSAGE_CHARS - out.chars().count();
    if rest.chars().count() <= room {
        out.push_str(rest);
    } else {
        let cut = rest.char_indices().nth(room - 1).map_or(rest.len(), |(i, _)| i);
        let cut = rest[..cut].rfind(' ').filter(|&i| i > room / 2).unwrap_or(cut);
        out.push_str(rest[..cut].trim_end_matches(|c: char| !c.is_alphanumeric()));
        out.push('\u{2026}');
    }
    Some(out.replace('\n', " "))
}

/// Byte spans of the sentences of `s` (ends at . ! ? followed by a space, or
/// a line break).
fn sentences(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    for (i, c) in s.char_indices() {
        let end = match c {
            '\n' => Some(i),
            '.' | '!' | '?' if bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace()) => Some(i + 1),
            _ => None,
        };
        if let Some(e) = end {
            // A break right after a sentence end ("?\n") ends nothing new.
            if e <= start {
                continue;
            }
            if s[start..e].trim().len() > 1 {
                out.push((start, e));
            }
            start = e;
            while start < s.len() && s.as_bytes()[start].is_ascii_whitespace() {
                start += 1;
            }
        }
    }
    if s[start..].trim().len() > 1 || out.is_empty() {
        out.push((start, s.len()));
    }
    out
}

/// The passage as snippet HTML: escaped, with the query's words marked.
pub(crate) fn passage_html(p: &str, hl: &Highlighter) -> String {
    let mut out = String::with_capacity(p.len() + 32);
    let mut cursor = 0;
    for (a, b, tok) in text::tokens(p) {
        text::escape_html(&p[cursor..a], &mut out);
        if hl.term_of(&tok).is_some() {
            out.push_str("<mark>");
            text::escape_html(&p[a..b], &mut out);
            out.push_str("</mark>");
        } else {
            text::escape_html(&p[a..b], &mut out);
        }
        cursor = b;
    }
    text::escape_html(&p[cursor..], &mut out);
    out
}

/// `passage` for a message read by rowid.
pub(crate) fn passage_for(
    c: &Connection,
    rowid: i64,
    chunk: u32,
    hl: &Highlighter,
) -> Result<Option<String>> {
    let (subject, body, snippet): (String, String, String) = c
        .prepare_cached(
            "SELECT m.subject, b.body_text, m.snippet FROM messages m JOIN message_bodies b ON b.rowid = m.rowid
             WHERE m.rowid = ?1",
        )?
        .query_row([rowid], |r| {
            Ok((r.get(0)?, r.get::<_, store::Body>(1)?.0, r.get(2)?))
        })?;
    let body = if body.is_empty() { snippet } else { body };
    Ok(passage(&subject, &body, chunk, hl))
}
