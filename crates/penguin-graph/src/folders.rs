//! Graph mail folders onto Penguin's label model (docs/PROVIDERS-IMPL.md §4).
//!
//! - Well-known folders: inbox → INBOX, sentitems → SENT, drafts → DRAFT,
//!   deleteditems → TRASH, junkemail → SPAM, archive → no label (archived
//!   = not in the inbox). Folders under Deleted Items are TRASH and under
//!   Junk Email SPAM (a deleted folder keeps its id and moves there).
//! - Outbox, Conversation History, Sync Issues, Scheduled and search
//!   folders are not mail Penguin shows: not synced.
//! - Every other folder is a user label `f:<folder id>` named by its path
//!   ("Inbox/Receipts"). A message lives in one folder; "adding" such a
//!   label is a move (`Capabilities::folders`).
//! - Categories are user labels `c:<name>` (several per message).

use std::collections::{BTreeSet, HashMap};

use penguin_core::Label;
use penguin_provider::ids::{label_id_for_name, label_name_from_id, system};

use crate::wire::MailFolder;

/// Well-known folder names Penguin resolves to ids.
pub(crate) const WELL_KNOWN: [&str; 11] = [
    "inbox",
    "sentitems",
    "drafts",
    "deleteditems",
    "junkemail",
    "archive",
    "outbox",
    "conversationhistory",
    "syncissues",
    "scheduled",
    "searchfolders",
];

/// What a folder means for labels and sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// INBOX, SENT, DRAFT, TRASH or SPAM.
    System(&'static str),
    /// Archive: messages here carry no folder label.
    Archive,
    /// A user folder: label `f:<id>`.
    User,
    /// Not synced (Outbox, Sync Issues, …).
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: String,
    /// Display path, `/` between levels.
    pub name: String,
    pub parent_id: Option<String>,
    pub role: Role,
    pub total: u64,
}

impl Folder {
    /// The label a message in this folder carries (None: archived or skipped).
    pub fn label(&self) -> Option<String> {
        match &self.role {
            Role::System(l) => Some((*l).to_string()),
            Role::User => Some(folder_label_id(&self.id)),
            Role::Archive | Role::Skip => None,
        }
    }
}

pub fn folder_label_id(folder_id: &str) -> String {
    label_id_for_name("f:", folder_id)
}

pub fn category_label_id(name: &str) -> String {
    label_id_for_name("c:", name)
}

/// The folder id behind an `f:` label.
pub fn folder_id_of_label(label: &str) -> Option<String> {
    label_name_from_id("f:", label)
}

/// The category name behind a `c:` label.
pub fn category_of_label(label: &str) -> Option<String> {
    label_name_from_id("c:", label)
}

/// Labels that encode where a message is (at most one per message).
pub fn is_folder_label(label: &str) -> bool {
    matches!(
        label,
        system::INBOX | system::SENT | system::DRAFT | system::TRASH | system::SPAM
    ) || label.starts_with("f:")
}

/// One account's folders, in sync order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderMap {
    folders: Vec<Folder>,
    by_id: HashMap<String, usize>,
    /// well-known name → folder id (only those the mailbox has).
    well_known: HashMap<String, String>,
    /// Master category names.
    pub categories: Vec<String>,
    /// The folder listing stopped early (too many folders): folders
    /// missing from it may still exist.
    pub truncated: bool,
}

impl FolderMap {
    /// Build from every folder (any order, parents before or after
    /// children) and the resolved well-known ids.
    pub(crate) fn build(
        raw: Vec<MailFolder>,
        well_known: HashMap<String, String>,
        categories: Vec<String>,
    ) -> FolderMap {
        let by_raw: HashMap<&str, &MailFolder> = raw.iter().map(|f| (f.id.as_str(), f)).collect();
        let wk_of: HashMap<&str, &str> = well_known
            .iter()
            .map(|(name, id)| (id.as_str(), name.as_str()))
            .collect();
        let has_archive = well_known.contains_key("archive");
        // Roots are folders whose parent isn't in the list (msgfolderroot).
        let is_root = |f: &MailFolder| {
            f.parent_folder_id
                .as_deref()
                .is_none_or(|p| !by_raw.contains_key(p))
        };
        let own_role = |f: &MailFolder| -> Option<Role> {
            match wk_of.get(f.id.as_str()).copied() {
                Some("inbox") => Some(Role::System(system::INBOX)),
                Some("sentitems") => Some(Role::System(system::SENT)),
                Some("drafts") => Some(Role::System(system::DRAFT)),
                Some("deleteditems") => Some(Role::System(system::TRASH)),
                Some("junkemail") => Some(Role::System(system::SPAM)),
                Some("archive") => Some(Role::Archive),
                Some(_) => Some(Role::Skip),
                None if !has_archive
                    && is_root(f)
                    && f.display_name.eq_ignore_ascii_case("Archive") =>
                {
                    Some(Role::Archive)
                }
                None => None,
            }
        };
        let mut folders = Vec::with_capacity(raw.len());
        for f in &raw {
            // Walk up: a well-known ancestor decides trash/spam/skip.
            let mut role = own_role(f);
            let mut path = vec![f.display_name.clone()];
            let mut cur = f.parent_folder_id.as_deref();
            let mut guard = 0;
            while let Some(pid) = cur {
                guard += 1;
                let Some(parent) = by_raw.get(pid) else { break };
                if guard > 64 {
                    break;
                }
                path.push(parent.display_name.clone());
                if role.is_none() {
                    match own_role(parent) {
                        Some(Role::System(l)) if l == system::TRASH || l == system::SPAM => {
                            role = Some(Role::System(l))
                        }
                        Some(Role::Skip) => role = Some(Role::Skip),
                        _ => {}
                    }
                }
                cur = parent.parent_folder_id.as_deref();
            }
            path.reverse();
            folders.push(Folder {
                id: f.id.clone(),
                name: path.join("/"),
                parent_id: f.parent_folder_id.clone(),
                role: role.unwrap_or(Role::User),
                total: f.total_item_count,
            });
        }
        let rank = |f: &Folder| -> (u8, String) {
            let r = match &f.role {
                Role::System(l) if *l == system::INBOX => 0,
                Role::System(l) if *l == system::SENT => 1,
                Role::System(l) if *l == system::DRAFT => 2,
                Role::Archive => 3,
                Role::User => 4,
                Role::System(l) if *l == system::SPAM => 5,
                Role::System(_) => 6,
                Role::Skip => 7,
            };
            (r, f.name.to_lowercase())
        };
        folders.sort_by_key(rank);
        let by_id = folders
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id.clone(), i))
            .collect();
        FolderMap {
            folders,
            by_id,
            well_known,
            categories,
            truncated: false,
        }
    }

    pub fn get(&self, id: &str) -> Option<&Folder> {
        self.by_id.get(id).map(|&i| &self.folders[i])
    }

    /// Folders whose mail Penguin syncs, in sync order (inbox first).
    pub fn synced(&self) -> impl Iterator<Item = &Folder> {
        self.folders.iter().filter(|f| f.role != Role::Skip)
    }

    /// Junk Email or a folder under it.
    pub fn is_spam(&self, id: &str) -> bool {
        self.get(id)
            .is_some_and(|f| f.role == Role::System(system::SPAM))
    }

    pub fn is_synced(&self, id: &str) -> bool {
        self.get(id).is_some_and(|f| f.role != Role::Skip)
    }

    /// The id of a well-known folder (`inbox`, `archive`, …) if the mailbox has it.
    pub fn well_known(&self, name: &str) -> Option<&str> {
        self.well_known.get(name).map(String::as_str)
    }

    /// The destination for a folder label: system labels as well-known
    /// names (Graph accepts them as `destinationId`), `f:` labels as ids.
    pub fn destination_for_label(&self, label: &str) -> Option<String> {
        match label {
            system::INBOX => Some("inbox".into()),
            system::SENT => Some("sentitems".into()),
            system::DRAFT => Some("drafts".into()),
            system::TRASH => Some("deleteditems".into()),
            system::SPAM => Some("junkemail".into()),
            _ => folder_id_of_label(label).filter(|id| self.get(id).is_some()),
        }
    }

    /// The folder id a destination name or id stands for.
    pub fn resolve(&self, destination: &str) -> Option<String> {
        if self.by_id.contains_key(destination) {
            return Some(destination.to_string());
        }
        self.well_known(destination).map(str::to_string)
    }

    /// The account's label list: system labels, user folders, categories
    /// (master list plus any seen on messages).
    pub fn labels(&self, account_id: &str, seen_categories: &[String]) -> Vec<Label> {
        let mut out: Vec<Label> = [
            system::INBOX,
            system::SENT,
            system::DRAFT,
            system::TRASH,
            system::SPAM,
            system::STARRED,
            system::UNREAD,
        ]
        .into_iter()
        .map(|id| Label {
            account_id: account_id.to_string(),
            id: id.to_string(),
            name: id.to_string(),
            kind: "system".into(),
            color: None,
            unread_count: None,
            hidden: false,
        })
        .collect();
        for f in self.folders.iter().filter(|f| f.role == Role::User) {
            out.push(Label {
                account_id: account_id.to_string(),
                id: folder_label_id(&f.id),
                name: f.name.clone(),
                kind: "user".into(),
                color: None,
                unread_count: None,
                hidden: false,
            });
        }
        let names: BTreeSet<&str> = self
            .categories
            .iter()
            .chain(seen_categories)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        for name in names {
            out.push(Label {
                account_id: account_id.to_string(),
                id: category_label_id(name),
                name: name.to_string(),
                kind: "user".into(),
                color: None,
                unread_count: None,
                hidden: false,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn raw(id: &str, name: &str, parent: &str) -> MailFolder {
        MailFolder {
            id: id.into(),
            display_name: name.into(),
            parent_folder_id: Some(parent.into()),
            ..MailFolder::default()
        }
    }

    fn sample() -> FolderMap {
        let folders = vec![
            raw("F-inbox", "Inbox", "ROOT"),
            raw("F-sent", "Sent Items", "ROOT"),
            raw("F-drafts", "Drafts", "ROOT"),
            raw("F-trash", "Deleted Items", "ROOT"),
            raw("F-junk", "Junk Email", "ROOT"),
            raw("F-archive", "Archive", "ROOT"),
            raw("F-outbox", "Outbox", "ROOT"),
            raw("F-receipts", "Receipts", "F-inbox"),
            raw("F-2024", "2024", "F-receipts"),
            raw("F-old", "Old Project", "F-trash"),
            raw("F-clients", "Clients", "ROOT"),
        ];
        let wk = [
            ("inbox", "F-inbox"),
            ("sentitems", "F-sent"),
            ("drafts", "F-drafts"),
            ("deleteditems", "F-trash"),
            ("junkemail", "F-junk"),
            ("archive", "F-archive"),
            ("outbox", "F-outbox"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        FolderMap::build(folders, wk, vec!["Red category".into()])
    }

    #[test]
    fn well_known_folders_are_system_labels() {
        let m = sample();
        let label = |id: &str| m.get(id).unwrap().label();
        assert_eq!(label("F-inbox").as_deref(), Some("INBOX"));
        assert_eq!(label("F-sent").as_deref(), Some("SENT"));
        assert_eq!(label("F-drafts").as_deref(), Some("DRAFT"));
        assert_eq!(label("F-trash").as_deref(), Some("TRASH"));
        assert_eq!(label("F-junk").as_deref(), Some("SPAM"));
        assert_eq!(label("F-archive"), None);
        assert_eq!(m.get("F-outbox").unwrap().role, Role::Skip);
        // A deleted folder's mail is trash; nested user folders keep paths.
        assert_eq!(label("F-old").as_deref(), Some("TRASH"));
        assert_eq!(m.get("F-2024").unwrap().name, "Inbox/Receipts/2024");
        assert_eq!(label("F-2024").as_deref(), Some("f:F-2024"));
        assert!(!m.is_synced("F-outbox") && m.is_synced("F-clients"));
        // Sync order: inbox first, trash and skipped last.
        let order: Vec<&str> = m.synced().map(|f| f.id.as_str()).collect();
        assert_eq!(order[0], "F-inbox");
        assert_eq!(order[1], "F-sent");
        assert!(
            order.iter().position(|i| *i == "F-trash")
                > order.iter().position(|i| *i == "F-clients")
        );
    }

    #[test]
    fn label_list_passes_the_conformance_checks() {
        let m = sample();
        let labels = m.labels(
            "sam@outlook.example",
            &["Blue category".into(), "Red category".into()],
        );
        assert!(
            penguin_provider::conformance::check_labels("sam@outlook.example", &labels).is_empty()
        );
        let ids: Vec<&str> = labels.iter().map(|l| l.id.as_str()).collect();
        assert!(ids.contains(&"c:Red%20category") && ids.contains(&"c:Blue%20category"));
        assert!(ids.contains(&"f:F-receipts") && !ids.contains(&"f:F-old"));
        assert_eq!(ids.iter().filter(|i| **i == "c:Red%20category").count(), 1);
    }

    #[test]
    fn destinations() {
        let m = sample();
        assert_eq!(m.destination_for_label("INBOX").as_deref(), Some("inbox"));
        assert_eq!(
            m.destination_for_label("f:F-clients").as_deref(),
            Some("F-clients")
        );
        assert_eq!(m.destination_for_label("f:gone"), None);
        assert_eq!(m.resolve("archive").as_deref(), Some("F-archive"));
        assert_eq!(folder_id_of_label("f:F-2024").as_deref(), Some("F-2024"));
        assert_eq!(
            category_of_label("c:Red%20category").as_deref(),
            Some("Red category")
        );
    }

    #[test]
    fn a_root_folder_named_archive_is_the_archive_when_there_is_no_well_known_one() {
        let m = FolderMap::build(
            vec![raw("A", "Archive", "ROOT"), raw("I", "Inbox", "ROOT")],
            [("inbox".to_string(), "I".to_string())]
                .into_iter()
                .collect(),
            vec![],
        );
        assert_eq!(m.get("A").unwrap().role, Role::Archive);
    }
}
