//! Smart views (store side: penguin-core `store_smart.rs`): the header
//! figures and the sidebar counts. The lists themselves go through
//! `list_threads` (`MailboxView::Smart` / `MailboxView::Query`). Also the
//! Split Inbox's tab counts (penguin-core `store_split.rs`; its lists are
//! `list_threads` with `ListQuery.split`). All local.

use std::sync::Arc;

use chrono::{Local, Offset};
use penguin_core::{AccountId, SmartCount, SmartViewInfo, SplitCounts};
use tauri::State;

use crate::error::CmdResult;
use crate::ops;
use crate::state::{blocking, AppState};

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Views counted per `smart_counts` call, at most.
const MAX_COUNTED: usize = 40;
/// Splits counted per `split_counts` call, at most (the UI allows 12).
const MAX_SPLITS: usize = 20;

/// This Mac's UTC offset now: what "today" means for the views.
fn utc_offset_secs() -> i32 {
    Local::now().offset().fix().local_minus_utc()
}

/// The slim header over a smart view (`view` = "receipts", "bills", …).
#[tauri::command]
pub async fn smart_view_info(
    state: AppStateRef<'_>,
    view: String,
    account_ids: Option<Vec<AccountId>>,
) -> CmdResult<SmartViewInfo> {
    let store = state.store.clone();
    blocking(move || {
        Ok(store.smart_view_info(
            &view,
            account_ids.as_deref(),
            ops::now_ms(),
            utc_offset_secs(),
        )?)
    })
    .await
}

/// Sidebar counts for the views that show one (`query:<q>` for a saved
/// search).
#[tauri::command]
pub async fn smart_counts(
    state: AppStateRef<'_>,
    views: Vec<String>,
    account_ids: Option<Vec<AccountId>>,
) -> CmdResult<Vec<SmartCount>> {
    let store = state.store.clone();
    let views: Vec<String> = views.into_iter().take(MAX_COUNTED).collect();
    blocking(move || {
        Ok(store.smart_counts(
            &views,
            account_ids.as_deref(),
            ops::now_ms(),
            utc_offset_secs(),
        )?)
    })
    .await
}

/// Split Inbox: conversations (and unread ones) in each split, in order,
/// then Other. `queries` are the splits' queries in tab order.
#[tauri::command]
pub async fn split_counts(
    state: AppStateRef<'_>,
    queries: Vec<String>,
    account_ids: Option<Vec<AccountId>>,
) -> CmdResult<SplitCounts> {
    let store = state.store.clone();
    let queries: Vec<String> = queries.into_iter().take(MAX_SPLITS).collect();
    blocking(move || Ok(store.split_counts(&queries, account_ids.as_deref())?)).await
}
