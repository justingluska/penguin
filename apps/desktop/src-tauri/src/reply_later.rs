//! Reply Later and Follow up (the triage views under Inbox; store side in
//! penguin-core `store_triage.rs`).
//!
//! Reply Later is a real label so the providers' own apps show it too: each
//! account's [`REPLY_LATER_LABEL`] (Gmail user label, IMAP folder, Microsoft
//! category), created on first use through `MailProvider::ensure_label`.
//! Marking = add the label + archive + mark read in one optimistic action
//! (`ThreadAction::ReplyLater`; on IMAP that's a single move into the
//! folder, on Microsoft a category plus the move to Archive). Sending from
//! Penguin in a Reply Later conversation takes the label off
//! ([`after_send`]). New mail in the thread brings it back to the Inbox the
//! normal way and it keeps the label.
//!
//! Follow up is local: `Store::list_follow_ups` with `Settings.followUpDays`,
//! and per-thread dismissals.

use std::collections::BTreeMap;
use std::sync::Arc;

use penguin_core::{
    account_scope, AccountId, ListQuery, ThreadSummary, TriageCount, REPLY_LATER_LABEL,
};
use tauri::State;

use crate::commands::apply_thread_action;
use crate::error::CmdResult;
use crate::ops;
use crate::state::{blocking, AppState};
use crate::views::{ThreadAction, ThreadRef};

type AppStateRef<'a> = State<'a, Arc<AppState>>;

fn by_account(targets: Vec<ThreadRef>) -> BTreeMap<String, Vec<ThreadRef>> {
    let mut out: BTreeMap<String, Vec<ThreadRef>> = BTreeMap::new();
    for t in targets {
        out.entry(t.account_id.clone()).or_default().push(t);
    }
    out
}

/// The account's Reply Later label id: the stored one, else found or
/// created on the provider and stored. One at a time, so two quick marks
/// on a new account don't both create it.
async fn ensure_label(state: &AppState, account_id: &str) -> CmdResult<String> {
    static CREATING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = CREATING.lock().await;
    let (store, acct) = (state.store.clone(), account_id.to_string());
    if let Some(id) = blocking(move || Ok(store.reply_later_label(&acct)?)).await? {
        return Ok(id);
    }
    let provider = state.provider(account_id).await?;
    let mut label = provider.ensure_label(REPLY_LATER_LABEL).await?;
    label.account_id = account_id.to_string();
    let id = label.id.clone();
    let store = state.store.clone();
    blocking(move || Ok(store.upsert_label(&label)?)).await?;
    tracing::info!(account = %account_id, "reply later label ready");
    Ok(id)
}

/// Mark (`on`) conversations Reply Later: out of the inbox, read, labelled.
/// `on: false` only takes the label off (undo puts the inbox and unread
/// state back with `modify_threads`). Optimistic like `modify_threads`.
#[tauri::command]
pub async fn reply_later(
    state: AppStateRef<'_>,
    targets: Vec<ThreadRef>,
    on: bool,
) -> CmdResult<()> {
    if targets.is_empty() {
        return Ok(());
    }
    state
        .require_providers(targets.iter().map(|t| t.account_id.as_str()))
        .await?;
    for (account_id, refs) in by_account(targets) {
        let label_id = if on {
            ensure_label(&state, &account_id).await?
        } else {
            let (store, acct) = (state.store.clone(), account_id.clone());
            match blocking(move || Ok(store.reply_later_label(&acct)?)).await? {
                Some(id) => id,
                None => continue,
            }
        };
        let action = if on {
            ThreadAction::ReplyLater { label_id }
        } else {
            ThreadAction::RemoveLabel { label_id }
        };
        apply_thread_action(state.inner().clone(), refs, action).await?;
    }
    Ok(())
}

/// A message went out in `threads` (a reply from Penguin, now or scheduled):
/// they leave Reply Later. Best effort, in the background.
pub fn after_send(state: Arc<AppState>, account_id: String, threads: Vec<String>) {
    tauri::async_runtime::spawn(async move {
        let (store, acct) = (state.store.clone(), account_id.clone());
        let found = blocking(move || {
            let Some(label) = store.reply_later_label(&acct)? else {
                return Ok(None);
            };
            let mut hit = Vec::new();
            for t in threads {
                if hit.contains(&t) {
                    continue;
                }
                let labelled = store
                    .get_thread(&acct, &t)?
                    .is_some_and(|d| d.label_ids.contains(&label));
                if labelled {
                    hit.push(t);
                }
            }
            Ok(Some((label, hit)))
        })
        .await;
        let (label_id, hit) = match found {
            Ok(Some((l, h))) if !h.is_empty() => (l, h),
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(account = %account_id, error = %e.message, "reply later check after send failed");
                return;
            }
        };
        let refs = hit
            .into_iter()
            .map(|thread_id| ThreadRef {
                account_id: account_id.clone(),
                thread_id,
            })
            .collect();
        if let Err(e) =
            apply_thread_action(state, refs, ThreadAction::RemoveLabel { label_id }).await
        {
            tracing::warn!(account = %account_id, error = %e.message, "couldn't take a replied conversation out of Reply Later");
        }
    });
}

/// `list_threads` for `MailboxView::FollowUp`, with the saved wait.
pub async fn list_follow_ups(state: &AppState, query: ListQuery) -> CmdResult<Vec<ThreadSummary>> {
    // One page holds them all (like Snoozed): `before` pages are empty.
    if query.before.is_some() {
        return Ok(Vec::new());
    }
    let days = state.settings.get().follow_up_days;
    let store = state.store.clone();
    blocking(move || {
        let scope = account_scope(query.account_id.as_deref(), query.account_ids.as_deref());
        Ok(store.list_follow_ups(scope.as_deref(), days, ops::now_ms(), query.unread_only)?)
    })
    .await
}

/// Sidebar counts for Reply Later and Follow up, per account (null = every
/// account).
#[tauri::command]
pub async fn triage_counts(
    state: AppStateRef<'_>,
    account_ids: Option<Vec<AccountId>>,
) -> CmdResult<Vec<TriageCount>> {
    let days = state.settings.get().follow_up_days;
    let store = state.store.clone();
    blocking(move || Ok(store.triage_counts(account_ids.as_deref(), days, ops::now_ms())?)).await
}

/// Hide conversations from Follow up until something newer is sent in them
/// (`dismissed: false` shows them again: undo). Local only.
#[tauri::command]
pub async fn dismiss_follow_ups(
    state: AppStateRef<'_>,
    targets: Vec<ThreadRef>,
    dismissed: bool,
) -> CmdResult<()> {
    if targets.is_empty() {
        return Ok(());
    }
    let store = state.store.clone();
    let rows: Vec<(String, String)> = targets
        .iter()
        .map(|t| (t.account_id.clone(), t.thread_id.clone()))
        .collect();
    blocking(move || Ok(store.dismiss_follow_ups(&rows, dismissed)?)).await?;
    for (account_id, refs) in by_account(targets) {
        state.emit_mail_changed(&account_id, refs.into_iter().map(|r| r.thread_id).collect());
    }
    Ok(())
}
