//! "Ask your inbox" without a language model (design and sources:
//! docs/ASK.md). A question is matched against a fixed grammar in English
//! and Spanish (intent.rs), people and companies are resolved against the
//! local people table (resolve.rs), and the answer is computed one of two
//! ways:
//!
//! - **Exact**: SQL over the index and over facts extracted from mail at
//!   index time (`crate::structured`: flights, stays, orders, parcels,
//!   bills, reservations; facts.rs), with amounts and dates read by fixed
//!   rules (extract.rs). Counts and sums cover every matching message and
//!   show their parts.
//! - **Passages**: for topic questions, hybrid retrieval (FTS5 plus the
//!   vector index when the app provides one, fused by reciprocal rank),
//!   then the best sentences, scored by term overlap, meaning and answer
//!   type, quoted with who said them (passages.rs).
//!
//! Every answer cites the messages it came from and lists the steps it
//! took; when nothing matches it says so and offers the search it ran. It
//! never guesses a number.
//! Mirrored in `apps/desktop/src/lib/types.ts` (AskAnswer and friends).

pub(crate) mod extract;
mod intent;
mod qparse;
pub mod query;
mod resolve;
mod run;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_facts;

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::structured::{Extracted, Source};
use crate::types::{AccountId, Address};
use crate::{Result, Store};

pub use penguin_semantic::{Embedder, VectorIndex};

/// The on-device embedding model and chunk index, when the app has them.
/// Ask works without (keyword retrieval only); with them, topic questions
/// also find mail by meaning and sentences are ranked by meaning too.
#[derive(Clone, Copy)]
pub struct AskSemantic<'a> {
    pub embedder: &'a dyn Embedder,
    pub index: &'a dyn VectorIndex,
}

pub use intent::looks_like_question;
pub use query::{
    AskGroup, AskQuery, AskResult, AskResultKind, AskUnderstood, QueryDirection, QueryDraft,
    QueryField, QueryGroup, QueryMeasure, QueryOp, QuerySource, QuerySubject, QueryTense,
};
pub(crate) use run::automated;

/// What to answer over, and conversational context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AskScope {
    /// Limit to these accounts (a profile). None = every account.
    pub account_ids: Option<Vec<AccountId>>,
    /// Addresses of the person the previous answer was about, so "he",
    /// "she" or "they" in a follow-up question resolve to them.
    pub person: Option<Vec<String>>,
    /// Extra names for groups of your accounts (profile names). A question
    /// naming one ("when did we start with Acme") means the companies at
    /// those accounts' domains. Account nicknames are added automatically.
    pub aliases: Vec<AskAlias>,
    /// Show verification codes in answers ("what's my Rydeo code"). The app
    /// sets it; the CLI and MCP never do (codes stay out of their output).
    pub reveal_codes: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskAlias {
    pub name: String,
    pub account_ids: Vec<AccountId>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AskIntent {
    LastContact,
    FirstContact,
    Relationship,
    LatestItem,
    Count,
    Spend,
    WhoAbout,
    TopSenders,
    WhoIs,
    WaitingOn,
    OweReplies,
    When,
    /// "My flight to Lisbon", "when do I fly next".
    Flight,
    /// "Where am I staying in Porto".
    Stay,
    /// "Where's my package", "tracking for my Paperleaf order".
    Package,
    /// "What did I order from Paperleaf".
    Orders,
    /// "When is the Brightwave bill due", "upcoming bills".
    Bills,
    /// "When is my dinner reservation", "my tickets for the Lanterns".
    Booking,
    /// "Latest verification code from Rydeo".
    Code,
    /// "What's Priya's phone number", "Priya's email", "how do I reach Dana".
    ContactInfo,
    /// "What subscriptions do I pay for": recurring charges.
    Subscriptions,
    /// "What did Priya say about pricing".
    Said,
    /// "Did Priya reply about the contract".
    DidReply,
    /// "Find the lease from Dana".
    Find,
    /// A topic question answered with quoted sentences.
    Passage,
    /// Not a question the grammar knows; the answer is a plain search.
    Unknown,
    /// A question read as a structured query (counts, sums, averages,
    /// extremes, lists, groups, comparisons over facts or mail).
    Query,
}

/// How sure the answer is. `none` = nothing found (the headline says so).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AskConfidence {
    High,
    Medium,
    Low,
    None,
}

/// A message an answer relies on (open the thread at this message).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AskCite {
    pub account_id: AccountId,
    pub thread_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskFact {
    pub label: String,
    pub value: String,
    /// Unix ms when the fact is a date (the UI may show it relatively).
    pub date: Option<i64>,
    pub cite: Option<AskCite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskAmount {
    pub value: f64,
    /// ISO 4217 ("USD").
    pub currency: String,
    /// The line of the message the amount came from ("Total $23.40").
    pub source: String,
}

/// A cited message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskItem {
    pub account_id: AccountId,
    pub thread_id: String,
    pub message_id: String,
    pub subject: String,
    pub from: Address,
    /// Unix ms.
    pub date: i64,
    /// Plain text (no HTML).
    pub snippet: String,
    /// Why it's here ("Handoff", "Waiting 5 days", a quoted sentence).
    pub note: Option<String>,
    pub amount: Option<AskAmount>,
    /// You sent it.
    pub sent: bool,
}

/// Activity per period, for the sparkline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskBucket {
    /// Unix ms of the period start (local midnight).
    pub start: i64,
    /// "May 2025" or "2025".
    pub label: String,
    pub from_them: u32,
    pub from_me: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskMarker {
    pub date: i64,
    /// "First contact", "Regular from", "Handoff", "Last contact".
    pub label: String,
    pub cite: Option<AskCite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskTimeline {
    /// "month" | "year"
    pub unit: String,
    pub buckets: Vec<AskBucket>,
    pub markers: Vec<AskMarker>,
}

/// Who the answer is about (for the person card and follow-up pronouns).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskPerson {
    pub label: String,
    pub name: Option<String>,
    /// Lowercased, most mail first.
    pub emails: Vec<String>,
    /// For a company: its domain.
    pub domain: Option<String>,
    pub company: bool,
}

/// A clickable question: a follow-up, or a "did you mean" choice.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskSuggestion {
    pub label: String,
    pub question: String,
}

/// A structured fact shown as a card (flight, stay, order or parcel, bill,
/// reservation, contact details).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskCard {
    pub fact: Extracted,
    pub cite: AskCite,
    pub from: Address,
    pub subject: String,
    /// Unix ms of the email it came from.
    pub date: i64,
    pub source: Source,
    /// Other emails about the same booking, order or parcel (updates,
    /// reminders), newest first.
    pub related: Vec<AskCite>,
    /// "Tomorrow", "In 12 days", "Delivered", "Due in 3 days", "Paid".
    pub status: Option<String>,
}

/// A sentence quoted from an email as (part of) the answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskPassage {
    /// Plain text, one or two sentences.
    pub text: String,
    /// Ranges in `text` of the question's words, in UTF-16 code units (JS
    /// string offsets), for highlighting.
    pub marks: Vec<[u32; 2]>,
    pub cite: AskCite,
    pub from: Address,
    pub subject: String,
    /// Unix ms.
    pub date: i64,
    /// You wrote it.
    pub sent: bool,
    /// 0–1: how well it answers (words, meaning, answer type).
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskTotal {
    pub value: f64,
    /// ISO 4217.
    pub currency: String,
    /// Emails added into this total.
    pub count: usize,
}

/// The math behind a sum: what was added, and what was left out and why.
/// The emails added are the answer's `items` (each with its amount).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskSum {
    /// One per currency, most emails first (never converted).
    pub totals: Vec<AskTotal>,
    /// What each amount is ("order totals from receipts").
    pub basis: String,
    /// Emails about an order already counted (confirmation + shipping
    /// notice of one order count once).
    pub duplicates: usize,
    /// Emails from them without an amount (promotions, notices).
    pub skipped: usize,
    /// Invoices still due (not money spent yet).
    pub unpaid: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskAnswer {
    pub intent: AskIntent,
    pub question: String,
    /// One sentence: "You last emailed Mike Kestrel on Sep 24, 2026".
    pub headline: String,
    /// A second line when useful (a quoted sentence, a caveat).
    pub detail: Option<String>,
    pub facts: Vec<AskFact>,
    pub timeline: Option<AskTimeline>,
    pub items: Vec<AskItem>,
    pub person: Option<AskPerson>,
    /// Other people/companies the question could mean.
    pub candidates: Vec<AskSuggestion>,
    pub confidence: AskConfidence,
    /// How the answer was computed: resolution and every query run.
    pub steps: Vec<String>,
    /// A Penguin search that shows the underlying mail ("Show in search").
    pub search_query: Option<String>,
    pub followups: Vec<AskSuggestion>,
    /// Structured cards (flights, stays, orders, parcels, bills, bookings).
    pub cards: Vec<AskCard>,
    /// Quoted sentences answering a topic question, best first.
    pub passages: Vec<AskPassage>,
    /// The math behind a sum.
    pub sum: Option<AskSum>,
    /// A caveat about what the answer could see ("Still reading 1,204
    /// emails for bookings and receipts").
    pub coverage: Option<String>,
    /// How the question was read as a query, when it was (the chips).
    pub understood: Option<AskUnderstood>,
    /// Grouped counts or sums, or the sides of a comparison.
    pub groups: Vec<AskGroup>,
    /// The answer's value, machine-readable.
    pub result: Option<AskResult>,
    pub took_ms: f64,
}

impl Store {
    /// Answer `question` from the local index. `now_ms` and
    /// `utc_offset_secs` fix "today" and how dates are shown. Read-only.
    pub fn ask(
        &self,
        question: &str,
        scope: &AskScope,
        now_ms: i64,
        utc_offset_secs: i32,
    ) -> Result<AskAnswer> {
        self.ask_with(question, scope, now_ms, utc_offset_secs, None)
    }

    /// [`Store::ask`] with the embedding model and vector index, so topic
    /// questions also retrieve and rank by meaning.
    pub fn ask_with(
        &self,
        question: &str,
        scope: &AskScope,
        now_ms: i64,
        utc_offset_secs: i32,
        semantic: Option<AskSemantic<'_>>,
    ) -> Result<AskAnswer> {
        let started = Instant::now();
        let mut answer = run::answer(self, question, scope, now_ms, utc_offset_secs, semantic)?;
        answer.took_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(answer)
    }

    /// Answer a query that the model or the user wrote (the chips). It is
    /// checked first (`query::check`); a query that doesn't fit gets an
    /// answer saying why. `question` is the text shown with the answer.
    pub fn ask_query(
        &self,
        question: &str,
        query: &AskQuery,
        source: QuerySource,
        scope: &AskScope,
        now_ms: i64,
        utc_offset_secs: i32,
    ) -> Result<AskAnswer> {
        let started = Instant::now();
        let mut answer =
            run::answer_query(self, question, query, source, scope, now_ms, utc_offset_secs)?;
        answer.took_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(answer)
    }

    /// The on-device model's reading of a question (a [`QueryDraft`]):
    /// validated against the schema and the date grammar, then answered
    /// exactly. None when the draft doesn't validate, or names something the
    /// mailbox doesn't have: the grammar's answer stands.
    pub fn ask_draft(
        &self,
        question: &str,
        draft: QueryDraft,
        scope: &AskScope,
        now_ms: i64,
        utc_offset_secs: i32,
    ) -> Result<Option<AskAnswer>> {
        let today = run::today_of(now_ms, utc_offset_secs);
        let Ok(query) = draft.into_query(today) else {
            return Ok(None);
        };
        let a = self.ask_query(question, &query, QuerySource::Model, scope, now_ms, utc_offset_secs)?;
        Ok(a.result.is_some().then_some(a))
    }
}

/// How the grammar reads `question` as a query, if it does (for tests,
/// the eval and the UI's "understood as").
pub fn parse_query(question: &str, now_ms: i64, utc_offset_secs: i32) -> Option<AskQuery> {
    qparse::parse(question, run::today_of(now_ms, utc_offset_secs))
        .filter(|p| p.unread.is_empty())
        .map(|p| p.query)
}

/// Example questions, for empty states and "what can I ask?".
pub fn example_questions() -> Vec<&'static str> {
    vec![
        "When did I last email Priya?",
        "How long have I known Linden?",
        "Latest invoice from Linden",
        "How much did I spend on Uber this year?",
        "How many emails from Priya this year?",
        "Who emailed me about the lease?",
        "What am I waiting on?",
        "What do I owe replies to?",
        "When is my lease renewal?",
        "Who is Priya?",
        "When is my flight to Lisbon?",
        "Where's my Paperleaf order?",
        "What bills are due?",
        "What did Priya say about pricing?",
        "¿Cuánto gasté en Uber este año?",
    ]
}
