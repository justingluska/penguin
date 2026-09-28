//! Token endpoint payloads, error mapping, ID-token claims, and the
//! access-token cache entry.

use std::fmt;
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;

use crate::Error;

/// Refresh when the cached access token has less than this left.
pub(crate) const REFRESH_MARGIN: Duration = Duration::from_secs(60);
/// Google access tokens last an hour; used if `expires_in` is absent.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(3600);

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Space-separated granted scopes.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

impl TokenResponse {
    pub fn granted_scopes(&self) -> Vec<String> {
        self.scope
            .as_deref()
            .unwrap_or_default()
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect()
    }

    pub fn cached(&self, now: SystemTime) -> CachedToken {
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
}

/// Which request produced a token-endpoint error; `invalid_grant` means
/// different things for each.
pub(crate) enum Grant<'a> {
    Refresh { email: &'a str },
    AuthorizationCode,
}

/// Map a non-2xx token endpoint response to our error type. Google's error
/// bodies are `{"error": ..., "error_description": ...}` and carry no secrets,
/// but we still only surface those two fields.
pub(crate) fn token_error(status: u16, body: &str, grant: Grant<'_>) -> Error {
    if status == 429 {
        return Error::RateLimited;
    }
    let Ok(parsed) = serde_json::from_str::<TokenErrorResponse>(body) else {
        return Error::Http {
            status,
            body: format!("token endpoint returned {} bytes", body.len()),
        };
    };
    let detail = match &parsed.error_description {
        Some(d) => format!("{}: {d}", parsed.error),
        None => parsed.error.clone(),
    };
    match (parsed.error.as_str(), grant) {
        ("invalid_grant", Grant::Refresh { email }) => Error::NeedsReauth(email.to_owned()),
        ("invalid_grant", Grant::AuthorizationCode) => Error::OAuth(format!(
            "Google rejected the sign-in code ({detail}); try again"
        )),
        ("invalid_client" | "unauthorized_client", _) => Error::NotConfigured(format!(
            "Google rejected the OAuth client ({detail}); check the client JSON"
        )),
        _ if status >= 500 => Error::Http {
            status,
            body: detail,
        },
        _ => Error::OAuth(detail),
    }
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct IdClaims {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Read the claims from an ID token's payload segment without verifying the
/// signature. That's acceptable only because the token came straight from
/// Google's token endpoint over TLS in response to our own code exchange.
pub(crate) fn id_token_claims(id_token: &str) -> Option<IdClaims> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A short-lived access token held in memory only.
#[derive(Clone)]
pub(crate) struct CachedToken {
    pub access_token: String,
    pub expires_at: SystemTime,
}

impl CachedToken {
    /// Usable without refreshing: more than [`REFRESH_MARGIN`] left.
    pub fn is_fresh(&self, now: SystemTime) -> bool {
        now + REFRESH_MARGIN < self.expires_at
    }

    /// Not yet expired, even if inside the refresh margin.
    pub fn is_unexpired(&self, now: SystemTime) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn freshness_respects_refresh_margin() {
        let token = CachedToken {
            access_token: "at".into(),
            expires_at: t(10_000),
        };
        assert!(token.is_fresh(t(10_000 - 61)));
        assert!(!token.is_fresh(t(10_000 - 60)));
        assert!(!token.is_fresh(t(10_000 - 30)));
        assert!(token.is_unexpired(t(10_000 - 30)));
        assert!(!token.is_unexpired(t(10_000)));
        assert!(!token.is_fresh(t(20_000)));
    }

    #[test]
    fn cached_uses_expires_in_or_default() {
        let resp: TokenResponse =
            serde_json::from_str(r#"{"access_token":"a","expires_in":3599}"#).unwrap();
        assert_eq!(resp.cached(t(100)).expires_at, t(3699));
        let resp: TokenResponse = serde_json::from_str(r#"{"access_token":"a"}"#).unwrap();
        assert_eq!(resp.cached(t(100)).expires_at, t(3700));
    }

    #[test]
    fn granted_scopes_split() {
        let resp: TokenResponse = serde_json::from_str(
            r#"{"access_token":"a","scope":"openid https://www.googleapis.com/auth/gmail.modify  email"}"#,
        )
        .unwrap();
        assert_eq!(
            resp.granted_scopes(),
            [
                "openid",
                "https://www.googleapis.com/auth/gmail.modify",
                "email"
            ]
        );
    }

    #[test]
    fn maps_token_errors() {
        let body =
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        assert!(matches!(
            token_error(400, body, Grant::Refresh { email: "ada@example.com" }),
            Error::NeedsReauth(e) if e == "ada@example.com"
        ));
        assert!(matches!(
            token_error(400, body, Grant::AuthorizationCode),
            Error::OAuth(_)
        ));
        assert!(matches!(
            token_error(
                401,
                r#"{"error":"invalid_client"}"#,
                Grant::AuthorizationCode
            ),
            Error::NotConfigured(_)
        ));
        assert!(matches!(
            token_error(429, "", Grant::AuthorizationCode),
            Error::RateLimited
        ));
        assert!(matches!(
            token_error(503, r#"{"error":"server_error"}"#, Grant::AuthorizationCode),
            Error::Http { status: 503, .. }
        ));
        match token_error(502, "<html>secretish</html>", Grant::AuthorizationCode) {
            Error::Http { status: 502, body } => assert!(!body.contains("secretish")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn decodes_id_token_claims() {
        let payload = URL_SAFE_NO_PAD
            .encode(r#"{"email":"Ada@Example.com","email_verified":true,"name":"Ada"}"#);
        let claims = id_token_claims(&format!("eyJhbGciOiJSUzI1NiJ9.{payload}.sig")).unwrap();
        assert_eq!(claims.email.as_deref(), Some("Ada@Example.com"));
        assert_eq!(claims.email_verified, Some(true));
        assert_eq!(claims.name.as_deref(), Some("Ada"));
        assert!(id_token_claims("not-a-jwt").is_none());
        assert!(id_token_claims("a.!!!.c").is_none());
    }

    #[test]
    fn debug_redacts_access_token() {
        let token = CachedToken {
            access_token: "ya29.secret".into(),
            expires_at: t(0),
        };
        assert!(!format!("{token:?}").contains("ya29"));
    }
}
