//! The Add account flow's request, checked without the network, and the
//! account record a successful sign-in stores. The provider step in
//! between (IMAP login, Microsoft sign-in) belongs to the app.

use penguin_core::{Account, AccountProvider, AuthMethod, MailSecurity, ProviderConfig};

use crate::{detect, provider_for, ConnectAccountRequest, ServerSettings};

/// A request that passed [`validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The address as typed, trimmed, domain lowercased.
    pub email: String,
    /// The account id: the address lowercased.
    pub id: String,
}

/// Longest password accepted (app passwords are 16–19 characters).
pub const MAX_PASSWORD: usize = 1024;

fn check_server(what: &str, s: &ServerSettings) -> Result<(), String> {
    let host = s.host.trim();
    if host.is_empty() || host.len() > 253 || host.chars().any(|c| c.is_whitespace()) {
        return Err(format!("Enter the {what} server's host name."));
    }
    if s.port == 0 {
        return Err(format!("Enter the {what} server's port."));
    }
    if s.username.trim().is_empty() {
        return Err(format!("Enter the {what} user name."));
    }
    if s.security == MailSecurity::Plain {
        let local = matches!(
            host.to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "::1" | "[::1]"
        );
        if !local {
            return Err(format!(
                "The {what} server needs TLS or STARTTLS; unencrypted connections are only allowed to this Mac (Proton Mail Bridge)."
            ));
        }
    }
    Ok(())
}

/// Everything about the request that doesn't need the network: a real
/// address, an auth method that belongs to its provider, and the fields
/// that method needs (IMAP: servers and a password; Microsoft: a client
/// id). Err is a user-facing message.
pub fn validate(request: &ConnectAccountRequest) -> Result<Checked, String> {
    let addr = detect::parse_email(&request.email)?;
    match request.auth {
        AuthMethod::GoogleOAuth => {}
        AuthMethod::MicrosoftOAuth => {
            let id = request.client_id.as_deref().unwrap_or("").trim();
            if id.is_empty() {
                return Err("Paste your Microsoft app's Application (client) ID first.".into());
            }
            normalize_client_id(id)?;
        }
        AuthMethod::AppPassword | AuthMethod::ImapPassword => {
            match request.password.as_deref() {
                Some(p) if !p.is_empty() && p.len() <= MAX_PASSWORD => {}
                _ => return Err("Enter the password.".into()),
            }
            let imap = request
                .imap
                .as_ref()
                .ok_or("Enter the incoming (IMAP) server.")?;
            let smtp = request
                .smtp
                .as_ref()
                .ok_or("Enter the outgoing (SMTP) server.")?;
            check_server("IMAP", imap)?;
            check_server("SMTP", smtp)?;
        }
    }
    Ok(Checked {
        id: addr.email.to_lowercase(),
        email: addr.email,
    })
}

/// The account record a successful connect stores: provider from the auth
/// method, `provider_config` from the request (never the password), the
/// given color. The provider step may fill in more (display name, tenant).
pub fn new_account(
    checked: &Checked,
    request: &ConnectAccountRequest,
    color: String,
    now_ms: i64,
) -> Account {
    let provider = provider_for(request.auth);
    let host = Some(request.kind.as_str().to_string());
    let provider_config = match provider {
        AccountProvider::Gmail => ProviderConfig::default(),
        AccountProvider::Imap => ProviderConfig {
            auth: Some(request.auth),
            imap: request.imap.clone().map(trimmed),
            smtp: request.smtp.clone().map(trimmed),
            host,
            ..ProviderConfig::default()
        },
        AccountProvider::Microsoft => ProviderConfig {
            auth: Some(request.auth),
            client_id: request
                .client_id
                .as_deref()
                .and_then(|c| normalize_client_id(c).ok()),
            host,
            ..ProviderConfig::default()
        },
    };
    Account {
        id: checked.id.clone(),
        email: checked.email.clone(),
        display_name: None,
        nickname: None,
        color,
        added_at: now_ms,
        provider,
        capabilities: provider.capabilities(),
        provider_config,
    }
}

fn trimmed(s: ServerSettings) -> ServerSettings {
    ServerSettings {
        host: s.host.trim().to_ascii_lowercase(),
        username: s.username.trim().to_string(),
        ..s
    }
}

fn is_guid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// The first GUID in `input`, lowercased: a Microsoft Entra app's
/// Application (client) ID. Takes the bare ID, `{…}`, or a line copied
/// from the portal ("Application (client) ID : 1b2c…").
pub fn normalize_client_id(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    (0..bytes.len().saturating_sub(35))
        .filter(|&i| input.is_char_boundary(i) && input.is_char_boundary(i + 36))
        .map(|i| &input[i..i + 36])
        .find(|s| is_guid(s))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            "That isn't an Application (client) ID. Copy the ID from the app's Overview page; it looks like 1b2c3d4e-0000-1111-2222-333344445555.".into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderKind;

    fn server(host: &str, port: u16, security: MailSecurity) -> ServerSettings {
        ServerSettings {
            host: host.into(),
            port,
            security,
            username: "sam".into(),
        }
    }

    fn imap_request() -> ConnectAccountRequest {
        ConnectAccountRequest {
            email: " Sam@iCloud.com ".into(),
            kind: ProviderKind::Icloud,
            auth: AuthMethod::AppPassword,
            client_id: None,
            password: Some("abcd-efgh-ijkl-mnop".into()),
            imap: Some(server(" imap.mail.me.com ", 993, MailSecurity::Tls)),
            smtp: Some(server("smtp.mail.me.com", 587, MailSecurity::Starttls)),
        }
    }

    fn ms_request() -> ConnectAccountRequest {
        ConnectAccountRequest {
            email: "sam@outlook.com".into(),
            kind: ProviderKind::OutlookPersonal,
            auth: AuthMethod::MicrosoftOAuth,
            client_id: Some("{1B2C3D4E-0000-1111-2222-333344445555}".into()),
            password: None,
            imap: None,
            smtp: None,
        }
    }

    #[test]
    fn requests_dispatch_to_their_provider() {
        assert_eq!(
            provider_for(AuthMethod::GoogleOAuth),
            AccountProvider::Gmail
        );
        assert_eq!(
            provider_for(AuthMethod::MicrosoftOAuth),
            AccountProvider::Microsoft
        );
        assert_eq!(provider_for(AuthMethod::AppPassword), AccountProvider::Imap);
        assert_eq!(
            provider_for(AuthMethod::ImapPassword),
            AccountProvider::Imap
        );
    }

    #[test]
    fn imap_requests_become_imap_accounts_without_the_password() {
        let req = imap_request();
        let checked = validate(&req).unwrap();
        assert_eq!(checked.email, "Sam@icloud.com");
        assert_eq!(checked.id, "sam@icloud.com");
        let a = new_account(&checked, &req, "#4F7CFF".into(), 5);
        assert_eq!(a.provider, AccountProvider::Imap);
        assert_eq!(a.capabilities, AccountProvider::Imap.capabilities());
        assert_eq!(a.provider_config.auth, Some(AuthMethod::AppPassword));
        assert_eq!(
            a.provider_config.imap.as_ref().unwrap().host,
            "imap.mail.me.com"
        );
        assert_eq!(a.provider_config.host.as_deref(), Some("icloud"));
        assert_eq!(a.added_at, 5);
        assert_eq!(a.color, "#4F7CFF");
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("abcd-efgh"), "{json}");
    }

    #[test]
    fn microsoft_requests_keep_the_normalized_client_id() {
        let req = ms_request();
        let a = new_account(&validate(&req).unwrap(), &req, "#E0685A".into(), 1);
        assert_eq!(a.provider, AccountProvider::Microsoft);
        assert_eq!(
            a.provider_config.client_id.as_deref(),
            Some("1b2c3d4e-0000-1111-2222-333344445555")
        );
        assert_eq!(a.provider_config.host.as_deref(), Some("outlookPersonal"));
        assert_eq!(a.provider_config.imap, None);
    }

    #[test]
    fn host_names_are_the_wire_names() {
        for kind in [
            ProviderKind::Gmail,
            ProviderKind::GoogleWorkspace,
            ProviderKind::OutlookPersonal,
            ProviderKind::Microsoft365,
            ProviderKind::Yahoo,
            ProviderKind::Aol,
            ProviderKind::Icloud,
            ProviderKind::Fastmail,
            ProviderKind::ImapGeneric,
            ProviderKind::Unknown,
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }

    #[test]
    fn bad_requests_say_what_is_missing() {
        let mut r = imap_request();
        r.email = "not an address".into();
        assert!(validate(&r).is_err());
        let mut r = imap_request();
        r.password = None;
        assert_eq!(validate(&r).unwrap_err(), "Enter the password.");
        let mut r = imap_request();
        r.smtp = None;
        assert!(validate(&r).unwrap_err().contains("SMTP"));
        let mut r = imap_request();
        r.imap = Some(server("imap.example.com", 143, MailSecurity::Plain));
        assert!(validate(&r).unwrap_err().contains("TLS or STARTTLS"));
        // Proton Mail Bridge on this Mac may be plain.
        r.imap = Some(server("127.0.0.1", 1143, MailSecurity::Plain));
        assert!(validate(&r).is_ok());
        let mut r = ms_request();
        r.client_id = None;
        assert!(validate(&r).unwrap_err().contains("client) ID"));
        r.client_id = Some("penguin".into());
        assert!(validate(&r).is_err());
    }
}
