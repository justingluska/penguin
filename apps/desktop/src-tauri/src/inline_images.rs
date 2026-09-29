//! Inline (`cid:`) image cache.
//!
//! Choice: plain files under the app cache dir,
//! `<cache>/inline/<account>/<message id>/<hex(content-id)>`, holding the raw
//! image bytes. They are immutable per (message, cid), so a file cache needs
//! no invalidation, keeps blobs out of the mail DB/WAL, can be wiped by the OS
//! at no cost (we just refetch), and is deleted with the account.
//!
//! `get_thread` never waits on the network: it renders with whatever is
//! cached, fetches the rest in the background, then emits `mail-changed` for
//! the thread so the UI re-requests it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use penguin_core::{AttachmentMeta, Message};
use penguin_provider::MailProvider;

use crate::ops::{safe_component, Paths};

/// Larger inline images are left as broken references rather than inlined as
/// data: URLs into the iframe document.
const MAX_INLINE_BYTES: u64 = 8 * 1024 * 1024;

pub struct InlineImages {
    paths: Paths,
    in_flight: Mutex<HashSet<PathBuf>>,
    /// Fetches that failed this session; not retried until restart so a
    /// broken attachment can't cause a refetch loop on every render.
    failed: Mutex<HashSet<PathBuf>>,
}

/// Inline images `m` may reference: image parts with a Content-ID, when the
/// HTML uses `cid:` at all. (Matching each id textually would miss
/// percent-encoded references, which penguin-render does resolve.)
fn referenced(m: &Message) -> Vec<&AttachmentMeta> {
    let Some(html) = m.body_html.as_deref() else {
        return vec![];
    };
    if !html.to_ascii_lowercase().contains("cid:") {
        return vec![];
    }
    m.attachments
        .iter()
        .filter(|a| a.content_id.is_some())
        .filter(|a| {
            a.mime_type.to_ascii_lowercase().starts_with("image/") && a.size <= MAX_INLINE_BYTES
        })
        .collect()
}

fn data_url(mime: &str, bytes: &[u8]) -> Option<String> {
    let mime = mime.to_ascii_lowercase();
    let safe = mime.starts_with("image/")
        && mime
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '+' | '-'));
    safe.then(|| {
        format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    })
}

/// File name for a content id: hex so any cid is a safe name; very long ids
/// are truncated plus a hash to stay under filesystem name limits.
fn cid_file_name(cid: &str) -> String {
    use std::hash::{Hash, Hasher};
    let hex: String = cid.bytes().map(|b| format!("{b:02x}")).collect();
    if hex.len() <= 120 {
        return hex;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cid.hash(&mut h);
    format!("{}-{:016x}", &hex[..96], h.finish())
}

fn cache_file(paths: &Paths, account_id: &str, message_id: &str, cid: &str) -> PathBuf {
    paths
        .inline_cache_dir(account_id)
        .join(safe_component(message_id))
        .join(cid_file_name(cid))
}

/// An inline image already in this cache (the message was shown), without
/// touching the network. For agent reads (penguin-cli's get_attachment).
pub fn cached_bytes(
    paths: &Paths,
    account_id: &str,
    message_id: &str,
    cid: &str,
) -> Option<Vec<u8>> {
    std::fs::read(cache_file(paths, account_id, message_id, cid)).ok()
}

impl InlineImages {
    pub fn new(paths: Paths) -> Self {
        InlineImages {
            paths,
            in_flight: Mutex::new(HashSet::new()),
            failed: Mutex::new(HashSet::new()),
        }
    }

    fn file(&self, account_id: &str, message_id: &str, cid: &str) -> PathBuf {
        cache_file(&self.paths, account_id, message_id, cid)
    }

    /// cid → data: URL for everything already cached, plus the referenced
    /// attachments that still need fetching. Blocking (reads files).
    pub fn cached(&self, m: &Message) -> (HashMap<String, String>, Vec<AttachmentMeta>) {
        let mut map = HashMap::new();
        let mut missing = Vec::new();
        for att in referenced(m) {
            let cid = att.content_id.as_deref().unwrap_or_default();
            match std::fs::read(self.file(&m.account_id, &m.id, cid)) {
                Ok(bytes) => {
                    if let Some(url) = data_url(&att.mime_type, &bytes) {
                        map.insert(cid.to_string(), url);
                    }
                }
                Err(_) => missing.push(att.clone()),
            }
        }
        (map, missing)
    }

    /// Fetch and cache `missing` for one message. Returns true if at least
    /// one new image landed (the caller then tells the UI to re-render).
    pub async fn fetch(
        &self,
        provider: &dyn MailProvider,
        account_id: &str,
        message_id: &str,
        missing: &[AttachmentMeta],
    ) -> bool {
        let mut fetched_any = false;
        for att in missing {
            let cid = att.content_id.as_deref().unwrap_or_default();
            let path = self.file(account_id, message_id, cid);
            {
                if self.failed.lock().unwrap().contains(&path) {
                    continue;
                }
                if !self.in_flight.lock().unwrap().insert(path.clone()) {
                    continue;
                }
            }
            let result = match provider.get_attachment(message_id, att).await {
                Ok(bytes) => write_atomic(&path, &bytes).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            self.in_flight.lock().unwrap().remove(&path);
            match result {
                Ok(()) => fetched_any = true,
                Err(e) => {
                    tracing::warn!(account = %account_id, message = %message_id, error = %e, "inline image fetch failed");
                    self.failed.lock().unwrap().insert(path);
                }
            }
        }
        fetched_any
    }

    pub fn remove_account(&self, account_id: &str) {
        let dir = self.paths.inline_cache_dir(account_id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(error = %e, "could not delete inline image cache"),
        }
    }
}

fn write_atomic(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(html: &str, atts: Vec<AttachmentMeta>) -> Message {
        Message {
            account_id: "ada@x.example".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 0,
            from: penguin_core::Address {
                name: None,
                email: "bo@x.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: String::new(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: Some(html.into()),
            label_ids: vec![],
            attachments: atts,
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    fn att(cid: &str, mime: &str) -> AttachmentMeta {
        AttachmentMeta {
            id: format!("att-{cid}"),
            filename: "a.png".into(),
            mime_type: mime.into(),
            size: 10,
            content_id: Some(cid.into()),
            inline: true,
        }
    }

    #[test]
    fn only_cid_images_are_wanted() {
        let atts = vec![
            att("logo@x.example", "image/png"),
            att("doc@x", "application/pdf"),
        ];
        let m = msg(r#"<img src="CID:Logo%40x.example">"#, atts.clone());
        let ids: Vec<_> = referenced(&m).iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids, vec!["att-logo@x.example"]);
        assert!(referenced(&msg("<p>no inline images</p>", atts)).is_empty());
    }

    #[test]
    fn caches_round_trip() {
        let root = std::env::temp_dir().join(format!("penguin-inline-test-{}", std::process::id()));
        let paths = Paths {
            data_dir: root.clone(),
            config_dir: root.clone(),
            cache_dir: root.clone(),
        };
        let images = InlineImages::new(paths);
        let m = msg(r#"<img src="cid:logo">"#, vec![att("logo", "image/png")]);
        let (map, missing) = images.cached(&m);
        assert!(map.is_empty());
        assert_eq!(missing.len(), 1);
        write_atomic(&images.file(&m.account_id, &m.id, "logo"), b"png").unwrap();
        let (map, missing) = images.cached(&m);
        assert!(missing.is_empty());
        assert_eq!(map["logo"], "data:image/png;base64,cG5n");
        images.remove_account(&m.account_id);
        let _ = std::fs::remove_dir_all(root);
    }
}
