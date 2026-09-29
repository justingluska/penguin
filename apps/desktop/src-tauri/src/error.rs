//! The one error shape every command returns: `{ code, message }`.
//! `code` lets the UI branch (re-auth prompt, onboarding, offline banner)
//! without parsing text.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// The account's refresh token is missing or revoked: sign in again.
    NeedsReauth,
    /// No Google OAuth client JSON yet: show the setup screen.
    NotConfigured,
    /// Offline, timeout, rate limit or a Google 5xx: retryable.
    Network,
    NotFound,
    InvalidInput,
    /// The user cancelled (e.g. cancel_sign_in during a browser sign-in).
    Cancelled,
    /// Settings don't allow it (an agent above its level in Settings →
    /// Developer → Agents), or the caller couldn't prove it may ask.
    PermissionDenied,
    /// Something it needs isn't running (penguin-cli: the Penguin app).
    Unavailable,
    Other,
}

impl ErrorCode {
    /// The wire name (`needsReauth`, `permissionDenied`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NeedsReauth => "needsReauth",
            ErrorCode::NotConfigured => "notConfigured",
            ErrorCode::Network => "network",
            ErrorCode::NotFound => "notFound",
            ErrorCode::InvalidInput => "invalidInput",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::PermissionDenied => "permissionDenied",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdError {
    pub code: ErrorCode,
    pub message: String,
}

pub type CmdResult<T> = Result<T, CmdError>;

impl CmdError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        CmdError {
            code,
            message: message.into(),
        }
    }
    pub fn not_configured() -> Self {
        Self::new(
            ErrorCode::NotConfigured,
            "Google OAuth client is not configured yet",
        )
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "Sign-in cancelled")
    }
    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Other, message)
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl From<penguin_core::Error> for CmdError {
    fn from(e: penguin_core::Error) -> Self {
        let code = match e {
            penguin_core::Error::NotFound(_) => ErrorCode::NotFound,
            penguin_core::Error::InvalidQuery(_) => ErrorCode::InvalidInput,
            penguin_core::Error::Db(_) => ErrorCode::Other,
        };
        CmdError::new(code, e.to_string())
    }
}

impl From<penguin_gmail::Error> for CmdError {
    fn from(e: penguin_gmail::Error) -> Self {
        use penguin_gmail::Error as G;
        if let G::Store(inner) = e {
            return inner.into();
        }
        let code = match &e {
            G::NotConfigured(_) => ErrorCode::NotConfigured,
            G::NeedsReauth(_) => ErrorCode::NeedsReauth,
            G::SignInCancelled => ErrorCode::Cancelled,
            G::Network(_) | G::RateLimited => ErrorCode::Network,
            G::Http { status, .. } if *status == 429 || *status >= 500 => ErrorCode::Network,
            G::Http { status: 404, .. } => ErrorCode::NotFound,
            _ => ErrorCode::Other,
        };
        CmdError::new(code, e.to_string())
    }
}

/// Provider errors (every mailbox call goes through the provider seam).
/// Gmail's arrive here variant for variant, so they map exactly as
/// `penguin_gmail::Error` does above.
impl From<penguin_provider::Error> for CmdError {
    fn from(e: penguin_provider::Error) -> Self {
        use penguin_provider::Error as P;
        if let P::Store(inner) = e {
            return inner.into();
        }
        let code = match &e {
            P::NotConfigured(_) => ErrorCode::NotConfigured,
            P::NeedsReauth(_) => ErrorCode::NeedsReauth,
            P::SignInCancelled => ErrorCode::Cancelled,
            P::Network(_) | P::RateLimited => ErrorCode::Network,
            P::Http { status, .. } if *status == 429 || *status >= 500 => ErrorCode::Network,
            P::Http { status: 404, .. } | P::NotFound(_) => ErrorCode::NotFound,
            P::InvalidInput(_) => ErrorCode::InvalidInput,
            _ => ErrorCode::Other,
        };
        CmdError::new(code, e.to_string())
    }
}

impl From<tauri::Error> for CmdError {
    fn from(e: tauri::Error) -> Self {
        CmdError::other(e.to_string())
    }
}

impl From<std::io::Error> for CmdError {
    fn from(e: std::io::Error) -> Self {
        CmdError::other(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_code_and_message() {
        let e = CmdError::from(penguin_gmail::Error::NeedsReauth("ada@x.example".into()));
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["code"], "needsReauth");
        assert!(json["message"].as_str().unwrap().contains("sign in again"));
        let e = CmdError::from(penguin_gmail::Error::Http {
            status: 503,
            body: String::new(),
        });
        assert_eq!(e.code, ErrorCode::Network);
    }

    /// A Gmail error reaches the UI the same whether it came straight from
    /// penguin-gmail or through the provider seam.
    #[test]
    fn provider_errors_map_like_gmail_errors() {
        use penguin_gmail::Error as G;
        let cases = || {
            vec![
                G::NotConfigured("x".into()),
                G::NeedsReauth("ada@x.example".into()),
                G::SignInCancelled,
                G::Keychain("denied".into()),
                G::RateLimited,
                G::Network("offline".into()),
                G::Http {
                    status: 404,
                    body: "gone".into(),
                },
                G::Http {
                    status: 429,
                    body: String::new(),
                },
                G::Http {
                    status: 400,
                    body: "bad".into(),
                },
                G::Other("odd".into()),
                G::Store(penguin_core::Error::NotFound("m".into())),
            ]
        };
        for (direct, via) in cases().into_iter().zip(cases()) {
            let a = CmdError::from(direct);
            let b = CmdError::from(penguin_provider::Error::from(via));
            assert_eq!((a.code, a.message), (b.code, b.message));
        }
        use penguin_provider::Error as P;
        assert_eq!(
            CmdError::from(P::NotFound("gone".into())).code,
            ErrorCode::NotFound
        );
        let e = CmdError::from(P::InvalidInput("Bad password".into()));
        assert_eq!(
            (e.code, e.message.as_str()),
            (ErrorCode::InvalidInput, "Bad password")
        );
        assert_eq!(
            CmdError::from(P::Unsupported("nope".into())).code,
            ErrorCode::Other
        );
    }
}
