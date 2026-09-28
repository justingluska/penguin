//! Tauri-free helpers shared by the app commands and `penguin-cli`, so both
//! resolve the same directories and create accounts the same way.

use std::path::PathBuf;

use penguin_core::{Account, Address, Message};
use penguin_gmail::auth::SignedInAccount;

/// Must match `identifier` in tauri.conf.json: Tauri resolves its app dirs as
/// `<platform dir>/<identifier>`, and we resolve them the same way here so the
/// CLI and the app share one database, config and cache.
pub const APP_IDENTIFIER: &str = "co.gluska.penguin";

pub const DB_FILE: &str = "penguin.db";
pub const OAUTH_CLIENT_FILE: &str = "google-oauth-client.json";
/// Same env override `OAuthClientConfig::load` honours.
pub const OAUTH_CLIENT_ENV: &str = "PENGUIN_GOOGLE_CLIENT_JSON";

#[derive(Debug, Clone)]
pub struct Paths {
    /// Holds penguin.db.
    pub data_dir: PathBuf,
    /// Holds google-oauth-client.json.
    pub config_dir: PathBuf,
    /// Disposable caches (inline images).
    pub cache_dir: PathBuf,
}

impl Paths {
    /// `PENGUIN_DATA_DIR` puts everything under one directory (handy for
    /// testing against a throwaway mailbox); otherwise the platform app dirs.
    pub fn resolve() -> Result<Paths, String> {
        if let Some(root) = std::env::var_os("PENGUIN_DATA_DIR") {
            let root = PathBuf::from(root);
            return Ok(Paths {
                data_dir: root.clone(),
                config_dir: root.clone(),
                cache_dir: root.join("cache"),
            });
        }
        let dir = |base: Option<PathBuf>, what: &str| {
            base.map(|b| b.join(APP_IDENTIFIER))
                .ok_or_else(|| format!("could not resolve the {what} directory"))
        };
        Ok(Paths {
            data_dir: dir(dirs::data_dir(), "data")?,
            config_dir: dir(dirs::config_dir(), "config")?,
            cache_dir: dir(dirs::cache_dir(), "cache")?,
        })
    }

    pub fn create_all(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.cache_dir)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join(DB_FILE)
    }

    /// Where the OAuth client JSON is read from (env override first).
    pub fn oauth_client_path(&self) -> PathBuf {
        match std::env::var_os(OAUTH_CLIENT_ENV) {
            Some(p) => PathBuf::from(p),
            None => self.config_dir.join(OAUTH_CLIENT_FILE),
        }
    }

    /// Per-account inline image cache root.
    pub fn inline_cache_dir(&self, account_id: &str) -> PathBuf {
        self.cache_dir
            .join("inline")
            .join(safe_component(account_id))
    }
}

/// Account dot colors, chosen to stay distinguishable in light and dark themes.
pub const ACCOUNT_PALETTE: &[&str] = &[
    "#4F7CFF", // blue
    "#E0685A", // coral
    "#2FA37A", // green
    "#B06AD9", // violet
    "#E3A13B", // amber
    "#2BA3B8", // teal
    "#D9648F", // rose
    "#7C8A9E", // slate
];

/// First palette color not used by another account; wraps around when all
/// are taken.
pub fn pick_color(existing: &[Account]) -> String {
    ACCOUNT_PALETTE
        .iter()
        .find(|c| !existing.iter().any(|a| a.color.eq_ignore_ascii_case(c)))
        .unwrap_or(&ACCOUNT_PALETTE[existing.len() % ACCOUNT_PALETTE.len()])
        .to_string()
}

/// Account record for a completed sign-in. Re-signing an existing account
/// keeps its color and `added_at`.
pub fn account_for_sign_in(signed: &SignedInAccount, existing: &[Account], now_ms: i64) -> Account {
    let id = signed.email.trim().to_lowercase();
    match existing.iter().find(|a| a.id == id) {
        Some(prev) => Account {
            display_name: signed
                .display_name
                .clone()
                .or_else(|| prev.display_name.clone()),
            email: signed.email.clone(),
            ..prev.clone()
        },
        None => {
            let others: Vec<Account> = existing.to_vec();
            Account {
                id,
                email: signed.email.clone(),
                display_name: signed.display_name.clone(),
                nickname: None,
                color: pick_color(&others),
                added_at: now_ms,
                ..Account::default()
            }
        }
    }
}

/// Longest account nickname, in characters ("Sam Work", "Shop Help").
pub const MAX_NICKNAME_CHARS: usize = 32;

/// `update_account` argument: omitted fields are unchanged; `nickname: null`
/// (or blank) clears the nickname.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountPatch {
    #[serde(default, deserialize_with = "present")]
    pub nickname: Option<Option<String>>,
    pub color: Option<String>,
}

/// A present field (even `null`) is `Some`; a missing one stays `None` via `default`.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    use serde::Deserialize;
    Ok(Some(Option::<String>::deserialize(d)?))
}

/// A validated account patch: nickname trimmed with inner whitespace
/// collapsed (blank clears it), color `#rrggbb`.
pub type AccountUpdate = (Option<Option<String>>, Option<String>);

/// Validate an `AccountPatch`. Err is a user-facing message.
pub fn validate_account_patch(patch: AccountPatch) -> Result<AccountUpdate, String> {
    let nickname = match patch.nickname {
        None => None,
        Some(None) => Some(None),
        Some(Some(raw)) => {
            if raw.chars().any(char::is_control) {
                return Err("A nickname can't contain control characters.".into());
            }
            let n = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            if n.chars().count() > MAX_NICKNAME_CHARS {
                return Err(format!(
                    "Keep the nickname to {MAX_NICKNAME_CHARS} characters."
                ));
            }
            Some(if n.is_empty() { None } else { Some(n) })
        }
    };
    let color = match patch.color {
        None => None,
        Some(c) => {
            let ok = c.len() == 7
                && c.starts_with('#')
                && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
            if !ok {
                return Err(format!("{c:?} isn't a #rrggbb color."));
            }
            Some(c)
        }
    };
    Ok((nickname, color))
}

/// Gmail's label name limit, in characters.
pub const MAX_LABEL_NAME_CHARS: usize = 225;

/// `update_label` argument: omitted fields are unchanged; `color: null`
/// removes the label's color. `color` is a background hex from
/// `penguin_gmail::label_colors::LABEL_COLORS` (the text color is implied).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LabelPatch {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub color: Option<Option<String>>,
    pub hidden: Option<bool>,
}

/// A validated label patch, ready for `GmailClient::patch_label`:
/// (name, color as (background, text), hidden).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelUpdate {
    pub name: Option<String>,
    pub color: Option<Option<(&'static str, &'static str)>>,
    pub hidden: Option<bool>,
}

impl LabelUpdate {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.color.is_none() && self.hidden.is_none()
    }
}

/// Validate a `LabelPatch`. The name is trimmed and must be non-empty, at
/// most 225 characters, without control characters; "/" nests ("Clients/Acme")
/// but no level may be blank. Err is a user-facing message.
pub fn validate_label_patch(patch: LabelPatch) -> Result<LabelUpdate, String> {
    let name = match patch.name {
        None => None,
        Some(raw) => {
            let n = raw.trim();
            if n.is_empty() {
                return Err("A label needs a name.".into());
            }
            if n.chars().any(char::is_control) {
                return Err("A label name can't contain control characters.".into());
            }
            if n.chars().count() > MAX_LABEL_NAME_CHARS {
                return Err(format!(
                    "Keep the label name to {MAX_LABEL_NAME_CHARS} characters."
                ));
            }
            if n.split('/').any(|part| part.trim().is_empty()) {
                return Err(
                    "Each level of a nested label needs a name (no empty parts around \"/\")."
                        .into(),
                );
            }
            Some(n.to_string())
        }
    };
    let color = match patch.color {
        None => None,
        Some(None) => Some(None),
        Some(Some(bg)) => match penguin_gmail::label_colors::by_background(&bg) {
            Some(c) => Some(Some((c.background, c.text))),
            None => return Err(format!("{bg:?} isn't one of the label colors.")),
        },
    };
    Ok(LabelUpdate {
        name,
        color,
        hidden: patch.hidden,
    })
}

pub fn from_address(account: &Account) -> Address {
    Address {
        name: account.display_name.clone(),
        email: account.email.clone(),
    }
}

/// In-Reply-To and References inputs for `build_rfc822` when replying to
/// `parent`: the parent's own Message-ID and References (build_rfc822 appends
/// the parent id to References itself).
pub fn reply_headers(parent: &Message) -> (Option<String>, Vec<String>) {
    (parent.message_id_header.clone(), parent.references.clone())
}

/// Attachment filename safe to create in a user folder: no path separators,
/// control characters, or leading dots; bounded length.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    let mut out: String = trimmed.chars().take(180).collect();
    if out.is_empty() {
        out = "attachment".into();
    }
    out
}

/// Write `bytes` to `dir/name`, or `name (1).ext`, `name (2).ext`, … if
/// taken. Never overwrites (create_new), so concurrent saves can't collide.
pub fn write_unique(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 0..1000 {
        let candidate = if n == 0 {
            dir.join(name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut f) => {
                f.write_all(bytes)?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many files with this name",
    ))
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Keep a string usable as a single path component.
pub fn safe_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '-' | '_' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_start_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label_patch(json: &str) -> Result<LabelUpdate, String> {
        validate_label_patch(serde_json::from_str::<LabelPatch>(json).map_err(|e| e.to_string())?)
    }

    #[test]
    fn label_patch_wire_format_and_validation() {
        let empty = label_patch("{}").unwrap();
        assert!(empty.is_empty());
        let u = label_patch(r##"{"name":"  Clients/Acme  ","color":"#FB4C2F","hidden":true}"##)
            .unwrap();
        assert_eq!(u.name.as_deref(), Some("Clients/Acme"));
        assert_eq!(u.color, Some(Some(("#fb4c2f", "#ffffff"))));
        assert_eq!(u.hidden, Some(true));
        assert_eq!(
            label_patch(r##"{"color":null}"##).unwrap().color,
            Some(None)
        );
        assert!(label_patch(r##"{"color":"#123456"}"##).is_err());
        assert!(label_patch(r##"{"name":"   "}"##).is_err());
        assert!(label_patch(r##"{"name":"a//b"}"##).is_err());
        assert!(label_patch(r##"{"name":"/a"}"##).is_err());
        assert!(label_patch(r##"{"name":"a\nb"}"##).is_err());
        assert!(label_patch(&format!(r#"{{"name":"{}"}}"#, "x".repeat(226))).is_err());
        assert!(label_patch(&format!(r#"{{"name":"{}"}}"#, "x".repeat(225))).is_ok());
        assert!(label_patch(r##"{"colour":"#fb4c2f"}"##).is_err());
    }

    fn patch(json: &str) -> Result<AccountUpdate, String> {
        validate_account_patch(
            serde_json::from_str::<AccountPatch>(json).map_err(|e| e.to_string())?,
        )
    }

    #[test]
    fn account_patch_wire_format_and_validation() {
        assert_eq!(patch("{}"), Ok((None, None)));
        assert_eq!(patch(r#"{"nickname":null}"#), Ok((Some(None), None)));
        assert_eq!(
            patch(r#"{"nickname":"  Sam   Work "}"#),
            Ok((Some(Some("Sam Work".into())), None))
        );
        assert_eq!(patch(r#"{"nickname":"   "}"#), Ok((Some(None), None)));
        assert_eq!(
            patch(r##"{"color":"#4F7CFF"}"##),
            Ok((None, Some("#4F7CFF".into())))
        );
        assert!(patch(r#"{"color":"blue"}"#).is_err());
        assert!(patch(r##"{"color":"#12345g"}"##).is_err());
        assert!(patch(r#"{"nickname":"a\u0007b"}"#).is_err());
        assert!(patch(&format!(
            r#"{{"nickname":"{}"}}"#,
            "x".repeat(MAX_NICKNAME_CHARS + 1)
        ))
        .is_err());
        assert!(patch(&format!(
            r#"{{"nickname":"{}"}}"#,
            "é".repeat(MAX_NICKNAME_CHARS)
        ))
        .is_ok());
        assert!(
            patch(r#"{"displayName":"x"}"#).is_err(),
            "unknown fields are rejected"
        );
    }

    fn account(id: &str, color: &str) -> Account {
        Account {
            id: id.into(),
            email: id.into(),
            display_name: None,
            nickname: None,
            color: color.into(),
            added_at: 1,
            ..Account::default()
        }
    }

    #[test]
    fn picks_first_unused_color() {
        let existing = vec![
            account("a@x.example", ACCOUNT_PALETTE[0]),
            account("b@x.example", ACCOUNT_PALETTE[2]),
        ];
        assert_eq!(pick_color(&existing), ACCOUNT_PALETTE[1]);
    }

    #[test]
    fn resign_in_keeps_color_and_lowercases_id() {
        let existing = vec![account("ada@x.example", "#123456")];
        let signed = SignedInAccount {
            email: "Ada@X.example".into(),
            display_name: Some("Ada".into()),
        };
        let acct = account_for_sign_in(&signed, &existing, 99);
        assert_eq!(acct.id, "ada@x.example");
        assert_eq!(acct.color, "#123456");
        assert_eq!(acct.added_at, 1);
        assert_eq!(acct.display_name.as_deref(), Some("Ada"));
    }

    #[test]
    fn filenames_are_sanitized_and_unique() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_filename("a\u{0}b\\c:d.pdf"), "a_b_c_d.pdf");
        assert_eq!(sanitize_filename("  ...  "), "attachment");
        let dir = std::env::temp_dir().join(format!("penguin-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = write_unique(&dir, "report.pdf", b"1").unwrap();
        let b = write_unique(&dir, "report.pdf", b"2").unwrap();
        assert_eq!(a.file_name().unwrap(), "report.pdf");
        assert_eq!(b.file_name().unwrap(), "report (1).pdf");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn safe_component_blocks_traversal() {
        assert_eq!(safe_component("../etc/passwd"), "_etc_passwd");
        assert_eq!(safe_component("ada@x.example"), "ada@x.example");
    }
}
