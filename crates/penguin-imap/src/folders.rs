//! Folders → Penguin's label model (docs/PROVIDERS-IMPL.md §4).
//!
//! Special-use folders (RFC 6154 attributes, else well-known names) map to
//! the canonical system labels; every other folder is a user label
//! `f:<path>`. On Gmail (X-GM-EXT-1) only `[Gmail]/All Mail`, Trash and
//! Spam are synced and a message's labels come from X-GM-LABELS.

use penguin_core::Label;
use penguin_provider::ids::{label_id_for_name, label_name_from_id, system};
use serde::{Deserialize, Serialize};

use crate::proto::utf7;
use crate::session::ListEntry;

/// Prefix of folder (and Gmail label) ids.
pub const FOLDER_PREFIX: &str = "f:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    /// `\All` (Gmail's All Mail; virtual on other servers).
    All,
    /// Virtual folders we never sync (`\Flagged`, `\Important`).
    Virtual,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Wire name (modified UTF-7), used in every command.
    pub raw: String,
    /// Decoded full path with the server's delimiter.
    pub path: String,
    /// Path as shown: `/` for nesting, a Courier-style `INBOX.` prefix removed.
    pub display: String,
    pub role: Role,
}

impl Folder {
    /// The label this folder gives its messages (None: archive-like).
    pub fn label(&self, gmail: bool) -> Option<String> {
        match self.role {
            Role::Inbox => Some(system::INBOX.into()),
            Role::Sent if !gmail => Some(system::SENT.into()),
            Role::Drafts if !gmail => Some(system::DRAFT.into()),
            Role::Trash => Some(system::TRASH.into()),
            Role::Junk => Some(system::SPAM.into()),
            Role::Other if !gmail => Some(label_id_for_name(FOLDER_PREFIX, &self.path)),
            _ => None,
        }
    }
}

const SENT: &[&str] = &[
    "sent",
    "sent items",
    "sent messages",
    "sent mail",
    "sent-mail",
    "gesendet",
    "gesendete elemente",
    "envoyés",
    "éléments envoyés",
    "enviados",
    "posta inviata",
];
const DRAFTS: &[&str] = &[
    "drafts",
    "draft",
    "entwürfe",
    "brouillons",
    "borradores",
    "bozze",
];
const TRASH: &[&str] = &[
    "trash",
    "deleted items",
    "deleted messages",
    "deleted",
    "bin",
    "papierkorb",
    "gelöschte elemente",
    "corbeille",
    "papelera",
    "cestino",
];
const JUNK: &[&str] = &[
    "junk",
    "spam",
    "bulk mail",
    "bulk",
    "junk e-mail",
    "junk email",
    "spamverdacht",
    "courrier indésirable",
    "correo no deseado",
];
const ARCHIVE: &[&str] = &["archive", "archives", "archiv", "archivio"];

fn attr_role(e: &ListEntry) -> Option<Role> {
    for (attr, role) in [
        ("\\Sent", Role::Sent),
        ("\\Drafts", Role::Drafts),
        ("\\Trash", Role::Trash),
        ("\\Junk", Role::Junk),
        ("\\Spam", Role::Junk),
        ("\\Archive", Role::Archive),
        ("\\All", Role::All),
        ("\\AllMail", Role::All),
        ("\\Flagged", Role::Virtual),
        ("\\Starred", Role::Virtual),
        ("\\Important", Role::Virtual),
    ] {
        if e.has_attr(attr) {
            return Some(role);
        }
    }
    None
}

fn name_role(path: &str, delimiter: Option<char>) -> Option<Role> {
    // Top level, or directly under INBOX (Courier-style namespaces).
    let parts: Vec<&str> = match delimiter {
        Some(d) => path.split(d).collect(),
        None => vec![path],
    };
    let leaf = match parts.as_slice() {
        [one] => *one,
        [first, leaf] if first.eq_ignore_ascii_case("INBOX") => *leaf,
        _ => return None,
    };
    let leaf = leaf.to_lowercase();
    for (names, role) in [
        (SENT, Role::Sent),
        (DRAFTS, Role::Drafts),
        (TRASH, Role::Trash),
        (JUNK, Role::Junk),
        (ARCHIVE, Role::Archive),
    ] {
        if names.contains(&leaf.as_str()) {
            return Some(role);
        }
    }
    None
}

/// The account's folders as the server listed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderSet {
    pub folders: Vec<Folder>,
    pub gmail: bool,
}

impl FolderSet {
    /// Classify a LIST answer. Non-selectable entries are dropped; each
    /// special role goes to one folder (an attribute beats a name).
    pub fn from_list(entries: &[ListEntry], gmail: bool) -> FolderSet {
        let selectable: Vec<&ListEntry> = entries
            .iter()
            .filter(|e| !e.has_attr("\\Noselect") && !e.has_attr("\\NonExistent"))
            .collect();
        let delimiter_of = |e: &ListEntry| e.delimiter.as_deref().and_then(|d| d.chars().next());
        let courier = selectable.iter().all(|e| {
            let d = delimiter_of(e).unwrap_or('/');
            e.raw.eq_ignore_ascii_case("INBOX")
                || e.raw.to_ascii_uppercase().starts_with(&format!("INBOX{d}"))
        }) && selectable.len() > 1;
        let mut folders: Vec<Folder> = Vec::new();
        let mut taken: Vec<Role> = Vec::new();
        // First pass: attributes.
        let mut roles: Vec<Option<Role>> = selectable
            .iter()
            .map(|e| {
                if e.raw.eq_ignore_ascii_case("INBOX") {
                    return Some(Role::Inbox);
                }
                let r = attr_role(e)?;
                if r != Role::Virtual && taken.contains(&r) {
                    return None;
                }
                taken.push(r);
                Some(r)
            })
            .collect();
        // Second pass: names, for roles no attribute claimed.
        for (i, e) in selectable.iter().enumerate() {
            if roles[i].is_some() {
                continue;
            }
            let path = utf7::decode(e.raw.as_bytes());
            if let Some(r) = name_role(&path, delimiter_of(e)) {
                if !taken.contains(&r) {
                    taken.push(r);
                    roles[i] = Some(r);
                }
            }
        }
        for (e, role) in selectable.iter().zip(roles) {
            let path = utf7::decode(e.raw.as_bytes());
            let d = delimiter_of(e);
            let mut display = path.clone();
            if courier {
                if let Some(d) = d {
                    let prefix = format!("INBOX{d}");
                    if display.len() > prefix.len()
                        && display[..prefix.len()].eq_ignore_ascii_case(&prefix)
                    {
                        display = display[prefix.len()..].to_string();
                    }
                }
            }
            if let Some(d) = d {
                if d != '/' {
                    display = display.replace(d, "/");
                }
            }
            let role = if e.raw.eq_ignore_ascii_case("INBOX") {
                Role::Inbox
            } else {
                role.unwrap_or(Role::Other)
            };
            folders.push(Folder {
                raw: e.raw.clone(),
                path,
                display,
                role,
            });
        }
        FolderSet { folders, gmail }
    }

    pub fn by_role(&self, role: Role) -> Option<&Folder> {
        self.folders.iter().find(|f| f.role == role)
    }

    pub fn by_raw(&self, raw: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.raw == raw)
    }

    /// The folder a user label id (`f:<path>`) names.
    pub fn by_label(&self, label_id: &str) -> Option<&Folder> {
        let path = label_name_from_id(FOLDER_PREFIX, label_id)?;
        self.folders
            .iter()
            .find(|f| f.role == Role::Other && f.path == path)
    }

    /// Folders synced, in backfill priority order: the inbox first, then
    /// sent and drafts, archive and user folders, spam and trash last. On
    /// Gmail: All Mail, then Spam and Trash.
    pub fn synced(&self) -> Vec<&Folder> {
        let rank = |r: Role| -> Option<u8> {
            if self.gmail {
                return match r {
                    Role::All => Some(0),
                    Role::Junk => Some(1),
                    Role::Trash => Some(2),
                    _ => None,
                };
            }
            match r {
                Role::Inbox => Some(0),
                Role::Sent => Some(1),
                Role::Drafts => Some(2),
                Role::Archive => Some(3),
                Role::Other => Some(4),
                Role::Junk => Some(5),
                Role::Trash => Some(6),
                Role::All | Role::Virtual => None,
            }
        };
        let mut out: Vec<(u8, &Folder)> = self
            .folders
            .iter()
            .filter_map(|f| rank(f.role).map(|r| (r, f)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.display.cmp(&b.1.display)));
        out.into_iter().map(|(_, f)| f).collect()
    }

    /// The folder IDLE watches: INBOX, or All Mail on Gmail.
    pub fn primary(&self) -> Option<&Folder> {
        if self.gmail {
            self.by_role(Role::All)
                .or_else(|| self.by_role(Role::Inbox))
        } else {
            self.by_role(Role::Inbox)
        }
    }

    /// Where archived mail goes: `\Archive`, else `\All`, else None (the
    /// caller creates "Archive").
    pub fn archive(&self) -> Option<&Folder> {
        self.by_role(Role::Archive)
            .or_else(|| self.by_role(Role::All))
    }

    /// The label list for `replace_labels`: the system labels this account
    /// has, then one user label per user folder (Gmail: per Gmail label).
    pub fn labels(&self, account_id: &str) -> Vec<Label> {
        let mut out = Vec::new();
        let mut system_ids: Vec<&str> = vec![system::INBOX];
        let has = |r| self.by_role(r).is_some();
        if self.gmail || has(Role::Sent) {
            system_ids.push(system::SENT);
        }
        if self.gmail || has(Role::Drafts) {
            system_ids.push(system::DRAFT);
        }
        if has(Role::Trash) {
            system_ids.push(system::TRASH);
        }
        if has(Role::Junk) {
            system_ids.push(system::SPAM);
        }
        system_ids.push(system::STARRED);
        system_ids.push(system::UNREAD);
        if self.gmail {
            system_ids.push(system::IMPORTANT);
        }
        for id in system_ids {
            out.push(Label {
                account_id: account_id.to_string(),
                id: id.to_string(),
                name: id.to_string(),
                kind: "system".into(),
                color: None,
                unread_count: None,
                hidden: false,
            });
        }
        for f in &self.folders {
            if f.role != Role::Other {
                continue;
            }
            out.push(Label {
                account_id: account_id.to_string(),
                id: label_id_for_name(FOLDER_PREFIX, &f.path),
                name: f.display.clone(),
                kind: "user".into(),
                color: None,
                unread_count: None,
                hidden: false,
            });
        }
        out
    }
}

/// Gmail X-GM-LABELS value (decoded) → Penguin label id. None for labels
/// with no Penguin meaning.
pub fn gmail_label_id(name: &str) -> Option<String> {
    if let Some(flag) = name.strip_prefix('\\') {
        return match flag.to_ascii_lowercase().as_str() {
            "inbox" => Some(system::INBOX.into()),
            "sent" => Some(system::SENT.into()),
            "draft" | "drafts" => Some(system::DRAFT.into()),
            "important" => Some(system::IMPORTANT.into()),
            "starred" => Some(system::STARRED.into()),
            "trash" => Some(system::TRASH.into()),
            "spam" | "junk" => Some(system::SPAM.into()),
            _ => None,
        };
    }
    if name.eq_ignore_ascii_case("INBOX") {
        return Some(system::INBOX.into());
    }
    if name.is_empty() {
        return None;
    }
    Some(label_id_for_name(FOLDER_PREFIX, name))
}

/// Penguin label id → the X-GM-LABELS value to STORE (None: not a Gmail
/// label, e.g. UNREAD/STARRED are flags).
pub fn gmail_label_value(label_id: &str) -> Option<String> {
    match label_id {
        system::INBOX => Some("\\Inbox".into()),
        system::IMPORTANT => Some("\\Important".into()),
        system::SENT => Some("\\Sent".into()),
        _ => label_name_from_id(FOLDER_PREFIX, label_id).map(|n| utf7::encode(&n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(raw: &str, attrs: &[&str], delim: &str) -> ListEntry {
        ListEntry {
            raw: raw.into(),
            attrs: attrs.iter().map(|s| s.to_string()).collect(),
            delimiter: Some(delim.into()),
        }
    }

    #[test]
    fn special_use_beats_names_and_names_fill_gaps() {
        let set = FolderSet::from_list(
            &[
                entry("INBOX", &["\\HasNoChildren"], "/"),
                entry("Sent Messages", &["\\Sent"], "/"),
                entry("Sent", &[], "/"),
                entry("Deleted Messages", &[], "/"),
                entry("Bulk Mail", &[], "/"),
                entry("Archive", &["\\Archive"], "/"),
                entry("Clients/Acme", &[], "/"),
                entry("Clients", &["\\Noselect"], "/"),
                entry("Entw&APw-rfe", &[], "/"),
            ],
            false,
        );
        let role = |raw: &str| set.by_raw(raw).map(|f| f.role);
        assert_eq!(role("INBOX"), Some(Role::Inbox));
        assert_eq!(role("Sent Messages"), Some(Role::Sent));
        assert_eq!(role("Sent"), Some(Role::Other));
        assert_eq!(role("Deleted Messages"), Some(Role::Trash));
        assert_eq!(role("Bulk Mail"), Some(Role::Junk));
        assert_eq!(role("Entw&APw-rfe"), Some(Role::Drafts));
        assert_eq!(role("Clients"), None);
        let acme = set.by_raw("Clients/Acme").unwrap();
        assert_eq!(acme.label(false).as_deref(), Some("f:Clients/Acme"));
        assert_eq!(set.by_label("f:Clients/Acme").unwrap().raw, "Clients/Acme");
        let order: Vec<&str> = set.synced().iter().map(|f| f.raw.as_str()).collect();
        assert_eq!(order[0], "INBOX");
        assert_eq!(order[1], "Sent Messages");
        assert_eq!(*order.last().unwrap(), "Deleted Messages");
        let labels = set.labels("sam@mail.example");
        assert!(
            penguin_provider::conformance::check_labels("sam@mail.example", &labels).is_empty()
        );
        assert!(labels
            .iter()
            .any(|l| l.id == "f:Clients/Acme" && l.name == "Clients/Acme"));
        assert!(labels.iter().any(|l| l.id == "SPAM"));
    }

    #[test]
    fn courier_namespaces_show_without_the_inbox_prefix() {
        let set = FolderSet::from_list(
            &[
                entry("INBOX", &[], "."),
                entry("INBOX.Sent", &[], "."),
                entry("INBOX.Trash", &[], "."),
                entry("INBOX.Clients.Acme", &[], "."),
            ],
            false,
        );
        assert_eq!(set.by_role(Role::Sent).unwrap().raw, "INBOX.Sent");
        let acme = set.by_raw("INBOX.Clients.Acme").unwrap();
        assert_eq!(acme.display, "Clients/Acme");
        assert_eq!(acme.label(false).as_deref(), Some("f:INBOX.Clients.Acme"));
    }

    #[test]
    fn gmail_syncs_all_mail_spam_and_trash() {
        let set = FolderSet::from_list(
            &[
                entry("INBOX", &[], "/"),
                entry("[Gmail]", &["\\Noselect"], "/"),
                entry("[Gmail]/All Mail", &["\\All"], "/"),
                entry("[Gmail]/Sent Mail", &["\\Sent"], "/"),
                entry("[Gmail]/Drafts", &["\\Drafts"], "/"),
                entry("[Gmail]/Spam", &["\\Junk"], "/"),
                entry("[Gmail]/Bin", &["\\Trash"], "/"),
                entry("[Gmail]/Starred", &["\\Flagged"], "/"),
                entry("[Gmail]/Important", &["\\Important"], "/"),
                entry("Receipts", &[], "/"),
            ],
            true,
        );
        let order: Vec<&str> = set.synced().iter().map(|f| f.raw.as_str()).collect();
        assert_eq!(
            order,
            vec!["[Gmail]/All Mail", "[Gmail]/Spam", "[Gmail]/Bin"]
        );
        assert_eq!(set.primary().unwrap().raw, "[Gmail]/All Mail");
        let labels = set.labels("sam@gmail.example");
        assert!(labels.iter().any(|l| l.id == "f:Receipts"));
        assert!(labels.iter().any(|l| l.id == "IMPORTANT"));
        assert!(!labels.iter().any(|l| l.id.contains("Gmail")));
        assert!(
            penguin_provider::conformance::check_labels("sam@gmail.example", &labels).is_empty()
        );
    }

    #[test]
    fn gmail_labels_map_both_ways() {
        assert_eq!(gmail_label_id("\\Inbox").as_deref(), Some("INBOX"));
        assert_eq!(gmail_label_id("\\Important").as_deref(), Some("IMPORTANT"));
        assert_eq!(gmail_label_id("\\Muted"), None);
        assert_eq!(
            gmail_label_id("Two words").as_deref(),
            Some("f:Two%20words")
        );
        assert_eq!(gmail_label_value("INBOX").as_deref(), Some("\\Inbox"));
        assert_eq!(
            gmail_label_value("f:Two%20words").as_deref(),
            Some("Two words")
        );
        assert_eq!(
            gmail_label_value("f:%C3%A9t%C3%A9").as_deref(),
            Some("&AOk-t&AOk-")
        );
        assert_eq!(gmail_label_value("UNREAD"), None);
    }
}
