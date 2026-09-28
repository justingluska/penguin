//! Adding an IMAP account: refuse what can't work (Google Workspace with an
//! app password), prove the password on both servers (IMAP login, SMTP
//! AUTH, nothing sent), then save it in the Keychain.
//!
//! Errors are what the Add account screen shows: a refused password is
//! `InvalidInput` with the provider's own advice ("use an app password")
//! and the server's words; an unreachable server is `Network`.

use std::sync::Arc;

use penguin_core::Account;
use penguin_provider::credentials::{PasswordCredential, SecretVault};

use crate::account::Config;
use crate::session::Session;
use crate::smtp::{Smtp, SmtpError};
use crate::{Error, Result};

const GOOGLE_DOMAINS: &[&str] = &["gmail.com", "googlemail.com"];

/// Why a Google Workspace address can't use Gmail quick setup.
pub const WORKSPACE_REFUSAL: &str = "Google Workspace accounts can't use app passwords: Google turned off password sign-in for IMAP and SMTP in Workspace in March 2025. Add this account with Sign in with Google (your own Google Cloud client) instead.";

fn domain(email: &str) -> String {
    email
        .rsplit_once('@')
        .map(|(_, d)| d.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Refuse settings that can't work before touching the network.
pub fn precheck(cfg: &Config) -> Result<()> {
    let google_servers = cfg.imap.host.eq_ignore_ascii_case("imap.gmail.com")
        || cfg.imap.host.eq_ignore_ascii_case("imap.googlemail.com")
        || cfg.smtp.host.eq_ignore_ascii_case("smtp.gmail.com");
    let workspace = cfg.host.as_deref() == Some("googleWorkspace")
        || (google_servers && !GOOGLE_DOMAINS.contains(&domain(&cfg.email).as_str()));
    if workspace {
        return Err(Error::InvalidInput(WORKSPACE_REFUSAL.into()));
    }
    Ok(())
}

/// Who runs the server, for messages ("Yahoo didn't accept …").
fn provider_name(cfg: &Config) -> &'static str {
    match cfg.host.as_deref() {
        Some("gmail") => "Google",
        Some("yahoo") => "Yahoo",
        Some("aol") => "AOL",
        Some("icloud") => "iCloud",
        Some("fastmail") => "Fastmail",
        _ if cfg.is_gmail_host() => "Google",
        _ if crate::net::is_loopback(&cfg.imap.host) => "Proton Mail Bridge",
        _ => "The mail server",
    }
}

/// The provider's advice when it refuses the password.
pub fn password_hint(cfg: &Config) -> String {
    let name = provider_name(cfg);
    let tail = match name {
        "Google" => "Use an app password from https://myaccount.google.com/apppasswords (it needs 2-Step Verification), not your Google password.",
        "Yahoo" => "Use an app password (Yahoo Account Security → Generate app password), not your Yahoo password.",
        "AOL" => "Use an app password (AOL Account Security → Generate app password), not your AOL password.",
        "iCloud" => "Use an app-specific password from https://account.apple.com (Sign-In and Security → App-Specific Passwords), not your Apple Account password.",
        "Fastmail" => "Use an app password (Fastmail Settings → Privacy & Security → App passwords), not your Fastmail password.",
        "Proton Mail Bridge" => "Use the password Proton Mail Bridge shows for this account, not your Proton password.",
        _ => "Check the user name and password.",
    };
    if name == "The mail server" {
        format!("The mail server didn't accept that user name and password. {tail}")
    } else {
        format!("{name} didn't accept that password. {tail}")
    }
}

/// Log in to IMAP and authenticate to SMTP with `password` (nothing sent).
/// Returns whether the server speaks Gmail's extensions.
pub async fn verify(cfg: &Config, password: &str) -> Result<bool> {
    precheck(cfg)?;
    let mut s = Session::connect(&cfg.imap).await?;
    match s.login(&cfg.imap.username, password).await {
        Ok(()) => {}
        Err(Error::NeedsReauth(server)) => {
            return Err(Error::InvalidInput(format!(
                "{} (The server said: {})",
                password_hint(cfg),
                server.trim()
            )))
        }
        Err(e) => return Err(e),
    }
    let gmail = s.caps.gmail();
    // Listing proves the session works beyond login.
    s.list().await?;
    s.logout().await;
    let mut smtp = Smtp::connect(&cfg.smtp).await.map_err(Error::from)?;
    match smtp.login(&cfg.smtp.username, password).await {
        Ok(()) => {}
        Err(SmtpError::Auth(server)) => {
            return Err(Error::InvalidInput(format!(
                "{} accepted the password for incoming mail, but its outgoing (SMTP) server refused it: {}. Check the outgoing server's user name.",
                provider_name(cfg),
                server
            )))
        }
        Err(SmtpError::Other(e)) => return Err(e),
    }
    smtp.quit().await;
    Ok(gmail)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Save the account's password (after [`verify`]).
pub async fn save_password(
    vault: Arc<SecretVault<PasswordCredential>>,
    account_id: &str,
    password: &str,
) -> Result<()> {
    let (id, cred) = (
        account_id.to_string(),
        PasswordCredential {
            password: password.to_string(),
            smtp_password: None,
            saved_at: now_secs(),
        },
    );
    tokio::task::spawn_blocking(move || vault.save(&id, &cred))
        .await
        .map_err(|e| Error::Other(format!("keychain task failed: {e}")))?
}

/// `connect_account` for IMAP: check, verify, save (Keychain of the app).
pub async fn connect(account: &Account, password: &str) -> Result<()> {
    let cfg = Config::from_account(account)?;
    verify(&cfg, password).await?;
    save_password(crate::backend::keychain_vault(), &account.id, password).await
}

/// `reconnect_account` for IMAP: the saved password must work again (after
/// the user fixed it on the provider's side, or a server outage).
pub async fn reconnect(account: &Account) -> Result<()> {
    let cfg = Config::from_account(account)?;
    let vault = crate::backend::keychain_vault();
    let id = account.id.clone();
    let saved = tokio::task::spawn_blocking(move || vault.load(&id))
        .await
        .map_err(|e| Error::Other(format!("keychain task failed: {e}")))??;
    let Some(cred) = saved else {
        return Err(Error::InvalidInput(
            "Penguin has no saved password for this account. Remove it and add it again with a new app password.".into(),
        ));
    };
    match verify(&cfg, &cred.password).await {
        Ok(_) => Ok(()),
        Err(Error::InvalidInput(msg)) => Err(Error::InvalidInput(format!(
            "{msg} To use a new password, remove the account and add it again."
        ))),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use penguin_core::{MailSecurity, ServerSettings};

    use super::*;

    fn cfg(email: &str, host: Option<&str>, imap: &str) -> Config {
        let server = |h: &str| ServerSettings {
            host: h.into(),
            port: 993,
            security: MailSecurity::Tls,
            username: email.into(),
        };
        Config {
            account_id: email.to_lowercase(),
            email: email.into(),
            imap: server(imap),
            smtp: server(&imap.replace("imap", "smtp")),
            host: host.map(str::to_string),
        }
    }

    #[test]
    fn workspace_is_refused_before_any_connection() {
        let e = precheck(&cfg("sam@harbor.example", Some("gmail"), "imap.gmail.com")).unwrap_err();
        assert!(
            matches!(e, Error::InvalidInput(ref m) if m.contains("Workspace")),
            "{e}"
        );
        let e = precheck(&cfg(
            "sam@harbor.example",
            Some("googleWorkspace"),
            "imap.gmail.com",
        ))
        .unwrap_err();
        assert!(matches!(e, Error::InvalidInput(_)));
        assert!(precheck(&cfg("sam@gmail.com", Some("gmail"), "imap.gmail.com")).is_ok());
        assert!(precheck(&cfg("sam@icloud.com", Some("icloud"), "imap.mail.me.com")).is_ok());
    }

    #[test]
    fn hints_name_the_provider_and_the_fix() {
        let h = password_hint(&cfg("sam@yahoo.com", Some("yahoo"), "imap.mail.yahoo.com"));
        assert!(
            h.starts_with("Yahoo didn't accept") && h.contains("app password"),
            "{h}"
        );
        let h = password_hint(&cfg("sam@gmail.com", Some("gmail"), "imap.gmail.com"));
        assert!(h.contains("myaccount.google.com/apppasswords"), "{h}");
        let h = password_hint(&cfg("sam@icloud.com", Some("icloud"), "imap.mail.me.com"));
        assert!(h.contains("App-Specific"), "{h}");
        let h = password_hint(&cfg("sam@proton.me", Some("imapGeneric"), "127.0.0.1"));
        assert!(h.starts_with("Proton Mail Bridge"), "{h}");
        let h = password_hint(&cfg(
            "sam@harbor.example",
            Some("imapGeneric"),
            "mail.harbor.example",
        ));
        assert!(h.starts_with("The mail server didn't accept"), "{h}");
    }
}
