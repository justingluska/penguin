//! Google contact photos via the People API: a bulk, paginated sync of
//! "Other contacts" (people you've emailed; `contacts.other.readonly`) and
//! your contacts (`contacts.readonly`) into a local map of
//! sha256(address) → photo URL. It runs about once a day per account that
//! granted the scopes; senders are never looked up one by one.
//!
//! Generated letter avatars (`photos[].default = true`) are skipped: the
//! monogram already does that job.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cache::write_atomic;
use super::net::{FetchError, Net};
use super::sources::email_hash;

const OTHER_CONTACTS: &str = "https://people.googleapis.com/v1/otherContacts?readMask=emailAddresses,photos&sources=READ_SOURCE_TYPE_CONTACT&sources=READ_SOURCE_TYPE_PROFILE&pageSize=1000";
const CONNECTIONS: &str = "https://people.googleapis.com/v1/people/me/connections?personFields=emailAddresses,photos&sources=READ_SOURCE_TYPE_CONTACT&sources=READ_SOURCE_TYPE_PROFILE&pageSize=1000";
/// 50 pages × 1000 people is far past any real address book.
const MAX_PAGES: usize = 50;
const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Page {
    other_contacts: Vec<Person>,
    connections: Vec<Person>,
    next_page_token: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Person {
    email_addresses: Vec<EmailAddress>,
    photos: Vec<Photo>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EmailAddress {
    value: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Photo {
    url: String,
    default: bool,
    metadata: Option<PhotoMeta>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PhotoMeta {
    primary: bool,
}

/// (email hash, photo URL) pairs and the next page token.
pub type ParsedPage = (Vec<(String, String)>, Option<String>);

/// (email hash → photo URL) from one response page, plus the next token.
pub fn parse_page(json: &[u8]) -> Result<ParsedPage, String> {
    let page: Page = serde_json::from_slice(json).map_err(|e| format!("People API: {e}"))?;
    let mut out = Vec::new();
    for person in page.other_contacts.iter().chain(page.connections.iter()) {
        let mut photos: Vec<&Photo> = person
            .photos
            .iter()
            .filter(|p| !p.default && !p.url.is_empty())
            .collect();
        photos.sort_by_key(|p| !p.metadata.as_ref().is_some_and(|m| m.primary));
        let Some(photo) = photos.first() else {
            continue;
        };
        for e in &person.email_addresses {
            if e.value.contains('@') {
                out.push((email_hash(&e.value), photo.url.clone()));
            }
        }
    }
    Ok((out, page.next_page_token.filter(|t| !t.is_empty())))
}

#[derive(Debug)]
pub enum SyncError {
    /// 401/403: the token lacks the contacts scopes (or was revoked).
    NotGranted,
    /// 403 because the People API isn't enabled in the user's Google Cloud
    /// project (Penguin uses the user's own OAuth client).
    ApiDisabled,
    Failed(String),
}

/// Page through one listing.
async fn list_all(
    net: &dyn Net,
    token: &str,
    base: &str,
    into: &mut HashMap<String, String>,
) -> Result<(), SyncError> {
    let mut page_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let url = match &page_token {
            Some(t) => format!("{base}&pageToken={}", url_encode(t)),
            None => base.to_string(),
        };
        let body = net
            .get(&url, MAX_PAGE_BYTES, Some(token))
            .await
            .map_err(|e| match e {
                FetchError::Forbidden(body)
                    if body.contains("SERVICE_DISABLED")
                        || body.contains("accessNotConfigured") =>
                {
                    SyncError::ApiDisabled
                }
                FetchError::Unauthorized | FetchError::Forbidden(_) | FetchError::NotFound => {
                    SyncError::NotGranted
                }
                FetchError::Transient(e) => SyncError::Failed(e),
            })?;
        let (entries, next) = parse_page(&body).map_err(SyncError::Failed)?;
        for (hash, url) in entries {
            into.insert(hash, url);
        }
        match next {
            Some(t) => page_token = Some(t),
            None => return Ok(()),
        }
    }
    Ok(())
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Other contacts first, then contacts, which win for the same address.
pub async fn sync(net: &dyn Net, token: &str) -> Result<HashMap<String, String>, SyncError> {
    let mut map = HashMap::new();
    list_all(net, token, OTHER_CONTACTS, &mut map).await?;
    list_all(net, token, CONNECTIONS, &mut map).await?;
    Ok(map)
}

/// One account's synced map, stored as `contacts/<hash(account)>.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContactPhotos {
    pub synced_at: u64,
    pub photos: HashMap<String, String>,
}

pub fn file_for(dir: &Path, account_id: &str) -> PathBuf {
    dir.join(format!("{}.json", &email_hash(account_id)[..32]))
}

pub fn load(path: &Path) -> Option<ContactPhotos> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn save(path: &Path, photos: &ContactPhotos) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(photos).map_err(std::io::Error::other)?;
    write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTHER_PAGE: &str = r#"{
      "otherContacts": [
        {
          "resourceName": "otherContacts/c1",
          "etag": "e1",
          "emailAddresses": [{"metadata": {"primary": true}, "value": "Ada@Northwind.example"}],
          "photos": [
            {"metadata": {"primary": false}, "url": "https://lh3.googleusercontent.com/cm/letter=s100", "default": true},
            {"metadata": {"primary": true}, "url": "https://lh3.googleusercontent.com/a-/ada=s100"}
          ]
        },
        {
          "resourceName": "otherContacts/c2",
          "emailAddresses": [{"value": "bo@shop.example"}],
          "photos": [{"url": "https://lh3.googleusercontent.com/cm/bo=s100", "default": true}]
        },
        {"resourceName": "otherContacts/c3", "emailAddresses": [{"value": "nophoto@x.example"}]},
        {"resourceName": "otherContacts/c4", "photos": [{"url": "https://lh3.googleusercontent.com/a/orphan"}]}
      ],
      "nextPageToken": "page 2/=",
      "totalSize": 4
    }"#;

    #[test]
    fn parses_other_contacts_skipping_default_photos() {
        let (entries, next) = parse_page(OTHER_PAGE.as_bytes()).unwrap();
        assert_eq!(
            entries,
            vec![(
                email_hash("ada@northwind.example"),
                "https://lh3.googleusercontent.com/a-/ada=s100".to_string()
            )]
        );
        assert_eq!(next.as_deref(), Some("page 2/="));
        assert_eq!(url_encode("page 2/="), "page%202%2F%3D");
    }

    #[test]
    fn parses_connections_and_empty_pages() {
        let json = r#"{"connections":[{"resourceName":"people/1","emailAddresses":[{"value":"cy@x.example"},{"value":"cy@home.example"}],"photos":[{"url":"https://lh3.googleusercontent.com/cy"}]}],"totalPeople":1}"#;
        let (entries, next) = parse_page(json.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(next.is_none());
        let (entries, next) = parse_page(b"{}").unwrap();
        assert!(entries.is_empty() && next.is_none());
        assert!(parse_page(b"<html>").is_err());
    }

    struct FakePeople {
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl Net for FakePeople {
        fn txt<'a>(
            &'a self,
            _: &'a str,
        ) -> super::super::net::BoxFut<'a, Result<Vec<String>, String>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn get<'a>(
            &'a self,
            url: &'a str,
            _max: usize,
            bearer: Option<&'a str>,
        ) -> super::super::net::BoxFut<'a, Result<Vec<u8>, FetchError>> {
            Box::pin(async move {
                assert_eq!(bearer, Some("tok"));
                self.calls.lock().unwrap().push(url.to_string());
                let body = if url.contains("otherContacts") && !url.contains("pageToken") {
                    OTHER_PAGE.to_string()
                } else if url.contains("otherContacts") {
                    r#"{"otherContacts":[{"emailAddresses":[{"value":"dee@x.example"}],"photos":[{"url":"https://lh3.googleusercontent.com/dee"}]}]}"#.to_string()
                } else {
                    r#"{"connections":[{"emailAddresses":[{"value":"ada@northwind.example"}],"photos":[{"url":"https://lh3.googleusercontent.com/ada-contact"}]}]}"#.to_string()
                };
                Ok(body.into_bytes())
            })
        }
    }

    #[test]
    fn disabled_api_is_told_apart_from_a_missing_scope() {
        struct Forbidden(&'static str);
        impl Net for Forbidden {
            fn txt<'a>(
                &'a self,
                _: &'a str,
            ) -> super::super::net::BoxFut<'a, Result<Vec<String>, String>> {
                Box::pin(async { Ok(vec![]) })
            }
            fn get<'a>(
                &'a self,
                _: &'a str,
                _: usize,
                _: Option<&'a str>,
            ) -> super::super::net::BoxFut<'a, Result<Vec<u8>, FetchError>> {
                Box::pin(async move { Err(FetchError::Forbidden(self.0.to_string())) })
            }
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let disabled = r#"{"error":{"code":403,"status":"PERMISSION_DENIED","details":[{"reason":"SERVICE_DISABLED"}]}}"#;
        assert!(matches!(
            rt.block_on(sync(&Forbidden(disabled), "tok")),
            Err(SyncError::ApiDisabled)
        ));
        let scope = r#"{"error":{"code":403,"status":"PERMISSION_DENIED","message":"Request had insufficient authentication scopes."}}"#;
        assert!(matches!(
            rt.block_on(sync(&Forbidden(scope), "tok")),
            Err(SyncError::NotGranted)
        ));
    }

    #[tokio::test]
    async fn sync_pages_and_contacts_win() {
        let net = FakePeople {
            calls: Default::default(),
        };
        let map = sync(&net, "tok").await.unwrap();
        assert_eq!(net.calls.lock().unwrap().len(), 3);
        assert_eq!(
            map.get(&email_hash("ada@northwind.example"))
                .map(String::as_str),
            Some("https://lh3.googleusercontent.com/ada-contact")
        );
        assert!(map.contains_key(&email_hash("dee@x.example")));
        // No addresses in clear in what we'd store.
        let json = serde_json::to_string(&ContactPhotos {
            synced_at: 1,
            photos: map,
        })
        .unwrap();
        assert!(!json.contains("northwind"));
    }
}
