//! Versioned JSON shapes for `penguin-cli --json` and MCP tool results.
//! One schema, two transports. These are deliberately separate from the
//! IPC types in penguin-core so an internal change can't silently break a
//! script: changing a field here means bumping SCHEMA_VERSION and
//! regenerating docs/cli-schemas (see the snapshot test below).

use penguin_core::text::strip_quoted;
use penguin_core::{
    Account, Address, AttachmentMeta, Label, Message, PersonHit, SearchResponse, ThreadDetail,
    ThreadSummary,
};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Scope;

pub const SCHEMA_VERSION: u32 = 1;

/// Every payload: `{"schemaVersion": 1, "kind": "...", "data": {...}}`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct Envelope<T> {
    pub schema_version: u32,
    /// "search" | "thread" | "threads" | "labels" | "people" | "accounts" | "attachmentText" | "attachments" | "attachment" | "ask" | "draft" | "drafts" | "draftDeleted" | "sendQueued" | "shareLink" | "ruleMatch" | "error"
    pub kind: String,
    pub data: T,
}

pub fn envelope<T>(kind: &str, data: T) -> Envelope<T> {
    Envelope {
        schema_version: SCHEMA_VERSION,
        kind: kind.to_string(),
        data,
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
#[schemars(crate = "rmcp::schemars")]
pub struct AddressOut {
    pub name: Option<String>,
    pub email: String,
}

impl From<&Address> for AddressOut {
    fn from(a: &Address) -> Self {
        AddressOut {
            name: a.name.clone(),
            email: a.email.clone(),
        }
    }
}

fn addrs(v: &[Address]) -> Vec<AddressOut> {
    v.iter().map(AddressOut::from).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AttachmentOut {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

impl From<&AttachmentMeta> for AttachmentOut {
    fn from(a: &AttachmentMeta) -> Self {
        AttachmentOut {
            id: a.id.clone(),
            filename: a.filename.clone(),
            mime_type: a.mime_type.clone(),
            size: a.size,
        }
    }
}

/// What a request was limited to.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ScopeOut {
    pub account: Option<String>,
    pub profile: Option<String>,
    /// The profile's member accounts, when a profile was given.
    pub account_ids: Option<Vec<String>>,
}

impl From<&Scope> for ScopeOut {
    fn from(s: &Scope) -> Self {
        ScopeOut {
            account: s.account_id.clone(),
            profile: s.profile.clone(),
            account_ids: s.account_ids.clone(),
        }
    }
}

// ---------- search ----------

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchOut {
    pub query: String,
    pub scope: ScopeOut,
    /// Parsed operators, e.g. {kind:"from", label:"From Bo Park", raw:"from:bo"}.
    pub chips: Vec<ChipOut>,
    /// One per matching thread, best first.
    pub hits: Vec<HitOut>,
    pub attachments: Vec<AttachmentHitOut>,
    pub people: Vec<PersonOut>,
    pub took_ms: f64,
    /// Messages in the local index (all accounts).
    pub indexed_messages: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ChipOut {
    pub kind: String,
    pub label: String,
    pub raw: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct HitOut {
    pub account_id: String,
    pub thread_id: String,
    /// Best-matching message in the thread.
    pub message_id: String,
    pub subject: String,
    pub from: AddressOut,
    /// Unix ms.
    pub date: i64,
    /// RFC 3339, UTC.
    pub date_iso: String,
    /// Plain-text excerpt (no markup).
    pub snippet: String,
    pub match_count: u32,
    pub label_ids: Vec<String>,
    pub has_attachments: bool,
    pub unread: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AttachmentHitOut {
    pub account_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub attachment: AttachmentOut,
    pub from: AddressOut,
    pub date: i64,
    pub date_iso: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct PersonOut {
    pub name: Option<String>,
    pub email: String,
    pub message_count: u32,
}

impl From<&PersonHit> for PersonOut {
    fn from(p: &PersonHit) -> Self {
        PersonOut {
            name: p.address.name.clone(),
            email: p.address.email.clone(),
            message_count: p.message_count,
        }
    }
}

pub fn search_out(query: &str, scope: &Scope, r: &SearchResponse, took_ms: f64) -> SearchOut {
    SearchOut {
        query: query.to_string(),
        scope: scope.into(),
        chips: r
            .chips
            .iter()
            .map(|c| ChipOut {
                kind: c.kind.clone(),
                label: c.label.clone(),
                raw: c.raw.clone(),
            })
            .collect(),
        hits: r
            .hits
            .iter()
            .map(|h| HitOut {
                account_id: h.account_id.clone(),
                thread_id: h.thread_id.clone(),
                message_id: h.message_id.clone(),
                subject: h.subject.clone(),
                from: (&h.from).into(),
                date: h.date,
                date_iso: iso(h.date),
                snippet: plain_snippet(&h.snippet_html),
                match_count: h.match_count,
                label_ids: h.label_ids.clone(),
                has_attachments: h.has_attachments,
                unread: h.unread,
            })
            .collect(),
        attachments: r
            .attachments
            .iter()
            .map(|a| AttachmentHitOut {
                account_id: a.account_id.clone(),
                thread_id: a.thread_id.clone(),
                message_id: a.message_id.clone(),
                attachment: (&a.attachment).into(),
                from: (&a.from).into(),
                date: a.date,
                date_iso: iso(a.date),
            })
            .collect(),
        people: r.people.iter().map(PersonOut::from).collect(),
        took_ms,
        indexed_messages: r.indexed_messages,
    }
}

// ---------- threads ----------

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ThreadOut {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
    pub label_ids: Vec<String>,
    pub message_count: usize,
    /// Oldest first.
    pub messages: Vec<MessageOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct MessageOut {
    pub id: String,
    pub date: i64,
    pub date_iso: String,
    pub from: AddressOut,
    pub to: Vec<AddressOut>,
    pub cc: Vec<AddressOut>,
    pub bcc: Vec<AddressOut>,
    pub subject: String,
    pub label_ids: Vec<String>,
    pub unread: bool,
    pub starred: bool,
    pub attachments: Vec<AttachmentOut>,
    /// Pictures embedded in the body (cid:), apart from `attachments`;
    /// get_attachment returns them as images.
    pub inline_images: Vec<AttachmentOut>,
    /// Plain text the sender wrote, with quoted reply history removed.
    pub text: String,
    /// Characters of quoted history removed from `text`.
    pub quoted_chars: usize,
    /// The full plain-text body including quotes; only when requested.
    pub full_text: Option<String>,
    /// `text` (and `fullText`) were cut to the requested maximum.
    pub truncated: bool,
    /// Only headers are stored locally (outside the sync window); the body
    /// hasn't been downloaded, so `text` is empty. Opening it in Penguin
    /// fetches it.
    pub body_pending: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadOptions {
    pub include_full_text: bool,
    /// Per-message character cap for `text`/`fullText`; None = no cap.
    pub max_chars_per_message: Option<usize>,
}

/// Cut to at most `max` chars; returns (text, truncated).
pub fn cap(text: &str, max: Option<usize>) -> (String, bool) {
    match max {
        Some(n) if text.chars().count() > n => (text.chars().take(n).collect(), true),
        _ => (text.to_string(), false),
    }
}

pub fn message_out(m: &Message, opts: ThreadOptions, body_pending: bool) -> MessageOut {
    let authored = strip_quoted(&m.body_text);
    let quoted_chars = m
        .body_text
        .chars()
        .count()
        .saturating_sub(authored.chars().count());
    let (text, cut_text) = cap(authored.trim(), opts.max_chars_per_message);
    let (full_text, cut_full) = if opts.include_full_text {
        let (t, c) = cap(&m.body_text, opts.max_chars_per_message);
        (Some(t), c)
    } else {
        (None, false)
    };
    MessageOut {
        id: m.id.clone(),
        date: m.date,
        date_iso: iso(m.date),
        from: (&m.from).into(),
        to: addrs(&m.to),
        cc: addrs(&m.cc),
        bcc: addrs(&m.bcc),
        subject: m.subject.clone(),
        label_ids: m.label_ids.clone(),
        unread: m.is_unread(),
        starred: m.is_starred(),
        attachments: m
            .attachments
            .iter()
            .filter(|a| !a.inline)
            .map(AttachmentOut::from)
            .collect(),
        inline_images: m
            .attachments
            .iter()
            .filter(|a| a.inline)
            .map(AttachmentOut::from)
            .collect(),
        text,
        quoted_chars,
        full_text,
        truncated: cut_text || cut_full,
        body_pending,
    }
}

/// `pending`: ids of messages whose body isn't stored yet.
pub fn thread_out(t: &ThreadDetail, opts: ThreadOptions, pending: &[String]) -> ThreadOut {
    ThreadOut {
        account_id: t.account_id.clone(),
        thread_id: t.thread_id.clone(),
        subject: t.subject.clone(),
        label_ids: t.label_ids.clone(),
        message_count: t.messages.len(),
        messages: t
            .messages
            .iter()
            .map(|m| message_out(m, opts, pending.contains(&m.id)))
            .collect(),
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ThreadListOut {
    /// "inbox" | "starred" | "sent" | "drafts" | "done" | "trash" | "spam" | "all" | "label"
    pub view: String,
    pub label_id: Option<String>,
    pub scope: ScopeOut,
    pub threads: Vec<ThreadSummaryOut>,
    /// Pass as `before` to get the next page; null when this page wasn't full.
    pub next_before: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ThreadSummaryOut {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
    pub snippet: String,
    /// Distinct senders, latest first.
    pub participants: Vec<AddressOut>,
    pub message_count: u32,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
    pub label_ids: Vec<String>,
    pub last_date: i64,
    pub last_date_iso: String,
}

impl From<&ThreadSummary> for ThreadSummaryOut {
    fn from(t: &ThreadSummary) -> Self {
        ThreadSummaryOut {
            account_id: t.account_id.clone(),
            thread_id: t.thread_id.clone(),
            subject: t.subject.clone(),
            snippet: t.snippet.clone(),
            participants: addrs(&t.participants),
            message_count: t.message_count,
            unread: t.unread,
            starred: t.starred,
            has_attachments: t.has_attachments,
            label_ids: t.label_ids.clone(),
            last_date: t.last_date,
            last_date_iso: iso(t.last_date),
        }
    }
}

// ---------- labels, people, accounts, attachment text ----------

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct LabelsOut {
    pub scope: ScopeOut,
    pub labels: Vec<LabelOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct LabelOut {
    pub account_id: String,
    pub id: String,
    pub name: String,
    /// "system" | "user"
    pub kind: String,
    pub unread_count: Option<u32>,
}

impl From<&Label> for LabelOut {
    fn from(l: &Label) -> Self {
        LabelOut {
            account_id: l.account_id.clone(),
            id: l.id.clone(),
            name: l.name.clone(),
            kind: l.kind.clone(),
            unread_count: l.unread_count,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct PeopleOut {
    pub query: String,
    pub scope: ScopeOut,
    pub people: Vec<PersonOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AccountsOut {
    pub accounts: Vec<AccountOut>,
    pub profiles: Vec<ProfileOut>,
    pub indexed_messages: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AccountOut {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub nickname: Option<String>,
    pub indexed_messages: u64,
}

impl AccountOut {
    pub fn new(a: &Account, indexed_messages: u64) -> Self {
        AccountOut {
            id: a.id.clone(),
            email: a.email.clone(),
            display_name: a.display_name.clone(),
            nickname: a.nickname.clone(),
            indexed_messages,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ProfileOut {
    pub id: String,
    pub name: String,
    pub account_ids: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AttachmentTextOut {
    pub account_id: String,
    pub message_id: String,
    pub attachment: AttachmentOut,
    pub text: String,
    pub truncated: bool,
}

// ---------- ask ----------

/// `ask`: a deterministic answer from the local index (no model). `answer`
/// is penguin-core's AskAnswer as-is (camelCase: intent, headline, detail,
/// facts, timeline, items citing accountId/threadId/messageId, person,
/// candidates, confidence, steps, searchQuery, followups, tookMs). Its inner
/// shape follows the app's Ask feature and may gain fields within v1.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct AskOut {
    pub scope: ScopeOut,
    #[schemars(with = "serde_json::Value")]
    pub answer: penguin_core::ask::AskAnswer,
}

// ---------- helpers ----------

/// Search snippets arrive as escaped HTML with <mark> around matches.
pub fn plain_snippet(s: &str) -> String {
    s.replace("<mark>", "")
        .replace("</mark>", "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Unix ms → `YYYY-MM-DDTHH:MM:SSZ` (UTC), without a date crate.
pub fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil_from_days.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testkit;

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(iso(1_767_265_200_000), "2026-01-01T11:00:00Z");
    }

    #[test]
    fn snippets_lose_markup() {
        assert_eq!(
            plain_snippet("a <mark>walrus</mark> &amp; &lt;b&gt;"),
            "a walrus & <b>"
        );
    }

    /// The machine-readable schemas in docs/cli-schemas are the contract.
    /// A shape change fails here: bump SCHEMA_VERSION if it breaks consumers,
    /// then regenerate with `UPDATE_SCHEMAS=1 cargo test -p penguin-desktop schemas`.
    #[test]
    fn schemas_match_docs() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/cli-schemas");
        let schemas = [
            ("search", rmcp::schemars::schema_for!(Envelope<SearchOut>)),
            ("thread", rmcp::schemars::schema_for!(Envelope<ThreadOut>)),
            (
                "threads",
                rmcp::schemars::schema_for!(Envelope<ThreadListOut>),
            ),
            ("labels", rmcp::schemars::schema_for!(Envelope<LabelsOut>)),
            ("people", rmcp::schemars::schema_for!(Envelope<PeopleOut>)),
            (
                "accounts",
                rmcp::schemars::schema_for!(Envelope<AccountsOut>),
            ),
            (
                "attachmentText",
                rmcp::schemars::schema_for!(Envelope<AttachmentTextOut>),
            ),
            ("ask", rmcp::schemars::schema_for!(Envelope<AskOut>)),
            (
                "attachments",
                rmcp::schemars::schema_for!(Envelope<crate::agent::files::AttachmentsOut>),
            ),
            (
                "attachment",
                rmcp::schemars::schema_for!(Envelope<crate::agent::mcp::AttachmentPayloadOut>),
            ),
            (
                "draft",
                rmcp::schemars::schema_for!(Envelope<crate::agent::writes::DraftOut>),
            ),
            (
                "drafts",
                rmcp::schemars::schema_for!(Envelope<crate::agent::writes::DraftsOut>),
            ),
            (
                "draftDeleted",
                rmcp::schemars::schema_for!(Envelope<crate::agent::writes::DraftDeletedOut>),
            ),
            (
                "sendQueued",
                rmcp::schemars::schema_for!(Envelope<crate::agent::writes::SendQueuedOut>),
            ),
            (
                "shareLink",
                rmcp::schemars::schema_for!(Envelope<crate::agent::sharing::ShareLinkOut>),
            ),
        ];
        let update = std::env::var_os("UPDATE_SCHEMAS").is_some();
        for (kind, schema) in schemas {
            let path = dir.join(format!("{kind}.v{SCHEMA_VERSION}.schema.json"));
            let generated = serde_json::to_string_pretty(&schema).unwrap() + "\n";
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &generated).unwrap();
                continue;
            }
            let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|_| {
                panic!("missing {}; run with UPDATE_SCHEMAS=1", path.display())
            });
            assert_eq!(
                on_disk, generated,
                "{kind} output schema changed; see the test doc comment"
            );
        }
    }

    /// Sample output on the fixed fixture: field names and values scripts
    /// rely on.
    #[test]
    fn search_and_thread_output_snapshot() {
        let (ctx, root) = testkit::fixture("snapshot");
        let scope = ctx.scope(Some(testkit::ADA), None).unwrap();
        let r = ctx
            .store
            .search(&penguin_core::SearchRequest {
                query: "walrus".into(),
                account_id: scope.account_id.clone(),
                account_ids: None,
                limit: 10,
            })
            .unwrap();
        let out = serde_json::to_value(envelope("search", search_out("walrus", &scope, &r, 1.5)))
            .unwrap();
        assert_eq!(out["schemaVersion"], 1);
        assert_eq!(out["kind"], "search");
        assert_eq!(
            out["data"]["scope"],
            serde_json::json!({"account": testkit::ADA, "profile": null, "accountIds": null})
        );
        let hit = &out["data"]["hits"][0];
        assert_eq!(hit["threadId"], "t1");
        assert_eq!(hit["accountId"], testkit::ADA);
        assert!(hit["dateIso"].as_str().unwrap().starts_with("2026-01-01T1"));
        assert!(!hit["snippet"].as_str().unwrap().contains("<mark>"));
        let mut keys: Vec<&str> = hit
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "accountId",
                "date",
                "dateIso",
                "from",
                "hasAttachments",
                "labelIds",
                "matchCount",
                "messageId",
                "snippet",
                "subject",
                "threadId",
                "unread"
            ]
        );

        let t = ctx.store.get_thread(testkit::ADA, "t1").unwrap().unwrap();
        let out = serde_json::to_value(envelope(
            "thread",
            thread_out(
                &t,
                ThreadOptions {
                    include_full_text: false,
                    max_chars_per_message: None,
                },
                &["m1".to_string()],
            ),
        ))
        .unwrap();
        let reply = &out["data"]["messages"][1];
        assert_eq!(out["data"]["messageCount"], 2);
        assert_eq!(reply["text"], "Looks good, ship it.");
        assert_eq!(reply["bodyPending"], false);
        assert_eq!(out["data"]["messages"][0]["bodyPending"], true);
        assert!(reply["quotedChars"].as_u64().unwrap() > 0);
        assert_eq!(reply["fullText"], serde_json::Value::Null);
        assert_eq!(reply["dateIso"], "2026-01-01T11:00:00Z");
        let _ = std::fs::remove_dir_all(root);
    }
}
