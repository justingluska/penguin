//! Minimal HTTP listener on 127.0.0.1 for the OAuth redirect.
//!
//! Browsers open speculative connections that never send a request, so every
//! connection is read on its own task with a deadline, and only well-formed
//! `GET /callback?...` requests reach the sign-in flow (over a channel, with
//! the stream kept open so the page can reflect the final outcome).

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{Error, Result};

pub(crate) const CALLBACK_PATH: &str = "/callback";
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
    /// Headers not finished yet; read more.
    Incomplete,
    Malformed,
}

/// Parse the request line of an HTTP/1.x request head. Only needs the bytes
/// up to the blank line; the body (there is none for GET) is ignored.
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
        url::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| ParseError::Malformed)?;
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

/// What Google sent back to the redirect URI.
#[derive(PartialEq, Eq)]
pub(crate) enum Callback {
    Code { code: String, state: String },
    Denied { error: String, state: String },
}

impl Callback {
    pub fn from_query(query: &HashMap<String, String>) -> Option<Self> {
        let state = query.get("state").cloned().unwrap_or_default();
        if let Some(error) = query.get("error") {
            return Some(Self::Denied {
                error: error.clone(),
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
    accept_task: JoinHandle<()>,
}

impl LoopbackServer {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| Error::OAuth(format!("could not open loopback listener: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::OAuth(format!("loopback listener has no address: {e}")))?
            .port();
        let (tx, rx) = mpsc::channel(8);
        let accept_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(handle_connection(stream, tx.clone()));
            }
        });
        Ok(Self {
            port,
            rx,
            accept_task,
        })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The next callback request. The stream is left open for [`respond`].
    pub async fn next_callback(&mut self) -> Option<(Callback, TcpStream)> {
        self.rx.recv().await
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

async fn handle_connection(mut stream: TcpStream, tx: mpsc::Sender<(Callback, TcpStream)>) {
    let head = match tokio::time::timeout(READ_DEADLINE, read_head(&mut stream)).await {
        Ok(Some(head)) => head,
        // Idle preconnect, garbage, or oversized: just drop it.
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
    if head.path != CALLBACK_PATH {
        respond(stream, 404, "").await;
        return;
    }
    match Callback::from_query(&head.query) {
        Some(callback) => {
            // Receiver gone means sign-in already finished or was abandoned.
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

/// Write a complete response and close. Failures only mean the browser tab
/// went away, which doesn't affect the sign-in result.
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

/// Closes the tab once Penguin has taken focus. Browsers only honor
/// `window.close()` for script-opened tabs, so this often does nothing; the
/// copy on the page covers that case. Allowed by hash in the CSP.
const CLOSE_SCRIPT: &str = "setTimeout(function(){window.close()},800)";

/// `sha256-…` CSP source for [`CLOSE_SCRIPT`].
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
/// Success pages also try to close themselves.
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
    fn parses_callback_request() {
        let raw = b"GET /callback?state=abc&code=4%2F0Ab-x&scope=email%20openid HTTP/1.1\r\nHost: 127.0.0.1:5555\r\nUser-Agent: test\r\n\r\n";
        let head = parse_request_head(raw).unwrap();
        assert_eq!(head.method, "GET");
        assert_eq!(head.path, "/callback");
        assert_eq!(head.query["code"], "4/0Ab-x");
        assert_eq!(head.query["scope"], "email openid");
        assert_eq!(
            Callback::from_query(&head.query),
            Some(Callback::Code {
                code: "4/0Ab-x".into(),
                state: "abc".into()
            })
        );
    }

    #[test]
    fn parses_error_callback() {
        let raw = b"GET /callback?error=access_denied&state=s1 HTTP/1.1\r\n\r\n";
        let head = parse_request_head(raw).unwrap();
        let cb = Callback::from_query(&head.query).unwrap();
        assert_eq!(
            cb,
            Callback::Denied {
                error: "access_denied".into(),
                state: "s1".into()
            }
        );
        assert_eq!(cb.state(), "s1");
    }

    #[test]
    fn incomplete_until_blank_line() {
        assert_eq!(
            parse_request_head(b"GET /callback?code=1 HTTP/1.1\r\nHost: x\r\n"),
            Err(ParseError::Incomplete)
        );
        assert!(parse_request_head(b"GET /callback?code=1 HTTP/1.1\n\n").is_ok());
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
            parse_request_head(b"GET / SPDY/3\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse_request_head(b"GET / HTTP/1.1 extra\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse_request_head(b"GET /\xff HTTP/1.1\r\n\r\n"),
            Err(ParseError::Malformed)
        );
    }

    #[test]
    fn non_callback_paths_have_no_callback() {
        let head = parse_request_head(b"GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(head.path, "/favicon.ico");
        assert!(Callback::from_query(&head.query).is_none());
    }

    #[test]
    fn callback_debug_redacts_code() {
        let cb = Callback::Code {
            code: "4/secret-code".into(),
            state: "s".into(),
        };
        assert!(!format!("{cb:?}").contains("secret-code"));
    }

    #[test]
    fn success_page_script_matches_csp_hash() {
        let html = page(PageKind::Success, "Signed in", "x");
        assert!(html.contains(&format!("<script>{CLOSE_SCRIPT}</script>")));
        assert!(!page(PageKind::Error, "Oops", "x").contains("<script>"));
        // Precomputed independently (Python hashlib) from the script bytes.
        assert_eq!(
            close_script_hash(),
            "sha256-dugbY/27KPxjwwJcbDcFVe+2cTvs8aT0QC4Re0lo6r8="
        );
    }

    #[test]
    fn page_escapes_detail() {
        let html = page(PageKind::Error, "Oops", "<script>alert(1)</script>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[tokio::test]
    async fn server_routes_only_callbacks() {
        let mut server = LoopbackServer::bind().await.unwrap();
        let port = server.port();
        assert!(server
            .redirect_uri()
            .ends_with(&format!(":{port}/callback")));

        // An idle connection (browser preconnect) must not block others.
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
            .write_all(b"GET /callback?code=c1&state=s1 HTTP/1.1\r\n\r\n")
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
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.ends_with("done"));
    }
}
