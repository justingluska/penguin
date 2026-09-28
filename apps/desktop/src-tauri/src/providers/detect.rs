//! Email address → provider, the way Thunderbird's account setup does it
//! (docs/providers-and-onboarding.md §4):
//!
//! 1. The built-in domain table (table.rs): instant, offline.
//! 2. The domain's MX records through the system resolver: Google Workspace,
//!    Microsoft 365, Yahoo, AOL, iCloud, Fastmail and Proton by host pattern.
//! 3. Autoconfig, fetched in parallel and read in priority order: the
//!    domain's own `autoconfig.<domain>` and `.well-known` files, the ISPDB
//!    entry for the domain, then the ISPDB entry for the MX host's domain.
//! 4. Otherwise unknown (the UI offers the picker and manual IMAP).
//!
//! Only the domain ever leaves the Mac. Every network step has a timeout;
//! when DNS and HTTP all fail the answer is marked `offline` and not cached.
//! Answers are cached per domain for the session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::avatars::net::{BoxFut, FetchError, HttpNet, Net};

use super::autoconfig::{self, Autoconfig};
use super::table::{self, Known, ServerTemplate};
use super::{
    connectable, AuthMethod, DetectedProvider, DetectionSource, ProviderKind, ServerSettings,
    SetupKind,
};

/// The network detection uses, behind a trait so tests run without sockets.
pub trait ProviderNet: Send + Sync {
    /// MX records as (preference, host). `Ok(vec![])` = the domain has none;
    /// `Err` = the resolver failed (offline, timeout, SERVFAIL).
    fn mx<'a>(&'a self, domain: &'a str) -> BoxFut<'a, Result<Vec<(u16, String)>, String>>;
    /// GET an autoconfig URL. `Ok(None)` = a definite "no file here" (404,
    /// too large, not text); `Err` = couldn't tell (offline, timeout, 5xx).
    fn get<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<Option<String>, String>>;
}

/// The real network: libresolv for MX, the avatar service's hardened HTTPS
/// client (public addresses only, https redirects only, size-capped).
pub struct SystemNet {
    http: HttpNet,
}

impl SystemNet {
    pub fn new() -> SystemNet {
        SystemNet {
            http: HttpNet::new(),
        }
    }
}

impl Default for SystemNet {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderNet for SystemNet {
    fn mx<'a>(&'a self, domain: &'a str) -> BoxFut<'a, Result<Vec<(u16, String)>, String>> {
        let domain = domain.to_string();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || crate::avatars::dns::mx(&domain))
                .await
                .map_err(|e| e.to_string())?
        })
    }

    fn get<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            match self.http.get(url, autoconfig::MAX_BYTES, None).await {
                Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
                Err(FetchError::Transient(e)) => Err(e),
                Err(FetchError::NotFound | FetchError::Unauthorized | FetchError::Forbidden(_)) => {
                    Ok(None)
                }
            }
        })
    }
}

/// An address split for detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Trimmed, domain lowercased (and punycoded).
    pub email: String,
    pub local: String,
    pub domain: String,
}

const BAD_ADDRESS: &str = "Type your full email address, like name@example.com.";

/// Accepts `name@domain`, also pasted as `Name <name@domain>` or
/// `mailto:name@domain`. International domains become punycode.
pub fn parse_email(input: &str) -> Result<Address, String> {
    let mut s = input.trim();
    if let (Some(open), Some(close)) = (s.rfind('<'), s.rfind('>')) {
        if open < close {
            s = s[open + 1..close].trim();
        }
    }
    if s.len() > 7 && s[..7].eq_ignore_ascii_case("mailto:") {
        s = &s[7..];
    }
    let (local, domain) = s.rsplit_once('@').ok_or(BAD_ADDRESS)?;
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !"()<>[]:;@\\,\"".contains(c));
    if !local_ok {
        return Err(BAD_ADDRESS.into());
    }
    let domain = domain.trim_end_matches('.').to_lowercase();
    let domain = if domain.is_ascii() {
        domain
    } else {
        match url::Host::parse(&domain) {
            Ok(url::Host::Domain(d)) => d,
            _ => return Err(BAD_ADDRESS.into()),
        }
    };
    let tld = domain.rsplit('.').next().unwrap_or("");
    if !domain.contains('.')
        || !autoconfig::valid_host(&domain)
        || tld.len() < 2
        || tld.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(BAD_ADDRESS.into());
    }
    Ok(Address {
        email: format!("{local}@{domain}"),
        local: local.to_string(),
        domain,
    })
}

/// What detection found for a domain, before an address fills in usernames.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainInfo {
    pub kind: ProviderKind,
    pub setup: SetupKind,
    pub display_name: String,
    pub auth: AuthMethod,
    pub imap: Option<ServerTemplate>,
    pub smtp: Option<ServerTemplate>,
    pub source: DetectionSource,
    pub mx_host: Option<String>,
    pub gateway: Option<String>,
    pub offline: bool,
}

fn from_known(known: Known, source: DetectionSource) -> DomainInfo {
    let p = table::preset(known);
    DomainInfo {
        kind: p.kind,
        setup: p.setup,
        display_name: p.display_name.to_string(),
        auth: p.auth,
        imap: Some(p.imap),
        smtp: Some(p.smtp),
        source,
        mx_host: None,
        gateway: None,
        offline: false,
    }
}

fn unknown(source: DetectionSource) -> DomainInfo {
    DomainInfo {
        kind: ProviderKind::Unknown,
        setup: SetupKind::Unsupported,
        display_name: String::new(),
        auth: AuthMethod::ImapPassword,
        imap: None,
        smtp: None,
        source,
        mx_host: None,
        gateway: None,
        offline: false,
    }
}

fn from_autoconfig(ac: Autoconfig, source: DetectionSource, domain: &str) -> DomainInfo {
    if let Some(known) = ac
        .imap
        .as_ref()
        .and_then(|s| table::imap_host_known(&s.host))
    {
        return from_known(known, source);
    }
    DomainInfo {
        kind: ProviderKind::ImapGeneric,
        setup: SetupKind::Imap,
        display_name: ac.display_name.unwrap_or_else(|| domain.to_string()),
        auth: AuthMethod::ImapPassword,
        imap: ac.imap,
        smtp: ac.smtp,
        source,
        mx_host: None,
        gateway: None,
        offline: false,
    }
}

fn personalize(info: &DomainInfo, addr: &Address) -> DetectedProvider {
    let fill = |s: &Option<ServerTemplate>| {
        s.as_ref().map(|t| ServerSettings {
            host: t.host.clone(),
            port: t.port,
            security: t.security,
            username: autoconfig::fill_username(
                &t.username,
                &addr.email,
                &addr.local,
                &addr.domain,
            ),
        })
    };
    DetectedProvider {
        email: addr.email.clone(),
        domain: addr.domain.clone(),
        kind: info.kind,
        display_name: if info.display_name.is_empty() {
            addr.domain.clone()
        } else {
            info.display_name.clone()
        },
        auth: info.auth,
        setup: info.setup,
        imap: fill(&info.imap),
        smtp: fill(&info.smtp),
        source: info.source,
        mx_host: info.mx_host.clone(),
        gateway: info.gateway.clone(),
        offline: info.offline,
        available: connectable(info.auth) && info.setup != SetupKind::Unsupported,
    }
}

/// One autoconfig URL's outcome.
enum Fetched {
    Found(Autoconfig),
    /// A definite answer that there's nothing usable there.
    Missing,
    /// Couldn't tell: network error or timeout.
    Failed,
}

const CACHE_MAX: usize = 256;

pub struct Detector {
    net: Arc<dyn ProviderNet>,
    cache: Mutex<HashMap<String, DomainInfo>>,
    dns_timeout: Duration,
    http_timeout: Duration,
}

impl Detector {
    pub fn new(net: Arc<dyn ProviderNet>) -> Detector {
        Detector::with_timeouts(net, Duration::from_secs(4), Duration::from_secs(6))
    }

    pub fn with_timeouts(net: Arc<dyn ProviderNet>, dns: Duration, http: Duration) -> Detector {
        Detector {
            net,
            cache: Mutex::new(HashMap::new()),
            dns_timeout: dns,
            http_timeout: http,
        }
    }

    /// Detect the provider for `email`, or with `choose`, give the chosen
    /// provider's settings for it (no network; Imap reuses whatever servers
    /// detection already found for the domain). Errors are user-facing text.
    pub async fn detect(
        &self,
        email: &str,
        choose: Option<SetupKind>,
    ) -> Result<DetectedProvider, String> {
        let addr = parse_email(email)?;
        let info = match choose {
            None => self.domain_info(&addr.domain).await,
            Some(SetupKind::Unsupported) => return Err("Choose a provider.".into()),
            Some(SetupKind::Imap) => self.manual_imap(&addr.domain),
            Some(setup) => {
                let known = table::chosen(setup, &addr.domain).ok_or("Choose a provider.")?;
                from_known(known, DetectionSource::Manual)
            }
        };
        Ok(personalize(&info, &addr))
    }

    fn cached(&self, domain: &str) -> Option<DomainInfo> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(domain)
            .cloned()
    }

    fn manual_imap(&self, domain: &str) -> DomainInfo {
        match self.cached(domain) {
            Some(info) if info.setup == SetupKind::Imap => DomainInfo {
                source: DetectionSource::Manual,
                ..info
            },
            _ => DomainInfo {
                kind: ProviderKind::ImapGeneric,
                setup: SetupKind::Imap,
                display_name: "Other mail server".into(),
                auth: AuthMethod::ImapPassword,
                ..unknown(DetectionSource::Manual)
            },
        }
    }

    pub async fn domain_info(&self, domain: &str) -> DomainInfo {
        if let Some(known) = table::domain_known(domain) {
            return from_known(known, DetectionSource::DomainTable);
        }
        if let Some(hit) = self.cached(domain) {
            return hit;
        }
        let info = self.lookup(domain).await;
        if !info.offline {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if cache.len() >= CACHE_MAX {
                cache.clear();
            }
            cache.insert(domain.to_string(), info.clone());
        }
        info
    }

    async fn lookup(&self, domain: &str) -> DomainInfo {
        let mx = match tokio::time::timeout(self.dns_timeout, self.net.mx(domain)).await {
            Ok(result) => result,
            Err(_) => Err("DNS lookup timed out".into()),
        };
        let mut records = mx.as_ref().cloned().unwrap_or_default();
        records.sort();
        let mx_host = records
            .first()
            .map(|(_, h)| h.trim_end_matches('.').to_string());
        for (_, host) in &records {
            if let Some(known) = table::mx_known(host) {
                return DomainInfo {
                    mx_host: mx_host.clone(),
                    ..from_known(known, DetectionSource::Mx)
                };
            }
        }
        let gateway = records
            .iter()
            .find_map(|(_, h)| table::gateway(h))
            .map(str::to_string);
        // A gateway's own domain says nothing about the mailbox host.
        let mx_base = match (&mx_host, &gateway) {
            (Some(h), None) => Some(table::base_domain(h)),
            _ => None,
        };

        let urls = autoconfig::urls(domain, mx_base.as_deref());
        let slot = |i: usize| {
            let target = urls.get(i).cloned();
            async move {
                match target {
                    Some((source, url)) => Some((source, self.fetch(&url, domain).await)),
                    None => None,
                }
            }
        };
        let (a, b, c, d) = tokio::join!(slot(0), slot(1), slot(2), slot(3));
        let results: Vec<(DetectionSource, Fetched)> = [a, b, c, d].into_iter().flatten().collect();

        let reached_web = results.iter().any(|(_, f)| !matches!(f, Fetched::Failed));
        for (source, fetched) in results {
            if let Fetched::Found(ac) = fetched {
                return DomainInfo {
                    mx_host,
                    gateway,
                    ..from_autoconfig(ac, source, domain)
                };
            }
        }
        DomainInfo {
            mx_host,
            gateway,
            offline: mx.is_err() && !reached_web,
            ..unknown(DetectionSource::Unknown)
        }
    }

    async fn fetch(&self, url: &str, domain: &str) -> Fetched {
        match tokio::time::timeout(self.http_timeout, self.net.get(url)).await {
            Ok(Ok(Some(body))) => match autoconfig::parse(&body, domain) {
                Some(ac) => Fetched::Found(ac),
                None => Fetched::Missing,
            },
            Ok(Ok(None)) => Fetched::Missing,
            Ok(Err(_)) | Err(_) => Fetched::Failed,
        }
    }
}
