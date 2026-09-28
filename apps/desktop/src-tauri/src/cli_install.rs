//! Settings → Developer → "Install command-line tool": symlink
//! `~/.local/bin/penguin` to the `penguin-cli` shipped next to the app
//! binary (Contents/MacOS in the bundle). No admin rights needed, and the
//! link keeps working across updates because it points into the bundle.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{CmdError, CmdResult, ErrorCode};

pub const LINK_NAME: &str = "penguin";
const BIN_DIR: &str = ".local/bin";
/// Asking the login shell for PATH runs the user's rc files; don't hang on them.
const SHELL_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliLinkStatus {
    /// `~/.local/bin/penguin`
    pub link_path: String,
    /// The bundled CLI; null when this build has none (plain `cargo build`).
    pub target: Option<String>,
    /// The link exists and points at `target`.
    pub installed: bool,
    /// Something else is at `link_path` (another install, a stale link).
    pub conflict: Option<String>,
    /// Whether ~/.local/bin is on the login shell's PATH; null if unknown.
    pub on_path: Option<bool>,
    /// The exact line to run to put ~/.local/bin on PATH for this shell.
    pub path_line: String,
}

/// `penguin-cli` next to the running app binary.
pub fn bundled_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cli = exe.parent()?.join("penguin-cli");
    cli.is_file().then_some(cli)
}

fn bin_dir(home: &Path) -> PathBuf {
    home.join(BIN_DIR)
}

/// The shell command that adds ~/.local/bin to PATH, for `$SHELL`.
pub fn path_line(shell: Option<&str>) -> String {
    let name = shell
        .and_then(|s| Path::new(s).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("zsh");
    match name {
        "fish" => "fish_add_path ~/.local/bin".to_string(),
        "bash" => r#"echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bash_profile"#.to_string(),
        _ => r#"echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc"#.to_string(),
    }
}

/// PATH as the user's login shell sees it. GUI apps get launchd's minimal
/// PATH, so our own environment says nothing about the user's terminal.
/// None on timeout or failure.
pub fn login_shell_path(shell: &str) -> Option<String> {
    const START: &str = "__PENGUIN_PATH_START__";
    const END: &str = "__PENGUIN_PATH_END__";
    let mut child = Command::new(shell)
        .args([
            "-l",
            "-i",
            "-c",
            &format!("printf '{START}%s{END}' \"$PATH\""),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < SHELL_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    // rc files may print banners; take what's between the markers.
    let start = out.find(START)? + START.len();
    let end = out[start..].find(END)? + start;
    Some(out[start..end].to_string())
}

fn dir_on_path(dir: &Path, path: &str, home: &Path) -> bool {
    path.split(':').any(|p| {
        let p = p.trim_end_matches('/');
        let expanded = p
            .strip_prefix("~")
            .or_else(|| p.strip_prefix("$HOME"))
            .map(|rest| format!("{}{rest}", home.display()))
            .unwrap_or_else(|| p.to_string());
        Path::new(&expanded) == dir
    })
}

/// Status of the link. `login_path` is the login shell's PATH if known.
pub fn status(
    home: &Path,
    target: Option<&Path>,
    login_path: Option<&str>,
    shell: Option<&str>,
) -> CliLinkStatus {
    let link = bin_dir(home).join(LINK_NAME);
    let current = std::fs::read_link(&link).ok();
    let exists = link.symlink_metadata().is_ok();
    let installed = matches!((&current, target), (Some(c), Some(t)) if c == t);
    let conflict = if exists && !installed {
        Some(match &current {
            Some(c) => format!("{} points to {}", link.display(), c.display()),
            None => format!("{} exists and isn't a link", link.display()),
        })
    } else {
        None
    };
    CliLinkStatus {
        link_path: link.display().to_string(),
        target: target.map(|t| t.display().to_string()),
        installed,
        conflict,
        on_path: login_path.map(|p| dir_on_path(&bin_dir(home), p, home)),
        path_line: path_line(shell),
    }
}

/// Create (or re-point) `~/.local/bin/penguin` → `target`. Replaces an
/// existing symlink (e.g. to an older install location) but never a real
/// file or directory.
pub fn install(home: &Path, target: &Path) -> CmdResult<()> {
    let dir = bin_dir(home);
    std::fs::create_dir_all(&dir)?;
    let link = dir.join(LINK_NAME);
    if let Ok(meta) = link.symlink_metadata() {
        if !meta.file_type().is_symlink() {
            return Err(CmdError::new(
                ErrorCode::InvalidInput,
                format!(
                    "{} already exists and isn't a link; move it away first",
                    link.display()
                ),
            ));
        }
    }
    // Build the link beside it and rename over: never a moment without one.
    let tmp = dir.join(format!(".{LINK_NAME}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &tmp)?;
    #[cfg(not(unix))]
    return Err(CmdError::other(
        "installing the command-line tool is supported on macOS only",
    ));
    #[cfg(unix)]
    {
        std::fs::rename(&tmp, &link)?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let h =
            std::env::temp_dir().join(format!("penguin-cli-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(&h).unwrap();
        h
    }

    #[test]
    fn installs_repoints_and_refuses_real_files() {
        let h = home("link");
        let target = h.join("Penguin.app/Contents/MacOS/penguin-cli");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "#!/bin/sh\n").unwrap();

        let before = status(&h, Some(&target), None, Some("/bin/zsh"));
        assert!(!before.installed && before.conflict.is_none() && before.on_path.is_none());

        install(&h, &target).unwrap(); // creates ~/.local/bin too
        let after = status(
            &h,
            Some(&target),
            Some("/usr/bin:~/.local/bin"),
            Some("/bin/zsh"),
        );
        assert!(after.installed, "{after:?}");
        assert_eq!(after.on_path, Some(true));

        // A stale link to an old location is re-pointed.
        let old = h.join("old-penguin-cli");
        std::fs::remove_file(h.join(".local/bin/penguin")).unwrap();
        std::os::unix::fs::symlink(&old, h.join(".local/bin/penguin")).unwrap();
        assert!(status(&h, Some(&target), None, None)
            .conflict
            .unwrap()
            .contains("points to"));
        install(&h, &target).unwrap();
        assert!(status(&h, Some(&target), None, None).installed);

        // A real file is never replaced.
        std::fs::remove_file(h.join(".local/bin/penguin")).unwrap();
        std::fs::write(h.join(".local/bin/penguin"), "mine").unwrap();
        assert!(install(&h, &target).is_err());
        assert_eq!(
            std::fs::read_to_string(h.join(".local/bin/penguin")).unwrap(),
            "mine"
        );
        let _ = std::fs::remove_dir_all(h);
    }

    #[test]
    fn path_detection_and_lines() {
        let h = PathBuf::from("/Users/ada");
        let dir = h.join(".local/bin");
        assert!(dir_on_path(&dir, "/usr/bin:/Users/ada/.local/bin/", &h));
        assert!(dir_on_path(&dir, "$HOME/.local/bin:/bin", &h));
        assert!(!dir_on_path(&dir, "/usr/bin:/usr/local/bin", &h));
        assert!(path_line(Some("/opt/homebrew/bin/fish")).starts_with("fish_add_path"));
        assert!(path_line(Some("/bin/bash")).ends_with("~/.bash_profile"));
        assert!(path_line(None).ends_with("~/.zshrc"));
    }

    #[test]
    fn login_shell_path_reads_between_markers() {
        let path = login_shell_path("/bin/sh").expect("sh prints PATH");
        assert!(path.contains("/bin"), "{path}");
        assert!(login_shell_path("/nonexistent/shell").is_none());
    }
}
