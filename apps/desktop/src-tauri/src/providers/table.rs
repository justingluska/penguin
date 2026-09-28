//! What Penguin knows about providers without asking anyone: consumer mail
//! domains, MX host patterns, the IMAP hosts ISPDB answers can point at, and
//! each provider's servers.
//!
//! The consumer domain lists come from Thunderbird's ISPDB entries
//! (autoconfig.thunderbird.net/v1.1/{gmail.com,outlook.com,yahoo.com,
//! aol.com,icloud.com}, 2026-09-25), keeping only address domains; the
//! Fastmail list was checked against each domain's MX (messagingengine.com)
//! the same day. A domain missing here still resolves through its MX, so
//! these lists are a fast path, not the only path.

use super::{AuthMethod, MailSecurity, ProviderKind, SetupKind};

/// A provider Penguin has presets for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    Gmail,
    GoogleWorkspace,
    OutlookPersonal,
    Microsoft365,
    Yahoo,
    Aol,
    Icloud,
    Fastmail,
    Proton,
}

/// A server with its username as an autoconfig template
/// (`%EMAILADDRESS%`, `%EMAILLOCALPART%`, `%EMAILDOMAIN%`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerTemplate {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: String,
}

pub const ADDRESS: &str = "%EMAILADDRESS%";
pub const LOCAL_PART: &str = "%EMAILLOCALPART%";

fn server(host: &str, port: u16, security: MailSecurity, username: &str) -> ServerTemplate {
    ServerTemplate {
        host: host.into(),
        port,
        security,
        username: username.into(),
    }
}

pub struct Preset {
    pub kind: ProviderKind,
    pub setup: SetupKind,
    pub display_name: &'static str,
    pub auth: AuthMethod,
    pub imap: ServerTemplate,
    pub smtp: ServerTemplate,
}

pub fn preset(known: Known) -> Preset {
    use MailSecurity::{Starttls, Tls};
    let google = |kind, name| Preset {
        kind,
        setup: SetupKind::Google,
        display_name: name,
        auth: AuthMethod::GoogleOAuth,
        imap: server("imap.gmail.com", 993, Tls, ADDRESS),
        smtp: server("smtp.gmail.com", 465, Tls, ADDRESS),
    };
    match known {
        Known::Gmail => google(ProviderKind::Gmail, "Gmail"),
        Known::GoogleWorkspace => google(ProviderKind::GoogleWorkspace, "Google Workspace"),
        Known::OutlookPersonal => Preset {
            kind: ProviderKind::OutlookPersonal,
            setup: SetupKind::Microsoft,
            display_name: "Outlook.com",
            auth: AuthMethod::MicrosoftOAuth,
            imap: server("outlook.office365.com", 993, Tls, ADDRESS),
            smtp: server("smtp-mail.outlook.com", 587, Starttls, ADDRESS),
        },
        Known::Microsoft365 => Preset {
            kind: ProviderKind::Microsoft365,
            setup: SetupKind::Microsoft,
            display_name: "Microsoft 365",
            auth: AuthMethod::MicrosoftOAuth,
            imap: server("outlook.office365.com", 993, Tls, ADDRESS),
            smtp: server("smtp.office365.com", 587, Starttls, ADDRESS),
        },
        Known::Yahoo => Preset {
            kind: ProviderKind::Yahoo,
            setup: SetupKind::Yahoo,
            display_name: "Yahoo Mail",
            auth: AuthMethod::AppPassword,
            imap: server("imap.mail.yahoo.com", 993, Tls, ADDRESS),
            smtp: server("smtp.mail.yahoo.com", 465, Tls, ADDRESS),
        },
        Known::Aol => Preset {
            kind: ProviderKind::Aol,
            setup: SetupKind::Aol,
            display_name: "AOL Mail",
            auth: AuthMethod::AppPassword,
            imap: server("imap.aol.com", 993, Tls, ADDRESS),
            smtp: server("smtp.aol.com", 465, Tls, ADDRESS),
        },
        // Apple: the IMAP username is usually the part before the @ (the
        // full address also works for most accounts); SMTP takes the address.
        Known::Icloud => Preset {
            kind: ProviderKind::Icloud,
            setup: SetupKind::Icloud,
            display_name: "iCloud Mail",
            auth: AuthMethod::AppPassword,
            imap: server("imap.mail.me.com", 993, Tls, LOCAL_PART),
            smtp: server("smtp.mail.me.com", 587, Starttls, ADDRESS),
        },
        Known::Fastmail => Preset {
            kind: ProviderKind::Fastmail,
            setup: SetupKind::Fastmail,
            display_name: "Fastmail",
            auth: AuthMethod::AppPassword,
            imap: server("imap.fastmail.com", 993, Tls, ADDRESS),
            smtp: server("smtp.fastmail.com", 465, Tls, ADDRESS),
        },
        // Proton Mail Bridge's defaults; Bridge shows the password to use.
        Known::Proton => Preset {
            kind: ProviderKind::ImapGeneric,
            setup: SetupKind::Proton,
            display_name: "Proton Mail",
            auth: AuthMethod::ImapPassword,
            imap: server("127.0.0.1", 1143, Starttls, ADDRESS),
            smtp: server("127.0.0.1", 1025, Starttls, ADDRESS),
        },
    }
}

const GOOGLE: &[&str] = &["gmail.com", "googlemail.com"];

const MICROSOFT_PERSONAL: &[&str] = &[
    "hotmail.com",
    "live.com",
    "msn.com",
    "outlook.com",
    "windowslive.com",
    "outlook.at",
    "outlook.be",
    "outlook.cl",
    "outlook.cz",
    "outlook.de",
    "outlook.dk",
    "outlook.es",
    "outlook.fr",
    "outlook.hu",
    "outlook.ie",
    "outlook.in",
    "outlook.it",
    "outlook.jp",
    "outlook.kr",
    "outlook.lv",
    "outlook.my",
    "outlook.ph",
    "outlook.pt",
    "outlook.sa",
    "outlook.sg",
    "outlook.sk",
    "outlook.co.id",
    "outlook.co.il",
    "outlook.co.th",
    "outlook.com.ar",
    "outlook.com.au",
    "outlook.com.br",
    "outlook.com.gr",
    "outlook.com.tr",
    "outlook.com.vn",
    "hotmail.be",
    "hotmail.ca",
    "hotmail.cl",
    "hotmail.cz",
    "hotmail.de",
    "hotmail.dk",
    "hotmail.es",
    "hotmail.fi",
    "hotmail.fr",
    "hotmail.gr",
    "hotmail.hu",
    "hotmail.it",
    "hotmail.lt",
    "hotmail.lv",
    "hotmail.my",
    "hotmail.nl",
    "hotmail.no",
    "hotmail.ph",
    "hotmail.rs",
    "hotmail.se",
    "hotmail.sg",
    "hotmail.sk",
    "hotmail.co.id",
    "hotmail.co.il",
    "hotmail.co.in",
    "hotmail.co.jp",
    "hotmail.co.kr",
    "hotmail.co.th",
    "hotmail.co.uk",
    "hotmail.co.za",
    "hotmail.com.ar",
    "hotmail.com.au",
    "hotmail.com.br",
    "hotmail.com.hk",
    "hotmail.com.tr",
    "hotmail.com.tw",
    "hotmail.com.vn",
    "live.at",
    "live.be",
    "live.ca",
    "live.cl",
    "live.cn",
    "live.de",
    "live.dk",
    "live.fi",
    "live.fr",
    "live.hk",
    "live.ie",
    "live.in",
    "live.it",
    "live.jp",
    "live.nl",
    "live.no",
    "live.ru",
    "live.se",
    "live.co.jp",
    "live.co.kr",
    "live.co.uk",
    "live.co.za",
    "live.com.ar",
    "live.com.au",
    "live.com.mx",
    "live.com.my",
    "live.com.ph",
    "live.com.pt",
    "live.com.sg",
    "livemail.tw",
];

const YAHOO: &[&str] = &[
    "yahoo.com",
    "yahoo.ca",
    "yahoo.de",
    "yahoo.it",
    "yahoo.fr",
    "yahoo.es",
    "yahoo.se",
    "yahoo.co.in",
    "yahoo.co.uk",
    "yahoo.co.nz",
    "yahoo.com.au",
    "yahoo.com.ar",
    "yahoo.com.br",
    "yahoo.com.mx",
    "ymail.com",
    "myyahoo.com",
    "rocketmail.com",
];

const AOL: &[&str] = &[
    "aol.com",
    "aim.com",
    "netscape.net",
    "netscape.com",
    "compuserve.com",
    "cs.com",
    "wmconnect.com",
    "aol.de",
    "aol.it",
    "aol.fr",
    "aol.es",
    "aol.se",
    "aol.co.uk",
    "aol.co.nz",
    "aol.com.au",
    "aol.com.ar",
    "aol.com.br",
    "aol.com.mx",
];

const ICLOUD: &[&str] = &["icloud.com", "me.com", "mac.com"];

const FASTMAIL: &[&str] = &[
    "fastmail.com",
    "fastmail.fm",
    "fastmail.cn",
    "fastmail.co.uk",
    "fastmail.com.au",
    "fastmail.de",
    "fastmail.es",
    "fastmail.in",
    "fastmail.jp",
    "fastmail.net",
    "fastmail.nl",
    "fastmail.org",
    "fastmail.se",
    "fastmail.to",
    "fastmail.tw",
    "fastmail.uk",
    "fastmail.us",
    "sent.com",
    "messagingengine.com",
];

const PROTON: &[&str] = &["proton.me", "protonmail.com", "protonmail.ch", "pm.me"];

/// A consumer mail domain from the built-in table.
pub fn domain_known(domain: &str) -> Option<Known> {
    let d = domain;
    if GOOGLE.contains(&d) {
        Some(Known::Gmail)
    } else if MICROSOFT_PERSONAL.contains(&d) {
        Some(Known::OutlookPersonal)
    } else if YAHOO.contains(&d) {
        Some(Known::Yahoo)
    } else if AOL.contains(&d) {
        Some(Known::Aol)
    } else if ICLOUD.contains(&d) {
        Some(Known::Icloud)
    } else if FASTMAIL.contains(&d) {
        Some(Known::Fastmail)
    } else if PROTON.contains(&d) {
        Some(Known::Proton)
    } else {
        None
    }
}

/// `host` is `suffix` or a subdomain of it (label boundary, so
/// "evilgoogle.com" is not google.com).
pub fn is_under(host: &str, suffix: &str) -> bool {
    host == suffix
        || (host.len() > suffix.len()
            && host.ends_with(suffix)
            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.')
}

/// The provider behind an MX host, for custom domains.
pub fn mx_known(host: &str) -> Option<Known> {
    let h = host.trim_end_matches('.');
    if is_under(h, "google.com") || is_under(h, "googlemail.com") {
        // smtp.google.com (the single record since 2023), aspmx.l.google.com
        // and its alt*, aspmx*.googlemail.com.
        Some(Known::GoogleWorkspace)
    } else if is_under(h, "mail.protection.outlook.com") {
        Some(Known::Microsoft365)
    } else if is_under(h, "olc.protection.outlook.com") {
        Some(Known::OutlookPersonal)
    } else if is_under(h, "mail.gm0.yahoodns.net") {
        // AOL's own MX (also verizon.net).
        Some(Known::Aol)
    } else if is_under(h, "yahoodns.net") {
        Some(Known::Yahoo)
    } else if is_under(h, "mail.icloud.com") {
        Some(Known::Icloud)
    } else if is_under(h, "messagingengine.com") {
        Some(Known::Fastmail)
    } else if is_under(h, "protonmail.ch") {
        Some(Known::Proton)
    } else {
        None
    }
}

/// Filtering services that sit in front of the real mailbox host.
pub fn gateway(host: &str) -> Option<&'static str> {
    const GATEWAYS: &[(&str, &str)] = &[
        ("pphosted.com", "Proofpoint"),
        ("ppe-hosted.com", "Proofpoint"),
        ("mimecast.com", "Mimecast"),
        ("mimecast-offshore.com", "Mimecast"),
        ("barracudanetworks.com", "Barracuda"),
        ("iphmx.com", "Cisco Secure Email"),
        ("messagelabs.com", "Symantec Email Security"),
        ("hydra.sophos.com", "Sophos"),
        ("hes.trendmicro.com", "Trend Micro"),
        ("mailcontrol.com", "Forcepoint"),
        ("mxrecord.io", "Cloudflare Email Security"),
        ("mxrecord.mx", "Cloudflare Email Security"),
    ];
    let h = host.trim_end_matches('.');
    GATEWAYS
        .iter()
        .find(|(suffix, _)| is_under(h, suffix))
        .map(|(_, name)| *name)
}

/// The provider behind an IMAP host named by an autoconfig file, so a custom
/// domain whose ISPDB entry points at Gmail gets the Google flow.
pub fn imap_host_known(host: &str) -> Option<Known> {
    match host {
        "imap.gmail.com" | "imap.googlemail.com" => Some(Known::GoogleWorkspace),
        "outlook.office365.com" => Some(Known::Microsoft365),
        "imap-mail.outlook.com" => Some(Known::OutlookPersonal),
        "imap.mail.yahoo.com" => Some(Known::Yahoo),
        "imap.aol.com" => Some(Known::Aol),
        "imap.mail.me.com" => Some(Known::Icloud),
        "imap.fastmail.com" => Some(Known::Fastmail),
        _ => None,
    }
}

/// The provider a manual "Choose provider" pick means for this domain.
/// None for `Imap` and `Unsupported` (no preset).
pub fn chosen(setup: SetupKind, domain: &str) -> Option<Known> {
    Some(match setup {
        SetupKind::Google if GOOGLE.contains(&domain) => Known::Gmail,
        SetupKind::Google => Known::GoogleWorkspace,
        SetupKind::Microsoft if MICROSOFT_PERSONAL.contains(&domain) => Known::OutlookPersonal,
        SetupKind::Microsoft => Known::Microsoft365,
        SetupKind::Yahoo => Known::Yahoo,
        SetupKind::Aol => Known::Aol,
        SetupKind::Icloud => Known::Icloud,
        SetupKind::Fastmail => Known::Fastmail,
        SetupKind::Proton => Known::Proton,
        SetupKind::Imap | SetupKind::Unsupported => return None,
    })
}

/// The registrable part of a host, for looking an MX's operator up in the
/// ISPDB: `mx1.mail.provider.example` → `provider.example`. Without a public
/// suffix list: two labels, or three under a short second-level label
/// (`co.uk`, `com.au`).
pub fn base_domain(host: &str) -> String {
    let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let n = labels.len();
    let tld = labels[n - 1];
    let second = labels[n - 2];
    let take = if tld.len() == 2
        && matches!(
            second,
            "co" | "com" | "net" | "org" | "ac" | "gov" | "edu" | "ne" | "or"
        ) {
        3
    } else {
        2
    };
    labels[n - take..].join(".")
}
