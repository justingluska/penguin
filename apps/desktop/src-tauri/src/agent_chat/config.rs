//! Settings → AI → "Chat with Claude Code or Codex": the switch, which CLI,
//! an optional path to it, and whether an API key is saved. Plain JSON in
//! the app config dir (`agent-chat.json`, never in settings.json). An API
//! key is one generic-password item per CLI in the macOS Keychain (service
//! [`KEYCHAIN_SERVICE`]) behind the `SecretBackend` seam, so tests use a
//! memory backend. Keys are only ever handed to the CLI's process as an
//! environment variable: never logged, never returned to the UI.

use std::path::{Path, PathBuf};

use penguin_provider::credentials::{KeyringBackend, SecretBackend};
use serde::{Deserialize, Serialize};

use super::detect::AgentCliKind;
use crate::error::{CmdError, CmdResult};

pub const KEYCHAIN_SERVICE: &str = "co.gluska.penguin.agent-chat";
const FILE_NAME: &str = "agent-chat.json";
const FILE_VERSION: u32 = 1;
/// Longer than any provider's key; anything bigger is a paste gone wrong.
const MAX_KEY_LEN: usize = 512;

fn keychain_account(kind: AgentCliKind) -> String {
    format!("{}-api-key.v1", kind.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentChatConfig {
    pub version: u32,
    /// Off by default: nothing is looked for or run until the user turns it on.
    pub enabled: bool,
    /// The CLI chats use; null = the first one found.
    pub cli: Option<AgentCliKind>,
    /// A CLI the search doesn't find (for `cli`, or Claude Code when unset).
    pub custom_path: Option<String>,
    /// Which CLIs have an API key in the Keychain (no Keychain read needed
    /// to show it, which may prompt).
    pub claude_key_saved: bool,
    pub codex_key_saved: bool,
}

impl Default for AgentChatConfig {
    fn default() -> Self {
        AgentChatConfig {
            version: FILE_VERSION,
            enabled: false,
            cli: None,
            custom_path: None,
            claude_key_saved: false,
            codex_key_saved: false,
        }
    }
}

impl AgentChatConfig {
    pub fn key_saved(&self, kind: AgentCliKind) -> bool {
        match kind {
            AgentCliKind::Claude => self.claude_key_saved,
            AgentCliKind::Codex => self.codex_key_saved,
        }
    }
    fn set_key_saved(&mut self, kind: AgentCliKind, saved: bool) {
        match kind {
            AgentCliKind::Claude => self.claude_key_saved = saved,
            AgentCliKind::Codex => self.codex_key_saved = saved,
        }
    }
}

/// agent_chat_config_set: omitted fields are unchanged. `cli` / `customPath`
/// take `""` to clear (null means unchanged).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatConfigPatch {
    pub enabled: Option<bool>,
    pub cli: Option<String>,
    pub custom_path: Option<String>,
}

pub fn path_in(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE_NAME)
}

/// The saved config; a missing or unreadable file is the default (off).
pub fn load(path: &Path) -> AgentChatConfig {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "agent-chat.json unreadable; chat is off");
            AgentChatConfig::default()
        }),
        Err(_) => AgentChatConfig::default(),
    }
}

pub fn save(path: &Path, config: &AgentChatConfig) -> CmdResult<()> {
    let bytes = serde_json::to_vec_pretty(config).map_err(|e| CmdError::other(e.to_string()))?;
    crate::share::config::write_atomic(path, &bytes)
        .map_err(|e| CmdError::other(format!("Couldn't save the chat settings: {e}")))
}

/// Apply `patch`, validating the CLI name and the custom path.
pub fn apply(mut config: AgentChatConfig, patch: &AgentChatConfigPatch) -> CmdResult<AgentChatConfig> {
    if let Some(on) = patch.enabled {
        config.enabled = on;
    }
    if let Some(cli) = &patch.cli {
        config.cli = match cli.as_str() {
            "" => None,
            "claude" => Some(AgentCliKind::Claude),
            "codex" => Some(AgentCliKind::Codex),
            other => return Err(CmdError::invalid(format!("Unknown CLI “{other}”: choose claude or codex."))),
        };
    }
    if let Some(p) = &patch.custom_path {
        let p = p.trim();
        config.custom_path = if p.is_empty() {
            None
        } else {
            let expanded = match p.strip_prefix("~/") {
                Some(rest) => dirs::home_dir()
                    .map(|h| h.join(rest).display().to_string())
                    .unwrap_or_else(|| p.to_string()),
                None => p.to_string(),
            };
            if !Path::new(&expanded).is_absolute() {
                return Err(CmdError::invalid("Enter the full path to the CLI, like /opt/homebrew/bin/claude."));
            }
            Some(expanded)
        };
    }
    Ok(config)
}

/// API keys in the Keychain, one item per CLI.
pub struct ApiKeys {
    backend: Box<dyn SecretBackend>,
}

impl ApiKeys {
    pub fn new(backend: Box<dyn SecretBackend>) -> ApiKeys {
        ApiKeys { backend }
    }
    pub fn keychain() -> ApiKeys {
        ApiKeys::new(Box::new(KeyringBackend::new(KEYCHAIN_SERVICE)))
    }
    /// Blocking (a Keychain read can show a prompt).
    pub fn get(&self, kind: AgentCliKind) -> CmdResult<Option<String>> {
        self.backend.get(&keychain_account(kind)).map_err(keychain_error)
    }
    /// Save (Some) or remove (None) `kind`'s key and record that in `config`.
    pub fn set(&self, config: &mut AgentChatConfig, kind: AgentCliKind, key: Option<&str>) -> CmdResult<()> {
        match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => {
                if k.len() > MAX_KEY_LEN || k.chars().any(|c| c.is_whitespace() || c.is_control()) {
                    return Err(CmdError::invalid("That doesn't look like an API key: paste just the key, on one line."));
                }
                self.backend.set(&keychain_account(kind), k).map_err(keychain_error)?;
                config.set_key_saved(kind, true);
            }
            None => {
                self.backend.delete(&keychain_account(kind)).map_err(keychain_error)?;
                config.set_key_saved(kind, false);
            }
        }
        Ok(())
    }
}

fn keychain_error(e: penguin_provider::Error) -> CmdError {
    tracing::warn!(error = %e, "agent chat API key: Keychain error");
    CmdError::other(format!("The Keychain refused the API key ({e})."))
}
