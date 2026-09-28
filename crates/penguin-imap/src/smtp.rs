//! SMTP submission (RFC 6409): implicit TLS (465) or STARTTLS (587), AUTH
//! PLAIN or LOGIN, one message per connection.
//!
//! Small on purpose: it shares the TLS setup (OS verifier, loopback
//! exception for Proton Mail Bridge) and the error mapping with the IMAP
//! side, and needs nothing else from an SMTP library.

use base64::Engine;
use penguin_core::{MailSecurity, ServerSettings};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::net::{self, BoxIo};
use crate::{redact, Error, Result};

/// A server reply: code and its (joined) text lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub code: u16,
    pub lines: Vec<String>,
}

impl Reply {
    fn text(&self) -> String {
        redact(&self.lines.join(" "))
    }
}

/// How an SMTP step failed, for the callers' messages.
#[derive(Debug)]
pub enum SmtpError {
    /// The server refused the credentials (535 and friends).
    Auth(String),
    /// Anything else, already mapped.
    Other(Error),
}

impl From<Error> for SmtpError {
    fn from(e: Error) -> Self {
        SmtpError::Other(e)
    }
}

impl From<SmtpError> for Error {
    fn from(e: SmtpError) -> Self {
        match e {
            SmtpError::Auth(text) => Error::NeedsReauth(format!("SMTP: {text}")),
            SmtpError::Other(e) => e,
        }
    }
}

type SResult<T> = std::result::Result<T, SmtpError>;

pub struct Smtp {
    io: BufReader<BoxIo>,
    host: String,
    ext: Vec<String>,
}

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Map a failing reply for step `what`.
fn reply_error(what: &str, r: &Reply) -> Error {
    let text = r.text();
    let lower = text.to_ascii_lowercase();
    if r.code == 421
        || r.code == 450 && lower.contains("rate")
        || lower.contains("too many")
        || lower.contains("rate limit")
        || lower.contains("daily user sending limit")
        || lower.contains("throttl")
    {
        return Error::RateLimited;
    }
    if (400..500).contains(&r.code) {
        return Error::Network(format!(
            "the mail server couldn't {what} right now: {} {text}",
            r.code
        ));
    }
    Error::InvalidInput(format!(
        "The mail server refused to {what}: {} {text}",
        r.code
    ))
}

impl Smtp {
    /// Connect, read the greeting, EHLO, and STARTTLS when asked.
    pub async fn connect(server: &ServerSettings) -> SResult<Smtp> {
        let host = server.host.trim().to_string();
        if server.security == MailSecurity::Plain && !net::is_loopback(&host) {
            return Err(Error::InvalidInput(format!(
                "{host} needs TLS or STARTTLS; unencrypted connections are only allowed to this Mac"
            ))
            .into());
        }
        let mut stream = net::tcp(&host, server.port).await?;
        if server.security == MailSecurity::Tls {
            stream = net::tls(stream, &host).await?;
        }
        let mut s = Smtp {
            io: BufReader::new(stream),
            host: host.clone(),
            ext: Vec::new(),
        };
        let greet = s.reply().await?;
        if greet.code != 220 {
            return Err(reply_error("accept the connection", &greet).into());
        }
        s.ehlo().await?;
        if server.security == MailSecurity::Starttls {
            if !s.has_ext("STARTTLS") {
                return Err(Error::Network(format!(
                    "{host} doesn't offer STARTTLS; choose TLS (usually port 465)"
                ))
                .into());
            }
            let r = s.cmd("STARTTLS").await?;
            if r.code != 220 {
                return Err(reply_error("start TLS", &r).into());
            }
            if !s.io.buffer().is_empty() {
                return Err(Error::Network(
                    "unexpected data after STARTTLS (possible attack); not continuing".into(),
                )
                .into());
            }
            let Smtp { io, host, .. } = s;
            let tls = net::tls(io.into_inner(), &host).await?;
            s = Smtp {
                io: BufReader::new(tls),
                host,
                ext: Vec::new(),
            };
            s.ehlo().await?;
        }
        Ok(s)
    }

    fn has_ext(&self, name: &str) -> bool {
        self.ext.iter().any(|e| {
            e.split_whitespace()
                .next()
                .is_some_and(|k| k.eq_ignore_ascii_case(name))
        })
    }

    fn auth_mechs(&self) -> Vec<String> {
        self.ext
            .iter()
            .filter_map(|e| {
                let mut words = e.split_whitespace();
                let k = words.next()?;
                (k.eq_ignore_ascii_case("AUTH") || k.eq_ignore_ascii_case("AUTH="))
                    .then(|| words.map(|w| w.to_ascii_uppercase()).collect::<Vec<_>>())
            })
            .flatten()
            .collect()
    }

    async fn ehlo(&mut self) -> SResult<()> {
        // A literal address rather than this Mac's name (privacy).
        let r = self.cmd("EHLO [127.0.0.1]").await?;
        if r.code != 250 {
            return Err(reply_error("greet", &r).into());
        }
        self.ext = r.lines.into_iter().skip(1).collect();
        Ok(())
    }

    async fn reply(&mut self) -> SResult<Reply> {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(TIMEOUT, self.io.read_line(&mut line))
                .await
                .map_err(|_| Error::Network(format!("{} stopped answering", self.host)))?
                .map_err(|e| Error::Network(format!("connection to {} lost: {e}", self.host)))?;
            if n == 0 {
                return Err(Error::Network(format!("{} closed the connection", self.host)).into());
            }
            if line.len() > 64 * 1024 {
                return Err(Error::Network("SMTP reply too long".into()).into());
            }
            let line = line.trim_end_matches(['\r', '\n']);
            let code: u16 = line.get(..3).and_then(|c| c.parse().ok()).ok_or_else(|| {
                Error::Network(format!("{} didn't answer like an SMTP server", self.host))
            })?;
            let more = line.as_bytes().get(3) == Some(&b'-');
            lines.push(line.get(4..).unwrap_or("").to_string());
            if !more {
                return Ok(Reply { code, lines });
            }
        }
    }

    async fn send_line(&mut self, line: &str) -> SResult<()> {
        let w = async {
            self.io.get_mut().write_all(line.as_bytes()).await?;
            self.io.get_mut().write_all(b"\r\n").await?;
            self.io.get_mut().flush().await
        };
        tokio::time::timeout(TIMEOUT, w)
            .await
            .map_err(|_| Error::Network(format!("{} stopped reading", self.host)))?
            .map_err(|e| Error::Network(format!("connection to {} lost: {e}", self.host)).into())
    }

    async fn cmd(&mut self, line: &str) -> SResult<Reply> {
        self.send_line(line).await?;
        self.reply().await
    }

    /// AUTH PLAIN (or LOGIN). A refusal is [`SmtpError::Auth`].
    pub async fn login(&mut self, user: &str, password: &str) -> SResult<()> {
        let mechs = self.auth_mechs();
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let r = if mechs.iter().any(|m| m == "PLAIN") || mechs.is_empty() {
            self.cmd(&format!(
                "AUTH PLAIN {}",
                b64(&format!("\0{user}\0{password}"))
            ))
            .await?
        } else if mechs.iter().any(|m| m == "LOGIN") {
            let r = self.cmd("AUTH LOGIN").await?;
            if r.code != 334 {
                return Err(reply_error("sign in", &r).into());
            }
            let r = self.cmd(&b64(user)).await?;
            if r.code != 334 {
                return Err(SmtpError::Auth(r.text()));
            }
            self.cmd(&b64(password)).await?
        } else {
            return Err(Error::InvalidInput(format!(
                "{} offers no sign-in method Penguin supports ({})",
                self.host,
                mechs.join(", ")
            ))
            .into());
        };
        match r.code {
            235 => Ok(()),
            // 535 bad credentials, 534 "web login required" / app password
            // needed (Gmail), 530 authentication required.
            530 | 534 | 535 => Err(SmtpError::Auth(format!("{} {}", r.code, r.text()))),
            _ => Err(reply_error("sign in", &r).into()),
        }
    }

    /// Send one message: `from` and `recipients` are bare addresses; `data`
    /// is the RFC 822 message (Bcc already removed).
    pub async fn send(
        &mut self,
        from: &str,
        recipients: &[String],
        data: &[u8],
    ) -> SResult<String> {
        if recipients.is_empty() {
            return Err(Error::InvalidInput("The message has no recipients.".into()).into());
        }
        let eight_bit = data.iter().any(|b| *b >= 0x80);
        let utf8_addrs = !from.is_ascii() || recipients.iter().any(|r| !r.is_ascii());
        let mut mail = format!("MAIL FROM:<{from}>");
        if eight_bit && self.has_ext("8BITMIME") {
            mail.push_str(" BODY=8BITMIME");
        }
        if utf8_addrs {
            if !self.has_ext("SMTPUTF8") {
                return Err(Error::InvalidInput(
                    "This mail server can't send to or from international (non-ASCII) addresses."
                        .into(),
                )
                .into());
            }
            mail.push_str(" SMTPUTF8");
        }
        if self.has_ext("SIZE") {
            mail.push_str(&format!(" SIZE={}", data.len()));
        }
        let r = self.cmd(&mail).await?;
        if r.code != 250 {
            return Err(reply_error("send from this address", &r).into());
        }
        for rcpt in recipients {
            let r = self.cmd(&format!("RCPT TO:<{rcpt}>")).await?;
            if !(r.code == 250 || r.code == 251) {
                return Err(reply_error("accept a recipient", &r).into());
            }
        }
        let r = self.cmd("DATA").await?;
        if r.code != 354 {
            return Err(reply_error("take the message", &r).into());
        }
        let body = dot_stuff(data);
        let w = async {
            self.io.get_mut().write_all(&body).await?;
            self.io.get_mut().write_all(b".\r\n").await?;
            self.io.get_mut().flush().await
        };
        tokio::time::timeout(TIMEOUT, w)
            .await
            .map_err(|_| Error::Network(format!("{} stopped reading", self.host)))?
            .map_err(|e| Error::Network(format!("connection to {} lost: {e}", self.host)))?;
        let r = self.reply().await?;
        if r.code != 250 {
            return Err(reply_error("send the message", &r).into());
        }
        Ok(r.lines.join(" "))
    }

    pub async fn quit(mut self) {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.cmd("QUIT")).await;
    }
}

/// CRLF line endings, leading dots doubled, ending in CRLF (the caller
/// adds the terminating `.`).
pub fn dot_stuff(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    let mut at_line_start = true;
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        if at_line_start && c == b'.' {
            out.push(b'.');
        }
        match c {
            b'\r' if data.get(i + 1) == Some(&b'\n') => {
                out.extend_from_slice(b"\r\n");
                i += 2;
                at_line_start = true;
                continue;
            }
            b'\n' | b'\r' => {
                out.extend_from_slice(b"\r\n");
                at_line_start = true;
            }
            _ => {
                out.push(c);
                at_line_start = false;
            }
        }
        i += 1;
    }
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// The envelope of an outgoing RFC 822 message and its bytes without Bcc:
/// (from, every To/Cc/Bcc recipient, message without its Bcc header).
pub fn envelope(raw: &[u8]) -> Result<(String, Vec<String>, Vec<u8>)> {
    let parsed = mail_parser::MessageParser::default()
        .parse_headers(raw)
        .ok_or_else(|| Error::Other("outgoing message has no headers".into()))?;
    let addrs = |a: Option<&mail_parser::Address>| -> Vec<String> {
        a.map(|a| {
            a.iter()
                .filter_map(|x| x.address.as_deref())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
    };
    let from = addrs(parsed.from())
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("outgoing message has no From".into()))?;
    let mut rcpts = addrs(parsed.to());
    rcpts.extend(addrs(parsed.cc()));
    rcpts.extend(addrs(parsed.bcc()));
    let mut seen = std::collections::HashSet::new();
    rcpts.retain(|r| seen.insert(r.to_ascii_lowercase()));
    Ok((from, rcpts, strip_bcc(raw)))
}

/// Remove the Bcc header (and its folded continuation lines) from the
/// top-level header block.
pub fn strip_bcc(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    let mut skipping = false;
    while i < raw.len() {
        let end = raw[i..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(raw.len(), |p| i + p + 1);
        let line = &raw[i..end];
        let blank = line == b"\r\n" || line == b"\n";
        if blank {
            out.extend_from_slice(&raw[i..]);
            return out;
        }
        let folded = matches!(line.first(), Some(b' ') | Some(b'\t'));
        if !folded {
            skipping = line.len() >= 4 && line[..4].eq_ignore_ascii_case(b"bcc:");
        }
        if !skipping {
            out.extend_from_slice(line);
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_stuffing() {
        assert_eq!(
            dot_stuff(b"a\r\n.b\r\n..c"),
            b"a\r\n..b\r\n...c\r\n".to_vec()
        );
        assert_eq!(dot_stuff(b".\nx\n"), b"..\r\nx\r\n".to_vec());
        assert_eq!(dot_stuff(b""), b"\r\n".to_vec());
    }

    #[test]
    fn envelope_strips_bcc_and_collects_recipients() {
        let raw = b"From: Ada <ada@mail.example>\r\nTo: bea@mail.example, Cy <cy@mail.example>\r\nBcc: dee@mail.example,\r\n  eve@mail.example\r\nCc: BEA@mail.example\r\nSubject: hi\r\n\r\nBcc: not a header\r\n";
        let (from, rcpts, data) = envelope(raw).unwrap();
        assert_eq!(from, "ada@mail.example");
        assert_eq!(
            rcpts,
            vec![
                "bea@mail.example",
                "cy@mail.example",
                "dee@mail.example",
                "eve@mail.example"
            ]
        );
        let text = String::from_utf8(data).unwrap();
        assert!(!text.contains("dee@"), "{text}");
        assert!(!text.contains("eve@"), "{text}");
        assert!(text.contains("Subject: hi"));
        assert!(text.ends_with("\r\n\r\nBcc: not a header\r\n"));
    }

    #[test]
    fn reply_errors() {
        let r = |code, t: &str| Reply {
            code,
            lines: vec![t.into()],
        };
        assert!(matches!(
            reply_error("x", &r(421, "try later")),
            Error::RateLimited
        ));
        assert!(matches!(
            reply_error("x", &r(550, "Daily user sending limit exceeded")),
            Error::RateLimited
        ));
        assert!(matches!(
            reply_error("x", &r(451, "local error")),
            Error::Network(_)
        ));
        let e = reply_error("accept a recipient", &r(550, "no such user sam@x.example"));
        assert!(matches!(e, Error::InvalidInput(_)));
        assert!(!e.to_string().contains("sam@x.example"), "{e}");
    }
}
