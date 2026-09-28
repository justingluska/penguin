//! HTTP for Microsoft Graph and the Microsoft identity platform.
//!
//! Everything goes through a [`Transport`] (reqwest in the app, an
//! in-memory Graph in tests), so the whole provider (sign-in, token
//! refresh, sync, drafts) runs against fixtures without a network.
//!
//! [`GraphApi`] is one account's Graph client: bearer token, the
//! `Prefer: IdType="ImmutableId"` header on **every** request (without it
//! message ids change when a message moves), at most four requests in
//! flight per mailbox (Graph's limit), and throttling handled here: 429 and
//! 503 are retried after `Retry-After` (or a backoff), and a 429 that
//! outlasts the retries surfaces as `RateLimited`.
//!
//! Logs carry the method, the path without its query (queries can hold a
//! search) and status codes; never tokens or bodies.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use penguin_provider::{async_trait, Error, Result};
use serde::Deserialize;
use tokio::sync::Semaphore;

use crate::auth::Auth;

/// The Microsoft endpoints (overridable for tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Graph root, e.g. `https://graph.microsoft.com/v1.0`.
    pub graph: String,
    /// The `/common` authorization endpoint.
    pub authorize: String,
    /// The `/common` token endpoint.
    pub token: String,
}

impl Endpoints {
    /// Microsoft's public cloud with the `/common` authority (personal
    /// Microsoft accounts and every work or school tenant).
    pub fn microsoft() -> Endpoints {
        Endpoints {
            graph: "https://graph.microsoft.com/v1.0".into(),
            authorize: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Patch => "PATCH",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// One HTTP request. `Debug` shows the method and path only.
#[derive(Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.method.as_str(), path_of(&self.url))
    }
}

/// The path of a URL without its query (safe to log).
pub(crate) fn path_of(url: &str) -> &str {
    let no_query = url.split('?').next().unwrap_or(url);
    match no_query.find("://") {
        Some(i) => {
            let rest = &no_query[i + 3..];
            rest.find('/').map(|j| &rest[j..]).unwrap_or("/")
        }
        None => no_query,
    }
}

#[derive(Debug, Clone, Default)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json<T: for<'de> Deserialize<'de>>(&self, what: &str) -> Result<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| Error::Other(format!("{what}: unexpected response from Microsoft ({e})")))
    }
}

/// Sends one request. Transport failures (DNS, TLS, timeouts, resets) are
/// `Error::Network`; any HTTP status is a `Response`.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    async fn send(&self, request: Request) -> Result<Response>;
}

/// The real network.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> ReqwestTransport {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // Attachments and raw sources can be tens of MB.
            .timeout(Duration::from_secs(180))
            .build()
            .expect("reqwest client with rustls builds");
        ReqwestTransport { client }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, request: Request) -> Result<Response> {
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Patch => reqwest::Method::PATCH,
            Method::Put => reqwest::Method::PUT,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut b = self.client.request(method, &request.url);
        for (k, v) in &request.headers {
            b = b.header(k, v);
        }
        if let Some(body) = request.body {
            b = b.body(body);
        }
        let resp = b.send().await.map_err(network)?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let body = resp.bytes().await.map_err(network)?.to_vec();
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

fn network(e: reqwest::Error) -> Error {
    Error::Network(e.without_url().to_string())
}

/// Graph's error body: `{"error": {"code": "...", "message": "..."}}`.
#[derive(Deserialize)]
struct GraphErrorBody {
    error: GraphErrorInner,
}

#[derive(Deserialize)]
struct GraphErrorInner {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// An HTTP error from Graph as `Error::Http`, with `code: message` from
/// Graph's error JSON (never user content; capped).
pub(crate) fn http_error(status: u16, body: &[u8]) -> Error {
    let text = match serde_json::from_slice::<GraphErrorBody>(body) {
        Ok(e) => {
            let mut s = format!("{}: {}", e.error.code, e.error.message);
            if s.len() > 300 {
                let mut cut = 300;
                while !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                s.truncate(cut);
            }
            s
        }
        Err(_) => format!("{} bytes", body.len()),
    };
    Error::Http { status, body: text }
}

/// Graph's error code of an `Error::Http` built by [`http_error`].
pub(crate) fn graph_code(e: &Error) -> Option<&str> {
    match e {
        Error::Http { body, .. } => body.split(':').next(),
        _ => None,
    }
}

/// The object doesn't exist: 404, or an id Graph can't even parse (400
/// `ErrorInvalidIdMalformed`), which for us also means "no such message".
pub(crate) fn is_gone(e: &Error) -> bool {
    match e {
        Error::Http { status: 404, .. } => true,
        Error::Http { status: 400, .. } => {
            matches!(graph_code(e), Some("ErrorInvalidIdMalformed"))
        }
        _ => false,
    }
}

/// What may be retried after a throttle or transport failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Retry {
    /// Reads, PATCHes, moves, deletes: safe to repeat.
    Idempotent,
    /// Sends and creates: only a 429 (certainly not processed) is retried.
    OnlyIfRejected,
}

/// A request body.
pub(crate) enum Body {
    None,
    Json(serde_json::Value),
    /// Raw bytes with their content type (MIME drafts are base64 text/plain).
    Bytes(Vec<u8>, &'static str),
}

/// Attempts per request (throttling and transport retries).
const MAX_ATTEMPTS: u32 = 5;
/// Longest `Retry-After` honored in one wait.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(120);
/// Graph allows 4 concurrent requests per app per mailbox.
pub(crate) const MAILBOX_CONCURRENCY: usize = 4;

/// One account's Graph client.
#[derive(Clone)]
pub struct GraphApi {
    account_id: String,
    transport: Arc<dyn Transport>,
    auth: Arc<Auth>,
    base: String,
    permits: Arc<Semaphore>,
}

impl GraphApi {
    pub fn new(account_id: &str, transport: Arc<dyn Transport>, auth: Arc<Auth>) -> GraphApi {
        let base = auth.endpoints().graph.trim_end_matches('/').to_string();
        GraphApi {
            account_id: account_id.to_string(),
            transport,
            auth,
            base,
            permits: Arc::new(Semaphore::new(MAILBOX_CONCURRENCY)),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// `path` relative to the Graph root (`/me/...`), or an absolute
    /// `@odata.nextLink` / `@odata.deltaLink`, plus query pairs.
    pub(crate) fn url(&self, path: &str, query: &[(&str, String)]) -> Result<String> {
        let full = if path.starts_with("https://") || path.starts_with("http://") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
        };
        if query.is_empty() {
            return Ok(full);
        }
        let mut url = url::Url::parse(&full)
            .map_err(|e| Error::Other(format!("bad Graph URL {}: {e}", path_of(&full))))?;
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
        }
        Ok(url.to_string())
    }

    /// Send with auth, the immutable-id preference, the mailbox concurrency
    /// cap, throttling retries and one token refresh on 401. Non-2xx
    /// answers become `Error::Http` (429 after retries: `RateLimited`).
    pub(crate) async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Body,
        prefer: &[&str],
        retry: Retry,
    ) -> Result<Response> {
        let url = self.url(path, query)?;
        let (body, content_type) = match body {
            Body::None => (None, None),
            Body::Json(v) => (
                Some(serde_json::to_vec(&v).map_err(|e| Error::Other(e.to_string()))?),
                Some("application/json"),
            ),
            Body::Bytes(b, ct) => (Some(b), Some(ct)),
        };
        let mut prefs = vec!["IdType=\"ImmutableId\""];
        prefs.extend_from_slice(prefer);
        let prefer_header = prefs.join(", ");
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let token = self.auth.access_token(&self.account_id).await?;
            let mut headers = vec![
                ("Authorization".to_string(), format!("Bearer {token}")),
                ("Prefer".to_string(), prefer_header.clone()),
                ("Accept".to_string(), "application/json".to_string()),
            ];
            if let Some(ct) = content_type {
                headers.push(("Content-Type".to_string(), ct.to_string()));
            }
            let request = Request {
                method,
                url: url.clone(),
                headers,
                body: body.clone(),
            };
            let sent = {
                let _permit = self
                    .permits
                    .acquire()
                    .await
                    .map_err(|_| Error::Other("Graph client closed".into()))?;
                self.transport.send(request).await
            };
            let resp = match sent {
                Ok(r) => r,
                Err(Error::Network(msg)) => {
                    if retry == Retry::Idempotent && attempt < 3 {
                        tracing::debug!(path = path_of(&url), error = %msg, "Graph request failed; retrying");
                        tokio::time::sleep(backoff(attempt)).await;
                        continue;
                    }
                    return Err(Error::Network(msg));
                }
                Err(e) => return Err(e),
            };
            if resp.is_success() {
                return Ok(resp);
            }
            match resp.status {
                401 if !refreshed => {
                    // A token the cache still thought fresh: refresh once.
                    refreshed = true;
                    self.auth.invalidate(&self.account_id).await;
                    continue;
                }
                429 | 503 | 504 => {
                    let retryable = resp.status == 429 || retry == Retry::Idempotent;
                    if retryable && attempt < MAX_ATTEMPTS {
                        let wait = retry_after(&resp).unwrap_or_else(|| backoff(attempt));
                        tracing::info!(
                            path = path_of(&url),
                            status = resp.status,
                            wait_secs = wait.as_secs(),
                            "Graph throttled; waiting"
                        );
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    if resp.status == 429 {
                        return Err(Error::RateLimited);
                    }
                    return Err(http_error(resp.status, &resp.body));
                }
                _ => return Err(http_error(resp.status, &resp.body)),
            }
        }
    }

    pub(crate) async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, String)],
        prefer: &[&str],
        what: &str,
    ) -> Result<T> {
        self.call(
            Method::Get,
            path,
            query,
            Body::None,
            prefer,
            Retry::Idempotent,
        )
        .await?
        .json(what)
    }

    /// PUT bytes to an absolute URL without the Graph token (attachment
    /// upload sessions: the URL itself authorizes the upload).
    pub(crate) async fn put_unauthenticated(
        &self,
        url: &str,
        bytes: Vec<u8>,
        headers: Vec<(String, String)>,
    ) -> Result<Response> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let request = Request {
                method: Method::Put,
                url: url.to_string(),
                headers: headers.clone(),
                body: Some(bytes.clone()),
            };
            let resp = {
                let _permit = self
                    .permits
                    .acquire()
                    .await
                    .map_err(|_| Error::Other("Graph client closed".into()))?;
                self.transport.send(request).await?
            };
            if resp.is_success() {
                return Ok(resp);
            }
            if matches!(resp.status, 429 | 503) && attempt < MAX_ATTEMPTS {
                tokio::time::sleep(retry_after(&resp).unwrap_or_else(|| backoff(attempt))).await;
                continue;
            }
            return Err(http_error(resp.status, &resp.body));
        }
    }
}

/// `Retry-After` in seconds (Graph never sends the HTTP-date form), capped.
pub(crate) fn retry_after(resp: &Response) -> Option<Duration> {
    let secs: u64 = resp.header("Retry-After")?.trim().parse().ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}

/// 2 s, 4 s, 8 s, … for retries without a `Retry-After`.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1u64 << attempt.clamp(1, 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_drop_queries_and_hosts() {
        assert_eq!(
            path_of("https://graph.microsoft.com/v1.0/me/messages?$search=%22secret%22"),
            "/v1.0/me/messages"
        );
        assert_eq!(path_of("/me/messages?x=1"), "/me/messages");
        let r = Request {
            method: Method::Get,
            url: "https://g.example/v1.0/me/messages?$search=private".into(),
            headers: vec![("Authorization".into(), "Bearer tok-secret".into())],
            body: None,
        };
        let s = format!("{r:?}");
        assert!(!s.contains("private") && !s.contains("tok-secret"), "{s}");
    }

    #[test]
    fn graph_errors_keep_code_and_message() {
        let e = http_error(
            404,
            br#"{"error":{"code":"ErrorItemNotFound","message":"The specified object was not found in the store."}}"#,
        );
        assert!(e.is_not_found());
        assert_eq!(graph_code(&e), Some("ErrorItemNotFound"));
        assert!(is_gone(&e));
        let malformed = http_error(
            400,
            br#"{"error":{"code":"ErrorInvalidIdMalformed","message":"Id is malformed."}}"#,
        );
        assert!(is_gone(&malformed));
        assert!(!is_gone(&http_error(400, b"{}")));
        assert_eq!(http_error(502, b"<html>").to_string(), "http 502: 6 bytes");
    }

    #[test]
    fn retry_after_is_capped_seconds() {
        let resp = |v: &str| Response {
            status: 429,
            headers: vec![("retry-after".into(), v.into())],
            body: vec![],
        };
        assert_eq!(retry_after(&resp("7")), Some(Duration::from_secs(7)));
        assert_eq!(retry_after(&resp("9000")), Some(MAX_RETRY_AFTER));
        assert_eq!(retry_after(&resp("soon")), None);
    }
}
