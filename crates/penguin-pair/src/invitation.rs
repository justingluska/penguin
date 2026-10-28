//! The QR code's text:
//!
//! ```text
//! penguin-pair:3?k=<Mac public key>&s=<one-time secret>&p=<port>&a=<host>,<host>…
//! ```
//!
//! `k` and `s` are 32 bytes each, base64url without padding. Hosts are IPv4
//! addresses, bracketed IPv6 addresses (`[fd12::3]`) or a Bonjour name
//! (`Justins-MacBook.local`), in the order the phone should prefer them.
//! The whole thing is about 150 characters, a small QR code.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use zeroize::Zeroizing;

use crate::{PairError, Result, PROTOCOL_VERSION};

const SCHEME: &str = "penguin-pair:";
/// More addresses than any Mac has interfaces worth trying.
pub const MAX_HOSTS: usize = 8;
const MAX_HOST_LEN: usize = 253;

/// What the phone learns from the QR code.
#[derive(Clone, PartialEq, Eq)]
pub struct Invitation {
    /// The Mac's X25519 public key for this offer.
    pub mac_public: [u8; 32],
    /// The one-time secret mixed into the keys.
    pub secret: Zeroizing<[u8; 32]>,
    pub port: u16,
    /// Where the Mac listens, most preferred first.
    pub hosts: Vec<String>,
}

impl fmt::Debug for Invitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invitation")
            .field("mac_public", &URL_SAFE_NO_PAD.encode(self.mac_public))
            .field("secret", &"<redacted>")
            .field("port", &self.port)
            .field("hosts", &self.hosts)
            .finish()
    }
}

impl Invitation {
    /// The QR code's text. It holds the secret: show it, never log it.
    pub fn to_qr_text(&self) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "{SCHEME}{PROTOCOL_VERSION}?k={}&s={}&p={}&a={}",
            URL_SAFE_NO_PAD.encode(self.mac_public),
            URL_SAFE_NO_PAD.encode(*self.secret),
            self.port,
            self.hosts.join(",")
        ))
    }

    /// Parse a scanned code. Anything that isn't exactly a Penguin pairing
    /// code is [`PairError::NotACode`].
    pub fn parse(text: &str) -> Result<Self> {
        let rest = text
            .trim()
            .strip_prefix(SCHEME)
            .ok_or(PairError::NotACode)?;
        let (version, query) = rest.split_once('?').ok_or(PairError::NotACode)?;
        let version: u8 = version.parse().map_err(|_| PairError::NotACode)?;
        match version {
            0 => return Err(PairError::NotACode),
            v if v > PROTOCOL_VERSION => return Err(PairError::UnsupportedVersion),
            // A Mac with an older Penguin: say so rather than half-pair.
            v if v < PROTOCOL_VERSION => return Err(PairError::OlderVersion),
            _ => {}
        }
        let (mut key, mut secret, mut port, mut hosts) = (None, None, None, None);
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or(PairError::NotACode)?;
            match name {
                "k" => key = Some(decode32(value)?),
                "s" => secret = Some(Zeroizing::new(decode32(value)?)),
                "p" => {
                    port = Some(
                        value
                            .parse::<u16>()
                            .ok()
                            .filter(|p| *p != 0)
                            .ok_or(PairError::NotACode)?,
                    )
                }
                "a" => hosts = Some(parse_hosts(value)?),
                // Later versions may add fields; this one ignores them.
                _ => {}
            }
        }
        let hosts = hosts.ok_or(PairError::NotACode)?;
        if hosts.is_empty() {
            return Err(PairError::NotACode);
        }
        Ok(Self {
            mac_public: key.ok_or(PairError::NotACode)?,
            secret: secret.ok_or(PairError::NotACode)?,
            port: port.ok_or(PairError::NotACode)?,
            hosts,
        })
    }
}

fn decode32(value: &str) -> Result<[u8; 32]> {
    let bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| PairError::NotACode)?,
    );
    bytes.as_slice().try_into().map_err(|_| PairError::NotACode)
}

fn parse_hosts(value: &str) -> Result<Vec<String>> {
    let hosts: Vec<String> = value
        .split(',')
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
        .collect();
    if hosts.len() > MAX_HOSTS || !hosts.iter().all(|h| valid_host(h)) {
        return Err(PairError::NotACode);
    }
    Ok(hosts)
}

/// An IPv4 address, a bracketed IPv6 address or a DNS name: nothing a
/// connection API could read as something else.
pub(crate) fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > MAX_HOST_LEN {
        return false;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    host.parse::<std::net::Ipv4Addr>().is_ok()
        || (host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
            && !host.starts_with(['-', '.']))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Invitation {
        Invitation {
            mac_public: [7; 32],
            secret: Zeroizing::new([9; 32]),
            port: 50123,
            hosts: vec![
                "192.168.1.20".into(),
                "[fd12:3456::1]".into(),
                "Ada-MacBook.local".into(),
            ],
        }
    }

    #[test]
    fn roundtrips_through_text() {
        let inv = sample();
        let text = inv.to_qr_text();
        assert!(text.starts_with("penguin-pair:3?k="));
        assert!(text.len() < 200, "{}", text.len());
        assert_eq!(Invitation::parse(&text).unwrap(), inv);
    }

    #[test]
    fn debug_hides_the_secret() {
        let shown = format!("{:?}", sample());
        assert!(shown.contains("<redacted>"));
        assert!(!shown.contains(&URL_SAFE_NO_PAD.encode([9u8; 32])));
    }

    #[test]
    fn refuses_other_codes() {
        for text in [
            "",
            "https://example.com",
            "penguin-pair:3",
            "penguin-pair:x?k=a",
            "penguin-pair:0?k=a",
            // short key
            "penguin-pair:3?k=AAAA&s=AAAA&p=1&a=10.0.0.1",
        ] {
            assert_eq!(Invitation::parse(text), Err(PairError::NotACode), "{text}");
        }
        let good = sample().to_qr_text().to_string();
        let swap = |from: &str, to: &str| Invitation::parse(&good.replace(from, to));
        assert_eq!(swap("p=50123", "p=0"), Err(PairError::NotACode));
        assert_eq!(swap("p=50123", "p=70000"), Err(PairError::NotACode));
        assert_eq!(
            swap("Ada-MacBook.local", "evil host/x"),
            Err(PairError::NotACode)
        );
        assert_eq!(swap("[fd12:3456::1]", "[nope]"), Err(PairError::NotACode));
        assert_eq!(
            Invitation::parse(&good.replace("penguin-pair:3?", "penguin-pair:4?")),
            Err(PairError::UnsupportedVersion)
        );
        // A Mac with an earlier version (no desktop client in 1, Gmail
        // only in 2): update it.
        for older in ["penguin-pair:1?", "penguin-pair:2?"] {
            assert_eq!(
                Invitation::parse(&good.replace("penguin-pair:3?", older)),
                Err(PairError::OlderVersion),
                "{older}"
            );
        }
        let many = format!("{}{}", good, ",10.0.0.1".repeat(MAX_HOSTS));
        assert_eq!(Invitation::parse(&many), Err(PairError::NotACode));
    }
}
