//! Per-sender facts for the avatar service (apps/desktop/src-tauri/src/avatars):
//! whether the newest stored message from an address was authenticated by
//! Gmail's MX (DMARC pass or aligned DKIM). Brand imagery (BIMI logos,
//! company icons) is only shown for senders that pass.

use std::collections::{HashMap, HashSet};

use rusqlite::params_from_iter;

use super::{Store, F_AUTH};
use crate::Result;

/// Above this many addresses, one grouped scan beats an IN list.
const CHUNK: usize = 200;

impl Store {
    /// For each address (compared case-insensitively), whether its newest
    /// stored message has `sender_authenticated`. Addresses with no stored
    /// mail are absent from the map. Up to 200 addresses use one filtered
    /// scan of `messages`; more use a single grouped scan filtered in Rust,
    /// so the cost is at most one scan however large the batch. Meant for
    /// background work, not the render path.
    pub fn latest_sender_authenticated(&self, emails: &[String]) -> Result<HashMap<String, bool>> {
        let wanted: HashSet<String> = emails.iter().map(|e| e.trim().to_lowercase()).collect();
        if wanted.is_empty() {
            return Ok(HashMap::new());
        }
        let list: Vec<&String> = wanted.iter().collect();
        let sql = if list.len() <= CHUNK {
            let marks = vec!["?"; list.len()].join(",");
            format!(
                "SELECT lower(from_email), max(date), flags FROM messages \
                 WHERE lower(from_email) IN ({marks}) GROUP BY 1"
            )
        } else {
            "SELECT lower(from_email), max(date), flags FROM messages GROUP BY 1".to_string()
        };
        let params: Vec<&String> = if list.len() <= CHUNK { list } else { vec![] };
        self.read(|c| {
            let mut stmt = c.prepare(&sql)?;
            // SQLite's bare-column rule: with max(), `flags` comes from the
            // row holding the maximum date.
            let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(2)?))
            })?;
            let mut out = HashMap::new();
            for row in rows {
                let (email, flags) = row?;
                if wanted.contains(&email) {
                    out.insert(email, flags & F_AUTH != 0);
                }
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{Address, Message};
    use crate::Store;

    fn msg(id: &str, from: &str, date: i64, authed: bool) -> Message {
        Message {
            account_id: "ada@x.example".into(),
            id: id.into(),
            thread_id: id.into(),
            date,
            from: Address {
                name: None,
                email: from.into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "s".into(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: authed,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    #[test]
    fn newest_message_decides() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_messages(&[
                msg("a1", "News@Shop.example", 100, true),
                msg("a2", "news@shop.example", 200, false),
                msg("b1", "bo@bank.example", 100, false),
                msg("b2", "bo@bank.example", 300, true),
            ])
            .unwrap();
        let got = store
            .latest_sender_authenticated(&[
                "news@shop.example".into(),
                "BO@bank.example".into(),
                "nobody@x.example".into(),
            ])
            .unwrap();
        assert_eq!(got.get("news@shop.example"), Some(&false));
        assert_eq!(got.get("bo@bank.example"), Some(&true));
        assert!(!got.contains_key("nobody@x.example"));
        // A batch larger than CHUNK takes the single-scan path; same answers.
        let mut many: Vec<String> = (0..300).map(|i| format!("p{i}@x.example")).collect();
        many.push("NEWS@shop.example".into());
        many.push("bo@bank.example".into());
        let got = store.latest_sender_authenticated(&many).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got.get("news@shop.example"), Some(&false));
        assert_eq!(got.get("bo@bank.example"), Some(&true));
    }
}
