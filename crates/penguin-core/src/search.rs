//! Search execution: parsed query → SQL + FTS5 MATCH → ranked, thread-grouped
//! hits with highlighted snippets, plus attachment and people panels.
//!
//! Ranking. Every matching message gets
//!   score = rel + 0.35·recency + boosts
//! where rel is bm25 normalized to [0, 1] against the best match of this query
//! (column weights: subject 4, sender 2.5, filenames 3, authored body 1,
//! recipients 0.5, cc 0.3, quoted history 0.15), recency = 1 / (1 + age/90d),
//! and the boosts are small: starred +0.10, unread +0.03, sender you write to
//! (from SENT, log-scaled) up to +0.12, and +0.30 when the sender matches a
//! typed name ("mike" boosts mail from the Mikes you know without filtering).
//! Hits are grouped per thread: best message wins, +0.03·ln(matches).
//! Queries without free text (pure filters) are ordered newest first.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};

use crate::query::{self, Atom, Field, Folder, HasKind, Me, ParsedQuery, State};
use crate::store::{
    self, Store, ATTACHMENT_SLOTS, F_ATTACH, F_DOC, F_DRAFT, F_IMAGE, F_IMPORTANT, F_INBOX,
    F_LIST_UNSUB, F_NEWSLETTER, F_PDF, F_PRES, F_SENT, F_SHEET, F_SPAM, F_STARRED, F_TO_ME,
    F_TRASH, F_UNREAD, ROWID_SLOTS,
};
use crate::store::{has_word, word_range};
use crate::hybrid::{self, HybridParams, SemanticHandles};
use crate::text;
use crate::types::*;
use crate::Result;

const BM25: &str = "bm25(messages_fts, 4.0, 2.5, 0.5, 0.3, 1.0, 0.15, 3.0)";
/// The last word is prefix-matched only from this many characters; shorter
/// fragments would expand to most of the vocabulary.
pub const MIN_PREFIX_CHARS: usize = 3;
const MAX_ATTACHMENTS: usize = 24;
const MAX_PEOPLE: usize = 5;
const SNIPPET_TOKENS: usize = 32;
/// Free-text queries score at most this many matching messages, walking the
/// index newest first (rowids are date-ordered). A query matching more than
/// this is unspecific ("the", "in"); scoring every match would cost O(matches)
/// (≈250 ms at 300k), while the newest candidates are where re-finding wins.
pub const MAX_CANDIDATES: usize = 5000;
/// Relevance of a message that matched only another form of the query's
/// words (lexicon.rs), relative to bm25 on the words as typed.
const VARIANT_WEIGHT: f64 = 0.8;
/// Bound for the subject/sender/filename second pass (see `rank_fts`).
const MAX_HIGH_CANDIDATES: usize = 2000;

/// The row's rowid in a predicate that is a rowid set membership test
/// (`@rowid IN (SELECT …)`: label:, is:new-sender, filename: …). Filled in
/// by `preds_sql`: `m.rowid` where messages drives the query, and
/// `FTS_ROWID` where the full-text index does.
pub(crate) const ROWID: &str = "@rowid";
/// The FTS row's rowid, so a set test runs before the messages row is
/// looked up: with a selective set most FTS matches are dropped without
/// touching `messages` (`label:X meeting` walks the whole "meeting"
/// doclist). The unary `+` keeps FTS5 from taking the IN as a list of
/// rowids to seek one at a time, which costs a full MATCH set-up per rowid.
const FTS_ROWID: &str = "+messages_fts.rowid";
/// A rowid prefilter (see `Plan::prefilters`) is used when its set has at
/// most this many rows; building it costs about as much as a few
/// thousand FTS matches' row lookups.
const MAX_PREFILTER_ROWS: usize = 20_000;
/// Unread messages, from the `messages_unread` partial index.
const UNREAD_SET: &str = "SELECT rowid FROM messages WHERE flags & 1 != 0";

#[cfg(test)]
thread_local! {
    /// Tests: plan every query the plain way (set tests on `m.rowid`, no
    /// prefilters), to check the faster plans return the same results.
    pub(crate) static PLAIN_PLANS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn plain_plans() -> bool {
    #[cfg(test)]
    if PLAIN_PLANS.get() {
        return true;
    }
    false
}

/// What `ROWID` becomes where FTS drives the query.
fn fts_rowid() -> &'static str {
    if plain_plans() {
        "m.rowid"
    } else {
        FTS_ROWID
    }
}

pub(crate) fn search(
    store: &Store,
    req: &SearchRequest,
    semantic: Option<&SemanticHandles>,
    params: &HybridParams,
) -> Result<SearchResponse> {
    let started = Instant::now();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let (status, progress) = semantic.map_or((SemanticStatus::Off, None), SemanticHandles::status);
    // Search by meaning as soon as there is an index. It fills newest mail
    // first, so a partial index already covers what most searches are
    // after; the response says "indexing" (with progress) meanwhile, and
    // mail not embedded yet is still found by its words.
    let semantic = semantic.filter(|_| status != SemanticStatus::Off);
    let mut resp = store.read(|c| {
        // Dates in words and misspellings are rewritten only with meaning
        // on; keyword search stays exactly as documented.
        let rw = match semantic {
            Some(_) => hybrid::rewrite(c, &req.query, now)?,
            None => hybrid::Rewrite {
                raw: req.query.clone(),
                embed_raw: req.query.clone(),
                soft_date: None,
                respelled: Vec::new(),
            },
        };
        let q = query::parse(&rw.raw, now);
        run(store, c, q, req, &rw, now, semantic, params)
    })?;
    resp.semantic = status;
    resp.semantic_progress = progress;
    resp.took_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(resp)
}

#[allow(clippy::too_many_arguments)]
fn run(
    store: &Store,
    c: &Connection,
    mut q: ParsedQuery,
    req: &SearchRequest,
    rw: &hybrid::Rewrite,
    now: i64,
    semantic: Option<&SemanticHandles>,
    params: &HybridParams,
) -> Result<SearchResponse> {
    let limit = if req.limit == 0 {
        50
    } else {
        req.limit.min(500)
    } as usize;
    let mut resp = SearchResponse {
        chips: Vec::new(),
        hits: Vec::new(),
        attachments: Vec::new(),
        people: Vec::new(),
        took_ms: 0.0,
        indexed_messages: store::count_messages(c, None)?,
        after_ms: q.after,
        before_ms: q.before,
        events: Vec::new(),
        semantic: SemanticStatus::Off,
        semantic_progress: None,
    };
    if q.is_empty() && !q.events_only {
        resp.chips = std::mem::take(&mut q.chips);
        return Ok(resp);
    }
    let scope = crate::types::account_scope(req.account_id.as_deref(), req.account_ids.as_deref());
    let accounts = resolve_accounts(c, scope.as_deref(), &q.accounts)?;
    let own: HashSet<String> = c
        .prepare_cached("SELECT lower(email) FROM accounts")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut stages = Stages::new();
    resp.events = store::search_events(c, &q, accounts.as_deref(), now, limit, &fts_string)?;
    stages.mark("events");
    if q.events_only {
        resp.chips = std::mem::take(&mut q.chips);
        return Ok(resp);
    }
    upgrade_from_chips(c, &mut q)?;
    merge_label_words(c, &mut q, &rw.raw)?;
    crate::lexicon::expand(c, &mut q)?;
    stages.mark("lexicon");
    resp.people = people_panel(c, &q, &own)?;
    let boost = person_boost(c, &q, &own)?;
    // Cloned, not taken: routing strips the chips' source text from what
    // it embeds (hybrid::semantic_text).
    resp.chips = q.chips.clone();
    stages.mark("people");

    let mut plan = compile(c, &q, accounts.as_deref())?;
    plan.prefilters = selective_prefilters(c, &plan.prefilters)?;
    stages.mark("compile");
    if plan.impossible {
        return Ok(resp);
    }
    let has_text = q.text_terms().next().is_some();
    let mut matches = hybrid::Matches::default();
    let groups = match &plan.fts {
        Some(expr) => {
            let sent_to = sent_to_map(store, c)?;
            stages.mark("sent_to");
            let kw = match fts_candidates(c, &plan, expr, now, &sent_to, &boost) {
                Ok(k) => k,
                Err(e) => {
                    // Our FTS expressions are fully quoted, so this should not
                    // happen; if it does, degrade to a plain phrase match
                    // rather than surfacing FTS syntax to the user.
                    tracing::warn!(error = %e, "fts query failed; retrying as phrase");
                    let phrase: Vec<&str> = q.text_terms().map(|t| t.0).collect();
                    if phrase.is_empty() {
                        KwCands::default()
                    } else {
                        fts_candidates(
                            c,
                            &plan,
                            &fts_string(&phrase.join(" "), false),
                            now,
                            &sent_to,
                            &boost,
                        )
                        .unwrap_or_default()
                    }
                }
            };
            stages.mark("keyword");
            let fused = match semantic {
                Some(sem) => {
                    let mut route = hybrid::route(c, &q, &plan, &rw.embed_raw, params)?;
                    if route.kind == hybrid::RouteKind::Off
                        && rw.soft_date.is_some()
                        && !plan.boolean_text
                    {
                        route = hybrid::Route::dates_only(route.words);
                    }
                    hybrid::fuse(
                        c,
                        &hybrid::Inputs {
                            plan: &plan,
                            q: &q,

                            route: &route,
                            kw: &kw,
                            accounts: accounts.as_deref(),
                            now,
                            sent_to: &sent_to,
                            boost: &boost,
                            params,
                            soft_date: rw.soft_date,
                        },
                        sem,
                        &mut stages,
                    )?
                }
                None => None,
            };
            match fused {
                Some((groups, m)) => {
                    matches = m;
                    groups
                }
                None => {
                    matches.all_words = has_text;
                    kw.groups()
                }
            }
        }
        None => scan_recent(c, &plan, limit, now)?,
    };
    stages.mark("retrieve");
    let hl = Highlighter::new(&q);
    let top: Vec<&Group> = groups.iter().take(limit).collect();
    resp.hits = hydrate(c, &top, &hl, &matches)?;
    stages.mark("hydrate");
    resp.attachments = attachment_panel(c, &q, &plan, &top)?;
    stages.mark("attachments");
    tracing::debug!(stages = %stages, "search");
    Ok(resp)
}

/// Per-stage timings, logged at debug level (never the query text).
pub(crate) struct Stages {
    last: Instant,
    parts: Vec<(&'static str, f64)>,
}

impl Stages {
    fn new() -> Stages {
        Stages {
            last: Instant::now(),
            parts: Vec::new(),
        }
    }
    pub(crate) fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.parts
            .push((name, (now - self.last).as_secs_f64() * 1000.0));
        self.last = now;
    }
}

impl std::fmt::Display for Stages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, (name, ms)) in self.parts.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{name}={ms:.2}ms")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// compilation

#[derive(Clone)]
pub(crate) struct Plan {
    /// Positive FTS expression driving retrieval (None = filter-only query).
    pub(crate) fts: Option<String>,
    /// `fts` without the words' other forms (lexicon.rs), when it has any:
    /// messages matching the words as typed are scored on those alone.
    pub(crate) fts_exact: Option<String>,
    /// Positive from:/to:/cc:/subject: part only (filters the attachment panel).
    field_fts: Option<String>,
    /// SQL predicates over alias `m`, AND-ed; `params` in placeholder order.
    pub(crate) preds: Vec<String>,
    pub(crate) params: Vec<Value>,
    pub(crate) lo: Option<i64>,
    pub(crate) hi: Option<i64>,
    impossible: bool,
    /// Attachment kinds requested via has: (0 = any attachment).
    has_kinds: Vec<i64>,
    /// The FTS expression contains a multi-token phrase (bm25 must then
    /// count phrase matches across the whole index for IDF).
    has_phrase: bool,
    /// The positive part restricted to subject/sender/filenames (exclusions
    /// unchanged): a second, bounded candidate pass for unspecific queries.
    fts_high: Option<String>,
    /// Rowid sets (parameterless SELECTs) that contain every row `preds`
    /// accepts, from an index. Where FTS drives the query and a set is small
    /// (`MAX_PREFILTER_ROWS`), testing it on the FTS rowid skips the row
    /// lookups of matches the filter would reject (`is:unread invoice`).
    /// Redundant with `preds`, so results don't depend on it.
    prefilters: Vec<&'static str>,
    /// The FTS parts that are filters, not free text (from:, to:, cc:,
    /// subject:, domain:, has:link), AND-ed: what search by meaning must
    /// also satisfy. Exclusions are in `fts_neg`.
    pub(crate) fts_filters: Vec<String>,
    /// Excluded FTS terms (`-word`, `-from:x`): no result may match them.
    pub(crate) fts_neg: Vec<String>,
    /// Some free-text term sits in an OR clause or a group: boolean syntax,
    /// answered by the keyword index alone.
    pub(crate) boolean_text: bool,
    /// The query restricts more than accounts, dates and the Trash/Spam
    /// default, so search by meaning pre-filters its candidates when the
    /// restricted set is small.
    pub(crate) narrowing: bool,
}

fn compile(c: &Connection, q: &ParsedQuery, accounts: Option<&[String]>) -> Result<Plan> {
    let mut plan = Plan {
        fts: None,
        fts_exact: None,
        field_fts: None,
        preds: Vec::new(),
        params: Vec::new(),
        lo: q.after.map(|a| a.max(0) * ROWID_SLOTS),
        hi: q.before.map(|b| b.max(0) * ROWID_SLOTS),
        impossible: q.empty_range(),
        has_kinds: Vec::new(),
        has_phrase: false,
        fts_high: None,
        prefilters: Vec::new(),
        fts_filters: Vec::new(),
        fts_neg: Vec::new(),
        boolean_text: false,
        narrowing: false,
    };
    let mut pos: Vec<String> = Vec::new();
    let mut neg: Vec<String> = Vec::new();
    // Does any positive FTS term carry signal (not just "the", "to")?
    let mut informative = false;
    let mut fields: Vec<String> = Vec::new();
    let mut labels_cache: Option<Vec<(String, String, String)>> = None;
    // Your addresses, for from:me / to:me.
    let own: Vec<String> = if q
        .clauses
        .iter()
        .flat_map(|c| &c.terms)
        .any(|t| matches!(t.atom, Atom::Me(_)))
    {
        c.prepare_cached("SELECT DISTINCT lower(email) FROM accounts")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    } else {
        Vec::new()
    };
    let sql_atom =
        |a: &Atom, params: &mut Vec<Value>, labels: &mut Option<Vec<(String, String, String)>>| {
            sql_atom(c, a, &own, params, labels)
        };

    let text = |t: &query::Term| matches!(t.atom, Atom::Text { .. });
    for clause in &q.clauses {
        if clause.terms.len() > 1 && clause.terms.iter().any(text) {
            plan.boolean_text = true;
        }
        if clause.terms.iter().any(|t| !text(t)) {
            plan.narrowing = true;
        }
        if let [t] = clause.terms.as_slice() {
            if let Some(f) = fts_atom(&t.atom, &q.alternatives) {
                if t.negated {
                    neg.push(f);
                } else {
                    if !text(t) {
                        plan.fts_filters.push(f.clone());
                    }
                    if matches!(t.atom, Atom::Field { .. }) {
                        fields.push(f.clone());
                    }
                    plan.has_phrase |= atom_is_phrase(&t.atom);
                    informative |= atom_is_informative(&t.atom);
                    pos.push(f);
                }
                continue;
            }
            if let Atom::Has(k) = t.atom {
                if let (false, Some(code)) = (t.negated, has_kind_code(k)) {
                    plan.has_kinds.push(code);
                }
            }
            if !t.negated && t.atom == Atom::Is(State::Unread) {
                plan.prefilters.push(UNREAD_SET);
            }
            let p = sql_atom(&t.atom, &mut plan.params, &mut labels_cache)?;
            plan.preds
                .push(if t.negated { format!("NOT ({p})") } else { p });
            continue;
        }
        // OR clause.
        let all_fts = clause
            .terms
            .iter()
            .all(|t| !t.negated && fts_atom(&t.atom, &q.alternatives).is_some());
        if all_fts {
            plan.has_phrase |= clause.terms.iter().any(|t| atom_is_phrase(&t.atom));
            informative |= clause.terms.iter().any(|t| atom_is_informative(&t.atom));
            let parts: Vec<String> = clause
                .terms
                .iter()
                .filter_map(|t| fts_atom(&t.atom, &q.alternatives))
                .collect();
            let part = format!("({})", parts.join(" OR "));
            if !clause.terms.iter().any(text) {
                plan.fts_filters.push(part.clone());
            }
            pos.push(part);
            continue;
        }
        let mut ors = Vec::new();
        for t in &clause.terms {
            let p = match fts_atom(&t.atom, &q.alternatives) {
                Some(f) => {
                    plan.params.push(Value::Text(f));
                    format!(
                        "{ROWID} IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)"
                    )
                }
                None => sql_atom(&t.atom, &mut plan.params, &mut labels_cache)?,
            };
            ors.push(if t.negated { format!("NOT ({p})") } else { p });
        }
        plan.preds.push(format!("({})", ors.join(" OR ")));
    }

    if !pos.is_empty() {
        let positive = pos.join(" AND ");
        let mut expr = positive.clone();
        let mut high = format!("{{subject sender filenames}} : ({positive})");
        if !neg.is_empty() {
            expr = format!("({expr}) NOT ({})", neg.join(" OR "));
            high = format!("({high}) NOT ({})", neg.join(" OR "));
        }
        // The same words without their other forms: scored first, so a
        // message that uses both "report" and "reports" isn't counted twice.
        if !q.alternatives.is_empty() {
            let none = std::collections::BTreeMap::new();
            let exact: Vec<String> = q
                .clauses
                .iter()
                .filter_map(|clause| match clause.terms.as_slice() {
                    [t] if !t.negated => fts_atom(&t.atom, &none),
                    [_] => None,
                    terms => terms
                        .iter()
                        .all(|t| !t.negated && fts_atom(&t.atom, &none).is_some())
                        .then(|| {
                            let parts: Vec<String> =
                                terms.iter().filter_map(|t| fts_atom(&t.atom, &none)).collect();
                            format!("({})", parts.join(" OR "))
                        }),
                })
                .collect();
            let exact_pos = exact.join(" AND ");
            let mut exact = exact_pos.clone();
            if !neg.is_empty() {
                exact = format!("({exact}) NOT ({})", neg.join(" OR "));
            }
            if exact != expr {
                plan.fts_exact = Some(exact);
                // The bounded subject/sender/filename pass runs when the
                // typed words fill the window: on the typed words too.
                high = format!("{{subject sender filenames}} : ({exact_pos})");
                if !neg.is_empty() {
                    high = format!("({high}) NOT ({})", neg.join(" OR "));
                }
            }
        }
        plan.fts = Some(expr);
        if informative {
            plan.fts_high = Some(high);
        }
    } else if !neg.is_empty() {
        plan.params.push(Value::Text(neg.join(" OR ")));
        plan.preds.push(format!(
            "{ROWID} NOT IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)"
        ));
    }
    if !fields.is_empty() {
        plan.field_fts = Some(fields.join(" AND "));
    }
    plan.narrowing |= !neg.is_empty();
    plan.fts_neg = neg;

    // Scope: Trash and Spam only when asked for.
    let mut hidden = 0;
    if !q.include_trash {
        hidden |= F_TRASH;
    }
    if !q.include_spam {
        hidden |= F_SPAM;
    }
    if hidden != 0 {
        plan.preds.push(format!("(m.flags & {hidden}) = 0"));
    }
    if let Some(accts) = accounts {
        if accts.is_empty() {
            plan.impossible = true;
        } else {
            plan.preds.push(format!(
                "m.account_id IN ({})",
                vec!["?"; accts.len()].join(",")
            ));
            plan.params
                .extend(accts.iter().map(|a| Value::Text(a.clone())));
        }
    }
    Ok(plan)
}

#[cfg(test)]
pub(crate) fn compile_for_tests(c: &Connection, q: &ParsedQuery) -> Result<Plan> {
    compile(c, q, None)
}

/// FTS5 string literal: the whole input quoted (so no operator or syntax can
/// leak through), `*` for as-you-type prefix matching.
fn fts_string(s: &str, prefix: bool) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\"\""),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    if prefix
        && text::tokens(s)
            .last()
            .is_some_and(|t| t.2.chars().count() >= MIN_PREFIX_CHARS)
    {
        out.push('*');
    }
    out
}

/// Link-looking tokens in the message's own text (has:link).
const LINK_FTS: &str = "{body} : (\"https\" OR \"http\" OR \"www\")";

/// An atom's FTS5 expression; a plain word with other forms in the mail
/// (`ParsedQuery::alternatives`, lexicon.rs) matches any of them.
fn fts_atom(atom: &Atom, alternatives: &std::collections::BTreeMap<String, Vec<String>>) -> Option<String> {
    match atom {
        Atom::Text {
            text,
            prefix,
            phrase: false,
        } if alternatives.contains_key(text) => {
            let mut parts = vec![fts_string(text, *prefix)];
            parts.extend(alternatives[text].iter().map(|a| fts_string(a, false)));
            Some(format!("({})", parts.join(" OR ")))
        }
        Atom::Text { text, prefix, .. } => Some(fts_string(text, *prefix)),
        Atom::Field {
            field,
            value,
            prefix,
        } => {
            let cols = match field {
                Field::From => "sender",
                Field::To => "recipients cc",
                Field::Cc => "cc",
                Field::Subject => "subject",
                Field::Participant | Field::Domain => "sender recipients cc",
            };
            Some(format!("{{{cols}}} : {}", fts_string(value, *prefix)))
        }
        Atom::Near { a, b, distance } => Some(format!(
            "NEAR({} {}, {distance})",
            fts_string(a, false),
            fts_string(b, false)
        )),
        Atom::Has(HasKind::Link) => Some(LINK_FTS.to_string()),
        _ => None,
    }
}

fn atom_is_phrase(atom: &Atom) -> bool {
    match atom {
        Atom::Text { text, .. } | Atom::Field { value: text, .. } => text::tokens(text).len() > 1,
        _ => false,
    }
}

fn atom_is_informative(atom: &Atom) -> bool {
    match atom {
        Atom::Text { text, .. } => text::tokens(text)
            .iter()
            .any(|t| !query::is_noise_word(&t.2)),
        _ => true,
    }
}

/// `attachments.kind` for an attachment has: (0 = any); None for the rest.
fn has_kind_code(k: HasKind) -> Option<i64> {
    Some(match k {
        HasKind::Attachment => 0,
        HasKind::Pdf => 1,
        HasKind::Image => 2,
        HasKind::Doc => 3,
        HasKind::Spreadsheet => 4,
        HasKind::Presentation => 5,
        HasKind::Invite => store::KIND_CALENDAR,
        HasKind::Link | HasKind::Otp | HasKind::Unsubscribe => return None,
    })
}

fn flag_is(bit: i64) -> String {
    format!("(m.flags & {bit}) != 0")
}

fn flag_not(bit: i64) -> String {
    format!("(m.flags & {bit}) = 0")
}

/// Messages received (not sent, not a draft).
const RECEIVED: i64 = F_SENT | F_DRAFT;

/// A later message in the same thread with `flags & mask == value`.
fn later_in_thread(mask: i64, value: i64) -> String {
    format!(
        "EXISTS (SELECT 1 FROM messages s WHERE s.thread_rowid = m.thread_rowid \
         AND s.rowid > m.rowid AND (s.flags & {mask}) = {value})"
    )
}

fn sql_atom(
    c: &Connection,
    atom: &Atom,
    own: &[String],
    params: &mut Vec<Value>,
    labels: &mut Option<Vec<(String, String, String)>>,
) -> Result<String> {
    Ok(match atom {
        Atom::Has(k) => flag_is(match k {
            HasKind::Attachment => F_ATTACH,
            HasKind::Pdf => F_PDF,
            HasKind::Image => F_IMAGE,
            HasKind::Doc => F_DOC,
            HasKind::Spreadsheet => F_SHEET,
            HasKind::Presentation => F_PRES,
            HasKind::Unsubscribe => F_LIST_UNSUB,
            // A rowid list from the `attachments_calendar` partial index
            // (its WHERE must match for SQLite to use it): filter-only
            // queries visit just these messages, where an EXISTS per row
            // walked the whole mailbox (127 ms p50 at 300k messages with no
            // invitations; 0.02 ms now).
            HasKind::Invite => {
                return Ok(format!(
                    "{ROWID} IN (SELECT message_rowid FROM attachments WHERE kind = {})",
                    store::KIND_CALENDAR
                ))
            }
            // otp: NULL = not scanned, '' = scanned and nothing found. The
            // rowid list comes from the messages_otp partial index.
            HasKind::Otp => {
                return Ok(format!(
                    "{ROWID} IN (SELECT rowid FROM messages \
                     WHERE otp IS NOT NULL AND otp != '')"
                ))
            }
            HasKind::Link => {
                params.push(Value::Text(LINK_FTS.to_string()));
                return Ok(format!(
                    "{ROWID} IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)"
                ));
            }
        }),
        Atom::Me(Me::From) => {
            let mut p = format!("(m.flags & {RECEIVED}) != 0");
            if !own.is_empty() {
                p = format!(
                    "({p} OR m.from_email COLLATE NOCASE IN ({}))",
                    vec!["?"; own.len()].join(",")
                );
                params.extend(own.iter().map(|e| Value::Text(e.clone())));
            }
            p
        }
        // Precomputed at insert (a phrase match over every recipient
        // column would cost ~200 ms at 300k).
        Atom::Me(Me::To) => flag_is(F_TO_ME),
        Atom::Size { min, max } => {
            // att_bytes: total size of the attached files (set at insert).
            let clamp = |b: u64| b.min(i64::MAX as u64) as i64;
            let mut parts = Vec::new();
            if let Some(lo) = min {
                parts.push(format!("m.att_bytes >= {}", clamp(*lo)));
            }
            if let Some(hi) = max {
                parts.push(format!("m.att_bytes < {}", clamp(*hi)));
            }
            if parts.is_empty() {
                "1".into()
            } else {
                format!("({})", parts.join(" AND "))
            }
        }
        Atom::ThreadLen { min, max } => {
            let n = "(SELECT tl.message_count FROM threads tl WHERE tl.rowid = m.thread_rowid)";
            match max {
                Some(hi) => format!("{n} BETWEEN {min} AND {hi}"),
                None => format!("{n} >= {min}"),
            }
        }
        Atom::Weekday {
            days,
            utc_offset_secs,
        } => {
            let dow = match utc_offset_secs {
                Some(off) => format!("(((m.date / 1000 + {off}) / 86400 + 4) % 7)"),
                None => "CAST(strftime('%w', m.date / 1000, 'unixepoch', 'localtime') AS INTEGER)"
                    .to_string(),
            };
            format!("((1 << {dow}) & {days}) != 0")
        }
        Atom::In(f) => match f {
            Folder::Inbox => flag_is(F_INBOX),
            Folder::Sent => flag_is(F_SENT),
            Folder::Drafts => flag_is(F_DRAFT),
            Folder::Trash => flag_is(F_TRASH),
            Folder::Spam => flag_is(F_SPAM),
            Folder::Starred => flag_is(F_STARRED),
            Folder::Important => flag_is(F_IMPORTANT),
            Folder::Done => flag_not(F_INBOX),
            Folder::Anywhere => "1".into(),
        },
        Atom::Is(s) => match s {
            State::Unread => flag_is(F_UNREAD),
            State::Read => flag_not(F_UNREAD),
            State::Starred => flag_is(F_STARRED),
            State::Unstarred => flag_not(F_STARRED),
            State::Important => flag_is(F_IMPORTANT),
            State::Newsletter => flag_is(F_NEWSLETTER),
            // The messages of snoozed threads, as a rowid list (few).
            State::Snoozed => format!(
                "{ROWID} IN (SELECT sm.rowid FROM snoozes z \
                 CROSS JOIN threads t ON t.account_id = z.account_id AND t.thread_id = z.thread_id \
                 JOIN messages sm ON sm.thread_rowid = t.rowid WHERE z.woke_at IS NULL)"
            ),
            // First contacts: an indexed lookup of this message in the
            // derived first_contacts table (store_contacts.rs).
            // A rowid IN list: filter-only queries then visit just these
            // messages instead of walking the mailbox newest first.
            State::NewSender => format!(
                "(m.flags & {RECEIVED}) = 0 AND {ROWID} IN (SELECT msg FROM first_contacts WHERE dir = 0)"
            ),
            State::FirstOutbound => format!(
                "{} AND {ROWID} IN (SELECT msg FROM first_contacts WHERE dir = 1)",
                flag_is(F_SENT)
            ),
            // Received, from an address with a first-outbound row: you have
            // written to them (first_contacts dir 1, store_contacts.rs). The
            // address set is built once per statement.
            State::KnownSender => format!(
                "(m.flags & {RECEIVED}) = 0 AND lower(trim(m.from_email)) IN (SELECT email FROM first_contacts WHERE dir = 1)"
            ),
            State::Unanswered => format!(
                "(m.flags & {}) = 0 AND NOT {}",
                RECEIVED | F_NEWSLETTER,
                later_in_thread(F_SENT | F_DRAFT, F_SENT)
            ),
            State::Replied => format!(
                "(m.flags & {RECEIVED}) = 0 AND {}",
                later_in_thread(F_SENT | F_DRAFT, F_SENT)
            ),
            State::Awaiting => format!(
                "{} AND NOT {}",
                flag_is(F_SENT),
                later_in_thread(F_DRAFT | F_TRASH | F_SPAM, 0)
            ),
            State::Reply => "EXISTS (SELECT 1 FROM messages p WHERE p.thread_rowid = m.thread_rowid \
                 AND p.rowid < m.rowid)"
                .into(),
        },
        Atom::Label(name) => {
            if labels.is_none() {
                let rows = c
                    .prepare_cached("SELECT account_id, id, name FROM labels")?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                *labels = Some(rows);
            }
            let ids = resolve_label(labels.as_deref().unwrap_or_default(), name, true);
            if ids.is_empty() {
                return Ok("0".into());
            }
            let mut by_id: HashMap<&str, Vec<&str>> = HashMap::new();
            for (acct, id) in &ids {
                by_id.entry(id.as_str()).or_default().push(acct.as_str());
            }
            let mut ors = Vec::new();
            for (id, accts) in by_id {
                params.push(Value::Text(id.to_string()));
                params.extend(accts.iter().map(|a| Value::Text(a.to_string())));
                ors.push(format!(
                    "({ROWID} IN (SELECT msg FROM message_labels WHERE label = ?) AND m.account_id IN ({}))",
                    vec!["?"; accts.len()].join(",")
                ));
            }
            format!("({})", ors.join(" OR "))
        }
        // Several words ("q3 report" for Q3_report_final.xlsx): each one
        // somewhere in the same attachment's name, whatever joins them.
        Atom::Filename(v) if v.split_whitespace().nth(1).is_some() => {
            let mut parts = Vec::new();
            for w in v.split_whitespace() {
                if w.chars().count() >= 3 {
                    params.push(Value::Text(fts_string(w, false)));
                    parts.push("a.rowid IN (SELECT rowid FROM attachments_fts WHERE attachments_fts MATCH ?)");
                } else {
                    params.push(Value::Text(format!("%{}%", like_escape(w))));
                    parts.push("a.filename LIKE ? ESCAPE '\\'");
                }
            }
            format!(
                "{ROWID} IN (SELECT a.message_rowid FROM attachments a WHERE a.inline = 0 AND {})",
                parts.join(" AND ")
            )
        }
        Atom::Filename(v) => {
            if v.chars().count() >= 3 {
                params.push(Value::Text(fts_string(v, false)));
                format!("{ROWID} IN (SELECT a.message_rowid FROM attachments_fts JOIN attachments a ON a.rowid = attachments_fts.rowid WHERE attachments_fts MATCH ?)")
            } else {
                params.push(Value::Text(format!("%{}%", like_escape(v))));
                format!("{ROWID} IN (SELECT message_rowid FROM attachments WHERE inline = 0 AND filename LIKE ? ESCAPE '\\')")
            }
        }
        Atom::Text { .. } | Atom::Field { .. } | Atom::Near { .. } => {
            unreachable!("FTS atoms are compiled by fts_atom")
        }
    })
}

fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn norm_label(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if c.is_whitespace() || matches!(c, '-' | '_' | '/' | '.') {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect::<String>()
        .to_lowercase()
}

/// Resolve `label:value` to (account_id, label_id) pairs: by id, full name,
/// Gmail's dashed form of nested names ("clients-acme"), last path segment,
/// or the category shorthands (label:promotions).
fn resolve_label(labels: &[(String, String, String)], value: &str, partial: bool) -> Vec<(String, String)> {
    let v = norm_label(value);
    let category = match v.as_str() {
        "promotions" | "social" | "updates" | "forums" | "personal" => {
            Some(format!("CATEGORY_{}", v.to_uppercase()))
        }
        _ => None,
    };
    let pick = |f: &dyn Fn(&str, &str) -> bool| -> Vec<(String, String)> {
        labels
            .iter()
            .filter(|(_, id, name)| f(id, name))
            .map(|(a, id, _)| (a.clone(), id.clone()))
            .collect()
    };
    let mut out = pick(&|id, name| {
        id.eq_ignore_ascii_case(value)
            || category.as_deref() == Some(id)
            || norm_label(name) == v
            // The last segments of a nested name: "atlas/finance" for
            // "Work/Atlas/Finance", "finance".
            || name
                .char_indices()
                .filter(|(_, c)| *c == '/')
                .any(|(i, _)| norm_label(&name[i + 1..]) == v)
    });
    // Nothing by name: labels with a word that starts with it
    // ("fernhill" → "Clients/Fernhill Bakery").
    if partial && out.is_empty() && category.is_none() && v.chars().count() >= 3 && !v.contains('-') {
        out = pick(&|_, name| norm_label(name).split('-').any(|w| w.starts_with(&v)));
    }
    if out.is_empty() {
        if let Some(cat) = category {
            // Categories may be absent from the labels table; they still exist.
            let accts: HashSet<&String> = labels.iter().map(|(a, _, _)| a).collect();
            out = accts
                .into_iter()
                .map(|a| (a.clone(), cat.clone()))
                .collect();
        }
    }
    out
}

/// `account:` values matched against the accounts, within the request's
/// scope (None = every account). Without `account:` the scope is the answer.
fn resolve_accounts(
    c: &Connection,
    scope: Option<&[String]>,
    values: &[String],
) -> Result<Option<Vec<String>>> {
    if values.is_empty() {
        return Ok(scope.map(<[String]>::to_vec));
    }
    let all: Vec<(String, String, Option<String>, Option<String>)> = c
        .prepare_cached("SELECT id, email, display_name, nickname FROM accounts")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let contains = |field: &Option<String>, v: &str| {
        field
            .as_deref()
            .is_some_and(|n| n.to_lowercase().contains(v))
    };
    let matched = all
        .into_iter()
        .filter(|(id, email, name, nickname)| {
            let email = email.to_lowercase();
            let domain = email.split('@').nth(1).unwrap_or("");
            values.iter().any(|v| {
                id.eq_ignore_ascii_case(v)
                    || email.starts_with(v.as_str())
                    || domain.starts_with(v.as_str())
                    || contains(nickname, v)
                    || contains(name, v)
            })
        })
        .map(|(id, ..)| id)
        .filter(|id| scope.is_none_or(|s| s.contains(id)))
        .collect();
    Ok(Some(matched))
}

// ---------------------------------------------------------------------------
// retrieval + ranking

pub(crate) struct Group {
    pub(crate) thread_rowid: i64,
    pub(crate) best_rowid: i64,
    pub(crate) best_score: f64,
    pub(crate) count: u32,
}

/// Collapse scored messages `(rowid, thread rowid, score)` into threads: the
/// best message represents its thread, which gets +0.03·ln(matches); best
/// first (newest first on ties).
pub(crate) fn group_scored(scored: impl Iterator<Item = (i64, i64, f64)>) -> Vec<Group> {
    let mut groups: HashMap<i64, Group> = HashMap::new();
    for (rowid, thread, score) in scored {
        let g = groups.entry(thread).or_insert(Group {
            thread_rowid: thread,
            best_rowid: rowid,
            best_score: f64::MIN,
            count: 0,
        });
        g.count += 1;
        if score > g.best_score || (score == g.best_score && rowid > g.best_rowid) {
            g.best_score = score;
            g.best_rowid = rowid;
        }
    }
    let mut out: Vec<Group> = groups
        .into_values()
        .map(|mut g| {
            g.best_score += 0.03 * (g.count as f64).ln();
            g
        })
        .collect();
    sort_groups(&mut out);
    out
}

pub(crate) fn sort_groups(groups: &mut [Group]) {
    groups.sort_by(|a, b| {
        b.best_score
            .total_cmp(&a.best_score)
            .then(b.best_rowid.cmp(&a.best_rowid))
    });
}

pub(crate) fn rowid_range_sql(col: &str, plan: &Plan, params: &mut Vec<Value>) -> String {
    let mut s = String::new();
    if let Some(lo) = plan.lo {
        s.push_str(&format!(" AND {col} >= ?"));
        params.push(Value::Integer(lo));
    }
    if let Some(hi) = plan.hi {
        s.push_str(&format!(" AND {col} < ?"));
        params.push(Value::Integer(hi));
    }
    s
}

/// `plan.preds` as ` AND …` SQL, with `rowid` for the `ROWID` placeholder.
pub(crate) fn preds_sql(plan: &Plan, params: &mut Vec<Value>, rowid: &str) -> String {
    params.extend(plan.params.iter().cloned());
    plan.preds
        .iter()
        .map(|p| format!(" AND {}", p.replace(ROWID, rowid)))
        .collect()
}

/// The prefilter sets small enough to be worth building (a bounded count
/// on each set's index decides).
fn selective_prefilters(c: &Connection, sets: &[&'static str]) -> Result<Vec<&'static str>> {
    let mut out = Vec::new();
    if plain_plans() {
        return Ok(out);
    }
    for set in sets {
        let n: i64 = c
            .prepare_cached(&format!(
                "SELECT count(*) FROM ({set} LIMIT {})",
                MAX_PREFILTER_ROWS + 1
            ))?
            .query_row([], |r| r.get(0))?;
        if n as usize <= MAX_PREFILTER_ROWS {
            out.push(*set);
        }
    }
    Ok(out)
}

/// ` AND <rowid> IN (…)` for each of the plan's prefilters (only the
/// selective ones remain after `selective_prefilters`).
fn prefilter_sql(plan: &Plan, rowid: &str) -> String {
    plan.prefilters
        .iter()
        .map(|set| format!(" AND {rowid} IN ({set})"))
        .collect()
}

pub(crate) fn recency(now: i64, date: i64) -> f64 {
    let age_days = (now - date).max(0) as f64 / 86_400_000.0;
    1.0 / (1.0 + age_days / 90.0)
}

/// The keyword side of a search: every scored full-text match.
#[derive(Default)]
pub(crate) struct KwCands {
    pub(crate) cands: HashMap<i64, Cand>,
    /// 1 / the best relevance (0 when nothing was scored): `rel · norm` is
    /// bm25 normalized to [0, 1] against this query's best match.
    pub(crate) norm: f64,
}

impl KwCands {
    /// Today's keyword ranking: normalized bm25 + recency and boosts.
    pub(crate) fn groups(&self) -> Vec<Group> {
        group_scored(
            self.cands
                .iter()
                .map(|(&rowid, c)| (rowid, c.thread, c.rel * self.norm + c.extra)),
        )
    }
}

fn fts_candidates(
    c: &Connection,
    plan: &Plan,
    expr: &str,
    now: i64,
    sent_to: &HashMap<String, u32>,
    boost: &HashSet<String>,
) -> Result<KwCands> {
    // A phrase of common words ("to the") matches most of the index, and
    // bm25's IDF for it means evaluating the phrase on every match (~60 ms at
    // 300k). Such a phrase has no discriminating IDF anyway: when a cheap,
    // bounded rowid walk finds more matches than we would score, rank those
    // by recency and boosts instead.
    let mut use_bm25 = true;
    if plan.has_phrase {
        let mut params = vec![Value::Text(expr.to_string())];
        let range = rowid_range_sql("rowid", plan, &mut params);
        let n: i64 = c
            .prepare_cached(&format!(
                "SELECT count(*) FROM (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?{range} LIMIT {})",
                MAX_CANDIDATES + 1
            ))?
            .query_row(params_from_iter(params), |r| r.get(0))?;
        use_bm25 = n as usize <= MAX_CANDIDATES;
    }

    let prefilter = prefilter_sql(plan, fts_rowid());
    let mut cands: HashMap<i64, Cand> = HashMap::new();
    // With other forms of the words (lexicon.rs): messages matching the
    // words as typed are scored on those first; the rest come in on the
    // forms they use. bm25 would otherwise add up both "report" and
    // "reports" in a message that has both.
    let mut exact_cap = false;
    if let Some(exact) = plan.fts_exact.as_deref() {
        exact_cap = collect_candidates(
            c,
            plan,
            &prefilter,
            exact,
            use_bm25,
            MAX_CANDIDATES,
            now,
            sent_to,
            boost,
            &mut cands,
        )?;
    }
    // The typed words alone already fill the candidate window: the other
    // forms could only add to its tail, at the cost of a second full walk.
    let hit_cap = exact_cap
        || collect_candidates(
            c,
            plan,
            &prefilter,
            expr,
            use_bm25,
            MAX_CANDIDATES,
            now,
            sent_to,
            boost,
            &mut cands,
        )?;
    // Unspecific query: the newest-first window may have cut off older mail
    // whose subject, sender or filename matches. Those are the strongest
    // known-item signals, so give them their own bounded pass.
    if hit_cap && use_bm25 {
        if let Some(high) = plan.fts_high.as_deref().filter(|h| *h != expr) {
            collect_candidates(
                c,
                plan,
                &prefilter,
                high,
                true,
                MAX_HIGH_CANDIDATES,
                now,
                sent_to,
                boost,
                &mut cands,
            )?;
        }
    }

    let max_rel = cands.values().map(|c| c.rel).fold(0.0, f64::max);
    let norm = if max_rel > 0.0 { 1.0 / max_rel } else { 0.0 };
    Ok(KwCands { cands, norm })
}

pub(crate) struct Cand {
    pub(crate) thread: i64,
    /// -bm25 (higher is better), 0 when not scored.
    pub(crate) rel: f64,
    /// Recency and boosts.
    pub(crate) extra: f64,
    pub(crate) flags: i64,
}

/// The query-independent part of a message's score: recency, starred,
/// unread, a sender you write to, and a sender a typed word names.
pub(crate) fn prior(
    now: i64,
    rowid: i64,
    flags: i64,
    from_email: &str,
    sent_to: &HashMap<String, u32>,
    boost: &HashSet<String>,
) -> f64 {
    let mut extra = 0.35 * recency(now, rowid / ROWID_SLOTS);
    if flags & F_STARRED != 0 {
        extra += 0.10;
    }
    if flags & F_UNREAD != 0 {
        extra += 0.03;
    }
    if !sent_to.is_empty() || !boost.is_empty() {
        let lower;
        let email = if from_email.bytes().any(|b| b.is_ascii_uppercase()) {
            lower = from_email.to_ascii_lowercase();
            lower.as_str()
        } else {
            from_email
        };
        if let Some(&n) = sent_to.get(email) {
            extra += 0.12 * ((1.0 + n as f64).ln() / 50f64.ln()).min(1.0);
        }
        if boost.contains(email) {
            extra += 0.30;
        }
    }
    extra
}

/// Score up to `cap` newest matches of `expr` into `cands` (keeping the
/// better relevance for rows seen twice). Returns whether the cap was hit.
#[allow(clippy::too_many_arguments)]
fn collect_candidates(
    c: &Connection,
    plan: &Plan,
    prefilter: &str,
    expr: &str,
    use_bm25: bool,
    cap: usize,
    now: i64,
    sent_to: &HashMap<String, u32>,
    boost: &HashSet<String>,
    cands: &mut HashMap<i64, Cand>,
) -> Result<bool> {
    let rel_expr = if use_bm25 { BM25 } else { "0.0" };
    let mut params = vec![Value::Text(expr.to_string())];
    let range = rowid_range_sql("messages_fts.rowid", plan, &mut params);
    let preds = preds_sql(plan, &mut params, fts_rowid());
    let sql = format!(
        "SELECT m.rowid, m.thread_rowid, m.flags, m.from_email, {rel_expr}
         FROM messages_fts CROSS JOIN messages m ON m.rowid = messages_fts.rowid
         WHERE messages_fts MATCH ?{range}{prefilter}{preds}
         ORDER BY messages_fts.rowid DESC LIMIT {cap}"
    );
    let mut stmt = c.prepare_cached(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut n = 0;
    while let Some(r) = rows.next()? {
        n += 1;
        let rowid: i64 = r.get(0)?;
        let mut rel = -r.get::<_, f64>(4)?;
        // The expanded pass after the exact one (`fts_exact`).
        let variants = plan.fts_exact.is_some() && expr == plan.fts.as_deref().unwrap_or_default();
        if let Some(existing) = cands.get_mut(&rowid) {
            // Scored on the exact words already: keep that.
            if !variants {
                existing.rel = existing.rel.max(rel);
            }
            continue;
        }
        if variants {
            // Only another form of the words matched: a little less sure
            // than the words as typed (exact matches stay authoritative).
            rel *= VARIANT_WEIGHT;
        }
        let flags: i64 = r.get(2)?;
        let email = r.get_ref(3)?.as_str().unwrap_or("");
        let extra = prior(now, rowid, flags, email, sent_to, boost);
        cands.insert(
            rowid,
            Cand {
                thread: r.get(1)?,
                rel,
                extra,
                flags,
            },
        );
    }
    Ok(n >= cap)
}

/// Filter-only queries: newest matching messages first, grouped by thread.
fn scan_recent(c: &Connection, plan: &Plan, limit: usize, now: i64) -> Result<Vec<Group>> {
    let mut params = Vec::new();
    let range = rowid_range_sql("m.rowid", plan, &mut params);
    let preds = preds_sql(plan, &mut params, "m.rowid");
    // NOT INDEXED keeps the newest-first rowid walk that stops after `limit`
    // threads. With an account restriction SQLite otherwise picks the
    // (account_id, id) index and sorts every message of those accounts
    // (~120 ms for `is:unread` in a 200k-message scope). Rowid IN lookups
    // (label:, filename:) are still used.
    let sql = format!(
        "SELECT m.rowid, m.thread_rowid FROM messages m NOT INDEXED WHERE 1{range}{preds} ORDER BY m.rowid DESC"
    );
    let mut out: Vec<Group> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    {
        let mut stmt = c.prepare_cached(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        while let Some(r) = rows.next()? {
            let rowid: i64 = r.get(0)?;
            let thread: i64 = r.get(1)?;
            if seen.insert(thread) {
                out.push(Group {
                    thread_rowid: thread,
                    best_rowid: rowid,
                    best_score: recency(now, rowid / ROWID_SLOTS),
                    count: 0,
                });
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    // Exact per-thread match counts for the page we return, in one
    // statement: a predicate with an IN (subquery) (label:, is:new-sender…)
    // is then materialized once, not once per thread.
    if out.is_empty() {
        return Ok(out);
    }
    let mut params: Vec<Value> = out.iter().map(|g| Value::Integer(g.thread_rowid)).collect();
    let range = rowid_range_sql("m.rowid", plan, &mut params);
    let preds = preds_sql(plan, &mut params, "m.rowid");
    let sql = format!(
        "SELECT m.thread_rowid, count(*) FROM messages m WHERE m.thread_rowid IN ({}){range}{preds}
         GROUP BY m.thread_rowid",
        vec!["?"; out.len()].join(",")
    );
    let counts: HashMap<i64, u32> = c
        .prepare(&sql)?
        .query_map(params_from_iter(params), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u32))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for g in &mut out {
        g.count = counts.get(&g.thread_rowid).copied().unwrap_or(0);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// rule conditions (OWNER of this block: rules agent)
//
// Rules use the search language unchanged: the same parse + compile, so a
// condition matches exactly what typing it into search would (every
// matching message, not ranked and not grouped by thread).

/// (account id, message id) pairs per `IN (VALUES …)` statement.
const MATCH_ONLY_CHUNK: usize = 200;

/// `rowid`, then the message identity, for every row the plan matches.
fn match_sql(plan: &Plan, only: Option<usize>, params: &mut Vec<Value>) -> String {
    let only_sql = only
        .map(|n| {
            format!(
                " AND (m.account_id, m.id) IN (VALUES {})",
                vec!["(?, ?)"; n].join(", ")
            )
        })
        .unwrap_or_default();
    let cols =
        "m.rowid, m.account_id, m.id, t.thread_id, m.date, m.from_name, m.from_email, m.subject";
    match &plan.fts {
        // A handful of known messages: drive from them (the UNIQUE index)
        // and let FTS check each rowid, instead of walking every FTS match.
        Some(expr) if only.is_some() => {
            // Placeholders: range, MATCH, preds; the caller appends the pairs.
            let range = rowid_range_sql("m.rowid", plan, params);
            params.push(Value::Text(expr.clone()));
            let preds = preds_sql(plan, params, "m.rowid");
            format!(
                "SELECT {cols} FROM messages m CROSS JOIN messages_fts ON messages_fts.rowid = m.rowid
                 JOIN threads t ON t.rowid = m.thread_rowid
                 WHERE 1{range} AND messages_fts MATCH ?{preds}{only_sql} ORDER BY m.rowid DESC"
            )
        }
        Some(expr) => {
            params.push(Value::Text(expr.clone()));
            let range = rowid_range_sql("messages_fts.rowid", plan, params);
            let preds = preds_sql(plan, params, fts_rowid());
            format!(
                "SELECT {cols} FROM messages_fts CROSS JOIN messages m ON m.rowid = messages_fts.rowid
                 JOIN threads t ON t.rowid = m.thread_rowid
                 WHERE messages_fts MATCH ?{range}{preds}{only_sql} ORDER BY messages_fts.rowid DESC"
            )
        }
        None => {
            let range = rowid_range_sql("m.rowid", plan, params);
            let preds = preds_sql(plan, params, "m.rowid");
            format!(
                "SELECT {cols} FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
                 WHERE 1{range}{preds}{only_sql} ORDER BY m.rowid DESC"
            )
        }
    }
}

/// Split Inbox (`store_split.rs`): a split's query compiled the way a rule's
/// condition is (an unreadable date or `type:event` is `InvalidQuery`).
/// None = it can't match anything.
pub(crate) fn compile_split(
    c: &Connection,
    query: &str,
    accounts: Option<&[AccountId]>,
    now: i64,
) -> Result<Option<Plan>> {
    compile_condition(c, query, accounts, now)
}

/// Split Inbox: a split query ready to test chunks of the inbox with. A
/// full-text part is looked up once, the first time a chunk needs it
/// (`fts_rows`: the matching message rowids), not once per chunk; past
/// `MAX_SPLIT_FTS_ROWS` it stays a subquery in each chunk's statement
/// instead of a set in memory.
pub(crate) struct SplitPlan {
    pub(crate) plan: Plan,
    fts_rows: std::cell::OnceCell<Option<HashSet<i64>>>,
}

const MAX_SPLIT_FTS_ROWS: usize = 500_000;

pub(crate) fn prepare_split(plan: Plan) -> SplitPlan {
    SplitPlan {
        plan,
        fts_rows: std::cell::OnceCell::new(),
    }
}

/// The plan's full-text matches as a rowid set (built on first use), or
/// None: no full-text part, or too many matches to hold.
fn split_fts_rows<'a>(c: &Connection, sp: &'a SplitPlan) -> Result<Option<&'a HashSet<i64>>> {
    let Some(expr) = &sp.plan.fts else {
        return Ok(None);
    };
    if sp.fts_rows.get().is_none() {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1 LIMIT {}",
            MAX_SPLIT_FTS_ROWS + 1
        ))?;
        let rows: HashSet<i64> = stmt
            .query_map([expr], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let _ = sp
            .fts_rows
            .set((rows.len() <= MAX_SPLIT_FTS_ROWS).then_some(rows));
    }
    Ok(sp.fts_rows.get().and_then(Option::as_ref))
}

/// Split Inbox: which of `threads` (thread rowids) have a message in the
/// inbox that matches the split. Driven by the threads' messages (the
/// `messages_thread` index); the full-text part is the prepared rowid set
/// (or one doclist walk per statement), never a MATCH per row, which sets
/// the whole query up again for each row (seconds over a chunk). So it costs
/// O(their messages).
pub(crate) fn inbox_threads_matching(
    c: &Connection,
    sp: &SplitPlan,
    threads: &[i64],
) -> Result<HashSet<i64>> {
    let plan = &sp.plan;
    if threads.is_empty() || plan.impossible {
        return Ok(HashSet::new());
    }
    let mut params = vec![Value::Text(serde_json::to_string(threads)?)];
    let range = rowid_range_sql("m.rowid", plan, &mut params);
    let fts_rows = split_fts_rows(c, sp)?;
    let fts = match (&plan.fts, fts_rows) {
        (Some(expr), None) => {
            params.push(Value::Text(expr.clone()));
            " AND m.rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)"
        }
        _ => "",
    };
    let preds = preds_sql(plan, &mut params, "m.rowid");
    let sql = format!(
        "SELECT m.thread_rowid, m.rowid FROM messages m JOIN threads t ON t.rowid = m.thread_rowid
         WHERE m.thread_rowid IN (SELECT value FROM json_each(?)){range} AND m.flags & {F_INBOX} != 0{fts}{preds}"
    );
    let mut stmt = c.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut out = HashSet::new();
    for row in rows {
        let (thread, msg) = row?;
        if fts_rows.is_none_or(|set| set.contains(&msg)) {
            out.insert(thread);
        }
    }
    Ok(out)
}

/// Split Inbox: the threads (rowids) of every inbox message matching a
/// full-text `plan`, when there are at most `cap` such messages; None when
/// there are more (the set would be incomplete) or the plan has no
/// full-text part to drive the lookup.
pub(crate) fn inbox_threads_of_matches(
    c: &Connection,
    plan: &Plan,
    cap: usize,
) -> Result<Option<HashSet<i64>>> {
    if plan.fts.is_none() {
        return Ok(None);
    }
    if plan.impossible {
        return Ok(Some(HashSet::new()));
    }
    let mut p = plan.clone();
    p.preds.push(format!("m.flags & {F_INBOX} != 0"));
    let mut params = Vec::new();
    let sql = match_sql(&p, None, &mut params);
    let mut stmt = c.prepare(&format!(
        "SELECT (SELECT t2.rowid FROM threads t2 WHERE t2.account_id = x.account_id AND t2.thread_id = x.thread_id)
         FROM ({sql} LIMIT {}) x",
        cap.saturating_add(1)
    ))?;
    let rows: Vec<Option<i64>> = stmt
        .query_map(params_from_iter(params), |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if rows.len() > cap {
        return Ok(None);
    }
    Ok(Some(rows.into_iter().flatten().collect()))
}

fn compile_condition(
    c: &Connection,
    query: &str,
    accounts: Option<&[AccountId]>,
    now: i64,
) -> Result<Option<Plan>> {
    let q = query::parse(query, now);
    // Interactive search shrugs these off; a rule must not silently widen
    // (an unreadable date constrains nothing) or target calendar events.
    if let Some(bad) = q.chips.iter().find(|c| c.kind == "error") {
        return Err(crate::Error::InvalidQuery(format!(
            "{}: {}",
            bad.label, bad.raw
        )));
    }
    if q.events_only {
        return Err(crate::Error::InvalidQuery(
            "rules act on mail; type:event can't be a rule condition".into(),
        ));
    }
    if q.is_empty() {
        return Err(crate::Error::InvalidQuery(
            "a rule condition needs at least one search term".into(),
        ));
    }
    let scope = accounts.map(<[AccountId]>::to_vec);
    let accounts = resolve_accounts(c, scope.as_deref(), &q.accounts)?;
    let plan = compile(c, &q, accounts.as_deref())?;
    Ok((!plan.impossible).then_some(plan))
}

impl Store {
    /// Messages matching `query` (the search language), newest first, at
    /// most `limit`. `accounts`: limit to these accounts (None = all).
    /// `only`: limit to these (account id, message id) pairs — how a rule
    /// checks just-arrived mail. An empty query is `InvalidQuery`.
    pub fn match_query(
        &self,
        query: &str,
        accounts: Option<&[AccountId]>,
        only: Option<&[(AccountId, String)]>,
        limit: usize,
    ) -> Result<Vec<QueryMatch>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.read(|c| {
            let Some(plan) = compile_condition(c, query, accounts, now)? else {
                return Ok(Vec::new());
            };
            let chunks: Vec<Option<&[(AccountId, String)]>> = match only {
                None => vec![None],
                Some(pairs) => pairs.chunks(MATCH_ONLY_CHUNK).map(Some).collect(),
            };
            let mut out: Vec<(i64, QueryMatch)> = Vec::new();
            for chunk in chunks {
                if chunk.is_some_and(<[_]>::is_empty) {
                    continue;
                }
                let mut params = Vec::new();
                let sql = match_sql(&plan, chunk.map(<[_]>::len), &mut params);
                for (a, m) in chunk.unwrap_or_default() {
                    params.push(Value::Text(a.clone()));
                    params.push(Value::Text(m.clone()));
                }
                let mut stmt = c.prepare(&format!("{sql} LIMIT {limit}"))?;
                let rows = stmt.query_map(params_from_iter(params), |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        QueryMatch {
                            account_id: r.get(1)?,
                            message_id: r.get(2)?,
                            thread_id: r.get(3)?,
                            date: r.get(4)?,
                            from: Address {
                                name: r.get(5)?,
                                email: r.get(6)?,
                            },
                            subject: r.get(7)?,
                        },
                    ))
                })?;
                for row in rows {
                    out.push(row?);
                }
            }
            out.sort_by_key(|m| std::cmp::Reverse(m.0));
            out.truncate(limit);
            Ok(out.into_iter().map(|(_, m)| m).collect())
        })
    }

    /// How many messages match `query`, counting at most `cap`
    /// (returns `(count, capped)`).
    pub fn count_query_matches(
        &self,
        query: &str,
        accounts: Option<&[AccountId]>,
        cap: usize,
    ) -> Result<(u64, bool)> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.read(|c| {
            let Some(plan) = compile_condition(c, query, accounts, now)? else {
                return Ok((0, false));
            };
            let mut params = Vec::new();
            let sql = match_sql(&plan, None, &mut params);
            let n: i64 = c
                .prepare(&format!(
                    "SELECT count(*) FROM ({sql} LIMIT {})",
                    cap.saturating_add(1)
                ))?
                .query_row(params_from_iter(params), |r| r.get(0))?;
            let n = n as u64;
            Ok((n.min(cap as u64), n > cap as u64))
        })
    }
}

// ---------------------------------------------------------------------------
// hydration + snippets

fn hydrate(
    c: &Connection,
    groups: &[&Group],
    hl: &Highlighter,
    matches: &hybrid::Matches,
) -> Result<Vec<SearchHit>> {
    let mut stmt = c.prepare_cached(
        "SELECT m.account_id, m.id, m.subject, m.from_name, m.from_email, m.date, m.snippet, t.thread_id, t.labels, t.flags, b.body_text
         FROM messages m JOIN threads t ON t.rowid = m.thread_rowid JOIN message_bodies b ON b.rowid = m.rowid
         WHERE m.rowid = ?1",
    )?;
    let mut files = c.prepare_cached(
        "SELECT filename FROM attachments WHERE message_rowid = ?1 AND inline = 0 ORDER BY ord",
    )?;
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        let filenames: Vec<String> = files
            .query_map([g.best_rowid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut hit = stmt.query_row([g.best_rowid], |r| {
            let snippet: String = r.get(6)?;
            let body = r.get::<_, store::Body>(10)?.0;
            // Headers-only messages index the snippet as their body.
            let body = if body.is_empty() {
                snippet.clone()
            } else {
                body
            };
            let tflags: i64 = r.get(9)?;
            let subject: String = r.get(2)?;
            let by = matches.thread(g.thread_rowid);
            // The best passage by meaning, from this row when it is the
            // message shown.
            let passage = by
                .passage_from
                .filter(|p| p.0 == g.best_rowid)
                .and_then(|(_, chunk)| hybrid::passage(&subject, &body, chunk, hl));
            // Found only by meaning: the passage is the snippet, with any
            // typed words that happen to be in it marked.
            let snippet_html = match &passage {
                Some(p) if !by.words => hybrid::passage_html(p, hl),
                _ => hl.snippet(&body, &filenames, &snippet),
            };
            Ok(SearchHit {
                account_id: r.get(0)?,
                thread_id: r.get(7)?,
                message_id: r.get(1)?,
                subject,
                from: Address {
                    name: r.get(3)?,
                    email: r.get(4)?,
                },
                date: r.get(5)?,
                snippet_html,
                match_count: g.count,
                label_ids: store::split_labels(&r.get::<_, String>(8)?),
                has_attachments: tflags & F_ATTACH != 0,
                unread: tflags & F_UNREAD != 0,
                score: g.best_score as f32,
                matched_by: by.labels(),
                passage,
            })
        })?;
        if hit.passage.is_none() {
            if let Some((rowid, chunk)) = matches.thread(g.thread_rowid).passage_from {
                hit.passage = hybrid::passage_for(c, rowid, chunk, hl)?;
            }
        }
        out.push(hit);
    }
    Ok(out)
}

/// Builds `<mark>`-highlighted, HTML-escaped excerpts around query terms.
pub(crate) struct Highlighter {
    /// (folded token, prefix match)
    pub(crate) terms: Vec<(String, bool)>,
}

impl Highlighter {
    pub(crate) fn new(q: &ParsedQuery) -> Self {
        let mut terms = Vec::new();
        for (textv, _phrase, prefix) in q.text_terms() {
            let toks = text::tokens(textv);
            let n = toks.len();
            for (i, (_, _, t)) in toks.into_iter().enumerate() {
                let p = prefix && i + 1 == n && t.chars().count() >= MIN_PREFIX_CHARS;
                if !terms.iter().any(|(x, xp)| *x == t && *xp == p) {
                    terms.push((t, p));
                }
            }
        }
        // The other forms the query matched (invoice for "invoices").
        for alt in q.alternatives.values().flatten() {
            for (_, _, t) in text::tokens(alt) {
                if !terms.iter().any(|(x, xp)| *x == t && !*xp) {
                    terms.push((t, false));
                }
            }
        }
        Highlighter { terms }
    }

    pub(crate) fn term_of(&self, tok: &str) -> Option<usize> {
        self.terms.iter().position(|(t, p)| {
            if *p {
                tok.starts_with(t.as_str())
            } else {
                tok == t
            }
        })
    }

    /// Excerpt of the authored body, else the quoted part, else a matching
    /// attachment name, else the plain snippet; always HTML-safe.
    pub(crate) fn snippet(&self, body: &str, filenames: &[String], fallback: &str) -> String {
        if !self.terms.is_empty() {
            let (authored, quoted) = text::split_quoted(body);
            for part in [&authored, &quoted] {
                if let Some(s) = self.window(part) {
                    return s;
                }
            }
            for f in filenames {
                if let Some(s) = self.window(f) {
                    return format!("Attachment: {s}");
                }
            }
        }
        let src = if fallback.trim().is_empty() {
            text::strip_quoted(body)
        } else {
            fallback.to_string()
        };
        let mut out = String::new();
        let collapsed = collapse_ws(truncate_chars(&src, 200));
        text::escape_html(&collapsed, &mut out);
        out
    }

    fn window(&self, text_in: &str) -> Option<String> {
        let text_in = truncate_bytes(text_in, 256 * 1024);
        let toks = text::tokens(text_in);
        let hits: Vec<(usize, usize)> = toks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| self.term_of(&t.2).map(|k| (i, k)))
            .collect();
        if hits.is_empty() {
            return None;
        }
        // Window start (a hit) that covers the most distinct terms.
        let mut best = (0usize, hits[0].0);
        for (hi, &(start, _)) in hits.iter().enumerate() {
            let mut seen = 0u64;
            for &(ti, k) in &hits[hi..] {
                if ti >= start + SNIPPET_TOKENS - 8 {
                    break;
                }
                seen |= 1 << (k.min(63));
            }
            let n = seen.count_ones() as usize;
            if n > best.0 {
                best = (n, start);
            }
            if n == self.terms.len() {
                break;
            }
        }
        let first = best.1.saturating_sub(5);
        let last = (first + SNIPPET_TOKENS).min(toks.len());
        let mut out = String::with_capacity(400);
        if first > 0 {
            out.push('\u{2026}');
        }
        let mut cursor = toks[first].0;
        for t in &toks[first..last] {
            text::escape_html(&collapse_ws(&text_in[cursor..t.0]), &mut out);
            let word = &text_in[t.0..t.1];
            if self.term_of(&t.2).is_some() {
                out.push_str("<mark>");
                text::escape_html(word, &mut out);
                out.push_str("</mark>");
            } else {
                text::escape_html(word, &mut out);
            }
            cursor = t.1;
        }
        if last < toks.len() {
            // Trailing punctuation up to the next token, then an ellipsis.
            text::escape_html(
                collapse_ws(&text_in[cursor..toks[last].0]).trim_end(),
                &mut out,
            );
            out.push('\u{2026}');
        } else {
            text::escape_html(collapse_ws(&text_in[cursor..]).trim_end(), &mut out);
        }
        Some(out)
    }
}

fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn truncate_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            ws = true;
        } else {
            if ws {
                out.push(' ');
            }
            ws = false;
            out.push(c);
        }
    }
    if ws {
        out.push(' ');
    }
    out
}

// ---------------------------------------------------------------------------
// attachments panel

fn attachment_panel(
    c: &Connection,
    q: &ParsedQuery,
    plan: &Plan,
    top: &[&Group],
) -> Result<Vec<AttachmentHit>> {
    let kinds_ok = |kind: i64| {
        plan.has_kinds.is_empty() || plan.has_kinds.iter().any(|&k| k == 0 || k == kind)
    };
    let mut out: Vec<AttachmentHit> = Vec::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    let select = "SELECT a.rowid, a.message_rowid, a.att_id, a.filename, a.mime_type, a.size, a.content_id, a.inline, a.kind,
                         m.account_id, m.id, t.thread_id, m.from_name, m.from_email, m.date";
    let mut push_row = |r: &rusqlite::Row, out: &mut Vec<AttachmentHit>| -> rusqlite::Result<()> {
        let key = (r.get::<_, i64>(1)?, r.get::<_, i64>(0)?);
        if !kinds_ok(r.get(8)?) || !seen.insert(key) || out.len() >= MAX_ATTACHMENTS {
            return Ok(());
        }
        out.push(AttachmentHit {
            account_id: r.get(9)?,
            thread_id: r.get(11)?,
            message_id: r.get(10)?,
            attachment: AttachmentMeta {
                id: r.get(2)?,
                filename: r.get(3)?,
                mime_type: r.get(4)?,
                size: r.get::<_, i64>(5)? as u64,
                content_id: r.get(6)?,
                inline: r.get(7)?,
            },
            from: Address {
                name: r.get(12)?,
                email: r.get(13)?,
            },
            date: r.get(14)?,
        });
        Ok(())
    };

    // 1) Filenames containing every typed word (trigram substring match).
    let words: Vec<String> = q
        .text_terms()
        .map(|(t, _, _)| t.trim().to_string())
        .filter(|t| t.chars().filter(|c| c.is_alphanumeric()).count() >= 3)
        .collect();
    if !words.is_empty() && plan.fts.is_some() {
        let expr = words
            .iter()
            .map(|w| fts_string(w, false))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut params = vec![Value::Text(expr)];
        // Attachment rowids are date-ordered like message rowids.
        let mut range = String::new();
        if let Some(lo) = plan.lo {
            range.push_str(" AND attachments_fts.rowid >= ?");
            params.push(Value::Integer(lo.saturating_mul(ATTACHMENT_SLOTS)));
        }
        if let Some(hi) = plan.hi {
            range.push_str(" AND attachments_fts.rowid < ?");
            params.push(Value::Integer(hi.saturating_mul(ATTACHMENT_SLOTS)));
        }
        if !plan.has_kinds.is_empty() && !plan.has_kinds.contains(&0) {
            range.push_str(&format!(
                " AND a.kind IN ({})",
                plan.has_kinds
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        // Set tests on the attachment's message rowid, before the message
        // and thread rows are looked up (see FTS_ROWID).
        let att_rowid = if plain_plans() {
            "m.rowid"
        } else {
            "+a.message_rowid"
        };
        let prefilter = prefilter_sql(plan, att_rowid);
        let preds = preds_sql(plan, &mut params, att_rowid);
        let field = match &plan.field_fts {
            Some(f) => {
                params.push(Value::Text(f.clone()));
                " AND m.rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)"
            }
            None => "",
        };
        let sql = format!(
            "{select} FROM attachments_fts CROSS JOIN attachments a ON a.rowid = attachments_fts.rowid
             CROSS JOIN messages m ON m.rowid = a.message_rowid CROSS JOIN threads t ON t.rowid = m.thread_rowid
             WHERE attachments_fts MATCH ?{range}{prefilter}{preds}{field}
             ORDER BY attachments_fts.rowid DESC LIMIT {MAX_ATTACHMENTS}"
        );
        let mut stmt = c.prepare_cached(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        while let Some(r) = rows.next()? {
            push_row(r, &mut out)?;
        }
    }

    // 2) Files on the top hits, in rank order.
    let mut stmt = c.prepare_cached(&format!(
        "{select} FROM attachments a JOIN messages m ON m.rowid = a.message_rowid JOIN threads t ON t.rowid = m.thread_rowid
         WHERE a.message_rowid = ?1 AND a.inline = 0 ORDER BY a.ord"
    ))?;
    for g in top {
        if out.len() >= MAX_ATTACHMENTS {
            break;
        }
        let mut rows = stmt.query([g.best_rowid])?;
        while let Some(r) = rows.next()? {
            push_row(r, &mut out)?;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// people

/// People whose name/email contains every typed word (prefix match on the
/// last one), for the People group and from: suggestions.
fn people_panel(c: &Connection, q: &ParsedQuery, own: &HashSet<String>) -> Result<Vec<PersonHit>> {
    let mut words: Vec<(String, bool)> = Vec::new();
    for t in q
        .clauses
        .iter()
        .flat_map(|c| c.terms.iter())
        .filter(|t| !t.negated)
    {
        let (s, prefix) = match &t.atom {
            Atom::Text {
                text,
                phrase: false,
                prefix,
            } => (text.as_str(), *prefix),
            Atom::Field {
                field: Field::From | Field::To | Field::Cc | Field::Participant,
                value,
                prefix,
            } => (value.as_str(), *prefix),
            _ => continue,
        };
        let toks = text::tokens(s);
        let n = toks.len();
        for (i, (_, _, tok)) in toks.into_iter().enumerate() {
            words.push((tok, prefix && i + 1 == n));
        }
    }
    if words.is_empty() || words.iter().map(|w| w.0.len()).sum::<usize>() < 2 {
        return Ok(Vec::new());
    }
    // Every word must match; words that match nobody (e.g. "invoice") are
    // dropped so "mike invoice" still surfaces Mike.
    let mut matching: Vec<(String, bool)> = Vec::new();
    for w in &words {
        let n: i64 = c
            .prepare_cached(
                "SELECT count(*) FROM (SELECT 1 FROM people_words WHERE word BETWEEN ?1 AND ?2 LIMIT 1)",
            )?
            .query_row(params_from_iter(word_range(&w.0, w.1)), |r| r.get(0))?;
        if n > 0 {
            matching.push(w.clone());
        }
    }
    if matching.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = String::from("SELECT email, name, from_count + sent_to_count FROM people WHERE from_count + sent_to_count > 0");
    let mut params = Vec::new();
    for (w, p) in &matching {
        sql.push_str(&format!(" AND {}", has_word(params.len() + 1)));
        params.extend(word_range(w, *p));
    }
    sql.push_str(" ORDER BY sent_to_count * 3 + from_count DESC LIMIT 20");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |r| {
        Ok(PersonHit {
            address: Address {
                email: r.get(0)?,
                name: r.get(1)?,
            },
            message_count: r.get::<_, i64>(2)? as u32,
        })
    })?;
    let mut out = Vec::new();
    for p in rows {
        let p = p?;
        if own.contains(&p.address.email) {
            continue;
        }
        out.push(p);
        if out.len() >= MAX_PEOPLE {
            break;
        }
    }
    Ok(out)
}

/// Senders whose name matches a bare typed word ("mike"): their mail gets a
/// ranking boost. This never filters; only a typed from: filters.
fn person_boost(c: &Connection, q: &ParsedQuery, own: &HashSet<String>) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    let mut stmt = c.prepare_cached(&format!(
        "SELECT email FROM people WHERE from_count + sent_to_count > 0 AND {}
         ORDER BY sent_to_count * 3 + from_count DESC LIMIT 10",
        has_word(1)
    ))?;
    for (textv, phrase, prefix) in q.text_terms() {
        if phrase {
            continue;
        }
        let toks = text::tokens(textv);
        if toks.len() != 1 || toks[0].2.chars().count() < 2 {
            continue;
        }
        let rows = stmt.query_map(params_from_iter(word_range(&toks[0].2, prefix)), |r| {
            r.get::<_, String>(0)
        })?;
        for e in rows {
            let e = e?;
            if !own.contains(&e) {
                out.insert(e);
            }
        }
    }
    Ok(out)
}

/// "from:mike" → chip "From Mike Delgado" when one known sender clearly fits.
/// The filter itself stays a token match on the sender field.
/// `label:tax docs 2025` typed without quotes: when the first word names no
/// label but it and the words after it name one ("Tax Docs 2025"), they are
/// the label, not free text (the chip then covers all of them).
fn merge_label_words(c: &Connection, q: &mut ParsedQuery, raw: &str) -> Result<()> {
    let is_label = |cl: &query::Clause| {
        matches!(cl.terms.as_slice(), [query::Term { negated: false, atom: Atom::Label(_) }])
    };
    if !q.clauses.iter().any(is_label) {
        return Ok(());
    }
    let labels: Vec<(String, String, String)> = c
        .prepare_cached("SELECT account_id, id, name FROM labels")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut i = 0;
    while i < q.clauses.len() {
        let value = match q.clauses[i].terms.as_slice() {
            [query::Term { negated: false, atom: Atom::Label(v) }] => v.clone(),
            _ => {
                i += 1;
                continue;
            }
        };
        if !resolve_label(&labels, &value, false).is_empty() {
            i += 1;
            continue;
        }
        let words: Vec<String> = q.clauses[i + 1..]
            .iter()
            .take(4)
            .map_while(|cl| match cl.terms.as_slice() {
                [query::Term { negated: false, atom: Atom::Text { text, phrase: false, .. } }] => Some(text.clone()),
                _ => None,
            })
            .collect();
        let found = (1..=words.len()).rev().find(|&k| {
            let name = format!("{value} {}", words[..k].join(" "));
            !resolve_label(&labels, &name, false).is_empty()
        });
        if let Some(k) = found {
            let name = format!("{value} {}", words[..k].join(" "));
            q.clauses[i].terms[0].atom = Atom::Label(name.clone());
            q.clauses.drain(i + 1..=i + k);
            // Stretch the chip over the words, when they follow it as typed.
            if let Some(chip) = q
                .chips
                .iter_mut()
                .find(|ch| ch.kind == "label" && ch.raw.split_once(':').is_some_and(|(_, v)| v == value))
            {
                if let Some(start) = raw.find(&chip.raw) {
                    let mut end = start + chip.raw.len();
                    for w in &words[..k] {
                        match raw[end..].find(w.as_str()) {
                            Some(p) if raw[end..end + p].trim().is_empty() => end += p + w.len(),
                            _ => break,
                        }
                    }
                    chip.raw = raw[start..end].to_string();
                }
                chip.label = format!("Label {name}");
            }
        }
        i += 1;
    }
    Ok(())
}

fn upgrade_from_chips(c: &Connection, q: &mut ParsedQuery) -> Result<()> {
    for chip in q.chips.iter_mut().filter(|ch| ch.kind == "from") {
        let value = chip
            .raw
            .split_once(':')
            .map_or("", |(_, v)| v)
            .trim_matches('"');
        if value.contains('@') {
            continue;
        }
        let toks = text::tokens(value);
        if toks.is_empty() {
            continue;
        }
        let mut sql =
            String::from("SELECT name, email, from_count FROM people WHERE from_count > 0");
        let mut params = Vec::new();
        for (_, _, t) in &toks {
            sql.push_str(&format!(" AND {}", has_word(params.len() + 1)));
            params.extend(word_range(t, true));
        }
        sql.push_str(" ORDER BY from_count DESC LIMIT 2");
        let rows: Vec<(Option<String>, String, i64)> = c
            .prepare(&sql)?
            .query_map(params_from_iter(params), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let pick = match rows.as_slice() {
            [only] => Some(only),
            [a, b] if a.2 >= 3 * b.2 => Some(a),
            _ => None,
        };
        if let Some((name, email, _)) = pick {
            chip.label = format!(
                "From {}",
                name.as_deref().filter(|n| !n.is_empty()).unwrap_or(email)
            );
        }
    }
    Ok(())
}

fn sent_to_map(store: &Store, c: &Connection) -> Result<Arc<HashMap<String, u32>>> {
    let gen = store
        .inner
        .generation
        .load(std::sync::atomic::Ordering::Acquire);
    {
        let cache = store
            .inner
            .sent_to_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((g, m)) = cache.as_ref() {
            if *g == gen {
                return Ok(m.clone());
            }
        }
    }
    let map: HashMap<String, u32> = c
        .prepare_cached("SELECT email, sent_to_count FROM people WHERE sent_to_count > 0")?
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let map = Arc::new(map);
    *store
        .inner
        .sent_to_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some((gen, map.clone()));
    Ok(map)
}
