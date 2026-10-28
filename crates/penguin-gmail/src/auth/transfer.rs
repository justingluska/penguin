//! Moving refresh tokens between devices (Pair iPhone, docs/PAIRING.md):
//! the Mac exports chosen accounts' vault entries as v3 vault JSON, the
//! phone imports the ones it can refresh.
//!
//! A refresh token only refreshes with the client that minted it. The
//! phone has its build's "iOS" client (no secret), and the Mac sends its
//! "Desktop app" client (id and secret) along with any browser sign-ins,
//! which the phone keeps in its Keychain ([`super::paired_clients`]). So a
//! token moves when it's from the phone's iOS client or from the desktop
//! client that came with it. Both devices then hold the same grant:
//! revoking it on either signs both out, which is why
//! [`AuthManager::forget`] exists.

use std::collections::BTreeMap;

use zeroize::Zeroizing;

use super::keychain::{decode_vault, encode_vault, TokenClient};
use super::{normalize_email, AuthManager, OAuthClientConfig, GMAIL_MODIFY_SCOPE};
use crate::{Error, Result};

/// Which OAuth client minted an account's refresh token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenOrigin {
    /// A "Desktop app" client (browser sign-in on the Mac). `client_id` is
    /// None for tokens stored before ids were recorded: those belong to the
    /// configured desktop client.
    Desktop { client_id: Option<String> },
    /// An "iOS" client (the sign-in sheet on the Mac, or the phone).
    Ios { client_id: String },
}

/// What [`AuthManager::import_vault`] did with each account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Stored; these accounts can sync now.
    pub imported: Vec<String>,
    /// Already had a token this device can refresh, granting everything
    /// the incoming one does; left alone.
    pub kept: Vec<String>,
    /// Not stored, with why (worded for the user).
    pub refused: Vec<(String, String)>,
}

/// Whether `incoming` grants every scope `have` does and at least one more.
/// An empty list (a token stored before scopes were recorded) is unknown
/// and never counts as more.
pub fn grants_more(incoming: &[String], have: &[String]) -> bool {
    !incoming.is_empty()
        && have.iter().all(|s| incoming.contains(s))
        && incoming.iter().any(|s| !have.contains(s))
}

/// Longest client id or secret taken from another device.
const MAX_CLIENT_FIELD: usize = 200;

impl AuthManager {
    /// The client behind an account's stored token, None when it has none.
    /// May read the Keychain (and so block) the first time in a process.
    pub fn token_origin(&self, email: &str) -> Result<Option<TokenOrigin>> {
        let email = normalize_email(email);
        Ok(self.inner.store.load(&email)?.map(|c| match c.client {
            TokenClient::Desktop { client_id } => TokenOrigin::Desktop { client_id },
            TokenClient::Ios { client_id } => TokenOrigin::Ios { client_id },
        }))
    }

    /// The stored tokens of `emails` as v3 vault JSON, for another device.
    /// Accounts without a token are left out. A desktop-client token stored
    /// without its client id gets the configured desktop client's id (the
    /// client it belongs to), so the other device can match it with the
    /// [`desktop_client`](Self::desktop_client) sent alongside. The result
    /// holds refresh tokens: send it only over an authenticated, encrypted
    /// channel.
    pub fn export_vault(&self, emails: &[String]) -> Result<Zeroizing<String>> {
        let desktop_id = self.desktop_client().map(|c| c.client_id);
        let mut accounts = BTreeMap::new();
        for email in emails {
            let email = normalize_email(email);
            if let Some(mut credential) = self.inner.store.load(&email)? {
                if let (
                    TokenClient::Desktop {
                        client_id: id @ None,
                    },
                    Some(desktop),
                ) = (&mut credential.client, &desktop_id)
                {
                    *id = Some(desktop.clone());
                }
                accounts.insert(email, credential);
            }
        }
        let json = encode_vault(&accounts)?;
        tracing::info!(accounts = accounts.len(), "exported tokens for pairing");
        Ok(json)
    }

    /// Store tokens from another device's [`export_vault`](Self::export_vault).
    /// Taken: tokens minted by this manager's iOS client, and tokens minted
    /// by a desktop client this device holds — `desktop` (the Mac's, sent
    /// in the same transfer) or one stored by an earlier pairing. `desktop`
    /// is kept in the Keychain (before the tokens) when a token needs it.
    /// An account that already has a token this device can refresh keeps
    /// it, unless the incoming token grants more (the calendar, or RSVP,
    /// connected on the Mac since): then the incoming one replaces it, so
    /// pairing again carries a grant made on the Mac. One Keychain write
    /// for all the tokens.
    pub fn import_vault(
        &self,
        json: &str,
        desktop: Option<&OAuthClientConfig>,
    ) -> Result<ImportReport> {
        let ios = self.ios_client();
        let desktop = match desktop {
            Some(d) if !valid_client(d) => {
                tracing::warn!("ignoring a malformed desktop client from pairing");
                None
            }
            other => other,
        };
        if ios.is_none() && desktop.is_none() {
            return Err(Error::NotConfigured(
                "this build has no Google sign-in client, so it can't use tokens from another device"
                    .into(),
            ));
        }
        let incoming = decode_vault(json)?;
        let mut report = ImportReport::default();
        let mut accepted = Vec::new();
        let mut needs_desktop = false;
        for (email, credential) in incoming {
            let email = normalize_email(&email);
            if !email.contains('@') || email.len() > 320 || credential.refresh_token.is_empty() {
                report
                    .refused
                    .push((email, "the entry is malformed".into()));
                continue;
            }
            let (usable, uses_sent_desktop) = match &credential.client {
                TokenClient::Ios { client_id } => (
                    ios.as_ref()
                        .is_some_and(|i| client_id.eq_ignore_ascii_case(&i.client_id)),
                    false,
                ),
                TokenClient::Desktop {
                    client_id: Some(id),
                } => {
                    let sent = desktop.is_some_and(|d| d.client_id == *id);
                    (sent || self.holds_client(&credential.client), sent)
                }
                // The Mac stamps the id on export; without it there's no
                // telling which client this is.
                TokenClient::Desktop { client_id: None } => (false, false),
            };
            if !usable {
                report.refused.push((
                    email,
                    "it was signed in with a Google client this iPhone doesn't have; sign in here instead"
                        .into(),
                ));
                continue;
            }
            if !credential.scopes.is_empty()
                && !credential.scopes.iter().any(|s| s == GMAIL_MODIFY_SCOPE)
            {
                report
                    .refused
                    .push((email, "its sign-in didn't grant Gmail access".into()));
                continue;
            }
            if self.can_refresh(&email)? {
                let have = self
                    .inner
                    .store
                    .load(&email)?
                    .map(|c| c.scopes)
                    .unwrap_or_default();
                if !grants_more(&credential.scopes, &have) {
                    report.kept.push(email);
                    continue;
                }
                tracing::info!(account = %email, "pairing brings a token that grants more; replacing");
            }
            needs_desktop |= uses_sent_desktop;
            accepted.push((email, credential));
        }
        if needs_desktop {
            // The client first: a token stored without it couldn't refresh.
            self.inner
                .paired
                .add(desktop.expect("needs_desktop implies a desktop client"))?;
        }
        if !accepted.is_empty() {
            self.inner.store.save_all(&accepted)?;
            let mut tokens = self.inner.tokens.lock().unwrap_or_else(|p| p.into_inner());
            for (email, _) in &accepted {
                // Any cached access token belonged to the token replaced.
                tokens.remove(email);
            }
        }
        report.imported = accepted.into_iter().map(|(email, _)| email).collect();
        tracing::info!(
            imported = report.imported.len(),
            kept = report.kept.len(),
            refused = report.refused.len(),
            desktop_client = needs_desktop,
            "imported tokens from pairing"
        );
        Ok(report)
    }

    /// Delete an account's token from this device without revoking it at
    /// Google: for a token another device shares (Pair iPhone), where a
    /// revoke would sign that device out too.
    pub async fn forget(&self, email: &str) -> Result<()> {
        let email = normalize_email(email);
        let slot = self.slot(&email);
        let mut cached = slot.lock().await;
        *cached = None;
        let key = email.clone();
        self.with_store(move |store| store.delete(&key)).await?;
        drop(cached);
        self.inner
            .tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&email);
        tracing::info!(account = %email, "forgot the token (not revoked: another device shares it)");
        Ok(())
    }
}

/// A client id and secret fit to store: printable ASCII, not absurdly long.
fn valid_client(client: &OAuthClientConfig) -> bool {
    let ok = |s: &str| {
        !s.is_empty() && s.len() <= MAX_CLIENT_FIELD && s.bytes().all(|b| b.is_ascii_graphic())
    };
    ok(&client.client_id) && client.client_secret.as_deref().is_none_or(ok)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::SystemTime;

    use super::super::keychain::memory::{MemoryBackend, MemoryStore};
    use super::super::keychain::{CredentialStore, StoredCredential};
    use super::super::paired_clients::{PairedClients, PAIRED_CLIENTS_ACCOUNT};
    use super::super::{Endpoints, IosClientConfig};
    use super::*;

    const IOS: &str = "123456789012-phone.apps.googleusercontent.com";
    const DESK: &str = "123456789012-desk.apps.googleusercontent.com";

    fn desk_client() -> OAuthClientConfig {
        OAuthClientConfig {
            client_id: DESK.into(),
            client_secret: Some("GOCSPX-fictional".into()),
        }
    }

    fn with_config(
        config: OAuthClientConfig,
        store: Arc<MemoryStore>,
        ios: Option<&str>,
    ) -> AuthManager {
        let auth = AuthManager::with_parts(
            config,
            reqwest::Client::new(),
            Endpoints::default(),
            store,
            Arc::new(SystemTime::now),
        );
        auth.set_ios_client(ios.map(|id| IosClientConfig::parse(id).unwrap()));
        auth
    }

    /// The Mac: a desktop client and maybe an iOS one.
    fn manager(store: Arc<MemoryStore>, ios: Option<&str>) -> AuthManager {
        with_config(desk_client(), store, ios)
    }

    /// The phone: no desktop client of its own, its paired clients in `paired`.
    fn phone(store: Arc<MemoryStore>, paired: &Arc<MemoryBackend>) -> AuthManager {
        with_config(
            OAuthClientConfig {
                client_id: String::new(),
                client_secret: None,
            },
            store,
            Some(IOS),
        )
        .with_paired_clients(Arc::new(PairedClients::new(Box::new(paired.clone()))))
    }

    fn cred(rt: &str, client: TokenClient) -> StoredCredential {
        StoredCredential::new(rt.into(), vec![GMAIL_MODIFY_SCOPE.into()], 1).with_client(client)
    }

    fn ios(id: &str) -> TokenClient {
        TokenClient::Ios {
            client_id: id.into(),
        }
    }

    fn desk(id: Option<&str>) -> TokenClient {
        TokenClient::Desktop {
            client_id: id.map(str::to_owned),
        }
    }

    fn vault(entries: &[(&str, StoredCredential)]) -> Zeroizing<String> {
        let accounts: BTreeMap<String, StoredCredential> = entries
            .iter()
            .map(|(e, c)| (e.to_string(), c.clone()))
            .collect();
        encode_vault(&accounts).unwrap()
    }

    #[test]
    fn exports_the_chosen_accounts_in_vault_format() {
        let mac_store = Arc::new(MemoryStore::default());
        mac_store
            .save("ada@example.com", &cred("rt-a", ios(IOS)))
            .unwrap();
        mac_store
            .save("grace@example.com", &cred("rt-g", desk(None)))
            .unwrap();
        let mac = manager(mac_store, Some(IOS));
        assert_eq!(
            mac.token_origin("Ada@Example.com").unwrap(),
            Some(TokenOrigin::Ios {
                client_id: IOS.into()
            })
        );
        assert_eq!(
            mac.token_origin("grace@example.com").unwrap(),
            Some(TokenOrigin::Desktop { client_id: None })
        );
        assert_eq!(mac.token_origin("nobody@example.com").unwrap(), None);

        let json = mac
            .export_vault(&[
                "ADA@example.com".into(),
                "grace@example.com".into(),
                "nobody@example.com".into(),
            ])
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 3);
        assert_eq!(
            value["accounts"]["ada@example.com"]["refresh_token"],
            "rt-a"
        );
        // A browser sign-in stored without its client id leaves with the
        // Mac's desktop client id, which the phone matches.
        assert_eq!(
            value["accounts"]["grace@example.com"]["client"],
            serde_json::json!({"type": "desktop", "client_id": DESK})
        );
        assert_eq!(value["accounts"].as_object().unwrap().len(), 2);
        assert_eq!(mac.desktop_client().unwrap().client_id, DESK);
    }

    #[test]
    fn imports_only_tokens_this_client_can_refresh() {
        let json = vault(&[
            ("ada@example.com", cred("rt-a", ios(IOS))),
            ("grace@example.com", cred("rt-g", desk(Some(DESK)))),
            (
                "linus@example.com",
                cred("rt-l", ios("123456789012-other.apps.googleusercontent.com")),
            ),
            ("kept@example.com", cred("rt-new", ios(IOS))),
            (
                "noscope@example.com",
                StoredCredential::new("rt-n".into(), vec!["openid".into()], 1)
                    .with_client(ios(IOS)),
            ),
        ]);

        let phone_store = Arc::new(MemoryStore::default());
        phone_store
            .save("kept@example.com", &cred("rt-old", ios(IOS)))
            .unwrap();
        let paired = Arc::new(MemoryBackend::default());
        let phone = phone(phone_store.clone(), &paired);
        // No desktop client sent (a version 1 Mac): browser sign-ins stay.
        let report = phone.import_vault(&json, None).unwrap();
        assert_eq!(report.imported, ["ada@example.com"]);
        assert_eq!(report.kept, ["kept@example.com"]);
        let refused: Vec<&str> = report.refused.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(
            refused,
            [
                "grace@example.com",
                "linus@example.com",
                "noscope@example.com"
            ]
        );
        assert!(report.refused[0].1.contains("doesn't have"));
        assert_eq!(
            phone_store
                .load("ada@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-a"
        );
        assert_eq!(
            phone_store
                .load("kept@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-old"
        );
        assert!(phone_store.load("grace@example.com").unwrap().is_none());
        assert!(paired.sets.lock().unwrap().is_empty());
    }

    #[test]
    fn imports_browser_sign_ins_with_the_macs_desktop_client() {
        let json = vault(&[
            ("grace@example.com", cred("rt-g", desk(Some(DESK)))),
            (
                "other@example.com",
                cred(
                    "rt-o",
                    desk(Some("123456789012-elsewhere.apps.googleusercontent.com")),
                ),
            ),
            ("legacy@example.com", cred("rt-x", desk(None))),
        ]);
        let phone_store = Arc::new(MemoryStore::default());
        let paired = Arc::new(MemoryBackend::default());
        let phone = phone(phone_store.clone(), &paired);
        assert!(!phone.can_refresh("grace@example.com").unwrap());

        let report = phone.import_vault(&json, Some(&desk_client())).unwrap();
        assert_eq!(report.imported, ["grace@example.com"]);
        let refused: Vec<&str> = report.refused.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(refused, ["legacy@example.com", "other@example.com"]);
        assert_eq!(
            phone_store
                .load("grace@example.com")
                .unwrap()
                .unwrap()
                .client,
            desk(Some(DESK))
        );
        // The client went into its own Keychain item, before the token.
        assert_eq!(*paired.sets.lock().unwrap(), [PAIRED_CLIENTS_ACCOUNT]);
        assert!(phone.can_refresh("grace@example.com").unwrap());

        // Pairing again: the stored client is enough, and the account that
        // works now is kept rather than replaced.
        let again = vault(&[
            ("grace@example.com", cred("rt-g2", desk(Some(DESK)))),
            ("hopper@example.com", cred("rt-h", desk(Some(DESK)))),
        ]);
        let report = phone.import_vault(&again, None).unwrap();
        assert_eq!(report.imported, ["hopper@example.com"]);
        assert_eq!(report.kept, ["grace@example.com"]);
        assert_eq!(
            phone_store
                .load("grace@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-g"
        );
    }

    #[test]
    fn a_working_token_is_replaced_by_one_that_grants_more() {
        // The phone's token has Gmail only; the Mac connected the calendar
        // since. Pairing again brings the calendar grant over; a token that
        // grants the same or less (or isn't known) leaves the phone's alone.
        let cal = "https://www.googleapis.com/auth/calendar.readonly";
        let with_cal = |rt: &str| {
            StoredCredential::new(rt.into(), vec![GMAIL_MODIFY_SCOPE.into(), cal.into()], 1)
                .with_client(desk(Some(DESK)))
        };
        let phone_store = Arc::new(MemoryStore::default());
        phone_store
            .save("ada@example.com", &cred("rt-phone", ios(IOS)))
            .unwrap();
        phone_store
            .save(
                "grace@example.com",
                &with_cal("rt-g-phone").with_client(ios(IOS)),
            )
            .unwrap();
        let paired = Arc::new(MemoryBackend::default());
        let phone = phone(phone_store.clone(), &paired);
        let json = vault(&[
            ("ada@example.com", with_cal("rt-mac")),
            ("grace@example.com", cred("rt-g-mac", desk(Some(DESK)))),
        ]);
        let report = phone.import_vault(&json, Some(&desk_client())).unwrap();
        assert_eq!(report.imported, ["ada@example.com"]);
        assert_eq!(report.kept, ["grace@example.com"]);
        let ada = phone_store.load("ada@example.com").unwrap().unwrap();
        assert_eq!(ada.refresh_token, "rt-mac");
        assert!(ada.scopes.iter().any(|s| s == cal));
        assert_eq!(
            phone_store.load("grace@example.com").unwrap().unwrap().refresh_token,
            "rt-g-phone"
        );
        assert!(grants_more(&[GMAIL_MODIFY_SCOPE.into(), cal.into()], &[GMAIL_MODIFY_SCOPE.into()]));
        assert!(!grants_more(&[cal.into()], &[GMAIL_MODIFY_SCOPE.into()]));
        assert!(!grants_more(&[], &[]));
    }

    #[test]
    fn a_token_this_device_cant_refresh_is_replaced() {
        // The phone holds a browser sign-in whose client it never got: an
        // account waiting for sign-in. The Mac's next pairing fixes it.
        let phone_store = Arc::new(MemoryStore::default());
        phone_store
            .save("grace@example.com", &cred("rt-stale", desk(Some(DESK))))
            .unwrap();
        let paired = Arc::new(MemoryBackend::default());
        let phone = phone(phone_store.clone(), &paired);
        assert!(!phone.can_refresh("grace@example.com").unwrap());
        let json = vault(&[("grace@example.com", cred("rt-g", desk(Some(DESK))))]);
        let report = phone.import_vault(&json, Some(&desk_client())).unwrap();
        assert_eq!(report.imported, ["grace@example.com"]);
        assert!(report.kept.is_empty());
        assert_eq!(
            phone_store
                .load("grace@example.com")
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-g"
        );
    }

    #[test]
    fn import_needs_a_client_and_a_readable_vault() {
        let paired = Arc::new(MemoryBackend::default());
        let nothing = with_config(
            OAuthClientConfig {
                client_id: String::new(),
                client_secret: None,
            },
            Arc::new(MemoryStore::default()),
            None,
        );
        assert!(matches!(
            nothing.import_vault(r#"{"version":3,"accounts":{}}"#, None),
            Err(Error::NotConfigured(_))
        ));
        // A malformed desktop client counts as none.
        let junk = OAuthClientConfig {
            client_id: "bad id\n".into(),
            client_secret: None,
        };
        assert!(matches!(
            nothing.import_vault(r#"{"version":3,"accounts":{}}"#, Some(&junk)),
            Err(Error::NotConfigured(_))
        ));
        // A desktop client alone is enough.
        assert!(nothing
            .import_vault(r#"{"version":3,"accounts":{}}"#, Some(&desk_client()))
            .is_ok());
        let phone = phone(Arc::new(MemoryStore::default()), &paired);
        assert!(phone.import_vault("{}", None).is_err());
        assert!(phone
            .import_vault(r#"{"version":9,"accounts":{}}"#, None)
            .is_err());
    }

    #[test]
    fn can_refresh_follows_the_clients_held() {
        let store = Arc::new(MemoryStore::default());
        store.save("i@example.com", &cred("a", ios(IOS))).unwrap();
        store
            .save("legacy@example.com", &cred("b", desk(None)))
            .unwrap();
        store
            .save("d@example.com", &cred("c", desk(Some(DESK))))
            .unwrap();
        let mac = manager(store.clone(), None);
        assert!(mac.can_refresh("i@example.com").unwrap());
        assert!(mac.can_refresh("legacy@example.com").unwrap());
        assert!(mac.can_refresh("D@example.com").unwrap());
        assert!(!mac.can_refresh("none@example.com").unwrap());
        let paired = Arc::new(MemoryBackend::default());
        let phone = phone(store, &paired);
        assert!(phone.can_refresh("i@example.com").unwrap());
        assert!(!phone.can_refresh("legacy@example.com").unwrap());
        assert!(!phone.can_refresh("d@example.com").unwrap());
    }

    #[tokio::test]
    async fn forget_deletes_without_revoking() {
        let store = Arc::new(MemoryStore::default());
        store
            .save("ada@example.com", &cred("rt-a", ios(IOS)))
            .unwrap();
        // Endpoints::default() is Google itself: a revoke would go out over
        // the network. forget() makes no request at all.
        let auth = manager(store.clone(), Some(IOS));
        auth.forget("Ada@example.com").await.unwrap();
        assert!(store.load("ada@example.com").unwrap().is_none());
    }
}
