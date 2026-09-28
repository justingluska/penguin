//! Microsoft sign-in and tokens: authorization code + PKCE against the
//! `/common` authority with the user's own public client (no secret), a
//! loopback redirect on a random localhost port, refresh tokens in the
//! Microsoft Keychain vault (`penguin_provider::credentials`), access
//! tokens in memory only.
//!
//! Refresh is single-flight per account: one slot lock per account, held
//! while refreshing, so concurrent callers wait and reuse the result.
//! Microsoft rotates refresh tokens; every rotation is written back.
//!
//! Errors keep Microsoft's AADSTS code (the UI keys its help text on it,
//! `microsoftErrorText`) but never Microsoft's description, which can
//! contain the user's address.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use penguin_provider::credentials::{MicrosoftCredential, SecretVault};
use penguin_provider::{Error, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::http::{Endpoints, Method, Request, Response, Transport};
use crate::loopback::{self, Callback, LoopbackServer, PageKind};

/// Delegated scopes: sign-in, a refresh token, the profile, mail read and
/// write, send, mailbox settings (signature, time zone).
pub const SCOPES: &str =
    "openid profile email offline_access User.Read Mail.ReadWrite Mail.Send MailboxSettings.Read";
/// Without these Penguin can't work as a mail client.
const REQUIRED_SCOPES: [&str; 2] = ["Mail.ReadWrite", "Mail.Send"];
/// How long the browser sign-in may take.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Refresh when the cached access token has less than this left.
const REFRESH_MARGIN: Duration = Duration::from_secs(120);
/// Microsoft access tokens last 60–90 minutes; used if `expires_in` is absent.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(3600);

// ---------- PKCE ----------

fn random_urlsafe(n_bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; n_bytes];
    getrandom::fill(&mut buf).map_err(|e| Error::Other(format!("system RNG unavailable: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

pub(crate) fn challenge_s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------- token endpoint ----------

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

impl TokenResponse {
    /// Granted scopes without the Graph resource prefix.
    pub fn granted_scopes(&self) -> Vec<String> {
        self.scope
            .as_deref()
            .unwrap_or_default()
            .split_ascii_whitespace()
            .map(|s| {
                s.strip_prefix("https://graph.microsoft.com/")
                    .unwrap_or(s)
                    .to_string()
            })
            .collect()
    }

    fn cached(&self, now: SystemTime) -> CachedToken {
        let lifetime = self
            .expires_in
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_LIFETIME);
        CachedToken {
            access_token: self.access_token.clone(),
            expires_at: now + lifetime,
        }
    }
}

#[derive(Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    error_codes: Vec<u64>,
}

/// Which request failed: an expired refresh token means "sign in again",
/// a rejected code means the sign-in itself failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grant {
    Refresh,
    AuthorizationCode,
}

/// The first `AADSTS<digits>` in `text`.
pub(crate) fn aadsts_code(text: &str) -> Option<u64> {
    let i = text.find("AADSTS")?;
    let digits: String = text[i + 6..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// User-facing text for an AADSTS code (the UI maps the code to its own
/// help too). Microsoft's description is not used: it can name the user.
pub(crate) fn describe_aadsts(code: u64, error: &str) -> String {
    let text = match code {
        65001 | 90094 | 90095 => {
            "Your organization needs an admin to approve Penguin (need admin approval)"
        }
        65004 => "Consent was declined",
        700016 => "Microsoft doesn't know this Application (client) ID. Copy the ID from the app registration's Overview page (not the Directory (tenant) ID)",
        50011 => "The redirect URI doesn't match. Add http://localhost under Mobile and desktop applications in the app registration's Authentication page",
        7000218 => "The app registration expects a client secret: http://localhost is on the Web platform. Remove it there and add it under Mobile and desktop applications",
        9002326 => "The redirect is registered as a single-page application. Add http://localhost under Mobile and desktop applications instead",
        9002331 | 50020 | 500200 | 50194 => "The app registration doesn't allow this kind of account. Set Supported account types to include personal Microsoft accounts and any organization",
        53000 | 53003 => "Your organization only allows approved devices or apps (conditional access)",
        50076 | 50079 | 50158 => "Microsoft needs you to complete multi-factor sign-in again",
        70000 | 70008 | 700082 | 50173 | 50078 | 54005 | 70043 => "The sign-in has expired or was revoked",
        _ => "",
    };
    if text.is_empty() {
        format!("AADSTS{code}: Microsoft sign-in failed ({error})")
    } else {
        format!("AADSTS{code}: {text}")
    }
}

/// Map a non-2xx token endpoint answer.
pub(crate) fn token_error(status: u16, body: &[u8], grant: Grant) -> Error {
    if status == 429 {
        return Error::RateLimited;
    }
    let Ok(parsed) = serde_json::from_slice::<TokenErrorResponse>(body) else {
        return Error::Http {
            status,
            body: format!("token endpoint returned {} bytes", body.len()),
        };
    };
    let code = parsed
        .error_codes
        .first()
        .copied()
        .or_else(|| parsed.error_description.as_deref().and_then(aadsts_code));
    let text = match code {
        Some(c) => describe_aadsts(c, &parsed.error),
        None => format!("Microsoft sign-in failed ({})", parsed.error),
    };
    if status >= 500 {
        return Error::Http { status, body: text };
    }
    match grant {
        Grant::Refresh => match parsed.error.as_str() {
            "invalid_grant"
            | "interaction_required"
            | "consent_required"
            | "login_required"
            | "invalid_client"
            | "unauthorized_client" => Error::NeedsReauth(text),
            _ => Error::OAuth(text),
        },
        Grant::AuthorizationCode => {
            if parsed.error == "consent_required" && code.is_none() {
                Error::OAuth(describe_aadsts(65001, &parsed.error))
            } else {
                Error::OAuth(text)
            }
        }
    }
}

/// The redirect carried an error instead of a code.
pub(crate) fn callback_error(error: &str, description: &str, subcode: &str) -> Error {
    let code = aadsts_code(description);
    if error == "access_denied" && (subcode == "cancel" || code == Some(65004)) {
        return Error::SignInCancelled;
    }
    if error == "consent_required" || matches!(code, Some(65001 | 90094 | 90095)) {
        return Error::OAuth(describe_aadsts(code.unwrap_or(65001), error));
    }
    match code {
        Some(c) => Error::OAuth(describe_aadsts(c, error)),
        None if error == "access_denied" => Error::SignInCancelled,
        None => Error::OAuth(format!("Microsoft sign-in failed ({error})")),
    }
}

/// ID-token claims Penguin reads. The token came straight from the token
/// endpoint over TLS in answer to our own code exchange, so its signature
/// isn't checked.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct IdClaims {
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tid: Option<String>,
    #[serde(default)]
    pub oid: Option<String>,
}

pub(crate) fn id_token_claims(id_token: &str) -> Option<IdClaims> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at: SystemTime,
}

impl CachedToken {
    fn is_fresh(&self, now: SystemTime) -> bool {
        now + REFRESH_MARGIN < self.expires_at
    }
    fn is_unexpired(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }
}

impl fmt::Debug for CachedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedToken")
            .field("access_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

// ---------- the account's identity ----------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Me {
    display_name: Option<String>,
    mail: Option<String>,
    user_principal_name: Option<String>,
    proxy_addresses: Vec<String>,
}

/// Who signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    /// The address as Microsoft reports it (matches the one typed).
    pub email: String,
    pub display_name: Option<String>,
    /// The directory (tenant) id; the consumer tenant for personal accounts.
    pub tenant_id: Option<String>,
}

/// How the app shows a sign-in.
pub struct SignInUi<'a> {
    /// Opens the authorization URL in the system browser.
    pub open_browser: &'a (dyn Fn(&str) + Send + Sync),
    /// After the browser shows its success page (the app comes to front).
    pub on_success: Option<&'a (dyn Fn() + Send + Sync)>,
}

type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;
type Slot = Arc<Mutex<Option<CachedToken>>>;

/// Tokens for every Microsoft account in the process.
pub struct Auth {
    transport: Arc<dyn Transport>,
    endpoints: Endpoints,
    vault: Arc<SecretVault<MicrosoftCredential>>,
    clock: Clock,
    slots: StdMutex<HashMap<String, Slot>>,
}

impl Auth {
    pub fn new(
        transport: Arc<dyn Transport>,
        endpoints: Endpoints,
        vault: Arc<SecretVault<MicrosoftCredential>>,
    ) -> Auth {
        Auth::with_clock(transport, endpoints, vault, Arc::new(SystemTime::now))
    }

    pub(crate) fn with_clock(
        transport: Arc<dyn Transport>,
        endpoints: Endpoints,
        vault: Arc<SecretVault<MicrosoftCredential>>,
        clock: Clock,
    ) -> Auth {
        Auth {
            transport,
            endpoints,
            vault,
            clock,
            slots: StdMutex::default(),
        }
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    fn slot(&self, account_id: &str) -> Slot {
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(account_id.to_string())
            .or_default()
            .clone()
    }

    async fn vault_op<T: Send + 'static>(
        &self,
        f: impl FnOnce(&SecretVault<MicrosoftCredential>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || f(&vault))
            .await
            .map_err(|e| Error::Other(format!("keychain task failed: {e}")))?
    }

    /// Whether a refresh token is stored (may block on the Keychain once).
    pub fn has_credentials(&self, account_id: &str) -> bool {
        self.vault.contains(account_id)
    }

    /// A valid access token, refreshing within [`REFRESH_MARGIN`] of expiry.
    /// A rejected refresh token is `NeedsReauth`.
    pub async fn access_token(&self, account_id: &str) -> Result<String> {
        let slot = self.slot(account_id);
        let mut cached = slot.lock().await;
        let now = (self.clock)();
        if let Some(t) = cached.as_ref().filter(|t| t.is_fresh(now)) {
            return Ok(t.access_token.clone());
        }
        let id = account_id.to_string();
        let credential = self
            .vault_op(move |v| v.load(&id))
            .await?
            .ok_or_else(|| Error::NeedsReauth("Sign in to Microsoft again".into()))?;
        let form = [
            ("client_id", credential.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", credential.refresh_token.as_str()),
            ("scope", SCOPES),
        ];
        match self.post_token(&form, Grant::Refresh).await {
            Ok(tokens) => {
                let mut updated = credential.clone();
                if let Some(rotated) = tokens.refresh_token.as_deref().filter(|t| !t.is_empty()) {
                    updated.refresh_token = rotated.to_string();
                    updated.obtained_at = unix_secs(now);
                }
                let scopes = tokens.granted_scopes();
                if !scopes.is_empty() {
                    updated.scopes = scopes;
                }
                if updated != credential {
                    let id = account_id.to_string();
                    self.vault_op(move |v| v.save(&id, &updated)).await?;
                }
                let token = tokens.cached(now);
                let access = token.access_token.clone();
                *cached = Some(token);
                tracing::debug!(account = %account_id, "Microsoft access token refreshed");
                Ok(access)
            }
            // A blip inside the margin shouldn't fail callers while the
            // current token still works.
            Err(Error::Network(e)) => match cached.as_ref().filter(|t| t.is_unexpired(now)) {
                Some(t) => Ok(t.access_token.clone()),
                None => Err(Error::Network(e)),
            },
            Err(e) => {
                if matches!(e, Error::NeedsReauth(_)) {
                    *cached = None;
                    tracing::warn!(account = %account_id, "Microsoft refresh token rejected; account needs to sign in again");
                }
                Err(e)
            }
        }
    }

    /// Drop the cached access token (Graph answered 401 to it).
    pub async fn invalidate(&self, account_id: &str) {
        *self.slot(account_id).lock().await = None;
    }

    /// Forget the account's tokens. Public clients have nothing to revoke
    /// server-side; the refresh token just stops being used.
    pub async fn forget(&self, account_id: &str) -> Result<()> {
        let slot = self.slot(account_id);
        let mut cached = slot.lock().await;
        *cached = None;
        let id = account_id.to_string();
        self.vault_op(move |v| v.delete(&id)).await?;
        drop(cached);
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account_id);
        Ok(())
    }

    async fn post_token(&self, form: &[(&str, &str)], grant: Grant) -> Result<TokenResponse> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let resp: Response = self
            .transport
            .send(Request {
                method: Method::Post,
                url: self.endpoints.token.clone(),
                headers: vec![
                    (
                        "Content-Type".into(),
                        "application/x-www-form-urlencoded".into(),
                    ),
                    ("Accept".into(), "application/json".into()),
                ],
                body: Some(body.into_bytes()),
            })
            .await?;
        if !resp.is_success() {
            return Err(token_error(resp.status, &resp.body, grant));
        }
        resp.json("token response")
    }

    /// The authorization URL for one sign-in.
    pub(crate) fn authorization_url(
        &self,
        client_id: &str,
        redirect_uri: &str,
        challenge: &str,
        state: &str,
        login_hint: &str,
    ) -> Result<String> {
        let mut url = url::Url::parse(&self.endpoints.authorize)
            .map_err(|e| Error::OAuth(format!("bad authorization endpoint: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client_id", client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("response_mode", "query")
                .append_pair("scope", SCOPES)
                .append_pair("state", state)
                .append_pair("code_challenge", challenge)
                .append_pair("code_challenge_method", "S256");
            let hint = login_hint.trim();
            if !hint.is_empty() {
                q.append_pair("login_hint", hint);
            }
        }
        Ok(url.to_string())
    }

    /// Browser sign-in for `account_id` (the typed address, lowercased)
    /// with the user's client. Signing in as anyone else is refused
    /// (nothing is saved). On success the refresh token is in the vault
    /// under `account_id` and an access token is cached.
    pub async fn sign_in(
        &self,
        client_id: &str,
        account_id: &str,
        ui: &SignInUi<'_>,
    ) -> Result<SignedIn> {
        let mut server = LoopbackServer::bind().await?;
        let redirect_uri = server.redirect_uri();
        let verifier = random_urlsafe(32)?;
        let state = random_urlsafe(24)?;
        let url = self.authorization_url(
            client_id,
            &redirect_uri,
            &challenge_s256(&verifier),
            &state,
            account_id,
        )?;
        tracing::info!(
            port = server.port(),
            "opening browser for Microsoft sign-in"
        );
        (ui.open_browser)(&url);

        let wait = async {
            loop {
                let Some((callback, stream)) = server.next_callback().await else {
                    return Err(Error::OAuth("sign-in listener stopped unexpectedly".into()));
                };
                if !constant_time_eq(callback.state(), &state) {
                    tracing::warn!("ignoring Microsoft redirect with mismatched state");
                    let body = loopback::page(
                        PageKind::Error,
                        "This sign-in link has expired",
                        "Return to Penguin and start sign-in again.",
                    );
                    loopback::respond(stream, 400, &body).await;
                    continue;
                }
                return Ok((callback, stream));
            }
        };
        let (callback, stream) = tokio::time::timeout(SIGN_IN_TIMEOUT, wait)
            .await
            .map_err(|_| Error::OAuth("sign-in timed out after 5 minutes; try again".into()))??;

        let result = match callback {
            Callback::Code { code, .. } => {
                self.complete_sign_in(client_id, account_id, &code, &verifier, &redirect_uri)
                    .await
            }
            Callback::Denied {
                error,
                description,
                subcode,
                ..
            } => {
                tracing::info!(%error, "Microsoft sign-in was not completed");
                Err(callback_error(&error, &description, &subcode))
            }
        };
        let body = match &result {
            Ok(s) => loopback::page(
                PageKind::Success,
                "Signed in, returning to Penguin…",
                &format!(
                    "{} is connected. If this tab stays open, you can close it.",
                    s.email
                ),
            ),
            Err(Error::SignInCancelled) => loopback::page(
                PageKind::Error,
                "Sign-in cancelled",
                "Nothing was connected. You can close this tab.",
            ),
            Err(e) => loopback::page(
                PageKind::Error,
                "Sign-in didn't finish",
                &format!("{}. Return to Penguin to try again.", user_text(e)),
            ),
        };
        let status = if result.is_ok() { 200 } else { 400 };
        loopback::respond(stream, status, &body).await;
        if let (Ok(_), Some(hook)) = (&result, ui.on_success) {
            hook();
        }
        result
    }

    /// Exchange the code, check the grant and the identity, store the tokens.
    pub(crate) async fn complete_sign_in(
        &self,
        client_id: &str,
        account_id: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<SignedIn> {
        let form = [
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", SCOPES),
        ];
        let tokens = self.post_token(&form, Grant::AuthorizationCode).await?;
        let now = (self.clock)();
        let scopes = tokens.granted_scopes();
        let missing: Vec<&str> = REQUIRED_SCOPES
            .iter()
            .copied()
            .filter(|want| !scopes.iter().any(|s| s.eq_ignore_ascii_case(want)))
            .collect();
        if !missing.is_empty() {
            return Err(Error::OAuth(format!(
                "Microsoft didn't grant {}. Add it under API permissions (Microsoft Graph, delegated) and sign in again",
                missing.join(" and ")
            )));
        }
        let Some(refresh_token) = tokens.refresh_token.clone().filter(|t| !t.is_empty()) else {
            return Err(Error::OAuth(
                "Microsoft didn't return a refresh token; add the offline_access permission and sign in again".into(),
            ));
        };
        let claims = tokens
            .id_token
            .as_deref()
            .and_then(id_token_claims)
            .unwrap_or_default();
        let me = self
            .me(&tokens.access_token, "displayName,mail,userPrincipalName")
            .await?;
        let known: Vec<String> = [
            me.mail.clone(),
            me.user_principal_name.clone(),
            claims.preferred_username.clone(),
            claims.email.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let want = account_id.trim().to_lowercase();
        let matches = |list: &[String]| list.iter().any(|a| a.trim().eq_ignore_ascii_case(&want));
        // A work address typed as one of its aliases: read the aliases
        // (only work accounts have them; personal accounts may refuse the
        // property, which just means there are none to match).
        let aliases: Vec<String> = if matches(&known) {
            Vec::new()
        } else {
            match self.me(&tokens.access_token, "proxyAddresses").await {
                Ok(m) => m
                    .proxy_addresses
                    .iter()
                    .filter_map(|p| {
                        p.strip_prefix("smtp:")
                            .or_else(|| p.strip_prefix("SMTP:"))
                            .map(str::to_string)
                    })
                    .collect(),
                Err(Error::Http { status, .. }) if (400..500).contains(&status) => {
                    tracing::debug!(status, "no alias list for this account");
                    Vec::new()
                }
                Err(e) => return Err(e),
            }
        };
        if !matches(&known) && !matches(&aliases) {
            let got = me
                .mail
                .clone()
                .or(me.user_principal_name.clone())
                .or(claims.preferred_username.clone())
                .unwrap_or_else(|| "another account".into());
            return Err(Error::InvalidInput(format!(
                "You signed in as {got}; sign in as {want} instead."
            )));
        }
        let credential = MicrosoftCredential {
            refresh_token,
            client_id: client_id.to_string(),
            scopes,
            tenant_id: claims.tid.clone(),
            object_id: claims.oid.clone(),
            obtained_at: unix_secs(now),
        };
        let id = want.clone();
        self.vault_op(move |v| v.save(&id, &credential)).await?;
        *self.slot(&want).lock().await = Some(tokens.cached(now));
        tracing::info!(account = %want, "Microsoft account signed in");
        Ok(SignedIn {
            email: want,
            display_name: me
                .display_name
                .or(claims.name)
                .filter(|n| !n.trim().is_empty()),
            tenant_id: claims.tid,
        })
    }

    async fn me(&self, access_token: &str, select: &str) -> Result<Me> {
        let url = format!(
            "{}/me?$select={select}",
            self.endpoints.graph.trim_end_matches('/')
        );
        let resp = self
            .transport
            .send(Request {
                method: Method::Get,
                url,
                headers: vec![
                    ("Authorization".into(), format!("Bearer {access_token}")),
                    ("Accept".into(), "application/json".into()),
                ],
                body: None,
            })
            .await?;
        if !resp.is_success() {
            return Err(crate::http::http_error(resp.status, &resp.body));
        }
        resp.json("profile")
    }
}

/// An error as the browser page shows it.
fn user_text(e: &Error) -> String {
    match e {
        Error::OAuth(m) | Error::InvalidInput(m) => m.clone(),
        other => other.to_string(),
    }
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
