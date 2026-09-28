//! `connect_account`: add a Microsoft (Graph) or IMAP account from the Add
//! account flow (contract: docs/ARCHITECTURE.md → Providers and Add
//! account; implementation brief: docs/PROVIDERS-IMPL.md).
//!
//! The command validates the request, refuses an address already in
//! Penguin, dispatches by auth method to [`connect_imap`] or
//! [`connect_microsoft`], then [`finish`]es: store the account, start its
//! sync, emit `sync-status`. Google accounts use `add_account`.
//!
//! Phase 0: both provider steps answer "not implemented yet"; the IMAP and
//! Microsoft agents replace their function's body (and nothing else here),
//! then flip `providers::connectable` for their auth methods.

use std::sync::Arc;

use penguin_core::{Account, AccountProvider, AuthMethod, MailSecurity, ProviderConfig};
use tauri::{AppHandle, State};

use super::{detect, provider_for, ConnectAccountRequest, ServerSettings};
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
    let account = new_account(&checked, &request, &existing, ops::now_ms());
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
/// account, as Google's reconnect does); IMAP needs a new password, so the
/// UI sends it through `connect_account`-style input (extend the contract
/// in ARCHITECTURE.md when you add it). On success: save the credential,
/// `state.stop_account` + `state.start_account`, return the account.
pub async fn reconnect(
    app: &AppHandle,
    state: &Arc<AppState>,
    account: Account,
) -> CmdResult<Account> {
    match account.provider {
        // IMAP: the saved password must work again (fixed on the provider's
        // side, or a server outage); a new password means removing and
        // re-adding the account until reconnect takes one.
        AccountProvider::Imap => {
            penguin_imap::connect::reconnect(&account).await?;
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

/// A request that passed [`validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The address as typed, trimmed, domain lowercased.
    pub email: String,
    /// The account id: the address lowercased.
    pub id: String,
}

/// Longest password accepted (app passwords are 16–19 characters).
const MAX_PASSWORD: usize = 1024;

fn check_server(what: &str, s: &ServerSettings) -> Result<(), String> {
    let host = s.host.trim();
    if host.is_empty() || host.len() > 253 || host.chars().any(|c| c.is_whitespace()) {
        return Err(format!("Enter the {what} server's host name."));
    }
    if s.port == 0 {
        return Err(format!("Enter the {what} server's port."));
    }
    if s.username.trim().is_empty() {
        return Err(format!("Enter the {what} user name."));
    }
    if s.security == MailSecurity::Plain {
        let local = matches!(
            host.to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "::1" | "[::1]"
        );
        if !local {
            return Err(format!(
                "The {what} server needs TLS or STARTTLS; unencrypted connections are only allowed to this Mac (Proton Mail Bridge)."
            ));
        }
    }
    Ok(())
}

/// Everything about the request that doesn't need the network: a real
/// address, an auth method that belongs to its provider, and the fields
/// that method needs (IMAP: servers and a password; Microsoft: a client
/// id). Err is a user-facing message.
pub fn validate(request: &ConnectAccountRequest) -> Result<Checked, String> {
    let addr = detect::parse_email(&request.email)?;
    match request.auth {
        AuthMethod::GoogleOAuth => {}
        AuthMethod::MicrosoftOAuth => {
            let id = request.client_id.as_deref().unwrap_or("").trim();
            if id.is_empty() {
                return Err("Paste your Microsoft app's Application (client) ID first.".into());
            }
            super::ms_client::normalize_client_id(id)?;
        }
        AuthMethod::AppPassword | AuthMethod::ImapPassword => {
            match request.password.as_deref() {
                Some(p) if !p.is_empty() && p.len() <= MAX_PASSWORD => {}
                _ => return Err("Enter the password.".into()),
            }
            let imap = request
                .imap
                .as_ref()
                .ok_or("Enter the incoming (IMAP) server.")?;
            let smtp = request
                .smtp
                .as_ref()
                .ok_or("Enter the outgoing (SMTP) server.")?;
            check_server("IMAP", imap)?;
            check_server("SMTP", smtp)?;
        }
    }
    Ok(Checked {
        id: addr.email.to_lowercase(),
        email: addr.email,
    })
}

/// The account record a successful connect stores: provider from the auth
/// method, `provider_config` from the request (never the password), a
/// fresh color. The provider step may fill in more (display name, tenant).
pub fn new_account(
    checked: &Checked,
    request: &ConnectAccountRequest,
    existing: &[Account],
    now_ms: i64,
) -> Account {
    let provider = provider_for(request.auth);
    let host = serde_json::to_value(request.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string));
    let provider_config = match provider {
        AccountProvider::Gmail => ProviderConfig::default(),
        AccountProvider::Imap => ProviderConfig {
            auth: Some(request.auth),
            imap: request.imap.clone().map(trimmed),
            smtp: request.smtp.clone().map(trimmed),
            host,
            ..ProviderConfig::default()
        },
        AccountProvider::Microsoft => ProviderConfig {
            auth: Some(request.auth),
            client_id: request
                .client_id
                .as_deref()
                .and_then(|c| super::ms_client::normalize_client_id(c).ok()),
            host,
            ..ProviderConfig::default()
        },
    };
    Account {
        id: checked.id.clone(),
        email: checked.email.clone(),
        display_name: None,
        nickname: None,
        color: ops::pick_color(existing),
        added_at: now_ms,
        provider,
        capabilities: provider.capabilities(),
        provider_config,
    }
}

fn trimmed(s: ServerSettings) -> ServerSettings {
    ServerSettings {
        host: s.host.trim().to_ascii_lowercase(),
        username: s.username.trim().to_string(),
        ..s
    }
}

#[cfg(test)]
mod tests {
    use super::super::ProviderKind;
    use super::*;

    fn server(host: &str, port: u16, security: MailSecurity) -> ServerSettings {
        ServerSettings {
            host: host.into(),
            port,
            security,
            username: "sam".into(),
        }
    }

    fn imap_request() -> ConnectAccountRequest {
        ConnectAccountRequest {
            email: " Sam@iCloud.com ".into(),
            kind: ProviderKind::Icloud,
            auth: AuthMethod::AppPassword,
            client_id: None,
            password: Some("abcd-efgh-ijkl-mnop".into()),
            imap: Some(server(" imap.mail.me.com ", 993, MailSecurity::Tls)),
            smtp: Some(server("smtp.mail.me.com", 587, MailSecurity::Starttls)),
        }
    }

    fn ms_request() -> ConnectAccountRequest {
        ConnectAccountRequest {
            email: "sam@outlook.com".into(),
            kind: ProviderKind::OutlookPersonal,
            auth: AuthMethod::MicrosoftOAuth,
            client_id: Some("{1B2C3D4E-0000-1111-2222-333344445555}".into()),
            password: None,
            imap: None,
            smtp: None,
        }
    }

    #[test]
    fn requests_dispatch_to_their_provider() {
        assert_eq!(
            provider_for(AuthMethod::GoogleOAuth),
            AccountProvider::Gmail
        );
        assert_eq!(
            provider_for(AuthMethod::MicrosoftOAuth),
            AccountProvider::Microsoft
        );
        assert_eq!(provider_for(AuthMethod::AppPassword), AccountProvider::Imap);
        assert_eq!(
            provider_for(AuthMethod::ImapPassword),
            AccountProvider::Imap
        );
    }

    #[test]
    fn imap_requests_become_imap_accounts_without_the_password() {
        let req = imap_request();
        let checked = validate(&req).unwrap();
        assert_eq!(checked.email, "Sam@icloud.com");
        assert_eq!(checked.id, "sam@icloud.com");
        let a = new_account(&checked, &req, &[], 5);
        assert_eq!(a.provider, AccountProvider::Imap);
        assert_eq!(a.capabilities, AccountProvider::Imap.capabilities());
        assert_eq!(a.provider_config.auth, Some(AuthMethod::AppPassword));
        assert_eq!(
            a.provider_config.imap.as_ref().unwrap().host,
            "imap.mail.me.com"
        );
        assert_eq!(a.provider_config.host.as_deref(), Some("icloud"));
        assert_eq!(a.added_at, 5);
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("abcd-efgh"), "{json}");
    }

    #[test]
    fn microsoft_requests_keep_the_normalized_client_id() {
        let req = ms_request();
        let a = new_account(&validate(&req).unwrap(), &req, &[], 1);
        assert_eq!(a.provider, AccountProvider::Microsoft);
        assert_eq!(
            a.provider_config.client_id.as_deref(),
            Some("1b2c3d4e-0000-1111-2222-333344445555")
        );
        assert_eq!(a.provider_config.host.as_deref(), Some("outlookPersonal"));
        assert_eq!(a.provider_config.imap, None);
    }

    #[test]
    fn bad_requests_say_what_is_missing() {
        let mut r = imap_request();
        r.email = "not an address".into();
        assert!(validate(&r).is_err());
        let mut r = imap_request();
        r.password = None;
        assert_eq!(validate(&r).unwrap_err(), "Enter the password.");
        let mut r = imap_request();
        r.smtp = None;
        assert!(validate(&r).unwrap_err().contains("SMTP"));
        let mut r = imap_request();
        r.imap = Some(server("imap.example.com", 143, MailSecurity::Plain));
        assert!(validate(&r).unwrap_err().contains("TLS or STARTTLS"));
        // Proton Mail Bridge on this Mac may be plain.
        r.imap = Some(server("127.0.0.1", 1143, MailSecurity::Plain));
        assert!(validate(&r).is_ok());
        let mut r = ms_request();
        r.client_id = None;
        assert!(validate(&r).unwrap_err().contains("client) ID"));
        r.client_id = Some("penguin".into());
        assert!(validate(&r).is_err());
    }
}
