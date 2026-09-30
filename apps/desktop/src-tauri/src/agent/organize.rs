//! Organizing mail for agents: archive, read and unread, star, labels,
//! snooze, Reply Later, Trash and spam, on conversations an agent names by
//! account and thread id (as `search` and `list_threads` return them).
//!
//! - **The app's own path.** Every tool runs the same optimistic action the
//!   UI runs (crate::actions: `apply`, `snooze`, `reply_later`), so lists,
//!   counts and the open thread update at once, and the provider is asked
//!   with the app's credentials. The agent's call waits for the provider:
//!   a thread it refused is put back and reported under `failed`.
//! - **Level:** "Read, organize and draft" (stored as `draft`) or higher
//!   (agent/permission.rs).
//! - **Nothing is ever deleted.** Trash is the most destructive tool, and
//!   providers empty their Trash on their own schedule (about 30 days on
//!   Gmail and Outlook). Every call answers what changed, what each thread
//!   looked like before, and the calls that undo it; the app shows its
//!   usual toast with Undo, and Recent agent activity lists it.
//! - **Caps.** Prompt injection can tell an agent to hide mail ("trash every
//!   security alert"). `trash` and `report_spam` act on at most
//!   [`MAX_CAPPED_PER_CALL`] conversations per call and
//!   [`MAX_CAPPED_PER_HOUR`] per hour each (docs/SECURITY.md → Agents).
//! - **Audit:** tool, client, account (or how many), thread counts,
//!   outcome. Never subjects, addresses or label names.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use penguin_core::Store;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::audit::AppAuditEntry;
use super::output::iso;
use super::writes::WriteHost;
use crate::actions::{self, ActionHost, Applied, Outcome};
use crate::error::{CmdError, CmdResult};
use crate::state::blocking;
use crate::views::{ThreadAction, ThreadRef};

/// Every organizing tool (MCP names; the CLI uses them with dashes).
pub const ORGANIZE_TOOLS: [&str; 16] = [
    "archive",
    "unarchive",
    "mark_read",
    "mark_unread",
    "star",
    "unstar",
    "add_label",
    "remove_label",
    "snooze",
    "unsnooze",
    "reply_later",
    "clear_reply_later",
    "trash",
    "untrash",
    "report_spam",
    "not_spam",
];

/// Conversations one call may name.
pub const MAX_TARGETS: usize = 100;
/// The tools that hide mail the furthest (Trash, Spam) and so carry caps.
pub const CAPPED_TOOLS: [&str; 2] = ["trash", "report_spam"];
/// Conversations one `trash` or `report_spam` call may name: a page of
/// results (list_threads and search return 20 by default), not a mailbox.
pub const MAX_CAPPED_PER_CALL: usize = 25;
/// Conversations each capped tool may act on per rolling hour: a real
/// cleanup session (eight full calls), while a runaway or injected loop
/// stops at a number the user can review in Trash or Spam in minutes.
pub const MAX_CAPPED_PER_HOUR: usize = 200;
const HOUR_MS: i64 = 3_600_000;

/// Labels agents change only through their own tools (archive, star, …),
/// never with add_label / remove_label, so the caps can't be sidestepped.
const SYSTEM_LABELS: [&str; 9] = [
    "INBOX",
    "UNREAD",
    "STARRED",
    "TRASH",
    "SPAM",
    "SENT",
    "DRAFT",
    "IMPORTANT",
    "CHAT",
];

pub fn is_tool(tool: &str) -> bool {
    ORGANIZE_TOOLS.contains(&tool)
}

// ---------- arguments ----------

/// A conversation: its account (email) and thread id, as `search` and
/// `list_threads` return them.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ThreadTarget {
    pub account_id: String,
    pub thread_id: String,
}

impl From<&ThreadTarget> for ThreadRef {
    fn from(t: &ThreadTarget) -> ThreadRef {
        ThreadRef {
            account_id: t.account_id.clone(),
            thread_id: t.thread_id.clone(),
        }
    }
}

impl From<&ThreadRef> for ThreadTarget {
    fn from(t: &ThreadRef) -> ThreadTarget {
        ThreadTarget {
            account_id: t.account_id.clone(),
            thread_id: t.thread_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct TargetsArgs {
    /// The conversations (accountId + threadId from search or list_threads), 1 to 100.
    pub targets: Vec<ThreadTarget>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct LabelArgs {
    /// The conversations (accountId + threadId from search or list_threads), 1 to 100.
    pub targets: Vec<ThreadTarget>,
    /// A label's name or id from list_labels (a Gmail label, an IMAP folder,
    /// an Outlook category), looked up in each conversation's account. Only
    /// the user's own labels: Inbox, Trash, Spam, Starred and Unread have
    /// their own tools.
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
#[schemars(crate = "rmcp::schemars")]
pub enum Until {
    /// Unix milliseconds.
    Ms(i64),
    /// RFC 3339, e.g. "2026-10-01T09:00:00-04:00".
    Iso(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct SnoozeArgs {
    /// The conversations (accountId + threadId from search or list_threads), 1 to 100.
    pub targets: Vec<ThreadTarget>,
    /// When they come back to the inbox: RFC 3339 with an offset
    /// ("2026-10-01T09:00:00-04:00") or Unix milliseconds. In the future,
    /// within a year.
    pub until: Until,
}

impl Until {
    pub fn to_ms(&self) -> CmdResult<i64> {
        match self {
            Until::Ms(ms) => Ok(*ms),
            Until::Iso(s) => chrono::DateTime::parse_from_rfc3339(s.trim())
                .map(|d| d.timestamp_millis())
                .map_err(|_| {
                    CmdError::invalid(format!(
                        "until: {s:?} isn't a time; use RFC 3339 with an offset (2026-10-01T09:00:00-04:00) or Unix milliseconds"
                    ))
                }),
        }
    }
}

// ---------- results ----------

/// A conversation the call changed: the labels it added and removed
/// (label ids, as list_labels shows them).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ThreadChangeOut {
    pub account_id: String,
    pub thread_id: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// A conversation as it was before the call.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ThreadBeforeOut {
    pub account_id: String,
    pub thread_id: String,
    pub label_ids: Vec<String>,
    /// When it was snoozed until (Unix ms), if it was.
    pub snoozed_until: Option<i64>,
}

/// A provider refused the change for these conversations; Penguin put them
/// back as they were.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct FailedOut {
    pub account_id: String,
    pub thread_id: String,
    pub error: String,
}

/// The arguments of an undo call: the tool's own arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct UndoArgs {
    pub targets: Vec<ThreadTarget>,
    /// add_label / remove_label: the label id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// snooze: when (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
}

/// One call that undoes part of this one: `tool` with `arguments`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct UndoStep {
    pub tool: String,
    pub arguments: UndoArgs,
}

/// `organized`: what an organizing tool did, conversation by conversation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct OrganizedOut {
    pub tool: String,
    /// Changed, in Penguin and on the provider.
    pub changed: Vec<ThreadChangeOut>,
    /// Already that way: nothing to change.
    pub unchanged: Vec<ThreadTarget>,
    /// Not in Penguin's index (or an unknown account): not touched.
    pub not_found: Vec<ThreadTarget>,
    /// The provider refused: put back as they were.
    pub failed: Vec<FailedOut>,
    /// Every conversation found, as it was before this call.
    pub previous: Vec<ThreadBeforeOut>,
    /// Calls that put back what this one changed, in order.
    pub undo: Vec<UndoStep>,
    pub note: String,
}

/// `penguin://agent-organized`: an agent organized mail; the app shows a
/// toast with Undo (running `undo` through `agent_undo`). Mirrored in
/// types.ts (`AgentOrganized`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentOrganized {
    pub tool: String,
    /// `mcp` or `cli`.
    pub via: String,
    /// Conversations changed.
    pub count: usize,
    pub undo: Vec<UndoStep>,
}

// ---------- caps ----------

/// The rolling-hour counts of the capped tools (in memory: the app's).
#[derive(Default)]
pub struct Limits {
    used: Mutex<HashMap<String, VecDeque<(i64, usize)>>>,
}

impl Limits {
    /// Take `n` conversations of `tool`'s hourly allowance, or refuse.
    fn reserve(&self, tool: &str, n: usize, now: i64) -> CmdResult<()> {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        let q = used.entry(tool.to_string()).or_default();
        while q.front().is_some_and(|(t, _)| *t <= now - HOUR_MS) {
            q.pop_front();
        }
        let so_far: usize = q.iter().map(|(_, k)| k).sum();
        if so_far + n > MAX_CAPPED_PER_HOUR {
            let frees = q.front().map(|(t, _)| iso(t + HOUR_MS)).unwrap_or_default();
            return Err(CmdError::denied(format!(
                "Agents may {} at most {MAX_CAPPED_PER_HOUR} conversations an hour, and {so_far} were in the last hour \
                 (room for {} more; more room from {frees}). This limit protects the user from mail being hidden in bulk; \
                 ask the user to do the rest in Penguin.",
                verb(tool),
                MAX_CAPPED_PER_HOUR - so_far
            )));
        }
        q.push_back((now, n));
        Ok(())
    }

    /// Give back a reservation whose call failed.
    fn release(&self, tool: &str, n: usize, now: i64) {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(q) = used.get_mut(tool) {
            if let Some(i) = q.iter().rposition(|e| *e == (now, n)) {
                q.remove(i);
            }
        }
    }
}

fn verb(tool: &str) -> &'static str {
    match tool {
        "trash" => "move to Trash",
        "report_spam" => "report as spam",
        _ => "change",
    }
}

// ---------- what each tool is ----------

/// A tool as an action on one account's conversations. `label_id` is the
/// resolved label for add_label / remove_label and the account's Reply
/// Later label for reply_later / clear_reply_later.
pub fn thread_action(tool: &str, label_id: Option<&str>) -> Option<ThreadAction> {
    let label = || label_id.map(str::to_string);
    Some(match tool {
        "archive" | "snooze" => ThreadAction::Archive,
        "unarchive" | "unsnooze" => ThreadAction::MoveToInbox,
        "mark_read" => ThreadAction::MarkRead,
        "mark_unread" => ThreadAction::MarkUnread,
        "star" => ThreadAction::Star,
        "unstar" => ThreadAction::Unstar,
        "add_label" => ThreadAction::AddLabel { label_id: label()? },
        "remove_label" | "clear_reply_later" => ThreadAction::RemoveLabel { label_id: label()? },
        "reply_later" => ThreadAction::ReplyLater { label_id: label()? },
        "trash" => ThreadAction::Trash,
        "untrash" => ThreadAction::Untrash,
        "report_spam" => ThreadAction::ReportSpam,
        "not_spam" => ThreadAction::NotSpam,
        _ => return None,
    })
}

/// The label `wanted` (id or name, any case) in `account`: one of the
/// user's own labels, folders or categories.
pub fn resolve_label(store: &Store, account: &str, wanted: &str) -> CmdResult<String> {
    let w = wanted.trim();
    if w.is_empty() {
        return Err(CmdError::invalid("label is empty"));
    }
    let system = |id: &str| {
        let up = id.to_ascii_uppercase();
        SYSTEM_LABELS.contains(&up.as_str()) || up.starts_with("CATEGORY_")
    };
    let refuse = |name: &str| {
        CmdError::invalid(format!(
            "{name} is a system label or folder; use archive, unarchive, mark_read, mark_unread, star, unstar, \
             trash, untrash, report_spam or not_spam instead"
        ))
    };
    if system(w) {
        return Err(refuse(w));
    }
    let labels = store.list_labels(Some(account))?;
    let found = labels
        .iter()
        .find(|l| l.id == w)
        .or_else(|| labels.iter().find(|l| l.name == w))
        .or_else(|| labels.iter().find(|l| l.id.eq_ignore_ascii_case(w)))
        .or_else(|| labels.iter().find(|l| l.name.eq_ignore_ascii_case(w)));
    match found {
        Some(l) if l.kind != "user" || system(&l.id) => Err(refuse(&l.name)),
        Some(l) => Ok(l.id.clone()),
        None => Err(CmdError::not_found(format!(
            "no label named {w:?} in {account}; list_labels shows its labels, folders and categories"
        ))),
    }
}

fn group(targets: &[ThreadRef]) -> BTreeMap<String, Vec<ThreadRef>> {
    let mut out: BTreeMap<String, Vec<ThreadRef>> = BTreeMap::new();
    for t in targets {
        out.entry(t.account_id.clone()).or_default().push(t.clone());
    }
    out
}

/// Run `tool` on `targets` through the app's action path. Used by agents
/// (after the gate and the caps) and by the toast's Undo (`agent_undo`).
/// `unsnooze` acts only on the conversations that are snoozed.
pub async fn execute<H: ActionHost>(
    host: Arc<H>,
    tool: &str,
    targets: Vec<ThreadRef>,
    label: Option<&str>,
    until: Option<i64>,
    now: i64,
) -> CmdResult<Vec<Outcome>> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    match tool {
        "snooze" => {
            let until = until.ok_or_else(|| CmdError::invalid("snooze needs until"))?;
            Ok(vec![actions::snooze(host, targets, until, now).await?])
        }
        "unsnooze" => {
            let store = host.store().clone();
            let snoozed = blocking(move || {
                let mut out = Vec::new();
                for t in targets {
                    if store.get_snooze(&t.account_id, &t.thread_id)?.is_some() {
                        out.push(t);
                    }
                }
                Ok(out)
            })
            .await?;
            Ok(vec![actions::unsnooze(host, snoozed, true).await?])
        }
        "reply_later" => actions::reply_later(host, targets, true).await,
        "clear_reply_later" => actions::reply_later(host, targets, false).await,
        "add_label" | "remove_label" => {
            let wanted = label.ok_or_else(|| CmdError::invalid(format!("{tool} needs label")))?;
            // Every account's label first, so a missing one changes nothing.
            let mut plan = Vec::new();
            for (account, refs) in group(&targets) {
                let (store, acct, w) = (host.store().clone(), account.clone(), wanted.to_string());
                let id = blocking(move || resolve_label(&store, &acct, &w)).await?;
                plan.push((refs, id));
            }
            let mut out = Vec::new();
            for (refs, id) in plan {
                let action = thread_action(tool, Some(&id)).expect("a label tool");
                out.push(actions::apply(host.clone(), refs, action).await?);
            }
            Ok(out)
        }
        other => {
            let action = thread_action(other, None)
                .ok_or_else(|| CmdError::invalid(format!("{other} isn't an organizing tool")))?;
            Ok(vec![actions::apply(host, targets, action).await?])
        }
    }
}

// ---------- the agent's call ----------

/// What a call named, parsed from its arguments.
struct Call {
    targets: Vec<ThreadTarget>,
    label: Option<String>,
    until: Option<i64>,
}

fn parse_call(tool: &str, args: Value) -> CmdResult<Call> {
    let args = if args.is_null() {
        Value::Object(Default::default())
    } else {
        args
    };
    let bad = |e: serde_json::Error| CmdError::invalid(format!("{tool}: {e}"));
    Ok(match tool {
        "add_label" | "remove_label" => {
            let a: LabelArgs = serde_json::from_value(args).map_err(bad)?;
            Call {
                targets: a.targets,
                label: Some(a.label),
                until: None,
            }
        }
        "snooze" => {
            let a: SnoozeArgs = serde_json::from_value(args).map_err(bad)?;
            Call {
                targets: a.targets,
                label: None,
                until: Some(a.until.to_ms()?),
            }
        }
        _ => {
            let a: TargetsArgs = serde_json::from_value(args).map_err(bad)?;
            Call {
                targets: a.targets,
                label: None,
                until: None,
            }
        }
    })
}

/// Before-state of each named conversation that exists (None: not found).
struct Before {
    labels: Vec<String>,
    snoozed_until: Option<i64>,
}

/// An agent's organizing call, after the level gate (writes.rs).
pub async fn run<H: WriteHost>(
    host: &Arc<H>,
    limits: &Limits,
    client: &str,
    tool: &str,
    args: Value,
    audit: &mut AppAuditEntry,
) -> CmdResult<OrganizedOut> {
    let call = parse_call(tool, args)?;
    let mut targets: Vec<ThreadTarget> = Vec::new();
    for t in call.targets {
        let t = ThreadTarget {
            account_id: t.account_id.trim().to_lowercase(),
            thread_id: t.thread_id.trim().to_string(),
        };
        if t.account_id.is_empty() || t.thread_id.is_empty() {
            return Err(CmdError::invalid(format!(
                "{tool}: each target needs an accountId and a threadId"
            )));
        }
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    audit.thread_count = Some(targets.len());
    let accounts: Vec<&str> = {
        let mut a: Vec<&str> = targets.iter().map(|t| t.account_id.as_str()).collect();
        a.sort_unstable();
        a.dedup();
        a
    };
    audit.account_count = Some(accounts.len());
    if let [one] = accounts.as_slice() {
        audit.account = Some(one.to_string());
    }
    if targets.is_empty() {
        return Err(CmdError::invalid(format!(
            "{tool}: name at least one conversation in targets"
        )));
    }
    let capped = CAPPED_TOOLS.contains(&tool);
    let per_call = if capped {
        MAX_CAPPED_PER_CALL
    } else {
        MAX_TARGETS
    };
    if targets.len() > per_call {
        return Err(CmdError::invalid(format!(
            "{tool}: at most {per_call} conversations per call{}; this call named {}",
            if capped {
                " (a limit that keeps an agent from hiding mail in bulk)"
            } else {
                ""
            },
            targets.len()
        )));
    }

    // What exists, and how it looks now.
    let (store, named) = (host.store().clone(), targets.clone());
    let before: Vec<(ThreadTarget, Option<Before>)> = blocking(move || {
        let known: Vec<String> = store.list_accounts()?.into_iter().map(|a| a.id).collect();
        let mut out = Vec::new();
        for t in named {
            let state = if known.contains(&t.account_id) {
                store
                    .get_thread(&t.account_id, &t.thread_id)?
                    .map(|d| -> CmdResult<Before> {
                        Ok(Before {
                            labels: d.label_ids,
                            snoozed_until: store
                                .get_snooze(&t.account_id, &t.thread_id)?
                                .map(|s| s.wake_at),
                        })
                    })
                    .transpose()?
            } else {
                None
            };
            out.push((t, state));
        }
        Ok(out)
    })
    .await?;
    let found: Vec<ThreadRef> = before
        .iter()
        .filter(|(_, b)| b.is_some())
        .map(|(t, _)| ThreadRef::from(t))
        .collect();
    let not_found: Vec<ThreadTarget> = before
        .iter()
        .filter(|(_, b)| b.is_none())
        .map(|(t, _)| t.clone())
        .collect();

    let now = host.now_ms();
    if capped && !found.is_empty() {
        limits.reserve(tool, found.len(), now)?;
    }
    let result = execute(
        host.clone(),
        tool,
        found.clone(),
        call.label.as_deref(),
        call.until,
        now,
    )
    .await;
    let outcomes = match result {
        Ok(o) => o,
        Err(e) => {
            if capped && !found.is_empty() {
                limits.release(tool, found.len(), now);
            }
            return Err(e);
        }
    };
    // The agent waits for the provider, so the answer is the truth.
    let mut applied: Vec<Applied> = Vec::new();
    let mut failed: Vec<FailedOut> = Vec::new();
    for o in outcomes {
        let (a, pushed) = o.pushed().await;
        let error = pushed.error.unwrap_or_default();
        failed.extend(pushed.failed.iter().map(|t| FailedOut {
            account_id: t.account_id.clone(),
            thread_id: t.thread_id.clone(),
            error: error.clone(),
        }));
        applied.extend(a);
    }
    let is_failed = |t: &ThreadRef| {
        failed
            .iter()
            .any(|f| f.account_id == t.account_id && f.thread_id == t.thread_id)
    };
    let before_of = |t: &ThreadRef| {
        before
            .iter()
            .find(|(x, _)| x.account_id == t.account_id && x.thread_id == t.thread_id)
            .and_then(|(_, b)| b.as_ref())
    };
    let snooze_tool = matches!(tool, "snooze" | "unsnooze");
    let mut changed: Vec<ThreadChangeOut> = Vec::new();
    for a in &applied {
        if is_failed(&a.target) {
            continue;
        }
        let (added, removed) = (a.added(), a.removed());
        if added.is_empty() && removed.is_empty() && !snooze_tool {
            continue;
        }
        changed.push(ThreadChangeOut {
            account_id: a.target.account_id.clone(),
            thread_id: a.target.thread_id.clone(),
            added,
            removed,
        });
    }
    if tool == "unsnooze" {
        // Snoozed but already in the inbox: the snooze still ended.
        for t in &found {
            let was_snoozed = before_of(t).is_some_and(|b| b.snoozed_until.is_some());
            let listed = changed
                .iter()
                .any(|c| c.account_id == t.account_id && c.thread_id == t.thread_id);
            if was_snoozed && !listed && !is_failed(t) {
                changed.push(ThreadChangeOut {
                    account_id: t.account_id.clone(),
                    thread_id: t.thread_id.clone(),
                    added: Vec::new(),
                    removed: Vec::new(),
                });
            }
        }
    }
    let unchanged: Vec<ThreadTarget> = found
        .iter()
        .filter(|t| {
            !is_failed(t)
                && !changed
                    .iter()
                    .any(|c| c.account_id == t.account_id && c.thread_id == t.thread_id)
        })
        .map(ThreadTarget::from)
        .collect();
    let previous: Vec<ThreadBeforeOut> = before
        .iter()
        .filter_map(|(t, b)| {
            b.as_ref().map(|b| ThreadBeforeOut {
                account_id: t.account_id.clone(),
                thread_id: t.thread_id.clone(),
                label_ids: b.labels.clone(),
                snoozed_until: b.snoozed_until,
            })
        })
        .collect();
    let undo = undo_steps(tool, &changed, &previous);
    audit.changed_count = Some(changed.len());
    audit.result_count = changed.len();
    if !changed.is_empty() {
        host.organized(&AgentOrganized {
            tool: tool.to_string(),
            via: client.to_string(),
            count: changed.len(),
            undo: undo.clone(),
        });
    }
    tracing::info!(
        tool,
        via = client,
        threads = targets.len(),
        changed = changed.len(),
        failed = failed.len(),
        "agent organized mail"
    );
    Ok(OrganizedOut {
        tool: tool.to_string(),
        changed,
        unchanged,
        not_found,
        failed,
        previous,
        undo,
        note: note(tool),
    })
}

fn note(tool: &str) -> String {
    let undo = "Nothing is deleted: `undo` lists the calls that put it back, and the user sees an Undo in Penguin.";
    match tool {
        "trash" => format!(
            "Moved to Trash. {undo} `untrash` restores them until the provider empties its Trash on its own \
             schedule (about 30 days on Gmail and Outlook)."
        ),
        "report_spam" => format!(
            "Reported as spam: the provider may learn from it. {undo} `not_spam` brings them back until the provider \
             empties Spam (about 30 days on Gmail)."
        ),
        "snooze" => format!("Snoozed: archived now, back in the inbox at the time given (while Penguin runs). {undo}"),
        _ => undo.to_string(),
    }
}

/// An undo call's tool, label and time, which groups its targets.
type StepKey = (String, Option<String>, Option<i64>);

/// The calls that undo `tool`'s changes, thread by thread: the tool's
/// inverse, then any label the inverse wouldn't put back (untrash also
/// brings a thread to the inbox that was archived before it was trashed).
fn undo_steps(
    tool: &str,
    changed: &[ThreadChangeOut],
    previous: &[ThreadBeforeOut],
) -> Vec<UndoStep> {
    // (tool, label, until) → targets, in first-seen order.
    let mut steps: Vec<((String, Option<String>, Option<i64>), Vec<ThreadTarget>)> = Vec::new();
    let mut push = |key: (String, Option<String>, Option<i64>), t: ThreadTarget| match steps
        .iter_mut()
        .find(|(k, _)| *k == key)
    {
        Some((_, v)) => v.push(t),
        None => steps.push((key, vec![t])),
    };
    let mut residuals: Vec<(StepKey, ThreadTarget)> = Vec::new();
    for c in changed {
        let target = ThreadTarget {
            account_id: c.account_id.clone(),
            thread_id: c.thread_id.clone(),
        };
        let Some(prev) = previous
            .iter()
            .find(|p| p.account_id == c.account_id && p.thread_id == c.thread_id)
        else {
            continue;
        };
        // The inverse call, and the label change it makes.
        let label_of = |list: &[String]| {
            list.iter()
                .find(|l| !SYSTEM_LABELS.contains(&l.as_str()))
                .cloned()
        };
        let (inverse, label, until): (Option<&str>, Option<String>, Option<i64>) = match tool {
            "archive" => (Some("unarchive"), None, None),
            "unarchive" => (Some("archive"), None, None),
            "mark_read" => (Some("mark_unread"), None, None),
            "mark_unread" => (Some("mark_read"), None, None),
            "star" => (Some("unstar"), None, None),
            "unstar" => (Some("star"), None, None),
            "add_label" => (Some("remove_label"), label_of(&c.added), None),
            "remove_label" => (Some("add_label"), label_of(&c.removed), None),
            "trash" => (Some("untrash"), None, None),
            "untrash" => (Some("trash"), None, None),
            "report_spam" => (Some("not_spam"), None, None),
            "not_spam" => (Some("report_spam"), None, None),
            "reply_later" => (Some("clear_reply_later"), None, None),
            "clear_reply_later" => (None, None, None),
            "snooze" => match prev.snoozed_until {
                Some(t) => (Some("snooze"), None, Some(t)),
                None => (Some("unsnooze"), None, None),
            },
            "unsnooze" => (Some("snooze"), None, prev.snoozed_until),
            _ => (None, None, None),
        };
        if matches!(inverse, Some("add_label" | "remove_label")) && label.is_none() {
            continue;
        }
        let (inv_add, inv_remove) = match inverse {
            Some(inv) => {
                let label_id = match inv {
                    "clear_reply_later" => label_of(&c.added),
                    _ => label.clone(),
                };
                thread_action(inv, label_id.as_deref())
                    .map(|a| a.local_delta())
                    .unwrap_or_default()
            }
            None => (Vec::new(), Vec::new()),
        };
        if let Some(inv) = inverse {
            push((inv.to_string(), label.clone(), until), target.clone());
        }
        // Thread labels after this call, then after the inverse.
        let mut after: Vec<String> = prev
            .label_ids
            .iter()
            .filter(|l| !c.removed.contains(l))
            .cloned()
            .collect();
        after.extend(
            c.added
                .iter()
                .filter(|l| !after.contains(l))
                .cloned()
                .collect::<Vec<_>>(),
        );
        let mut undone: Vec<String> = after
            .into_iter()
            .filter(|l| !inv_remove.contains(l))
            .collect();
        undone.extend(
            inv_add
                .iter()
                .filter(|l| !undone.contains(l))
                .cloned()
                .collect::<Vec<_>>(),
        );
        let touched: Vec<&String> = c
            .added
            .iter()
            .chain(&c.removed)
            .chain(&inv_add)
            .chain(&inv_remove)
            .collect();
        for l in touched {
            let want = prev.label_ids.contains(l);
            let have = undone.contains(l);
            if want == have {
                continue;
            }
            let key = match (l.as_str(), want) {
                ("INBOX", true) => ("unarchive".to_string(), None),
                ("INBOX", false) => ("archive".to_string(), None),
                ("UNREAD", true) => ("mark_unread".to_string(), None),
                ("UNREAD", false) => ("mark_read".to_string(), None),
                ("STARRED", true) => ("star".to_string(), None),
                ("STARRED", false) => ("unstar".to_string(), None),
                ("TRASH", true) => ("trash".to_string(), None),
                ("TRASH", false) => ("untrash".to_string(), None),
                ("SPAM", true) => ("report_spam".to_string(), None),
                ("SPAM", false) => ("not_spam".to_string(), None),
                (id, true) => ("add_label".to_string(), Some(id.to_string())),
                (id, false) => ("remove_label".to_string(), Some(id.to_string())),
            };
            let key = (key.0, key.1, None);
            if !residuals.iter().any(|(k, t)| *k == key && *t == target) {
                residuals.push((key, target.clone()));
            }
        }
    }
    for (key, t) in residuals {
        push(key, t);
    }
    steps
        .into_iter()
        .map(|((tool, label, until), targets)| UndoStep {
            tool,
            arguments: UndoArgs {
                targets,
                label,
                until,
            },
        })
        .collect()
}

#[cfg(test)]
#[path = "organize_tests.rs"]
mod tests;
