//! Finding the user's own Claude Code and Codex CLIs. A GUI app gets
//! launchd's minimal PATH, so we look in the login shell's PATH and then in
//! the places the installers use. "Signed in" is a guess from files and
//! variables that exist; no credential is ever read, logged or returned.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// `--version` of a node-based CLI starts node; don't wait on a broken one.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentCliKind {
    Claude,
    Codex,
}

impl AgentCliKind {
    pub const ALL: [AgentCliKind; 2] = [AgentCliKind::Claude, AgentCliKind::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentCliKind::Claude => "claude",
            AgentCliKind::Codex => "codex",
        }
    }
    /// The executable's name.
    pub fn binary(self) -> &'static str {
        self.as_str()
    }
    /// What Settings and the panel call it.
    pub fn product(self) -> &'static str {
        match self {
            AgentCliKind::Claude => "Claude Code",
            AgentCliKind::Codex => "Codex",
        }
    }
    /// The variable an API key goes in for the child process.
    pub fn api_key_vars(self) -> &'static [&'static str] {
        match self {
            AgentCliKind::Claude => &["ANTHROPIC_API_KEY"],
            // `codex exec` reads CODEX_API_KEY; OPENAI_API_KEY for older builds.
            AgentCliKind::Codex => &["CODEX_API_KEY", "OPENAI_API_KEY"],
        }
    }
}

/// Where a CLI was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FoundIn {
    /// On the login shell's PATH.
    Path,
    /// A usual install location that isn't on PATH.
    Common,
    /// The path the user typed in Settings.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCliFound {
    pub kind: AgentCliKind,
    pub path: String,
    /// "2.1.289"; null when `--version` failed or timed out.
    pub version: Option<String>,
    /// Signed in, as far as files and variables tell; null = can't tell.
    pub signed_in: Option<bool>,
    pub found_in: FoundIn,
}

/// The directories to look in for `kind`, in order: the login shell's PATH,
/// then the installers' usual places under `home`.
pub fn search_dirs(kind: AgentCliKind, home: &Path, login_path: Option<&str>) -> Vec<(PathBuf, FoundIn)> {
    let mut dirs: Vec<(PathBuf, FoundIn)> = Vec::new();
    let mut push = |p: PathBuf, from: FoundIn| {
        if !p.as_os_str().is_empty() && !dirs.iter().any(|(d, _)| *d == p) {
            dirs.push((p, from));
        }
    };
    for p in login_path.unwrap_or("").split(':') {
        let p = p.trim();
        let expanded = p
            .strip_prefix("~/")
            .or_else(|| p.strip_prefix("$HOME/"))
            .map(|rest| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(p));
        push(expanded, FoundIn::Path);
    }
    if kind == AgentCliKind::Claude {
        // The native installer and `claude migrate-installer`.
        push(home.join(".claude/local"), FoundIn::Common);
    }
    for rel in [".local/bin", ".npm-global/bin", ".bun/bin", ".volta/bin", "Library/pnpm", ".cargo/bin"] {
        push(home.join(rel), FoundIn::Common);
    }
    for abs in ["/opt/homebrew/bin", "/usr/local/bin"] {
        push(PathBuf::from(abs), FoundIn::Common);
    }
    // nvm: every installed node's bin, newest name first.
    if let Ok(rd) = std::fs::read_dir(home.join(".nvm/versions/node")) {
        let mut nodes: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("bin")).collect();
        nodes.sort();
        for p in nodes.into_iter().rev() {
            push(p, FoundIn::Common);
        }
    }
    dirs
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// The first `kind` binary in `dirs`.
pub fn locate(kind: AgentCliKind, dirs: &[(PathBuf, FoundIn)]) -> Option<(PathBuf, FoundIn)> {
    dirs.iter()
        .map(|(d, from)| (d.join(kind.binary()), *from))
        .find(|(p, _)| is_executable(p))
}

/// "2.1.289 (Claude Code)" → "2.1.289"; "codex-cli 0.159.0" → "0.159.0".
pub fn parse_version(out: &str) -> Option<String> {
    let line = out.lines().find(|l| !l.trim().is_empty())?;
    line.split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))
        .map(str::to_string)
}

/// PATH for a child: the login shell's (node-based CLIs need `node` from
/// it), then the CLI's own directory, then ours.
pub fn child_path(cli: &Path, login_path: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(p) = login_path.filter(|p| !p.is_empty()) {
        parts.push(p.to_string());
    }
    if let Some(dir) = cli.parent() {
        parts.push(dir.display().to_string());
    }
    if let Ok(own) = std::env::var("PATH") {
        parts.push(own);
    }
    parts.join(":")
}

/// `<cli> --version`, killed after [`VERSION_TIMEOUT`]. Blocking.
pub fn version_of(cli: &Path, login_path: Option<&str>) -> Option<String> {
    let mut child = Command::new(cli)
        .arg("--version")
        .env("PATH", child_path(cli, login_path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < VERSION_TIMEOUT => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    parse_version(&out)
}

/// Whether `kind` looks signed in for this user: its credential files exist
/// (checked for existence only; `~/.claude.json` only for the word
/// "oauthAccount"), or an API key is in the environment or saved in
/// Penguin. False means none of that was found, which may still be wrong
/// (a login only in the Keychain), so the UI says "may need signing in".
pub fn looks_signed_in(kind: AgentCliKind, home: &Path, env: &dyn Fn(&str) -> Option<String>, key_saved: bool) -> bool {
    if key_saved || kind.api_key_vars().iter().any(|v| env(v).is_some_and(|s| !s.is_empty())) {
        return true;
    }
    match kind {
        AgentCliKind::Claude => {
            home.join(".claude/.credentials.json").is_file()
                || std::fs::read_to_string(home.join(".claude.json")).is_ok_and(|s| s.contains("\"oauthAccount\""))
        }
        AgentCliKind::Codex => {
            let codex_home = env("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"));
            codex_home.join("auth.json").is_file()
        }
    }
}

/// Everything found: one entry per kind (the custom path first when it
/// is set and runs). Blocking: runs `--version`.
pub fn detect(
    home: &Path,
    login_path: Option<&str>,
    custom: Option<(AgentCliKind, &Path)>,
    env: &dyn Fn(&str) -> Option<String>,
    keys_saved: &dyn Fn(AgentCliKind) -> bool,
) -> Vec<AgentCliFound> {
    let mut out = Vec::new();
    for kind in AgentCliKind::ALL {
        let custom_hit = custom
            .filter(|(k, p)| *k == kind && is_executable(p))
            .map(|(_, p)| (p.to_path_buf(), FoundIn::Custom));
        let Some((path, found_in)) = custom_hit.or_else(|| locate(kind, &search_dirs(kind, home, login_path))) else {
            continue;
        };
        out.push(AgentCliFound {
            kind,
            version: version_of(&path, login_path),
            signed_in: Some(looks_signed_in(kind, home, env, keys_saved(kind))),
            path: path.display().to_string(),
            found_in,
        });
    }
    out
}
