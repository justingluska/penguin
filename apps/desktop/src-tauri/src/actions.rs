//! The app's thread actions, one path for everyone who organizes mail: the
//! UI (`modify_threads`, `snooze_threads`, `reply_later`), the rules engine,
//! and agents (agent/organize.rs). Tauri-free: the app implements
//! [`ActionHost`] on `AppState`; tests implement it over the fake provider.
//!
//! An action is optimistic: local labels change at once and a mail-changed
//! event goes out, so every view and count updates; the provider call runs
//! in the background, and if the provider refuses, the exact local change
//! is taken back and the user is told (`action_failed`).
//!
//! - **Snoozing** = a local record (penguin-core `snoozes`) + the normal
//!   archive; a failed archive drops the record again.
//! - **Reply Later** = the account's Reply Later label (created on first
//!   use through `MailProvider::ensure_label`) + archive + mark read, in one
//!   delta (`ThreadAction::ReplyLater`).

use std::collections::BTreeMap;
use std::sync::Arc;

use penguin_core::{AccountId, Store, REPLY_LATER_LABEL};
use penguin_provider::snooze as rules;
use penguin_provider::{async_trait, MailProvider};

use crate::error::{CmdError, CmdResult};
use crate::state::blocking;
use crate::views::{ThreadAction, ThreadRef};

/// Concurrent provider calls for one bulk action (clients back off on 429s).
const ACTION_CONCURRENCY: usize = 6;

/// What the action path needs from the app.
#[async_trait]
pub trait ActionHost: Send + Sync + 'static {
    fn store(&self) -> &Store;
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>>;
    /// Refuse up front, before local state changes, when an account's
    /// provider isn't available (e.g. this build lacks its backend).
    async fn require_providers(&self, account_ids: &[String]) -> CmdResult<()> {
        let mut seen = std::collections::HashSet::new();
        for id in account_ids {
            if seen.insert(id.as_str()) {
                self.provider(id).await?;
            }
        }
        Ok(())
    }
    /// Local mail changed: tell the UI (lists, counts, the open thread).
    fn emit_mail_changed(&self, account_id: &str, thread_ids: Vec<String>);
    /// The provider refused an action and it was taken back.
    fn emit_action_failed(&self, message: String);
    /// Ask the account's sync for the server's view now.
    fn poke(&self, account_id: &str);
}

/// One thread an action changed locally, with the exact per-message label
/// changes, so a failure restores exactly the previous state (a star on an
/// already starred message must not be removed on revert).
#[derive(Debug, Clone)]
pub struct Applied {
    pub target: ThreadRef,
    /// The thread's labels before the action (all messages together).
    pub before: Vec<String>,
    /// (message id, labels added, labels removed)
    pub changes: Vec<(String, Vec<String>, Vec<String>)>,
}

impl Applied {
    /// Labels the action added to at least one message.
    pub fn added(&self) -> Vec<String> {
        union(self.changes.iter().map(|(_, a, _)| a))
    }
    /// Labels the action removed from at least one message.
    pub fn removed(&self) -> Vec<String> {
        union(self.changes.iter().map(|(_, _, r)| r))
    }
}

fn union<'a>(lists: impl Iterator<Item = &'a Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in lists.flatten() {
        if !out.contains(l) {
            out.push(l.clone());
        }
    }
    out
}

/// What the provider push of an action ended with: the threads it refused
/// (their local change was taken back) and its first error.
#[derive(Debug, Clone, Default)]
pub struct Pushed {
    pub failed: Vec<ThreadRef>,
    pub error: Option<String>,
}

/// What an action did locally, and its provider push (running in the
/// background; the UI never waits for it, agents do).
pub struct Outcome {
    pub applied: Vec<Applied>,
    pub push: Option<tauri::async_runtime::JoinHandle<Pushed>>,
}

impl Outcome {
    fn none() -> Outcome {
        Outcome {
            applied: Vec::new(),
            push: None,
        }
    }

    /// Wait for the provider push.
    pub async fn pushed(self) -> (Vec<Applied>, Pushed) {
        let pushed = match self.push {
            Some(p) => p.await.unwrap_or_else(|e| Pushed {
                failed: Vec::new(),
                error: Some(format!("the provider push stopped: {e}")),
            }),
            None => Pushed::default(),
        };
        (self.applied, pushed)
    }
}

/// The optimistic modify path: change local labels now, emit mail-changed,
/// push to each account's provider in the background (reverted with
/// action-failed on error). Threads not stored locally are skipped.
pub async fn apply<H: ActionHost>(
    host: Arc<H>,
    targets: Vec<ThreadRef>,
    action: ThreadAction,
) -> CmdResult<Outcome> {
    if targets.is_empty() {
        return Ok(Outcome::none());
    }
    // Refuse up front rather than change local state we could never push.
    let accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    host.require_providers(&accounts).await?;
    let (add, remove) = action.local_delta();
    let store = host.store().clone();
    let applied = blocking(move || {
        let mut applied = Vec::new();
        let mut last_err = None;
        for target in targets {
            let before = match store.get_thread(&target.account_id, &target.thread_id) {
                Ok(Some(t)) => t,
                Ok(None) => continue,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            if let Err(e) =
                store.modify_thread_labels(&target.account_id, &target.thread_id, &add, &remove)
            {
                last_err = Some(e);
                continue;
            }
            let changes = before
                .messages
                .iter()
                .map(|m| {
                    let added: Vec<String> = add
                        .iter()
                        .filter(|l| !m.label_ids.contains(l))
                        .cloned()
                        .collect();
                    let removed: Vec<String> = remove
                        .iter()
                        .filter(|l| m.label_ids.contains(l))
                        .cloned()
                        .collect();
                    (m.id.clone(), added, removed)
                })
                .filter(|(_, a, r)| !a.is_empty() || !r.is_empty())
                .collect();
            applied.push(Applied {
                target,
                before: before.label_ids,
                changes,
            });
        }
        match (applied.is_empty(), last_err) {
            (true, Some(e)) => Err(e.into()),
            (_, Some(e)) => {
                tracing::warn!(error = %e, "some threads could not be updated locally");
                Ok(applied)
            }
            _ => Ok(applied),
        }
    })
    .await?;

    for (account_id, thread_ids) in group_by_account(applied.iter().map(|a| &a.target)) {
        host.emit_mail_changed(&account_id, thread_ids);
    }
    let push = tauri::async_runtime::spawn(push_action(host, applied.clone(), action));
    Ok(Outcome {
        applied,
        push: Some(push),
    })
}

async fn push_action<H: ActionHost>(
    host: Arc<H>,
    applied: Vec<Applied>,
    action: ThreadAction,
) -> Pushed {
    let limit = Arc::new(tokio::sync::Semaphore::new(ACTION_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for item in applied {
        let (host, action, limit) = (host.clone(), action.clone(), limit.clone());
        tasks.spawn(async move {
            let _permit = limit.acquire_owned().await;
            let result = push_one(host.as_ref(), &item.target, &action).await;
            (item, result)
        });
    }
    let mut failed: Vec<Applied> = Vec::new();
    let mut first_error: Option<CmdError> = None;
    let mut succeeded_accounts = std::collections::BTreeSet::new();
    while let Some(joined) = tasks.join_next().await {
        let Ok((item, result)) = joined else { continue };
        match result {
            Ok(()) => {
                succeeded_accounts.insert(item.target.account_id.clone());
            }
            Err(e) => {
                tracing::warn!(account = %item.target.account_id, thread = %item.target.thread_id, error = %e, "provider rejected action; reverting");
                first_error.get_or_insert(e);
                failed.push(item);
            }
        }
    }
    // Pull the server's view of what we just changed (e.g. trash side effects).
    for account_id in succeeded_accounts {
        host.poke(&account_id);
    }
    let Some(err) = first_error else {
        return Pushed::default();
    };

    let store = host.store().clone();
    let targets: Vec<ThreadRef> = failed.iter().map(|a| a.target.clone()).collect();
    let revert = blocking(move || {
        for item in &failed {
            for (message_id, added, removed) in &item.changes {
                store.modify_message_labels(
                    &item.target.account_id,
                    std::slice::from_ref(message_id),
                    removed,
                    added,
                )?;
            }
        }
        Ok(())
    })
    .await;
    if let Err(e) = revert {
        tracing::error!(error = %e, "reverting a failed action failed; next sync will reconcile");
    }
    for (account_id, thread_ids) in group_by_account(targets.iter()) {
        host.emit_mail_changed(&account_id, thread_ids);
    }
    let n = targets.len();
    let noun = if n == 1 {
        "conversation"
    } else {
        "conversations"
    };
    host.emit_action_failed(format!(
        "Couldn't {} {n} {noun}: {}",
        action.verb(),
        err.message
    ));
    Pushed {
        failed: targets,
        error: Some(err.message),
    }
}

async fn push_one<H: ActionHost + ?Sized>(
    host: &H,
    target: &ThreadRef,
    action: &ThreadAction,
) -> CmdResult<()> {
    let provider = host.provider(&target.account_id).await?;
    let id = target.thread_id.as_str();
    match action {
        ThreadAction::Trash => provider.trash_thread(id).await?,
        // Out of the trash and into the inbox.
        ThreadAction::Untrash => provider.untrash_thread(id).await?,
        other => {
            let (add, remove) = other.local_delta();
            provider.modify_thread(id, &add, &remove).await?;
        }
    }
    Ok(())
}

pub fn group_by_account<'a>(
    targets: impl Iterator<Item = &'a ThreadRef>,
) -> BTreeMap<String, Vec<String>> {
    let mut by_account: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in targets {
        by_account
            .entry(t.account_id.clone())
            .or_default()
            .push(t.thread_id.clone());
    }
    by_account
}

fn refs_by_account(targets: Vec<ThreadRef>) -> BTreeMap<String, Vec<ThreadRef>> {
    let mut out: BTreeMap<String, Vec<ThreadRef>> = BTreeMap::new();
    for t in targets {
        out.entry(t.account_id.clone()).or_default().push(t);
    }
    out
}

// ---------- snooze ----------

/// Snooze `targets` until `until` (unix ms, future, within a year): record
/// the snooze, then archive them (optimistic; reverted with action-failed if
/// the provider refuses, which also ends the snooze: a revert puts the
/// thread back in the inbox). Replaces an existing snooze.
pub async fn snooze<H: ActionHost>(
    host: Arc<H>,
    targets: Vec<ThreadRef>,
    until: i64,
    now: i64,
) -> CmdResult<Outcome> {
    rules::check_until(until, now).map_err(CmdError::invalid)?;
    if targets.is_empty() {
        return Ok(Outcome::none());
    }
    // Refuse before writing records we could never act on.
    let accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    host.require_providers(&accounts).await?;
    let store = host.store().clone();
    let pairs: Vec<(AccountId, String)> = targets
        .iter()
        .map(|t| (t.account_id.clone(), t.thread_id.clone()))
        .collect();
    let rows = pairs.clone();
    blocking(move || Ok(store.snooze_threads(&rows, until, now)?)).await?;
    match apply(host.clone(), targets, ThreadAction::Archive).await {
        Ok(outcome) => {
            crate::outbox::schedule_changed();
            Ok(outcome)
        }
        Err(e) => {
            let store = host.store().clone();
            let undo = blocking(move || {
                for (a, t) in &pairs {
                    store.unsnooze(a, t)?;
                }
                Ok(())
            })
            .await;
            if let Err(u) = undo {
                tracing::warn!(error = %u.message, "could not drop snoozes after a failed archive");
            }
            Err(e)
        }
    }
}

/// End the snooze of `targets` now. `to_inbox`: also move them to the inbox
/// (the Snoozed view's Unsnooze, and undoing a snooze made from the inbox);
/// false only drops the record (undoing a snooze made elsewhere).
pub async fn unsnooze<H: ActionHost>(
    host: Arc<H>,
    targets: Vec<ThreadRef>,
    to_inbox: bool,
) -> CmdResult<Outcome> {
    if targets.is_empty() {
        return Ok(Outcome::none());
    }
    let store = host.store().clone();
    let rows = targets.clone();
    blocking(move || {
        for t in &rows {
            store.unsnooze(&t.account_id, &t.thread_id)?;
        }
        Ok(())
    })
    .await?;
    crate::outbox::schedule_changed();
    if to_inbox {
        return apply(host, targets, ThreadAction::MoveToInbox).await;
    }
    for (account, threads) in group_by_account(targets.iter()) {
        host.emit_mail_changed(&account, threads);
    }
    Ok(Outcome::none())
}

// ---------- Reply Later ----------

/// The account's Reply Later label id: the stored one, else found or
/// created on the provider and stored. One at a time, so two quick marks
/// on a new account don't both create it.
pub async fn ensure_reply_later_label<H: ActionHost + ?Sized>(
    host: &H,
    account_id: &str,
) -> CmdResult<String> {
    static CREATING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = CREATING.lock().await;
    let (store, acct) = (host.store().clone(), account_id.to_string());
    if let Some(id) = blocking(move || Ok(store.reply_later_label(&acct)?)).await? {
        return Ok(id);
    }
    let provider = host.provider(account_id).await?;
    let mut label = provider.ensure_label(REPLY_LATER_LABEL).await?;
    label.account_id = account_id.to_string();
    let id = label.id.clone();
    let store = host.store().clone();
    blocking(move || Ok(store.upsert_label(&label)?)).await?;
    tracing::info!(account = %account_id, "reply later label ready");
    Ok(id)
}

/// Mark (`on`) conversations Reply Later: out of the inbox, read, labelled.
/// `on: false` only takes the label off (an account that never used Reply
/// Later is left alone). One outcome per account.
pub async fn reply_later<H: ActionHost>(
    host: Arc<H>,
    targets: Vec<ThreadRef>,
    on: bool,
) -> CmdResult<Vec<Outcome>> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    host.require_providers(&accounts).await?;
    let mut out = Vec::new();
    for (account_id, refs) in refs_by_account(targets) {
        let label_id = if on {
            ensure_reply_later_label(host.as_ref(), &account_id).await?
        } else {
            let (store, acct) = (host.store().clone(), account_id.clone());
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
        out.push(apply(host.clone(), refs, action).await?);
    }
    Ok(out)
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outcome")
            .field("applied", &self.applied.len())
            .finish()
    }
}

#[async_trait]
impl ActionHost for crate::state::AppState {
    fn store(&self) -> &Store {
        &self.store
    }
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>> {
        crate::state::AppState::provider(self, account_id).await
    }
    async fn require_providers(&self, account_ids: &[String]) -> CmdResult<()> {
        crate::state::AppState::require_providers(self, account_ids.iter().map(String::as_str))
            .await
    }
    fn emit_mail_changed(&self, account_id: &str, thread_ids: Vec<String>) {
        crate::state::AppState::emit_mail_changed(self, account_id, thread_ids)
    }
    fn emit_action_failed(&self, message: String) {
        crate::state::AppState::emit_action_failed(self, message)
    }
    fn poke(&self, account_id: &str) {
        crate::state::AppState::poke(self, account_id)
    }
}
