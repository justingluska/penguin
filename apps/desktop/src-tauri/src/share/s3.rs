//! The S3 requests behind share links: PUT an object, DELETE it, presign a
//! GET. rusty-s3 signs (AWS Signature V4, query-string form, sans-IO); the
//! requests go through reqwest with rustls, redirects off, and no cookies.
//!
//! Every request is signed as a presigned URL: PUT and DELETE with a short
//! signature lifetime, the shared GET with the link lifetime. Presigned URLs
//! are bearer credentials: never log one (log the host and status only).

use std::time::{Duration, SystemTime};

use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

use super::config::Target;

/// How long a PUT/DELETE/test-GET signature is good for (it only has to
/// outlive the start of the request).
const REQUEST_SIGNATURE: Duration = Duration::from_secs(15 * 60);
/// Clocks further apart than this are reported as the likely cause of an
/// authentication failure (S3 itself allows 15 minutes).
const SKEW_TOLERANCE: Duration = Duration::from_secs(5 * 60);
/// Longest error body read (S3 error documents are a few hundred bytes).
const MAX_ERROR_BODY: usize = 16 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A signing context for one bucket.
pub struct Signer {
    bucket: Bucket,
    credentials: Credentials,
    host: String,
}

impl Signer {
    pub fn new(target: &Target, secret: &str) -> Result<Signer, String> {
        let host = target.endpoint.host_str().unwrap_or_default().to_string();
        // Virtual-hosted style for AWS itself (path style is deprecated
        // there), unless the bucket has dots (its certificate wouldn't match);
        // path style everywhere else (R2, MinIO and most S3-compatible
        // services document it).
        let style = if host.ends_with(".amazonaws.com") && !target.bucket.contains('.') {
            UrlStyle::VirtualHost
        } else {
            UrlStyle::Path
        };
        let bucket = Bucket::new(
            target.endpoint.clone(),
            style,
            target.bucket.clone(),
            target.region.clone(),
        )
        .map_err(|e| format!("The endpoint can't be used: {e}"))?;
        Ok(Signer {
            bucket,
            credentials: Credentials::new(target.access_key_id.clone(), secret.to_string()),
            host,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// Presigned PUT carrying `Content-Type` and `Content-Disposition` (both
    /// signed, so they must be sent exactly as given).
    pub fn put_url(
        &self,
        key: &str,
        content_type: &str,
        disposition: &str,
        now: SystemTime,
    ) -> url::Url {
        let mut action = self.bucket.put_object(Some(&self.credentials), key);
        action
            .headers_mut()
            .insert("content-disposition", disposition.to_string());
        action
            .headers_mut()
            .insert("content-type", content_type.to_string());
        action.sign_with_time(REQUEST_SIGNATURE, &timestamp(now))
    }

    pub fn delete_url(&self, key: &str, now: SystemTime) -> url::Url {
        let action = self.bucket.delete_object(Some(&self.credentials), key);
        action.sign_with_time(REQUEST_SIGNATURE, &timestamp(now))
    }

    /// The link: a presigned GET that works for `lifetime` from `now`.
    pub fn get_url(&self, key: &str, lifetime: Duration, now: SystemTime) -> url::Url {
        let action = self.bucket.get_object(Some(&self.credentials), key);
        action.sign_with_time(lifetime, &timestamp(now))
    }

    /// The object's plain address, without a signature (the test checks the
    /// bucket refuses it).
    pub fn unsigned_url(&self, key: &str) -> Result<url::Url, String> {
        self.bucket.object_url(key).map_err(|e| e.to_string())
    }
}

fn timestamp(t: SystemTime) -> jiff::Timestamp {
    jiff::Timestamp::try_from(t).unwrap_or(jiff::Timestamp::UNIX_EPOCH)
}

/// Why a request failed, worded for the Settings test and toasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3Error {
    /// DNS, connection, TLS or timeout: most likely a wrong endpoint.
    Unreachable { host: String, timeout: bool },
    /// This Mac's clock and the storage's differ by this many minutes.
    ClockSkew { minutes: i64 },
    /// `InvalidAccessKeyId`.
    BadKeyId,
    /// `SignatureDoesNotMatch`: the secret doesn't belong to the key id.
    BadSecret,
    /// `NoSuchBucket`.
    NoBucket,
    /// `AccessDenied`: the key has no permission for this.
    Denied,
    /// The bucket lives in another region (the one the storage named, if it did).
    WrongRegion(Option<String>),
    /// A 3xx: another endpoint serves this bucket.
    Moved,
    /// Anything else: the HTTP status and S3 error code, if any.
    Status { status: u16, code: Option<String> },
}

impl S3Error {
    /// A sentence for the user; `bucket` is the configured bucket's name.
    pub fn message(&self, bucket: &str) -> String {
        match self {
            S3Error::Unreachable { host, timeout: false } => format!(
                "Couldn't reach {host}. Check the endpoint (for R2 it is https://<account id>.r2.cloudflarestorage.com) and your connection."
            ),
            S3Error::Unreachable { host, timeout: true } => {
                format!("{host} didn't answer in time. Check your connection and try again.")
            }
            S3Error::ClockSkew { minutes } => format!(
                "This Mac's clock is {} minute{} off from the storage's, so it refuses the signature. Turn on \"Set time and date automatically\" in System Settings → General → Date & Time.",
                minutes.abs(),
                if minutes.abs() == 1 { "" } else { "s" }
            ),
            S3Error::BadKeyId => "The storage doesn't know this access key ID. Check it, or make a new API token.".into(),
            S3Error::BadSecret => {
                "The secret access key doesn't match the access key ID. Paste the secret again (it is shown only once when the token is made).".into()
            }
            S3Error::NoBucket => format!("There is no bucket named {bucket} at this endpoint."),
            S3Error::Denied => format!(
                "The key isn't allowed to do this in {bucket}. Give its token Object Read & Write on this bucket."
            ),
            S3Error::WrongRegion(Some(r)) => format!("The bucket is in region {r}: put {r} in Region."),
            S3Error::WrongRegion(None) => "The region is wrong for this bucket (for R2 it is auto).".into(),
            S3Error::Moved => "Another endpoint serves this bucket. Check the endpoint and region.".into(),
            S3Error::Status { status, code: Some(code) } => format!("The storage refused it: {code} (HTTP {status})."),
            S3Error::Status { status, code: None } => format!("The storage refused it (HTTP {status})."),
        }
    }
}

/// The `<Code>` (and `<Region>`) of an S3 XML error document. A string
/// search, not an XML parser: the document is small, flat and only read for
/// these two words.
pub fn error_fields(body: &str) -> (Option<String>, Option<String>) {
    let field = |name: &str| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = body.find(&open)? + open.len();
        let end = body[start..].find(&close)? + start;
        let v = body[start..end].trim();
        (!v.is_empty()
            && v.len() <= 64
            && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then(|| v.to_string())
    };
    (field("Code"), field("Region"))
}

/// Minutes between the storage's `Date` and this Mac's clock (positive: the
/// Mac is ahead), when they are further apart than the tolerance.
pub fn skew_minutes(server_date: Option<&str>, now: SystemTime) -> Option<i64> {
    let server = chrono::DateTime::parse_from_rfc2822(server_date?.trim()).ok()?;
    let server =
        SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(server.timestamp()).ok()?);
    let (diff, ahead) = match now.duration_since(server) {
        Ok(d) => (d, true),
        Err(e) => (e.duration(), false),
    };
    if diff <= SKEW_TOLERANCE {
        return None;
    }
    let minutes = i64::try_from(diff.as_secs() / 60).unwrap_or(i64::MAX);
    Some(if ahead { minutes } else { -minutes })
}

/// Classify a failed response.
pub fn classify(status: u16, body: &str, server_date: Option<&str>, now: SystemTime) -> S3Error {
    let (code, region) = error_fields(body);
    if code.as_deref() == Some("RequestTimeTooSkewed") {
        let minutes = skew_minutes(server_date, now).unwrap_or(15);
        return S3Error::ClockSkew { minutes };
    }
    // An authentication failure with clocks far apart: the clock is the
    // likeliest cause whatever the code says (R2 answers a skewed presigned
    // URL with a plain AccessDenied or SignatureDoesNotMatch).
    if matches!(status, 400 | 401 | 403) {
        if let Some(minutes) = skew_minutes(server_date, now) {
            return S3Error::ClockSkew { minutes };
        }
    }
    match code.as_deref() {
        Some("InvalidAccessKeyId") => S3Error::BadKeyId,
        Some("SignatureDoesNotMatch") => S3Error::BadSecret,
        Some("NoSuchBucket") => S3Error::NoBucket,
        Some("AccessDenied") => S3Error::Denied,
        Some(
            "AuthorizationHeaderMalformed"
            | "AuthorizationQueryParametersError"
            | "IllegalLocationConstraintException",
        ) => S3Error::WrongRegion(region),
        Some("PermanentRedirect" | "TemporaryRedirect") => S3Error::Moved,
        _ if (300..400).contains(&status) => S3Error::Moved,
        _ if status == 403 => S3Error::Denied,
        _ => S3Error::Status { status, code },
    }
}

/// The one HTTP client share links use.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("Penguin/{}", crate::VERSION))
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "share links: custom HTTP client unavailable; using defaults");
            reqwest::Client::new()
        })
}

fn transport_error(host: &str, e: &reqwest::Error) -> S3Error {
    // The error text can contain the URL (with its signature): log the kind only.
    tracing::warn!(host = %host, timeout = e.is_timeout(), connect = e.is_connect(), "share links: request failed");
    S3Error::Unreachable {
        host: host.to_string(),
        timeout: e.is_timeout(),
    }
}

async fn failure(host: &str, what: &str, resp: reqwest::Response) -> S3Error {
    let status = resp.status().as_u16();
    let date = resp
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = read_capped(resp, MAX_ERROR_BODY).await.unwrap_or_default();
    let err = classify(
        status,
        &String::from_utf8_lossy(&body),
        date.as_deref(),
        SystemTime::now(),
    );
    tracing::warn!(host = %host, what, status, error = ?err, "share links: storage refused");
    err
}

async fn read_capped(mut resp: reqwest::Response, max: usize) -> Result<Vec<u8>, reqwest::Error> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = max.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
}

/// Upload `bytes` as `key`.
pub async fn put(
    http: &reqwest::Client,
    signer: &Signer,
    key: &str,
    bytes: Vec<u8>,
    content_type: &str,
    disposition: &str,
) -> Result<(), S3Error> {
    let url = signer.put_url(key, content_type, disposition, SystemTime::now());
    let resp = http
        .put(url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .header(reqwest::header::CONTENT_DISPOSITION, disposition)
        .body(bytes)
        .timeout(UPLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| transport_error(signer.host(), &e))?;
    if resp.status().is_success() {
        return Ok(());
    }
    Err(failure(signer.host(), "put", resp).await)
}

/// Delete `key`. A missing object is success (S3 answers 204 either way).
pub async fn delete(http: &reqwest::Client, signer: &Signer, key: &str) -> Result<(), S3Error> {
    let url = signer.delete_url(key, SystemTime::now());
    let resp = http
        .delete(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| transport_error(signer.host(), &e))?;
    if resp.status().is_success() || resp.status().as_u16() == 404 {
        return Ok(());
    }
    Err(failure(signer.host(), "delete", resp).await)
}

/// GET a presigned link and return up to `max` bytes of the object.
pub async fn get(
    http: &reqwest::Client,
    host: &str,
    url: url::Url,
    max: usize,
) -> Result<Vec<u8>, S3Error> {
    let resp = http
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| transport_error(host, &e))?;
    if !resp.status().is_success() {
        return Err(failure(host, "get", resp).await);
    }
    read_capped(resp, max)
        .await
        .map_err(|e| transport_error(host, &e))
}

/// The HTTP status a plain GET of `url` gets (the test's "is the bucket
/// private" check); only a transport failure is an error.
pub async fn status(http: &reqwest::Client, host: &str, url: url::Url) -> Result<u16, S3Error> {
    let resp = http
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| transport_error(host, &e))?;
    Ok(resp.status().as_u16())
}
