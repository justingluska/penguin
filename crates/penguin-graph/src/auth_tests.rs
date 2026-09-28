//! Sign-in, token refresh and error mapping against the fake Microsoft
//! identity platform (`fake_graph.rs`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use penguin_provider::credentials::{MemorySecrets, MicrosoftCredential, SecretVault};
use penguin_provider::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::fake_graph::{endpoints, FakeGraph, ME};

const CLIENT: &str = "1b2c3d4e-0000-1111-2222-333344445555";

fn vault() -> (Arc<SecretVault<MicrosoftCredential>>, Arc<MemorySecrets>) {
    let mem = Arc::new(MemorySecrets::default());
    (Arc::new(SecretVault::new(Box::new(mem.clone()))), mem)
}

fn credential(rt: &str) -> MicrosoftCredential {
    MicrosoftCredential {
        refresh_token: rt.into(),
        client_id: CLIENT.into(),
        scopes: vec![],
        tenant_id: None,
        object_id: None,
        obtained_at: 1,
    }
}

/// A clock the test moves forward.
fn clock() -> (Clock, Arc<AtomicU64>) {
    let offset = Arc::new(AtomicU64::new(0));
    let o = offset.clone();
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_760_000_000);
    (
        Arc::new(move || start + Duration::from_secs(o.load(Ordering::SeqCst))),
        offset,
    )
}

#[test]
fn token_errors_keep_the_aadsts_code_but_not_microsofts_text() {
    let body = |error: &str, code: u64| {
        format!(
            r#"{{"error":"{error}","error_description":"AADSTS{code}: The user sam@outlook.example did something. Trace ID: 1","error_codes":[{code}]}}"#
        )
        .into_bytes()
    };
    let e = token_error(400, &body("invalid_grant", 70008), Grant::Refresh);
    assert!(matches!(e, Error::NeedsReauth(_)), "{e:?}");
    assert!(e.to_string().contains("AADSTS70008") && !e.to_string().contains("sam@"));
    let e = token_error(400, &body("invalid_client", 700016), Grant::Refresh);
    assert!(matches!(e, Error::NeedsReauth(_)));
    let e = token_error(400, &body("invalid_grant", 65001), Grant::AuthorizationCode);
    assert!(
        matches!(&e, Error::OAuth(m) if m.starts_with("AADSTS65001") && m.contains("admin approval"))
    );
    let e = token_error(
        400,
        &body("invalid_request", 50011),
        Grant::AuthorizationCode,
    );
    assert!(e.to_string().contains("http://localhost"), "{e}");
    let e = token_error(
        401,
        &body("invalid_client", 7000218),
        Grant::AuthorizationCode,
    );
    assert!(
        e.to_string().contains("AADSTS7000218") && e.to_string().contains("Mobile and desktop")
    );
    let e = token_error(
        400,
        br#"{"error":"consent_required"}"#,
        Grant::AuthorizationCode,
    );
    assert!(e.to_string().contains("AADSTS65001"), "{e}");
    assert!(matches!(
        token_error(429, b"", Grant::Refresh),
        Error::RateLimited
    ));
    assert!(matches!(
        token_error(503, &body("temporarily_unavailable", 90033), Grant::Refresh),
        Error::Http { status: 503, .. }
    ));
    assert!(matches!(
        token_error(400, b"<html>", Grant::Refresh),
        Error::Http { .. }
    ));
}

#[test]
fn redirect_errors() {
    assert!(matches!(
        callback_error("access_denied", "AADSTS65004: User declined to consent", ""),
        Error::SignInCancelled
    ));
    assert!(matches!(
        callback_error("access_denied", "", "cancel"),
        Error::SignInCancelled
    ));
    let e = callback_error(
        "access_denied",
        "AADSTS90094: The grant requires admin permission.",
        "",
    );
    assert!(
        e.to_string().contains("AADSTS90094") && e.to_string().contains("admin approval"),
        "{e}"
    );
    let e = callback_error("consent_required", "", "");
    assert!(e.to_string().contains("AADSTS65001"));
    let e = callback_error(
        "invalid_request",
        "AADSTS50011: The redirect URI 'http://localhost:1' does not match",
        "",
    );
    assert!(e.to_string().contains("AADSTS50011") && e.to_string().contains("Mobile and desktop"));
    assert_eq!(aadsts_code("x AADSTS700016: y"), Some(700016));
    assert_eq!(aadsts_code("no code"), None);
}

#[test]
fn authorization_url_has_pkce_and_the_hint() {
    let (v, _) = vault();
    let auth = Auth::new(FakeGraph::new(), endpoints(), v);
    let url = auth
        .authorization_url(CLIENT, "http://localhost:5555", "chal", "st", ME)
        .unwrap();
    let parsed = url::Url::parse(&url).unwrap();
    let q: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    assert!(url.starts_with("https://login.fake.example/common/oauth2/v2.0/authorize?"));
    assert_eq!(q["client_id"], CLIENT);
    assert_eq!(q["redirect_uri"], "http://localhost:5555");
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["login_hint"], ME);
    assert_eq!(q["scope"], SCOPES);
    assert!(q["scope"].contains("offline_access") && q["scope"].contains("Mail.Send"));
    assert!(!q.contains_key("client_secret"));
    // RFC 7636 appendix B.
    assert_eq!(
        challenge_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

/// The whole browser flow: the "browser" follows the redirect to the
/// loopback listener with the code; tokens land in the vault.
#[tokio::test]
async fn browser_sign_in_end_to_end() {
    let fake = FakeGraph::new();
    let (v, mem) = vault();
    let auth = Auth::new(fake.clone(), endpoints(), v.clone());
    let opened = Arc::new(std::sync::Mutex::new(None::<String>));
    let o = opened.clone();
    let open = move |url: &str| {
        *o.lock().unwrap() = Some(url.to_string());
        let url = url::Url::parse(url).unwrap();
        let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
        let redirect = url::Url::parse(&q["redirect_uri"]).unwrap();
        let (port, state) = (redirect.port().unwrap(), q["state"].clone());
        tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            s.write_all(
                format!("GET /?code=good-code&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).await.unwrap();
            assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        });
    };
    let succeeded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = succeeded.clone();
    let on_success = move || s2.store(true, Ordering::SeqCst);
    let ui = SignInUi {
        open_browser: &open,
        on_success: Some(&on_success),
    };
    let signed = auth.sign_in(CLIENT, ME, &ui).await.unwrap();
    assert_eq!(signed.email, ME);
    assert_eq!(signed.display_name.as_deref(), Some("Sam Rivers"));
    assert_eq!(
        signed.tenant_id.as_deref(),
        Some("9188040d-6c67-4c5b-b112-36a304b66dad")
    );
    assert!(succeeded.load(Ordering::SeqCst));
    let saved = v.load(ME).unwrap().unwrap();
    assert_eq!(saved.client_id, CLIENT);
    assert_eq!(saved.refresh_token, fake.lock().refresh_token);
    assert!(
        saved.scopes.iter().any(|s| s == "Mail.ReadWrite"),
        "{:?}",
        saved.scopes
    );
    assert!(opened
        .lock()
        .unwrap()
        .as_deref()
        .unwrap()
        .contains("code_challenge="));
    // The access token from sign-in is used without another token call.
    let calls = fake.lock().token_calls;
    auth.access_token(ME).await.unwrap();
    assert_eq!(fake.lock().token_calls, calls);
    // Nothing secret in the stored item's debug form.
    assert!(!format!("{saved:?}").contains(&saved.refresh_token));
    assert_eq!(*mem.reads.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_different_account_is_refused_and_nothing_is_saved() {
    let fake = FakeGraph::new();
    let (v, _) = vault();
    let auth = Auth::new(fake.clone(), endpoints(), v.clone());
    let e = auth
        .complete_sign_in(
            CLIENT,
            "someone.else@outlook.example",
            "good-code",
            "verifier",
            "http://localhost:1",
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Error::InvalidInput(m) if m.contains(ME) && m.contains("someone.else@")),
        "{e:?}"
    );
    assert!(v.load("someone.else@outlook.example").unwrap().is_none());
    assert!(v.load(ME).unwrap().is_none());
    // An alias of the signed-in mailbox is the same account.
    let alias = "sam.rivers@alias.example";
    let s = auth
        .complete_sign_in(CLIENT, alias, "good-code", "verifier", "http://localhost:1")
        .await
        .unwrap();
    assert_eq!(s.email, alias);
    assert!(v.load(alias).unwrap().is_some());
    // A bad code is an OAuth error with its code.
    let e = auth
        .complete_sign_in(CLIENT, ME, "stale-code", "verifier", "http://localhost:1")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("AADSTS70000"), "{e}");
}

#[tokio::test]
async fn refresh_rotates_tokens_and_is_single_flight() {
    let fake = FakeGraph::new();
    let (v, _) = vault();
    v.save(ME, &credential("rt-0")).unwrap();
    let (clk, offset) = clock();
    let auth = Arc::new(Auth::with_clock(fake.clone(), endpoints(), v.clone(), clk));
    let (a, b) = tokio::join!(auth.access_token(ME), auth.access_token(ME));
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(
        fake.lock().token_calls,
        1,
        "one refresh for concurrent callers"
    );
    let rotated = v.load(ME).unwrap().unwrap();
    assert_eq!(rotated.refresh_token, fake.lock().refresh_token);
    assert_ne!(rotated.refresh_token, "rt-0");
    // Fresh: no call. Near expiry: refresh again with the rotated token.
    auth.access_token(ME).await.unwrap();
    assert_eq!(fake.lock().token_calls, 1);
    offset.store(3600 - 60, Ordering::SeqCst);
    auth.access_token(ME).await.unwrap();
    assert_eq!(fake.lock().token_calls, 2);
    // Revoked: NeedsReauth, and the cache is cleared.
    fake.lock().reject_refresh = true;
    auth.invalidate(ME).await;
    let e = auth.access_token(ME).await.unwrap_err();
    assert!(
        matches!(&e, Error::NeedsReauth(m) if m.contains("AADSTS70008")),
        "{e:?}"
    );
    // No credential at all: NeedsReauth too.
    auth.forget(ME).await.unwrap();
    assert!(matches!(
        auth.access_token(ME).await,
        Err(Error::NeedsReauth(_))
    ));
    assert!(!auth.has_credentials(ME));
}

#[test]
fn id_token_claims_decode() {
    let payload = URL_SAFE_NO_PAD.encode(
        r#"{"preferred_username":"sam@outlook.example","tid":"t1","oid":"o1","name":"Sam"}"#,
    );
    let c = id_token_claims(&format!("h.{payload}.s")).unwrap();
    assert_eq!(c.preferred_username.as_deref(), Some(ME));
    assert_eq!(c.tid.as_deref(), Some("t1"));
    assert!(id_token_claims("garbage").is_none());
}
