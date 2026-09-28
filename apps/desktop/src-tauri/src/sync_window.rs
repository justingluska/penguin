//! Sync-window commands (Settings → Sync, search coverage, "Also search
//! Gmail", free up space) and the lazy body fetch behind get_thread.
//! OWNER: sync-window agent. The logic lives with each provider (Gmail:
//! penguin-gmail's `sync_window.rs`) behind `MailProvider`; this is glue.
//! Types are mirrored in `apps/desktop/src/lib/types.ts`.

use std::collections::HashMap;
use std::sync::Arc;

use penguin_core::{Account, SearchHit};
use penguin_gmail::sync as gsync;
use penguin_provider::window::{self, WindowPolicy};
use serde::Serialize;
use tauri::State;

use crate::error::{CmdError, CmdResult};
use crate::ops;
use crate::state::{blocking, AppState};
use crate::views::BodyFetchFailedEvent;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Run one task per account concurrently and collect the results in order.
async fn join_all<T: Send + 'static>(
    futures: impl IntoIterator<Item = impl std::future::Future<Output = T> + Send + 'static>,
) -> Vec<T> {
    let handles: Vec<_> = futures
        .into_iter()
        .map(tauri::async_runtime::spawn)
        .collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        match h.await {
            Ok(v) => out.push(v),
            Err(e) => tracing::error!(error = %e, "sync-window task failed"),
        }
    }
    out
}

/// Share of the per-minute quota the sync paces at (api.rs TARGET_UTILIZATION).
const PACING: f64 = 0.95;
/// Measured Total Query Cost: messages.get format=full and format=metadata.
const FULL_GET_UNITS: f64 = 60.0;
const HEADERS_GET_UNITS: f64 = 20.0;
/// Bytes per message before anything is stored to measure it from.
const TYPICAL_MESSAGE_BYTES: u64 = 40_000;

/// The accounts a request covers: all, narrowed by `account_ids` (a profile).
async fn scoped_accounts(
    state: &AppState,
    account_ids: Option<&[String]>,
) -> CmdResult<Vec<Account>> {
    Ok(state
        .accounts()
        .await?
        .into_iter()
        .filter(|a| account_ids.is_none_or(|ids| ids.contains(&a.id)))
        .collect())
}

/// Messages per minute one account downloads at `units` per message, from
/// its learned quota (or the configured one before it has made a call).
fn per_minute(state: &AppState, email: &str, units: f64, full: bool) -> f64 {
    let stats = penguin_gmail::api::quota_stats(email);
    let budget = stats
        .as_ref()
        .map(|q| q.units_per_min)
        .unwrap_or_else(|| state.settings.get().gmail_units_per_min as f64);
    // A full get's learned cost tracks quota changes; headers are flat.
    let cost = match (&stats, full) {
        (Some(q), true) => q.get_cost,
        _ => units,
    };
    budget * PACING / cost.max(1.0)
}

fn eta_secs(messages: u64, per_min: f64) -> u64 {
    (messages as f64 / per_min.max(0.1) * 60.0).round() as u64
}

// ---------- estimates (Settings → Sync slider) ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowEstimate {
    pub account_id: String,
    pub months: u32,
    /// Gmail's estimate of messages in the window (0 months = the mailbox).
    pub in_window: u64,
    /// Of those, already stored with a full body.
    pub have_full: u64,
    /// Time to download the rest in full at this account's quota.
    pub eta_secs: u64,
    /// Messages older than the window (Gmail's estimate).
    pub older: u64,
    /// Time to store the older mail headers-only once the window is done.
    pub older_headers_eta_secs: u64,
    /// Time to download the older mail in full (olderMail = full).
    pub older_full_eta_secs: u64,
    /// Average bytes a message takes in the local database (file size ÷
    /// messages), for "uses about 1.1 GB" estimates.
    pub bytes_per_message: u64,
    /// Set when Gmail couldn't be asked (e.g. the account needs sign-in).
    pub error: Option<String>,
}

/// Per-account estimates for a window of `months` (0 = everything). One
/// messages.list call per account and window, cached for 30 minutes.
#[tauri::command]
pub async fn sync_window_estimate(
    state: AppStateRef<'_>,
    months: u32,
    account_ids: Option<Vec<String>>,
) -> CmdResult<Vec<WindowEstimate>> {
    if !window::WINDOW_MONTH_CHOICES.contains(&months) {
        return Err(CmdError::invalid(format!(
            "unsupported window: {months} months"
        )));
    }
    let st = state.inner().clone();
    let accounts = scoped_accounts(&st, account_ids.as_deref()).await?;
    let now = ops::now_ms();
    let start = window::window_start_ms(now, months);
    let store = st.store.clone();
    let stored = blocking(move || Ok(store.count_messages(None)?)).await?;
    // Nothing synced yet: a typical full message with its index rows.
    let bytes_per_message = db_bytes(&st)
        .checked_div(stored)
        .unwrap_or(TYPICAL_MESSAGE_BYTES);
    let futures = accounts.into_iter().map(|a| {
        let st = st.clone();
        async move {
            let mut out = WindowEstimate {
                account_id: a.id.clone(),
                months,
                in_window: 0,
                have_full: 0,
                eta_secs: 0,
                older: 0,
                older_headers_eta_secs: 0,
                older_full_eta_secs: 0,
                bytes_per_message,
                error: None,
            };
            let counts = async {
                let provider = st.provider(&a.id).await?;
                let in_window = provider.window_estimate(months, now).await?;
                let all = provider.window_estimate(0, now).await?;
                CmdResult::Ok((in_window, all))
            }
            .await;
            match counts {
                Ok((in_window, all)) => {
                    let store = st.store.clone();
                    let id = a.id.clone();
                    let have =
                        blocking(move || Ok(store.count_full_between(&id, start, None)?)).await?;
                    out.in_window = in_window;
                    out.have_full = have.min(in_window);
                    out.eta_secs = eta_secs(
                        in_window.saturating_sub(have),
                        per_minute(&st, &a.email, FULL_GET_UNITS, true),
                    );
                    out.older = all.saturating_sub(in_window);
                    out.older_headers_eta_secs = eta_secs(
                        out.older,
                        per_minute(&st, &a.email, HEADERS_GET_UNITS, false),
                    );
                    out.older_full_eta_secs =
                        eta_secs(out.older, per_minute(&st, &a.email, FULL_GET_UNITS, true));
                }
                Err(e) => out.error = Some(e.message),
            }
            CmdResult::Ok(out)
        }
    });
    join_all(futures).await.into_iter().collect()
}

// ---------- coverage (search footer, Settings, Diagnostics) ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCoverage {
    pub account_id: String,
    /// Every message since this (unix ms) has its full body; 0 = all mail;
    /// None = the first window download hasn't finished.
    pub full_since_ms: Option<i64>,
    /// The current window's download is complete.
    pub window_complete: bool,
    /// Older mail is done for the current `olderMail` (always true for none).
    pub older_complete: bool,
    pub full: u64,
    pub headers_only: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCoverage {
    pub window_months: u32,
    /// "headers" | "none" | "full"
    pub older_mail: crate::settings::OlderMail,
    /// Start of the current window (unix ms; 0 = everything).
    pub window_start_ms: i64,
    pub accounts: Vec<AccountCoverage>,
}

/// What's downloaded where, per account. Local reads only.
#[tauri::command]
pub async fn sync_coverage(
    state: AppStateRef<'_>,
    account_ids: Option<Vec<String>>,
) -> CmdResult<SyncCoverage> {
    let settings = state.settings.get();
    let accounts = scoped_accounts(&state, account_ids.as_deref()).await?;
    let store = state.store.clone();
    let policy = settings.window_policy();
    let window_start_ms = window::window_start_ms(ops::now_ms(), policy.months);
    let rows = blocking(move || {
        let counts: HashMap<String, penguin_core::store::BodyCoverage> = store
            .body_coverage()?
            .into_iter()
            .map(|c| (c.account_id.clone(), c))
            .collect();
        let mut out = Vec::new();
        for a in accounts {
            let cursor = store.get_sync_cursor(&a.id)?;
            let c = counts.get(&a.id).cloned().unwrap_or_default();
            out.push(coverage_row(&a.id, &cursor, window_start_ms, policy, c));
        }
        Ok(out)
    })
    .await?;
    Ok(SyncCoverage {
        window_months: settings.sync_window_months,
        older_mail: settings.older_mail,
        window_start_ms,
        accounts: rows,
    })
}

fn coverage_row(
    account_id: &str,
    cursor: &penguin_core::SyncCursor,
    window_start_ms: i64,
    policy: WindowPolicy,
    counts: penguin_core::store::BodyCoverage,
) -> AccountCoverage {
    let w = &cursor.window;
    // A pre-window mailbox that finished its backfill has everything.
    let full_since_ms = w
        .full_since_ms
        .or((cursor.backfill_done && w.fill_after_ms.is_none()).then_some(0));
    let window_complete = full_since_ms.is_some_and(|fs| fs <= window_start_ms);
    let mode = match policy.older {
        window::OlderMail::Headers => "headers",
        window::OlderMail::None => "none",
        window::OlderMail::Full => "full",
    };
    let older_complete = policy.older == window::OlderMail::None
        || full_since_ms == Some(0)
        || (w.older_done
            && w.older_before_ms == full_since_ms
            && w.older_mode.as_deref() == Some(mode));
    AccountCoverage {
        account_id: account_id.to_string(),
        full_since_ms,
        window_complete,
        older_complete: window_complete && older_complete,
        full: counts.full,
        headers_only: counts.headers_only,
    }
}

// ---------- free up space ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeUpSpace {
    /// Messages whose bodies were (or, for a dry run, would be) dropped.
    pub messages: u64,
    /// Stored (compressed) size of those bodies; the file shrinks by about
    /// this much plus their index entries.
    pub body_bytes: u64,
    /// Database file size before and after (equal for a dry run).
    pub bytes_before: u64,
    pub bytes_after: u64,
}

fn db_bytes(state: &AppState) -> u64 {
    let path = state.paths.db_path();
    let wal = path.with_extension("db-wal");
    [path, wal]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// Drop stored bodies of mail older than the current window (headers,
/// labels, snippets and attachment names stay searchable), then VACUUM so
/// the file really shrinks. `dry_run` only counts. `months` previews or
/// applies a window other than the saved one (the Settings confirm dialog);
/// by default it's the current setting. Refused for "everything".
#[tauri::command]
pub async fn free_up_space(
    state: AppStateRef<'_>,
    dry_run: bool,
    account_ids: Option<Vec<String>>,
    months: Option<u32>,
) -> CmdResult<FreeUpSpace> {
    let months = months.unwrap_or_else(|| state.settings.get().sync_window_months);
    if months != 0 && !window::WINDOW_MONTH_CHOICES.contains(&months) {
        return Err(CmdError::invalid(format!(
            "unsupported window: {months} months"
        )));
    }
    if months == 0 {
        return Err(CmdError::invalid(
            "The sync window is set to everything; choose a window first",
        ));
    }
    let before = window::window_start_ms(ops::now_ms(), months);
    let accounts = scoped_accounts(&state, account_ids.as_deref()).await?;
    let st = state.inner().clone();
    let bytes_before = db_bytes(&st);
    let (messages, body_bytes) = blocking(move || {
        let (mut n, mut body_bytes) = (0, 0);
        for a in &accounts {
            let (count, size) = st.store.full_bodies_before(&a.id, before)?;
            body_bytes += size;
            n += if dry_run {
                count
            } else {
                st.store.drop_bodies_before(&a.id, before)?
            };
        }
        if !dry_run && n > 0 {
            st.store.vacuum()?;
        }
        Ok((n, body_bytes))
    })
    .await?;
    let bytes_after = if dry_run {
        bytes_before
    } else {
        db_bytes(&state)
    };
    if !dry_run {
        tracing::info!(
            messages,
            bytes_before,
            bytes_after,
            "freed space: dropped bodies older than the sync window"
        );
    }
    Ok(FreeUpSpace {
        messages,
        body_bytes,
        bytes_before,
        bytes_after,
    })
}

// ---------- "Also search Gmail" ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSearchAccount {
    pub account_id: String,
    /// Gmail's estimate of all matches in this account.
    pub estimate: u64,
    /// Messages newly stored (headers-only) by this search.
    pub fetched: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSearchResponse {
    /// What was sent to Gmail.
    pub gmail_query: String,
    /// Parts of the query Gmail can't express were dropped (results may be
    /// broader than local search).
    pub approximate: bool,
    /// Newest first across accounts; they're stored locally now.
    pub hits: Vec<SearchHit>,
    pub accounts: Vec<ServerSearchAccount>,
}

/// Search each account's server (Gmail: messages.list, ~5 units), store up
/// to 50 unknown matches per account headers-only, and return every match
/// we now have. Emits mail-changed for the threads it added. Accounts whose
/// provider has no server search are left out.
#[tauri::command]
pub async fn search_server(
    state: AppStateRef<'_>,
    query: String,
    account_ids: Option<Vec<String>>,
) -> CmdResult<ServerSearchResponse> {
    let parsed = penguin_core::query::parse(&query, ops::now_ms());
    let (gmail_query, approximate) = gsync::to_gmail_query(&parsed);
    let mut accounts = scoped_accounts(&state, account_ids.as_deref()).await?;
    accounts.retain(|a| a.capabilities.server_search);
    if !parsed.accounts.is_empty() {
        accounts.retain(|a| {
            let email = a.email.to_lowercase();
            parsed
                .accounts
                .iter()
                .any(|p| email.starts_with(p.as_str()) || a.id.starts_with(p.as_str()))
        });
    }
    if gmail_query.trim().is_empty() || parsed.empty_range() {
        return Ok(ServerSearchResponse {
            gmail_query,
            approximate,
            hits: vec![],
            accounts: vec![],
        });
    }
    let st = state.inner().clone();
    let results = join_all(accounts.into_iter().map(|a| {
        let (st, q) = (st.clone(), parsed.clone());
        async move {
            let outcome = async {
                let provider = st.provider(&a.id).await?;
                let r = provider
                    .server_search(&q, penguin_provider::SERVER_SEARCH_FETCH_CAP)
                    .await?;
                CmdResult::Ok(r)
            }
            .await;
            (a.id, outcome)
        }
    }))
    .await;
    let mut hits = Vec::new();
    let mut per_account = Vec::new();
    for (account_id, outcome) in results {
        match outcome {
            Ok(r) => {
                if r.fetched > 0 {
                    let threads: Vec<String> = r.hits.iter().map(|h| h.thread_id.clone()).collect();
                    st.emit_mail_changed(&account_id, threads);
                }
                per_account.push(ServerSearchAccount {
                    account_id,
                    estimate: r.estimate,
                    fetched: r.fetched,
                    error: None,
                });
                hits.extend(r.hits);
            }
            Err(e) => per_account.push(ServerSearchAccount {
                account_id,
                estimate: 0,
                fetched: 0,
                error: Some(e.message),
            }),
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.date));
    tracing::info!(
        accounts = per_account.len(),
        hits = hits.len(),
        approximate,
        "server search"
    );
    Ok(ServerSearchResponse {
        gmail_query,
        approximate,
        hits,
        accounts: per_account,
    })
}

// ---------- lazy bodies ----------

/// Download the bodies of an opened thread's headers-only messages in the
/// background (interactive priority, one messages.get each), then emit
/// mail-changed so the UI re-renders. get_thread never waits on this.
pub fn fetch_pending_in_background(st: Arc<AppState>, account_id: &str, ids: Vec<String>) {
    let account_id = account_id.to_string();
    tauri::async_runtime::spawn(async move {
        let fetched = match st.provider(&account_id).await {
            Ok(provider) => provider
                .fetch_pending_bodies(&ids)
                .await
                .map_err(CmdError::from),
            Err(e) => Err(e),
        };
        match fetched {
            Ok(threads) if !threads.is_empty() => st.emit_mail_changed(&account_id, threads),
            Ok(_) => {}
            Err(e) => {
                // The thread keeps showing "Loading full message…" (and no
                // attachments) until a reopen retries, so say why.
                tracing::warn!(account = %account_id, messages = ?ids, code = ?e.code, error = %e.message, "could not download message bodies on open");
                st.emit_body_fetch_failed(BodyFetchFailedEvent {
                    account_id: account_id.clone(),
                    message_ids: ids,
                    message: e.message,
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use penguin_core::{SyncCursor, WindowCursor};

    fn counts() -> penguin_core::store::BodyCoverage {
        penguin_core::store::BodyCoverage {
            account_id: "a@x.example".into(),
            full: 10,
            headers_only: 5,
        }
    }

    #[test]
    fn coverage_rows_follow_the_cursor() {
        let policy = WindowPolicy::default();
        let start = 1_000_000;
        // First download still running.
        let fresh = coverage_row("a", &SyncCursor::default(), start, policy, counts());
        assert_eq!(
            (
                fresh.full_since_ms,
                fresh.window_complete,
                fresh.older_complete
            ),
            (None, false, false)
        );
        // Window done, older headers running.
        let mut c = SyncCursor {
            backfill_done: true,
            window: WindowCursor {
                full_since_ms: Some(start),
                older_before_ms: Some(start),
                ..Default::default()
            },
            ..Default::default()
        };
        let r = coverage_row("a", &c, start, policy, counts());
        assert!(r.window_complete && !r.older_complete);
        c.window.older_done = true;
        c.window.older_mode = Some("headers".into());
        assert!(coverage_row("a", &c, start, policy, counts()).older_complete);
        // A bigger window than what's downloaded.
        assert!(!coverage_row("a", &c, start - 1, policy, counts()).window_complete);
        // Legacy: backfill finished before the window existed.
        let legacy = SyncCursor {
            backfill_done: true,
            ..Default::default()
        };
        let r = coverage_row("a", &legacy, start, policy, counts());
        assert_eq!(r.full_since_ms, Some(0));
        assert!(r.window_complete && r.older_complete);
    }

    #[test]
    fn eta_math() {
        // 14,200 messages at 6,000 units/min, 60 per message: ~2.5 h.
        let per_min = 6_000.0 * PACING / FULL_GET_UNITS;
        let h = eta_secs(14_200, per_min) as f64 / 3600.0;
        assert!((2.4..2.6).contains(&h), "{h}");
    }
}
