//! AuthManager tests against a local fake of Google's OAuth endpoints and an
//! in-memory credential store. No real network, no Keychain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::keychain::memory::MemoryStore;
use super::keychain::{CredentialStore, StoredCredential, TokenClient};
use super::pkce::challenge_s256;
use super::*;

type Form = HashMap<String, String>;
type Handler = Arc<dyn Fn(&str, &Form) -> (u16, String) + Send + Sync>;

/// Fake Google: one request per connection, form bodies, canned responses.
struct FakeGoogle {
    base: String,
    calls: Arc<StdMutex<Vec<(String, Form)>>>,
    task: JoinHandle<()>,
}

impl FakeGoogle {
    async fn start(handler: Handler) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls: Arc<StdMutex<Vec<(String, Form)>>> = Arc::default();
        let log = calls.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let (path, form) = read_request(&mut stream).await;
                    log.lock().unwrap().push((path.clone(), form.clone()));
                    // Slow enough that concurrent callers overlap.
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    let (status, body) = handler(&path, &form);
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(resp.as_bytes()).await.unwrap();
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { base, calls, task }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            auth: format!("{}/auth", self.base),
            token: format!("{}/token", self.base),
            revoke: format!("{}/revoke", self.base),
            userinfo: format!("{}/userinfo", self.base),
        }
    }

    fn calls_to(&self, path: &str) -> Vec<Form> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, f)| f.clone())
            .collect()
    }
}

impl Drop for FakeGoogle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> (String, Form) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "client closed before sending headers");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let content_length = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    while buf.len() < head_end + content_length {
        let n = stream.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
    }
    let path = head
        .split_whitespace()
        .nth(1)
        .unwrap()
        .split('?')
        .next()
        .unwrap()
        .to_owned();
    let form = url::form_urlencoded::parse(&buf[head_end..head_end + content_length])
        .into_owned()
        .collect();
    (path, form)
}

struct TestClock(Arc<AtomicU64>);

impl TestClock {
    fn new(secs: u64) -> Self {
        Self(Arc::new(AtomicU64::new(secs)))
    }
    fn clock(&self) -> Clock {
        let secs = self.0.clone();
        Arc::new(move || SystemTime::UNIX_EPOCH + Duration::from_secs(secs.load(Ordering::SeqCst)))
    }
    fn advance(&self, secs: u64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
}

fn config() -> OAuthClientConfig {
    OAuthClientConfig {
        client_id: "cid.apps.googleusercontent.com".into(),
        client_secret: Some("csecret".into()),
    }
}

fn test_http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn manager(google: &FakeGoogle, store: Arc<MemoryStore>, clock: &TestClock) -> AuthManager {
    AuthManager::with_parts(
        config(),
        test_http(),
        google.endpoints(),
        store,
        clock.clock(),
    )
}

fn store_with(email: &str, refresh_token: &str) -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::default());
    store
        .save(
            email,
            &StoredCredential::new(refresh_token.into(), vec![GMAIL_MODIFY_SCOPE.into()], 1),
        )
        .unwrap();
    store
}

fn id_token(email: &str, name: &str) -> String {
    let payload = serde_json::json!({ "email": email, "email_verified": true, "name": name });
    format!(
        "eyJhbGciOiJSUzI1NiJ9.{}.sig",
        URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

// ---- client JSON ------------------------------------------------------------

#[test]
fn parses_desktop_client_json() {
    let json = r#"{"installed":{"client_id":"123-abc.apps.googleusercontent.com","project_id":"p",
        "auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token",
        "client_secret":"GOCSPX-x","redirect_uris":["http://localhost"]}}"#;
    let cfg = OAuthClientConfig::from_json(json).unwrap();
    assert_eq!(cfg.client_id, "123-abc.apps.googleusercontent.com");
    assert_eq!(cfg.client_secret.as_deref(), Some("GOCSPX-x"));
    assert!(!format!("{cfg:?}").contains("GOCSPX"));

    let no_secret =
        OAuthClientConfig::from_json(r#"{"installed":{"client_id":"x","client_secret":""}}"#)
            .unwrap();
    assert_eq!(no_secret.client_secret, None);
}

#[test]
fn rejects_web_and_malformed_client_json() {
    let err = OAuthClientConfig::from_json(r#"{"web":{"client_id":"x","client_secret":"y"}}"#)
        .unwrap_err();
    assert!(
        matches!(&err, Error::NotConfigured(m) if m.contains("Desktop app")),
        "{err}"
    );
    assert!(matches!(
        OAuthClientConfig::from_json("not json"),
        Err(Error::NotConfigured(_))
    ));
    assert!(matches!(
        OAuthClientConfig::from_json("{}"),
        Err(Error::NotConfigured(_))
    ));
    assert!(matches!(
        OAuthClientConfig::from_json(r#"{"installed":{"client_id":"  "}}"#),
        Err(Error::NotConfigured(_))
    ));
}

#[test]
fn save_and_load_client_json() {
    let dir =
        std::env::temp_dir().join(format!("penguin-auth-test-{}", random_urlsafe(8).unwrap()));
    assert!(OAuthClientConfig::load_from(None, &dir).unwrap().is_none());

    let json = r#"{"installed":{"client_id":"cid","client_secret":"sec"}}"#;
    OAuthClientConfig::save(&dir, json).unwrap();
    let path = dir.join(CLIENT_FILE_NAME);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let loaded = OAuthClientConfig::load_from(None, &dir).unwrap().unwrap();
    assert_eq!(loaded.client_id, "cid");

    // Invalid input never replaces a good file.
    assert!(OAuthClientConfig::save(&dir, r#"{"web":{"client_id":"w"}}"#).is_err());
    assert_eq!(
        OAuthClientConfig::load_from(None, &dir)
            .unwrap()
            .unwrap()
            .client_id,
        "cid"
    );

    // The env path wins over the config dir, and a bad env path is an error.
    let other = dir.join("other.json");
    std::fs::write(&other, r#"{"installed":{"client_id":"from-env"}}"#).unwrap();
    assert_eq!(
        OAuthClientConfig::load_from(Some(&other), &dir)
            .unwrap()
            .unwrap()
            .client_id,
        "from-env"
    );
    assert!(OAuthClientConfig::load_from(Some(&dir.join("missing.json")), &dir).is_err());

    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- authorization URL ------------------------------------------------------

#[test]
fn builds_authorization_url() {
    let url = authorization_url(
        "https://accounts.google.com/o/oauth2/v2/auth",
        "cid.apps.googleusercontent.com",
        "http://127.0.0.1:5123/callback",
        "CHALLENGE",
        "STATE",
        Some("ada@example.com"),
        &[],
    )
    .unwrap();
    assert_eq!(url.host_str(), Some("accounts.google.com"));
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], "cid.apps.googleusercontent.com");
    assert_eq!(q["redirect_uri"], "http://127.0.0.1:5123/callback");
    assert_eq!(q["response_type"], "code");
    assert_eq!(
        q["scope"],
        "https://www.googleapis.com/auth/gmail.modify openid email profile"
    );
    assert_eq!(q["code_challenge"], "CHALLENGE");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["state"], "STATE");
    assert_eq!(q["access_type"], "offline");
    assert_eq!(q["prompt"], "consent");
    assert_eq!(q["include_granted_scopes"], "true");
    assert_eq!(q["login_hint"], "ada@example.com");

    let url = authorization_url(
        "https://a.example/auth",
        "c",
        "r",
        "ch",
        "s",
        Some(" "),
        &[],
    )
    .unwrap();
    assert!(!url.query_pairs().any(|(k, _)| k == "login_hint"));
}

// ---- access tokens ----------------------------------------------------------

#[tokio::test]
async fn refresh_is_single_flight_and_respects_expiry() {
    let n = Arc::new(AtomicUsize::new(0));
    let counter = n.clone();
    let google = FakeGoogle::start(Arc::new(move |path, form| {
        assert_eq!(path, "/token");
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(form["refresh_token"], "rt-1");
        assert_eq!(form["client_secret"], "csecret");
        let i = counter.fetch_add(1, Ordering::SeqCst) + 1;
        (
            200,
            format!(r#"{{"access_token":"at-{i}","expires_in":3600,"token_type":"Bearer"}}"#),
        )
    }))
    .await;
    let clock = TestClock::new(1_000_000);
    let auth = manager(&google, store_with("ada@example.com", "rt-1"), &clock);

    let calls: Vec<_> = (0..10)
        .map(|_| {
            let auth = auth.clone();
            tokio::spawn(async move { auth.access_token("Ada@Example.com").await })
        })
        .collect();
    for call in calls {
        assert_eq!(call.await.unwrap().unwrap(), "at-1");
    }
    assert_eq!(n.load(Ordering::SeqCst), 1);

    clock.advance(3600 - 61);
    assert_eq!(auth.access_token("ada@example.com").await.unwrap(), "at-1");
    assert_eq!(n.load(Ordering::SeqCst), 1);

    clock.advance(2); // now inside the 60s margin
    assert_eq!(auth.access_token("ada@example.com").await.unwrap(), "at-2");
    assert_eq!(n.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn invalid_grant_needs_reauth() {
    let google = FakeGoogle::start(Arc::new(|_, _| {
        (
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#
                .into(),
        )
    }))
    .await;
    let clock = TestClock::new(1_000_000);
    let auth = manager(&google, store_with("ada@example.com", "rt-dead"), &clock);
    match auth.access_token("ada@example.com").await {
        Err(Error::NeedsReauth(email)) => assert_eq!(email, "ada@example.com"),
        other => panic!("expected NeedsReauth, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_credential_needs_reauth() {
    let google = FakeGoogle::start(Arc::new(|_, _| panic!("no network expected"))).await;
    let clock = TestClock::new(1_000_000);
    let auth = manager(&google, Arc::new(MemoryStore::default()), &clock);
    assert!(!auth.has_credentials("nobody@example.com"));
    assert!(matches!(
        auth.access_token("nobody@example.com").await,
        Err(Error::NeedsReauth(_))
    ));
}

#[tokio::test]
async fn rotated_refresh_token_is_persisted() {
    let google = FakeGoogle::start(Arc::new(|_, _| {
        (
            200,
            r#"{"access_token":"at","expires_in":3600,"refresh_token":"rt-2"}"#.into(),
        )
    }))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = store_with("ada@example.com", "rt-1");
    let auth = manager(&google, store.clone(), &clock);
    auth.access_token("ada@example.com").await.unwrap();
    assert_eq!(
        store
            .load("ada@example.com")
            .unwrap()
            .unwrap()
            .refresh_token,
        "rt-2"
    );
}

// ---- sign-out ----------------------------------------------------------------

#[tokio::test]
async fn sign_out_revokes_and_forgets() {
    let google = FakeGoogle::start(Arc::new(|path, _| match path {
        "/token" => (200, r#"{"access_token":"at","expires_in":3600}"#.into()),
        "/revoke" => (200, "{}".into()),
        other => panic!("unexpected {other}"),
    }))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = store_with("ada@example.com", "rt-1");
    let auth = manager(&google, store.clone(), &clock);
    auth.access_token("ada@example.com").await.unwrap();
    assert!(auth.has_credentials("ADA@example.com"));

    auth.sign_out("Ada@Example.com").await.unwrap();
    let revokes = google.calls_to("/revoke");
    assert_eq!(revokes.len(), 1);
    assert_eq!(revokes[0]["token"], "rt-1");
    assert!(!auth.has_credentials("ada@example.com"));
    assert!(store.load("ada@example.com").unwrap().is_none());
    // The cached access token is gone too.
    assert!(matches!(
        auth.access_token("ada@example.com").await,
        Err(Error::NeedsReauth(_))
    ));
}

#[tokio::test]
async fn sign_out_succeeds_when_revoke_fails() {
    let google = FakeGoogle::start(Arc::new(|_, _| (503, r#"{"error":"backend"}"#.into()))).await;
    let clock = TestClock::new(1_000_000);
    let store = store_with("ada@example.com", "rt-1");
    let auth = manager(&google, store.clone(), &clock);
    auth.sign_out("ada@example.com").await.unwrap();
    assert!(store.load("ada@example.com").unwrap().is_none());
}

// ---- full sign-in -------------------------------------------------------------

/// The fake browser's in-flight request, set from inside `open_url`.
type BrowserTask<T> = Arc<StdMutex<Option<JoinHandle<T>>>>;

async fn browser_done<T>(task: &BrowserTask<T>) -> T {
    let handle = task.lock().unwrap().take().expect("browser was opened");
    handle.await.unwrap()
}

/// Plays the browser: optionally hits the redirect with a wrong `state`
/// first, then with the real one. Returns the final page it was shown.
fn fake_browser(
    challenge_seen: Arc<StdMutex<String>>,
    send_stale_first: bool,
) -> (impl Fn(&str) + Send + Sync + 'static, BrowserTask<String>) {
    let handle: BrowserTask<String> = Arc::default();
    let slot = handle.clone();
    let open = move |auth_url: &str| {
        let url = url::Url::parse(auth_url).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        *challenge_seen.lock().unwrap() = q["code_challenge"].clone();
        let redirect = q["redirect_uri"].clone();
        let state = q["state"].clone();
        assert!(redirect.starts_with("http://127.0.0.1:"));
        *slot.lock().unwrap() = Some(tokio::spawn(async move {
            let http = test_http();
            if send_stale_first {
                let stale = http
                    .get(format!("{redirect}?code=bad&state=wrong"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(stale.status().as_u16(), 400);
            }
            let resp = http
                .get(format!("{redirect}?code=the-code&state={state}"))
                .send()
                .await
                .unwrap();
            resp.text().await.unwrap()
        }));
    };
    (open, handle)
}

fn sign_in_handler(challenge_seen: Arc<StdMutex<String>>, scope: &'static str) -> Handler {
    Arc::new(move |path, form| match path {
        "/token" => {
            assert_eq!(form["grant_type"], "authorization_code");
            assert_eq!(form["code"], "the-code");
            assert_eq!(form["client_id"], "cid.apps.googleusercontent.com");
            assert!(form["redirect_uri"].starts_with("http://127.0.0.1:"));
            assert_eq!(
                challenge_s256(&form["code_verifier"]),
                *challenge_seen.lock().unwrap()
            );
            let body = serde_json::json!({
                "access_token": "at-signin",
                "expires_in": 3599,
                "refresh_token": "rt-signin",
                "scope": scope,
                "token_type": "Bearer",
                "id_token": id_token("Ada@Example.com", "Ada Lovelace"),
            });
            (200, body.to_string())
        }
        "/revoke" => (200, "{}".into()),
        other => panic!("unexpected {other}"),
    })
}

#[tokio::test]
async fn sign_in_end_to_end() {
    let challenge: Arc<StdMutex<String>> = Arc::default();
    let google = FakeGoogle::start(sign_in_handler(
        challenge.clone(),
        "openid https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/userinfo.email",
    ))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    let auth = manager(&google, store.clone(), &clock);

    let (open, browser) = fake_browser(challenge, true);
    let account = auth.sign_in(open).await.unwrap();
    assert_eq!(account.email, "ada@example.com");
    assert_eq!(account.display_name.as_deref(), Some("Ada Lovelace"));

    let page = browser_done(&browser).await;
    assert!(page.contains("ada@example.com is connected"));

    let saved = store.load("ada@example.com").unwrap().unwrap();
    assert_eq!(saved.refresh_token, "rt-signin");
    assert!(saved.scopes.iter().any(|s| s == GMAIL_MODIFY_SCOPE));
    assert_eq!(saved.obtained_at, 1_000_000);

    // The sign-in access token is cached: no refresh call.
    assert_eq!(
        auth.access_token("ada@example.com").await.unwrap(),
        "at-signin"
    );
    assert_eq!(google.calls_to("/token").len(), 1);
}

#[tokio::test]
async fn sign_in_without_gmail_scope_is_rejected() {
    let challenge: Arc<StdMutex<String>> = Arc::default();
    let google =
        FakeGoogle::start(sign_in_handler(challenge.clone(), "openid email profile")).await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    let auth = manager(&google, store.clone(), &clock);

    let (open, browser) = fake_browser(challenge, false);
    let err = auth.sign_in(open).await.unwrap_err();
    assert!(
        matches!(&err, Error::OAuth(m) if m.contains("Gmail")),
        "{err}"
    );

    let page = browser_done(&browser).await;
    assert!(page.contains("Gmail access wasn&#39;t granted"));
    assert!(store.0.lock().unwrap().is_empty());
    assert_eq!(google.calls_to("/revoke")[0]["token"], "rt-signin");
}

#[tokio::test]
async fn sign_in_cancelled_in_browser() {
    let google = FakeGoogle::start(Arc::new(|p, _| panic!("unexpected {p}"))).await;
    let clock = TestClock::new(1_000_000);
    let auth = manager(&google, Arc::new(MemoryStore::default()), &clock);
    let browser: BrowserTask<()> = Arc::default();
    let slot = browser.clone();
    let err = auth
        .sign_in(move |auth_url| {
            let q: HashMap<String, String> = url::Url::parse(auth_url)
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            let target = format!(
                "{}?error=access_denied&state={}",
                q["redirect_uri"], q["state"]
            );
            *slot.lock().unwrap() = Some(tokio::spawn(async move {
                let resp = test_http().get(target).send().await.unwrap();
                assert_eq!(resp.status().as_u16(), 400);
            }));
        })
        .await
        .unwrap_err();
    assert!(matches!(&err, Error::SignInCancelled), "{err}");
    browser_done(&browser).await;
}

#[tokio::test]
async fn invalidate_forces_refresh() {
    let n = Arc::new(AtomicUsize::new(0));
    let counter = n.clone();
    let google = FakeGoogle::start(Arc::new(move |_, _| {
        let i = counter.fetch_add(1, Ordering::SeqCst) + 1;
        (
            200,
            format!(r#"{{"access_token":"at-{i}","expires_in":3600}}"#),
        )
    }))
    .await;
    let clock = TestClock::new(1_000_000);
    let auth = manager(&google, store_with("ada@example.com", "rt-1"), &clock);
    assert_eq!(auth.access_token("ada@example.com").await.unwrap(), "at-1");
    auth.invalidate_access_token("Ada@Example.com").await;
    assert_eq!(auth.access_token("ada@example.com").await.unwrap(), "at-2");
    assert_eq!(n.load(Ordering::SeqCst), 2);
}

#[test]
fn authorization_url_appends_extra_scopes_once() {
    let url = authorization_url(
        "https://a.example/auth",
        "c",
        "r",
        "ch",
        "s",
        None,
        &[CONTACTS_OTHER_READONLY_SCOPE, GMAIL_MODIFY_SCOPE, " "],
    )
    .unwrap();
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(
        q["scope"],
        format!("{SCOPES} {CONTACTS_OTHER_READONLY_SCOPE}")
    );
    assert_eq!(q["include_granted_scopes"], "true");
}

#[tokio::test]
async fn granted_scopes_follow_refresh_responses() {
    let google = FakeGoogle::start(Arc::new(|_, _| {
        (
            200,
            format!(
                r#"{{"access_token":"at","expires_in":3600,"scope":"openid {GMAIL_MODIFY_SCOPE}"}}"#
            ),
        )
    }))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    store
        .save(
            "ada@example.com",
            &StoredCredential::new(
                "rt".into(),
                vec![
                    GMAIL_MODIFY_SCOPE.into(),
                    CONTACTS_OTHER_READONLY_SCOPE.into(),
                ],
                1,
            ),
        )
        .unwrap();
    let auth = manager(&google, store.clone(), &clock);
    assert!(auth
        .granted_scopes("Ada@example.com")
        .iter()
        .any(|s| s == CONTACTS_OTHER_READONLY_SCOPE));

    // Google reports contacts revoked: the stored scopes follow.
    auth.access_token("ada@example.com").await.unwrap();
    let scopes = auth.granted_scopes("ada@example.com");
    assert!(scopes.iter().any(|s| s == GMAIL_MODIFY_SCOPE));
    assert!(!scopes.iter().any(|s| s == CONTACTS_OTHER_READONLY_SCOPE));
    assert_eq!(
        store
            .load("ada@example.com")
            .unwrap()
            .unwrap()
            .refresh_token,
        "rt"
    );
    assert!(auth.granted_scopes("nobody@example.com").is_empty());
}

#[tokio::test]
async fn sign_in_with_scopes_requests_them() {
    let challenge: Arc<StdMutex<String>> = Arc::default();
    let google = FakeGoogle::start(sign_in_handler(
        challenge.clone(),
        "openid https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/contacts.other.readonly",
    ))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    let auth = manager(&google, store.clone(), &clock);

    let requested: Arc<StdMutex<String>> = Arc::default();
    let seen = requested.clone();
    let (open, browser) = fake_browser(challenge, false);
    let account = auth
        .sign_in_with_scopes(
            Some("ada@example.com"),
            &[CONTACTS_OTHER_READONLY_SCOPE],
            move |url: &str| {
                let q: HashMap<String, String> = url::Url::parse(url)
                    .unwrap()
                    .query_pairs()
                    .into_owned()
                    .collect();
                *seen.lock().unwrap() = q["scope"].clone();
                open(url)
            },
        )
        .await
        .unwrap();
    browser_done(&browser).await;
    assert_eq!(account.email, "ada@example.com");
    assert!(requested
        .lock()
        .unwrap()
        .ends_with(CONTACTS_OTHER_READONLY_SCOPE));
    assert!(auth
        .granted_scopes("ada@example.com")
        .iter()
        .any(|s| s == CONTACTS_OTHER_READONLY_SCOPE));
}

/// Add account asks for Gmail and calendar in one consent. Unticking the
/// calendar box leaves it out of the token's `scope`: the sign-in still
/// succeeds (no revoke), and the stored scopes say calendar isn't granted.
#[tokio::test]
async fn unticked_optional_scope_still_signs_in() {
    use crate::calendar::CALENDAR_READONLY_SCOPE;
    let challenge: Arc<StdMutex<String>> = Arc::default();
    // Google reports only what the user left ticked.
    let google = FakeGoogle::start(sign_in_handler(
        challenge.clone(),
        "openid https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/userinfo.email",
    ))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    let auth = manager(&google, store.clone(), &clock);

    let requested: Arc<StdMutex<String>> = Arc::default();
    let seen = requested.clone();
    let (open, browser) = fake_browser(challenge, false);
    let account = auth
        .sign_in_with_scopes(None, &[CALENDAR_READONLY_SCOPE], move |url: &str| {
            let q: HashMap<String, String> = url::Url::parse(url)
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            *seen.lock().unwrap() = format!("{} {}", q["scope"], q["include_granted_scopes"]);
            open(url)
        })
        .await
        .unwrap();
    let page = browser_done(&browser).await;
    assert!(page.contains("ada@example.com is connected"));
    assert_eq!(account.email, "ada@example.com");
    assert_eq!(
        *requested.lock().unwrap(),
        format!("{SCOPES} {CALENDAR_READONLY_SCOPE} true"),
        "one consent for mail and calendar, incremental"
    );
    let granted = auth.granted_scopes("ada@example.com");
    assert!(granted.iter().any(|s| s == GMAIL_MODIFY_SCOPE));
    assert!(!granted.iter().any(|s| s == CALENDAR_READONLY_SCOPE));
    assert!(google.calls_to("/revoke").is_empty(), "the grant is kept");
}

// ---- system sign-in sheet (iOS client) ---------------------------------------

const IOS_ID: &str = "123456789012-iosclient.apps.googleusercontent.com";
const IOS_SCHEME: &str = "com.googleusercontent.apps.123456789012-iosclient";

/// What the fake sheet does with the authorization URL.
#[derive(Clone, Copy)]
enum SheetBehavior {
    Approve,
    WrongState,
    Cancel,
}

/// Stands in for ASWebAuthenticationSession: checks the request, then
/// "navigates" to the custom-scheme redirect.
struct FakeSheet {
    behavior: SheetBehavior,
    challenge_seen: Arc<StdMutex<String>>,
    requests: Arc<StdMutex<Vec<(String, String)>>>,
}

impl WebAuthSheet for FakeSheet {
    fn authenticate(&self, url: String, callback_scheme: String) -> SheetFuture {
        self.requests
            .lock()
            .unwrap()
            .push((url.clone(), callback_scheme.clone()));
        let q: HashMap<String, String> = url::Url::parse(&url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        *self.challenge_seen.lock().unwrap() = q["code_challenge"].clone();
        let behavior = self.behavior;
        Box::pin(async move {
            assert_eq!(q["client_id"], IOS_ID);
            assert_eq!(q["redirect_uri"], format!("{IOS_SCHEME}:/oauth2redirect"));
            assert_eq!(q["code_challenge_method"], "S256");
            let state = match behavior {
                SheetBehavior::Cancel => return Err(SheetError::Cancelled),
                SheetBehavior::WrongState => "forged".to_owned(),
                SheetBehavior::Approve => q["state"].clone(),
            };
            Ok(format!(
                "{callback_scheme}:/oauth2redirect?code=the-code&state={state}&scope=x"
            ))
        })
    }
}

fn ios_handler(challenge_seen: Arc<StdMutex<String>>) -> Handler {
    Arc::new(move |path, form| match path {
        "/token" if form["grant_type"] == "authorization_code" => {
            assert_eq!(form["client_id"], IOS_ID);
            assert!(
                !form.contains_key("client_secret"),
                "iOS clients have no secret"
            );
            assert_eq!(
                form["redirect_uri"],
                format!("{IOS_SCHEME}:/oauth2redirect")
            );
            assert_eq!(form["code"], "the-code");
            assert_eq!(
                challenge_s256(&form["code_verifier"]),
                *challenge_seen.lock().unwrap()
            );
            let body = serde_json::json!({
                "access_token": "at-sheet",
                "expires_in": 3599,
                "refresh_token": "rt-sheet",
                "scope": format!("openid {GMAIL_MODIFY_SCOPE}"),
                "id_token": id_token("ada@example.com", "Ada"),
            });
            (200, body.to_string())
        }
        "/token" => (
            200,
            r#"{"access_token":"at-refreshed","expires_in":3600}"#.into(),
        ),
        other => panic!("unexpected {other}"),
    })
}

fn sheet_ui(sheet: FakeSheet) -> SignInUi {
    SignInUi::new(|_: &str| panic!("the browser must not open when the sheet is used"))
        .with_sheet(Arc::new(sheet))
}

fn fake_sheet(behavior: SheetBehavior, challenge: &Arc<StdMutex<String>>) -> FakeSheet {
    FakeSheet {
        behavior,
        challenge_seen: challenge.clone(),
        requests: Arc::default(),
    }
}

#[tokio::test]
async fn sheet_sign_in_uses_ios_client_and_moves_the_account() {
    let challenge: Arc<StdMutex<String>> = Arc::default();
    let google = FakeGoogle::start(ios_handler(challenge.clone())).await;
    let clock = TestClock::new(1_000_000);
    // Previously signed in through the browser with the desktop client.
    let store = store_with("ada@example.com", "rt-desktop");
    let auth = manager(&google, store.clone(), &clock);
    auth.set_ios_client(Some(IosClientConfig::parse(IOS_ID).unwrap()));

    let sheet = fake_sheet(SheetBehavior::Approve, &challenge);
    let requests = sheet.requests.clone();
    let account = auth
        .sign_in_interactive(
            Some("ada@example.com"),
            &[CONTACTS_OTHER_READONLY_SCOPE],
            &sheet_ui(sheet),
        )
        .await
        .unwrap();
    assert_eq!(account.email, "ada@example.com");
    assert_eq!(requests.lock().unwrap()[0].1, IOS_SCHEME);
    assert!(requests.lock().unwrap()[0]
        .0
        .contains("login_hint=ada%40example.com"));

    let saved = store.load("ada@example.com").unwrap().unwrap();
    assert_eq!(saved.refresh_token, "rt-sheet");
    assert_eq!(
        saved.client,
        TokenClient::Ios {
            client_id: IOS_ID.into()
        }
    );

    // Refresh goes to the iOS client, with no secret.
    clock.advance(3600);
    assert_eq!(
        auth.access_token("ada@example.com").await.unwrap(),
        "at-refreshed"
    );
    let refresh = google.calls_to("/token").into_iter().last().unwrap();
    assert_eq!(refresh["grant_type"], "refresh_token");
    assert_eq!(refresh["client_id"], IOS_ID);
    assert_eq!(refresh["refresh_token"], "rt-sheet");
    assert!(!refresh.contains_key("client_secret"));
}

#[tokio::test]
async fn refresh_uses_the_client_that_minted_each_token() {
    let google = FakeGoogle::start(Arc::new(|_, _| {
        (200, r#"{"access_token":"at","expires_in":3600}"#.into())
    }))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    store
        .save(
            "desk@example.com",
            &StoredCredential::new("rt-d".into(), vec![], 1),
        )
        .unwrap();
    store
        .save(
            "ios@example.com",
            &StoredCredential::new("rt-i".into(), vec![], 1).with_client(TokenClient::Ios {
                client_id: IOS_ID.into(),
            }),
        )
        .unwrap();
    // No iOS client configured any more: iOS tokens still refresh by their stored id.
    let auth = manager(&google, store, &clock);
    auth.access_token("desk@example.com").await.unwrap();
    auth.access_token("ios@example.com").await.unwrap();

    let calls = google.calls_to("/token");
    let by_token = |rt: &str| {
        calls
            .iter()
            .find(|f| f["refresh_token"] == rt)
            .unwrap()
            .clone()
    };
    let desk = by_token("rt-d");
    assert_eq!(desk["client_id"], "cid.apps.googleusercontent.com");
    assert_eq!(desk["client_secret"], "csecret");
    let ios = by_token("rt-i");
    assert_eq!(ios["client_id"], IOS_ID);
    assert!(!ios.contains_key("client_secret"));
}

#[tokio::test]
async fn sheet_rejects_mismatched_state() {
    let challenge: Arc<StdMutex<String>> = Arc::default();
    let google =
        FakeGoogle::start(Arc::new(|p, _| panic!("no token exchange expected: {p}"))).await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    let auth = manager(&google, store.clone(), &clock);
    auth.set_ios_client(Some(IosClientConfig::parse(IOS_ID).unwrap()));
    let err = auth
        .sign_in_interactive(
            None,
            &[],
            &sheet_ui(fake_sheet(SheetBehavior::WrongState, &challenge)),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::OAuth(m) if m.contains("didn't match")),
        "{err}"
    );
    assert!(store.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn closing_the_sheet_is_cancelled() {
    let challenge: Arc<StdMutex<String>> = Arc::default();
    let google = FakeGoogle::start(Arc::new(|p, _| panic!("unexpected {p}"))).await;
    let clock = TestClock::new(1_000_000);
    let auth = manager(&google, Arc::new(MemoryStore::default()), &clock);
    auth.set_ios_client(Some(IosClientConfig::parse(IOS_ID).unwrap()));
    let err = auth
        .sign_in_interactive(
            None,
            &[],
            &sheet_ui(fake_sheet(SheetBehavior::Cancel, &challenge)),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SignInCancelled));
}

#[tokio::test]
async fn without_ios_client_the_browser_is_used_and_focus_hook_fires() {
    let challenge: Arc<StdMutex<String>> = Arc::default();
    let google = FakeGoogle::start(sign_in_handler(
        challenge.clone(),
        "openid https://www.googleapis.com/auth/gmail.modify",
    ))
    .await;
    let clock = TestClock::new(1_000_000);
    let store = Arc::new(MemoryStore::default());
    let auth = manager(&google, store.clone(), &clock);

    let (open, browser) = fake_browser(challenge.clone(), false);
    let focused = Arc::new(AtomicUsize::new(0));
    let hook = focused.clone();
    let sheet = fake_sheet(SheetBehavior::Approve, &challenge);
    let sheet_requests = sheet.requests.clone();
    // A sheet is offered, but no iOS client is configured: loopback it is.
    let ui = SignInUi::new(open)
        .with_sheet(Arc::new(sheet))
        .on_browser_success(move || {
            hook.fetch_add(1, Ordering::SeqCst);
        });
    let account = auth.sign_in_interactive(None, &[], &ui).await.unwrap();
    assert_eq!(account.email, "ada@example.com");
    let page = browser_done(&browser).await;
    assert!(page.contains("returning to Penguin"));
    assert!(page.contains("window.close()"));
    assert_eq!(focused.load(Ordering::SeqCst), 1);
    assert!(sheet_requests.lock().unwrap().is_empty());
    assert_eq!(
        store.load("ada@example.com").unwrap().unwrap().client,
        TokenClient::Desktop {
            client_id: Some("cid.apps.googleusercontent.com".into())
        }
    );
}
