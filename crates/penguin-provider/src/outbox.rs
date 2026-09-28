//! Send later and remind-if-no-reply, for every provider (moved here from
//! penguin-gmail when the provider seam landed; the logic is unchanged).
//!
//! - **Send later** schedules an already-saved server draft. At `send_at`
//!   the draft is sent as saved ([`MailProvider::send_saved_draft`]; Gmail:
//!   `drafts.send` with only its id), so edits made before then, HTML and
//!   attachments all go out exactly as saved. Transient failures (offline,
//!   rate limits, signed out) retry with backoff and never drop the
//!   schedule; permanent ones (the draft is gone, the provider rejects it)
//!   end it with an event. A retry can't double-send: sending consumes the
//!   draft, so a second attempt gets not-found.
//! - **Remind if no reply** re-surfaces a thread (INBOX + UNREAD, on the
//!   provider and locally) at `remind_at` unless someone else has replied
//!   since `sent_at` (`Store::has_reply_after`: judged by flags, so your own
//!   replies from any address don't count).
//!
//! Everything takes `now_ms` explicitly so the app's scheduler and the tests
//! (with a fake clock) drive it the same way; the loop itself is app glue.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

pub use penguin_core::SentRef;
use penguin_core::{Reminder, ScheduledSend, Store};
use serde::Serialize;

use crate::{Error, MailProvider, Result};

/// First retry delay; doubles per attempt up to `MAX_RETRY_DELAY_MS`.
const FIRST_RETRY_DELAY_MS: i64 = 60_000;
const MAX_RETRY_DELAY_MS: i64 = 30 * 60_000;
/// A reminder whose provider update failed is retried this much later.
const REMINDER_RETRY_MS: i64 = 5 * 60_000;
/// Labels a fired reminder adds back.
const RESURFACE: [&str; 2] = ["INBOX", "UNREAD"];
/// A send time this far in the past is a mistake, not "now".
const PAST_GRACE_MS: i64 = 60_000;
/// Latest a send may be scheduled, from now.
const MAX_SCHEDULE_AHEAD_MS: i64 = 366 * 24 * 60 * 60_000;

/// The provider calls the outbox makes. Implemented for every
/// [`MailProvider`] (`Arc<dyn MailProvider>`); tests use a fake.
pub trait OutboxApi: Send + Sync {
    /// Send a saved draft as-is (Gmail: `drafts.send` with only its id).
    fn send_saved_draft(&self, draft_id: &str) -> impl Future<Output = Result<SentRef>> + Send;
    fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> impl Future<Output = Result<()>> + Send;
}

// For every trait-object lifetime: the scheduler future is checked for Send
// with erased lifetimes, which a `+ 'static`-only impl fails.
impl<'p> OutboxApi for Arc<dyn MailProvider + 'p> {
    async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef> {
        MailProvider::send_saved_draft(self.as_ref(), draft_id).await
    }

    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        MailProvider::modify_thread(self.as_ref(), thread_id, add, remove).await
    }
}

/// What happened when the outbox ran; the app turns these into toasts and
/// `mail-changed` events. Mirrored in `apps/desktop/src/lib/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OutboxEvent {
    Sent {
        account_id: String,
        schedule_id: String,
        draft_id: String,
        message_id: String,
        thread_id: String,
        /// When it was scheduled for (unix ms), to tell sends that were due
        /// while the app was closed.
        scheduled_for: i64,
    },
    /// Reported on the first failure of a schedule and when giving up.
    SendFailed {
        account_id: String,
        schedule_id: String,
        draft_id: String,
        error: String,
        /// `Some(next attempt, unix ms)` while it will be retried.
        retry_at: Option<i64>,
    },
    /// The draft no longer exists (sent or deleted elsewhere, or sent by an
    /// earlier attempt whose reply was lost).
    SendCancelled {
        account_id: String,
        schedule_id: String,
        draft_id: String,
        reason: String,
    },
    ReminderFired {
        account_id: String,
        reminder_id: String,
        thread_id: String,
        /// The thread's subject, for the notification.
        subject: String,
    },
}

impl OutboxEvent {
    pub fn account_id(&self) -> &str {
        match self {
            OutboxEvent::Sent { account_id, .. }
            | OutboxEvent::SendFailed { account_id, .. }
            | OutboxEvent::SendCancelled { account_id, .. }
            | OutboxEvent::ReminderFired { account_id, .. } => account_id,
        }
    }

    /// The thread whose local state changed, if any.
    pub fn changed_thread(&self) -> Option<&str> {
        match self {
            OutboxEvent::Sent { thread_id, .. } | OutboxEvent::ReminderFired { thread_id, .. } => {
                Some(thread_id)
            }
            _ => None,
        }
    }
}

pub fn new_id() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

async fn db<T: Send + 'static>(
    store: &Store,
    f: impl FnOnce(&Store) -> penguin_core::Result<T> + Send + 'static,
) -> Result<T> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| Error::Other(format!("store task failed: {e}")))?
        .map_err(Error::from)
}

/// Schedule `draft_id` to be sent at `send_at` (replacing any schedule the
/// draft already has). `remind_after_ms`: remind if no reply that long
/// after it's actually sent.
pub async fn schedule_send(
    store: &Store,
    account_id: &str,
    draft_id: &str,
    send_at: i64,
    remind_after_ms: Option<i64>,
    now_ms: i64,
) -> Result<ScheduledSend> {
    if remind_after_ms.is_some_and(|ms| ms <= 0) {
        return Err(Error::Other("remindAfterMs must be positive".into()));
    }
    if send_at < now_ms - PAST_GRACE_MS {
        return Err(Error::Other("sendAt is in the past".into()));
    }
    if send_at > now_ms + MAX_SCHEDULE_AHEAD_MS {
        return Err(Error::Other("sendAt is more than a year away".into()));
    }
    let s = ScheduledSend {
        id: new_id(),
        account_id: account_id.to_string(),
        draft_id: draft_id.to_string(),
        send_at,
        created_at: now_ms,
        remind_after_ms,
        attempts: 0,
        last_error: None,
    };
    let row = s.clone();
    db(store, move |st| {
        for old in st.scheduled_sends_for_draft(&row.account_id, &row.draft_id)? {
            st.delete_scheduled_send(&row.account_id, &old.id)?;
        }
        st.upsert_scheduled_send(&row)
    })
    .await?;
    Ok(s)
}

/// Remind about `thread_id` at `remind_at` unless someone replies after
/// `sent_message_id` was sent (its stored date; now when unknown, e.g. just
/// sent and not synced yet). Replaces the thread's existing reminder.
pub async fn set_reminder(
    store: &Store,
    account_id: &str,
    thread_id: &str,
    sent_message_id: Option<&str>,
    remind_at: i64,
    now_ms: i64,
) -> Result<Reminder> {
    let mut row = Reminder {
        id: new_id(),
        account_id: account_id.to_string(),
        thread_id: thread_id.to_string(),
        sent_message_id: sent_message_id.map(str::to_string),
        sent_at: now_ms,
        remind_at,
        created_at: now_ms,
    };
    db(store, move |st| {
        if let Some(mid) = &row.sent_message_id {
            if let Some(m) = st.get_message(&row.account_id, mid)? {
                row.sent_at = m.date;
            }
        }
        for old in st.list_reminders(Some(&row.account_id))? {
            if old.thread_id == row.thread_id {
                st.delete_reminder(&row.account_id, &old.id)?;
            }
        }
        st.upsert_reminder(&row)?;
        Ok(row)
    })
    .await
}

/// Accounts with outbox work due at `now_ms`, so the caller can build just
/// those clients.
pub async fn due_accounts(store: &Store, now_ms: i64) -> Result<Vec<String>> {
    db(store, move |st| {
        let mut ids: Vec<String> = st
            .due_scheduled_sends(now_ms)?
            .into_iter()
            .map(|s| s.account_id)
            .collect();
        ids.extend(st.due_reminders(now_ms)?.into_iter().map(|r| r.account_id));
        ids.sort();
        ids.dedup();
        Ok(ids)
    })
    .await
}

fn retry_delay(attempts: u32) -> i64 {
    (FIRST_RETRY_DELAY_MS << attempts.saturating_sub(1).min(10)).min(MAX_RETRY_DELAY_MS)
}

/// Worth retrying: the send may succeed later without anyone acting
/// (offline, throttled, Google hiccup, signed out until reconnect).
fn transient(e: &Error) -> bool {
    match e {
        Error::Http { status, .. } => {
            *status == 429 || *status >= 500 || *status == 401 || *status == 403
        }
        Error::Network(_) | Error::RateLimited | Error::NeedsReauth(_) | Error::Keychain(_) => true,
        _ => false,
    }
}

/// Fire everything due at `now_ms`. `clients` holds a provider client per
/// account that is connected; work for other accounts waits (sends count it
/// as a failed attempt so the user hears about it once).
pub async fn run_due<A: OutboxApi>(
    store: &Store,
    clients: &HashMap<String, A>,
    now_ms: i64,
) -> Result<Vec<OutboxEvent>> {
    let mut events = Vec::new();
    let sends = db(store, move |st| st.due_scheduled_sends(now_ms)).await?;
    for s in sends {
        let result = match clients.get(&s.account_id) {
            Some(api) => api.send_saved_draft(&s.draft_id).await,
            None => Err(Error::NeedsReauth(format!(
                "{} is not connected",
                s.account_id
            ))),
        };
        events.extend(finish_send(store, s, result, now_ms).await?);
    }
    let reminders = db(store, move |st| st.due_reminders(now_ms)).await?;
    for r in reminders {
        events.extend(fire_reminder(store, clients.get(&r.account_id), r, now_ms).await?);
    }
    Ok(events)
}

async fn finish_send(
    store: &Store,
    s: ScheduledSend,
    result: Result<SentRef>,
    now_ms: i64,
) -> Result<Option<OutboxEvent>> {
    match result {
        Ok(sent) => {
            tracing::info!(account = %s.account_id, message = %sent.message_id, "scheduled send sent");
            let (row, thread, message) =
                (s.clone(), sent.thread_id.clone(), sent.message_id.clone());
            db(store, move |st| {
                st.delete_scheduled_send(&row.account_id, &row.id)?;
                // The draft is gone on the server; drop its local copy and mapping.
                if let Some(mid) = st.message_for_draft(&row.account_id, &row.draft_id)? {
                    st.delete_messages(&row.account_id, &[mid])?;
                }
                st.remove_draft(&row.account_id, &row.draft_id)?;
                if let Some(after) = row.remind_after_ms {
                    st.upsert_reminder(&Reminder {
                        id: new_id(),
                        account_id: row.account_id.clone(),
                        thread_id: thread,
                        sent_message_id: Some(message),
                        sent_at: now_ms,
                        remind_at: now_ms + after,
                        created_at: now_ms,
                    })?;
                }
                Ok(())
            })
            .await?;
            Ok(Some(OutboxEvent::Sent {
                account_id: s.account_id,
                schedule_id: s.id,
                draft_id: s.draft_id,
                message_id: sent.message_id,
                thread_id: sent.thread_id,
                scheduled_for: s.send_at,
            }))
        }
        Err(e) if e.is_not_found() => {
            let reason = if s.attempts > 0 {
                "The draft no longer exists; an earlier attempt may have sent it".to_string()
            } else {
                "The draft no longer exists (sent or deleted elsewhere)".to_string()
            };
            let (a, id) = (s.account_id.clone(), s.id.clone());
            db(store, move |st| st.delete_scheduled_send(&a, &id)).await?;
            Ok(Some(OutboxEvent::SendCancelled {
                account_id: s.account_id,
                schedule_id: s.id,
                draft_id: s.draft_id,
                reason,
            }))
        }
        Err(e) if transient(&e) => {
            let mut next = s.clone();
            next.attempts += 1;
            next.last_error = Some(e.to_string());
            next.send_at = now_ms + retry_delay(next.attempts);
            tracing::warn!(account = %s.account_id, attempts = next.attempts, error = %e, "scheduled send failed; will retry");
            let row = next.clone();
            db(store, move |st| st.upsert_scheduled_send(&row)).await?;
            Ok((next.attempts == 1).then(|| OutboxEvent::SendFailed {
                account_id: next.account_id,
                schedule_id: next.id,
                draft_id: next.draft_id,
                error: e.to_string(),
                retry_at: Some(next.send_at),
            }))
        }
        Err(e) => {
            tracing::warn!(account = %s.account_id, error = %e, "scheduled send failed permanently");
            let (a, id) = (s.account_id.clone(), s.id.clone());
            db(store, move |st| st.delete_scheduled_send(&a, &id)).await?;
            Ok(Some(OutboxEvent::SendFailed {
                account_id: s.account_id,
                schedule_id: s.id,
                draft_id: s.draft_id,
                error: e.to_string(),
                retry_at: None,
            }))
        }
    }
}

async fn fire_reminder<A: OutboxApi>(
    store: &Store,
    api: Option<&A>,
    r: Reminder,
    now_ms: i64,
) -> Result<Option<OutboxEvent>> {
    let (a, t, since) = (r.account_id.clone(), r.thread_id.clone(), r.sent_at);
    if db(store, move |st| st.has_reply_after(&a, &t, since)).await? {
        let (a, id) = (r.account_id.clone(), r.id.clone());
        db(store, move |st| st.delete_reminder(&a, &id)).await?;
        return Ok(None);
    }
    let add: Vec<String> = RESURFACE.iter().map(|l| l.to_string()).collect();
    let result = match api {
        Some(api) => api.modify_thread(&r.thread_id, &add, &[]).await,
        None => Err(Error::NeedsReauth(format!(
            "{} is not connected",
            r.account_id
        ))),
    };
    match result {
        Ok(()) => {
            let row = r.clone();
            let subject = db(store, move |st| {
                st.modify_thread_labels(&row.account_id, &row.thread_id, &add, &[])?;
                st.delete_reminder(&row.account_id, &row.id)?;
                Ok(st
                    .get_thread(&row.account_id, &row.thread_id)?
                    .map(|t| t.subject)
                    .unwrap_or_default())
            })
            .await?;
            Ok(Some(OutboxEvent::ReminderFired {
                account_id: r.account_id,
                reminder_id: r.id,
                thread_id: r.thread_id,
                subject,
            }))
        }
        // Thread deleted on the server: nothing to remind about.
        Err(e) if e.is_not_found() => {
            let (a, id) = (r.account_id.clone(), r.id.clone());
            db(store, move |st| st.delete_reminder(&a, &id)).await?;
            Ok(None)
        }
        Err(e) => {
            tracing::warn!(account = %r.account_id, error = %e, "reminder could not re-surface the thread; retrying");
            let mut later = r;
            later.remind_at = now_ms + REMINDER_RETRY_MS;
            db(store, move |st| st.upsert_reminder(&later)).await?;
            Ok(None)
        }
    }
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
