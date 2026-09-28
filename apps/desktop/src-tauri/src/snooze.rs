//! Snooze commands and what the scheduler reports when snoozes wake.
//!
//! Snoozing = a local record (penguin-core `snoozes`) + the normal archive
//! (`commands::apply_thread_action`: optimistic, pushed to the provider,
//! reverted on failure — and a revert puts the thread back in the inbox,
//! which ends the snooze by itself). Waking runs in the outbox scheduler
//! loop (`outbox.rs`), with the rules in `penguin_provider::snooze`; this module
//! turns its results into `mail-changed`, `penguin://snooze-woke` and, when
//! the window is in the background, a notification.

use std::sync::Arc;

use penguin_core::{account_scope, AccountId, Snooze};
use penguin_provider::snooze::{self as rules, Woke};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::apply_thread_action;
use crate::error::{CmdError, CmdResult};
use crate::ops;
use crate::outbox;
use crate::state::{blocking, AppState};
use crate::views::{ThreadAction, ThreadRef};

/// One per scheduler pass that woke something.
pub const SNOOZE_WOKE_EVENT: &str = "penguin://snooze-woke";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WokeItem {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
}

/// `penguin://snooze-woke` payload.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeWokeBatch {
    pub items: Vec<WokeItem>,
    /// How many were already due when the app started (woke late).
    pub missed: usize,
}

/// The event payload for one pass (pure, for tests).
pub fn woke_batch(woke: Vec<Woke>, launched_at: i64) -> SnoozeWokeBatch {
    let missed = woke.iter().filter(|w| w.due_at < launched_at).count();
    SnoozeWokeBatch {
        items: woke
            .into_iter()
            .map(|w| WokeItem {
                account_id: w.account_id,
                thread_id: w.thread_id,
                subject: w.subject,
            })
            .collect(),
        missed,
    }
}

/// (title, body) of the background notification for a batch.
pub fn notification_text(batch: &SnoozeWokeBatch) -> Option<(String, String)> {
    match batch.items.as_slice() {
        [] => None,
        [one] => Some((
            "Snoozed conversation is back".into(),
            if one.subject.trim().is_empty() {
                "(no subject)".into()
            } else {
                one.subject.clone()
            },
        )),
        many => Some((
            format!("{} snoozed conversations are back", many.len()),
            "They're at the top of your inbox".into(),
        )),
    }
}

/// Called by the scheduler after a pass.
pub fn publish(app: &AppHandle, state: &AppState, woke: Vec<Woke>, launched_at: i64) {
    if woke.is_empty() {
        return;
    }
    let mut by_account: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for w in &woke {
        by_account
            .entry(w.account_id.clone())
            .or_default()
            .push(w.thread_id.clone());
    }
    for (account, threads) in by_account {
        state.emit_mail_changed(&account, threads);
    }
    let batch = woke_batch(woke, launched_at);
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if !focused {
        if let Some((title, body)) = notification_text(&batch) {
            use tauri_plugin_notification::NotificationExt;
            if let Err(e) = app.notification().builder().title(title).body(body).show() {
                tracing::warn!(error = %e, "could not show the snooze notification");
            }
        }
    }
    if let Err(e) = app.emit(SNOOZE_WOKE_EVENT, batch) {
        tracing::warn!(error = %e, "could not emit snooze-woke event");
    }
}

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Snooze `targets` until `until` (unix ms, future, within a year): record
/// the snooze, then archive them (optimistic; reverted with action-failed if
/// Gmail refuses, which also ends the snooze). Replaces an existing snooze.
#[tauri::command]
pub async fn snooze_threads(
    state: AppStateRef<'_>,
    targets: Vec<ThreadRef>,
    until: i64,
) -> CmdResult<()> {
    rules::check_until(until, ops::now_ms()).map_err(CmdError::invalid)?;
    if targets.is_empty() {
        return Ok(());
    }
    // Refuse before writing records we could never act on.
    state
        .require_providers(targets.iter().map(|t| t.account_id.as_str()))
        .await?;
    let store = state.store.clone();
    let pairs: Vec<(AccountId, String)> = targets
        .iter()
        .map(|t| (t.account_id.clone(), t.thread_id.clone()))
        .collect();
    let rows = pairs.clone();
    blocking(move || Ok(store.snooze_threads(&rows, until, ops::now_ms())?)).await?;
    if let Err(e) = apply_thread_action(state.inner().clone(), targets, ThreadAction::Archive).await
    {
        let store = state.store.clone();
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
        return Err(e);
    }
    outbox::schedule_changed();
    Ok(())
}

/// End the snooze of `targets` now. `toInbox`: also move them to the inbox
/// (the Snoozed view's Unsnooze, and undoing a snooze made from the inbox);
/// false only drops the record (undoing a snooze made elsewhere).
#[tauri::command]
pub async fn unsnooze_threads(
    state: AppStateRef<'_>,
    targets: Vec<ThreadRef>,
    to_inbox: bool,
) -> CmdResult<()> {
    if targets.is_empty() {
        return Ok(());
    }
    let store = state.store.clone();
    let rows = targets.clone();
    blocking(move || {
        for t in &rows {
            store.unsnooze(&t.account_id, &t.thread_id)?;
        }
        Ok(())
    })
    .await?;
    outbox::schedule_changed();
    if to_inbox {
        return apply_thread_action(state.inner().clone(), targets, ThreadAction::MoveToInbox)
            .await;
    }
    let mut by_account: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for t in targets {
        by_account
            .entry(t.account_id)
            .or_default()
            .push(t.thread_id);
    }
    for (account, threads) in by_account {
        state.emit_mail_changed(&account, threads);
    }
    Ok(())
}

/// Active snoozes, soonest first; `accountIds` narrows to a set (null =
/// every account).
#[tauri::command]
pub async fn list_snoozes(
    state: AppStateRef<'_>,
    account_ids: Option<Vec<String>>,
) -> CmdResult<Vec<Snooze>> {
    let store = state.store.clone();
    blocking(move || {
        let scope = account_scope(None, account_ids.as_deref());
        Ok(store.list_snoozes(scope.as_deref())?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn woke(thread: &str, due_at: i64, subject: &str) -> Woke {
        Woke {
            account_id: "ada@penguin.example".into(),
            thread_id: thread.into(),
            subject: subject.into(),
            due_at,
        }
    }

    #[test]
    fn batch_counts_wakes_that_were_due_before_launch() {
        let b = woke_batch(
            vec![woke("t1", 5, "A"), woke("t2", 10, "B"), woke("t3", 11, "C")],
            10,
        );
        assert_eq!(b.missed, 1);
        assert_eq!(b.items.len(), 3);
        assert_eq!(b.items[1].thread_id, "t2");
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["items"][0]["accountId"], "ada@penguin.example");
        assert_eq!(json["missed"], 1);
    }

    #[test]
    fn notification_names_one_thread_or_counts_many() {
        assert_eq!(notification_text(&woke_batch(vec![], 0)), None);
        let one = notification_text(&woke_batch(vec![woke("t1", 0, "Quarterly plan")], 0)).unwrap();
        assert_eq!(
            one,
            (
                "Snoozed conversation is back".into(),
                "Quarterly plan".into()
            )
        );
        let blank = notification_text(&woke_batch(vec![woke("t1", 0, "  ")], 0)).unwrap();
        assert_eq!(blank.1, "(no subject)");
        let many = notification_text(&woke_batch(vec![woke("t1", 0, "A"), woke("t2", 0, "B")], 0))
            .unwrap();
        assert_eq!(many.0, "2 snoozed conversations are back");
    }
}
