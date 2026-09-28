//! Thunderbird autoconfig (`config-v1.1.xml`): where to fetch it, and
//! reading the IMAP and SMTP servers out of it.
//!
//! Format: <https://wiki.mozilla.org/Thunderbird:Autoconfiguration:ConfigFileFormat>.
//! The file is untrusted input from the network: DTDs are refused (so no
//! entity expansion), the caller caps its size, and every host and port is
//! validated before it is shown.

use super::table::ServerTemplate;
use super::{DetectionSource, MailSecurity};

/// The most of a config file Penguin reads.
pub const MAX_BYTES: usize = 64 * 1024;

/// Thunderbird's order: the domain's own files, then the ISPDB for the
/// domain, then the ISPDB for the MX host's domain. Only the domain is sent
/// (Thunderbird adds `?emailaddress=` to the ISP's own URLs; Penguin doesn't).
pub fn urls(domain: &str, mx_base: Option<&str>) -> Vec<(DetectionSource, String)> {
    let mut out = vec![
        (
            DetectionSource::Autoconfig,
            format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
        ),
        (
            DetectionSource::Autoconfig,
            format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
        ),
        (
            DetectionSource::Ispdb,
            format!("https://autoconfig.thunderbird.net/v1.1/{domain}"),
        ),
    ];
    if let Some(base) = mx_base.filter(|b| *b != domain) {
        out.push((
            DetectionSource::Ispdb,
            format!("https://autoconfig.thunderbird.net/v1.1/{base}"),
        ));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Autoconfig {
    pub display_name: Option<String>,
    pub imap: Option<ServerTemplate>,
    pub smtp: Option<ServerTemplate>,
}

/// The first usable IMAP and SMTP servers of the file's first provider.
/// `%EMAILDOMAIN%` in host names is replaced with `domain`; usernames keep
/// their placeholders for the address to fill in. None when the file isn't
/// a config file or names neither server.
pub fn parse(xml: &str, domain: &str) -> Option<Autoconfig> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "clientConfig" {
        return None;
    }
    let provider = root.children().find(|n| n.has_tag_name("emailProvider"))?;
    let text = |node: roxmltree::Node, tag: &str| {
        node.children()
            .find(|c| c.has_tag_name(tag))
            .and_then(|c| c.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    let pick = |element: &str, typ: &str| {
        let servers: Vec<ServerTemplate> = provider
            .children()
            .filter(|n| n.has_tag_name(element) && n.attribute("type") == Some(typ))
            .filter_map(|n| {
                let host = text(n, "hostname")?
                    .replace("%EMAILDOMAIN%", domain)
                    .to_ascii_lowercase();
                if !valid_host(&host) {
                    return None;
                }
                let port: u16 = text(n, "port")?.parse().ok().filter(|p| *p > 0)?;
                let security = match text(n, "socketType")?.to_ascii_uppercase().as_str() {
                    "SSL" | "TLS" => MailSecurity::Tls,
                    "STARTTLS" => MailSecurity::Starttls,
                    "PLAIN" => MailSecurity::Plain,
                    _ => return None,
                };
                let username = text(n, "username").unwrap_or_else(|| "%EMAILADDRESS%".into());
                if username.len() > 320 {
                    return None;
                }
                Some(ServerTemplate {
                    host,
                    port,
                    security,
                    username,
                })
            })
            .collect();
        // The file lists servers in preference order; an encrypted one wins
        // over an earlier plaintext one.
        servers
            .iter()
            .find(|s| s.security != MailSecurity::Plain)
            .or(servers.first())
            .cloned()
    };
    let imap = pick("incomingServer", "imap");
    let smtp = pick("outgoingServer", "smtp");
    if imap.is_none() && smtp.is_none() {
        return None;
    }
    let display_name = text(provider, "displayName").filter(|n| n.chars().count() <= 80);
    Some(Autoconfig {
        display_name,
        imap,
        smtp,
    })
}

/// A DNS host name (letters, digits, hyphens, dots) or an IPv4 address.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Fill a username template for an address.
pub fn fill_username(template: &str, email: &str, local: &str, domain: &str) -> String {
    template
        .replace("%EMAILADDRESS%", email)
        .replace("%EMAILLOCALPART%", local)
        .replace("%EMAILDOMAIN%", domain)
}
