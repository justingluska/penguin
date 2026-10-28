//! `connect_account`: add a Microsoft (Graph) or IMAP account from the Add
//! account flow (contract: docs/ARCHITECTURE.md → Providers and Add
//! account; implementation brief: docs/PROVIDERS-IMPL.md).
//!
//! The command validates the request (penguin-onboard, shared with the
//! iPhone), refuses an address already in Penguin, dispatches by auth
//! method to [`connect_imap`] or [`connect_microsoft`], then [`finish`]es:
//! store the account, start its sync, emit `sync-status`. Google accounts
//! use `add_account`.

use std::sync::Arc;

use penguin_core::{Account, AccountProvider};
use penguin_onboard::{new_account, validate};
use tauri::{AppHandle, State};

use super::ConnectAccountRequest;
use crate::error::{CmdError, CmdResult};
use crate::ops;
use crate::state::{blocking, AppState};

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Sign in a non-Google account and start syncing it. Rejects
/// `invalidInput` for a malformed request or an address already in
/// Penguin; everything else is the provider's (see the module docs).
#[tauri::command]
pub async fn connect_account(
    app: AppHandle,
    state: AppStateRef<'_>,
    request: ConnectAccountRequest,
) -> CmdResult<Account> {
    let checked = validate(&request).map_err(CmdError::invalid)?;
    let existing = state.accounts().await?;
    if existing.iter().any(|a| a.id == checked.id) {
        return Err(CmdError::invalid(format!(
            "{} is already in Penguin.",
            checked.email
        )));
    }
    let account = new_account(
        &checked,
        &request,
        ops::pick_color(&existing),
        ops::now_ms(),
    );
    // Kind and auth only: the address, servers and password stay out of logs.
    tracing::info!(kind = ?request.kind, auth = ?request.auth, "connecting account");
    let connected = match account.provider {
        AccountProvider::Gmail => {
            return Err(CmdError::invalid(
                "Google accounts sign in with Google (add_account)",
            ))
        }
        AccountProvider::Imap => connect_imap(&app, state.inner(), &request, account).await?,
        AccountProvider::Microsoft => {
            connect_microsoft(&app, state.inner(), &request, account).await?
        }
    };
    finish(state.inner(), connected).await
}

/// IMAP + SMTP with a password. The IMAP provider implements this: log in
/// to `request.imap` and check `request.smtp` with `request.password`
/// (bad credentials → invalidInput with the server's words, unreachable →
/// network), then save the password in the IMAP vault
/// (`penguin_provider::credentials`, [`IMAP_SERVICE`]) under `account.id`
/// and return `account` (its `provider_config` is already filled in).
/// Never log the password.
///
/// [`IMAP_SERVICE`]: penguin_provider::credentials::IMAP_SERVICE
pub async fn connect_imap(
    _app: &AppHandle,
    _state: &Arc<AppState>,
    request: &ConnectAccountRequest,
    account: Account,
) -> CmdResult<Account> {
    // Refuses Google Workspace on Gmail's servers, logs in to IMAP, checks
    // SMTP AUTH (nothing sent), then saves the password in the Keychain.
    let password = request.password.as_deref().unwrap_or_default();
    penguin_imap::connect::connect(&account, password).await?;
    Ok(account)
}

/// Microsoft Graph. The Microsoft provider implements this: browser
/// sign-in with PKCE on `/common` with `request.client_id` (cancellable
/// through `state.begin_sign_in()`, rejecting `cancelled`), consent errors
/// with their AADSTS code in the message; save the tokens in the Microsoft
/// vault ([`MICROSOFT_SERVICE`]) and return `account` (set
/// `provider_config.tenant_id` and `display_name` from the token).
///
/// [`MICROSOFT_SERVICE`]: penguin_provider::credentials::MICROSOFT_SERVICE
pub async fn connect_microsoft(
    app: &AppHandle,
    state: &Arc<AppState>,
    _request: &ConnectAccountRequest,
    mut account: Account,
) -> CmdResult<Account> {
    let client_id = account.provider_config.client_id.clone().ok_or_else(|| {
        CmdError::invalid("Paste your Microsoft app's Application (client) ID first.")
    })?;
    let signed = microsoft_sign_in(app, state, &client_id, &account.id).await?;
    account.display_name = signed.display_name;
    account.provider_config.tenant_id = signed.tenant_id;
    Ok(account)
}

/// The browser sign-in (penguin-graph), cancellable through
/// `cancel_sign_in`: dropping the sign-in closes its loopback listener.
/// Penguin comes back to the front once the browser shows "signed in".
async fn microsoft_sign_in(
    app: &AppHandle,
    state: &AppState,
    client_id: &str,
    account_id: &str,
) -> CmdResult<penguin_graph::SignedIn> {
    use tauri_plugin_opener::OpenerExt;
    let cancelled = state.begin_sign_in();
    let opener = app.clone();
    let open_browser = move |url: &str| {
        if let Err(e) = opener.opener().open_url(url, None::<&str>) {
            tracing::error!(error = %e, "could not open the browser for Microsoft sign-in");
        }
    };
    let focus = app.clone();
    let on_success = move || crate::app_menu::bring_to_front(&focus);
    let ui = penguin_graph::SignInUi {
        open_browser: &open_browser,
        on_success: Some(&on_success),
    };
    tokio::select! {
        signed = penguin_graph::sign_in(client_id, account_id, &ui) => Ok(signed?),
        _ = cancelled => {
            tracing::info!("Microsoft sign-in cancelled");
            Err(CmdError::cancelled())
        }
    }
}

/// Microsoft's half of `reconnect_account`: sign in again as the same
/// account (with the saved client ID, which may have been changed since),
/// then restart its sync.
async fn reconnect_microsoft(
    app: &AppHandle,
    state: &Arc<AppState>,
    mut account: Account,
) -> CmdResult<Account> {
    let dir = state.paths.config_dir.clone();
    let saved = blocking(move || Ok(super::ms_client::load(&dir))).await?;
    let client_id = saved
        .or_else(|| account.provider_config.client_id.clone())
        .ok_or_else(|| {
            CmdError::invalid("Paste your Microsoft app's Application (client) ID first.")
        })?;
    let signed = microsoft_sign_in(app, state, &client_id, &account.id).await?;
    account.provider_config.client_id = Some(client_id);
    account.provider_config.tenant_id = signed.tenant_id;
    if account.display_name.is_none() {
        account.display_name = signed.display_name;
    }
    let store = state.store.clone();
    let record = account.clone();
    blocking(move || Ok(store.upsert_account(&record)?)).await?;
    tracing::info!(account = %account.id, "Microsoft account reconnected");
    state.stop_account(&account.id);
    state.start_account(&account).await;
    state.account(&account.id).await
}

/// `reconnect_account` for a non-Google account (after NeedsReauth). The
/// provider agents implement their half: Microsoft runs the browser
/// sign-in again with `login_hint` = the address (refusing another
/// account, as Google's reconnect does); IMAP takes the new `password` the
/// user typed (or retries the saved one without it). On success: save the
/// credential, `state.stop_account` + `state.start_account`, return the
/// account.
pub async fn reconnect(
    app: &AppHandle,
    state: &Arc<AppState>,
    account: Account,
    password: Option<String>,
) -> CmdResult<Account> {
    match account.provider {
        AccountProvider::Imap => {
            penguin_imap::connect::reconnect(&account, password.as_deref()).await?;
            state.stop_account(&account.id);
            state.start_account(&account).await;
            Ok(account)
        }
        AccountProvider::Microsoft => reconnect_microsoft(app, state, account).await,
        _ => Err(penguin_provider::Error::not_implemented(
            account.provider,
            "Reconnecting an account",
        )
        .into()),
    }
}

/// Store a connected account, start its sync (which emits `sync-status`),
/// and return it as the store has it (with its capabilities).
pub async fn finish(state: &Arc<AppState>, account: Account) -> CmdResult<Account> {
    let store = state.store.clone();
    let record = account.clone();
    blocking(move || Ok(store.upsert_account(&record)?)).await?;
    tracing::info!(account = %account.id, provider = account.provider.as_str(), "account connected");
    state.start_account(&account).await;
    state.account(&account.id).await
}
