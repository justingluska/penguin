//! Tauri commands: the backend half of the contract in docs/ARCHITECTURE.md
//! (the other half is apps/desktop/src/lib/api.ts). Thin glue — every Store
//! call runs on the blocking pool, and nothing a read command returns waits
//! on the network. Mailbox calls go through the account's provider
//! (`AppState::provider`), whichever backend serves it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use penguin_core::{
    Account, AccountProvider, Label, ListQuery, SearchRequest, SearchResponse, SyncStatus,
    ThreadSummary,
};
use penguin_gmail::auth::{OAuthClientConfig, SignedInAccount};
use penguin_provider::compose::Draft;
use penguin_provider::{DraftRef, LabelColor, LabelUpdate, OpenedDraft};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::attachments::{self, AttachmentPreview};
use crate::diagnostics::{self as diag, AccountDiagnostics, Diagnostics, RevealTarget, TableSizes};
use crate::error::{CmdError, CmdResult};
use crate::message_details;
use crate::ops;
use crate::outgoing;
use crate::settings::{RemoteImages, Settings, SettingsPatch};
use crate::state::{blocking, AppState};
use crate::views::{
    message_view, thread_view, MessageView, OAuthClientStatus, ThreadAction, ThreadRef, ThreadView,
};

type AppStateRef<'a> = State<'a, Arc<AppState>>;

// ---------- setup / accounts ----------

fn client_status(state: &AppState) -> OAuthClientStatus {
    // From the live AuthManager when configured, else from disk (saved
    // before the desktop client existed; install_services picks it up).
    let ios_client_id = match state.services() {
        Ok(s) => s.auth.ios_client().map(|c| c.client_id),
        Err(_) => state.ios_client().map(|c| c.client_id),
    };
    OAuthClientStatus {
        configured: state.is_configured(),
        path: state.paths.oauth_client_path().display().to_string(),
        ios_client_id,
    }
}

/// Save the iOS OAuth client (bare id, Google's .plist, or JSON) that enables
/// the macOS system sign-in sheet.
#[tauri::command]
pub async fn set_ios_oauth_client(
    state: AppStateRef<'_>,
    input: String,
) -> CmdResult<OAuthClientStatus> {
    let dir = state.paths.config_dir.clone();
    let config = blocking(move || {
        penguin_gmail::auth::IosClientConfig::save(&dir, &input).map_err(|e| match e {
            // Parse/validation problems are the user's input, not setup state.
            penguin_gmail::Error::NotConfigured(m) => CmdError::invalid(m),
            other => other.into(),
        })
    })
    .await?;
    if let Ok(services) = state.services() {
        services.auth.set_ios_client(Some(config));
    }
    tracing::info!("iOS OAuth client configured (system sign-in sheet on)");
    Ok(client_status(&state))
}

#[tauri::command]
pub async fn clear_ios_oauth_client(state: AppStateRef<'_>) -> CmdResult<OAuthClientStatus> {
    let dir = state.paths.config_dir.clone();
    blocking(move || Ok(penguin_gmail::auth::IosClientConfig::remove(&dir)?)).await?;
    if let Ok(services) = state.services() {
        services.auth.set_ios_client(None);
    }
    tracing::info!("iOS OAuth client removed (browser sign-in)");
    Ok(client_status(&state))
}

#[tauri::command]
pub async fn oauth_client_status(state: AppStateRef<'_>) -> CmdResult<OAuthClientStatus> {
    Ok(client_status(&state))
}

#[tauri::command]
pub async fn set_oauth_client(
    state: AppStateRef<'_>,
    json: String,
) -> CmdResult<OAuthClientStatus> {
    OAuthClientConfig::from_json(&json).map_err(|e| CmdError::invalid(e.to_string()))?;
    let dir = state.paths.config_dir.clone();
    let config = blocking(move || Ok(OAuthClientConfig::save(&dir, &json)?)).await?;
    state.install_services(config);
    tracing::info!("Google OAuth client configured");
    state.inner().start_all().await;
    Ok(client_status(&state))
}

#[tauri::command]
pub async fn list_accounts(state: AppStateRef<'_>) -> CmdResult<Vec<Account>> {
    state.accounts().await
}

/// Interactive sign-in for add/reconnect (see sign_in.rs), cancellable
/// through cancel_sign_in. `extra_scopes` are the optional calendar scopes
/// (`calendar::commands::sign_in_scopes`); Gmail alone is enough to succeed.
async fn browser_sign_in(
    app: &AppHandle,
    state: &AppState,
    hint: Option<&str>,
    extra_scopes: &[&str],
) -> CmdResult<SignedInAccount> {
    crate::sign_in::interactive_sign_in(app, state, hint, extra_scopes).await
}

type CalendarRef<'a> = State<'a, Arc<crate::calendar::commands::Calendar>>;

/// Sign in a new account (mail, plus read-only calendar in the same Google
/// consent when `calendar.connectOnSignIn` is on), then start its sync.
/// `login_hint`: the address typed in Add account, preselected by Google.
#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    state: AppStateRef<'_>,
    calendar: CalendarRef<'_>,
    login_hint: Option<String>,
) -> CmdResult<Account> {
    let hint = login_hint
        .as_deref()
        .map(str::trim)
        .filter(|h| h.contains('@') && h.len() <= 320);
    let calendar_scopes = crate::calendar::commands::sign_in_scopes(&state, None).await;
    let signed = browser_sign_in(&app, &state, hint, &calendar_scopes).await?;
    let existing = state.accounts().await?;
    // The same address connected through IMAP or Microsoft stays that
    // account; the Google tokens just minted for it are discarded.
    if let Some(other) = existing.iter().find(|a| {
        a.email.eq_ignore_ascii_case(&signed.email) && a.provider != AccountProvider::Gmail
    }) {
        if let Ok(services) = state.services() {
            if let Err(e) = services.auth.sign_out(&signed.email).await {
                tracing::warn!(error = %e, "could not discard credentials from a duplicate sign-in");
            }
        }
        return Err(CmdError::invalid(format!(
            "{} is already in Penguin as {} account",
            other.email,
            match other.provider {
                AccountProvider::Imap => "an IMAP",
                _ => "a Microsoft",
            }
        )));
    }
    let account = ops::account_for_sign_in(&signed, &existing, ops::now_ms());
    let store = state.store.clone();
    let record = account.clone();
    blocking(move || Ok(store.upsert_account(&record)?)).await?;
    tracing::info!(account = %account.id, "account signed in");
    state.inner().start_account(&account).await;
    crate::calendar::commands::after_sign_in(
        &app,
        state.inner(),
        calendar.inner(),
        &account,
        &calendar_scopes,
    )
    .await;
    Ok(account)
}

/// Sign an existing account in again (after NeedsReauth) and restart its
/// sync. Signing in as a different Google account is refused; if that other
/// account is also in Penguin its fresh credentials are kept (and its sync
/// restarted), otherwise they are discarded. Asks for the same calendar
/// scopes as add_account, plus any calendar access the account already had.
#[tauri::command]
pub async fn reconnect_account(
    app: AppHandle,
    state: AppStateRef<'_>,
    calendar: CalendarRef<'_>,
    account_id: String,
) -> CmdResult<Account> {
    let account = state.account(&account_id).await?;
    if account.provider != AccountProvider::Gmail {
        return crate::providers::connect::reconnect(&app, state.inner(), account).await;
    }
    let calendar_scopes = crate::calendar::commands::sign_in_scopes(&state, Some(&account)).await;
    let signed = browser_sign_in(&app, &state, Some(&account.email), &calendar_scopes).await?;
    if !signed.email.eq_ignore_ascii_case(&account.email) {
        let existing = state.accounts().await?;
        match existing
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(&signed.email))
        {
            Some(other) => state.inner().start_account(other).await,
            None => {
                if let Ok(services) = state.services() {
                    if let Err(e) = services.auth.sign_out(&signed.email).await {
                        tracing::warn!(error = %e, "could not discard credentials from a mismatched sign-in");
                    }
                }
            }
        }
        return Err(CmdError::invalid(format!(
            "You signed in as {}; choose {} instead",
            signed.email, account.email
        )));
    }
    let updated = ops::account_for_sign_in(&signed, &[account], ops::now_ms());
    let store = state.store.clone();
    let record = updated.clone();
    blocking(move || Ok(store.upsert_account(&record)?)).await?;
    tracing::info!(account = %updated.id, "account reconnected");
    // Restart sync: a task that stopped on NeedsReauth is replaced.
    state.stop_account(&updated.id);
    state.inner().start_account(&updated).await;
    crate::calendar::commands::after_sign_in(
        &app,
        state.inner(),
        calendar.inner(),
        &updated,
        &calendar_scopes,
    )
    .await;
    Ok(updated)
}

/// Abandon the browser sign-in in flight; its command rejects with `cancelled`.
#[tauri::command]
pub async fn cancel_sign_in(state: AppStateRef<'_>) -> CmdResult<bool> {
    Ok(state.cancel_sign_in())
}

#[tauri::command]
pub async fn remove_account(
    state: AppStateRef<'_>,
    calendar: CalendarRef<'_>,
    account_id: String,
) -> CmdResult<()> {
    let account = state.account(&account_id).await?;
    state.forget_account(&account_id);
    // Before sign-out revokes the tokens a calendar sync might be using.
    calendar.forget_account(&account);
    if let Some(backend) = state.registry.get(account.provider) {
        // Best effort: local data is removed even if the provider can't be reached.
        if let Err(e) = backend.sign_out(&account).await {
            tracing::warn!(account = %account_id, error = %e, "sign-out failed; removing local data anyway");
        }
    }
    let store = state.store.clone();
    let id = account_id.clone();
    blocking(move || Ok(store.remove_account(&id)?)).await?;
    let semantic = state.semantic_indexer.clone();
    let id = account_id.clone();
    blocking(move || {
        semantic.remove_account(&id);
        Ok(())
    })
    .await?;
    state.inline.remove_account(&account_id);
    attachments::remove_account(&state.paths, &account_id);
    tracing::info!(account = %account_id, "account removed");
    Ok(())
}

/// `account_ids` (a profile) limits the statuses to those accounts.
#[tauri::command]
pub async fn sync_status(
    state: AppStateRef<'_>,
    account_ids: Option<Vec<String>>,
) -> CmdResult<Vec<SyncStatus>> {
    let store = state.store.clone();
    let (accounts, counts) = blocking(move || {
        let mut accounts = store.list_accounts()?;
        if let Some(ids) = &account_ids {
            accounts.retain(|a| ids.contains(&a.id));
        }
        let mut counts = HashMap::new();
        for a in &accounts {
            counts.insert(a.id.clone(), store.count_messages(Some(&a.id))?);
        }
        Ok((accounts, counts))
    })
    .await?;
    Ok(state.statuses(&accounts, &counts))
}

/// "Retry sync" on an account in error: see AppState::retry_account.
#[tauri::command]
pub async fn retry_account_sync(state: AppStateRef<'_>, account_id: String) -> CmdResult<()> {
    let account = state.account(&account_id).await?;
    state.inner().retry_account(&account).await
}

/// Set an account's nickname and/or color (Settings → Accounts). Local only;
/// the nickname is display-only and never reaches mail headers.
#[tauri::command]
pub async fn update_account(
    state: AppStateRef<'_>,
    account_id: String,
    patch: ops::AccountPatch,
) -> CmdResult<Account> {
    let (nickname, color) = ops::validate_account_patch(patch).map_err(CmdError::invalid)?;
    let store = state.store.clone();
    let account = blocking(move || {
        Ok(store.update_account(
            &account_id,
            nickname.as_ref().map(|n| n.as_deref()),
            color.as_deref(),
        )?)
    })
    .await?;
    tracing::info!(account = %account.id, "account updated");
    Ok(account)
}

#[tauri::command]
pub async fn sync_now(state: AppStateRef<'_>) -> CmdResult<()> {
    state.poke_all();
    Ok(())
}

// ---------- reading ----------

/// `account_id` and `account_ids` (a profile) intersect, as in ListQuery.
#[tauri::command]
pub async fn list_labels(
    state: AppStateRef<'_>,
    account_id: Option<String>,
    account_ids: Option<Vec<String>>,
) -> CmdResult<Vec<Label>> {
    let store = state.store.clone();
    let scope = penguin_core::account_scope(account_id.as_deref(), account_ids.as_deref());
    blocking(move || Ok(store.list_labels_in(scope.as_deref())?)).await
}

/// The account's label `label_id` from the local store, if it's a user
/// label. System labels (INBOX, CATEGORY_*, …) can't be edited or deleted.
async fn local_user_label(state: &AppState, account_id: &str, label_id: &str) -> CmdResult<Label> {
    let store = state.store.clone();
    let (account, id) = (account_id.to_string(), label_id.to_string());
    let label = blocking(move || {
        Ok(store
            .list_labels(Some(&account))?
            .into_iter()
            .find(|l| l.id == id))
    })
    .await?
    .ok_or_else(|| CmdError::not_found(format!("unknown label {label_id}")))?;
    if label.kind != "user" {
        return Err(CmdError::invalid("System labels can't be changed."));
    }
    Ok(label)
}

/// Rename / recolor / hide a user label (Gmail labels.patch; the provider
/// turns a taken or reserved name into invalidInput), then write it to the
/// local store and emit mail-changed (the UI reloads labels on it).
#[tauri::command]
pub async fn update_label(
    state: AppStateRef<'_>,
    account_id: String,
    label_id: String,
    patch: ops::LabelPatch,
) -> CmdResult<Label> {
    let update = ops::validate_label_patch(patch).map_err(CmdError::invalid)?;
    let current = local_user_label(&state, &account_id, &label_id).await?;
    if update.is_empty() {
        return Ok(current);
    }
    let provider = state.provider(&account_id).await?;
    let change = LabelUpdate {
        name: update.name.clone(),
        color: update.color.map(|c| {
            c.map(|(background, text)| LabelColor {
                background: background.to_string(),
                text: text.to_string(),
            })
        }),
        hidden: update.hidden,
    };
    let mut label = provider.update_label(&label_id, &change).await?;
    label.account_id = account_id.clone();
    // Gmail's counts are server-side; the list shows the local one.
    label.unread_count = current.unread_count;
    let store = state.store.clone();
    let stored = label.clone();
    blocking(move || Ok(store.upsert_label(&stored)?)).await?;
    tracing::info!(account = %account_id, "label updated");
    state.emit_mail_changed(&account_id, Vec::new());
    Ok(label)
}

/// Delete a user label in Gmail (it disappears from every message), then
/// drop it locally — the row and its id on every message/thread — and emit
/// mail-changed for the touched threads.
#[tauri::command]
pub async fn delete_label(
    state: AppStateRef<'_>,
    account_id: String,
    label_id: String,
) -> CmdResult<()> {
    local_user_label(&state, &account_id, &label_id).await?;
    let provider = state.provider(&account_id).await?;
    provider.delete_label(&label_id).await?;
    let store = state.store.clone();
    let (acct, id) = (account_id.clone(), label_id.clone());
    let threads = blocking(move || Ok(store.delete_label(&acct, &id)?)).await?;
    tracing::info!(account = %account_id, threads = threads.len(), "label deleted");
    state.emit_mail_changed(&account_id, threads);
    Ok(())
}

#[tauri::command]
pub async fn list_threads(
    state: AppStateRef<'_>,
    query: ListQuery,
) -> CmdResult<Vec<ThreadSummary>> {
    if query.view == penguin_core::MailboxView::FollowUp {
        return crate::reply_later::list_follow_ups(&state, query).await;
    }
    let store = state.store.clone();
    let rows = blocking(move || {
        let mut rows = store.list_threads(&query)?;
        // Event chips (calendar::invites): a lookup per row with attachments.
        store.attach_invites(&mut rows, crate::ops::now_ms())?;
        Ok(rows)
    })
    .await?;
    crate::startup::mark("first rows");
    Ok(rows)
}

#[tauri::command]
pub async fn get_thread(
    state: AppStateRef<'_>,
    account_id: String,
    thread_id: String,
) -> CmdResult<Option<ThreadView>> {
    let st = state.inner().clone();
    let (acct, tid) = (account_id.clone(), thread_id.clone());
    let settings = state.settings.get();
    let rendered = blocking(move || {
        let Some(detail) = st.store.get_thread(&acct, &tid)? else {
            return Ok(None);
        };
        let mut cid_maps = HashMap::new();
        let mut missing = Vec::new();
        for m in &detail.messages {
            let (map, need) = st.inline.cached(m);
            cid_maps.insert(m.id.clone(), map);
            if !need.is_empty() {
                missing.push((m.id.clone(), need));
            }
        }
        let sent = crate::receipts::sent_ids(&detail.messages);
        let mut view = thread_view(
            detail,
            cid_maps,
            |m| settings.remote_images_for(m),
            settings.protection(),
        );
        // "Read by …" on messages you sent; a problem here never hides the thread.
        if let Err(e) = crate::receipts::annotate(&st.store, &acct, &sent, &mut view.messages) {
            tracing::warn!(error = %e, "receipts: couldn't read receipts for a thread");
        }
        for m in &view.messages {
            st.diag
                .record_trackers(&m.account_id, &m.id, m.trackers_removed);
        }
        let pending = st.store.pending_message_ids(&acct, &tid)?;
        for m in &mut view.messages {
            m.body_pending = pending.contains(&m.id);
        }
        crate::unsubscribe::annotate(&st.store, &mut view.messages)?;
        Ok(Some((view, missing, pending)))
    })
    .await?;
    let Some((view, missing, pending)) = rendered else {
        return Ok(None);
    };
    if !pending.is_empty() {
        crate::sync_window::fetch_pending_in_background(
            state.inner().clone(),
            &account_id,
            pending,
        );
    }
    if !missing.is_empty() {
        let st = state.inner().clone();
        tauri::async_runtime::spawn(async move {
            let Ok(provider) = st.provider(&account_id).await else {
                return;
            };
            let mut fetched = false;
            for (message_id, need) in missing {
                fetched |= st
                    .inline
                    .fetch(provider.as_ref(), &account_id, &message_id, &need)
                    .await;
            }
            if fetched {
                st.emit_mail_changed(&account_id, vec![thread_id]);
            }
        });
    }
    Ok(Some(view))
}

#[tauri::command]
pub async fn load_remote_images(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
) -> CmdResult<MessageView> {
    let settings = state.settings.get();
    if settings.remote_images == RemoteImages::Never {
        return Err(CmdError::invalid(
            "Remote images are turned off in Settings",
        ));
    }
    let protection = settings.protection();
    let st = state.inner().clone();
    blocking(move || {
        let m = st
            .store
            .get_message(&account_id, &message_id)?
            .ok_or_else(|| CmdError::not_found("message not found"))?;
        let (cids, _) = st.inline.cached(&m);
        let sent = crate::receipts::sent_ids(std::slice::from_ref(&m));
        let mut view = [message_view(m, true, protection, cids)];
        if let Err(e) = crate::receipts::annotate(&st.store, &account_id, &sent, &mut view) {
            tracing::warn!(error = %e, "receipts: couldn't read receipts for a message");
        }
        crate::unsubscribe::annotate(&st.store, &mut view)?;
        let [view] = view;
        Ok(view)
    })
    .await
}

// ---------- acting ----------

/// Archive, label, star, trash… `targets` (the optimistic action path in
/// actions.rs, shared with snooze, Reply Later, rules and agents).
#[tauri::command]
pub async fn modify_threads(
    state: AppStateRef<'_>,
    targets: Vec<ThreadRef>,
    action: ThreadAction,
) -> CmdResult<()> {
    apply_thread_action(state.inner().clone(), targets, action).await
}

/// `actions::apply` for callers that don't need the outcome (the push runs
/// on in the background).
pub async fn apply_thread_action(
    state: Arc<AppState>,
    targets: Vec<ThreadRef>,
    action: ThreadAction,
) -> CmdResult<()> {
    crate::actions::apply(state, targets, action)
        .await
        .map(|_| ())
}

/// Settings → Privacy "Ask for read receipts" decides for a draft whose
/// sender didn't (the composer leaves it unset). Saved drafts carry the
/// choice too, so a scheduled send (which sends the saved draft) keeps it.
pub fn with_receipt_choice(mut draft: Draft, ask_by_default: bool) -> Draft {
    if draft.request_read_receipt.is_none() {
        draft.request_read_receipt = Some(ask_by_default);
    }
    draft
}

#[tauri::command]
pub async fn send_message(
    state: AppStateRef<'_>,
    draft: Draft,
    draft_id: Option<String>,
) -> CmdResult<penguin_core::SentRef> {
    if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
        return Err(CmdError::invalid("Add at least one recipient"));
    }
    let draft = with_receipt_choice(draft, state.settings.get().request_read_receipts);
    let account = state.account(&draft.account_id).await?;
    let from = ops::from_address(&account);
    let provider = state.provider(&account.id).await?;
    let others = other_accounts(&state, &draft).await?;
    let sent = if let Some(draft_id) = draft_id {
        // Serialized with autosaves of the same draft.
        let _serial = state.drafts_lock.lock().await;
        let sent = outgoing::send(
            &state.store,
            &state.paths,
            provider.as_ref(),
            &others,
            &from,
            &draft,
            Some(&draft_id),
        )
        .await
        .inspect_err(|e| send_failed(&account.id, e))?;
        forget_schedules(&state, &account.id, &draft_id).await;
        state.emit_mail_changed(&account.id, vec![sent.thread_id.clone()]);
        sent
    } else {
        outgoing::send(
            &state.store,
            &state.paths,
            provider.as_ref(),
            &others,
            &from,
            &draft,
            None,
        )
        .await
        .inspect_err(|e| send_failed(&account.id, e))?
    };
    // A reply takes its conversation out of Reply Later.
    let mut replied = vec![sent.thread_id.clone()];
    replied.extend(draft.reply_to_thread_id.clone());
    crate::reply_later::after_send(state.inner().clone(), account.id.clone(), replied);
    // The sent copy arrives through sync; ask for it now so Sent and the
    // thread update within a moment.
    state.poke(&account.id);
    Ok(sent)
}

/// Providers for the other accounts `draft`'s attachments are stored in (a
/// forward sent from another account).
async fn other_accounts(state: &AppState, draft: &Draft) -> CmdResult<outgoing::OtherAccounts> {
    let mut out = outgoing::OtherAccounts::new();
    for a in &draft.attachments {
        if let penguin_provider::compose::OutgoingAttachment::Gmail {
            account_id: Some(acct),
            ..
        } = a
        {
            if *acct != draft.account_id && !out.contains_key(acct) {
                if let Ok(p) = state.provider(acct).await {
                    out.insert(acct.clone(), p);
                }
            }
        }
    }
    Ok(out)
}

/// A send that failed is logged (ids and the error only); the UI shows it
/// and keeps the draft.
fn send_failed(account_id: &str, e: &CmdError) {
    tracing::warn!(account = %account_id, code = ?e.code, error = %e.message, "send failed");
}

/// A draft sent by hand or discarded must not also fire from the send-later
/// schedule. Best effort: the send/delete already happened.
async fn forget_schedules(state: &AppState, account_id: &str, draft_id: &str) {
    let (store, a, d) = (
        state.store.clone(),
        account_id.to_string(),
        draft_id.to_string(),
    );
    match blocking(move || Ok(store.delete_scheduled_sends_for_draft(&a, &d)?)).await {
        Ok(0) => {}
        Ok(_) => crate::outbox::schedule_changed(),
        Err(e) => {
            tracing::warn!(account = %account_id, error = %e, "could not clear the draft's schedule")
        }
    }
}

/// The originals a reply or forward quotes, in order: sanitized HTML and
/// text (with `withBody`) and attachments. Headers-only messages (older than
/// the sync window) are downloaded first, so a forward never goes out
/// without attachments it couldn't see yet; everything else is local.
#[tauri::command]
pub async fn quote_sources(
    state: AppStateRef<'_>,
    account_id: String,
    message_ids: Vec<String>,
    with_body: bool,
) -> CmdResult<Vec<outgoing::QuoteSource>> {
    let provider = state.provider(&account_id).await.ok();
    let (sources, fetched) = outgoing::quote_sources(
        &state.store,
        provider.as_deref(),
        &account_id,
        &message_ids,
        with_body,
    )
    .await?;
    if !fetched.is_empty() {
        state.emit_mail_changed(&account_id, fetched);
    }
    Ok(sources)
}

// ---------- drafts ----------

/// Create (`draftId` null) or update a server draft. The saved draft is
/// mirrored locally (DRAFT label) so the Drafts view shows it at once.
/// Callers must wait for a create to return before saving again, or they
/// will create a second draft.
#[tauri::command]
pub async fn save_draft(
    state: AppStateRef<'_>,
    draft: Draft,
    draft_id: Option<String>,
) -> CmdResult<DraftRef> {
    let draft = with_receipt_choice(draft, state.settings.get().request_read_receipts);
    let account = state.account(&draft.account_id).await?;
    let from = ops::from_address(&account);
    let provider = state.provider(&account.id).await?;
    let others = other_accounts(&state, &draft).await?;
    // Serialized so autosaves can't land out of order.
    let _serial = state.drafts_lock.lock().await;
    let saved = outgoing::save_draft(
        &state.paths,
        provider.as_ref(),
        &others,
        &from,
        &draft,
        draft_id.as_deref(),
    )
    .await
    .inspect_err(|e| {
        tracing::warn!(account = %account.id, code = ?e.code, error = %e.message, "draft save failed")
    })?;
    state.emit_mail_changed(&account.id, vec![saved.thread_id.clone()]);
    Ok(saved)
}

#[tauri::command]
pub async fn delete_draft(
    state: AppStateRef<'_>,
    account_id: String,
    draft_id: String,
) -> CmdResult<()> {
    let provider = state.provider(&account_id).await?;
    let _serial = state.drafts_lock.lock().await;
    let thread = provider.delete_draft(&draft_id).await?;
    forget_schedules(&state, &account_id, &draft_id).await;
    if let Some(thread) = thread {
        state.emit_mail_changed(&account_id, vec![thread]);
    }
    Ok(())
}

/// Reopen a draft by `draftId` or by its `messageId` (Drafts view). Null if
/// the server no longer has it. `draft.bodyHtml` is always null: stored HTML is
/// untrusted, so drafts reopen as plain text.
#[tauri::command]
pub async fn get_draft(
    state: AppStateRef<'_>,
    account_id: String,
    draft_id: Option<String>,
    message_id: Option<String>,
) -> CmdResult<Option<OpenedDraft>> {
    if draft_id.is_none() && message_id.is_none() {
        return Err(CmdError::invalid(
            "get_draft needs a draftId or a messageId",
        ));
    }
    let provider = state.provider(&account_id).await?;
    Ok(provider
        .open_draft(draft_id.as_deref(), message_id.as_deref())
        .await?)
}

// ---------- search ----------

#[tauri::command]
pub async fn search(state: AppStateRef<'_>, request: SearchRequest) -> CmdResult<SearchResponse> {
    let started = Instant::now();
    let store = state.store.clone();
    // Hybrid when search by meaning is set up and its index is complete;
    // otherwise exactly the keyword search (the response says which).
    let semantic = state.semantic();
    let mut response =
        blocking(move || Ok(store.search_hybrid(&request, semantic.as_ref())?)).await?;
    response.took_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(response)
}

// ---------- misc ----------

/// "Gmail" stays "Gmail"; "the mail server" starts a sentence as "The mail server".
pub(crate) fn cap_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Only web and mail links leave the app; anything else (file:, javascript:,
/// custom schemes) is refused.
pub fn check_external_url(url: &str) -> CmdResult<tauri::Url> {
    let parsed =
        tauri::Url::parse(url.trim()).map_err(|_| CmdError::invalid("Not a valid link"))?;
    match parsed.scheme() {
        "http" | "https" | "mailto" => Ok(parsed),
        other => Err(CmdError::invalid(format!(
            "Refusing to open {other}: links"
        ))),
    }
}

#[tauri::command]
pub async fn save_attachment(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
    attachment_id: String,
) -> CmdResult<String> {
    let saved = async {
        let (attachment, bytes) =
            attachment_bytes(&state, &account_id, &message_id, &attachment_id).await?;
        let dir = dirs::download_dir()
            .ok_or_else(|| CmdError::other("Couldn't find your Downloads folder"))?;
        let name = ops::sanitize_filename(&attachment.filename);
        blocking(move || {
            ops::write_unique(&dir, &name, &bytes)
                .map_err(|e| CmdError::other(format!("Couldn't write the file to Downloads: {e}")))
        })
        .await
    }
    .await;
    let path = saved
        .inspect_err(|e| attachment_failed("save", &account_id, &message_id, &attachment_id, e))?;
    tracing::info!(account = %account_id, message = %message_id, "attachment saved");
    state.remember_saved_path(path.clone());
    Ok(path.display().to_string())
}

/// An attachment's metadata and bytes (from the cache a preview filled, else
/// fetched): Save, Save As and the drag-out file.
pub(crate) async fn attachment_bytes(
    state: &AppState,
    account_id: &str,
    message_id: &str,
    attachment_id: &str,
) -> CmdResult<(penguin_core::AttachmentMeta, Vec<u8>)> {
    let attachment = find_attachment(state, account_id, message_id, attachment_id).await?;
    let provider = state.provider(account_id).await?;
    // Reuses bytes a preview already fetched.
    let bytes = attachments::bytes(
        &state.paths,
        provider.as_ref(),
        account_id,
        message_id,
        &attachment,
    )
    .await?;
    Ok((attachment, bytes))
}

/// The stored attachment a Download/Preview names (see attachments::find).
async fn find_attachment(
    state: &AppState,
    account_id: &str,
    message_id: &str,
    attachment_id: &str,
) -> CmdResult<penguin_core::AttachmentMeta> {
    let store = state.store.clone();
    let (acct, mid, aid) = (
        account_id.to_string(),
        message_id.to_string(),
        attachment_id.to_string(),
    );
    blocking(move || attachments::find(&store, &acct, &mid, &aid)).await
}

/// Every failed Download/Preview lands in the log with its ids and error
/// (never the file name, subject or body).
fn attachment_failed(
    action: &str,
    account_id: &str,
    message_id: &str,
    attachment_id: &str,
    e: &CmdError,
) {
    tracing::warn!(
        account = %account_id,
        message = %message_id,
        attachment = %attachment_id,
        code = ?e.code,
        error = %e.message,
        "attachment {action} failed"
    );
}

/// "Message details": stored fields plus Gmail's authentication and transport
/// verdicts from a format=metadata fetch (interactive, ~20 units). Offline,
/// the stored half comes back with `headersFetched: false`.
#[tauri::command]
pub async fn get_message_details(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
) -> CmdResult<message_details::MessageDetails> {
    let store = state.store.clone();
    let (acct, mid) = (account_id.clone(), message_id.clone());
    let message = blocking(move || Ok(store.get_message(&acct, &mid)?))
        .await?
        .ok_or_else(|| CmdError::not_found("message not found"))?;
    // The stored half is still useful offline, so a failed fetch is reported
    // in the result (headersError) rather than failing the command.
    let (fetched, error) = match state.provider(&account_id).await {
        Ok(provider) => match provider.get_message_metadata(&message_id).await {
            Ok(Some(meta)) => (Some(meta), None),
            Ok(None) => (
                None,
                Some(format!(
                    "{} no longer has this message",
                    cap_first(provider.provider().service_name())
                )),
            ),
            Err(e) => (None, Some(CmdError::from(e).message)),
        },
        Err(e) => (None, Some(e.message)),
    };
    if let Some(e) = &error {
        tracing::warn!(account = %account_id, error = %e, "message details: header fetch failed");
    }
    if let Some(f) = &fetched {
        // Headers are here anyway: keep List-Unsubscribe-Post for mail
        // stored before Penguin kept it (the Unsubscribe button's one-click).
        crate::unsubscribe::remember_post_flag(&state, &message, &f.headers).await?;
    }
    let mut details = message_details::build(
        &message,
        fetched
            .as_ref()
            .map(|f| (f.headers.as_slice(), f.size_estimate)),
    );
    details.headers_error = error;
    Ok(details)
}

/// "Show original": the raw RFC 822 source (format=raw, fetched on demand),
/// as text for an escaped monospace view. Never rendered as HTML.
#[tauri::command]
pub async fn get_message_source(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
) -> CmdResult<String> {
    let provider = state.provider(&account_id).await?;
    let raw = provider
        .get_message_raw(&message_id)
        .await?
        .ok_or_else(|| {
            CmdError::not_found(format!(
                "message not found in {}",
                provider.provider().service_name()
            ))
        })?;
    blocking(move || Ok(message_details::source_text(&raw))).await
}

/// Person card: one correspondent's mail at a glance (local, indexed; ~1 ms).
#[tauri::command]
pub async fn person_summary(
    state: AppStateRef<'_>,
    email: String,
) -> CmdResult<penguin_core::store::PersonSummary> {
    let email = email.trim().to_string();
    if !email.contains('@') {
        return Err(CmdError::invalid("not an email address"));
    }
    let store = state.store.clone();
    blocking(move || Ok(store.person_summary(&email)?)).await
}

/// Composer To/Cc/Bcc suggestions from everyone in your mail (local, indexed).
#[tauri::command]
pub async fn suggest_recipients(
    state: AppStateRef<'_>,
    query: String,
    limit: Option<u32>,
) -> CmdResult<Vec<penguin_core::Address>> {
    let limit = limit.unwrap_or(8).clamp(1, 20) as usize;
    let store = state.store.clone();
    blocking(move || Ok(store.suggest_recipients(&query, limit)?)).await
}

/// In-app preview of one attachment (see attachments.rs for what renders).
/// Unsupported types answer from metadata without touching the network;
/// everything else is fetched once and cached for a later save_attachment.
#[tauri::command]
pub async fn preview_attachment(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
    attachment_id: String,
) -> CmdResult<AttachmentPreview> {
    async {
        let attachment = find_attachment(&state, &account_id, &message_id, &attachment_id).await?;
        if let Some(preview) = attachments::without_bytes(&attachment) {
            return Ok(preview);
        }
        let provider = state.provider(&account_id).await?;
        let bytes = attachments::bytes(
            &state.paths,
            provider.as_ref(),
            &account_id,
            &message_id,
            &attachment,
        )
        .await?;
        blocking(move || Ok(attachments::from_bytes(&attachment, &bytes))).await
    }
    .await
    .inspect_err(|e| attachment_failed("preview", &account_id, &message_id, &attachment_id, e))
}

/// In-app preview of a file in the composer that isn't on a message yet
/// (attachments.rs `outgoing_preview`). A saved draft's or a forward's
/// files are previewed with preview_attachment.
#[tauri::command]
pub async fn preview_outgoing_file(
    filename: String,
    mime_type: String,
    data_base64: String,
) -> CmdResult<AttachmentPreview> {
    blocking(move || attachments::outgoing_preview(filename, mime_type, &data_base64)).await
}

/// Opens only files this session saved; the UI can't open arbitrary paths.
/// `in_preview`: in macOS Preview (the image viewer) instead of the default app.
#[tauri::command]
pub async fn open_path(
    app: AppHandle,
    state: AppStateRef<'_>,
    path: String,
    in_preview: Option<bool>,
) -> CmdResult<()> {
    let path = std::path::PathBuf::from(path);
    if !state.is_saved_path(&path) {
        return Err(CmdError::invalid(
            "Only attachments saved by Penguin can be opened",
        ));
    }
    let with = (cfg!(target_os = "macos") && in_preview == Some(true)).then_some("Preview");
    app.opener()
        .open_path(path.display().to_string(), with)
        .map_err(|e| CmdError::other(e.to_string()))
}

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> CmdResult<()> {
    let parsed = check_external_url(&url)?;
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(|e| CmdError::other(e.to_string()))
}

/// Largest HTML the composer may ask to have cleaned (a big paste).
const MAX_COMPOSE_HTML: usize = 4 * 1024 * 1024;

/// HTML entering the rich-text composer (paste from the web, Google Docs)
/// through penguin-render's strict editor allowlist. Pure, no I/O.
#[tauri::command]
pub async fn sanitize_compose_html(html: String) -> CmdResult<String> {
    if html.len() > MAX_COMPOSE_HTML {
        return Err(CmdError::invalid(
            "That paste is too large to keep its formatting",
        ));
    }
    Ok(penguin_render::sanitize_compose_html(&html))
}

// ---------- settings ----------

/// Settings as the UI sees them: profile members that aren't signed-in
/// accounts are pruned (see Settings::for_accounts).
pub(crate) async fn settings_view(state: &AppState, settings: Settings) -> CmdResult<Settings> {
    if settings.profiles.is_empty() {
        return Ok(settings);
    }
    let known = state.accounts().await?.into_iter().map(|a| a.id).collect();
    Ok(settings.for_accounts(&known))
}

#[tauri::command]
pub async fn get_settings(state: AppStateRef<'_>) -> CmdResult<Settings> {
    settings_view(&state, state.settings.get()).await
}

#[tauri::command]
pub async fn update_settings(state: AppStateRef<'_>, patch: SettingsPatch) -> CmdResult<Settings> {
    save_settings(state.inner(), patch).await
}

/// Save a settings change and apply it everywhere (update_settings, and
/// menu items that are settings, like Check Spelling While Typing).
pub(crate) async fn save_settings(
    state: &Arc<AppState>,
    patch: SettingsPatch,
) -> CmdResult<Settings> {
    let st = state.clone();
    let sets_quota = patch.gmail_units_per_min.is_some();
    let sets_window = patch.sync_window_months.is_some() || patch.older_mail.is_some();
    let before = state.settings.get().mcp.access;
    if patch.mcp.as_ref().is_some_and(|m| m.raises_to_send(before)) {
        return Err(CmdError::invalid(
            "Allowing agents to send needs the confirmation in Settings → Developer → Agents",
        ));
    }
    let saved = blocking(move || Ok(st.settings.update(patch)?)).await?;
    crate::agent_app::level_changed(state, before, saved.mcp.access).await;
    let saved = settings_view(state, saved).await?;
    tracing::info!("settings updated");
    if sets_quota {
        // Resizes every account's limiter in place (no-op under the env override).
        penguin_gmail::api::set_units_per_min(saved.gmail_units_per_min);
    }
    if sets_window {
        state.set_window_policy(saved.window_policy());
    }
    state.semantic_indexer.set_enabled(saved.semantic_search);
    if saved.welcome_completed {
        // Finished or skipped the Welcome setup: the model may download now.
        state.semantic_indexer.set_downloads_allowed(true);
    }
    state.emit_settings_changed(saved.clone());
    Ok(saved)
}

// ---------- diagnostics ----------

fn log_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_log_dir().ok()
}

/// Counts, sizes and paths for Settings → Diagnostics. Never touches the
/// network or the Keychain; the store work is a handful of indexed reads.
#[tauri::command]
pub async fn diagnostics(app: AppHandle, state: AppStateRef<'_>) -> CmdResult<Diagnostics> {
    let started = Instant::now();
    let st = state.inner().clone();
    let logs = log_dir(&app);
    let (accounts, stats, mut cursors, sizes) = blocking(move || {
        let accounts = st.store.list_accounts()?;
        let stats = st.store.db_stats()?;
        let mut cursors = HashMap::new();
        for a in &accounts {
            cursors.insert(a.id.clone(), st.store.get_sync_cursor(&a.id)?);
        }
        let db = st.paths.db_path();
        let inline_by_account: HashMap<String, u64> = accounts
            .iter()
            .map(|a| {
                (
                    a.id.clone(),
                    diag::dir_size(&st.paths.inline_cache_dir(&a.id)),
                )
            })
            .collect();
        let log_bytes = logs
            .as_deref()
            .map(|d| diag::file_size(&d.join(crate::logging::LOG_FILE)))
            .unwrap_or(0);
        let client_tail = OAuthClientConfig::load(&st.paths.config_dir)
            .ok()
            .flatten()
            .map(|c| diag::client_id_tail(&c.client_id));
        let sizes = (
            diag::file_size(&db),
            diag::file_size(&diag::wal_path(&db)),
            diag::dir_size(&st.paths.cache_dir.join("inline")),
            log_bytes,
            client_tail,
            inline_by_account,
        );
        Ok((accounts, stats, cursors, sizes))
    })
    .await?;
    let (
        db_bytes,
        wal_bytes,
        inline_cache_bytes,
        log_bytes,
        oauth_client_id_tail,
        inline_by_account,
    ) = sizes;

    let statuses: HashMap<String, SyncStatus> = state
        .statuses(
            &accounts,
            &stats.messages_by_account.clone().into_iter().collect(),
        )
        .into_iter()
        .map(|s| (s.account_id.clone(), s))
        .collect();
    let store = state.store.clone();
    let headers_only: HashMap<String, u64> = blocking(move || Ok(store.body_coverage()?))
        .await?
        .into_iter()
        .map(|c| (c.account_id, c.headers_only))
        .collect();
    let accounts = accounts
        .into_iter()
        .map(|a| {
            let stored = stats.messages_by_account.get(&a.id).copied().unwrap_or(0);
            state.diag.sample(&a.id, stored);
            let status = statuses.get(&a.id);
            let cursor = cursors.remove(&a.id).unwrap_or_default();
            AccountDiagnostics {
                phase: status
                    .map(|s| s.phase)
                    .unwrap_or(penguin_core::SyncPhase::Idle),
                messages_stored: stored,
                headers_only_messages: headers_only.get(&a.id).copied().unwrap_or(0),
                gmail_total: status.and_then(|s| s.total_estimate),
                backfill_done: cursor.backfill_done,
                // Gmail: a history id is recorded; others: any resume state.
                history_id_present: match a.provider {
                    AccountProvider::Gmail => {
                        penguin_gmail::sync::GmailCursor::parse(&cursor.provider_state)
                            .is_ok_and(|g| g.history_id.is_some())
                    }
                    _ => !cursor.provider_state.is_empty(),
                },
                failed_message_ids: cursor.failed_message_ids.len() as u64,
                last_synced_at: status.and_then(|s| s.last_synced_at),
                msgs_per_minute: state.diag.rate_per_minute(&a.id),
                keychain: state.diag.keychain(&a.id),
                quota: penguin_gmail::api::quota_stats(&a.email),
                inline_cache_bytes: inline_by_account.get(&a.id).copied().unwrap_or(0),
                error: status.and_then(|s| s.error.clone()),
                email: a.email,
                account_id: a.id,
            }
        })
        .collect();
    let paths = &state.paths;
    Ok(Diagnostics {
        app_version: crate::VERSION.to_string(),
        os_version: crate::diagnostics::os_version(),
        data_dir: paths.data_dir.display().to_string(),
        config_dir: paths.config_dir.display().to_string(),
        cache_dir: paths.cache_dir.display().to_string(),
        log_dir: log_dir(&app).map(|d| d.display().to_string()),
        db_path: paths.db_path().display().to_string(),
        db_bytes,
        wal_bytes,
        page_size: stats.page_size,
        page_count: stats.page_count,
        freelist_count: stats.freelist_count,
        total_messages: stats.total_messages,
        total_threads: stats.total_threads,
        inline_cache_bytes,
        log_bytes,
        oauth_client_id_tail,
        accounts,
        trackers_removed_session: state.diag.trackers_removed(),
        gmail_units_env_override: std::env::var("PENGUIN_GMAIL_UNITS_PER_MIN")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&n| n > 0),
        took_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Per-table/index sizes via dbstat. Reads every page of the database, so it
/// is its own command that the UI calls on demand.
#[tauri::command]
pub async fn diagnostics_table_sizes(state: AppStateRef<'_>) -> CmdResult<TableSizes> {
    let started = Instant::now();
    let store = state.store.clone();
    let tables = blocking(move || Ok(store.table_sizes()?)).await?;
    Ok(TableSizes {
        method: "dbstat".into(),
        tables,
        took_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Show one of Penguin's own directories in Finder; the UI names a target,
/// never a path.
#[tauri::command]
pub async fn reveal_path(
    app: AppHandle,
    state: AppStateRef<'_>,
    target: RevealTarget,
) -> CmdResult<()> {
    let paths = &state.paths;
    let (dir, item) = match target {
        RevealTarget::Data => (paths.data_dir.clone(), Some(paths.db_path())),
        RevealTarget::Config => (paths.config_dir.clone(), None),
        RevealTarget::Cache => (paths.cache_dir.clone(), None),
        RevealTarget::Log => {
            let dir = log_dir(&app).ok_or_else(|| CmdError::not_found("no log directory"))?;
            let file = dir.join(crate::logging::LOG_FILE);
            (dir, Some(file))
        }
    };
    let opener = app.opener();
    let result = match item.filter(|p| p.exists()) {
        Some(file) => opener.reveal_item_in_dir(file),
        None if dir.exists() => opener.open_path(dir.display().to_string(), None::<&str>),
        None => {
            return Err(CmdError::not_found(format!(
                "{} does not exist yet",
                dir.display()
            )))
        }
    };
    result.map_err(|e| CmdError::other(e.to_string()))
}

/// Merge FTS segments and refresh planner stats (Store::optimize).
#[tauri::command]
pub async fn optimize_index(state: AppStateRef<'_>) -> CmdResult<()> {
    let started = Instant::now();
    let store = state.store.clone();
    blocking(move || Ok(store.optimize()?)).await?;
    tracing::info!(
        ms = started.elapsed().as_millis() as u64,
        "search index optimized"
    );
    Ok(())
}

// ---------- MCP (penguin-cli mcp) ----------

/// Everything Settings → Developer needs to show the MCP section.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInfo {
    pub enabled: bool,
    /// The agent level (Settings → Developer → Agents).
    pub access: crate::settings::AgentAccess,
    /// The penguin-cli binary next to this app's executable, if present.
    pub cli_path: Option<String>,
    /// `claude mcp add …` line for Claude Code.
    pub claude_code_command: String,
    /// Pretty JSON to merge into claude_desktop_config.json.
    pub claude_desktop_config: String,
    /// `claude mcp add penguin -- ssh you@your-mac … mcp`, for an agent on
    /// another machine (docs/CLI.md → Use from another machine).
    pub ssh_command: String,
    /// The `authorized_keys` options that pin that machine's key to the MCP
    /// server and nothing else; the key itself follows them.
    pub authorized_keys_prefix: String,
    pub audit_log_path: Option<String>,
}

/// Where the CLI ships: Contents/MacOS/penguin-cli in the bundle, or next
/// to the app binary in target/ during development.
const BUNDLED_CLI_PATH: &str = "/Applications/Penguin.app/Contents/MacOS/penguin-cli";

pub fn mcp_info_for(
    access: crate::settings::AgentAccess,
    cli_path: Option<String>,
    audit_log_path: Option<String>,
) -> McpInfo {
    let command = cli_path
        .clone()
        .unwrap_or_else(|| BUNDLED_CLI_PATH.to_string());
    let plain = command
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    let quoted = if plain {
        command.clone()
    } else {
        format!("'{}'", command.replace('\'', r"'\''"))
    };
    // Over ssh the command line is split again by the Mac's shell, so a
    // path with spaces needs its quotes to survive the local shell too.
    let remote = if plain {
        command.clone()
    } else {
        format!("\"{quoted}\"")
    };
    let desktop =
        serde_json::json!({ "mcpServers": { "penguin": { "command": command, "args": ["mcp"] } } });
    McpInfo {
        enabled: access > crate::settings::AgentAccess::Off,
        access,
        cli_path,
        claude_code_command: format!("claude mcp add penguin -- {quoted} mcp"),
        claude_desktop_config: serde_json::to_string_pretty(&desktop).unwrap_or_default(),
        ssh_command: format!("claude mcp add penguin -- ssh you@your-mac {remote} mcp"),
        authorized_keys_prefix: format!(
            "command=\"{} mcp\",restrict",
            command.replace('\\', "\\\\").replace('"', "\\\"")
        ),
        audit_log_path,
    }
}

#[tauri::command]
pub async fn mcp_info(state: AppStateRef<'_>) -> CmdResult<McpInfo> {
    let access = state.settings.get().mcp.access;
    let cli = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("penguin-cli")))
        .filter(|p| p.is_file())
        .map(|p| p.display().to_string());
    let audit = crate::agent::log_dir().map(|d| {
        d.join(crate::agent::audit::AUDIT_FILE)
            .display()
            .to_string()
    });
    Ok(mcp_info_for(access, cli, audit))
}

// ---------- command-line tool (Settings → Developer) ----------

fn cli_link_status() -> CmdResult<crate::cli_install::CliLinkStatus> {
    use crate::cli_install as ci;
    let home = dirs::home_dir().ok_or_else(|| CmdError::other("no home directory"))?;
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    let login_path = ci::login_shell_path(&shell);
    Ok(ci::status(
        &home,
        ci::bundled_cli().as_deref(),
        login_path.as_deref(),
        Some(&shell),
    ))
}

/// Whether `~/.local/bin/penguin` points at this app's CLI, and whether
/// ~/.local/bin is on the login shell's PATH.
#[tauri::command]
pub async fn cli_install_status() -> CmdResult<crate::cli_install::CliLinkStatus> {
    blocking(cli_link_status).await
}

/// Symlink `~/.local/bin/penguin` → the bundled penguin-cli (no admin).
#[tauri::command]
pub async fn install_cli() -> CmdResult<crate::cli_install::CliLinkStatus> {
    blocking(|| {
        let home = dirs::home_dir().ok_or_else(|| CmdError::other("no home directory"))?;
        let target = crate::cli_install::bundled_cli().ok_or_else(|| {
            CmdError::not_found("this build has no penguin-cli next to the app; build it with `cargo build --bin penguin-cli`")
        })?;
        crate::cli_install::install(&home, &target)?;
        tracing::info!("command-line tool linked into ~/.local/bin");
        cli_link_status()
    })
    .await
}

#[cfg(test)]
mod tests {

    #[test]
    fn mcp_snippets_quote_paths() {
        use crate::settings::AgentAccess;
        let info = mcp_info_for(
            AgentAccess::Draft,
            Some("/Users/ada/Code Projects/penguin/target/debug/penguin-cli".into()),
            None,
        );
        assert!(info.enabled);
        assert_eq!(
            info.claude_code_command,
            "claude mcp add penguin -- '/Users/ada/Code Projects/penguin/target/debug/penguin-cli' mcp"
        );
        assert_eq!(
            info.ssh_command,
            "claude mcp add penguin -- ssh you@your-mac \"'/Users/ada/Code Projects/penguin/target/debug/penguin-cli'\" mcp"
        );
        let v: serde_json::Value = serde_json::from_str(&info.claude_desktop_config).unwrap();
        assert_eq!(v["mcpServers"]["penguin"]["args"][0], "mcp");
        let bundled = mcp_info_for(AgentAccess::Off, None, None);
        assert!(!bundled.enabled);
        assert_eq!(
            bundled.claude_code_command,
            format!("claude mcp add penguin -- {BUNDLED_CLI_PATH} mcp")
        );
        assert_eq!(
            bundled.ssh_command,
            format!("claude mcp add penguin -- ssh you@your-mac {BUNDLED_CLI_PATH} mcp")
        );
        assert_eq!(
            bundled.authorized_keys_prefix,
            format!("command=\"{BUNDLED_CLI_PATH} mcp\",restrict")
        );
    }

    use super::*;

    #[test]
    fn external_urls_are_allowlisted() {
        assert!(check_external_url("https://example.com/x").is_ok());
        assert!(check_external_url("mailto:ada@x.example").is_ok());
        assert!(check_external_url("file:///etc/passwd").is_err());
        assert!(check_external_url("javascript:alert(1)").is_err());
        assert!(check_external_url("not a url").is_err());
    }
}
