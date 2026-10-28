//! Settings → Accounts → Pair iPhone (docs/PAIRING.md): show a QR code,
//! let one phone connect over the local network (crates/penguin-pair), and
//! after the user clicks Send hand it the accounts: the Gmail refresh tokens
//! it can use (its own iOS client's, and browser sign-ins together with this
//! Mac's "Desktop app" client, which refreshes them), IMAP passwords and
//! Microsoft refresh tokens (each with the client that minted it), plus
//! names, colors, order and server settings for all of them.
//!
//! One pairing at a time. `pair_iphone_start` opens the listener and returns
//! the QR code; progress arrives as `penguin://pair-iphone` events
//! ([`PairEvent`]); `pair_iphone_confirm` answers the Send prompt and
//! `pair_iphone_cancel` stops it (the Settings modal closing does too).
//! Tokens never cross the IPC bridge: the UI sees addresses and statuses.

use std::sync::{Arc, Mutex};

use penguin_core::{Account, AccountProvider};
use penguin_gmail::auth::{AuthManager, TokenOrigin};
use penguin_pair::server::{self, Decision, Outcome, Timeouts};
use penguin_pair::{
    AccountKind, AccountMeta, AccountStatus, DesktopClient, Offer, PairError, PhoneHello,
    SecretString, SignInReason, Transfer,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;

use crate::error::{CmdError, CmdResult};
use crate::state::{blocking, AppState};

pub const EVENT_PAIR_IPHONE: &str = "penguin://pair-iphone";
/// The most hosts a pairing code may list (penguin-pair refuses more).
const MAX_HOSTS: usize = 8;

/// The pairing in progress, if any (managed state).
#[derive(Default)]
pub struct Pairing(Mutex<Option<Running>>);

struct Running {
    id: String,
    task: tauri::async_runtime::JoinHandle<()>,
    /// The Send prompt's answer, once a phone is waiting for it.
    confirm: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
}

impl Pairing {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Running>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Stop the pairing `id` (any when None): the listener closes, a phone
    /// connected to it hears the connection drop.
    fn stop(&self, id: Option<&str>) -> bool {
        let mut current = self.lock();
        if current
            .as_ref()
            .is_some_and(|r| id.is_none_or(|id| r.id == id))
        {
            if let Some(running) = current.take() {
                running.task.abort();
                return true;
            }
        }
        false
    }
}

/// What `pair_iphone_start` returns: the QR code and what can move.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    pub session_id: String,
    pub qr: QrCode,
    /// Unix ms.
    pub expires_at: i64,
    /// Where the phone will look for this Mac (shown small, for when it
    /// can't connect).
    pub hosts: Vec<String>,
    pub port: u16,
    /// Every account before a phone is known: `token` = has a credential
    /// that can move (a Gmail browser sign-in with this Mac's desktop
    /// client, or the sign-in sheet's client if the phone's build uses the
    /// same one; an IMAP password; a Microsoft refresh token), `signIn` =
    /// sign in on the phone.
    pub accounts: Vec<PairAccount>,
    /// This Mac's sign-in-sheet (iOS) client, if set.
    pub mac_client_id: Option<String>,
}

/// The QR code as an SVG path over a `size`×`size` grid (one unit per
/// module, no quiet zone). Encodes the one-time secret: never logged.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrCode {
    pub size: u32,
    pub path: String,
}

impl std::fmt::Debug for QrCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QrCode({}×{}, <redacted>)", self.size, self.size)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairAccount {
    pub email: String,
    /// Nickname, else display name.
    pub name: Option<String>,
    pub color: String,
    /// `token`, `alreadyOnPhone` or `signIn`.
    pub status: &'static str,
    /// Why it needs signing in on the phone, worded for the user.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairPhone {
    pub name: String,
    pub model: String,
    pub system: String,
    pub app_version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairFailure {
    pub email: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum PairEvent {
    /// A phone proved it scanned the code; waiting for Send.
    #[serde(rename_all = "camelCase")]
    Connected {
        session_id: String,
        phone: PairPhone,
        /// Six digits the phone shows too ("482 193").
        code: String,
        accounts: Vec<PairAccount>,
        /// The phone signs in with a different Google client than this
        /// Mac's tokens: why nothing can move.
        client_note: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Sending { session_id: String },
    #[serde(rename_all = "camelCase")]
    Done {
        session_id: String,
        phone_name: String,
        imported: Vec<String>,
        failed: Vec<PairFailure>,
        /// Accounts listed on the phone to sign in there.
        sign_in: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Declined { session_id: String },
    #[serde(rename_all = "camelCase")]
    Failed {
        session_id: String,
        message: String,
        expired: bool,
    },
}

/// An account as the Mac has it, with what it holds for it.
#[derive(Debug, Clone)]
pub(crate) struct MacAccount {
    pub account: Account,
    /// Gmail: where its token came from (None = no token).
    pub origin: Option<TokenOrigin>,
    /// IMAP and Microsoft: a password or refresh token is in this Mac's
    /// Keychain.
    pub has_secret: bool,
}

#[tauri::command]
pub async fn pair_iphone_start(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    pairing: State<'_, Pairing>,
) -> CmdResult<PairStart> {
    pairing.stop(None);
    let services = state.services()?;
    let auth = services.auth.clone();
    let mac_client_id = auth.ios_client().map(|c| c.client_id);
    let desktop_id = auth.desktop_client().map(|c| c.client_id);

    let all = state.accounts().await?;
    let order = state.settings.get().account_order;
    let all = ordered(all, &order);
    let lookup = auth.clone();
    let accounts: Vec<MacAccount> = blocking(move || {
        Ok(all
            .into_iter()
            .map(|account| {
                // A Keychain that can't be read reads as "no token": the
                // account is listed to sign in on the phone.
                let (origin, has_secret) = match account.provider {
                    AccountProvider::Gmail => {
                        let origin = lookup.token_origin(&account.email).unwrap_or_else(|e| {
                            tracing::warn!(account = %account.id, error = %e, "pairing: token lookup failed");
                            None
                        });
                        (origin, false)
                    }
                    AccountProvider::Imap => (
                        None,
                        penguin_imap::backend::keychain_vault().contains(&account.id),
                    ),
                    AccountProvider::Microsoft => (
                        None,
                        penguin_graph::default_auth().has_credentials(&account.id),
                    ),
                };
                MacAccount {
                    account,
                    origin,
                    has_secret,
                }
            })
            .collect())
    })
    .await?;

    let listener = server::bind()
        .await
        .map_err(|e| CmdError::other(format!("Couldn't open a port for the iPhone: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| CmdError::other(format!("Couldn't open a port for the iPhone: {e}")))?
        .port();
    let hosts = local_hosts();
    if hosts.is_empty() {
        return Err(CmdError::new(
            crate::error::ErrorCode::Network,
            "This Mac isn't on a network. Connect it to the same Wi-Fi as your iPhone, then try again.",
        ));
    }
    let offer = Offer::new().map_err(pair_error)?;
    let ttl = offer
        .expires_at()
        .saturating_duration_since(std::time::Instant::now());
    let qr = qr_code(&offer.invitation(port, hosts.clone()).to_qr_text())?;
    let session_id = session_id()?;
    let preview = accounts
        .iter()
        .map(|a| preview_account(a, mac_client_id.as_deref(), desktop_id.as_deref()))
        .collect();

    let confirm = Arc::new(Mutex::new(None));
    let task = tauri::async_runtime::spawn(run(
        app,
        session_id.clone(),
        listener,
        offer,
        accounts,
        auth,
        mac_client_id.clone(),
        confirm.clone(),
    ));
    *pairing.lock() = Some(Running {
        id: session_id.clone(),
        task,
        confirm,
    });
    tracing::info!(
        port,
        hosts = hosts.len(),
        "pair iphone: waiting for a phone"
    );
    Ok(PairStart {
        session_id,
        qr,
        expires_at: now_ms() + ttl.as_millis() as i64,
        hosts,
        port,
        accounts: preview,
        mac_client_id,
    })
}

/// Answer the Send prompt: `send` false declines (the phone is told).
#[tauri::command]
pub async fn pair_iphone_confirm(
    pairing: State<'_, Pairing>,
    session_id: String,
    send: bool,
) -> CmdResult<()> {
    let current = pairing.lock();
    let Some(running) = current.as_ref().filter(|r| r.id == session_id) else {
        return Err(CmdError::not_found("That pairing has ended. Start again."));
    };
    let answer = running
        .confirm
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    match answer {
        Some(tx) => {
            let _ = tx.send(send);
            Ok(())
        }
        None => Err(CmdError::invalid("No iPhone is waiting for an answer.")),
    }
}

#[tauri::command]
pub async fn pair_iphone_cancel(pairing: State<'_, Pairing>, session_id: String) -> CmdResult<()> {
    if pairing.stop(Some(&session_id)) {
        tracing::info!("pair iphone: cancelled");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run(
    app: AppHandle,
    session_id: String,
    listener: tokio::net::TcpListener,
    offer: Offer,
    accounts: Vec<MacAccount>,
    auth: AuthManager,
    mac_client_id: Option<String>,
    confirm: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
) {
    let mac_name = mac_name();
    let emit = {
        let app = app.clone();
        move |event: PairEvent| {
            if let Err(e) = app.emit(EVENT_PAIR_IPHONE, event) {
                tracing::warn!(error = %e, "pair iphone: failed to emit event");
            }
        }
    };
    let plan_out: Arc<Mutex<Vec<AccountMeta>>> = Arc::default();
    let decide = {
        let emit = emit.clone();
        let id = session_id.clone();
        let mac_name = mac_name.clone();
        let plan_out = plan_out.clone();
        move |phone: PhoneHello, code: String| async move {
            let desktop = auth.desktop_client();
            let metas = plan(
                &accounts,
                &phone,
                desktop.as_ref().map(|d| d.client_id.as_str()),
            );
            *plan_out.lock().unwrap_or_else(|p| p.into_inner()) = metas.clone();
            let (tx, rx) = oneshot::channel();
            *confirm.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
            emit(PairEvent::Connected {
                session_id: id.clone(),
                phone: PairPhone {
                    name: phone.device_name.clone(),
                    model: phone.device_model.clone(),
                    system: phone.system_version.clone(),
                    app_version: phone.app_version.clone(),
                },
                code,
                accounts: metas.iter().map(|m| pair_account(m, &accounts)).collect(),
                client_note: client_note(&accounts, &phone),
            });
            if !rx.await.unwrap_or(false) {
                return Decision::Decline("Declined on the Mac.".into());
            }
            emit(PairEvent::Sending {
                session_id: id.clone(),
            });
            let moving = |kind: AccountKind| -> Vec<String> {
                metas
                    .iter()
                    .filter(|m| m.status == AccountStatus::Token && m.provider == kind)
                    .map(|m| m.email.clone())
                    .collect()
            };
            let (emails, imap, microsoft) = (
                moving(AccountKind::Gmail),
                moving(AccountKind::Imap),
                moving(AccountKind::Microsoft),
            );
            let desktop_client = desktop
                .filter(|_| sends_desktop_tokens(&accounts, &metas))
                .map(|d| DesktopClient {
                    client_id: d.client_id,
                    client_secret: d.client_secret.map(SecretString::new),
                });
            let exported = blocking(move || Ok(export(&auth, &emails, &imap, &microsoft)?)).await;
            match exported {
                Ok(Exported {
                    gmail,
                    imap,
                    microsoft,
                }) => Decision::Send(Transfer {
                    mac_name,
                    google_client_id: mac_client_id,
                    vault: gmail,
                    desktop_client,
                    imap_vault: imap,
                    microsoft_vault: microsoft,
                    accounts: metas,
                }),
                Err(e) => {
                    tracing::warn!(error = %e.message, "pair iphone: couldn't export tokens");
                    Decision::Decline(format!(
                        "The Mac couldn't read its Keychain ({}). Nothing was sent.",
                        e.message
                    ))
                }
            }
        }
    };

    let outcome = server::serve(listener, offer, mac_name, decide, Timeouts::default()).await;
    let event = match outcome {
        Ok(Outcome::Done { phone, done }) => {
            let sign_in = plan_out
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .filter(|m| matches!(m.status, AccountStatus::SignIn { .. }))
                .map(|m| m.email.clone())
                .collect();
            tracing::info!(
                imported = done.imported.len(),
                failed = done.failed.len(),
                "pair iphone: done"
            );
            PairEvent::Done {
                session_id: session_id.clone(),
                phone_name: phone.device_name,
                imported: done.imported,
                failed: done
                    .failed
                    .into_iter()
                    .map(|f| PairFailure {
                        email: f.email,
                        reason: f.reason,
                    })
                    .collect(),
                sign_in,
            }
        }
        Ok(Outcome::Declined { .. }) => PairEvent::Declined {
            session_id: session_id.clone(),
        },
        Err(e) => {
            tracing::info!(error = %e, "pair iphone: ended");
            PairEvent::Failed {
                session_id: session_id.clone(),
                expired: e == PairError::Expired,
                message: e.to_string(),
            }
        }
    };
    emit(event);
    let pairing = tauri::Manager::state::<Pairing>(&app);
    let mut current = pairing.lock();
    if current.as_ref().is_some_and(|r| r.id == session_id) {
        // Our own handle: dropping it doesn't abort the finished task.
        current.take();
    }
}

/// The credentials a transfer carries, each as its own vault item.
struct Exported {
    gmail: SecretString,
    imap: Option<SecretString>,
    microsoft: Option<SecretString>,
}

/// Read what moves from the Keychain: Gmail tokens for `gmail`, IMAP
/// passwords for `imap`, Microsoft refresh tokens for `microsoft`
/// (account ids). Blocking.
fn export(
    auth: &AuthManager,
    gmail: &[String],
    imap: &[String],
    microsoft: &[String],
) -> CmdResult<Exported> {
    let mut json = auth.export_vault(gmail)?;
    // Moved, not copied: the Zeroizing buffer is left empty.
    let gmail = SecretString::new(std::mem::take(&mut *json));
    let imap = if imap.is_empty() {
        None
    } else {
        Some(SecretString::new(
            penguin_imap::backend::keychain_vault().export(imap)?,
        ))
    };
    let microsoft = if microsoft.is_empty() {
        None
    } else {
        Some(SecretString::new(
            penguin_graph::default_auth().vault().export(microsoft)?,
        ))
    };
    Ok(Exported {
        gmail,
        imap,
        microsoft,
    })
}

/// The protocol's name for an account's provider.
fn kind(provider: AccountProvider) -> AccountKind {
    match provider {
        AccountProvider::Gmail => AccountKind::Gmail,
        AccountProvider::Imap => AccountKind::Imap,
        AccountProvider::Microsoft => AccountKind::Microsoft,
    }
}

/// What happens to each account on this phone, in the Mac's order.
/// `desktop` is this Mac's "Desktop app" client id: browser sign-ins it
/// minted move with it. The phone lists only accounts it holds a working
/// token for, so one it has waiting for sign-in gets its token now. IMAP
/// passwords and Microsoft refresh tokens move whenever the Mac has one
/// (a Microsoft token carries the client that minted it, which is all the
/// phone needs to refresh it).
pub(crate) fn plan(
    accounts: &[MacAccount],
    phone: &PhoneHello,
    desktop: Option<&str>,
) -> Vec<AccountMeta> {
    let phone_client = phone.google_client_id.as_deref();
    accounts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let email = a.account.email.trim().to_lowercase();
            let status = if phone.accounts.contains(&email) {
                AccountStatus::AlreadyOnPhone
            } else if a.account.provider != AccountProvider::Gmail {
                secret_status(a)
            } else {
                match &a.origin {
                    None => AccountStatus::SignIn {
                        reason: SignInReason::NoToken,
                    },
                    Some(TokenOrigin::Desktop { client_id }) => desktop_status(client_id, desktop),
                    Some(TokenOrigin::Ios { client_id }) => {
                        if phone_client.is_some_and(|p| p.eq_ignore_ascii_case(client_id)) {
                            AccountStatus::Token
                        } else {
                            AccountStatus::SignIn {
                                reason: SignInReason::OtherClient,
                            }
                        }
                    }
                }
            };
            AccountMeta {
                email,
                display_name: a.account.display_name.clone(),
                nickname: a.account.nickname.clone(),
                color: a.account.color.clone(),
                order: i as u32,
                status,
                provider: kind(a.account.provider),
                provider_config: provider_config(&a.account),
            }
        })
        .collect()
}

/// An IMAP or Microsoft account moves when the Mac has its secret.
fn secret_status(a: &MacAccount) -> AccountStatus {
    if a.has_secret {
        AccountStatus::Token
    } else {
        AccountStatus::SignIn {
            reason: SignInReason::NoToken,
        }
    }
}

/// IMAP and Microsoft: the account's server settings for the phone (no
/// secrets: `ProviderConfig` holds none).
fn provider_config(account: &Account) -> Option<serde_json::Value> {
    if account.provider == AccountProvider::Gmail {
        return None;
    }
    serde_json::to_value(&account.provider_config).ok()
}

/// A browser sign-in moves with this Mac's desktop client when that client
/// minted it (a token stored without an id is the configured client's).
fn desktop_status(minted_by: &Option<String>, desktop: Option<&str>) -> AccountStatus {
    match desktop {
        None => AccountStatus::SignIn {
            reason: SignInReason::DesktopClient,
        },
        Some(d) if minted_by.as_deref().is_none_or(|id| id == d) => AccountStatus::Token,
        // Minted by a client this Mac no longer has: it can't refresh it
        // either, so neither can the phone.
        Some(_) => AccountStatus::SignIn {
            reason: SignInReason::OtherClient,
        },
    }
}

/// Whether the transfer carries browser sign-ins (and so needs the
/// desktop client).
fn sends_desktop_tokens(accounts: &[MacAccount], metas: &[AccountMeta]) -> bool {
    metas
        .iter()
        .filter(|m| m.status == AccountStatus::Token)
        .any(|m| {
            accounts.iter().any(|a| {
                a.account.email.eq_ignore_ascii_case(&m.email)
                    && matches!(a.origin, Some(TokenOrigin::Desktop { .. }))
            })
        })
}

/// Before a phone connects: what would move if its build uses this Mac's
/// sign-in-sheet client.
fn preview_account(a: &MacAccount, mac_client: Option<&str>, desktop: Option<&str>) -> PairAccount {
    if a.account.provider != AccountProvider::Gmail {
        return describe(&a.account, &secret_status(a));
    }
    let status = match &a.origin {
        Some(TokenOrigin::Ios { client_id })
            if mac_client.is_none_or(|m| m.eq_ignore_ascii_case(client_id)) =>
        {
            AccountStatus::Token
        }
        Some(TokenOrigin::Ios { .. }) => AccountStatus::SignIn {
            reason: SignInReason::OtherClient,
        },
        Some(TokenOrigin::Desktop { client_id }) => desktop_status(client_id, desktop),
        None => AccountStatus::SignIn {
            reason: SignInReason::NoToken,
        },
    };
    describe(&a.account, &status)
}

fn pair_account(meta: &AccountMeta, accounts: &[MacAccount]) -> PairAccount {
    match accounts
        .iter()
        .find(|a| a.account.email.eq_ignore_ascii_case(&meta.email))
    {
        Some(a) => describe(&a.account, &meta.status),
        None => PairAccount {
            email: meta.email.clone(),
            name: None,
            color: meta.color.clone(),
            status: status_name(&meta.status),
            reason: None,
        },
    }
}

fn describe(account: &Account, status: &AccountStatus) -> PairAccount {
    PairAccount {
        email: account.email.clone(),
        name: account
            .nickname
            .clone()
            .or_else(|| account.display_name.clone())
            .filter(|n| !n.trim().is_empty()),
        color: account.color.clone(),
        status: status_name(status),
        reason: match status {
            AccountStatus::SignIn { reason } => Some(reason_text(account.provider, *reason).into()),
            _ => None,
        },
    }
}

fn status_name(status: &AccountStatus) -> &'static str {
    match status {
        AccountStatus::Token => "token",
        AccountStatus::AlreadyOnPhone => "alreadyOnPhone",
        AccountStatus::SignIn { .. } => "signIn",
    }
}

fn reason_text(provider: AccountProvider, reason: SignInReason) -> &'static str {
    match (provider, reason) {
        (AccountProvider::Imap, _) => return "This Mac has no saved password for it right now.",
        (AccountProvider::Microsoft, _) => {
            return "This Mac has no Microsoft sign-in for it right now."
        }
        (AccountProvider::Gmail, _) => {}
    }
    match reason {
        SignInReason::DesktopClient => {
            "Signed in through the browser, and this Mac has no Google client file to send with it."
        }
        SignInReason::OtherClient => "Signed in with a Google client the iPhone can't use.",
        SignInReason::NoToken => "This Mac has no sign-in for it right now.",
    }
}

/// Why no iOS-client token can move: the phone's client isn't this Mac's.
fn client_note(accounts: &[MacAccount], phone: &PhoneHello) -> Option<String> {
    let mac_clients: Vec<&str> = accounts
        .iter()
        .filter_map(|a| match &a.origin {
            Some(TokenOrigin::Ios { client_id }) => Some(client_id.as_str()),
            _ => None,
        })
        .collect();
    if mac_clients.is_empty() {
        return None;
    }
    let phone_client = phone.google_client_id.as_deref();
    if mac_clients
        .iter()
        .any(|c| phone_client.is_some_and(|p| p.eq_ignore_ascii_case(c)))
    {
        return None;
    }
    Some(match phone_client {
        Some(p) => format!(
            "This iPhone's Penguin signs in with a different Google client ({}) than this Mac's sign-in sheet ({}), so its sign-ins can't move. The accounts will be listed on the iPhone to sign in there.",
            short_client(p),
            short_client(mac_clients[0])
        ),
        None => "This iPhone's Penguin has no Google client, so it can't take accounts. Build it with PENGUIN_GOOGLE_IOS_CLIENT_ID.".into(),
    })
}

/// "1234…abcd.apps…": enough to tell two client ids apart.
fn short_client(id: &str) -> String {
    let core = id.strip_suffix(".apps.googleusercontent.com").unwrap_or(id);
    if core.len() <= 12 {
        return core.to_owned();
    }
    format!("{}…{}", &core[..6], &core[core.len() - 4..])
}

/// Accounts in the sidebar's order: `accountOrder` first, the rest as
/// stored (src/app/accountOrder.ts).
fn ordered(mut accounts: Vec<Account>, order: &[String]) -> Vec<Account> {
    let mut out = Vec::with_capacity(accounts.len());
    for id in order {
        if let Some(i) = accounts.iter().position(|a| &a.id == id) {
            out.push(accounts.remove(i));
        }
    }
    out.extend(accounts);
    out
}

/// Where the phone can reach this Mac: IPv4 addresses on Wi-Fi and
/// Ethernet first (`en*`), other private ones after, link-local last, then
/// the Bonjour name. VPN tunnels and loopback are left out.
fn local_hosts() -> Vec<String> {
    let mut found: Vec<(u8, String)> = Vec::new();
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => {
            for iface in interfaces {
                let std::net::IpAddr::V4(ip) = iface.ip() else {
                    continue;
                };
                if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                    continue;
                }
                let name = iface.name.as_str();
                if [
                    "utun", "ipsec", "ppp", "tun", "tap", "gif", "stf", "bridge", "awdl", "llw",
                ]
                .iter()
                .any(|p| name.starts_with(p))
                {
                    continue;
                }
                let rank = if ip.is_link_local() {
                    3
                } else if name.starts_with("en")
                    || name.starts_with("eth")
                    || name.starts_with("wl")
                {
                    if ip.is_private() {
                        0
                    } else {
                        1
                    }
                } else {
                    2
                };
                found.push((rank, ip.to_string()));
            }
        }
        Err(e) => tracing::warn!(error = %e, "pair iphone: can't list network interfaces"),
    }
    found.sort();
    found.dedup_by(|a, b| a.1 == b.1);
    let mut hosts: Vec<String> = found
        .into_iter()
        .map(|(_, h)| h)
        .take(MAX_HOSTS - 1)
        .collect();
    if !hosts.is_empty() {
        if let Some(name) = bonjour_name() {
            hosts.push(name);
        }
    }
    hosts
}

/// `Name.local` when the host name is a Bonjour name.
fn bonjour_name() -> Option<String> {
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    let valid = name.ends_with(".local")
        && name.len() <= 253
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    valid.then_some(name)
}

/// What the phone calls this Mac: its Computer Name ("Ada's MacBook Pro").
fn mac_name() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("/usr/sbin/scutil")
            .args(["--get", "ComputerName"])
            .output()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if out.status.success() && !name.is_empty() {
                return name;
            }
        }
    }
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let host = host.trim_end_matches(".local").trim();
    if host.is_empty() {
        "Mac".into()
    } else {
        host.to_owned()
    }
}

fn qr_code(text: &str) -> CmdResult<QrCode> {
    let qr = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium)
        .map_err(|e| CmdError::other(format!("Couldn't draw the pairing code: {e:?}")))?;
    let size = qr.size();
    let mut path = String::with_capacity((size * size * 6) as usize);
    for y in 0..size {
        let mut x = 0;
        while x < size {
            if qr.get_module(x, y) {
                // One run of dark modules per path segment.
                let start = x;
                while x < size && qr.get_module(x, y) {
                    x += 1;
                }
                path.push_str(&format!("M{start} {y}h{}v1H{start}z", x - start));
            } else {
                x += 1;
            }
        }
    }
    Ok(QrCode {
        size: size as u32,
        path,
    })
}

fn session_id() -> CmdResult<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|e| CmdError::other(format!("system RNG unavailable: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn pair_error(e: PairError) -> CmdError {
    CmdError::other(e.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "123456789012-phone.apps.googleusercontent.com";
    const DESK: &str = "123456789012-desk.apps.googleusercontent.com";

    fn account(email: &str, color: &str) -> Account {
        Account {
            id: email.into(),
            email: email.into(),
            color: color.into(),
            ..Account::default()
        }
    }

    fn mac(email: &str, origin: Option<TokenOrigin>) -> MacAccount {
        MacAccount {
            account: account(email, "#4F7CFF"),
            origin,
            has_secret: false,
        }
    }

    fn other(email: &str, provider: AccountProvider, has_secret: bool) -> MacAccount {
        let mut account = account(email, "#2FA37A");
        account.provider = provider;
        account.provider_config = penguin_core::ProviderConfig {
            auth: Some(match provider {
                AccountProvider::Microsoft => penguin_core::AuthMethod::MicrosoftOAuth,
                _ => penguin_core::AuthMethod::AppPassword,
            }),
            imap: (provider == AccountProvider::Imap).then(|| penguin_core::ServerSettings {
                host: "imap.harbor.example".into(),
                port: 993,
                security: penguin_core::MailSecurity::Tls,
                username: email.into(),
            }),
            client_id: (provider == AccountProvider::Microsoft)
                .then(|| "1b2c3d4e-0000-1111-2222-333344445555".into()),
            host: Some("imapGeneric".into()),
            ..Default::default()
        };
        MacAccount {
            account,
            origin: None,
            has_secret,
        }
    }

    fn ios(id: &str) -> Option<TokenOrigin> {
        Some(TokenOrigin::Ios {
            client_id: id.into(),
        })
    }

    fn desk(id: Option<&str>) -> Option<TokenOrigin> {
        Some(TokenOrigin::Desktop {
            client_id: id.map(str::to_owned),
        })
    }

    fn phone(client: Option<&str>, has: &[&str]) -> PhoneHello {
        PhoneHello {
            device_name: "Ada's iPhone".into(),
            device_model: "iPhone17,1".into(),
            system_version: "iOS 26.0".into(),
            app_version: "ios-1.0".into(),
            google_client_id: client.map(str::to_owned),
            accounts: has.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn statuses(metas: &[AccountMeta]) -> Vec<(&str, &AccountStatus)> {
        metas
            .iter()
            .map(|m| (m.email.as_str(), &m.status))
            .collect()
    }

    #[test]
    fn plan_sends_only_what_the_phone_can_use_and_lacks() {
        let accounts = vec![
            mac("ada@example.com", ios(PHONE)),
            mac("Grace@Example.com", desk(None)),
            mac("hopper@example.com", desk(Some(DESK))),
            mac(
                "linus@example.com",
                ios("123456789012-other.apps.googleusercontent.com"),
            ),
            mac(
                "stale@example.com",
                desk(Some("123456789012-old.apps.googleusercontent.com")),
            ),
            mac("nobody@example.com", None),
            mac("kept@example.com", ios(PHONE)),
        ];
        let metas = plan(
            &accounts,
            &phone(Some(PHONE), &["kept@example.com"]),
            Some(DESK),
        );
        let other = AccountStatus::SignIn {
            reason: SignInReason::OtherClient,
        };
        assert_eq!(
            statuses(&metas),
            [
                ("ada@example.com", &AccountStatus::Token),
                // Browser sign-ins move with this Mac's desktop client.
                ("grace@example.com", &AccountStatus::Token),
                ("hopper@example.com", &AccountStatus::Token),
                ("linus@example.com", &other),
                // Minted by a desktop client this Mac no longer has.
                ("stale@example.com", &other),
                (
                    "nobody@example.com",
                    &AccountStatus::SignIn {
                        reason: SignInReason::NoToken
                    }
                ),
                ("kept@example.com", &AccountStatus::AlreadyOnPhone),
            ]
        );
        assert_eq!(
            metas.iter().map(|m| m.order).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5, 6]
        );
        assert!(sends_desktop_tokens(&accounts, &metas));
        assert_eq!(client_note(&accounts, &phone(Some(PHONE), &[])), None);
    }

    #[test]
    fn imap_and_microsoft_accounts_move_with_their_settings() {
        let accounts = vec![
            mac("ada@example.com", ios(PHONE)),
            other("sam@harbor.example", AccountProvider::Imap, true),
            other("bea@outlook.example", AccountProvider::Microsoft, true),
            other("locked@harbor.example", AccountProvider::Imap, false),
            other("kept@outlook.example", AccountProvider::Microsoft, true),
        ];
        // Whatever Google client the phone has: these don't depend on it.
        let metas = plan(
            &accounts,
            &phone(None, &["kept@outlook.example"]),
            Some(DESK),
        );
        assert_eq!(
            statuses(&metas),
            [
                (
                    "ada@example.com",
                    &AccountStatus::SignIn {
                        reason: SignInReason::OtherClient
                    }
                ),
                ("sam@harbor.example", &AccountStatus::Token),
                ("bea@outlook.example", &AccountStatus::Token),
                (
                    "locked@harbor.example",
                    &AccountStatus::SignIn {
                        reason: SignInReason::NoToken
                    }
                ),
                ("kept@outlook.example", &AccountStatus::AlreadyOnPhone),
            ]
        );
        let kinds: Vec<AccountKind> = metas.iter().map(|m| m.provider).collect();
        assert_eq!(
            kinds,
            [
                AccountKind::Gmail,
                AccountKind::Imap,
                AccountKind::Microsoft,
                AccountKind::Imap,
                AccountKind::Microsoft
            ]
        );
        assert_eq!(metas[0].provider_config, None);
        let imap = metas[1].provider_config.as_ref().unwrap();
        assert_eq!(imap["imap"]["host"], "imap.harbor.example");
        assert_eq!(imap["auth"], "appPassword");
        let ms = metas[2].provider_config.as_ref().unwrap();
        assert_eq!(ms["clientId"], "1b2c3d4e-0000-1111-2222-333344445555");
        // Only Gmail browser sign-ins need the desktop client.
        assert!(!sends_desktop_tokens(&accounts, &metas));
        // Before a phone connects, and the wording for a missing password.
        let p = preview_account(&accounts[3], Some(PHONE), Some(DESK));
        assert_eq!(p.status, "signIn");
        assert!(p.reason.unwrap().contains("password"));
        assert_eq!(
            preview_account(&accounts[2], Some(PHONE), Some(DESK)).status,
            "token"
        );
    }

    #[test]
    fn accounts_the_phone_waits_to_sign_in_get_their_tokens() {
        // Justin's case: paired once with version 1, so the phone has every
        // account as "needs sign-in" and lists none as working. Pairing
        // again sends them all.
        let accounts = vec![
            mac("ada@example.com", desk(None)),
            mac("grace@example.com", desk(Some(DESK))),
        ];
        let metas = plan(&accounts, &phone(Some(PHONE), &[]), Some(DESK));
        assert!(metas.iter().all(|m| m.status == AccountStatus::Token));
        assert!(sends_desktop_tokens(&accounts, &metas));
        // Ones the phone has working are left alone, and with only those
        // the desktop client isn't sent.
        let metas = plan(
            &accounts,
            &phone(Some(PHONE), &["ada@example.com", "grace@example.com"]),
            Some(DESK),
        );
        assert!(metas
            .iter()
            .all(|m| m.status == AccountStatus::AlreadyOnPhone));
        assert!(!sends_desktop_tokens(&accounts, &metas));
    }

    #[test]
    fn without_a_desktop_client_browser_sign_ins_stay() {
        let accounts = vec![mac("ada@example.com", desk(None))];
        let metas = plan(&accounts, &phone(Some(PHONE), &[]), None);
        assert_eq!(
            metas[0].status,
            AccountStatus::SignIn {
                reason: SignInReason::DesktopClient
            }
        );
        assert!(!sends_desktop_tokens(&accounts, &metas));
    }

    #[test]
    fn a_phone_with_another_client_gets_no_ios_tokens_and_a_note() {
        let accounts = vec![
            mac("ada@example.com", ios(PHONE)),
            mac("grace@example.com", desk(None)),
        ];
        let other = "123456789012-zzzz.apps.googleusercontent.com";
        let metas = plan(&accounts, &phone(Some(other), &[]), Some(DESK));
        assert_eq!(
            statuses(&metas),
            [
                (
                    "ada@example.com",
                    &AccountStatus::SignIn {
                        reason: SignInReason::OtherClient
                    }
                ),
                // The browser sign-in doesn't depend on the phone's client.
                ("grace@example.com", &AccountStatus::Token),
            ]
        );
        let note = client_note(&accounts, &phone(Some(other), &[])).unwrap();
        assert!(note.contains("different Google client"), "{note}");
        // Browser sign-ins only: no client note.
        let desktop = vec![mac("ada@example.com", desk(None))];
        assert_eq!(client_note(&desktop, &phone(Some(other), &[])), None);
    }

    #[test]
    fn preview_and_order() {
        let a = mac("ada@example.com", ios(PHONE));
        assert_eq!(preview_account(&a, Some(PHONE), Some(DESK)).status, "token");
        assert_eq!(
            preview_account(
                &a,
                Some("123456789012-x.apps.googleusercontent.com"),
                Some(DESK)
            )
            .status,
            "signIn"
        );
        let d = mac("d@example.com", desk(None));
        assert_eq!(preview_account(&d, Some(PHONE), Some(DESK)).status, "token");
        let p = preview_account(&d, Some(PHONE), None);
        assert_eq!(p.status, "signIn");
        assert!(p.reason.unwrap().contains("browser"));

        let list = vec![
            account("a@example.com", "#1"),
            account("b@example.com", "#2"),
            account("c@example.com", "#3"),
        ];
        let ids: Vec<String> = ordered(list, &["c@example.com".into(), "gone@example.com".into()])
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, ["c@example.com", "a@example.com", "b@example.com"]);
    }

    #[test]
    fn qr_code_draws_and_hides_in_debug() {
        let offer = Offer::new().unwrap();
        let text = offer
            .invitation(50123, vec!["192.168.1.20".into(), "Ada-Mac.local".into()])
            .to_qr_text();
        let qr = qr_code(&text).unwrap();
        assert!(qr.size >= 21 && qr.size <= 57, "{}", qr.size);
        assert!(qr.path.starts_with('M'));
        assert!(!format!("{qr:?}").contains('M'));
    }

    #[test]
    fn this_machines_addresses_make_a_valid_code() {
        // Whatever interfaces the test machine has, the code they produce
        // must be one the phone accepts.
        let hosts = local_hosts();
        assert!(hosts.len() <= MAX_HOSTS);
        if hosts.is_empty() {
            return;
        }
        let offer = Offer::new().unwrap();
        let text = offer.invitation(50123, hosts.clone()).to_qr_text();
        assert_eq!(penguin_pair::Invitation::parse(&text).unwrap().hosts, hosts);
    }

    #[test]
    fn short_client_ids() {
        assert_eq!(
            short_client("123456789012-abcdefghij.apps.googleusercontent.com"),
            "123456…ghij"
        );
        assert_eq!(short_client("1-abc.apps.googleusercontent.com"), "1-abc");
    }
}
