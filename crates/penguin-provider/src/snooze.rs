//! Snooze: wake snoozed conversations on time, and tell the provider. Moved
//! here from penguin-gmail when the provider seam landed (logic unchanged);
//! it works for any provider that can archive and move back to the inbox.
//!
//! Gmail has no public snooze API, so Penguin snoozes locally: the app
//! archives the thread (the normal optimistic modify path) and keeps a row
//! in penguin-core's `snoozes` table (lifecycle in `store_snooze.rs`). At
//! `wake_at` this module:
//! 1. wakes it locally first (`Store::wake_snooze`: INBOX + UNREAD, sorted
//!    at the wake time), so the inbox is right even offline;
//! 2. then pushes the same labels to Gmail with `threads.modify`. Failures
//!    (offline, signed out, rate limited) retry every few minutes and never
//!    undo the local wake; a 404 (thread deleted on Gmail) drops the snooze
//!    and leaves the local copy to sync.
//!
//! Early wake (new mail in the thread) needs nothing here: the new message
//! arrives in the inbox and the store ends the snooze on that refresh.
//! Everything takes `now_ms` so the app's scheduler and the tests (fake
//! clock) drive it the same way; the loop is app glue (`src-tauri/src/outbox.rs`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use penguin_core::store::{DueSnooze, LocalWake, WAKE_LABELS};
use penguin_core::{AccountId, Store};
use serde::Serialize;

use crate::{Error, MailProvider, Result};

/// A woken thread whose provider update failed is retried this much later.
pub const PUSH_RETRY_MS: i64 = 5 * 60_000;
/// Latest a thread may be snoozed until, from now (Gmail's own limit is
/// far out; a year keeps typos from hiding mail for decades).
pub const MAX_SNOOZE_AHEAD_MS: i64 = 366 * 24 * 60 * 60_000;

/// The provider call a wake makes. Implemented for every [`MailProvider`]
/// (`Arc<dyn MailProvider>`); tests use a fake.
pub trait SnoozeApi: Send + Sync {
    fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> impl Future<Output = Result<()>> + Send;
}

// For every trait-object lifetime (see the note on OutboxApi in outbox.rs).
impl<'p> SnoozeApi for Arc<dyn MailProvider + 'p> {
    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        MailProvider::modify_thread(self.as_ref(), thread_id, add, remove).await
    }
}

/// What a scheduler pass did; the app turns these into `mail-changed`, a
/// toast and (when the window is in the background) a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Woke {
    pub account_id: String,
    pub thread_id: String,
    /// The thread's subject, for the notification.
    pub subject: String,
    /// When it was due (unix ms), to tell wakes that were due while the app
    /// was closed.
    pub due_at: i64,
}

/// `Err(reason)` unless `until` is in the future and within a year.
pub fn check_until(until: i64, now_ms: i64) -> std::result::Result<(), String> {
    if until <= now_ms {
        return Err("Pick a time in the future".into());
    }
    if until > now_ms + MAX_SNOOZE_AHEAD_MS {
        return Err("Pick a time within a year".into());
    }
    Ok(())
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

/// Accounts with snooze work due at `now_ms`, so the caller builds just
/// those clients.
pub async fn due_accounts(store: &Store, now_ms: i64) -> Result<Vec<AccountId>> {
    db(store, move |st| {
        let mut ids: Vec<AccountId> = st
            .due_snoozes(now_ms)?
            .into_iter()
            .map(|d| d.account_id)
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    })
    .await
}

/// Wake everything due at `now_ms` and push owed wakes to the provider.
/// `clients` holds a client per connected account; the others wake locally and
/// push later.
pub async fn run_due<A: SnoozeApi>(
    store: &Store,
    clients: &HashMap<AccountId, A>,
    now_ms: i64,
) -> Result<Vec<Woke>> {
    let due = db(store, move |st| st.due_snoozes(now_ms)).await?;
    let mut woke = Vec::new();
    for d in due {
        if !d.woken {
            let (a, t) = (d.account_id.clone(), d.thread_id.clone());
            match db(store, move |st| st.wake_snooze(&a, &t, now_ms)).await? {
                LocalWake::Woke { subject } => {
                    tracing::info!(account = %d.account_id, thread = %d.thread_id, "snooze woke");
                    woke.push(Woke {
                        account_id: d.account_id.clone(),
                        thread_id: d.thread_id.clone(),
                        subject,
                        due_at: d.wake_at,
                    });
                }
                LocalWake::Dropped | LocalWake::NotSnoozed => continue,
            }
        }
        push(store, clients.get(&d.account_id), &d, now_ms).await?;
    }
    Ok(woke)
}

async fn push<A: SnoozeApi>(
    store: &Store,
    api: Option<&A>,
    d: &DueSnooze,
    now_ms: i64,
) -> Result<()> {
    let add: Vec<String> = WAKE_LABELS.iter().map(|l| l.to_string()).collect();
    let result = match api {
        Some(api) => api.modify_thread(&d.thread_id, &add, &[]).await,
        None => Err(Error::NeedsReauth(format!(
            "{} is not connected",
            d.account_id
        ))),
    };
    let (a, t) = (d.account_id.clone(), d.thread_id.clone());
    match result {
        Ok(()) => db(store, move |st| st.snooze_pushed(&a, &t)).await,
        // Deleted on the server: nothing to wake there; sync removes the local copy.
        Err(e) if e.is_not_found() => db(store, move |st| st.unsnooze(&a, &t).map(|_| ())).await,
        Err(e) => {
            tracing::warn!(account = %d.account_id, error = %e, "snooze wake not pushed to the provider; retrying");
            db(store, move |st| {
                st.retry_snooze_push(&a, &t, now_ms + PUSH_RETRY_MS)
            })
            .await
        }
    }
}

#[cfg(test)]
#[path = "snooze_tests.rs"]
mod tests;
