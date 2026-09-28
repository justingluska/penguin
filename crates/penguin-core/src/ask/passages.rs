//! Topic questions answered with quoted sentences, no model writing text.
//!
//! 1. **Retrieve** candidate messages two ways: FTS5 (all the question's
//!    words, widened to any of them when that finds too little), and, when
//!    the app provides them, the embedding model + vector index (search by
//!    meaning). The two ranked lists are fused by reciprocal rank,
//!    score = Σ 1/(60 + rank) (Cormack, Clarke & Büttcher, SIGIR 2009).
//! 2. **Read** the authored text of the top messages (quoted history is
//!    left out) and split it into sentences.
//! 3. **Score** each sentence: the share of the question's term weight it
//!    contains (BM25-style idf over the candidate sentences; Robertson &
//!    Zaragoza 2009), how densely those terms sit together (Tellex et al.,
//!    SIGIR 2003: density-based passage scoring works best), meaning
//!    (cosine to the question, when there's a model), whether it holds the
//!    kind of answer the question asks for (a date for "when", an amount
//!    for "how much": Li & Roth 2002's answer types), and the message's
//!    retrieval rank.
//! 4. **Answer** with the best sentences from different messages, quoted
//!    with sender and date. Below the bar, show the closest emails and say
//!    no sentence answers it. Never paraphrase, never add up.

use std::collections::HashSet;

use penguin_semantic::{dot, ChunkRef};

use super::*;

/// Messages read for sentences.
const TOP_DOCS: usize = 12;
/// Sentences embedded per question (the best by words first).
const EMBED_BUDGET: usize = 96;
/// RRF's k (the paper's value).
const RRF_K: f64 = 60.0;

const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "at", "for", "with", "about", "from",
    "is", "are", "was", "were", "be", "been", "do", "does", "did", "i", "we", "you", "my", "our",
    "your", "me", "us", "it", "that", "this", "what", "which", "who", "whom", "when", "where",
    "why", "how", "much", "many", "have", "has", "had", "can", "could", "should", "would", "will",
    "there", "their", "they", "them", "he", "she", "his", "her", "any", "some", "email", "emails",
    "mail", "message", "say", "said", "tell", "know", "get", "got", "again", "el", "la", "los",
    "las", "de", "del", "que", "en", "y", "o", "un", "una", "mi", "mis", "por", "para", "con",
    "sobre", "es", "fue", "cual", "cuando", "donde", "quien", "como", "se", "lo", "le", "al",
];

/// What kind of answer the question wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnswerType {
    Date,
    Money,
    Number,
    Place,
    Person,
    Any,
}

pub(super) fn answer_type(q: &str) -> AnswerType {
    let t = q.trim_start();
    let starts = |ps: &[&str]| ps.iter().any(|p| t.starts_with(p));
    if starts(&[
        "when",
        "what date",
        "what day",
        "what time",
        "which day",
        "cuando",
        "que dia",
        "que fecha",
        "a que hora",
    ]) {
        AnswerType::Date
    } else if starts(&[
        "how much",
        "what did it cost",
        "what is the price",
        "what was the price",
        "what is the cost",
        "cuanto cuesta",
        "cuanto costo",
        "cuanto es",
    ]) || t.contains(" cost")
        || t.contains(" price")
        || t.contains(" budget")
    {
        AnswerType::Money
    } else if starts(&["how many", "cuantos", "cuantas"]) {
        AnswerType::Number
    } else if starts(&["where", "donde"]) {
        AnswerType::Place
    } else if starts(&["who", "quien"]) {
        AnswerType::Person
    } else {
        AnswerType::Any
    }
}

/// Content words of a question or topic, folded.
pub(super) fn content_words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, _, w) in crate::text::tokens(s) {
        if w.len() < 2 || STOP.contains(&w.as_str()) {
            continue;
        }
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// Prefix stem for matching "renewal" ~ "renew", "prices" ~ "price".
fn stem(w: &str) -> String {
    let n = if w.chars().count() > 6 {
        5
    } else {
        w.chars().count().max(4)
    };
    w.chars().take(n).collect()
}

struct Doc {
    row: Row,
    via_lexical: bool,
    via_meaning: bool,
}

struct Sentence {
    doc: usize,
    text: String,
    /// Token stems, in order.
    stems: Vec<String>,
    words: f64,
    density: f64,
    meaning: Option<f32>,
    typed: bool,
    score: f64,
}

/// What the passage pipeline found.
pub(super) struct Found {
    pub passages: Vec<AskPassage>,
    /// Messages retrieved, best first (sources when no sentence answers).
    pub sources: Vec<Row>,
    /// Best sentence's share of the question's term weight (0–1).
    pub best_words: f64,
    pub best_meaning: Option<f32>,
    pub typed: bool,
}

/// Retrieve and score. `person`: only mail from these addresses. `raw`:
/// the input as typed, searched first exactly as search would (so
/// operators, phrases and identifiers keep their meaning).
pub(super) fn find(
    cx: &mut Cx,
    question: &str,
    words: &[String],
    person: Option<&Target>,
    raw: Option<&str>,
) -> Result<Found> {
    let atype = answer_type(&cx.q.text);
    let docs = retrieve(cx, question, words, person, raw)?;
    let texts: Vec<String> = cx.store.read(|c| {
        docs.iter()
            .map(|d| {
                let body = load_text(c, d.row.rowid)?;
                let authored = crate::text::split_quoted(&body).0;
                Ok(if authored.trim().is_empty() {
                    d.row.snippet.clone()
                } else {
                    authored
                })
            })
            .collect()
    })?;
    let stems: Vec<String> = words.iter().map(|w| stem(w)).collect();
    let mut sents: Vec<Sentence> = Vec::new();
    for (i, t) in texts.iter().enumerate() {
        let anchor = local_date(docs[i].row.date, cx.off);
        for s in split_sentences(&format!("{}.\n{}", docs[i].row.subject, t)) {
            let toks: Vec<String> = crate::text::tokens(&s)
                .into_iter()
                .map(|x| stem(&x.2))
                .collect();
            if toks.len() < 3 {
                continue;
            }
            let typed = match atype {
                AnswerType::Date => !extract::date_mentions(&s, anchor).is_empty(),
                AnswerType::Money => !extract::amounts(&s).is_empty(),
                AnswerType::Number => s.chars().any(|c| c.is_ascii_digit()),
                AnswerType::Place => {
                    !crate::structured::patterns_addresses(&s).is_empty()
                        || s.split_whitespace()
                            .skip(1)
                            .any(|w| w.chars().next().is_some_and(char::is_uppercase))
                }
                AnswerType::Person => {
                    s.contains('@')
                        || s.split_whitespace()
                            .skip(1)
                            .any(|w| w.chars().next().is_some_and(char::is_uppercase))
                }
                AnswerType::Any => true,
            };
            sents.push(Sentence {
                doc: i,
                text: s,
                stems: toks,
                words: 0.0,
                density: 0.0,
                meaning: None,
                typed,
                score: 0.0,
            });
        }
    }
    // Term weights: BM25's idf over the candidate sentences.
    let n = sents.len().max(1) as f64;
    let idf: Vec<f64> = stems
        .iter()
        .map(|st| {
            let df = sents
                .iter()
                .filter(|s| s.stems.iter().any(|x| x == st))
                .count() as f64;
            (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
        })
        .collect();
    let total: f64 = idf.iter().sum::<f64>().max(1e-9);
    for s in &mut sents {
        let mut got = 0.0;
        let mut pos: Vec<usize> = Vec::new();
        for (k, st) in stems.iter().enumerate() {
            if let Some(p) = s.stems.iter().position(|x| x == st) {
                got += idf[k];
                pos.push(p);
            }
        }
        s.words = got / total;
        s.density = match (pos.iter().min(), pos.iter().max()) {
            (Some(a), Some(b)) if pos.len() > 1 => pos.len() as f64 / (b - a + 1) as f64,
            (Some(_), _) => 0.5 / (1.0 + s.stems.len() as f64 / 12.0),
            _ => 0.0,
        };
    }
    // Meaning: embed the best sentences by words, plus each doc's first.
    let qvec = match cx.semantic {
        Some(sem) => crate::hybrid::embed_query(sem.embedder, question).ok(),
        None => None,
    };
    if let (Some(sem), Some(q)) = (cx.semantic, &qvec) {
        let mut order: Vec<usize> = (0..sents.len()).collect();
        order.sort_by(|&a, &b| {
            (sents[b].words + 0.01 / (1.0 + sents[b].doc as f64))
                .total_cmp(&(sents[a].words + 0.01 / (1.0 + sents[a].doc as f64)))
        });
        order.truncate(EMBED_BUDGET);
        let owned: Vec<String> = order.iter().map(|&i| sents[i].text.clone()).collect();
        let texts: Vec<&str> = owned.iter().map(String::as_str).collect();
        if let Ok(vs) = sem.embedder.embed_passages_now(&texts) {
            for (i, v) in order.iter().zip(vs) {
                sents[*i].meaning = Some(dot(q, &v));
            }
        }
        cx.steps.push(format!(
            "Compared the meaning of {} with the question ({})",
            plural(owned.len(), "sentence", "sentences"),
            sem.embedder.model_id()
        ));
    }
    let with_meaning = qvec.is_some();
    for s in &mut sents {
        let rank_prior = 0.1 / (1.0 + s.doc as f64);
        let type_term = if atype == AnswerType::Any {
            0.0
        } else if s.typed {
            0.12
        } else {
            -0.25
        };
        s.score = if with_meaning {
            0.45 * s.words + 0.12 * s.density + 0.35 * s.meaning.unwrap_or(0.0).max(0.0) as f64
        } else {
            0.7 * s.words + 0.18 * s.density
        } + type_term
            + rank_prior;
    }
    sents.sort_by(|a, b| b.score.total_cmp(&a.score));
    cx.steps.push(format!(
        "Scored {} in the top {} by the question's words (weighted by rarity), how close together they are, {}and {}",
        plural(sents.len(), "sentence", "sentences"),
        plural(docs.len(), "email", "emails"),
        if with_meaning { "meaning, " } else { "" },
        match atype {
            AnswerType::Date => "whether it gives a date",
            AnswerType::Money => "whether it gives an amount",
            AnswerType::Number => "whether it gives a number",
            AnswerType::Place => "whether it names a place",
            AnswerType::Person => "whether it names someone",
            AnswerType::Any => "the email's rank",
        }
    ));
    // The bar: most of the question's words (or close in meaning), and the
    // right kind of answer.
    let good = |s: &Sentence| {
        let enough = s.words >= 0.5 || s.meaning.is_some_and(|m| m >= 0.75 && s.words >= 0.2);
        enough && (atype == AnswerType::Any || s.typed)
    };
    let mut passages: Vec<AskPassage> = Vec::new();
    let mut used_docs: HashSet<usize> = HashSet::new();
    let mut seen_text: HashSet<String> = HashSet::new();
    let best = sents.first().map(|s| (s.words, s.meaning, s.typed));
    for s in sents.iter().filter(|s| good(s)) {
        if passages.len() >= 3 {
            break;
        }
        let key: String = crate::text::tokens(&s.text)
            .into_iter()
            .map(|t| t.2)
            .collect::<Vec<_>>()
            .join(" ");
        if used_docs.contains(&s.doc) || !seen_text.insert(key) {
            continue;
        }
        used_docs.insert(s.doc);
        let r = &docs[s.doc].row;
        let text = clip(&s.text, 320);
        passages.push(AskPassage {
            marks: marks(&text, &stems),
            text,
            cite: r.cite(),
            from: Address {
                name: r.from_name.clone(),
                email: r.from_email.clone(),
            },
            subject: r.subject_or_none(),
            date: r.date,
            sent: r.by_me,
            score: (s.score.clamp(0.0, 1.0)) as f32,
        });
    }
    let (best_words, best_meaning, typed) = best.unwrap_or((0.0, None, false));
    Ok(Found {
        passages,
        sources: docs.into_iter().map(|d| d.row).collect(),
        best_words,
        best_meaning,
        typed,
    })
}

/// Ranges of `stems` in `text`, in UTF-16 code units (JavaScript string
/// offsets, for the UI's highlighting).
fn marks(text: &str, stems: &[String]) -> Vec<[u32; 2]> {
    let u16_at = |b: usize| text[..b].encode_utf16().count() as u32;
    crate::text::tokens(text)
        .into_iter()
        .filter(|t| stems.iter().any(|s| stem(&t.2) == *s))
        .map(|t| [u16_at(t.0), u16_at(t.1)])
        .collect()
}

/// Sentences of plain text: ends at . ! ? (not after an abbreviation) or a
/// line break; boilerplate lines are dropped.
pub(super) fn split_sentences(text: &str) -> Vec<String> {
    const BOILER: &[&str] = &[
        "unsubscribe",
        "view in browser",
        "sent from my",
        "privacy policy",
        "all rights reserved",
        "manage preferences",
        "this email was sent",
    ];
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let lower = l.to_lowercase();
        if BOILER.iter().any(|b| lower.contains(b)) {
            continue;
        }
        let mut start = 0;
        let b = l.as_bytes();
        for (i, c) in l.char_indices() {
            if matches!(c, '.' | '!' | '?') {
                let next = b.get(i + 1).copied();
                let ends = next.is_none_or(|n| n == b' ' || n == b'"' || n == b')');
                if ends && !(c == '.' && extract::abbrev_dot(l, i)) {
                    let s = l[start..=i].trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                    }
                    start = i + 1;
                }
            }
        }
        let rest = l[start..].trim();
        if !rest.is_empty() {
            out.push(rest.to_string());
        }
    }
    out
}

/// Candidate messages, fused from keyword and meaning retrieval.
fn retrieve(
    cx: &mut Cx,
    question: &str,
    words: &[String],
    person: Option<&Target>,
    raw: Option<&str>,
) -> Result<Vec<Doc>> {
    let who_ops = person.map(|t| format!(" ({})", target_query(t, Dir::FromThem)));
    let range = cx.range_ops();
    let mut lexical: Vec<crate::types::SearchHit> = Vec::new();
    // The input as typed, ranked by search itself: an Ask box that gets a
    // search ("account:studio invoice", "1,284") answers like search.
    if let Some(r) = raw.map(str::trim).filter(|r| !r.is_empty()) {
        let hits = search(cx, r, 30)?;
        cx.steps.push(format!(
            "Searched \u{201c}{r}\u{201d} as typed: {}",
            plural(hits.len(), "thread", "threads")
        ));
        lexical = hits;
    }
    if !words.is_empty() && lexical.len() < 8 {
        let all = format!(
            "{}{}{}",
            words.join(" "),
            who_ops.clone().unwrap_or_default(),
            range
        );
        let hits = search(cx, &all, 30)?;
        cx.steps.push(format!(
            "Searched \u{201c}{}\u{201d}: {}",
            all.trim(),
            plural(hits.len(), "thread", "threads")
        ));
        for h in hits {
            if !lexical
                .iter()
                .any(|x| x.account_id == h.account_id && x.message_id == h.message_id)
            {
                lexical.push(h);
            }
        }
        if lexical.len() < 8 && words.len() > 1 {
            let any = format!(
                "({}){}{}",
                words.join(" OR "),
                who_ops.clone().unwrap_or_default(),
                range
            );
            let more = search(cx, &any, 30)?;
            cx.steps.push(format!(
                "Widened to any of the words: {}",
                plural(more.len(), "thread", "threads")
            ));
            for h in more {
                if !lexical
                    .iter()
                    .any(|x| x.account_id == h.account_id && x.message_id == h.message_id)
                {
                    lexical.push(h);
                }
            }
        }
    }
    // (account, message id) → (lexical rank, meaning rank)
    type Ranked = ((String, String), Option<usize>, Option<usize>);
    let mut ranks: Vec<Ranked> = Vec::new();
    for (i, h) in lexical.iter().enumerate() {
        ranks.push((
            (h.account_id.clone(), h.message_id.clone()),
            Some(i + 1),
            None,
        ));
    }
    if let Some(sem) = cx.semantic {
        if !sem.index.is_empty() {
            let allowed: Option<HashSet<String>> = match person {
                Some(t) => {
                    let (lo, hi) = (cx.lo, cx.hi);
                    let rows = cx.store.read(|c| target_rows(cx, c, t, lo, hi))?;
                    Some(
                        rows.into_iter()
                            .filter(|r| r.from_them)
                            .map(|r| r.id)
                            .collect(),
                    )
                }
                None => None,
            };
            let scope = cx.scope.clone();
            let (lo, hi) = (cx.lo, cx.hi);
            let filter = move |c: &ChunkRef| {
                scope.as_ref().is_none_or(|s| s.contains(&c.account_id))
                    && lo.is_none_or(|l| c.date >= l)
                    && hi.is_none_or(|h| c.date < h)
                    && allowed.as_ref().is_none_or(|a| a.contains(&c.message_id))
            };
            match crate::hybrid::embed_query(sem.embedder, question) {
                Ok(q) => {
                    let hits = sem.index.search(&q, 40, Some(&filter)).unwrap_or_default();
                    let mut rank = 0;
                    let mut seen: HashSet<(String, String)> = HashSet::new();
                    for h in hits {
                        let key = (h.chunk.account_id.clone(), h.chunk.message_id.clone());
                        if !seen.insert(key.clone()) {
                            continue;
                        }
                        rank += 1;
                        match ranks.iter_mut().find(|r| r.0 == key) {
                            Some(r) => r.2 = Some(rank),
                            None => ranks.push((key, None, Some(rank))),
                        }
                    }
                    cx.steps.push(format!(
                        "Searched by meaning ({}, {} indexed passages): {}",
                        sem.embedder.model_id(),
                        sem.index.len(),
                        plural(rank, "email", "emails")
                    ));
                }
                Err(e) => cx.steps.push(format!("Search by meaning unavailable: {e}")),
            }
        }
    }
    let mut fused: Vec<(f64, (String, String), bool, bool)> = ranks
        .into_iter()
        .map(|(k, l, v)| {
            let s = l.map_or(0.0, |r| 1.0 / (RRF_K + r as f64))
                + v.map_or(0.0, |r| 1.0 / (RRF_K + r as f64));
            (s, k, l.is_some(), v.is_some())
        })
        .collect();
    fused.sort_by(|a, b| b.0.total_cmp(&a.0));
    let own = cx.store.read(own_addresses)?;
    let mut docs: Vec<Doc> = cx.store.read(|c| {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
             WHERE m.account_id = ?1 AND m.id = ?2 AND m.flags & ?3 = 0"
        ))?;
        let mut v = Vec::new();
        for (_, (acct, id), l, m) in fused.iter().take(TOP_DOCS * 2) {
            if let Some(mut r) = stmt
                .query_row(params![acct, id, HIDDEN], read_row)
                .optional()?
            {
                r.by_me = r.flags & F_SENT != 0 || own.contains(&r.from_email);
                v.push(Doc {
                    row: r,
                    via_lexical: *l,
                    via_meaning: *m,
                });
            }
        }
        Ok(v)
    })?;
    docs.retain(|d| cx.in_scope(&d.row.account_id));
    docs.truncate(TOP_DOCS);
    if docs.iter().any(|d| d.via_meaning) {
        cx.steps.push(format!(
            "Fused the two lists by reciprocal rank (k = 60): {} found by both, {} by meaning only",
            docs.iter()
                .filter(|d| d.via_lexical && d.via_meaning)
                .count(),
            docs.iter()
                .filter(|d| !d.via_lexical && d.via_meaning)
                .count()
        ));
    }
    Ok(docs)
}

fn aggregation_hint(text: &str) -> bool {
    let t = format!(" {text} ");
    [
        " all ", " every ", " total ", " list ", " each ", " sum ", " todos ", " todas ", " total ",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// Shared answer shape for passage questions.
fn passage_answer(
    cx: &mut Cx,
    intent: AskIntent,
    lead: Option<String>,
    found: Found,
    query: String,
) -> AskAnswer {
    let mut a = cx.answer(intent);
    a.search_query = Some(query);
    if let Some(best) = found.passages.first() {
        let who = if best.sent {
            "You".to_string()
        } else {
            best.from
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| best.from.email.clone())
        };
        let quote = clip(&best.text, 220);
        a.headline = match &lead {
            Some(l) => format!("{l}: \u{201c}{quote}\u{201d}"),
            None => format!("\u{201c}{quote}\u{201d}"),
        };
        a.detail = Some(format!(
            "{who}, {} · {}",
            fmt_day(best.date, cx.off),
            clip(&best.subject, 80)
        ));
        a.confidence = if found.typed
            && (found.best_words >= 0.75 || found.best_meaning.is_some_and(|m| m >= 0.8))
        {
            AskConfidence::Medium
        } else {
            AskConfidence::Low
        };
        a.passages = found.passages;
        a.steps
            .push("Quoted as written; nothing is paraphrased or combined".into());
    } else {
        a.headline = "No sentence in your mail clearly answers that".into();
        a.detail = Some(if found.sources.is_empty() {
            "Nothing matched the question's words either.".to_string()
        } else if aggregation_hint(&cx.q.text) {
            "Here are the closest emails. For totals or counts, ask \u{201c}how many …\u{201d} or \u{201c}how much did I spend …\u{201d}: those read every email."
                .to_string()
        } else {
            "Here are the closest emails.".to_string()
        });
        a.confidence = AskConfidence::None;
    }
    a.items = found
        .sources
        .iter()
        .take(MAX_ITEMS)
        .map(|r| {
            let mut it = r.item(None);
            if let Some(p) = a.passages.iter().find(|p| p.cite == r.cite()) {
                it.note = Some(clip(&p.text, 160));
            }
            it
        })
        .collect();
    a
}

/// "What is the wifi password for the offsite", and every question the
/// grammar doesn't know.
pub(super) fn topic(cx: &mut Cx, intent: AskIntent, topic: &str) -> Result<AskAnswer> {
    let question = cx.question.clone();
    let mut words = content_words(topic);
    if words.is_empty() {
        words = content_words(&cx.q.text);
    }
    let raw = question.trim_matches(['?', '\u{bf}', ' ']).to_string();
    let found = find(cx, &question, &words, None, Some(&raw))?;
    let query = format!("{}{}", words.join(" "), cx.range_ops());
    let a = passage_answer(cx, intent, None, found, query);
    Ok(cx.finish(a))
}

/// "What did Priya say about pricing?"
pub(super) fn said(cx: &mut Cx, who: Option<&str>, topic: &str) -> Result<AskAnswer> {
    let question = cx.question.clone();
    let words = content_words(topic);
    let Some(who) = who else {
        let found = find(cx, &question, &words, None, None)?;
        let query = format!("{}{}", words.join(" "), cx.range_ops());
        let a = passage_answer(cx, AskIntent::Said, None, found, query);
        return Ok(cx.finish(a));
    };
    let (t, candidates) = match resolve_who(cx, who, AskIntent::Said)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(_) => {
            // "what did the team say about the budget": not one person;
            // answer from everyone's mail and say so.
            let found = find(cx, &question, &words, None, None)?;
            let query = format!("{}{}", words.join(" "), cx.range_ops());
            let mut a = passage_answer(cx, AskIntent::Said, None, found, query);
            a.steps.insert(0, format!("\u{201c}{who}\u{201d} isn't a person or company in your mail; looked in everyone's"));
            return Ok(cx.finish(a));
        }
    };
    let found = find(cx, &question, &words, Some(&t), None)?;
    let query = format!(
        "{} ({}){}",
        words.join(" "),
        target_query(&t, Dir::FromThem),
        cx.range_ops()
    );
    let lead = format!(
        "{} on {}",
        t.name.clone().unwrap_or(t.label.clone()),
        words.join(" ")
    );
    let mut a = passage_answer(cx, AskIntent::Said, Some(lead), found, query);
    if a.passages.is_empty() {
        a.headline = format!(
            "I couldn't find {} saying anything about \u{201c}{}\u{201d}",
            t.label,
            words.join(" ")
        );
    }
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.followups = person_followups(&t, AskIntent::Said);
    Ok(cx.finish(a))
}

/// "Did Priya reply about the contract?": the thread about it with them,
/// and who wrote last.
pub(super) fn did_reply(cx: &mut Cx, who: &str, topic: &str) -> Result<AskAnswer> {
    let (t, candidates) = match resolve_who(cx, who, AskIntent::DidReply)? {
        Resolved::Found(t, c) => (t, c),
        Resolved::Answer(a) => return Ok(*a),
    };
    let words = content_words(topic);
    let query = format!(
        "{} ({}){}",
        words.join(" "),
        target_query(&t, Dir::Any),
        cx.range_ops()
    );
    let hits = search(cx, &query, 10)?;
    cx.steps.push(format!(
        "Searched \u{201c}{query}\u{201d}: {}",
        plural(hits.len(), "thread", "threads")
    ));
    let mut a = cx.answer(AskIntent::DidReply);
    a.person = Some(person_of(&t));
    a.candidates = candidates;
    a.search_query = Some(query);
    let name = t.name.clone().unwrap_or(t.label.clone());
    let Some(h) = hits.first() else {
        a.headline = format!(
            "I couldn't find a conversation with {name} about \u{201c}{}\u{201d}",
            words.join(" ")
        );
        a.confidence = AskConfidence::None;
        return Ok(cx.finish(a));
    };
    let own = cx.store.read(own_addresses)?;
    let emails: HashSet<String> = t.emails.iter().map(|e| e.to_lowercase()).collect();
    let thread: Vec<Row> = cx.store.read(|c| {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
             WHERE t.account_id = ?1 AND t.thread_id = ?2 AND m.flags & ?3 = 0 ORDER BY m.rowid"
        ))?;
        let v = stmt
            .query_map(params![h.account_id, h.thread_id, HIDDEN], read_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    })?;
    let thread: Vec<Row> = thread
        .into_iter()
        .map(|mut r| {
            r.by_me = r.flags & F_SENT != 0 || own.contains(&r.from_email);
            r.from_them = emails.contains(&r.from_email);
            r
        })
        .collect();
    let last_mine = thread.iter().rposition(|r| r.by_me);
    let last_theirs = thread.iter().rposition(|r| r.from_them);
    let subject = clip(
        &thread
            .last()
            .map(|r| r.subject_or_none())
            .unwrap_or_default(),
        80,
    );
    cx.steps.push(format!(
        "Read the thread \u{201c}{subject}\u{201d}: {} ({} from you, {} from {name})",
        plural(thread.len(), "message", "messages"),
        thread.iter().filter(|r| r.by_me).count(),
        thread.iter().filter(|r| r.from_them).count()
    ));
    match (last_mine, last_theirs) {
        (Some(m), Some(th)) if th > m => {
            let r = &thread[th];
            let text = cx.store.read(|c| load_text(c, r.rowid))?;
            let first = split_sentences(&crate::text::split_quoted(&text).0)
                .into_iter()
                .find(|s| s.split_whitespace().count() >= 4)
                .unwrap_or_else(|| r.snippet.clone());
            a.headline = format!(
                "Yes: {name} replied on {} (\u{201c}{subject}\u{201d})",
                fmt_day(r.date, cx.off)
            );
            a.detail = Some(format!("\u{201c}{}\u{201d}", clip(&first, 200)));
            a.items = vec![
                r.item(Some("Their reply".into())),
                thread[m].item(Some("Your message".into())),
            ];
            a.confidence = AskConfidence::Medium;
        }
        (Some(m), _) => {
            let r = &thread[m];
            a.headline = format!(
                "Not yet: you wrote last on {} (\u{201c}{subject}\u{201d}), nothing from {name} since",
                fmt_day(r.date, cx.off)
            );
            a.detail = Some(format!(
                "Waiting {}.",
                relative(local_date(r.date, cx.off), cx.today).replace(" ago", "")
            ));
            a.items = vec![r.item(Some("Your last message".into()))];
            a.confidence = AskConfidence::Medium;
        }
        (None, Some(th)) => {
            let r = &thread[th];
            a.headline = format!(
                "{name} wrote about it on {} (\u{201c}{subject}\u{201d}); you haven't replied in that thread",
                fmt_day(r.date, cx.off)
            );
            a.items = vec![r.item(None)];
            a.confidence = AskConfidence::Medium;
        }
        (None, None) => {
            a.headline = format!(
                "The closest thread (\u{201c}{subject}\u{201d}) has nothing from you or {name}"
            );
            a.confidence = AskConfidence::Low;
        }
    }
    a.steps.push(
        "Checked the order of messages in the thread; only the best-matching thread is read".into(),
    );
    Ok(cx.finish(a))
}

/// "Find the lease from Dana": the best matches, attachments first.
pub(super) fn find_thing(cx: &mut Cx, who: Option<&str>, topic: &str) -> Result<AskAnswer> {
    let words = content_words(topic);
    let (t, candidates) = match who {
        Some(w) => match resolve_who(cx, w, AskIntent::Find)? {
            Resolved::Found(t, c) => (Some(t), c),
            Resolved::Answer(a) => return Ok(*a),
        },
        None => (None, vec![]),
    };
    let who_ops = t
        .as_ref()
        .map(|t| format!(" ({})", target_query(t, Dir::FromThem)))
        .unwrap_or_default();
    let query = format!("{}{who_ops}{}", words.join(" "), cx.range_ops());
    let hits = search(cx, &query, 20)?;
    cx.steps.push(format!(
        "Searched \u{201c}{query}\u{201d}: {}",
        plural(hits.len(), "thread", "threads")
    ));
    let mut a = cx.answer(AskIntent::Find);
    a.search_query = Some(query);
    a.candidates = candidates;
    if let Some(t) = &t {
        a.person = Some(person_of(t));
    }
    let stems: Vec<String> = words.iter().map(|w| stem(w)).collect();
    // Attachments whose names carry the words come first.
    let named: Vec<(usize, String)> = cx.store.read(|c| {
        let mut id =
            c.prepare_cached("SELECT rowid FROM messages WHERE account_id = ?1 AND id = ?2")?;
        let mut v = Vec::new();
        for (i, h) in hits.iter().enumerate() {
            if !h.has_attachments {
                continue;
            }
            let Some(rowid) = id
                .query_row(params![h.account_id, h.message_id], |r| r.get::<_, i64>(0))
                .optional()?
            else {
                continue;
            };
            for n in attachment_names(c, rowid)? {
                let toks: Vec<String> = crate::text::tokens(&n)
                    .into_iter()
                    .map(|t| stem(&t.2))
                    .collect();
                if stems.iter().any(|s| toks.contains(s)) {
                    v.push((i, n));
                    break;
                }
            }
        }
        Ok(v)
    })?;
    let mut order: Vec<usize> = named.iter().map(|(i, _)| *i).collect();
    for i in 0..hits.len() {
        if !order.contains(&i) {
            order.push(i);
        }
    }
    a.items = order
        .iter()
        .take(MAX_ITEMS)
        .map(|&i| {
            let h = &hits[i];
            AskItem {
                account_id: h.account_id.clone(),
                thread_id: h.thread_id.clone(),
                message_id: h.message_id.clone(),
                subject: h.subject.clone(),
                from: h.from.clone(),
                date: h.date,
                snippet: clip(&strip_marks(&h.snippet_html), 180),
                note: named
                    .iter()
                    .find(|(j, _)| *j == i)
                    .map(|(_, n)| format!("Attachment: {n}")),
                amount: None,
                sent: h.label_ids.iter().any(|l| l == "SENT"),
            }
        })
        .collect();
    let what = words.join(" ");
    let from = t
        .as_ref()
        .map(|t| format!(" from {}", t.label))
        .unwrap_or_default();
    match a.items.first() {
        None => {
            a.headline = format!("I couldn't find \u{201c}{what}\u{201d}{from}");
            a.confidence = AskConfidence::None;
        }
        Some(first) => {
            a.headline = match named.first() {
                Some((_, n)) => format!("{n}{from}, {}", fmt_day(first.date, cx.off)),
                None => format!(
                    "\u{201c}{}\u{201d}{from}, {}",
                    clip(&first.subject, 90),
                    fmt_day(first.date, cx.off)
                ),
            };
            if hits.len() > 1 {
                a.detail = Some(format!(
                    "{} match; best first.",
                    plural(hits.len(), "conversation", "conversations")
                ));
            }
            a.confidence = if named.is_empty() {
                AskConfidence::Low
            } else {
                AskConfidence::Medium
            };
        }
    }
    Ok(cx.finish(a))
}
