//! The read operations behind both the CLI and the MCP tools. Blocking
//! (SQLite): call from `spawn_blocking` in async code.

use std::time::Instant;

use penguin_core::{account_scope, ListQuery, MailboxView, SearchRequest};

use super::output::{
    search_out, thread_out, AccountOut, AccountsOut, AttachmentOut, AttachmentTextOut, LabelOut,
    LabelsOut, PeopleOut, PersonOut, ProfileOut, SearchOut, ThreadListOut, ThreadOptions,
    ThreadOut, ThreadSummaryOut,
};
use super::{AgentCtx, Scope};
use crate::attachments::{self, PreviewKind};
use crate::error::{CmdError, CmdResult};

pub const DEFAULT_LIMIT: u32 = 20;
pub const MAX_LIMIT: u32 = 200;

fn limit(n: Option<u32>) -> u32 {
    n.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

pub fn search(ctx: &AgentCtx, query: &str, scope: &Scope, n: Option<u32>) -> CmdResult<SearchOut> {
    if query.trim().is_empty() {
        return Err(CmdError::invalid("query is empty"));
    }
    let started = Instant::now();
    let r = ctx.store.search(&SearchRequest {
        query: query.to_string(),
        account_id: scope.account_id.clone(),
        account_ids: scope.account_ids.clone(),
        limit: limit(n),
    })?;
    Ok(search_out(
        query,
        scope,
        &r,
        started.elapsed().as_secs_f64() * 1000.0,
    ))
}

pub fn thread(
    ctx: &AgentCtx,
    account: &str,
    thread_id: &str,
    opts: ThreadOptions,
) -> CmdResult<ThreadOut> {
    let account = account.trim().to_lowercase();
    let t = ctx
        .store
        .get_thread(&account, thread_id)?
        .ok_or_else(|| CmdError::not_found(format!("no thread {thread_id} in {account}")))?;
    let pending = ctx.store.pending_message_ids(&account, thread_id)?;
    Ok(thread_out(&t, opts, &pending))
}

pub fn thread_markdown(
    ctx: &AgentCtx,
    account: &str,
    thread_id: &str,
    max_chars: Option<usize>,
) -> CmdResult<String> {
    let account = account.trim().to_lowercase();
    let t = ctx
        .store
        .get_thread(&account, thread_id)?
        .ok_or_else(|| CmdError::not_found(format!("no thread {thread_id} in {account}")))?;
    let pending = ctx.store.pending_message_ids(&account, thread_id)?;
    Ok(super::context::thread_markdown(&t, max_chars, &pending))
}

/// `view`: inbox | starred | sent | drafts | done | trash | spam | all | label.
/// For `label`, `label` is a label id or name (any case) in the scope.
pub fn list_threads(
    ctx: &AgentCtx,
    view: &str,
    label: Option<&str>,
    scope: &Scope,
    n: Option<u32>,
    before: Option<i64>,
) -> CmdResult<ThreadListOut> {
    let (mailbox, label_id) = match view.trim().to_ascii_lowercase().as_str() {
        "inbox" => (MailboxView::Inbox, None),
        "starred" => (MailboxView::Starred, None),
        "sent" => (MailboxView::Sent, None),
        "drafts" => (MailboxView::Drafts, None),
        "done" | "archive" | "archived" => (MailboxView::Done, None),
        "trash" => (MailboxView::Trash, None),
        "spam" => (MailboxView::Spam, None),
        "all" => (MailboxView::All, None),
        "label" => {
            let wanted = label
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .ok_or_else(|| CmdError::invalid("view \"label\" needs a label (id or name)"))?;
            let accounts = account_scope(scope.account_id.as_deref(), scope.account_ids.as_deref());
            let labels = ctx.store.list_labels_in(accounts.as_deref())?;
            let id = labels
                .iter()
                .find(|l| l.id == wanted)
                .or_else(|| labels.iter().find(|l| l.name.eq_ignore_ascii_case(wanted)))
                .map(|l| l.id.clone())
                .ok_or_else(|| CmdError::not_found(format!("no label {wanted}")))?;
            (MailboxView::Label(id.clone()), Some(id))
        }
        other => {
            return Err(CmdError::invalid(format!(
                "unknown view {other}; use inbox, starred, sent, drafts, done, trash, spam, all or label"
            )))
        }
    };
    let n = limit(n);
    let threads = ctx.store.list_threads(&ListQuery {
        view: mailbox,
        tab: None,
        account_id: scope.account_id.clone(),
        account_ids: scope.account_ids.clone(),
        limit: n,
        before,
        unread_only: false,
        split: None,
    })?;
    let next_before = (threads.len() as u32 == n)
        .then(|| threads.last().map(|t| t.last_date))
        .flatten();
    Ok(ThreadListOut {
        view: view.trim().to_ascii_lowercase(),
        label_id,
        scope: scope.into(),
        threads: threads.iter().map(ThreadSummaryOut::from).collect(),
        next_before,
    })
}

pub fn labels(ctx: &AgentCtx, scope: &Scope) -> CmdResult<LabelsOut> {
    let accounts = account_scope(scope.account_id.as_deref(), scope.account_ids.as_deref());
    let labels = ctx.store.list_labels_in(accounts.as_deref())?;
    Ok(LabelsOut {
        scope: scope.into(),
        labels: labels.iter().map(LabelOut::from).collect(),
    })
}

/// People matching a name or address fragment, most-corresponded first.
pub fn people(ctx: &AgentCtx, query: &str, scope: &Scope, n: Option<u32>) -> CmdResult<PeopleOut> {
    if query.trim().is_empty() {
        return Err(CmdError::invalid("query is empty"));
    }
    let r = ctx.store.search(&SearchRequest {
        query: query.to_string(),
        account_id: scope.account_id.clone(),
        account_ids: scope.account_ids.clone(),
        limit: 1,
    })?;
    let mut people: Vec<PersonOut> = r.people.iter().map(PersonOut::from).collect();
    people.truncate(limit(n) as usize);
    Ok(PeopleOut {
        query: query.to_string(),
        scope: scope.into(),
        people,
    })
}

pub fn accounts(ctx: &AgentCtx) -> CmdResult<AccountsOut> {
    let mut out = Vec::new();
    for a in ctx.accounts()? {
        let n = ctx.store.count_messages(Some(&a.id))?;
        out.push(AccountOut::new(&a, n));
    }
    let profiles = ctx
        .settings()
        .profiles
        .into_iter()
        .map(|p| ProfileOut {
            id: p.id,
            name: p.name,
            account_ids: p.account_ids,
        })
        .collect();
    Ok(AccountsOut {
        accounts: out,
        profiles,
        indexed_messages: ctx.store.count_messages(None)?,
    })
}

/// Longest question accepted (same as the app's ask command).
const MAX_QUESTION: usize = 500;

/// Answer a question from the local index (penguin_core::ask: fixed grammar
/// and exact queries, no model, no network). Profiles are offered as aliases
/// the way the app does ("when did we start with Acme").
pub fn ask(ctx: &AgentCtx, question: &str, scope: &Scope) -> CmdResult<super::output::AskOut> {
    use chrono::Offset;
    use penguin_core::ask::{AskAlias, AskScope};
    let question = question.trim();
    if question.is_empty() || question.chars().count() > MAX_QUESTION {
        return Err(CmdError::invalid("question must be 1–500 characters"));
    }
    let aliases = ctx
        .settings()
        .profiles
        .into_iter()
        .map(|p| AskAlias {
            name: p.name,
            account_ids: p.account_ids,
        })
        .collect();
    let ask_scope = AskScope {
        account_ids: account_scope(scope.account_id.as_deref(), scope.account_ids.as_deref()),
        aliases,
        ..Default::default()
    };
    let offset = chrono::Local::now().offset().fix().local_minus_utc();
    let answer = ctx
        .store
        .ask(question, &ask_scope, crate::ops::now_ms(), offset)?;
    Ok(super::output::AskOut {
        scope: scope.into(),
        answer,
    })
}

/// Text of an attachment that is text-like AND already in the local cache
/// (previewed or downloaded in the app). Never fetches from Gmail.
pub fn attachment_text(
    ctx: &AgentCtx,
    account: &str,
    message_id: &str,
    attachment_id: &str,
) -> CmdResult<AttachmentTextOut> {
    let account = account.trim().to_lowercase();
    let m = ctx
        .store
        .get_message(&account, message_id)?
        .ok_or_else(|| CmdError::not_found(format!("no message {message_id} in {account}")))?;
    let att = m
        .attachments
        .iter()
        .find(|a| a.id == attachment_id)
        .ok_or_else(|| {
            CmdError::not_found(format!(
                "no attachment {attachment_id} on message {message_id}"
            ))
        })?;
    let bytes =
        attachments::cached_bytes(&ctx.paths, &account, message_id, &att.id).ok_or_else(|| {
            CmdError::not_found(format!(
                "{} isn't cached locally; preview or download it in Penguin first",
                att.filename
            ))
        })?;
    let preview = attachments::from_bytes(att, &bytes);
    match (preview.kind, preview.text) {
        (PreviewKind::Text, Some(text)) => Ok(AttachmentTextOut {
            account_id: account,
            message_id: message_id.to_string(),
            attachment: AttachmentOut::from(att),
            text,
            truncated: preview.truncated,
        }),
        _ => Err(CmdError::invalid(format!(
            "{} ({}) is not a text attachment",
            att.filename, att.mime_type
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testkit;

    #[test]
    fn queries_on_fixture() {
        let (ctx, root) = testkit::fixture("queries");
        let all = Scope::default();
        assert_eq!(search(&ctx, "walrus", &all, None).unwrap().hits.len(), 1);
        assert!(search(&ctx, "  ", &all, None).is_err());

        let inbox = list_threads(&ctx, "inbox", None, &all, Some(10), None).unwrap();
        assert!(inbox.threads.iter().any(|t| t.thread_id == "t2"));
        let by_name = list_threads(&ctx, "label", Some("projects"), &all, None, None).unwrap();
        assert_eq!(by_name.label_id.as_deref(), Some("Label_7"));
        assert_eq!(by_name.threads.len(), 1);
        assert!(list_threads(&ctx, "bogus", None, &all, None, None).is_err());

        assert_eq!(labels(&ctx, &all).unwrap().labels[0].name, "Projects");
        assert!(people(&ctx, "bo", &all, None)
            .unwrap()
            .people
            .iter()
            .any(|p| p.email == testkit::BO));
        let accts = accounts(&ctx).unwrap();
        assert_eq!(accts.accounts[0].indexed_messages, 3);

        let t = thread(
            &ctx,
            "ADA@penguin.example",
            "t1",
            ThreadOptions {
                include_full_text: true,
                max_chars_per_message: Some(10),
            },
        )
        .unwrap();
        assert!(t.messages[0].truncated);
        assert!(thread(
            &ctx,
            testkit::ADA,
            "nope",
            ThreadOptions {
                include_full_text: false,
                max_chars_per_message: None
            }
        )
        .is_err());

        assert!(ctx.scope(Some("stranger@x.example"), None).is_err());
        assert!(ctx.scope(None, Some("Work")).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ask_answers_from_the_read_only_store() {
        let (ctx, root) = testkit::fixture("ask");
        let all = Scope::default();
        let known = ask(&ctx, "when did I last hear from Bo Park?", &all).unwrap();
        let v = serde_json::to_value(&known.answer).unwrap();
        assert_ne!(v["confidence"], "none", "{v}");
        assert!(
            known.answer.items.iter().any(|i| {
                let i = serde_json::to_value(i).unwrap();
                i["threadId"] == "t1" || i["threadId"] == "t2"
            }),
            "{v}"
        );
        let unknown = ask(&ctx, "when did I last email Zyzzyva Quuxley?", &all).unwrap();
        assert_eq!(
            serde_json::to_value(&unknown.answer).unwrap()["confidence"],
            "none"
        );
        assert!(ask(&ctx, "   ", &all).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn attachment_text_only_from_cache() {
        let (ctx, root) = testkit::fixture("att");
        let err = attachment_text(&ctx, testkit::ADA, "m3", "att-1").unwrap_err();
        assert!(err.message.contains("isn't cached"), "{}", err.message);
        let dir = attachments::cache_dir(&ctx.paths, testkit::ADA).join("m3");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("att-1"), b"agenda: walrus migration").unwrap();
        let out = attachment_text(&ctx, testkit::ADA, "m3", "att-1").unwrap();
        assert_eq!(out.text, "agenda: walrus migration");
        let _ = std::fs::remove_dir_all(root);
    }
}
