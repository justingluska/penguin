//! The only network surface of the avatar service, behind a trait so the
//! resolver can be tested without sockets.
//!
//! The real client ([`HttpNet`]):
//! - HTTPS only, including redirects (at most 3); URLs with an IP-literal
//!   host are refused, and every hostname is resolved through a resolver that
//!   drops loopback, private, link-local and other non-public addresses, so
//!   a sender's DNS can't point our fetches at the local network.
//! - No cookies, no Referer, a generic User-Agent, short timeouts, and a hard
//!   cap on response size (the body is streamed and cut off).
//! - Requests carry only a domain, a fixed path, a BIMI logo URL, a Gravatar
//!   hash or a People API page; never a message id, subject or address.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// A definite answer: 403/404/410, or the response is not usable.
    NotFound,
    /// Offline, timeout, 429, 5xx: says nothing about the resource.
    Transient(String),
    /// 401: the access token was rejected.
    Unauthorized,
    /// 403, with the start of the body (Google says why: missing scope vs.
    /// an API not enabled in the Cloud project).
    Forbidden(String),
}

pub trait Net: Send + Sync {
    /// TXT records for a DNS name (`Ok(vec![])` when there are none).
    fn txt<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<Vec<String>, String>>;
    /// GET `url` (HTTPS), body up to `max` bytes; `bearer` for Google APIs.
    fn get<'a>(
        &'a self,
        url: &'a str,
        max: usize,
        bearer: Option<&'a str>,
    ) -> BoxFut<'a, Result<Vec<u8>, FetchError>>;
}

/// Whether an address is fine to connect to from a user's Mac on behalf of
/// a stranger's DNS record.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (18..20).contains(&o[1]))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x64 && s[1] == 0xff9b)) // NAT64 embeds v4 we can't vet
        }
    }
}

/// An https URL whose host is a DNS name (not an IP literal, not localhost).
pub fn acceptable_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(url.host(), Some(url::Host::Domain(d)) if d.contains('.') && !d.ends_with(".localhost") && !d.ends_with(".local"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|p| p == 443)
}

/// Also used by the one-click unsubscribe POST (src/unsubscribe.rs).
pub(crate) struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 443))
                .await?
                .filter(|a| is_public_ip(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err("no public address".into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

pub struct HttpNet {
    http: reqwest::Client,
}

impl HttpNet {
    pub fn new() -> HttpNet {
        let redirects = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                attempt.stop()
            } else if !acceptable_url(attempt.url()) {
                attempt.error("redirect to a disallowed URL")
            } else {
                attempt.follow()
            }
        });
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(redirects)
            .referer(false)
            .user_agent("Penguin")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .dns_resolver(Arc::new(PublicOnlyResolver))
            .build()
            .expect("reqwest client with rustls builds");
        HttpNet { http }
    }
}

impl Default for HttpNet {
    fn default() -> Self {
        Self::new()
    }
}

impl Net for HttpNet {
    fn txt<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<Vec<String>, String>> {
        let name = name.to_string();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || super::dns::txt(&name))
                .await
                .map_err(|e| e.to_string())?
        })
    }

    fn get<'a>(
        &'a self,
        url: &'a str,
        max: usize,
        bearer: Option<&'a str>,
    ) -> BoxFut<'a, Result<Vec<u8>, FetchError>> {
        Box::pin(async move {
            let parsed = reqwest::Url::parse(url).map_err(|_| FetchError::NotFound)?;
            if !acceptable_url(&parsed) {
                return Err(FetchError::NotFound);
            }
            let mut req = self.http.get(parsed);
            if let Some(token) = bearer {
                req = req.bearer_auth(token);
            }
            let mut resp = req
                .send()
                .await
                .map_err(|e| FetchError::Transient(e.without_url().to_string()))?;
            let status = resp.status().as_u16();
            match status {
                200..=299 => {}
                401 => return Err(FetchError::Unauthorized),
                403 => {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(FetchError::Forbidden(body.chars().take(2000).collect()));
                }
                404 | 410 | 400 => return Err(FetchError::NotFound),
                _ => return Err(FetchError::Transient(format!("HTTP {status}"))),
            }
            if resp.content_length().is_some_and(|n| n > max as u64) {
                return Err(FetchError::NotFound);
            }
            let mut body = Vec::new();
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|e| FetchError::Transient(e.without_url().to_string()))?
            {
                if body.len() + chunk.len() > max {
                    return Err(FetchError::NotFound);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_special_addresses_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
            "224.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.216.34", "142.250.72.14", "2606:4700::6810:84e5"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn only_https_dns_hosts() {
        let ok = |s: &str| acceptable_url(&reqwest::Url::parse(s).unwrap());
        assert!(ok("https://shop.example/logo.svg"));
        assert!(!ok("http://shop.example/logo.svg"));
        assert!(!ok("https://127.0.0.1/x"));
        assert!(!ok("https://[::1]/x"));
        assert!(!ok("https://localhost/x"));
        assert!(!ok("https://printer.local/x"));
        assert!(!ok("https://user:pw@shop.example/x"));
        assert!(!ok("https://shop.example:8443/x"));
        assert!(!ok("file:///etc/passwd"));
    }
}
