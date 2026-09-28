//! OAuth for installed apps. OWNER: auth agent.
//!
//! - Two sign-in paths, both PKCE S256 + `state`, `access_type=offline`,
//!   `prompt=consent` (so we always get a refresh token), `login_hint` optional:
//!   - the system sign-in sheet (ASWebAuthenticationSession on macOS) with
//!     an optional Google OAuth client of the "iOS" type (the only type with
//!     a custom-scheme redirect) and its reversed-client-id redirect
//!     ([`IosClientConfig`], [`WebAuthSheet`]), used when both are available;
//!   - otherwise the system browser with a loopback redirect on
//!     127.0.0.1:<random port> and the "Desktop app" client.
//!
//!   A refresh token only refreshes with the client that minted it, so each
//!   stored token records its client ([`keychain`]).
//! - Scopes: `https://www.googleapis.com/auth/gmail.modify` (read, label,
//!   archive, trash, send — no permanent delete) + `openid email profile`,
//!   plus optional extra scopes per sign-in (calendar at add-account, contacts
//!   and calendar later), always with `include_granted_scopes=true`. Only
//!   Gmail is required; what was granted comes from the token response's
//!   `scope` field and is stored with the credential.
//! - Refresh tokens live ONLY in the macOS Keychain: one item (service
//!   "co.gluska.penguin.google", account "tokens.v2") holding every account,
//!   read at most once per process; see [`keychain`]. Access tokens are
//!   cached in memory. Tokens, codes, verifiers and client secrets are never
//!   logged: every type holding one has a redacting `Debug`.

mod ios_client;
mod keychain;
mod loopback;
mod pkce;
mod sheet;
#[cfg(target_os = "macos")]
mod sheet_apple;
mod token;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{Error, Result};

pub use ios_client::IosClientConfig;
pub use keychain::{KEYCHAIN_SERVICE, VAULT_ACCOUNT};
pub use sheet::{SheetError, SheetFuture, WebAuthSheet};
#[cfg(target_os = "macos")]
pub use sheet_apple::AppleWebAuthSheet;

use keychain::{shared_keychain, CredentialStore, StoredCredential, TokenClient};
use loopback::{Callback, LoopbackServer, PageKind};
use pkce::{constant_time_eq, random_urlsafe, Pkce};
use token::{id_token_claims, token_error, CachedToken, Grant, TokenResponse};

pub const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
/// Optional, requested incrementally via [`AuthManager::sign_in_with_scopes`]:
/// "Other contacts" (people you've emailed), used for contact photos.
pub const CONTACTS_OTHER_READONLY_SCOPE: &str =
    "https://www.googleapis.com/auth/contacts.other.readonly";
/// Optional: the user's saved contacts, used for contact photos.
pub const CONTACTS_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/contacts.readonly";
const SCOPES: &str = "https://www.googleapis.com/auth/gmail.modify openid email profile";
const CLIENT_FILE_NAME: &str = "google-oauth-client.json";
const CLIENT_ENV_VAR: &str = "PENGUIN_GOOGLE_CLIENT_JSON";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MISSING_GMAIL_SCOPE: &str =
    "Penguin needs permission to read, organize, and send your Gmail. \
     Sign in again and leave the Gmail checkbox ticked on Google's consent screen.";

/// Contents of the "Desktop app" OAuth client JSON downloaded from Google
/// Cloud Console (`{"installed": {"client_id": …, "client_secret": …}}`).
/// For desktop clients the secret is not confidential, but we still never
/// commit or log it. Lookup order: env `PENGUIN_GOOGLE_CLIENT_JSON` (path) →
/// `<app config dir>/google-oauth-client.json`.
#[derive(Clone)]
pub struct OAuthClientConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Deserialize)]
struct ClientFile {
    installed: Option<ClientSection>,
    web: Option<ClientSection>,
}

#[derive(Deserialize)]
struct ClientSection {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

impl OAuthClientConfig {
    pub fn from_json(json: &str) -> Result<Self> {
        let file: ClientFile = serde_json::from_str(json).map_err(|e| {
            Error::NotConfigured(format!("OAuth client file is not valid JSON ({e})"))
        })?;
        let section = match (file.installed, file.web) {
            (Some(installed), _) => installed,
            (None, Some(_)) => {
                return Err(Error::NotConfigured(
                    "this is a \"Web application\" OAuth client; Penguin needs a \"Desktop app\" client. \
                     In Google Cloud Console choose Create credentials → OAuth client ID → \
                     Application type: Desktop app, then download its JSON"
                        .into(),
                ))
            }
            (None, None) => {
                return Err(Error::NotConfigured(
                    "OAuth client file has no \"installed\" section; download the JSON of a \
                     \"Desktop app\" OAuth client from Google Cloud Console"
                        .into(),
                ))
            }
        };
        let client_id = section.client_id.trim().to_owned();
        if client_id.is_empty() {
            return Err(Error::NotConfigured(
                "OAuth client file has no client_id".into(),
            ));
        }
        let client_secret = section
            .client_secret
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        Ok(Self {
            client_id,
            client_secret,
        })
    }

    /// `Ok(None)` when no client has been configured yet.
    pub fn load(config_dir: &Path) -> Result<Option<Self>> {
        let env_path = std::env::var_os(CLIENT_ENV_VAR)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self::load_from(env_path.as_deref(), config_dir)
    }

    fn load_from(env_path: Option<&Path>, config_dir: &Path) -> Result<Option<Self>> {
        if let Some(path) = env_path {
            let json = std::fs::read_to_string(path).map_err(|e| {
                Error::NotConfigured(format!(
                    "{CLIENT_ENV_VAR} points to {} which can't be read: {e}",
                    path.display()
                ))
            })?;
            return Self::from_json(&json).map(Some);
        }
        let path = config_dir.join(CLIENT_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(json) => Self::from_json(&json).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::NotConfigured(format!(
                "can't read {}: {e}",
                path.display()
            ))),
        }
    }

    /// Validate and persist a client JSON into `<config_dir>/google-oauth-client.json` (0600).
    pub fn save(config_dir: &Path, json: &str) -> Result<Self> {
        let config = Self::from_json(json)?;
        let io_err = |e: std::io::Error| Error::Other(format!("can't save OAuth client file: {e}"));
        std::fs::create_dir_all(config_dir).map_err(io_err)?;
        let path = config_dir.join(CLIENT_FILE_NAME);
        let tmp = config_dir.join(format!(".{CLIENT_FILE_NAME}.tmp"));
        write_private(&tmp, json.trim().as_bytes()).map_err(io_err)?;
        std::fs::rename(&tmp, &path).map_err(io_err)?;
        Ok(config)
    }
}

impl fmt::Debug for OAuthClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthClientConfig")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Write a file readable only by the owner. The mode is set at creation and
/// again explicitly, in case the file already existed with looser bits.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        let mut file = options.open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()
    }
    #[cfg(not(unix))]
    {
        let mut file = options.open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }
}

/// Identity returned after a successful sign-in.
#[derive(Debug, Clone)]
pub struct SignedInAccount {
    /// Lowercased; also the Keychain account name.
    pub email: String,
    pub display_name: Option<String>,
}

/// Google endpoints; overridable so tests can point at a local server.
#[derive(Clone)]
struct Endpoints {
    auth: String,
    token: String,
    revoke: String,
    userinfo: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
            userinfo: "https://openidconnect.googleapis.com/v1/userinfo".into(),
        }
    }
}

type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;
type OpenUrl = Arc<dyn Fn(&str) + Send + Sync>;
type Hook = Arc<dyn Fn() + Send + Sync>;

/// How the app shows interactive sign-in; see [`AuthManager::sign_in_interactive`].
#[derive(Clone)]
pub struct SignInUi {
    open_browser: OpenUrl,
    sheet: Option<Arc<dyn WebAuthSheet>>,
    on_browser_success: Option<Hook>,
}

impl SignInUi {
    /// `open_browser` opens a URL in the system browser (loopback fallback).
    pub fn new(open_browser: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            open_browser: Arc::new(open_browser),
            sheet: None,
            on_browser_success: None,
        }
    }

    /// The system sign-in sheet, used when an iOS client is configured.
    pub fn with_sheet(mut self, sheet: Arc<dyn WebAuthSheet>) -> Self {
        self.sheet = Some(sheet);
        self
    }

    /// Called after a successful browser sign-in, once the tab has its page:
    /// the app brings its window back to the front.
    pub fn on_browser_success(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_browser_success = Some(Arc::new(hook));
        self
    }
}

/// The client a sign-in exchanges its code with.
struct SignInClient<'a> {
    id: &'a str,
    secret: Option<&'a str>,
    redirect_uri: &'a str,
    token_client: TokenClient,
}
type TokenSlot = Arc<Mutex<Option<CachedToken>>>;

/// Owns the client config, the Keychain, and an in-memory access-token cache.
/// `Clone + Send + Sync`; clones share the cache.
#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<Inner>,
}

struct Inner {
    config: OAuthClientConfig,
    http: reqwest::Client,
    endpoints: Endpoints,
    store: Arc<dyn CredentialStore>,
    clock: Clock,
    /// Optional iOS client for the system sign-in sheet.
    ios: StdRwLock<Option<IosClientConfig>>,
    /// One slot per account. Holding a slot's lock while refreshing makes
    /// refresh single-flight: concurrent callers wait, then reuse the result.
    tokens: StdMutex<HashMap<String, TokenSlot>>,
}

impl AuthManager {
    pub fn new(config: OAuthClientConfig) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client with rustls builds");
        Self::with_parts(
            config,
            http,
            Endpoints::default(),
            shared_keychain(),
            Arc::new(SystemTime::now),
        )
    }

    fn with_parts(
        config: OAuthClientConfig,
        http: reqwest::Client,
        endpoints: Endpoints,
        store: Arc<dyn CredentialStore>,
        clock: Clock,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                http,
                endpoints,
                store,
                clock,
                ios: StdRwLock::new(None),
                tokens: StdMutex::default(),
            }),
        }
    }

    /// Run the full browser sign-in flow. `open_url` opens the system browser.
    /// Stores the refresh token in the Keychain and returns the account identity.
    pub async fn sign_in(
        &self,
        open_url: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<SignedInAccount> {
        self.sign_in_with_hint(None, open_url).await
    }

    /// [`sign_in`](Self::sign_in) with Google's `login_hint`, which preselects
    /// the account (useful when re-authenticating after `NeedsReauth`).
    pub async fn sign_in_with_hint(
        &self,
        login_hint: Option<&str>,
        open_url: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<SignedInAccount> {
        self.sign_in_with_scopes(login_hint, &[], open_url).await
    }

    /// Browser sign-in that also requests `extra_scopes` (incremental
    /// authorization, e.g. [`CONTACTS_OTHER_READONLY_SCOPE`]). With
    /// `include_granted_scopes` the new refresh token covers Gmail plus
    /// whatever else the user allows. Gmail is still required; extra scopes are
    /// optional, since the user can untick them, so check
    /// [`granted_scopes`](Self::granted_scopes) afterwards.
    pub async fn sign_in_with_scopes(
        &self,
        login_hint: Option<&str>,
        extra_scopes: &[&str],
        open_url: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<SignedInAccount> {
        self.sign_in_loopback(login_hint, extra_scopes, &open_url, None)
            .await
    }

    /// Interactive sign-in for the app: the system sign-in sheet when `ui`
    /// has one and an iOS client is configured, otherwise the browser.
    /// Signing in through the sheet moves the account's token to the iOS
    /// client. Closing the sheet or declining on Google's screen returns
    /// [`Error::SignInCancelled`].
    pub async fn sign_in_interactive(
        &self,
        login_hint: Option<&str>,
        extra_scopes: &[&str],
        ui: &SignInUi,
    ) -> Result<SignedInAccount> {
        match (&ui.sheet, self.ios_client()) {
            (Some(sheet), Some(ios)) => {
                self.sign_in_sheet(login_hint, extra_scopes, sheet.as_ref(), &ios)
                    .await
            }
            _ => {
                self.sign_in_loopback(
                    login_hint,
                    extra_scopes,
                    ui.open_browser.as_ref(),
                    ui.on_browser_success.as_deref(),
                )
                .await
            }
        }
    }

    /// Configure (or clear) the iOS client used by the sign-in sheet.
    pub fn set_ios_client(&self, client: Option<IosClientConfig>) {
        if client
            .as_ref()
            .is_some_and(|c| c.client_id == self.inner.config.client_id)
        {
            tracing::warn!(
                "the iOS client id is the desktop client's id; the sign-in sheet will fail"
            );
        }
        *self.inner.ios.write().unwrap_or_else(|p| p.into_inner()) = client;
    }

    pub fn ios_client(&self) -> Option<IosClientConfig> {
        self.inner
            .ios
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    async fn sign_in_sheet(
        &self,
        login_hint: Option<&str>,
        extra_scopes: &[&str],
        sheet: &dyn WebAuthSheet,
        ios: &IosClientConfig,
    ) -> Result<SignedInAccount> {
        let redirect_uri = ios.redirect_uri();
        let scheme = ios.callback_scheme();
        let pkce = Pkce::generate()?;
        let state = random_urlsafe(24)?;
        let url = authorization_url(
            &self.inner.endpoints.auth,
            &ios.client_id,
            &redirect_uri,
            &pkce.challenge,
            &state,
            login_hint,
            extra_scopes,
        )?;

        tracing::info!("opening the system sign-in sheet");
        let redirect = sheet.authenticate(url.into(), scheme.clone()).await?;
        let callback = sheet::parse_custom_redirect(&redirect, &scheme)?;
        if !constant_time_eq(callback.state(), &state) {
            // The sheet is one-shot, so unlike loopback there's no "keep waiting".
            return Err(Error::OAuth(
                "the sign-in response didn't match this request; try again".into(),
            ));
        }
        let code = callback_code(callback)?;
        let client = SignInClient {
            id: &ios.client_id,
            secret: None,
            redirect_uri: &redirect_uri,
            token_client: TokenClient::Ios {
                client_id: ios.client_id.clone(),
            },
        };
        self.complete_sign_in(&code, &pkce.verifier, &client).await
    }

    async fn sign_in_loopback(
        &self,
        login_hint: Option<&str>,
        extra_scopes: &[&str],
        open_url: &(dyn Fn(&str) + Send + Sync),
        on_success: Option<&(dyn Fn() + Send + Sync)>,
    ) -> Result<SignedInAccount> {
        let mut server = LoopbackServer::bind().await?;
        let redirect_uri = server.redirect_uri();
        let pkce = Pkce::generate()?;
        let state = random_urlsafe(24)?;
        let url = authorization_url(
            &self.inner.endpoints.auth,
            &self.inner.config.client_id,
            &redirect_uri,
            &pkce.challenge,
            &state,
            login_hint,
            extra_scopes,
        )?;

        tracing::info!(port = server.port(), "opening browser for Google sign-in");
        open_url(url.as_str());

        let wait_for_code = async {
            loop {
                let Some((callback, stream)) = server.next_callback().await else {
                    return Err(Error::OAuth("sign-in listener stopped unexpectedly".into()));
                };
                if !constant_time_eq(callback.state(), &state) {
                    // A stale tab or a request we didn't initiate: refuse it
                    // but keep waiting for the real redirect.
                    tracing::warn!("ignoring OAuth callback with mismatched state");
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
        let (callback, stream) = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code)
            .await
            .map_err(|_| Error::OAuth("sign-in timed out after 5 minutes; try again".into()))??;

        let client = SignInClient {
            id: &self.inner.config.client_id,
            secret: self.inner.config.client_secret.as_deref(),
            redirect_uri: &redirect_uri,
            token_client: TokenClient::Desktop {
                client_id: Some(self.inner.config.client_id.clone()),
            },
        };
        let result = match callback_code(callback) {
            Ok(code) => self.complete_sign_in(&code, &pkce.verifier, &client).await,
            Err(e) => Err(e),
        };

        // Answer the browser only now, so the tab reflects the real outcome.
        let body = match &result {
            Ok(account) => loopback::page(
                PageKind::Success,
                "Signed in, returning to Penguin…",
                &format!(
                    "{} is connected. If this tab stays open, you can close it.",
                    account.email
                ),
            ),
            Err(Error::SignInCancelled) => loopback::page(
                PageKind::Error,
                "Sign-in cancelled",
                "Nothing was connected. You can close this tab.",
            ),
            Err(Error::OAuth(msg)) if msg == MISSING_GMAIL_SCOPE => loopback::page(
                PageKind::Error,
                "Gmail access wasn't granted",
                MISSING_GMAIL_SCOPE,
            ),
            Err(e) => loopback::page(
                PageKind::Error,
                "Sign-in didn't finish",
                &format!("{e}. Return to Penguin to try again."),
            ),
        };
        let status = if result.is_ok() { 200 } else { 400 };
        loopback::respond(stream, status, &body).await;
        if let (Ok(_), Some(on_success)) = (&result, on_success) {
            on_success();
        }
        result
    }

    async fn complete_sign_in(
        &self,
        code: &str,
        verifier: &str,
        client: &SignInClient<'_>,
    ) -> Result<SignedInAccount> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", client.redirect_uri),
            ("client_id", client.id),
        ];
        if let Some(secret) = client.secret {
            form.push(("client_secret", secret));
        }
        let tokens = self.post_token(&form, Grant::AuthorizationCode).await?;
        let now = (self.inner.clock)();

        let scopes = tokens.granted_scopes();
        if !scopes.iter().any(|s| s == GMAIL_MODIFY_SCOPE) {
            // Don't leave a half-useful grant behind on the user's account.
            self.revoke_best_effort(
                tokens
                    .refresh_token
                    .as_deref()
                    .unwrap_or(&tokens.access_token),
            )
            .await;
            return Err(Error::OAuth(MISSING_GMAIL_SCOPE.into()));
        }
        let Some(refresh_token) = tokens.refresh_token.clone() else {
            return Err(Error::OAuth(
                "Google did not return a refresh token; try signing in again".into(),
            ));
        };

        let claims = tokens
            .id_token
            .as_deref()
            .and_then(id_token_claims)
            .unwrap_or_default();
        let (email, display_name) = match claims.email {
            Some(email) => (email, claims.name),
            None => self.userinfo(&tokens.access_token).await?,
        };
        let email = normalize_email(&email);
        if email.is_empty() {
            return Err(Error::OAuth(
                "Google did not report the account's email address".into(),
            ));
        }

        // Replaces any earlier token for this account, including one from the
        // other client: re-signing in through the sheet moves the account.
        let credential = StoredCredential::new(refresh_token, scopes, unix_secs(now))
            .with_client(client.token_client.clone());
        let key = email.clone();
        self.with_store(move |store| store.save(&key, &credential))
            .await?;
        *self.slot(&email).lock().await = Some(tokens.cached(now));

        tracing::info!(account = %email, client = client_kind(&client.token_client), "signed in");
        Ok(SignedInAccount {
            email,
            display_name,
        })
    }

    /// A valid access token for the account, refreshing if within 60s of expiry.
    /// Returns `Error::NeedsReauth` on `invalid_grant`.
    pub async fn access_token(&self, email: &str) -> Result<String> {
        let email = normalize_email(email);
        let slot = self.slot(&email);
        let mut cached = slot.lock().await;
        let now = (self.inner.clock)();
        if let Some(token) = cached.as_ref().filter(|t| t.is_fresh(now)) {
            return Ok(token.access_token.clone());
        }

        let key = email.clone();
        let credential = self
            .with_store(move |store| store.load(&key))
            .await?
            .ok_or_else(|| Error::NeedsReauth(email.clone()))?;

        match self.refresh(&email, &credential).await {
            Ok(tokens) => {
                let mut updated = credential.clone();
                if let Some(rotated) = &tokens.refresh_token {
                    updated.refresh_token = rotated.clone();
                }
                // Keep granted_scopes() true to Google, e.g. after the user
                // revokes an optional scope from their Google Account.
                let scopes = tokens.granted_scopes();
                if !scopes.is_empty() && !same_scopes(&scopes, &updated.scopes) {
                    updated.scopes = scopes;
                }
                if updated != credential {
                    let key = email.clone();
                    self.with_store(move |store| store.save(&key, &updated))
                        .await?;
                }
                let token = tokens.cached(now);
                let access = token.access_token.clone();
                *cached = Some(token);
                tracing::debug!(account = %email, "access token refreshed");
                Ok(access)
            }
            // A network blip inside the refresh margin shouldn't fail callers
            // while the current token is still valid.
            Err(Error::Network(e)) => match cached.as_ref().filter(|t| t.is_unexpired(now)) {
                Some(token) => {
                    tracing::warn!(account = %email, error = %e, "token refresh failed; using unexpired token");
                    Ok(token.access_token.clone())
                }
                None => Err(Error::Network(e)),
            },
            Err(e) => {
                if matches!(e, Error::NeedsReauth(_)) {
                    *cached = None;
                    tracing::warn!(account = %email, "refresh token rejected; account needs to sign in again");
                }
                Err(e)
            }
        }
    }

    /// Refresh with the client that minted the token: iOS tokens need only
    /// their stored client id (no secret); desktop tokens use the configured
    /// desktop client.
    async fn refresh(&self, email: &str, credential: &StoredCredential) -> Result<TokenResponse> {
        let (client_id, secret) = match &credential.client {
            TokenClient::Ios { client_id } => (client_id.as_str(), None),
            TokenClient::Desktop { client_id } => {
                let config = &self.inner.config;
                if client_id
                    .as_deref()
                    .is_some_and(|id| id != config.client_id)
                {
                    tracing::warn!(account = %email, "token was issued to a different desktop client; refresh will likely fail");
                }
                (config.client_id.as_str(), config.client_secret.as_deref())
            }
        };
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", credential.refresh_token.as_str()),
            ("client_id", client_id),
        ];
        if let Some(secret) = secret {
            form.push(("client_secret", secret));
        }
        self.post_token(&form, Grant::Refresh { email }).await
    }

    /// Drop the cached access token so the next [`access_token`](Self::access_token)
    /// refreshes. Call when Google rejects a token the cache still considers
    /// fresh (HTTP 401): the refresh then either succeeds or surfaces
    /// `NeedsReauth`. Waits for an in-flight refresh rather than racing it.
    pub async fn invalidate_access_token(&self, email: &str) {
        let email = normalize_email(email);
        *self.slot(&email).lock().await = None;
        tracing::debug!(account = %email, "cached access token invalidated");
    }

    /// Revoke with Google (best effort) and delete from the Keychain.
    pub async fn sign_out(&self, email: &str) -> Result<()> {
        let email = normalize_email(email);
        let slot = self.slot(&email);
        // Hold the slot so a concurrent refresh can't repopulate the cache.
        let mut cached = slot.lock().await;
        let key = email.clone();
        let credential = self.with_store(move |store| store.load(&key)).await;
        match (&credential, cached.as_ref()) {
            (Ok(Some(cred)), _) => self.revoke_best_effort(&cred.refresh_token).await,
            (_, Some(token)) => self.revoke_best_effort(&token.access_token).await,
            _ => {}
        }
        *cached = None;
        let key = email.clone();
        self.with_store(move |store| store.delete(&key)).await?;
        drop(cached);
        self.inner
            .tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&email);
        tracing::info!(account = %email, "signed out");
        Ok(())
    }

    /// Whether a refresh token is stored. Served from the process-wide cache;
    /// only the first credential lookup in a process touches the Keychain
    /// (and may block on its access prompt).
    pub fn has_credentials(&self, email: &str) -> bool {
        let email = normalize_email(email);
        match self.inner.store.load(&email) {
            Ok(found) => found.is_some(),
            Err(e) => {
                tracing::warn!(account = %email, error = %e, "keychain lookup failed");
                false
            }
        }
    }

    /// Scopes the account's refresh token was granted (from sign-in, kept
    /// current by refreshes). Served from the process-wide cache; empty when
    /// the account has no stored credential.
    pub fn granted_scopes(&self, email: &str) -> Vec<String> {
        let email = normalize_email(email);
        match self.inner.store.load(&email) {
            Ok(found) => found.map(|c| c.scopes).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(account = %email, error = %e, "keychain lookup failed");
                Vec::new()
            }
        }
    }

    fn slot(&self, email: &str) -> TokenSlot {
        let mut tokens = self.inner.tokens.lock().unwrap_or_else(|p| p.into_inner());
        tokens.entry(email.to_owned()).or_default().clone()
    }

    /// Run a blocking credential-store call off the async runtime.
    async fn with_store<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn CredentialStore) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.inner.store.clone();
        tokio::task::spawn_blocking(move || f(store.as_ref()))
            .await
            .map_err(|e| Error::Keychain(format!("keychain task failed: {e}")))?
    }

    async fn post_token(&self, form: &[(&str, &str)], grant: Grant<'_>) -> Result<TokenResponse> {
        let resp = self
            .inner
            .http
            .post(&self.inner.endpoints.token)
            .form(form)
            .send()
            .await
            .map_err(network)?;
        let status = resp.status();
        let body = resp.text().await.map_err(network)?;
        if !status.is_success() {
            return Err(token_error(status.as_u16(), &body, grant));
        }
        serde_json::from_str(&body)
            .map_err(|_| Error::OAuth("unexpected response from Google's token endpoint".into()))
    }

    async fn userinfo(&self, access_token: &str) -> Result<(String, Option<String>)> {
        #[derive(Deserialize)]
        struct UserInfo {
            email: Option<String>,
            name: Option<String>,
        }
        let resp = self
            .inner
            .http
            .get(&self.inner.endpoints.userinfo)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(network)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                body: "userinfo request failed".into(),
            });
        }
        let info: UserInfo = resp.json().await.map_err(network)?;
        let email = info.email.ok_or_else(|| {
            Error::OAuth("Google did not report the account's email address".into())
        })?;
        Ok((email, info.name))
    }

    async fn revoke_best_effort(&self, token: &str) {
        let result = self
            .inner
            .http
            .post(&self.inner.endpoints.revoke)
            .form(&[("token", token)])
            .send()
            .await;
        match result {
            Ok(resp) if resp.status().is_success() => {}
            // 400 invalid_token: already revoked or expired, which is the goal.
            Ok(resp) => tracing::info!(
                status = resp.status().as_u16(),
                "token revocation not confirmed"
            ),
            Err(e) => tracing::warn!(error = %e.without_url(), "token revocation failed"),
        }
    }
}

fn authorization_url(
    auth_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
    login_hint: Option<&str>,
    extra_scopes: &[&str],
) -> Result<url::Url> {
    let mut scope = SCOPES.to_owned();
    for extra in extra_scopes.iter().map(|s| s.trim()) {
        if !extra.is_empty() && !scope.split(' ').any(|s| s == extra) {
            scope.push(' ');
            scope.push_str(extra);
        }
    }
    let mut url = url::Url::parse(auth_endpoint)
        .map_err(|e| Error::OAuth(format!("bad authorization endpoint: {e}")))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &scope)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent")
            .append_pair("include_granted_scopes", "true");
        if let Some(hint) = login_hint.map(str::trim).filter(|h| !h.is_empty()) {
            q.append_pair("login_hint", hint);
        }
    }
    Ok(url)
}

/// The code from a redirect, or why there isn't one.
fn callback_code(callback: Callback) -> Result<String> {
    match callback {
        Callback::Code { code, .. } => Ok(code),
        Callback::Denied { error, .. } => {
            tracing::info!(%error, "Google sign-in was not completed");
            if error == "access_denied" {
                Err(Error::SignInCancelled)
            } else {
                Err(Error::OAuth(format!("Google returned an error: {error}")))
            }
        }
    }
}

fn client_kind(client: &TokenClient) -> &'static str {
    match client {
        TokenClient::Desktop { .. } => "desktop",
        TokenClient::Ios { .. } => "ios",
    }
}

fn same_scopes(a: &[String], b: &[String]) -> bool {
    let set = |v: &[String]| v.iter().cloned().collect::<std::collections::BTreeSet<_>>();
    set(a) == set(b)
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn network(e: reqwest::Error) -> Error {
    Error::Network(e.without_url().to_string())
}
