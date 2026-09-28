//! Send later + remind-if-no-reply + snooze wakes: the scheduler loop and
//! its commands. The firing rules live in `penguin_provider::outbox` and
//! `penguin_provider::snooze` (tested with a fake clock) and reach each
//! account through its provider; this is glue: a tokio task that wakes at
//! the next due time (or
//! when the schedule changes), fires what's due, and emits
//! `penguin://scheduled-sent` / `penguin://reminder-due` /
//! `penguin://snooze-woke` (see `snooze.rs`) plus `mail-changed`.
//! Overdue items (the app was closed) fire on the first pass after launch;
//! sleeping at most `MAX_IDLE` catches up after the Mac sleeps. With
//! nothing scheduled the task holds no timer: every command that schedules
//! something (send later, reminders, snooze) wakes it.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use penguin_core::{Reminder, ScheduledSend};
use penguin_provider::outbox::{self, OutboxEvent};
use penguin_provider::snooze::{self as snoozes, Woke};
use penguin_provider::MailProvider;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Notify;

use crate::error::{CmdError, CmdResult};
use crate::ops;
use crate::state::{blocking, AppState};

/// One per scheduler pass that sent or gave up on something.
pub const SCHEDULED_SENT_EVENT: &str = "penguin://scheduled-sent";
/// One per fired reminder.
pub const REMINDER_DUE_EVENT: &str = "penguin://reminder-due";
/// Longest sleep before something scheduled, so the clock jumping after
/// the Mac sleeps is caught within a minute.
const MAX_IDLE: Duration = Duration::from_secs(60);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SentItem {
    id: String,
    account_id: String,
    draft_id: String,
    message_id: String,
    thread_id: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FailedItem {
    id: String,
    account_id: String,
    draft_id: String,
    message: String,
}

/// `penguin://scheduled-sent` payload.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ScheduledSentBatch {
    sent: Vec<SentItem>,
    failed: Vec<FailedItem>,
    /// How many of `sent` were already due when the app started.
    missed: usize,
}

/// `penguin://reminder-due` payload.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReminderDue {
    account_id: String,
    thread_id: String,
    subject: String,
}

fn wake() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

/// Start the scheduler (once, at app setup; never in the CLI).
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    let launched_at = ops::now_ms();
    tauri::async_runtime::spawn(async move {
        loop {
            let now = ops::now_ms();
            match run_once(&state, now).await {
                Ok((events, woke)) => {
                    publish(&app, &state, events, launched_at);
                    crate::snooze::publish(&app, &state, woke, launched_at);
                }
                Err(e) => tracing::warn!(error = %e, "outbox pass failed"),
            }
            let store = state.store.clone();
            let next =
                blocking(move || Ok(earliest(store.next_outbox_due()?, store.next_snooze_due()?)))
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "could not read the outbox schedule");
                        None
                    });
            match idle_wait(next, ops::now_ms()) {
                Some(wait) => tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = wake().notified() => {}
                },
                // Nothing scheduled: no timer until something is.
                None => wake().notified().await,
            }
        }
    });
}

fn earliest(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// How long to sleep before the next pass: until `next` is due, but at
/// least 250 ms and at most `MAX_IDLE` (a timer doesn't count time the Mac
/// spends asleep, so a long sleep would wake late). None: nothing is
/// scheduled, sleep until woken.
fn idle_wait(next: Option<i64>, now: i64) -> Option<Duration> {
    next.map(|t| {
        Duration::from_millis((t - now).max(0) as u64)
            .min(MAX_IDLE)
            .max(Duration::from_millis(250))
    })
}

fn publish(app: &AppHandle, state: &Arc<AppState>, events: Vec<OutboxEvent>, launched_at: i64) {
    let mut batch = ScheduledSentBatch::default();
    for event in events {
        if let Some(thread) = event.changed_thread() {
            state.emit_mail_changed(event.account_id(), vec![thread.to_string()]);
        }
        match event {
            OutboxEvent::Sent {
                account_id,
                schedule_id,
                draft_id,
                message_id,
                thread_id,
                scheduled_for,
            } => {
                // The sent copy arrives through history; fetch it now.
                state.poke(&account_id);
                // A scheduled reply takes its conversation out of Reply Later.
                crate::reply_later::after_send(
                    state.clone(),
                    account_id.clone(),
                    vec![thread_id.clone()],
                );
                if scheduled_for < launched_at {
                    batch.missed += 1;
                }
                batch.sent.push(SentItem {
                    id: schedule_id,
                    account_id,
                    draft_id,
                    message_id,
                    thread_id,
                });
            }
            OutboxEvent::SendFailed {
                account_id,
                schedule_id,
                draft_id,
                error,
                retry_at: None,
            } => {
                batch.failed.push(FailedItem {
                    id: schedule_id,
                    account_id,
                    draft_id,
                    message: error,
                });
            }
            // Will retry by itself; the schedule stays visible with its error.
            OutboxEvent::SendFailed { .. } => {}
            OutboxEvent::SendCancelled {
                account_id,
                schedule_id,
                draft_id,
                reason,
            } => {
                batch.failed.push(FailedItem {
                    id: schedule_id,
                    account_id,
                    draft_id,
                    message: reason,
                });
            }
            OutboxEvent::ReminderFired {
                account_id,
                thread_id,
                subject,
                ..
            } => {
                if let Err(e) = app.emit(
                    REMINDER_DUE_EVENT,
                    ReminderDue {
                        account_id,
                        thread_id,
                        subject,
                    },
                ) {
                    tracing::warn!(error = %e, "could not emit reminder event");
                }
            }
        }
    }
    if !batch.sent.is_empty() || !batch.failed.is_empty() {
        if let Err(e) = app.emit(SCHEDULED_SENT_EVENT, batch) {
            tracing::warn!(error = %e, "could not emit scheduled-sent event");
        }
    }
}

/// Fire everything due, with a provider client for each account that has
/// work due. Accounts without one (signed out, a provider whose backend
/// isn't installed) keep their sends and reminders for later (snoozes still
/// wake locally). Does nothing until some backend exists (for a Gmail-only
/// user: until Google sign-in is configured).
async fn run_once(
    state: &AppState,
    now: i64,
) -> penguin_provider::Result<(Vec<OutboxEvent>, Vec<Woke>)> {
    if state.registry.all().is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut accounts = outbox::due_accounts(&state.store, now).await?;
    accounts.extend(snoozes::due_accounts(&state.store, now).await?);
    accounts.sort();
    accounts.dedup();
    if accounts.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut clients: HashMap<String, Arc<dyn MailProvider>> = HashMap::new();
    for account in accounts {
        match state.provider(&account).await {
            Ok(client) => {
                clients.insert(account, client);
            }
            Err(e) => {
                tracing::debug!(account = %account, error = %e, "outbox: account not connected")
            }
        }
    }
    let events = outbox::run_due(&state.store, &clients, now).await?;
    let woke = snoozes::run_due(&state.store, &clients, now).await?;
    Ok((events, woke))
}

/// Wake the scheduler after the schedule changed elsewhere (e.g. a draft
/// with a schedule was sent by hand).
pub fn schedule_changed() {
    wake().notify_one();
}

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Send the saved draft `draftId` at `sendAt` (unix ms; up to a minute in
/// the past is treated as now, further back or over a year out is invalid).
/// Re-scheduling a draft replaces its schedule. `remindAfterMs`: after it's
/// sent, bring the thread back if nobody replies within that long.
#[tauri::command]
pub async fn schedule_send(
    state: AppStateRef<'_>,
    account_id: String,
    draft_id: String,
    send_at: i64,
    remind_after_ms: Option<i64>,
) -> CmdResult<ScheduledSend> {
    state.account(&account_id).await?;
    let s = outbox::schedule_send(
        &state.store,
        &account_id,
        &draft_id,
        send_at,
        remind_after_ms,
        ops::now_ms(),
    )
    .await
    .map_err(|e| CmdError::invalid(e.to_string()))?;
    wake().notify_one();
    Ok(s)
}

/// Idempotent; the draft itself stays in Drafts.
#[tauri::command]
pub async fn cancel_scheduled_send(state: AppStateRef<'_>, id: String) -> CmdResult<()> {
    let store = state.store.clone();
    blocking(move || Ok(store.delete_scheduled_send_by_id(&id)?)).await?;
    wake().notify_one();
    Ok(())
}

/// Soonest first; `accountId` omitted = every account.
#[tauri::command]
pub async fn list_scheduled_sends(
    state: AppStateRef<'_>,
    account_id: Option<String>,
) -> CmdResult<Vec<ScheduledSend>> {
    let store = state.store.clone();
    blocking(move || Ok(store.list_scheduled_sends(account_id.as_deref())?)).await
}

/// Bring `threadId` back to the inbox (unread) at `remindAt` unless someone
/// else replies after `sentMessageId` was sent. Replaces the thread's
/// reminder. Call it right after an immediate send (with send_message's
/// SentRef); scheduled sends arm theirs via `remindAfterMs`.
#[tauri::command]
pub async fn set_reminder(
    state: AppStateRef<'_>,
    account_id: String,
    thread_id: String,
    sent_message_id: Option<String>,
    remind_at: i64,
) -> CmdResult<Reminder> {
    state.account(&account_id).await?;
    let now = ops::now_ms();
    if remind_at <= now {
        return Err(CmdError::invalid("remindAt must be in the future"));
    }
    let r = outbox::set_reminder(
        &state.store,
        &account_id,
        &thread_id,
        sent_message_id.as_deref(),
        remind_at,
        now,
    )
    .await?;
    wake().notify_one();
    Ok(r)
}

/// Idempotent.
#[tauri::command]
pub async fn cancel_reminder(state: AppStateRef<'_>, id: String) -> CmdResult<()> {
    let store = state.store.clone();
    blocking(move || Ok(store.delete_reminder_by_id(&id)?)).await?;
    wake().notify_one();
    Ok(())
}

/// Soonest first; `accountId` omitted = every account.
#[tauri::command]
pub async fn list_reminders(
    state: AppStateRef<'_>,
    account_id: Option<String>,
) -> CmdResult<Vec<Reminder>> {
    let store = state.store.clone();
    blocking(move || Ok(store.list_reminders(account_id.as_deref())?)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_pass_is_the_earliest_of_sends_reminders_and_snoozes() {
        assert_eq!(earliest(None, None), None);
        assert_eq!(earliest(Some(5), None), Some(5));
        assert_eq!(earliest(None, Some(7)), Some(7));
        assert_eq!(earliest(Some(9), Some(7)), Some(7));
    }

    #[test]
    fn idle_wait_is_bounded() {
        let now = 1_000_000;
        // Nothing scheduled: no timer at all.
        assert_eq!(idle_wait(None, now), None);
        assert_eq!(idle_wait(Some(now + 5_000), now), Some(Duration::from_secs(5)));
        // Overdue (e.g. woke from sleep): run again right away.
        assert_eq!(
            idle_wait(Some(now - 60_000), now),
            Some(Duration::from_millis(250))
        );
        // Far off: still look again within a minute.
        assert_eq!(idle_wait(Some(now + 86_400_000), now), Some(MAX_IDLE));
    }
}
