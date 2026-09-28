//! The system sign-in sheet abstraction. On macOS it's
//! ASWebAuthenticationSession ([`super::sheet_apple`]): a Safari-backed sheet
//! Google allows for native apps (unlike embedded web views, which Google
//! rejects with `disallowed_useragent`). The sheet hands back the full
//! redirect URL on the iOS-type client's custom scheme; no listener or URL-scheme
//! registration is involved.

use std::future::Future;
use std::pin::Pin;

use super::ios_client::REDIRECT_PATH;
use super::loopback::Callback;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetError {
    /// The user closed the sheet (or its "share your login" alert).
    Cancelled,
    /// The sheet couldn't be shown at all (no window, OS refused to start it).
    Unavailable(String),
    Failed(String),
}

pub type SheetFuture =
    Pin<Box<dyn Future<Output = std::result::Result<String, SheetError>> + Send>>;

/// Shows `url` in a system authentication sheet and resolves with the
/// redirect URL once the page navigates to `callback_scheme:`. Dropping the
/// future must dismiss the sheet.
pub trait WebAuthSheet: Send + Sync {
    fn authenticate(&self, url: String, callback_scheme: String) -> SheetFuture;
}

impl From<SheetError> for Error {
    fn from(e: SheetError) -> Self {
        match e {
            SheetError::Cancelled => Error::SignInCancelled,
            SheetError::Unavailable(m) => {
                Error::OAuth(format!("the sign-in sheet couldn't open: {m}"))
            }
            SheetError::Failed(m) => Error::OAuth(format!("the sign-in sheet failed: {m}")),
        }
    }
}

/// Parse `com.googleusercontent.apps.<id>:/oauth2redirect?code=…&state=…`.
pub(crate) fn parse_custom_redirect(redirect: &str, scheme: &str) -> Result<Callback> {
    let bad = || Error::OAuth("Google's sign-in sheet returned an unexpected address".into());
    let url = url::Url::parse(redirect).map_err(|_| bad())?;
    if !url.scheme().eq_ignore_ascii_case(scheme) || url.path() != REDIRECT_PATH {
        return Err(bad());
    }
    let query = url.query_pairs().into_owned().collect();
    Callback::from_query(&query).ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEME: &str = "com.googleusercontent.apps.123-abc";

    #[test]
    fn parses_code_redirect() {
        let cb = parse_custom_redirect(
            "com.googleusercontent.apps.123-abc:/oauth2redirect?state=s1&code=4%2F0Ab-x&scope=email%20openid",
            SCHEME,
        )
        .unwrap();
        assert_eq!(
            cb,
            Callback::Code {
                code: "4/0Ab-x".into(),
                state: "s1".into()
            }
        );
    }

    #[test]
    fn parses_error_redirect() {
        let cb = parse_custom_redirect(
            "com.googleusercontent.apps.123-abc:/oauth2redirect?error=access_denied&state=s1",
            SCHEME,
        )
        .unwrap();
        assert_eq!(
            cb,
            Callback::Denied {
                error: "access_denied".into(),
                state: "s1".into()
            }
        );
    }

    #[test]
    fn rejects_other_schemes_paths_and_empty_redirects() {
        for bad in [
            "com.googleusercontent.apps.999-zzz:/oauth2redirect?code=c&state=s",
            "https://evil.example/oauth2redirect?code=c&state=s",
            "com.googleusercontent.apps.123-abc:/elsewhere?code=c&state=s",
            "com.googleusercontent.apps.123-abc:/oauth2redirect",
            "not a url",
        ] {
            assert!(parse_custom_redirect(bad, SCHEME).is_err(), "{bad}");
        }
    }

    #[test]
    fn sheet_cancel_maps_to_cancelled() {
        assert!(matches!(
            Error::from(SheetError::Cancelled),
            Error::SignInCancelled
        ));
        assert!(matches!(
            Error::from(SheetError::Failed("x".into())),
            Error::OAuth(_)
        ));
    }
}
