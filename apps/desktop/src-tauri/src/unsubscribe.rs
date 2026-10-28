//! Unsubscribe (thread view button, message menu, ⌘U): acting on the plan
//! `penguin_core::unsubscribe` makes for a message. Every action here is a
//! user click the UI has already confirmed; nothing runs on its own.
//!
//! - One-click (RFC 8058): a POST of `List-Unsubscribe=One-Click` to the
//!   header's https URL, from Rust: no cookies, no Referer, no redirects,
//!   public addresses only, short timeouts (`penguin_provider::unsubscribe`,
//!   shared with the iPhone).
//! - mailto: the message is sent from the receiving account through Gmail,
//!   like any other send; only for an authenticated sender.
//! - Link: opened in the default browser.
//!
//! Logs name the domain, never the URL (it carries a per-recipient token).
//! Successful unsubscribes are remembered per account and sender
//! (`Store::record_unsubscribe`) so later mail shows "Unsubscribed".

use std::sync::Arc;

use penguin_core::unsubscribe::{
    self as core_unsub, UnsubscribeMethod, UnsubscribeOffer, UnsubscribeRecord,
};
use penguin_core::{Message, Store};
use penguin_provider::compose::build_rfc822_with_attachments;
use penguin_provider::unsubscribe::{
    headers_say_one_click, mailto_draft, one_click_post, plan_for,
};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops;
use crate::state::{blocking, AppState};
use crate::views::MessageView;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Result of `unsubscribe`, for the UI's toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeOutcome {
    pub method: UnsubscribeMethod,
    pub domain: String,
    /// The sender now remembered as unsubscribed.
    pub sender: String,
    /// Where the mailto message went (method `mailto`).
    pub sent_to: Option<String>,
    pub record: UnsubscribeRecord,
}

/// Fill in "already unsubscribed" for rendered messages (get_thread,
/// load_remote_images). One indexed lookup per message with an offer.
pub fn annotate(store: &Store, views: &mut [MessageView]) -> penguin_core::Result<()> {
    for v in views {
        if let Some(offer) = v.unsubscribe.as_mut() {
            offer.unsubscribed = store.unsubscribe_record(&v.account_id, &v.from.email)?;
        }
    }
    Ok(())
}

async fn load(state: &AppState, account_id: &str, message_id: &str) -> CmdResult<Message> {
    let (store, a, m) = (
        state.store.clone(),
        account_id.to_string(),
        message_id.to_string(),
    );
    blocking(move || Ok(store.get_message(&a, &m)?))
        .await?
        .ok_or_else(|| CmdError::not_found("message not found"))
}

/// Keep a fetched `List-Unsubscribe-Post` for a message stored without it
/// (get_message_details and unsubscribe_check fetch every header anyway).
/// Returns the flag.
pub async fn remember_post_flag(
    state: &AppState,
    m: &Message,
    headers: &[(String, String)],
) -> CmdResult<bool> {
    let one_click = headers_say_one_click(headers);
    if m.list_unsubscribe_post.is_none() {
        let (store, a, id) = (state.store.clone(), m.account_id.clone(), m.id.clone());
        blocking(move || Ok(store.set_list_unsubscribe_post(&a, &id, one_click)?)).await?;
    }
    Ok(one_click)
}

/// The offer for a message, first learning whether one-click is available
/// when it was stored before Penguin kept that header (`needsCheck`): one
/// format=metadata fetch at interactive priority (~20 units), stored so it
/// is never fetched again.
#[tauri::command]
pub async fn unsubscribe_check(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
) -> CmdResult<Option<UnsubscribeOffer>> {
    let mut m = load(&state, &account_id, &message_id).await?;
    let needs_check = plan_for(&m).is_some_and(|p| p.offer.needs_check);
    if needs_check {
        let provider = state.provider(&account_id).await?;
        let meta = provider
            .get_message_metadata(&message_id)
            .await?
            .ok_or_else(|| {
                CmdError::not_found(format!(
                    "{} no longer has this message",
                    crate::commands::cap_first(provider.provider().service_name())
                ))
            })?;
        m.list_unsubscribe_post = Some(remember_post_flag(&state, &m, &meta.headers).await?);
    }
    let Some(plan) = plan_for(&m) else {
        return Ok(None);
    };
    let mut offer = plan.offer;
    let store = state.store.clone();
    let sender = m.from.email.clone();
    offer.unsubscribed =
        blocking(move || Ok(store.unsubscribe_record(&account_id, &sender)?)).await?;
    Ok(Some(offer))
}

/// Unsubscribe with `method`, which must be what the UI confirmed: the
/// message's own plan (re-derived here; the UI never supplies a URL), or
/// `link` to open the page of a one-click plan instead.
#[tauri::command]
pub async fn unsubscribe(
    app: AppHandle,
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
    method: UnsubscribeMethod,
) -> CmdResult<UnsubscribeOutcome> {
    let m = load(&state, &account_id, &message_id).await?;
    let plan =
        plan_for(&m).ok_or_else(|| CmdError::invalid("This message has no unsubscribe option"))?;
    let allowed =
        method == plan.offer.method || (method == UnsubscribeMethod::Link && plan.url.is_some());
    if !allowed {
        return Err(CmdError::invalid(
            "This message's unsubscribe option changed. Try again.",
        ));
    }
    let domain = plan.offer.domain.clone();
    let mut sent_to = None;
    match method {
        UnsubscribeMethod::OneClick => {
            let url = plan
                .url
                .as_deref()
                .ok_or_else(|| CmdError::other("no unsubscribe URL"))?;
            one_click_post(url).await.map_err(|e| {
                tracing::warn!(account = %account_id, %domain, error = %e, "one-click unsubscribe failed");
                CmdError::new(ErrorCode::Network, format!("{domain} didn't accept the unsubscribe ({e})"))
            })?;
        }
        UnsubscribeMethod::Mailto => {
            if !plan.offer.verified {
                return Err(CmdError::invalid(
                    "Couldn't verify this sender, so Penguin won't send mail for it. Use Edit first.",
                ));
            }
            let mailto = plan
                .offer
                .mailto
                .clone()
                .ok_or_else(|| CmdError::other("no unsubscribe address"))?;
            send_mailto(&state, &account_id, &mailto).await?;
            sent_to = Some(mailto.to);
        }
        UnsubscribeMethod::Link => {
            let url = plan
                .url
                .as_deref()
                .ok_or_else(|| CmdError::other("no unsubscribe URL"))?;
            let parsed = crate::commands::check_external_url(url)?;
            app.opener()
                .open_url(parsed.as_str(), None::<&str>)
                .map_err(|e| CmdError::other(e.to_string()))?;
        }
    }
    let record = UnsubscribeRecord {
        at: now_ms(),
        method,
    };
    let (store, a, sender) = (
        state.store.clone(),
        account_id.clone(),
        m.from.email.clone(),
    );
    blocking(move || Ok(store.record_unsubscribe(&a, &sender, method, record.at)?)).await?;
    tracing::info!(account = %account_id, %domain, method = method.as_str(), "unsubscribed");
    Ok(UnsubscribeOutcome {
        method,
        domain,
        sender: m.from.email.to_lowercase(),
        sent_to,
        record,
    })
}

async fn send_mailto(
    state: &AppState,
    account_id: &str,
    mailto: &core_unsub::Mailto,
) -> CmdResult<()> {
    let account = state.account(account_id).await?;
    let draft = mailto_draft(&account.id, mailto);
    let raw = build_rfc822_with_attachments(&draft, &ops::from_address(&account), None, &[], &[])?;
    let provider = state.provider(&account.id).await?;
    provider.send_raw(&raw, None).await?;
    // The sent copy arrives through sync.
    state.poke(&account.id);
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Tests for the one-click target rules, the public-only resolver and the
// body-link plan live with the code in penguin-provider (unsubscribe.rs).
