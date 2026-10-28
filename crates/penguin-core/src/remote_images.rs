//! Remote images: whether a message renders with them on first open
//! (Settings → Privacy → Remote images, plus "Always load images from"),
//! and the trusted-sender list's rules. The Mac (src-tauri `settings.rs`)
//! and the iPhone (penguin-ffi `settings.rs`) both decide through here, with
//! the same settings keys (`remoteImages`, `trustedImageSenders`).

use serde::{Deserialize, Serialize};

use crate::types::Message;

/// Most addresses "Always load images from" keeps.
pub const MAX_TRUSTED_SENDERS: usize = 5000;
/// Longest address kept (RFC 5321's path limit).
const MAX_ADDRESS_LEN: usize = 320;

/// Settings → Privacy → Remote images (`remoteImages`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RemoteImages {
    /// Held back until "Load images", or a trusted sender's authenticated
    /// mail.
    #[default]
    Ask,
    /// Always loaded when a message is rendered.
    Always,
    /// Never loaded; "Load images" is refused.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteImageDecision {
    Load,
    Block,
    /// Trusted sender, but the message isn't authenticated as theirs.
    BlockUnverifiedSender,
}

/// Whether `m` renders with remote images on first open. `trusted` is a
/// normalized list ([`normalize_trusted_senders`]: lowercased, sorted).
///
/// A trusted sender only counts when the provider authenticated the
/// message as really coming from that address
/// (`Message::sender_authenticated`: Gmail's DMARC pass or aligned DKIM):
/// From is attacker-controlled, and a spoofed "trusted" sender would
/// otherwise learn the reader's IP and open time.
///
/// Spam never loads them on its own, whatever the setting: a remote image
/// there is how a spammer learns the address is read. Loading them by hand
/// ("Load images") still works.
pub fn decide(mode: RemoteImages, trusted: &[String], m: &Message) -> RemoteImageDecision {
    if m.label_ids.iter().any(|l| l == "SPAM") {
        return RemoteImageDecision::Block;
    }
    match mode {
        RemoteImages::Always => RemoteImageDecision::Load,
        RemoteImages::Never => RemoteImageDecision::Block,
        RemoteImages::Ask => {
            if !is_trusted(trusted, &m.from.email) {
                RemoteImageDecision::Block
            } else if m.sender_authenticated {
                RemoteImageDecision::Load
            } else {
                RemoteImageDecision::BlockUnverifiedSender
            }
        }
    }
}

/// Whether `email` is in the normalized `trusted` list.
pub fn is_trusted(trusted: &[String], email: &str) -> bool {
    trusted.binary_search(&email.trim().to_lowercase()).is_ok()
}

/// Lowercase, trim, sort, dedupe and bound the list; drop entries that
/// aren't an address.
pub fn normalize_trusted_senders(senders: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = senders
        .iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| {
            s.contains('@') && s.len() <= MAX_ADDRESS_LEN && !s.chars().any(char::is_whitespace)
        })
        .collect();
    out.sort();
    out.dedup();
    out.truncate(MAX_TRUSTED_SENDERS);
    out
}

/// `senders` with `email` added (`trusted`) or removed, normalized.
pub fn set_trusted(mut senders: Vec<String>, email: &str, trusted: bool) -> Vec<String> {
    let addr = email.trim().to_lowercase();
    senders.retain(|e| e.trim().to_lowercase() != addr);
    if trusted {
        senders.push(addr);
    }
    normalize_trusted_senders(senders)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Address;

    fn from(email: &str) -> Message {
        Message {
            account_id: "me@example.com".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 0,
            from: Address {
                name: None,
                email: email.into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Sale".into(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    #[test]
    fn the_policy() {
        let trusted = normalize_trusted_senders(vec!["news@shop.example".into()]);
        let ask = RemoteImages::Ask;
        // Trusted but unauthenticated: blocked, and flagged for the UI note.
        assert_eq!(
            decide(ask, &trusted, &from("News@Shop.example")),
            RemoteImageDecision::BlockUnverifiedSender
        );
        assert_eq!(
            decide(ask, &trusted, &from("other@shop.example")),
            RemoteImageDecision::Block
        );
        // Trusted and authenticated: loads.
        let mut authed = from("news@shop.example");
        authed.sender_authenticated = true;
        assert_eq!(decide(ask, &trusted, &authed), RemoteImageDecision::Load);
        // Authenticated but not trusted: still blocked.
        let mut stranger = from("other@shop.example");
        stranger.sender_authenticated = true;
        assert_eq!(decide(ask, &trusted, &stranger), RemoteImageDecision::Block);
        let never = RemoteImages::Never;
        assert_eq!(decide(never, &trusted, &authed), RemoteImageDecision::Block);
        assert_eq!(
            decide(never, &trusted, &from("news@shop.example")),
            RemoteImageDecision::Block
        );
        let always = RemoteImages::Always;
        assert_eq!(
            decide(always, &trusted, &from("other@shop.example")),
            RemoteImageDecision::Load
        );
        // Spam: blocked under Always, and for a trusted, authenticated sender.
        let mut spam = from("other@shop.example");
        spam.label_ids = vec!["SPAM".into(), "UNREAD".into()];
        assert_eq!(decide(always, &trusted, &spam), RemoteImageDecision::Block);
        let mut trusted_spam = authed.clone();
        trusted_spam.label_ids = vec!["SPAM".into()];
        assert_eq!(
            decide(ask, &trusted, &trusted_spam),
            RemoteImageDecision::Block
        );
    }

    #[test]
    fn the_list_is_normalized() {
        let got = normalize_trusted_senders(vec![
            " Bo@X.example ".into(),
            "bo@x.example".into(),
            "junk".into(),
            "a b@x.example".into(),
            format!("{}@x.example", "a".repeat(320)),
            "al@x.example".into(),
        ]);
        assert_eq!(got, vec!["al@x.example", "bo@x.example"]);
        let many: Vec<String> = (0..MAX_TRUSTED_SENDERS + 3)
            .map(|i| format!("n{i}@x.example"))
            .collect();
        assert_eq!(normalize_trusted_senders(many).len(), MAX_TRUSTED_SENDERS);
    }

    #[test]
    fn adding_and_removing() {
        let list = set_trusted(vec![], " News@Shop.example", true);
        assert_eq!(list, vec!["news@shop.example"]);
        assert!(is_trusted(&list, "NEWS@shop.example "));
        let list = set_trusted(list, "ann@x.example", true);
        assert_eq!(list, vec!["ann@x.example", "news@shop.example"]);
        let list = set_trusted(list, "news@SHOP.example", false);
        assert_eq!(list, vec!["ann@x.example"]);
        assert!(!is_trusted(&list, "news@shop.example"));
    }
}
