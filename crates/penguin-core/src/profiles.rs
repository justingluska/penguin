//! Profiles: named groups of accounts (`Settings.profiles` on the Mac and
//! the iPhone, the same JSON). The account switcher narrows the inbox,
//! counts and search to one (`account_scope`); which one is active is each
//! app's UI state, not a setting. Shared so both apps check them the same
//! way.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Bound on account profiles (⌃1–⌃9 reach the first nine on the Mac).
pub const MAX_PROFILES: usize = 20;
pub const MAX_PROFILE_NAME: usize = 40;
pub const MAX_PROFILE_ACCOUNTS: usize = 50;
pub const MAX_EMOJI_CHARS: usize = 8;
pub const DEFAULT_PROFILE_COLOR: &str = "#8d5ffe";

/// A named group of accounts (a company, a role). The UI can narrow the
/// inbox, search, labels and sync to one profile; the active profile is UI
/// state, not a setting. An account may be in several profiles.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    /// Stable id chosen by the UI: 1–64 of [A-Za-z0-9_-].
    pub id: String,
    pub name: String,
    /// `#rrggbb`, from the account palette.
    pub color: String,
    /// Member account ids (lowercased emails), in the user's order.
    pub account_ids: Vec<String>,
    pub emoji: Option<String>,
}

/// Drop profiles without a valid id (and repeated ids), trim and bound names,
/// fall back to the default color for anything not `#rrggbb`, dedupe members
/// (lowercased, order kept) and keep at most MAX_PROFILES.
pub fn normalize_profiles(profiles: Vec<Profile>) -> Vec<Profile> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for mut p in profiles {
        let id_ok = !p.id.is_empty()
            && p.id.len() <= 64
            && p.id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !id_ok || !seen.insert(p.id.clone()) {
            continue;
        }
        let name: String = p.name.split_whitespace().collect::<Vec<_>>().join(" ");
        p.name = if name.is_empty() {
            "Untitled".into()
        } else {
            name.chars().take(MAX_PROFILE_NAME).collect()
        };
        let hex = p.color.len() == 7
            && p.color.starts_with('#')
            && p.color[1..].chars().all(|c| c.is_ascii_hexdigit());
        p.color = if hex {
            p.color.to_ascii_lowercase()
        } else {
            DEFAULT_PROFILE_COLOR.into()
        };
        p.emoji = p
            .emoji
            .map(|e| e.trim().chars().take(MAX_EMOJI_CHARS).collect::<String>())
            .filter(|e| {
                !e.is_empty()
                    && !e
                        .chars()
                        .any(|c| c.is_control() || c.is_ascii_alphanumeric())
            });
        let mut members = Vec::new();
        for a in p.account_ids {
            let a = a.trim().to_lowercase();
            if !a.is_empty() && !members.contains(&a) {
                members.push(a);
            }
        }
        members.truncate(MAX_PROFILE_ACCOUNTS);
        p.account_ids = members;
        out.push(p);
        if out.len() == MAX_PROFILES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_checked() {
        let p =
            |id: &str, name: &str, color: &str, accounts: &[&str], emoji: Option<&str>| Profile {
                id: id.into(),
                name: name.into(),
                color: color.into(),
                account_ids: accounts.iter().map(|a| a.to_string()).collect(),
                emoji: emoji.map(str::to_string),
            };
        let out = normalize_profiles(vec![
            p(
                "work",
                "  Work   stuff ",
                "#ABCDEF",
                &[" Bo@Example.com", "bo@example.com"],
                Some("A"),
            ),
            p("work", "again", "", &[], None),
            p("bad id!", "x", "", &[], None),
            p("home", "", "blue", &[], Some(" 🏠 ")),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "Work stuff");
        assert_eq!(out[0].color, "#abcdef");
        assert_eq!(out[0].account_ids, ["bo@example.com"]);
        assert_eq!(out[0].emoji, None, "a letter isn't an emoji");
        assert_eq!(out[1].name, "Untitled");
        assert_eq!(out[1].color, DEFAULT_PROFILE_COLOR);
        assert_eq!(out[1].emoji.as_deref(), Some("🏠"));
        let many: Vec<Profile> = (0..30)
            .map(|i| p(&format!("p{i}"), "n", "", &[], None))
            .collect();
        assert_eq!(normalize_profiles(many).len(), MAX_PROFILES);
    }
}
