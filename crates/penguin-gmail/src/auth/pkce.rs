//! PKCE (RFC 7636, S256) and random URL-safe tokens for `state`.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// `n_bytes` of OS randomness, base64url-encoded without padding.
pub(crate) fn random_urlsafe(n_bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; n_bytes];
    getrandom::fill(&mut buf).map_err(|e| Error::Other(format!("system RNG unavailable: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// `BASE64URL(SHA256(verifier))` — the S256 code challenge.
pub(crate) fn challenge_s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// 32 random bytes → a 43-char verifier (RFC 7636 requires 43..=128).
    pub fn generate() -> Result<Self> {
        Ok(Self::from_verifier(random_urlsafe(32)?))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = challenge_s256(&verifier);
        Self {
            verifier,
            challenge,
        }
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// Compare secrets without an early exit on the first differing byte.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s256_matches_rfc7636_appendix_b() {
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_verifier_is_valid_length_and_charset() {
        let pkce = Pkce::generate().unwrap();
        assert_eq!(pkce.verifier.len(), 43);
        assert!(pkce
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(pkce.challenge, challenge_s256(&pkce.verifier));
        assert_ne!(pkce.verifier, Pkce::generate().unwrap().verifier);
    }

    #[test]
    fn debug_redacts_verifier() {
        let pkce = Pkce::from_verifier("super-secret-verifier".into());
        assert!(!format!("{pkce:?}").contains("super-secret-verifier"));
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
