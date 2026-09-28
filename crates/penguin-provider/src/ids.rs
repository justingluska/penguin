//! Message, thread and label identity across providers
//! (docs/PROVIDERS-IMPL.md → Identity).
//!
//! Every id the store keeps is an opaque string, unique within its account:
//! the store keys rows by `(account_id, id)` and never parses ids, so each
//! provider picks its own scheme and existing Gmail rows keep theirs.
//!
//! | provider | message id | thread id |
//! |---|---|---|
//! | Gmail | Gmail's hex id (unchanged) | Gmail's thread id |
//! | Microsoft | the immutable id (`Prefer: IdType="ImmutableId"`), as is | `conversationId`, as is |
//! | IMAP | `e:<EMAILID>` (RFC 8474 OBJECTID) when the server has it, else [`imap_fallback_message_id`] (`h:` + hash) | [`imap_thread_id`] of the thread root (`t:` + hash) |
//!
//! IMAP UIDs are locations, not identities (a move changes them, UIDVALIDITY
//! resets them), so IMAP keeps `(folder, uidvalidity, uid) → message id` in
//! its own table (`Store::migrate_provider_schema`).

use sha2::{Digest, Sha256};

/// Longest id the store accepts from a provider.
pub const MAX_ID_BYTES: usize = 512;

/// Ids starting with this are reserved for the store's synthetic keys.
pub const RESERVED_PREFIX: char = '~';

/// Why an id can't be stored, or None when it's fine: non-empty, at most
/// [`MAX_ID_BYTES`], printable ASCII without spaces, not starting with `~`.
/// (Gmail, Graph immutable ids and the IMAP forms here all qualify.)
pub fn check_id(id: &str) -> Option<&'static str> {
    if id.is_empty() {
        return Some("empty");
    }
    if id.len() > MAX_ID_BYTES {
        return Some("too long");
    }
    if id.starts_with(RESERVED_PREFIX) {
        return Some("starts with the reserved '~'");
    }
    if !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Some("not printable ASCII");
    }
    None
}

fn short_hash(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_be_bytes());
        h.update(p);
    }
    h.finalize()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `Message-ID` as a comparable key: angle brackets and whitespace removed,
/// the domain part lowercased (the local part is case-sensitive).
pub fn normalize_message_id(raw: &str) -> String {
    let bare = raw
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    match bare.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_ascii_lowercase()),
        None => bare.to_string(),
    }
}

/// IMAP message id from the server's EMAILID (RFC 8474), when it has one.
pub fn imap_message_id_from_emailid(emailid: &str) -> String {
    format!("e:{emailid}")
}

/// IMAP message id when the server has no EMAILID: a hash of what stays the
/// same when a message moves between folders — its Message-ID header, its
/// Date header value and its size. Two copies of one message (Sent and
/// Inbox for mail to yourself) get the same id and are stored once, with
/// both locations. Mail without a Message-ID still gets a stable id from
/// the other two.
pub fn imap_fallback_message_id(
    message_id_header: Option<&str>,
    date_header: Option<&str>,
    rfc822_size: u64,
) -> String {
    let mid = message_id_header
        .map(normalize_message_id)
        .unwrap_or_default();
    let date = date_header.unwrap_or("").trim();
    format!(
        "h:{}",
        short_hash(&[mid.as_bytes(), date.as_bytes(), &rfc822_size.to_be_bytes()])
    )
}

/// IMAP thread id: a hash of the thread root's normalized Message-ID (the
/// first entry of References, else In-Reply-To, else the message's own).
/// Threads that JWZ later merges are moved with `Store::set_message_thread`.
pub fn imap_thread_id(root_message_id: &str) -> String {
    format!(
        "t:{}",
        short_hash(&[normalize_message_id(root_message_id).as_bytes()])
    )
}

/// A user label id for a folder or category name. The store keeps a
/// message's labels space-separated, so label ids must pass [`check_id`]:
/// `prefix` (e.g. `f:` for IMAP folders, `c:` for Microsoft categories)
/// plus the name with `%`, spaces, controls and non-ASCII bytes
/// percent-encoded (UTF-8). Reversible with [`label_name_from_id`].
pub fn label_id_for_name(prefix: &str, name: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + name.len());
    out.push_str(prefix);
    for b in name.bytes() {
        if b.is_ascii_graphic() && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The name a [`label_id_for_name`] id was made from (None: not that
/// prefix, or malformed encoding).
pub fn label_name_from_id(prefix: &str, id: &str) -> Option<String> {
    let rest = id.strip_prefix(prefix)?.as_bytes();
    let mut bytes = Vec::with_capacity(rest.len());
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == b'%' {
            let hex = std::str::from_utf8(rest.get(i + 1..i + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(rest[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

/// The canonical system label ids. Providers map their folders and flags
/// onto these (docs/PROVIDERS-IMPL.md → Labels and folders); everything
/// else is a user label.
pub mod system {
    pub const INBOX: &str = "INBOX";
    pub const SENT: &str = "SENT";
    pub const DRAFT: &str = "DRAFT";
    pub const TRASH: &str = "TRASH";
    pub const SPAM: &str = "SPAM";
    pub const STARRED: &str = "STARRED";
    pub const UNREAD: &str = "UNREAD";
    pub const IMPORTANT: &str = "IMPORTANT";
    /// Every system label a non-Gmail provider may emit.
    pub const ALL: [&str; 8] = [INBOX, SENT, DRAFT, TRASH, SPAM, STARRED, UNREAD, IMPORTANT];

    /// A label id the store treats as a system label: the set above plus
    /// Gmail's CATEGORY_* and CHAT.
    pub fn is_system(label: &str) -> bool {
        ALL.contains(&label) || label.starts_with("CATEGORY_") || label == "CHAT"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_every_provider_emits_are_storable() {
        for id in [
            "18c2f0a1b2c3d4e5",
            "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0AUmYz7bIa2kOkZ8zVv-G4ZgAAAYo=",
            &imap_message_id_from_emailid("M6d99ac3275bb4e"),
            &imap_fallback_message_id(Some("<a@b.example>"), Some("Mon, 1 Jan 2024"), 10),
            &imap_thread_id("<root@b.example>"),
        ] {
            assert_eq!(check_id(id), None, "{id}");
        }
        assert!(check_id("").is_some());
        assert!(check_id("~all").is_some());
        assert!(check_id("has space").is_some());
        assert!(check_id(&"x".repeat(MAX_ID_BYTES + 1)).is_some());
    }

    #[test]
    fn fallback_ids_survive_moves_and_tell_messages_apart() {
        let a = imap_fallback_message_id(Some("<Abc@Mail.Example>"), Some(" Mon "), 42);
        // Same message seen in another folder (brackets, domain case, spaces).
        let b = imap_fallback_message_id(Some("Abc@mail.example"), Some("Mon"), 42);
        assert_eq!(a, b);
        assert!(a.starts_with("h:") && a.len() == 2 + 24);
        // The local part is case-sensitive; size and date matter.
        assert_ne!(
            a,
            imap_fallback_message_id(Some("<abc@mail.example>"), Some("Mon"), 42)
        );
        assert_ne!(
            a,
            imap_fallback_message_id(Some("<Abc@mail.example>"), Some("Mon"), 43)
        );
        assert_ne!(
            a,
            imap_fallback_message_id(Some("<Abc@mail.example>"), Some("Tue"), 42)
        );
        // No Message-ID at all is still stable.
        assert_eq!(
            imap_fallback_message_id(None, Some("Mon"), 1),
            imap_fallback_message_id(None, Some("Mon"), 1)
        );
    }

    #[test]
    fn thread_ids_follow_the_normalized_root() {
        assert_eq!(
            imap_thread_id("<Root@Example.COM>"),
            imap_thread_id("Root@example.com")
        );
        assert_ne!(imap_thread_id("<a@x>"), imap_thread_id("<b@x>"));
        assert!(imap_thread_id("<a@x>").starts_with("t:"));
    }

    #[test]
    fn folder_and_category_names_become_storable_label_ids() {
        for name in [
            "Receipts",
            "Clients/Acme 2024",
            "100% done",
            "Überweisungen",
            "日本",
        ] {
            let id = label_id_for_name("f:", name);
            assert_eq!(check_id(&id), None, "{id}");
            assert_eq!(label_name_from_id("f:", &id).as_deref(), Some(name));
        }
        assert_eq!(label_id_for_name("c:", "Red category"), "c:Red%20category");
        assert_eq!(label_name_from_id("c:", "f:x"), None);
        assert_eq!(label_name_from_id("f:", "f:bad%2"), None);
    }

    #[test]
    fn system_labels() {
        for l in system::ALL {
            assert!(system::is_system(l));
        }
        assert!(system::is_system("CATEGORY_PROMOTIONS"));
        assert!(!system::is_system("Receipts"));
        assert!(!system::is_system("Label_12"));
    }
}
