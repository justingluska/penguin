//! Split Inbox, the settings half: the splits a user keeps (an ordered list
//! of named search queries), the ones a new Split Inbox starts with, how a
//! saved list is cleaned up, and which [`SplitFilter`] a tab asks the store
//! for. Listing and counting a split is `store_split.rs`.
//!
//! Shared by the Mac (src-tauri settings.rs keeps `inboxSplits` in its
//! settings file) and the iPhone (penguin-ffi keeps its own list), so both
//! start with the same tabs and read a tab the same way. The Mac's UI has
//! the same tab rules in TypeScript (apps/desktop/src/app/splits.ts).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::types::SplitFilter;

/// One tab of the Split Inbox.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct InboxSplit {
    /// 1–64 of [A-Za-z0-9_-], chosen by the UI.
    pub id: String,
    /// Trimmed, ≤30 chars; blank = the query.
    pub name: String,
    /// What it holds, in the search language (≤500 chars).
    pub query: String,
    /// Leave the tab out while it has nothing in it.
    pub hide_when_empty: bool,
}

/// At most this many splits (plus Other).
pub const MAX_INBOX_SPLITS: usize = 12;
pub const MAX_INBOX_SPLIT_NAME: usize = 30;
/// Longest split query (the same bound as a saved search's).
pub const MAX_INBOX_SPLIT_QUERY: usize = 500;

/// Other's id: never a valid split id (ids are [A-Za-z0-9_-] only).
pub const OTHER_SPLIT: &str = "~other";

/// The splits a new Split Inbox starts with (the Mac UI's presets of the
/// same names): Important (Gmail's own marker, minus bulk mail and
/// invitations), Calendar and News. Other is always there.
pub fn default_inbox_splits() -> Vec<InboxSplit> {
    let split = |id: &str, name: &str, query: &str| InboxSplit {
        id: id.into(),
        name: name.into(),
        query: query.into(),
        hide_when_empty: false,
    };
    vec![
        split(
            "important",
            "Important",
            "is:important -is:newsletter -has:invite",
        ),
        split(
            "calendar",
            "Calendar",
            "has:invite OR from:calendar-notification@google.com OR from:@calendly.com",
        ),
        split("news", "News", "is:newsletter"),
    ]
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Whether `id` can name a split: 1–64 of [A-Za-z0-9_-].
pub fn valid_split_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Valid ids, deduped, names and queries trimmed and bounded, no empty
/// query, at most [`MAX_INBOX_SPLITS`].
pub fn normalize_inbox_splits(splits: Vec<InboxSplit>) -> Vec<InboxSplit> {
    let mut out: Vec<InboxSplit> = Vec::new();
    let mut ids = HashSet::new();
    for mut sp in splits {
        let query: String = truncate_chars(
            &sp.query.split_whitespace().collect::<Vec<_>>().join(" "),
            MAX_INBOX_SPLIT_QUERY,
        );
        if !valid_split_id(&sp.id) || query.is_empty() || !ids.insert(sp.id.clone()) {
            continue;
        }
        let name = sp.name.split_whitespace().collect::<Vec<_>>().join(" ");
        sp.name = truncate_chars(
            if name.is_empty() { &query } else { &name },
            MAX_INBOX_SPLIT_NAME,
        );
        sp.query = query;
        out.push(sp);
        if out.len() == MAX_INBOX_SPLITS {
            break;
        }
    }
    out
}

/// The list filter for tab `id`: its query, minus what the splits before it
/// claim. Other ([`OTHER_SPLIT`], or an id no longer in the list) is every
/// conversation none of them claims.
pub fn split_filter_for(splits: &[InboxSplit], id: &str) -> SplitFilter {
    match splits.iter().position(|s| s.id == id) {
        Some(i) => SplitFilter {
            include: Some(splits[i].query.clone()),
            exclude: splits[..i].iter().map(|s| s.query.clone()).collect(),
        },
        None => SplitFilter {
            include: None,
            exclude: splits.iter().map(|s| s.query.clone()).collect(),
        },
    }
}

/// A fresh split id from a name ("VIP" → "vip", "vip-2" when taken).
pub fn new_split_id(name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c);
        } else if !base.ends_with('-') {
            base.push('-');
        }
    }
    let base: String = base.trim_matches('-').chars().take(40).collect();
    let base = if base.is_empty() {
        "split".to_string()
    } else {
        base
    };
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// A ready-made split (Add a split). Each is just a query, so everything a
/// preset does can be edited after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPreset {
    pub key: &'static str,
    pub name: &'static str,
    /// One line in the Add menu.
    pub blurb: &'static str,
    /// The query for your accounts; None when it can't be made (Team with
    /// no work domain; VIP, which is made from people with [`vip_query`]).
    pub query: Option<String>,
}

/// Consumer mail providers: an account on one of these isn't a work domain
/// (the Mac's `isConsumerAddress`).
fn is_consumer_address(email: &str) -> bool {
    const CONSUMER: [&str; 14] = [
        "gmail",
        "googlemail",
        "icloud",
        "me",
        "mac",
        "outlook",
        "hotmail",
        "live",
        "yahoo",
        "proton",
        "protonmail",
        "pm",
        "fastmail",
        "aol",
    ];
    let domain = email.split('@').nth(1).unwrap_or_default();
    let first = domain.split('.').next().unwrap_or_default().to_lowercase();
    CONSUMER.contains(&first.as_str())
}

/// The presets (the Mac's features/split/presets.ts, same names and
/// queries), given your accounts' addresses (Team needs them). The first
/// three are what [`default_inbox_splits`] starts with.
pub fn split_presets(my_emails: &[String]) -> Vec<SplitPreset> {
    let mut domains: Vec<String> = Vec::new();
    for e in my_emails.iter().filter(|e| !is_consumer_address(e)) {
        if let Some(d) = e.split('@').nth(1).map(str::to_lowercase) {
            if !d.is_empty() && !domains.contains(&d) {
                domains.push(d);
            }
        }
    }
    let team = (!domains.is_empty()).then(|| {
        domains
            .iter()
            .map(|d| format!("from:@{d}"))
            .collect::<Vec<_>>()
            .join(" OR ")
    });
    let p = |key, name, blurb, query: Option<String>| SplitPreset {
        key,
        name,
        blurb,
        query,
    };
    let defaults = default_inbox_splits();
    vec![
        p("important", "Important", "What Gmail marks important, without newsletters or invitations. Gmail accounts only.", Some(defaults[0].query.clone())),
        p("calendar", "Calendar", "Invitations and updates from Google Calendar and Calendly.", Some(defaults[1].query.clone())),
        p("news", "News", "Newsletters and other bulk mail (a List-Unsubscribe header or a promotions/updates category).", Some(defaults[2].query.clone())),
        p("people", "People", "Mail from people you've written to, without bulk mail. Works for every account.", Some("is:known-sender -is:newsletter".into())),
        p("vip", "VIP", "The people you pick.", None),
        p("team", "Team", "People at your work domains (your non-personal accounts).", team),
        p("tools", "Notifications", "Automated mail from apps and services: no-reply and notification senders.", Some("from:noreply OR from:no-reply OR from:notifications OR from:notification OR from:alerts".into())),
        p("files", "Attachments", "Conversations with a file attached.", Some("has:attachment -is:newsletter".into())),
    ]
}

/// A VIP split's query for these people.
pub fn vip_query(emails: &[String]) -> String {
    emails
        .iter()
        .map(|e| format!("from:{}", e.trim().to_lowercase()))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn presets_know_your_work_domains() {
        let p = split_presets(&[
            "sam@gmail.com".into(),
            "sam@northwind.example".into(),
            "s@northwind.example".into(),
        ]);
        let team = p.iter().find(|x| x.key == "team").unwrap();
        assert_eq!(team.query.as_deref(), Some("from:@northwind.example"));
        assert!(split_presets(&["sam@gmail.com".into()])
            .iter()
            .find(|x| x.key == "team")
            .unwrap()
            .query
            .is_none());
        assert_eq!(
            p[0].query.as_deref(),
            Some(default_inbox_splits()[0].query.as_str())
        );
        assert_eq!(
            vip_query(&[" A@x.example".into(), "b@y.example".into()]),
            "from:a@x.example OR from:b@y.example"
        );
    }

    use super::*;

    fn split(id: &str, query: &str) -> InboxSplit {
        InboxSplit {
            id: id.into(),
            name: String::new(),
            query: query.into(),
            hide_when_empty: false,
        }
    }

    #[test]
    fn defaults_are_important_calendar_news() {
        let d = default_inbox_splits();
        let ids: Vec<&str> = d.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["important", "calendar", "news"]);
        assert_eq!(normalize_inbox_splits(d.clone()), d, "already normal");
    }

    #[test]
    fn normalize_drops_bad_ids_duplicates_and_empty_queries() {
        let out = normalize_inbox_splits(vec![
            split("vip", "  from:a@x.example   OR from:b@x.example "),
            split("vip", "from:c@x.example"),
            split("bad id", "is:unread"),
            split("", "is:unread"),
            split("empty", "   "),
            InboxSplit {
                name: "  A   very long name that goes on and on and on  ".into(),
                ..split("long", "is:starred")
            },
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].query, "from:a@x.example OR from:b@x.example");
        assert_eq!(
            out[0].name,
            out[0].query.chars().take(30).collect::<String>()
        );
        assert_eq!(out[1].name, "A very long name that goes on ");
        let many: Vec<InboxSplit> = (0..20).map(|i| split(&format!("s{i}"), "x")).collect();
        assert_eq!(normalize_inbox_splits(many).len(), MAX_INBOX_SPLITS);
    }

    #[test]
    fn a_tab_excludes_what_earlier_tabs_claim() {
        let splits = default_inbox_splits();
        let f = split_filter_for(&splits, "calendar");
        assert_eq!(f.include.as_deref(), Some(splits[1].query.as_str()));
        assert_eq!(f.exclude, vec![splits[0].query.clone()]);
        let other = split_filter_for(&splits, OTHER_SPLIT);
        assert_eq!(other.include, None);
        assert_eq!(other.exclude.len(), 3);
        assert_eq!(split_filter_for(&splits, "gone"), other);
        assert!(!valid_split_id(OTHER_SPLIT));
    }

    #[test]
    fn new_ids_are_slugs_and_unique() {
        assert_eq!(new_split_id("VIP", &[]), "vip");
        assert_eq!(new_split_id("VIP", &["vip".into()]), "vip-2");
        assert_eq!(
            new_split_id("VIP", &["vip".into(), "vip-2".into()]),
            "vip-3"
        );
        assert_eq!(new_split_id("  Team / Ops! ", &[]), "team-ops");
        assert_eq!(new_split_id("✨", &[]), "split");
    }
}
