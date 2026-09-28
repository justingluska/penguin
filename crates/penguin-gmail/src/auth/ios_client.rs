//! The optional second OAuth client: a Google "iOS" client used by the macOS
//! system sign-in sheet (ASWebAuthenticationSession), the same arrangement
//! Google's own macOS sign-in SDK uses. iOS clients have no secret and
//! redirect to the reversed client id as a custom URL scheme:
//! `com.googleusercontent.apps.<id>:/oauth2redirect`.
//!
//! Stored as `{"client_id": "..."}` in `<config_dir>/google-oauth-ios-client.json`.

use std::path::Path;

use serde::Deserialize;

use super::write_private;
use crate::{Error, Result};

const IOS_CLIENT_FILE_NAME: &str = "google-oauth-ios-client.json";
const ID_SUFFIX: &str = ".apps.googleusercontent.com";
const SCHEME_PREFIX: &str = "com.googleusercontent.apps.";
pub(crate) const REDIRECT_PATH: &str = "/oauth2redirect";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosClientConfig {
    pub client_id: String,
}

impl IosClientConfig {
    /// Accepts the bare client id, the `.plist` Google offers for iOS
    /// clients, or a client JSON (`{"installed": {...}}` or `{"client_id": ...}`).
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        let client_id = if input.starts_with('{') {
            from_json(input)?
        } else if input.starts_with('<') {
            plist_string(input, "CLIENT_ID").ok_or_else(|| {
                Error::NotConfigured("the iOS client .plist has no CLIENT_ID".into())
            })?
        } else {
            input.to_owned()
        };
        Self::from_client_id(&client_id)
    }

    fn from_client_id(client_id: &str) -> Result<Self> {
        // "Smart punctuation" (smart dashes) turns a typed hyphen into an en or em dash.
        let client_id: String = client_id
            .trim()
            .chars()
            .map(|c| {
                if matches!(c, '\u{2010}'..='\u{2015}' | '\u{2212}') {
                    '-'
                } else {
                    c
                }
            })
            .collect::<String>()
            .to_ascii_lowercase();
        let Some(prefix) = client_id.strip_suffix(ID_SUFFIX) else {
            return Err(Error::NotConfigured(format!(
                "\"{client_id}\" isn't a Google OAuth client ID (it should end in {ID_SUFFIX})"
            )));
        };
        // Google's client IDs are "<project number>-<id>": digits, one hyphen, letters and digits.
        let shaped = prefix.split_once('-').is_some_and(|(number, rest)| {
            !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit())
                && !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_alphanumeric())
        });
        if !shaped {
            return Err(Error::NotConfigured(format!(
                "\"{client_id}\" doesn't look like a Google client ID: it should be the project number, a hyphen, then letters and digits (123456789012-abc…{ID_SUFFIX}). Copy it from Google Cloud → Clients rather than typing it"
            )));
        }
        Ok(Self { client_id })
    }

    /// `com.googleusercontent.apps.<id>`: the callback URL scheme.
    pub fn callback_scheme(&self) -> String {
        let prefix = self
            .client_id
            .strip_suffix(ID_SUFFIX)
            .unwrap_or(&self.client_id);
        format!("{SCHEME_PREFIX}{prefix}")
    }

    pub fn redirect_uri(&self) -> String {
        format!("{}:{REDIRECT_PATH}", self.callback_scheme())
    }

    /// `Ok(None)` when no iOS client is configured.
    pub fn load(config_dir: &Path) -> Result<Option<Self>> {
        let path = config_dir.join(IOS_CLIENT_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(json) => Self::parse(&json).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::NotConfigured(format!(
                "can't read {}: {e}",
                path.display()
            ))),
        }
    }

    /// Validate `input` (see [`parse`](Self::parse)) and persist it (0600).
    pub fn save(config_dir: &Path, input: &str) -> Result<Self> {
        let config = Self::parse(input)?;
        let io_err = |e: std::io::Error| Error::Other(format!("can't save iOS OAuth client: {e}"));
        std::fs::create_dir_all(config_dir).map_err(io_err)?;
        let json = serde_json::json!({ "client_id": config.client_id }).to_string();
        let tmp = config_dir.join(format!(".{IOS_CLIENT_FILE_NAME}.tmp"));
        write_private(&tmp, json.as_bytes()).map_err(io_err)?;
        std::fs::rename(&tmp, config_dir.join(IOS_CLIENT_FILE_NAME)).map_err(io_err)?;
        Ok(config)
    }

    /// Forget the iOS client; sign-in falls back to the browser. Accounts
    /// already moved to it keep refreshing (iOS refreshes need only the id,
    /// which is stored with each token).
    pub fn remove(config_dir: &Path) -> Result<()> {
        match std::fs::remove_file(config_dir.join(IOS_CLIENT_FILE_NAME)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Other(format!("can't remove iOS OAuth client: {e}"))),
        }
    }
}

fn from_json(json: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Section {
        #[serde(default)]
        client_id: String,
        #[serde(default)]
        client_secret: Option<String>,
    }
    #[derive(Deserialize)]
    struct File {
        installed: Option<Section>,
        web: Option<Section>,
        #[serde(flatten)]
        top: Section,
    }
    let file: File = serde_json::from_str(json)
        .map_err(|e| Error::NotConfigured(format!("iOS client is not valid JSON ({e})")))?;
    if file.web.is_some() {
        return Err(Error::NotConfigured(
            "this is a \"Web application\" client; the sign-in sheet needs an \"iOS\" client"
                .into(),
        ));
    }
    let section = file.installed.unwrap_or(file.top);
    if section.client_secret.is_some_and(|s| !s.trim().is_empty()) {
        return Err(Error::NotConfigured(
            "this client has a secret, so it's a \"Desktop app\" client; the sign-in sheet needs \
             an \"iOS\" client (Clients → Create client → iOS)"
                .into(),
        ));
    }
    Ok(section.client_id)
}

/// `<key>KEY</key> <string>VALUE</string>` from a flat plist.
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let after_key = plist.split_once(&format!("<key>{key}</key>"))?.1;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    Some(value.split_once("</string>")?.0.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "123456789012-abcdef0123456789.apps.googleusercontent.com";

    #[test]
    fn parses_bare_id_plist_and_json() {
        assert_eq!(
            IosClientConfig::parse(&format!("  {ID}\n"))
                .unwrap()
                .client_id,
            ID
        );
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CLIENT_ID</key>
  <string>{ID}</string>
  <key>REVERSED_CLIENT_ID</key>
  <string>com.googleusercontent.apps.123456789012-abcdef0123456789</string>
  <key>BUNDLE_ID</key><string>co.gluska.penguin</string>
</dict></plist>"#
        );
        assert_eq!(IosClientConfig::parse(&plist).unwrap().client_id, ID);
        let json = format!(r#"{{"installed":{{"client_id":"{ID}","project_id":"p"}}}}"#);
        assert_eq!(IosClientConfig::parse(&json).unwrap().client_id, ID);
        let json = format!(r#"{{"client_id":"{ID}"}}"#);
        assert_eq!(IosClientConfig::parse(&json).unwrap().client_id, ID);
    }

    #[test]
    fn rejects_desktop_web_and_junk() {
        let desktop =
            format!(r#"{{"installed":{{"client_id":"{ID}","client_secret":"GOCSPX-x"}}}}"#);
        assert!(
            matches!(IosClientConfig::parse(&desktop), Err(Error::NotConfigured(m)) if m.contains("Desktop"))
        );
        let web = format!(r#"{{"web":{{"client_id":"{ID}"}}}}"#);
        assert!(IosClientConfig::parse(&web).is_err());
        assert!(IosClientConfig::parse("hello").is_err());
        assert!(IosClientConfig::parse(".apps.googleusercontent.com").is_err());
        assert!(IosClientConfig::parse("a b.apps.googleusercontent.com").is_err());
        assert!(IosClientConfig::parse("<plist></plist>").is_err());
    }

    #[test]
    fn a_missing_hyphen_is_refused_and_smart_dashes_are_fixed() {
        // The project number and the rest run together: Google says invalid_client.
        let joined = "123456789012abcdef0123456789.apps.googleusercontent.com";
        assert!(
            matches!(IosClientConfig::parse(joined), Err(Error::NotConfigured(m)) if m.contains("hyphen"))
        );
        // Smart punctuation typed an en dash.
        let en_dash = "123456789012\u{2013}abcdef0123456789.apps.googleusercontent.com";
        assert_eq!(IosClientConfig::parse(en_dash).unwrap().client_id, ID);
        assert!(IosClientConfig::parse("-abc.apps.googleusercontent.com").is_err());
        assert!(IosClientConfig::parse("12ab-cd.apps.googleusercontent.com").is_err());
    }

    #[test]
    fn derives_reversed_scheme_and_redirect() {
        let c = IosClientConfig::parse(ID).unwrap();
        assert_eq!(
            c.callback_scheme(),
            "com.googleusercontent.apps.123456789012-abcdef0123456789"
        );
        assert_eq!(
            c.redirect_uri(),
            "com.googleusercontent.apps.123456789012-abcdef0123456789:/oauth2redirect"
        );
    }

    #[test]
    fn save_load_remove() {
        let dir = std::env::temp_dir().join(format!(
            "penguin-ios-client-test-{}",
            super::super::pkce::random_urlsafe(8).unwrap()
        ));
        assert!(IosClientConfig::load(&dir).unwrap().is_none());
        IosClientConfig::save(&dir, ID).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(IOS_CLIENT_FILE_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(IosClientConfig::load(&dir).unwrap().unwrap().client_id, ID);
        assert!(IosClientConfig::save(&dir, "nope").is_err());
        assert_eq!(IosClientConfig::load(&dir).unwrap().unwrap().client_id, ID);
        IosClientConfig::remove(&dir).unwrap();
        IosClientConfig::remove(&dir).unwrap();
        assert!(IosClientConfig::load(&dir).unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
