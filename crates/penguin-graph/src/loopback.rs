//! The loopback listener for Microsoft's redirect (adapted from
//! penguin-gmail's `auth/loopback.rs`; copied, not shared, per the provider
//! brief).
//!
//! The app registration's redirect is `http://localhost` on the "Mobile and
//! desktop applications" platform; Microsoft ignores the port for
//! localhost, so the request's `redirect_uri` is `http://localhost:<port>`
//! and the code arrives at `/`. `localhost` may resolve to 127.0.0.1 or
//! ::1, so the port is bound on both when IPv6 is available.
//!
//! Browsers open speculative connections that never send a request, so
//! every connection is read on its own task with a deadline, and only
//! well-formed `GET /?…` requests reach the sign-in flow (with the stream
//! kept open so the page can reflect the final outcome).

use std::collections::HashMap;
use std::time::Duration;

use penguin_provider::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const MAX_HEAD_BYTES: usize = 16 * 1024;
const READ_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RequestHead {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    Incomplete,
    Malformed,
}

pub(crate) fn parse_request_head(buf: &[u8]) -> std::result::Result<RequestHead, ParseError> {
    let end = find_head_end(buf).ok_or(ParseError::Incomplete)?;
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| ParseError::Malformed)?;
    let request_line = head.lines().next().ok_or(ParseError::Malformed)?;
    let mut parts = request_line.split_ascii_whitespace();
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::Malformed);
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(ParseError::Malformed);
    }
    let url =
        url::Url::parse(&format!("http://localhost{target}")).map_err(|_| ParseError::Malformed)?;
    Ok(RequestHead {
        method: method.to_owned(),
        path: url.path().to_owned(),
        query: url.query_pairs().into_owned().collect(),
    })
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n"))
}

/// What Microsoft sent back to the redirect URI.
#[derive(PartialEq, Eq)]
pub(crate) enum Callback {
    Code {
        code: String,
        state: String,
    },
    Denied {
        error: String,
        /// `error_description` (starts with the AADSTS code). Never logged.
        description: String,
        /// `error_subcode` (e.g. `cancel`).
        subcode: String,
        state: String,
    },
}

impl Callback {
    pub fn from_query(query: &HashMap<String, String>) -> Option<Self> {
        let state = query.get("state").cloned().unwrap_or_default();
        if let Some(error) = query.get("error") {
            return Some(Self::Denied {
                error: error.clone(),
                description: query.get("error_description").cloned().unwrap_or_default(),
                subcode: query.get("error_subcode").cloned().unwrap_or_default(),
                state,
            });
        }
        query.get("code").map(|code| Self::Code {
            code: code.clone(),
            state,
        })
    }

    pub fn state(&self) -> &str {
        match self {
            Self::Code { state, .. } | Self::Denied { state, .. } => state,
        }
    }
}

impl std::fmt::Debug for Callback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code { .. } => f.write_str("Callback::Code(<redacted>)"),
            Self::Denied { error, .. } => write!(f, "Callback::Denied({error})"),
        }
    }
}

pub(crate) struct LoopbackServer {
    port: u16,
    rx: mpsc::Receiver<(Callback, TcpStream)>,
    tasks: Vec<JoinHandle<()>>,
}

impl LoopbackServer {
    pub async fn bind() -> Result<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| Error::OAuth(format!("could not open loopback listener: {e}")))?;
        let port = v4
            .local_addr()
            .map_err(|e| Error::OAuth(format!("loopback listener has no address: {e}")))?
            .port();
        let (tx, rx) = mpsc::channel(8);
        let mut tasks = vec![tokio::spawn(accept_loop(v4, tx.clone()))];
        // Browsers may resolve localhost to ::1 first. Best effort: a Mac
        // without IPv6 on loopback still has the IPv4 listener.
        match TcpListener::bind(("::1", port)).await {
            Ok(v6) => tasks.push(tokio::spawn(accept_loop(v6, tx))),
            Err(e) => tracing::debug!(error = %e, "no IPv6 loopback listener for sign-in"),
        }
        Ok(Self { port, rx, tasks })
    }

    /// `http://localhost:<port>`: matches the registered `http://localhost`
    /// (Microsoft ignores the port for localhost redirects).
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn next_callback(&mut self) -> Option<(Callback, TcpStream)> {
        self.rx.recv().await
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

async fn accept_loop(listener: TcpListener, tx: mpsc::Sender<(Callback, TcpStream)>) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, tx.clone()));
    }
}

async fn handle_connection(mut stream: TcpStream, tx: mpsc::Sender<(Callback, TcpStream)>) {
    let head = match tokio::time::timeout(READ_DEADLINE, read_head(&mut stream)).await {
        Ok(Some(head)) => head,
        _ => return,
    };
    if head.method != "GET" {
        respond(
            stream,
            405,
            &page(PageKind::Error, "Unsupported request", ""),
        )
        .await;
        return;
    }
    if head.path != "/" {
        respond(stream, 404, "").await;
        return;
    }
    match Callback::from_query(&head.query) {
        Some(callback) => {
            let _ = tx.send((callback, stream)).await;
        }
        None => {
            let body = page(
                PageKind::Error,
                "Missing sign-in response",
                "Return to Penguin and try again.",
            );
            respond(stream, 400, &body).await;
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> Option<RequestHead> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        match parse_request_head(&buf) {
            Ok(head) => return Some(head),
            Err(ParseError::Incomplete) if buf.len() < MAX_HEAD_BYTES => continue,
            Err(_) => return None,
        }
    }
}

pub(crate) async fn respond(mut stream: TcpStream, status: u16, html: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src '{}'\r\n\
         Connection: close\r\n\r\n{html}",
        html.len(),
        close_script_hash(),
    );
    if let Err(e) = stream.write_all(response.as_bytes()).await {
        tracing::debug!(error = %e, "loopback response not delivered");
    }
    let _ = stream.shutdown().await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageKind {
    Success,
    Error,
}

const CLOSE_SCRIPT: &str = "setTimeout(function(){window.close()},800)";

fn close_script_hash() -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(CLOSE_SCRIPT.as_bytes());
    format!(
        "sha256-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// The small self-contained page shown in the browser tab after the redirect.
pub(crate) fn page(kind: PageKind, title: &str, detail: &str) -> String {
    let (mark, mark_class) = match kind {
        PageKind::Success => ("&#10003;", "ok"),
        PageKind::Error => ("!", "err"),
    };
    let title = html_escape(title);
    let detail = html_escape(detail);
    let script = match kind {
        PageKind::Success => format!("<script>{CLOSE_SCRIPT}</script>"),
        PageKind::Error => String::new(),
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Penguin</title>
<style>
:root{{color-scheme:light dark;--bg:#fcfcfd;--fg:#1c2024;--muted:#60646c;--line:#0000000f;--ok:#0090ff;--err:#e5484d}}
@media (prefers-color-scheme:dark){{:root{{--bg:#0c0d0f;--fg:#edeef0;--muted:#9ba1a6;--line:#ffffff12}}}}
*{{box-sizing:border-box}}
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);
font:14px/1.5 -apple-system,BlinkMacSystemFont,"Inter","Segoe UI",system-ui,sans-serif;-webkit-font-smoothing:antialiased}}
main{{width:min(360px,calc(100vw - 32px));padding:32px 28px;border:1px solid var(--line);border-radius:14px;text-align:center}}
.brand{{font-size:12px;letter-spacing:.08em;text-transform:uppercase;color:var(--muted);margin-bottom:20px}}
.mark{{width:40px;height:40px;margin:0 auto 16px;border-radius:50%;display:grid;place-items:center;color:#fff;font-weight:600}}
.ok{{background:var(--ok)}}.err{{background:var(--err)}}
h1{{font-size:17px;font-weight:600;margin:0 0 6px}}
p{{margin:0;color:var(--muted)}}
</style></head>
<body><main>
<div class="brand">Penguin</div>
<div class="mark {mark_class}">{mark}</div>
<h1>{title}</h1>
<p>{detail}</p>
</main>{script}</body></html>
"#
    )
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_microsoft_redirects() {
        let raw = b"GET /?code=M.C5_BAY.2.U.abc&state=s1 HTTP/1.1\r\nHost: localhost:5555\r\n\r\n";
        let head = parse_request_head(raw).unwrap();
        assert_eq!(head.path, "/");
        assert_eq!(
            Callback::from_query(&head.query),
            Some(Callback::Code {
                code: "M.C5_BAY.2.U.abc".into(),
                state: "s1".into()
            })
        );
        let raw = b"GET /?error=access_denied&error_subcode=cancel&error_description=AADSTS65004%3a+User+declined&state=s2 HTTP/1.1\r\n\r\n";
        let head = parse_request_head(raw).unwrap();
        let cb = Callback::from_query(&head.query).unwrap();
        match &cb {
            Callback::Denied {
                error,
                description,
                subcode,
                state,
            } => {
                assert_eq!(error, "access_denied");
                assert!(description.starts_with("AADSTS65004"));
                assert_eq!(subcode, "cancel");
                assert_eq!(state, "s2");
            }
            other => panic!("{other:?}"),
        }
        assert!(!format!("{cb:?}").contains("AADSTS"));
    }

    #[test]
    fn rejects_malformed_requests() {
        assert_eq!(
            parse_request_head(b"GARBAGE\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse_request_head(b"GET http://evil/ HTTP/1.1\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse_request_head(b"GET /?code=1 HTTP/1.1\r\nHost: x\r\n"),
            Err(ParseError::Incomplete)
        );
    }

    #[tokio::test]
    async fn listener_routes_root_callbacks_only() {
        let mut server = LoopbackServer::bind().await.unwrap();
        let port = server.port();
        assert_eq!(server.redirect_uri(), format!("http://localhost:{port}"));
        let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut favicon = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        favicon
            .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        favicon.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 404"));

        let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        client
            .write_all(b"GET /?code=c1&state=s1 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let (cb, stream) = server.next_callback().await.unwrap();
        assert_eq!(
            cb,
            Callback::Code {
                code: "c1".into(),
                state: "s1".into()
            }
        );
        respond(stream, 200, "done").await;
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK") && resp.ends_with("done"));
    }

    #[test]
    fn page_escapes() {
        let html = page(PageKind::Error, "Oops", "<script>x</script>");
        assert!(!html.contains("<script>x"));
        assert!(page(PageKind::Success, "Signed in", "").contains("<script>setTimeout"));
    }
}
