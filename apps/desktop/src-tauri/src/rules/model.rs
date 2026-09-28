//! Rule definitions: `<config dir>/rules.json`, rewritten atomically (0600)
//! on every change like settings.json. Mirrored in
//! `apps/desktop/src/lib/types.ts` (Rule, RuleAction, RuleTrigger,
//! RulesConfig) — change both together.
//!
//! A rule is: a trigger, a condition in the search language (exactly what
//! the search box accepts), and actions. Every rule starts in dry-run.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

pub const RULES_FILE: &str = "rules.json";
pub const MAX_RULES: usize = 200;
const MAX_NAME: usize = 80;
const MAX_CONDITION: usize = 1000;
const MAX_ACTIONS: usize = 12;
const MAX_HOOK_ARGS: usize = 32;
const MAX_ARG_LEN: usize = 4096;
pub const HOOK_TIMEOUT_DEFAULT: u32 = 10;
pub const HOOK_TIMEOUT_MAX: u32 = 120;
const MAX_SECRET: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RulesConfig {
    pub version: u32,
    /// Settings → Developer → "Allow hooks". Off: shell-hook and webhook
    /// actions are skipped (and logged as such).
    pub allow_hooks: bool,
    /// Evaluated in order; `stop_processing` ends the chain for a message.
    pub rules: Vec<Rule>,
}

impl Default for RulesConfig {
    fn default() -> Self {
        RulesConfig {
            version: 1,
            allow_hooks: false,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    /// `[A-Za-z0-9_-]{1,64}`, chosen by the backend on create.
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Match and log, but take no action. New rules start here.
    pub dry_run: bool,
    /// Accounts the rule watches: `null` = every account.
    #[serde(default)]
    pub account_ids: Option<Vec<String>>,
    /// Or a profile's accounts (resolved when the rule runs).
    #[serde(default)]
    pub profile_id: Option<String>,
    pub trigger: Trigger,
    /// Search query, e.g. `from:@uber.com has:pdf`.
    pub condition: String,
    pub actions: Vec<Action>,
    /// A message this rule matched is not offered to later rules.
    #[serde(default)]
    pub stop_processing: bool,
    /// Hooks and webhooks receive the message text (off: headers only).
    #[serde(default)]
    pub include_body: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    /// Set when the engine switched the rule off (e.g. it fired too often).
    #[serde(default)]
    pub paused_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Every {
    Daily,
    Weekly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Trigger {
    /// Mail that arrives through incremental sync (never backfill).
    NewMessage,
    /// A message gains `label` (a label name, or a system id like STARRED).
    LabelAdded { label: String },
    /// Local time `at` ("HH:MM"), daily or on `weekday` (0 = Sunday).
    Schedule {
        every: Every,
        at: String,
        #[serde(default)]
        weekday: Option<u8>,
    },
    /// Only "Run now".
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// `label`: a label name (resolved per account) or system id.
    AddLabel {
        label: String,
    },
    RemoveLabel {
        label: String,
    },
    Archive,
    MarkRead,
    Star,
    /// Needs `confirmed` (the editor asks once when it's added).
    Trash {
        #[serde(default)]
        confirmed: bool,
    },
    /// Sends a copy (plain text + attachments) from the matching account.
    /// Needs `confirmed`; rate-limited.
    Forward {
        to: String,
        #[serde(default)]
        confirmed: bool,
    },
    /// macOS notification.
    Notify,
    /// Run `program` with `args` (argv, no shell); the match is JSON on stdin.
    Hook {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default = "default_timeout")]
        timeout_secs: u32,
    },
    /// POST the match as JSON; signed with HMAC-SHA256 when `secret` is set.
    Webhook {
        url: String,
        #[serde(default)]
        secret: Option<String>,
    },
}

fn default_timeout() -> u32 {
    HOOK_TIMEOUT_DEFAULT
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::AddLabel { .. } => "addLabel",
            Action::RemoveLabel { .. } => "removeLabel",
            Action::Archive => "archive",
            Action::MarkRead => "markRead",
            Action::Star => "star",
            Action::Trash { .. } => "trash",
            Action::Forward { .. } => "forward",
            Action::Notify => "notify",
            Action::Hook { .. } => "hook",
            Action::Webhook { .. } => "webhook",
        }
    }
    pub fn is_hook(&self) -> bool {
        matches!(self, Action::Hook { .. } | Action::Webhook { .. })
    }
}

/// `save_rule` input: a rule without the backend-owned fields. `id` null
/// creates.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub dry_run: bool,
    #[serde(default)]
    pub account_ids: Option<Vec<String>>,
    #[serde(default)]
    pub profile_id: Option<String>,
    pub trigger: Trigger,
    pub condition: String,
    pub actions: Vec<Action>,
    #[serde(default)]
    pub stop_processing: bool,
    #[serde(default)]
    pub include_body: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// "HH:MM" (24 h) → minutes after midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// A forward target: one plain address, no display name, no header tricks.
pub fn valid_forward_address(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    s.len() <= 254
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && s.chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '<' | '>' | ',' | ';' | '"' | '\\'))
}

/// Webhooks go over https, or plain http to this machine only.
pub fn valid_webhook_url(s: &str) -> Result<url::Url, String> {
    let u = url::Url::parse(s.trim()).map_err(|_| format!("{s:?} isn't a URL."))?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err("Put credentials in the signing secret, not the URL.".into());
    }
    let local = matches!(
        u.host(),
        Some(url::Host::Domain("localhost"))
            | Some(url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST))
            | Some(url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST))
    );
    match u.scheme() {
        "https" if u.host().is_some() => Ok(u),
        "http" if local => Ok(u),
        "http" => Err("Webhooks must use https (http only to localhost).".into()),
        _ => Err("Webhooks must use https.".into()),
    }
}

/// `~/x` → `$HOME/x`. The result must be absolute: hooks never search PATH.
pub fn resolve_program(program: &str) -> Result<PathBuf, String> {
    let p = program.trim();
    let path = match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().ok_or("No home directory.")?.join(rest),
        None => PathBuf::from(p),
    };
    if !path.is_absolute() {
        return Err("Give the hook's full path (e.g. ~/bin/on-receipt.sh).".into());
    }
    Ok(path)
}

impl RuleInput {
    /// Validate and normalize into a stored rule. `prev` is the rule being
    /// edited (keeps id and created_at). Err is a user-facing message.
    pub fn into_rule(self, prev: Option<&Rule>, new_id: String, now: i64) -> Result<Rule, String> {
        let name = self.name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            return Err("Give the rule a name.".into());
        }
        let name: String = name.chars().take(MAX_NAME).collect();
        let condition = self.condition.trim().to_string();
        if condition.is_empty() {
            return Err("Add a condition (a search, e.g. from:@uber.com has:pdf).".into());
        }
        if condition.chars().count() > MAX_CONDITION {
            return Err("The condition is too long.".into());
        }
        if self.actions.is_empty() {
            return Err("Add at least one action.".into());
        }
        if self.actions.len() > MAX_ACTIONS {
            return Err(format!("A rule can have at most {MAX_ACTIONS} actions."));
        }
        match &self.trigger {
            Trigger::LabelAdded { label } if label.trim().is_empty() => {
                return Err("Pick the label that triggers the rule.".into())
            }
            Trigger::Schedule { at, weekday, every } => {
                if parse_hhmm(at).is_none() {
                    return Err(format!("{at:?} isn't a time (use HH:MM)."));
                }
                if *every == Every::Weekly && !weekday.is_some_and(|d| d < 7) {
                    return Err("Pick a day of the week.".into());
                }
            }
            _ => {}
        }
        let mut actions = Vec::with_capacity(self.actions.len());
        for a in self.actions {
            actions.push(match a {
                Action::AddLabel { label } | Action::RemoveLabel { label }
                    if label.trim().is_empty() =>
                {
                    return Err("Pick a label.".into())
                }
                Action::AddLabel { label } => Action::AddLabel {
                    label: label.trim().to_string(),
                },
                Action::RemoveLabel { label } => Action::RemoveLabel {
                    label: label.trim().to_string(),
                },
                Action::Trash { confirmed: false } => {
                    return Err("Confirm that this rule may move mail to Trash.".into())
                }
                Action::Forward {
                    confirmed: false, ..
                } => return Err("Confirm that this rule may forward mail.".into()),
                Action::Forward { to, confirmed } => {
                    if !valid_forward_address(&to) {
                        return Err(format!("{to:?} isn't an email address."));
                    }
                    Action::Forward {
                        to: to.trim().to_lowercase(),
                        confirmed,
                    }
                }
                Action::Hook {
                    program,
                    args,
                    timeout_secs,
                } => {
                    resolve_program(&program)?;
                    if args.len() > MAX_HOOK_ARGS
                        || args
                            .iter()
                            .any(|a| a.len() > MAX_ARG_LEN || a.contains('\0'))
                    {
                        return Err("Too many or too long hook arguments.".into());
                    }
                    Action::Hook {
                        program: program.trim().to_string(),
                        args,
                        timeout_secs: timeout_secs.clamp(1, HOOK_TIMEOUT_MAX),
                    }
                }
                Action::Webhook { url, secret } => {
                    let u = valid_webhook_url(&url)?;
                    let secret = secret.filter(|s| !s.is_empty());
                    if secret.as_ref().is_some_and(|s| s.len() > MAX_SECRET) {
                        return Err("The signing secret is too long.".into());
                    }
                    Action::Webhook {
                        url: u.to_string(),
                        secret,
                    }
                }
                other => other,
            });
        }
        let account_ids = self.account_ids.map(|ids| {
            let mut out: Vec<String> = Vec::new();
            for a in ids {
                let a = a.trim().to_lowercase();
                if !a.is_empty() && !out.contains(&a) {
                    out.push(a);
                }
            }
            out
        });
        if account_ids.as_ref().is_some_and(Vec::is_empty) {
            return Err("Pick at least one account, or all accounts.".into());
        }
        let profile_id = self.profile_id.filter(|p| !p.is_empty());
        if profile_id.as_deref().is_some_and(|p| !valid_id(p)) {
            return Err("Unknown profile.".into());
        }
        Ok(Rule {
            id: prev.map(|p| p.id.clone()).unwrap_or(new_id),
            name,
            enabled: self.enabled,
            dry_run: self.dry_run,
            account_ids,
            profile_id,
            trigger: self.trigger,
            condition,
            actions,
            stop_processing: self.stop_processing,
            include_body: self.include_body,
            created_at: prev.map_or(now, |p| p.created_at),
            updated_at: now,
            // Saving is the user's acknowledgement of a pause.
            paused_reason: None,
        })
    }
}

impl RulesConfig {
    /// Missing file → empty. A malformed file is kept aside (rules.json.bad)
    /// and replaced by an empty config rather than blocking the app, and
    /// rules with invalid ids are dropped.
    pub fn load(path: &Path) -> RulesConfig {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RulesConfig::default(),
            Err(e) => {
                tracing::warn!(error = %e, "could not read rules; none will run");
                return RulesConfig::default();
            }
        };
        match serde_json::from_str::<RulesConfig>(&text) {
            Ok(mut c) => {
                let mut seen = std::collections::HashSet::new();
                c.rules
                    .retain(|r| valid_id(&r.id) && seen.insert(r.id.clone()));
                c.rules.truncate(MAX_RULES);
                c
            }
            Err(e) => {
                tracing::warn!(error = %e, "rules.json is malformed; keeping it as rules.json.bad");
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                RulesConfig::default()
            }
        }
    }

    /// Temp file (0600), fsync, rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{RULES_FILE}.{}.tmp", std::process::id()));
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let result = (|| {
            let mut f = opts.open(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

/// The in-memory copy plus where it lives; writes are serialized.
pub struct RulesStore {
    path: PathBuf,
    current: RwLock<RulesConfig>,
}

impl RulesStore {
    pub fn load(config_dir: &Path) -> RulesStore {
        let path = config_dir.join(RULES_FILE);
        let current = RulesConfig::load(&path);
        RulesStore {
            path,
            current: RwLock::new(current),
        }
    }

    pub fn get(&self) -> RulesConfig {
        self.current
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Apply `f` to a copy, persist, then publish. `f`'s error aborts
    /// without writing. Blocking (file I/O).
    pub fn update<T>(
        &self,
        f: impl FnOnce(&mut RulesConfig) -> Result<T, String>,
    ) -> Result<(RulesConfig, T), String> {
        let mut current = self.current.write().unwrap_or_else(|p| p.into_inner());
        let mut next = current.clone();
        let out = f(&mut next)?;
        next.save(&self.path)
            .map_err(|e| format!("Couldn't save rules: {e}"))?;
        *current = next.clone();
        Ok((next, out))
    }
}

/// A fresh rule id: `r_` + 12 random hex chars.
pub fn new_rule_id() -> String {
    let mut buf = [0u8; 6];
    if getrandom::fill(&mut buf).is_err() {
        // Clock-derived fallback; uniqueness is re-checked by the caller.
        let n = crate::ops::now_ms() as u64;
        buf.copy_from_slice(&n.to_le_bytes()[..6]);
    }
    format!(
        "r_{}",
        buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(json: serde_json::Value) -> Result<Rule, String> {
        serde_json::from_value::<RuleInput>(json)
            .map_err(|e| e.to_string())?
            .into_rule(None, "r_1".into(), 5)
    }

    fn base() -> serde_json::Value {
        serde_json::json!({
            "name": "  Uber   receipts ",
            "enabled": true,
            "dryRun": true,
            "trigger": {"kind": "newMessage"},
            "condition": " from:@uber.com has:pdf ",
            "actions": [{"kind": "addLabel", "label": " Receipts "}, {"kind": "archive"}]
        })
    }

    #[test]
    fn wire_format_and_normalization() {
        let r = input(base()).unwrap();
        assert_eq!(r.name, "Uber receipts");
        assert_eq!(r.condition, "from:@uber.com has:pdf");
        assert_eq!(
            r.actions[0],
            Action::AddLabel {
                label: "Receipts".into()
            }
        );
        assert_eq!((r.id.as_str(), r.created_at, r.updated_at), ("r_1", 5, 5));
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["trigger"], serde_json::json!({"kind": "newMessage"}));
        assert_eq!(json["dryRun"], true);
        assert_eq!(json["actions"][1], serde_json::json!({"kind": "archive"}));
    }

    #[test]
    fn risky_actions_need_confirmation() {
        let mut j = base();
        j["actions"] = serde_json::json!([{"kind": "trash"}]);
        assert!(input(j.clone()).unwrap_err().contains("Trash"));
        j["actions"] = serde_json::json!([{"kind": "trash", "confirmed": true}]);
        assert!(input(j.clone()).is_ok());
        j["actions"] = serde_json::json!([{"kind": "forward", "to": "me@x.example"}]);
        assert!(input(j.clone()).unwrap_err().contains("forward"));
        j["actions"] =
            serde_json::json!([{"kind": "forward", "to": "Me@X.example", "confirmed": true}]);
        assert_eq!(
            input(j.clone()).unwrap().actions[0],
            Action::Forward {
                to: "me@x.example".into(),
                confirmed: true
            }
        );
        for bad in [
            "a@b",
            "x\r\nBcc: evil@x.example",
            "\"Evil\" <e@x.example>",
            "a@x.example,b@x.example",
        ] {
            j["actions"] = serde_json::json!([{"kind": "forward", "to": bad, "confirmed": true}]);
            assert!(input(j.clone()).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn hooks_and_webhooks_are_validated() {
        let mut j = base();
        j["actions"] = serde_json::json!([{"kind": "hook", "program": "notify.sh"}]);
        assert!(input(j.clone()).unwrap_err().contains("full path"));
        j["actions"] =
            serde_json::json!([{"kind": "hook", "program": "/usr/bin/true", "timeoutSecs": 9999}]);
        assert_eq!(
            input(j.clone()).unwrap().actions[0],
            Action::Hook {
                program: "/usr/bin/true".into(),
                args: vec![],
                timeout_secs: HOOK_TIMEOUT_MAX
            }
        );
        for (url, ok) in [
            ("https://hooks.example/in", true),
            ("http://localhost:8080/x", true),
            ("http://127.0.0.1/x", true),
            ("http://hooks.example/in", false),
            ("https://user:pw@hooks.example/", false),
            ("file:///etc/passwd", false),
            ("javascript:alert(1)", false),
        ] {
            j["actions"] = serde_json::json!([{"kind": "webhook", "url": url}]);
            assert_eq!(input(j.clone()).is_ok(), ok, "{url}");
        }
    }

    #[test]
    fn schedule_and_scope_validation() {
        let mut j = base();
        j["trigger"] = serde_json::json!({"kind": "schedule", "every": "weekly", "at": "08:00"});
        assert!(input(j.clone()).unwrap_err().contains("day"));
        j["trigger"] =
            serde_json::json!({"kind": "schedule", "every": "weekly", "at": "08:00", "weekday": 1});
        assert!(input(j.clone()).is_ok());
        j["trigger"] = serde_json::json!({"kind": "schedule", "every": "daily", "at": "25:00"});
        assert!(input(j.clone()).is_err());
        assert_eq!(parse_hhmm("7:05"), Some(425));
        assert_eq!(parse_hhmm("07:5"), None);
        let mut j = base();
        j["accountIds"] = serde_json::json!([" A@x.example", "a@x.example"]);
        assert_eq!(
            input(j.clone()).unwrap().account_ids,
            Some(vec!["a@x.example".to_string()])
        );
        j["accountIds"] = serde_json::json!([]);
        assert!(input(j.clone()).is_err());
        let mut j = base();
        j["condition"] = serde_json::json!("  ");
        assert!(input(j).is_err());
        let mut j = base();
        j["extra"] = serde_json::json!(1);
        assert!(input(j).is_err(), "unknown keys are rejected");
    }

    #[test]
    fn file_round_trip_is_private_and_survives_garbage() {
        let dir = std::env::temp_dir().join(format!("penguin-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = RulesStore::load(&dir);
        assert_eq!(store.get(), RulesConfig::default());
        let rule = input(base()).unwrap();
        store
            .update(|c| {
                c.rules.push(rule.clone());
                Ok(())
            })
            .unwrap();
        let path = dir.join(RULES_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(RulesStore::load(&dir).get().rules, vec![rule]);
        std::fs::write(&path, "{nope").unwrap();
        assert!(RulesStore::load(&dir).get().rules.is_empty());
        assert!(dir.join("rules.json.bad").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
