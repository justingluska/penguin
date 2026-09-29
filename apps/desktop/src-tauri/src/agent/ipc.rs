//! The agent socket: how `penguin-cli` (and its MCP server) asks the running
//! Penguin app to do what the CLI process may not do itself: save, change,
//! delete and send drafts, and download an attachment that isn't cached.
//! The CLI never holds credentials; the app does the provider work with its
//! own tokens after checking the agent level (agent/permission.rs).
//!
//! - **Where:** `<data dir>/agent/penguin.sock`, a Unix domain socket, in a
//!   directory the app creates `0700` (the socket itself is `0600`), so
//!   only the same user can reach it.
//! - **Who:** every connection's peer uid must be the app's own (checked
//!   with the kernel's peer credentials), and every request carries the
//!   per-install token from `<data dir>/agent/token` (`0600`, 32 random
//!   bytes as hex, created once), compared in constant time. The token is
//!   a second lock on the same door: a process running as the user can
//!   read it, and that is the boundary (docs/SECURITY.md → Agents).
//! - **Wire:** one request per connection, one JSON line each way:
//!   `{"v":1,"token":"…","client":"mcp"|"cli","tool":"create_draft","args":{…}}`
//!   → `{"ok":true,"data":…}` or `{"ok":false,"error":{"code","message"}}`
//!   with `code` from `ErrorCode`. Requests are capped at 48 MB (inline
//!   attachments travel as base64).
//! - **Not running:** no socket, or nothing listening on it, is
//!   `ErrorCode::Unavailable`; penguin-cli exits 69 and never starts the
//!   app.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use penguin_provider::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops::Paths;

pub const AGENT_DIR: &str = "agent";
pub const SOCKET_FILE: &str = "penguin.sock";
pub const TOKEN_FILE: &str = "token";
/// Bumped when a request or response changes incompatibly.
pub const PROTOCOL: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 48 << 20;
pub const MAX_RESPONSE_BYTES: usize = 48 << 20;
/// How long the app waits for a connected client to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the CLI waits for an answer (an upload of 25 MB of attachments
/// on a slow link takes a while).
const CALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Requests handled at once; more wait their turn.
const MAX_CONNECTIONS: usize = 8;
/// macOS `sun_path` holds 104 bytes including the terminating NUL.
const MAX_SOCKET_PATH: usize = 103;

pub const NOT_RUNNING: &str = "Penguin isn't running. Open Penguin on this Mac and try again \
    (penguin-cli never starts it by itself).";

pub fn agent_dir(paths: &Paths) -> PathBuf {
    paths.data_dir.join(AGENT_DIR)
}

pub fn socket_path(paths: &Paths) -> PathBuf {
    agent_dir(paths).join(SOCKET_FILE)
}

pub fn token_path(paths: &Paths) -> PathBuf {
    agent_dir(paths).join(TOKEN_FILE)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub token: String,
    /// `mcp` or `cli`, for the audit log.
    pub client: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireError {
    code: String,
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Response {
    ok: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<WireError>,
}

impl Response {
    fn from_result(r: CmdResult<Value>) -> Response {
        match r {
            Ok(data) => Response {
                ok: true,
                data,
                error: None,
            },
            Err(e) => Response {
                ok: false,
                data: Value::Null,
                error: Some(WireError {
                    code: serde_json::to_value(e.code)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_else(|| "other".into()),
                    message: e.message,
                }),
            },
        }
    }
}

/// What the app does with an authenticated request.
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn handle(&self, client: &str, tool: &str, args: Value) -> CmdResult<Value>;
}

// ---------- files ----------

#[cfg(unix)]
fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Create `dir` (and parents) and make it private: a real directory (not a
/// symlink), owned by this user, mode 0700.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(std::io::Error::other(format!(
            "{} isn't a directory",
            dir.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != euid() {
            return Err(std::io::Error::other(format!(
                "{} belongs to another user",
                dir.display()
            )));
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn valid_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The install's token: reused while it's well-formed and private, else a
/// new random one, written 0600 (temp file + rename).
pub fn ensure_token(path: &Path) -> std::io::Result<String> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let t = text.trim();
        #[cfg(unix)]
        let private = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o077 == 0)
                .unwrap_or(false)
        };
        #[cfg(not(unix))]
        let private = true;
        if valid_token(t) && private {
            return Ok(t.to_string());
        }
    }
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| std::io::Error::other(e.to_string()))?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let written = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    Ok(token)
}

/// Equal-length strings compared without an early exit.
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

// ---------- server (the app) ----------

/// A bound agent socket, ready to [`Listener::serve`].
pub struct Listener {
    std: std::os::unix::net::UnixListener,
    token: String,
    socket: PathBuf,
}

impl Listener {
    /// Make the private directory and token, and bind the socket. A socket
    /// file left by an app that exited is replaced; one another running
    /// Penguin is serving is an error (AddrInUse).
    pub fn bind(paths: &Paths) -> std::io::Result<Listener> {
        let dir = agent_dir(paths);
        ensure_private_dir(&dir)?;
        let token = ensure_token(&token_path(paths))?;
        let socket = socket_path(paths);
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(std::io::Error::other(format!(
                "the agent socket path is too long for macOS ({} bytes): {}",
                socket.as_os_str().len(),
                socket.display()
            )));
        }
        if let Ok(meta) = std::fs::symlink_metadata(&socket) {
            use std::os::unix::fs::FileTypeExt;
            if !meta.file_type().is_socket() {
                return Err(std::io::Error::other(format!(
                    "{} exists and isn't a socket",
                    socket.display()
                )));
            }
            if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "another Penguin is already serving agents",
                ));
            }
            std::fs::remove_file(&socket)?;
        }
        let std = std::os::unix::net::UnixListener::bind(&socket)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        }
        std.set_nonblocking(true)?;
        Ok(Listener { std, token, socket })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Accept connections until the process ends. Each one is checked (same
    /// user, right token) before `handler` sees it.
    pub async fn serve<H: Handler>(self, handler: Arc<H>) -> std::io::Result<()> {
        let listener = tokio::net::UnixListener::from_std(self.std)?;
        let token: Arc<str> = self.token.into();
        let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "agent socket accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let peer = stream.peer_cred().map(|c| c.uid()).ok();
            if peer != Some(euid()) {
                tracing::warn!(peer = ?peer, "agent socket: refused a connection from another user");
                continue;
            }
            let Ok(permit) = slots.clone().acquire_owned().await else {
                return Ok(());
            };
            let (token, handler) = (token.clone(), handler.clone());
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(e) = connection(stream, &token, handler.as_ref()).await {
                    tracing::debug!(error = %e, "agent connection ended early");
                }
            });
        }
    }
}

/// Read up to the first newline (or EOF); more than `max` bytes is an error.
async fn read_line<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok(buf);
        }
        if let Some(i) = chunk[..n].iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&chunk[..i]);
            if buf.len() > max {
                break;
            }
            return Ok(buf);
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > max {
            break;
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "message too large",
    ))
}

async fn connection<H: Handler + ?Sized>(
    stream: tokio::net::UnixStream,
    token: &str,
    handler: &H,
) -> std::io::Result<()> {
    let (mut r, mut w) = stream.into_split();
    let result = match tokio::time::timeout(READ_TIMEOUT, read_line(&mut r, MAX_REQUEST_BYTES))
        .await
    {
        Err(_) => Err(CmdError::invalid("no request within 30 seconds")),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => Err(CmdError::invalid(
            format!("request larger than {} MB", MAX_REQUEST_BYTES >> 20),
        )),
        Ok(Err(e)) => return Err(e),
        Ok(Ok(line)) => match serde_json::from_slice::<Request>(&line) {
            Err(e) => Err(CmdError::invalid(format!("malformed request: {e}"))),
            Ok(req) if req.v != PROTOCOL => Err(CmdError::invalid(format!(
                "penguin-cli speaks agent protocol {} and this Penguin speaks {PROTOCOL}; \
                 use the penguin-cli inside this Penguin.app",
                req.v
            ))),
            Ok(req) if !same_token(&req.token, token) => {
                tracing::warn!(tool = %req.tool, "agent request with a wrong token refused");
                Err(CmdError::denied(
                    "the agent token doesn't match this Penguin's (a penguin-cli from another install?)",
                ))
            }
            Ok(req) => handler.handle(&req.client, &req.tool, req.args).await,
        },
    };
    let mut line = serde_json::to_vec(&Response::from_result(result))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.shutdown().await
}

// ---------- client (penguin-cli) ----------

fn unavailable(detail: Option<&str>) -> CmdError {
    CmdError::new(
        ErrorCode::Unavailable,
        match detail {
            Some(d) => format!("{NOT_RUNNING} ({d})"),
            None => NOT_RUNNING.to_string(),
        },
    )
}

/// Ask the running app to do `tool`. Errors: `Unavailable` when Penguin
/// isn't running, `PermissionDenied` when the token is refused or the agent
/// level doesn't allow `tool`, otherwise what the app reported.
pub async fn call(paths: &Paths, client: &str, tool: &str, args: Value) -> CmdResult<Value> {
    let socket = socket_path(paths);
    let token = match std::fs::read_to_string(token_path(paths)) {
        Ok(t) => t.trim().to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(unavailable(None)),
        Err(e) => {
            return Err(CmdError::denied(format!(
                "can't read the agent token at {}: {e}",
                token_path(paths).display()
            )))
        }
    };
    let stream = match tokio::net::UnixStream::connect(&socket).await {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(unavailable(None))
        }
        Err(e) => return Err(unavailable(Some(&e.to_string()))),
    };
    let req = Request {
        v: PROTOCOL,
        token,
        client: client.to_string(),
        tool: tool.to_string(),
        args,
    };
    let exchange = async {
        let (mut r, mut w) = stream.into_split();
        let mut line = serde_json::to_vec(&req).map_err(|e| CmdError::other(e.to_string()))?;
        line.push(b'\n');
        w.write_all(&line)
            .await
            .map_err(|e| unavailable(Some(&e.to_string())))?;
        w.shutdown()
            .await
            .map_err(|e| unavailable(Some(&e.to_string())))?;
        let reply = read_line(&mut r, MAX_RESPONSE_BYTES)
            .await
            .map_err(|e| CmdError::other(format!("reading Penguin's answer: {e}")))?;
        if reply.is_empty() {
            return Err(unavailable(Some("Penguin closed the connection")));
        }
        let resp: Response = serde_json::from_slice(&reply)
            .map_err(|e| CmdError::other(format!("malformed answer from Penguin: {e}")))?;
        if resp.ok {
            return Ok(resp.data);
        }
        let err = resp.error.unwrap_or(WireError {
            code: "other".into(),
            message: "Penguin reported an error without details".into(),
        });
        let code = serde_json::from_value(Value::String(err.code)).unwrap_or(ErrorCode::Other);
        Err(CmdError::new(code, err.message))
    };
    tokio::time::timeout(CALL_TIMEOUT, exchange)
        .await
        .map_err(|_| {
            CmdError::new(
                ErrorCode::Network,
                "Penguin didn't answer within 10 minutes",
            )
        })?
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub fn temp_paths(tag: &str) -> Paths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Short: socket paths are limited to 103 bytes.
        let root = std::env::temp_dir().join(format!("pg-ipc-{tag}-{}", nanos % 1_000_000_000));
        Paths {
            data_dir: root.clone(),
            config_dir: root.clone(),
            cache_dir: root.join("cache"),
        }
    }

    struct Echo;

    #[async_trait]
    impl Handler for Echo {
        async fn handle(&self, client: &str, tool: &str, args: Value) -> CmdResult<Value> {
            match tool {
                "fail" => Err(CmdError::denied("not at this level")),
                _ => Ok(serde_json::json!({"client": client, "tool": tool, "args": args})),
            }
        }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    async fn raw(paths: &Paths, line: &str) -> Value {
        let mut s = tokio::net::UnixStream::connect(socket_path(paths))
            .await
            .unwrap();
        s.write_all(line.as_bytes()).await.unwrap();
        s.shutdown().await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        serde_json::from_str(out.trim()).unwrap()
    }

    #[tokio::test]
    async fn round_trip_auth_and_permissions() {
        let paths = temp_paths("rt");
        let listener = Listener::bind(&paths).unwrap();
        // Private by construction.
        assert_eq!(mode(&agent_dir(&paths)), 0o700);
        assert_eq!(mode(&token_path(&paths)), 0o600);
        assert_eq!(mode(listener.socket()), 0o600);
        let server = tokio::spawn(listener.serve(Arc::new(Echo)));

        let v = call(
            &paths,
            "cli",
            "create_draft",
            serde_json::json!({"subject": "Hi"}),
        )
        .await
        .unwrap();
        assert_eq!(v["client"], "cli");
        assert_eq!(v["args"]["subject"], "Hi");
        // The app's own errors come back typed.
        let e = call(&paths, "mcp", "fail", Value::Null).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert_eq!(e.message, "not at this level");

        // A wrong token (same length or not) never reaches the handler.
        for bad in ["0".repeat(64), "short".into(), String::new()] {
            let line = serde_json::to_string(&Request {
                v: PROTOCOL,
                token: bad,
                client: "cli".into(),
                tool: "create_draft".into(),
                args: Value::Null,
            })
            .unwrap();
            let resp = raw(&paths, &format!("{line}\n")).await;
            assert_eq!(resp["ok"], false);
            assert_eq!(resp["error"]["code"], "permissionDenied", "{resp}");
            assert!(resp.get("data").is_none());
        }
        // A token file an attacker swapped in after start doesn't help:
        // the server compares with the one it loaded.
        std::fs::write(token_path(&paths), "f".repeat(64)).unwrap();
        let e = call(&paths, "cli", "create_draft", Value::Null)
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);

        // Garbage, another protocol version.
        let resp = raw(&paths, "not json\n").await;
        assert_eq!(resp["error"]["code"], "invalidInput");
        let resp = raw(&paths, r#"{"v":99,"token":"x","client":"cli","tool":"t"}"#).await;
        assert!(resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("protocol"));
        server.abort();
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[tokio::test]
    async fn not_running_is_unavailable() {
        let paths = temp_paths("down");
        // Never started: no token, no socket.
        let e = call(&paths, "cli", "create_draft", Value::Null)
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable);
        assert!(e.message.contains("Penguin isn't running"), "{}", e.message);
        // Started once and quit: the token and a dead socket file remain.
        let listener = Listener::bind(&paths).unwrap();
        drop(listener);
        assert!(socket_path(&paths).exists());
        let e = call(&paths, "cli", "create_draft", Value::Null)
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable, "{}", e.message);
        // The next launch replaces the dead socket and keeps the token.
        let token = std::fs::read_to_string(token_path(&paths)).unwrap();
        let again = Listener::bind(&paths).unwrap();
        assert_eq!(std::fs::read_to_string(token_path(&paths)).unwrap(), token);
        let server = tokio::spawn(again.serve(Arc::new(Echo)));
        assert!(call(&paths, "cli", "x", Value::Null).await.is_ok());
        // While one is serving, a second can't take over the socket.
        let second = Listener::bind(&paths).map(|_| ()).unwrap_err();
        assert_eq!(second.kind(), std::io::ErrorKind::AddrInUse);
        server.abort();
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[tokio::test]
    async fn oversized_requests_are_refused() {
        let paths = temp_paths("big");
        let server = tokio::spawn(Listener::bind(&paths).unwrap().serve(Arc::new(Echo)));
        let huge = "x".repeat(MAX_REQUEST_BYTES + 10);
        let mut s = tokio::net::UnixStream::connect(socket_path(&paths))
            .await
            .unwrap();
        // The server may stop reading once it's over the cap; ignore a broken pipe.
        let _ = s.write_all(huge.as_bytes()).await;
        let _ = s.shutdown().await;
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("larger than"),
            "{v}"
        );
        server.abort();
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn tokens_are_private_random_and_repaired() {
        let paths = temp_paths("tok");
        ensure_private_dir(&agent_dir(&paths)).unwrap();
        let p = token_path(&paths);
        let a = ensure_token(&p).unwrap();
        assert!(valid_token(&a));
        assert_eq!(ensure_token(&p).unwrap(), a, "reused");
        // Made readable by others: replaced.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let b = ensure_token(&p).unwrap();
        assert_ne!(a, b);
        assert_eq!(mode(&p), 0o600);
        // Malformed: replaced.
        std::fs::write(&p, "nope").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(valid_token(&ensure_token(&p).unwrap()));
        assert!(same_token(&a, &a) && !same_token(&a, &b) && !same_token(&a, "x"));
        // A loose directory is tightened.
        std::fs::set_permissions(agent_dir(&paths), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        ensure_private_dir(&agent_dir(&paths)).unwrap();
        assert_eq!(mode(&agent_dir(&paths)), 0o700);
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }
}
