//! What a user keeps for writing mail, as data: snippets, instant replies,
//! the Undo send window and the send-later morning hour. The Mac keeps them
//! in its settings file (src-tauri settings.rs re-exports these types); the
//! iPhone keeps its own copy (penguin-ffi `compose_prefs`). Using them is
//! [`crate::snippets`] (filling a snippet in) and [`crate::when`] (send
//! later times).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// A composer snippet. The body may use {first_name}, {last_name},
/// {full_name}, {company}, {sender_name}, {my_name}, {my_first_name},
/// {date} and {cursor}; [`crate::snippets::expand_snippet`] fills them in
/// (the Mac's UI does the same in features/compose/snippets.ts).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Snippet {
    /// Stable id chosen by the UI: 1–64 of [A-Za-z0-9_-].
    pub id: String,
    /// Typed after ";": 1–32 of [a-z0-9_-], unique.
    pub trigger: String,
    pub title: String,
    pub body: String,
    pub uses: u32,
    /// Subject it fills in when the message has none yet; empty = none. ≤ 200 chars.
    pub subject: String,
    /// Addresses ("Name <a@b.example>" or a bare address) it adds to Cc / Bcc. ≤ 20 each.
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    /// Files it attaches (kept on the Mac by `save_snippet_file`). ≤ 10.
    pub attachments: Vec<SnippetFile>,
}

/// A file kept with a snippet, stored at `<data dir>/snippets/<id>` on the
/// Mac (src/snippet_files.rs); `id` is the sha256 of its bytes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SnippetFile {
    /// 64 lowercase hex.
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

/// Instant replies (Settings → Compose).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct InstantReplies {
    /// Offer the one-liners. On by default.
    pub enabled: bool,
    /// The one-liners, in order (⌃1…⌃9). ≤ 9, each ≤ 200 chars.
    pub replies: Vec<String>,
    /// Also suggest up to three replies to the conversation with Apple's
    /// on-device model when a reply opens. Off by default.
    pub ai_suggestions: bool,
}

impl Default for InstantReplies {
    fn default() -> Self {
        InstantReplies {
            enabled: true,
            replies: vec![
                "Sounds good, thanks!".into(),
                "Thanks, got it.".into(),
                "Let me check and get back to you.".into(),
            ],
            ai_suggestions: false,
        }
    }
}

pub const MAX_INSTANT_REPLIES: usize = 9;
pub const MAX_INSTANT_REPLY: usize = 200;
pub const MAX_SNIPPETS: usize = 200;
pub const MAX_SNIPPET_TITLE: usize = 80;
pub const MAX_SNIPPET_BODY: usize = 10_000;
pub const MAX_SNIPPET_SUBJECT: usize = 200;
pub const MAX_SNIPPET_ADDRESSES: usize = 20;
pub const MAX_SNIPPET_FILES: usize = 10;
/// Undo send window in seconds: 0 (off), 5, 10 (default), 20 or 30.
pub const UNDO_SEND_CHOICES: [u8; 5] = [0, 5, 10, 20, 30];
pub const UNDO_SEND_DEFAULT: u8 = 10;
/// The hour (local) "Tomorrow morning" and "Monday morning" mean: 5–11.
pub const SEND_LATER_HOURS: std::ops::RangeInclusive<u8> = 5..=11;
pub const SEND_LATER_HOUR_DEFAULT: u8 = 8;
/// "Remind me if no reply" choices, in days.
pub const REMIND_DAYS: [u32; 4] = [1, 2, 3, 7];

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Fictional starters for a fresh install.
pub fn default_snippets() -> Vec<Snippet> {
    vec![
        Snippet {
            id: "thanks".into(),
            trigger: "thx".into(),
            title: "Thanks + next steps".into(),
            body: "Thanks, {first_name}. Next steps on my side: {cursor}. I'll follow up by {day}.\n\n{my_name}".into(),
            ..Default::default()
        },
        Snippet {
            id: "review".into(),
            trigger: "review".into(),
            title: "Review checklist".into(),
            body: "could you review the deck ahead of time and flag anything blocking by {day before}? That keeps the session focused on decisions.".into(),
            ..Default::default()
        },
    ]
}

/// Whether `t` can be typed after ";": 1–32 of [a-z0-9_-].
pub fn valid_trigger(t: &str) -> bool {
    (1..=32).contains(&t.len())
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Snippet addresses: trimmed, non-empty, ≤ 320 chars, deduped (case-blind), ≤ 20.
fn normalize_snippet_addresses(list: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    list.into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty() && a.len() <= 320 && seen.insert(a.to_lowercase()))
        .take(MAX_SNIPPET_ADDRESSES)
        .collect()
}

/// Snippet files: a sha256 id, a display name, deduped, ≤ 10.
fn normalize_snippet_files(list: Vec<SnippetFile>) -> Vec<SnippetFile> {
    let mut seen = HashSet::new();
    list.into_iter()
        .filter_map(|mut f| {
            let id_ok = f.id.len() == 64
                && f.id
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
            if !id_ok || !seen.insert(f.id.clone()) {
                return None;
            }
            f.filename = truncate_chars(f.filename.trim(), 200);
            if f.filename.is_empty() {
                f.filename = "Attachment".into();
            }
            f.mime_type = truncate_chars(f.mime_type.trim(), 100);
            if f.mime_type.is_empty() {
                f.mime_type = "application/octet-stream".into();
            }
            Some(f)
        })
        .take(MAX_SNIPPET_FILES)
        .collect()
}

/// Drop snippets with a bad id or trigger, trim sizes, keep the first of any
/// duplicate trigger, and cap the list.
pub fn normalize_snippets(snippets: Vec<Snippet>) -> Vec<Snippet> {
    let mut seen_ids = HashSet::new();
    let mut seen_triggers = HashSet::new();
    snippets
        .into_iter()
        .filter_map(|mut s| {
            s.trigger = s.trigger.trim().to_lowercase();
            if !valid_id(&s.id)
                || !valid_trigger(&s.trigger)
                || !seen_ids.insert(s.id.clone())
                || !seen_triggers.insert(s.trigger.clone())
            {
                return None;
            }
            s.title = truncate_chars(s.title.trim(), MAX_SNIPPET_TITLE);
            s.body = truncate_chars(&s.body, MAX_SNIPPET_BODY);
            s.subject = truncate_chars(s.subject.trim(), MAX_SNIPPET_SUBJECT);
            s.cc = normalize_snippet_addresses(std::mem::take(&mut s.cc));
            s.bcc = normalize_snippet_addresses(std::mem::take(&mut s.bcc));
            s.attachments = normalize_snippet_files(std::mem::take(&mut s.attachments));
            Some(s)
        })
        .take(MAX_SNIPPETS)
        .collect()
}

/// Trimmed, non-empty, deduped, ≤ 200 chars each, at most nine.
pub fn normalize_instant_replies(mut r: InstantReplies) -> InstantReplies {
    let mut seen = HashSet::new();
    r.replies = r
        .replies
        .into_iter()
        .map(|t| truncate_chars(t.trim(), MAX_INSTANT_REPLY))
        .filter(|t| !t.is_empty() && seen.insert(t.clone()))
        .take(MAX_INSTANT_REPLIES)
        .collect();
    r
}

/// One instant reply as offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstantChoice {
    /// 1–9 (⌃n picks it on the Mac).
    pub n: u32,
    pub text: String,
    /// Suggested by an on-device model (not one of yours).
    pub ai: bool,
}

/// The choices, numbered: yours first, then the model's (skipping ones you
/// already have, ignoring case and closing punctuation), nine at most.
/// (The Mac's UI: features/compose/instant.ts `instantChoices`.)
pub fn instant_choices(settings: &InstantReplies, suggestions: &[String]) -> Vec<InstantChoice> {
    let mut out: Vec<InstantChoice> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |text: &str, ai: bool| {
        let t = text.trim();
        let key = t
            .to_lowercase()
            .trim_end_matches(|c: char| matches!(c, '.' | '!' | '?') || c.is_whitespace())
            .to_string();
        if t.is_empty() || out.len() >= MAX_INSTANT_REPLIES || !seen.insert(key) {
            return;
        }
        out.push(InstantChoice {
            n: out.len() as u32 + 1,
            text: t.to_string(),
            ai,
        });
    };
    if settings.enabled {
        for r in &settings.replies {
            push(r, false);
        }
        if settings.ai_suggestions {
            for r in suggestions {
                push(r, true);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_normalizing() {
        assert_eq!(normalize_snippets(default_snippets()), default_snippets());
        assert_eq!(
            normalize_instant_replies(InstantReplies::default()),
            InstantReplies::default()
        );
    }

    #[test]
    fn snippets_with_bad_ids_or_repeated_triggers_go() {
        let s = |id: &str, trigger: &str| Snippet {
            id: id.into(),
            trigger: trigger.into(),
            title: "  T  ".into(),
            cc: vec![" a@x.example ".into(), "A@x.example".into(), "".into()],
            ..Default::default()
        };
        let out = normalize_snippets(vec![
            s("one", " Hi "),
            s("two", "hi"),
            s("bad id", "x"),
            s("three", "no spaces"),
            s("four", "ok_1-2"),
        ]);
        let triggers: Vec<&str> = out.iter().map(|s| s.trigger.as_str()).collect();
        assert_eq!(triggers, ["hi", "ok_1-2"]);
        assert_eq!(out[0].title, "T");
        assert_eq!(out[0].cc, ["a@x.example"]);
    }

    #[test]
    fn instant_replies_are_cleaned_and_numbered() {
        let r = normalize_instant_replies(InstantReplies {
            enabled: true,
            replies: vec![
                " Thanks! ".into(),
                "".into(),
                "Thanks!".into(),
                "x".repeat(300),
            ],
            ai_suggestions: true,
        });
        assert_eq!(r.replies.len(), 2);
        assert_eq!(r.replies[1].chars().count(), MAX_INSTANT_REPLY);
        let c = instant_choices(&r, &["thanks".into(), "On my way".into()]);
        assert_eq!(c.len(), 3, "the model's \"thanks\" is one you have");
        assert_eq!(
            (c[2].n, c[2].text.as_str(), c[2].ai),
            (3, "On my way", true)
        );
        let off = InstantReplies {
            enabled: false,
            ..r
        };
        assert!(instant_choices(&off, &[]).is_empty());
    }
}
