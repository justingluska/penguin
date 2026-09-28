//! One authenticated IMAP connection: commands, FETCH decoding, selection
//! state. Everything above this (folders, sync, the provider) speaks in
//! these calls; nothing above it sees protocol text.
//!
//! Never logs message content, subjects, addresses or passwords: commands
//! are logged by name only.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use base64::Engine;
use penguin_core::{MailSecurity, ServerSettings};
use tokio::io::{AsyncWriteExt, BufReader};

use crate::compress::Deflate;
use crate::net::{self, BoxIo};
use crate::proto::{self, seqset, utf7, Arg, Response, Status, StatusKind, Untagged, Value};
use crate::{redact, Error, Result};

/// A single server response may take this long to arrive (big literals on
/// slow links included).
pub const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// The server's capabilities, uppercased.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caps(HashSet<String>);

impl Caps {
    pub fn from_list<I: IntoIterator<Item = S>, S: AsRef<str>>(list: I) -> Caps {
        Caps(
            list.into_iter()
                .map(|s| s.as_ref().to_ascii_uppercase())
                .collect(),
        )
    }
    pub fn has(&self, name: &str) -> bool {
        self.0.contains(&name.to_ascii_uppercase())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Gmail's IMAP extensions (X-GM-LABELS, X-GM-MSGID, X-GM-THRID, X-GM-RAW).
    pub fn gmail(&self) -> bool {
        self.has("X-GM-EXT-1")
    }
    pub fn condstore(&self) -> bool {
        self.has("CONDSTORE") || self.has("QRESYNC")
    }
}

/// The selected mailbox as the server described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    /// Wire name (modified UTF-7).
    pub name: String,
    pub readonly: bool,
    pub exists: u32,
    pub uidvalidity: u32,
    pub uidnext: u32,
    /// None when the server has no CONDSTORE or the mailbox has NOMODSEQ.
    pub highestmodseq: Option<u64>,
}

/// One message's FETCH data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FetchItem {
    pub seq: u32,
    pub uid: Option<u32>,
    pub flags: Option<Vec<String>>,
    /// INTERNALDATE as unix ms.
    pub internal_date: Option<i64>,
    pub size: Option<u64>,
    pub modseq: Option<u64>,
    /// RFC 8474 EMAILID.
    pub emailid: Option<String>,
    pub gm_msgid: Option<u64>,
    pub gm_thrid: Option<u64>,
    /// X-GM-LABELS as sent (modified UTF-7 names or `\Inbox`-style atoms).
    pub gm_labels: Option<Vec<Vec<u8>>>,
    /// BODY[...] sections: normalized key (`BODY[HEADER.FIELDS]`, `BODY[]`,
    /// `BODY[1.2]`) → bytes (None for NIL).
    pub sections: Vec<(String, Option<Vec<u8>>)>,
    pub bodystructure: Option<Value>,
}

impl FetchItem {
    /// A body section by its key without the `<origin>` suffix; header
    /// field fetches are keyed `BODY[HEADER.FIELDS]` whatever their list.
    pub fn section(&self, key: &str) -> Option<&[u8]> {
        self.sections
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .and_then(|(_, v)| v.as_deref())
    }

    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags
            .as_ref()
            .is_some_and(|f| f.iter().any(|x| x.eq_ignore_ascii_case(flag)))
    }
}

/// Normalize a FETCH section name: `BODY[HEADER.FIELDS (DATE)]<0>` →
/// `BODY[HEADER.FIELDS]`, `BINARY[1]` stays, `RFC822` → `BODY[]`.
fn section_key(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    if upper == "RFC822" {
        return "BODY[]".into();
    }
    if upper == "RFC822.HEADER" {
        return "BODY[HEADER]".into();
    }
    let Some(open) = upper.find('[') else {
        return upper;
    };
    let close = upper.rfind(']').unwrap_or(upper.len() - 1);
    let inner = &upper[open + 1..close];
    let inner = match inner.find(" (").or_else(|| inner.find('(')) {
        Some(i) => inner[..i].trim(),
        None => inner.trim(),
    };
    format!("{}[{}]", &upper[..open], inner)
}

/// Parse `INTERNALDATE` (`" 7-Feb-2026 09:10:11 +0100"`) to unix ms.
pub fn parse_internal_date(s: &str) -> Option<i64> {
    let s = s.trim();
    chrono::DateTime::parse_from_str(s, "%d-%b-%Y %H:%M:%S %z")
        .ok()
        .map(|d| d.timestamp_millis())
}

/// `1-Feb-2026` for SEARCH SINCE/BEFORE (UTC date of `ms`).
pub fn search_date(ms: i64) -> String {
    let d = chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default();
    d.format("%-d-%b-%Y").to_string()
}

fn decode_fetch(seq: u32, items: Vec<Value>) -> FetchItem {
    let mut out = FetchItem {
        seq,
        ..FetchItem::default()
    };
    let mut it = items.into_iter();
    while let (Some(name), Some(value)) = (it.next(), it.next()) {
        let Some(name) = name.as_atom().map(str::to_string) else {
            continue;
        };
        let upper = name.to_ascii_uppercase();
        match upper.as_str() {
            "UID" => out.uid = value.as_u64().map(|n| n as u32),
            "FLAGS" => {
                out.flags = value
                    .as_list()
                    .map(|l| l.iter().filter_map(Value::as_text).collect())
            }
            "INTERNALDATE" => {
                out.internal_date = value.as_text().and_then(|s| parse_internal_date(&s))
            }
            "RFC822.SIZE" => out.size = value.as_u64(),
            "MODSEQ" => {
                out.modseq = value
                    .as_list()
                    .and_then(|l| l.first())
                    .and_then(Value::as_u64)
            }
            "EMAILID" => {
                out.emailid = value
                    .as_list()
                    .and_then(|l| l.first())
                    .or(Some(&value))
                    .and_then(Value::as_text)
                    .filter(|s| !s.is_empty())
            }
            "X-GM-MSGID" => out.gm_msgid = value.as_u64(),
            "X-GM-THRID" => out.gm_thrid = value.as_u64(),
            "X-GM-LABELS" => {
                out.gm_labels = value.as_list().map(|l| {
                    l.iter()
                        .filter_map(|v| v.as_bytes().map(<[u8]>::to_vec))
                        .collect()
                })
            }
            "BODYSTRUCTURE" | "BODY" => out.bodystructure = Some(value),
            _ if upper.starts_with("BODY[")
                || upper.starts_with("BINARY[")
                || upper == "RFC822"
                || upper == "RFC822.HEADER" =>
            {
                let bytes = match value {
                    Value::Nil => None,
                    v => v.as_bytes().map(<[u8]>::to_vec),
                };
                out.sections.push((section_key(&name), bytes));
            }
            _ => {}
        }
    }
    out
}

/// Merge FETCH responses for the same UID (servers may send flags and
/// bodies separately, or add unsolicited FLAGS).
fn merge_fetches(items: Vec<FetchItem>) -> Vec<FetchItem> {
    let mut out: Vec<FetchItem> = Vec::with_capacity(items.len());
    let mut by_uid: HashMap<u32, usize> = HashMap::new();
    for f in items {
        let Some(uid) = f.uid else {
            // Without a UID we can't tell which message it is (unsolicited
            // flag updates by sequence number): drop it.
            continue;
        };
        match by_uid.get(&uid) {
            Some(&i) => {
                let e = &mut out[i];
                if f.flags.is_some() {
                    e.flags = f.flags;
                }
                e.internal_date = e.internal_date.or(f.internal_date);
                e.size = e.size.or(f.size);
                e.modseq = e.modseq.max(f.modseq);
                e.emailid = e.emailid.take().or(f.emailid);
                e.gm_msgid = e.gm_msgid.or(f.gm_msgid);
                e.gm_thrid = e.gm_thrid.or(f.gm_thrid);
                if f.gm_labels.is_some() {
                    e.gm_labels = f.gm_labels;
                }
                e.sections.extend(f.sections);
                if f.bodystructure.is_some() {
                    e.bodystructure = f.bodystructure;
                }
            }
            None => {
                by_uid.insert(uid, out.len());
                out.push(f);
            }
        }
    }
    out
}

/// COPYUID / APPENDUID data: where copies landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyUid {
    pub uidvalidity: u32,
    /// (source uid, destination uid) pairs.
    pub pairs: Vec<(u32, u32)>,
}

fn copyuid(status: &Status) -> Option<CopyUid> {
    let (code, args) = status.code.as_ref()?;
    if code != "COPYUID" {
        return None;
    }
    let uidvalidity = args.first()?.as_u64()? as u32;
    let src = seqset::parse(&args.get(1)?.as_text()?, 0, 100_000)?;
    let dst = seqset::parse(&args.get(2)?.as_text()?, 0, 100_000)?;
    (src.len() == dst.len()).then(|| CopyUid {
        uidvalidity,
        pairs: src.into_iter().zip(dst).collect(),
    })
}

/// A LIST entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    /// Wire name (modified UTF-7), as the server spelled it.
    pub raw: String,
    pub attrs: Vec<String>,
    pub delimiter: Option<String>,
}

impl ListEntry {
    pub fn has_attr(&self, attr: &str) -> bool {
        self.attrs.iter().any(|a| a.eq_ignore_ascii_case(attr))
    }
}

/// What a command returned: its untagged responses and the tagged status.
#[derive(Debug, Default)]
pub struct Done {
    pub untagged: Vec<Untagged>,
    pub status: Option<Status>,
}

/// Why IDLE returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    /// The mailbox changed (new mail, expunge, flags).
    Changed,
    /// The wait elapsed with nothing new (re-IDLE).
    Timeout,
}

pub struct Session {
    io: Option<BufReader<BoxIo>>,
    next_tag: u32,
    pub caps: Caps,
    pub selected: Option<Selected>,
    pub qresync: bool,
    pub condstore_enabled: bool,
    /// COMPRESS=DEFLATE is on (RFC 4978).
    pub compressed: bool,
    /// The server is on this machine or the local network.
    local_network: bool,
    /// Where we're connected (for messages; never a secret).
    pub host: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("host", &self.host)
            .field("selected", &self.selected.as_ref().map(|s| s.name.len()))
            .finish_non_exhaustive()
    }
}

/// Map a failed status to the provider error for `cmd`.
pub fn status_error(cmd: &str, status: &Status) -> Error {
    let code = status
        .code
        .as_ref()
        .map(|(c, _)| c.as_str())
        .unwrap_or_default();
    let text = redact(&status.text);
    let lower = text.to_ascii_lowercase();
    if matches!(code, "THROTTLED" | "LIMIT" | "OVERQUOTA")
        || lower.contains("bandwidth")
        || lower.contains("throttl")
        || lower.contains("too many")
        || lower.contains("rate limit")
    {
        return Error::RateLimited;
    }
    if status.kind == StatusKind::Bye || matches!(code, "UNAVAILABLE" | "SERVERBUG" | "INUSE") {
        return Error::Network(format!("{cmd}: {text}"));
    }
    if matches!(
        code,
        "AUTHENTICATIONFAILED" | "AUTHORIZATIONFAILED" | "EXPIRED"
    ) {
        return Error::NeedsReauth(text);
    }
    if matches!(code, "NONEXISTENT" | "TRYCREATE") {
        return Error::NotFound(format!("{cmd}: {text}"));
    }
    Error::Other(format!("IMAP {cmd} failed: {text}"))
}

impl Session {
    /// Connect and read the greeting (TLS or STARTTLS per `server`).
    pub async fn connect(server: &ServerSettings) -> Result<Session> {
        let host = server.host.trim().to_string();
        if server.security == MailSecurity::Plain && !net::is_loopback(&host) {
            return Err(Error::InvalidInput(format!(
                "{host} needs TLS or STARTTLS; unencrypted connections are only allowed to this Mac"
            )));
        }
        let (mut stream, local_network) = net::tcp_to(&host, server.port).await?;
        if server.security == MailSecurity::Tls {
            stream = net::tls(stream, &host).await?;
        }
        let mut s = Session {
            io: Some(BufReader::new(stream)),
            next_tag: 1,
            caps: Caps::default(),
            selected: None,
            qresync: false,
            condstore_enabled: false,
            compressed: false,
            local_network,
            host: host.clone(),
        };
        let greeting = s.read().await?;
        match greeting {
            Response::Untagged(Untagged::Status(st))
                if matches!(st.kind, StatusKind::Ok | StatusKind::Preauth) =>
            {
                if let Some(("CAPABILITY", args)) = st.code.as_ref().map(|(c, a)| (c.as_str(), a)) {
                    s.caps = Caps::from_list(args.iter().filter_map(Value::as_text));
                }
            }
            Response::Untagged(Untagged::Status(st)) => {
                return Err(status_error("connect", &st));
            }
            _ => {
                return Err(Error::Network(format!(
                    "{host} didn't answer like an IMAP server"
                )))
            }
        }
        if server.security == MailSecurity::Starttls {
            if s.caps.is_empty() {
                s.capability().await?;
            }
            if !s.caps.has("STARTTLS") {
                return Err(Error::Network(format!(
                    "{host} doesn't offer STARTTLS; choose TLS (usually port 993)"
                )));
            }
            s.run("STARTTLS", vec![]).await?;
            let reader = s.io.take().expect("connected");
            if !reader.buffer().is_empty() {
                return Err(Error::Network(
                    "unexpected data after STARTTLS (possible attack); not continuing".into(),
                ));
            }
            let tls = net::tls(reader.into_inner(), &host).await?;
            s.io = Some(BufReader::new(tls));
            s.caps = Caps::default();
        }
        if s.caps.is_empty() {
            s.capability().await?;
        }
        Ok(s)
    }

    fn io(&mut self) -> Result<&mut BufReader<BoxIo>> {
        self.io
            .as_mut()
            .ok_or_else(|| Error::Network("connection closed".into()))
    }

    async fn read(&mut self) -> Result<Response> {
        let io = self.io()?;
        let bytes = match tokio::time::timeout(READ_TIMEOUT, proto::read_response(io)).await {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                self.io = None;
                return Err(Error::Network(format!("connection lost: {e}")));
            }
            Err(_) => {
                self.io = None;
                return Err(Error::Network("the mail server stopped answering".into()));
            }
        };
        proto::parse_response(&bytes).map_err(|e| {
            self.io = None;
            Error::Network(e.to_string())
        })
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let io = self.io()?;
        let r = async {
            io.get_mut().write_all(bytes).await?;
            io.get_mut().flush().await
        };
        match tokio::time::timeout(READ_TIMEOUT, r).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                self.io = None;
                Err(Error::Network(format!("connection lost: {e}")))
            }
            Err(_) => {
                self.io = None;
                Err(Error::Network("the mail server stopped reading".into()))
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.io.is_some()
    }

    /// Run `name args…` and collect its responses. A tagged NO/BAD is an
    /// error (see [`status_error`]); so is an untagged BYE.
    pub async fn run(&mut self, name: &str, args: Vec<Arg>) -> Result<Done> {
        let tag = format!("p{:04}", self.next_tag);
        self.next_tag += 1;
        let literal_plus = self.caps.has("LITERAL+") || self.caps.has("LITERAL-");
        let mut line: Vec<u8> = Vec::with_capacity(64);
        line.extend_from_slice(tag.as_bytes());
        line.push(b' ');
        line.extend_from_slice(name.as_bytes());
        let mut after_open = false;
        for arg in args {
            // `(` and `)` group search keys: no space inside the parens.
            let close = matches!(&arg, Arg::Raw(s) if s == ")");
            if !after_open && !close {
                line.push(b' ');
            }
            after_open = matches!(&arg, Arg::Raw(s) if s == "(");
            match arg {
                Arg::Raw(s) => line.extend_from_slice(s.as_bytes()),
                Arg::Str(s) if proto::quotable(&s) => line.extend_from_slice(&proto::quote(&s)),
                Arg::Str(s) => {
                    // LITERAL- only allows non-synchronizing literals up to 4096.
                    let non_sync = self.caps.has("LITERAL+") || (literal_plus && s.len() <= 4096);
                    if non_sync {
                        line.extend_from_slice(format!("{{{}+}}\r\n", s.len()).as_bytes());
                        line.extend_from_slice(&s);
                    } else {
                        line.extend_from_slice(format!("{{{}}}\r\n", s.len()).as_bytes());
                        self.write(&line).await?;
                        line.clear();
                        loop {
                            match self.read().await? {
                                Response::Continue(_) => break,
                                Response::Tagged { status, .. } => {
                                    return Err(status_error(name, &status))
                                }
                                Response::Untagged(Untagged::Status(st))
                                    if st.kind == StatusKind::Bye =>
                                {
                                    self.io = None;
                                    return Err(status_error(name, &st));
                                }
                                Response::Untagged(_) => {}
                            }
                        }
                        line.extend_from_slice(&s);
                    }
                }
            }
        }
        line.extend_from_slice(b"\r\n");
        self.write(&line).await?;
        self.collect(&tag, name).await
    }

    async fn collect(&mut self, tag: &str, name: &str) -> Result<Done> {
        let mut done = Done::default();
        loop {
            match self.read().await? {
                Response::Tagged { tag: t, status } if t == tag => {
                    if let Some(("CAPABILITY", args)) =
                        status.code.as_ref().map(|(c, a)| (c.as_str(), a))
                    {
                        self.caps = Caps::from_list(args.iter().filter_map(Value::as_text));
                    }
                    return match status.kind {
                        StatusKind::Ok => {
                            done.status = Some(status);
                            Ok(done)
                        }
                        _ => Err(status_error(name, &status)),
                    };
                }
                Response::Tagged { .. } => {}
                Response::Continue(_) => {
                    return Err(Error::Network(format!(
                        "unexpected continuation during {name}"
                    )))
                }
                Response::Untagged(Untagged::Status(st)) if st.kind == StatusKind::Bye => {
                    self.io = None;
                    return Err(status_error(name, &st));
                }
                Response::Untagged(Untagged::Capability(c)) => {
                    self.caps = Caps::from_list(&c);
                    done.untagged.push(Untagged::Capability(c));
                }
                Response::Untagged(u) => done.untagged.push(u),
            }
        }
    }

    pub async fn capability(&mut self) -> Result<()> {
        self.run("CAPABILITY", vec![]).await?;
        Ok(())
    }

    /// Log in: AUTHENTICATE PLAIN when offered (UTF-8 safe), else LOGIN.
    /// A refusal is `NeedsReauth` with the server's text.
    pub async fn login(&mut self, user: &str, password: &str) -> Result<()> {
        if self.caps.has("LOGINDISABLED") {
            return Err(Error::Network(
                "the server doesn't allow signing in on this connection (needs TLS)".into(),
            ));
        }
        let result = if self.caps.has("AUTH=PLAIN") {
            let token =
                base64::engine::general_purpose::STANDARD.encode(format!("\0{user}\0{password}"));
            if self.caps.has("SASL-IR") {
                self.run("AUTHENTICATE", vec![Arg::raw("PLAIN"), Arg::raw(token)])
                    .await
            } else {
                self.authenticate_plain_continuation(&token).await
            }
        } else {
            self.run(
                "LOGIN",
                vec![
                    Arg::string(user.as_bytes()),
                    Arg::string(password.as_bytes()),
                ],
            )
            .await
        };
        match result {
            Ok(done) => {
                let refreshed = done
                    .status
                    .as_ref()
                    .is_some_and(|s| s.code_is("CAPABILITY"));
                if !refreshed {
                    self.capability().await?;
                }
                Ok(())
            }
            Err(Error::Other(text)) | Err(Error::NotFound(text)) => Err(Error::NeedsReauth(
                text.trim_start_matches("IMAP ").to_string(),
            )),
            Err(e) => Err(e),
        }
    }

    async fn authenticate_plain_continuation(&mut self, token: &str) -> Result<Done> {
        let tag = format!("p{:04}", self.next_tag);
        self.next_tag += 1;
        self.write(format!("{tag} AUTHENTICATE PLAIN\r\n").as_bytes())
            .await?;
        loop {
            match self.read().await? {
                Response::Continue(_) => break,
                Response::Tagged { status, .. } => {
                    return Err(status_error("AUTHENTICATE", &status))
                }
                Response::Untagged(_) => {}
            }
        }
        self.write(format!("{token}\r\n").as_bytes()).await?;
        self.collect(&tag, "AUTHENTICATE").await
    }

    /// Turn on COMPRESS=DEFLATE (RFC 4978) when the server offers it, after
    /// login. Mail text, HTML and headers shrink several-fold on the wire
    /// (base64 attachments by about a quarter), for ~4 us of inflating per
    /// KB of mail. Not for servers on this Mac (Proton Mail Bridge) or the
    /// local network: bandwidth there is plentiful and the CPU isn't. A
    /// refusal leaves the connection as it was. Returns whether it's on.
    pub async fn compress(&mut self) -> Result<bool> {
        // The scripted test server is on loopback too, and tests must run
        // the compressed path.
        let local = (self.local_network || net::is_loopback(&self.host)) && !cfg!(test);
        if self.compressed || local || !self.caps.has("COMPRESS=DEFLATE") {
            return Ok(self.compressed);
        }
        match self.run("COMPRESS", vec![Arg::raw("DEFLATE")]).await {
            Ok(_) => {}
            // Lost the connection: that's an error like any other.
            Err(e) if !self.is_open() => return Err(e),
            Err(e) => {
                tracing::debug!(host = %self.host, error = %e, "COMPRESS refused; staying uncompressed");
                return Ok(false);
            }
        }
        // The server deflates everything after its OK; anything already
        // buffered past that line is compressed and goes to the inflater.
        let reader = self.io.take().expect("connected");
        let leftover = reader.buffer().to_vec();
        let stream: BoxIo = Box::new(Deflate::new(reader.into_inner(), leftover));
        self.io = Some(BufReader::new(stream));
        self.compressed = true;
        Ok(true)
    }

    /// ENABLE QRESYNC (implies CONDSTORE) when offered, else note CONDSTORE.
    pub async fn enable_extensions(&mut self) -> Result<()> {
        if self.caps.has("QRESYNC") && self.caps.has("ENABLE") {
            let done = self.run("ENABLE", vec![Arg::raw("QRESYNC")]).await?;
            self.qresync = done.untagged.iter().any(|u| {
                matches!(u, Untagged::Enabled(e) if e.iter().any(|x| x.eq_ignore_ascii_case("QRESYNC")))
            });
            self.condstore_enabled = self.qresync;
        }
        if !self.condstore_enabled && self.caps.has("CONDSTORE") && self.caps.has("ENABLE") {
            let done = self.run("ENABLE", vec![Arg::raw("CONDSTORE")]).await?;
            self.condstore_enabled = done.untagged.iter().any(|u| {
                matches!(u, Untagged::Enabled(e) if e.iter().any(|x| x.eq_ignore_ascii_case("CONDSTORE")))
            });
        }
        Ok(())
    }

    /// `LIST "" "*"`.
    pub async fn list(&mut self) -> Result<Vec<ListEntry>> {
        let done = self
            .run("LIST", vec![Arg::string(""), Arg::string("*")])
            .await?;
        Ok(done
            .untagged
            .into_iter()
            .filter_map(|u| match u {
                Untagged::List {
                    attrs,
                    delimiter,
                    name,
                } => Some(ListEntry {
                    raw: String::from_utf8_lossy(&name).into_owned(),
                    attrs,
                    delimiter,
                }),
                _ => None,
            })
            .collect())
    }

    /// SELECT (or EXAMINE when `readonly`) `raw` (a wire name), always
    /// re-issued so EXISTS/UIDNEXT/HIGHESTMODSEQ are fresh.
    pub async fn select(&mut self, raw: &str, readonly: bool) -> Result<Selected> {
        let cmd = if readonly { "EXAMINE" } else { "SELECT" };
        let mut args = vec![Arg::string(raw.as_bytes())];
        if self.caps.condstore() && !self.condstore_enabled {
            args.push(Arg::raw("(CONDSTORE)"));
        }
        self.selected = None;
        let done = self.run(cmd, args).await?;
        let mut sel = Selected {
            name: raw.to_string(),
            readonly,
            exists: 0,
            uidvalidity: 0,
            uidnext: 0,
            highestmodseq: None,
        };
        let mut nomodseq = false;
        for u in &done.untagged {
            match u {
                Untagged::Exists(n) => sel.exists = *n,
                Untagged::Status(st) => {
                    if let Some(v) = st.code_num("UIDVALIDITY") {
                        sel.uidvalidity = v as u32;
                    }
                    if let Some(v) = st.code_num("UIDNEXT") {
                        sel.uidnext = v as u32;
                    }
                    if let Some(v) = st.code_num("HIGHESTMODSEQ") {
                        sel.highestmodseq = Some(v);
                    }
                    if st.code_is("NOMODSEQ") {
                        nomodseq = true;
                    }
                }
                _ => {}
            }
        }
        if let Some(st) = &done.status {
            if st.code_is("READ-ONLY") {
                sel.readonly = true;
            }
        }
        if nomodseq {
            sel.highestmodseq = None;
        }
        if sel.uidnext == 0 {
            // Not announced (old servers): one past the highest UID.
            self.selected = Some(sel.clone());
            let all = if sel.exists == 0 {
                Vec::new()
            } else {
                self.uid_search(vec![Arg::raw("ALL")]).await?
            };
            sel.uidnext = all.iter().max().map_or(1, |m| m + 1);
        }
        self.selected = Some(sel.clone());
        Ok(sel)
    }

    /// Select `raw` unless it's already selected with enough rights.
    pub async fn ensure_selected(&mut self, raw: &str, write: bool) -> Result<Selected> {
        if let Some(s) = &self.selected {
            if s.name == raw && (!write || !s.readonly) {
                return Ok(s.clone());
            }
        }
        self.select(raw, !write).await
    }

    /// `UID SEARCH <criteria>` (CHARSET UTF-8 when a string needs it).
    pub async fn uid_search(&mut self, criteria: Vec<Arg>) -> Result<Vec<u32>> {
        let needs_charset = criteria
            .iter()
            .any(|a| matches!(a, Arg::Str(s) if !s.is_ascii()));
        let mut args = vec![Arg::raw("SEARCH")];
        if needs_charset {
            args.push(Arg::raw("CHARSET UTF-8"));
        }
        args.extend(criteria);
        let done = self.run("UID", args).await?;
        let mut out = Vec::new();
        for u in done.untagged {
            match u {
                Untagged::Search(ids) => out.extend(ids),
                // ESEARCH answer if the server volunteers one.
                Untagged::Other(k, vals) if k == "ESEARCH" => {
                    let mut it = vals.iter();
                    while let Some(v) = it.next() {
                        if v.is_atom("ALL") {
                            if let Some(set) = it.next().and_then(Value::as_text) {
                                out.extend(seqset::parse(&set, 0, 10_000_000).unwrap_or_default());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    /// `UID FETCH <set> <items> [(CHANGEDSINCE m [VANISHED])]`. Returns the
    /// merged items (only those with a UID) and any VANISHED UIDs.
    pub async fn uid_fetch(
        &mut self,
        set: &str,
        items: &str,
        changed_since: Option<u64>,
    ) -> Result<(Vec<FetchItem>, Vec<u32>)> {
        if set.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut args = vec![Arg::raw("FETCH"), Arg::raw(set), Arg::raw(items)];
        if let Some(m) = changed_since {
            if self.qresync {
                args.push(Arg::raw(format!("(CHANGEDSINCE {m} VANISHED)")));
            } else {
                args.push(Arg::raw(format!("(CHANGEDSINCE {m})")));
            }
        }
        let done = self.run("UID", args).await?;
        let mut fetched = Vec::new();
        let mut vanished = Vec::new();
        let star = self
            .selected
            .as_ref()
            .map_or(0, |s| s.uidnext.saturating_sub(1));
        for u in done.untagged {
            match u {
                Untagged::Fetch { seq, items } => fetched.push(decode_fetch(seq, items)),
                Untagged::Vanished { uids, .. } => {
                    vanished.extend(seqset::parse(&uids, star, 10_000_000).unwrap_or_default())
                }
                _ => {}
            }
        }
        Ok((merge_fetches(fetched), vanished))
    }

    /// Several independent commands in one write, then read until every
    /// one has completed: one round trip instead of one each (pipelining,
    /// RFC 3501 §5.5). Returns the untagged responses and each command's
    /// completion, in the order sent. `name` is for errors.
    async fn pipeline(
        &mut self,
        name: &str,
        commands: &[String],
    ) -> Result<(Vec<Untagged>, Vec<Status>)> {
        let mut line = Vec::new();
        let mut tags = Vec::with_capacity(commands.len());
        for c in commands {
            let tag = format!("p{:04}", self.next_tag);
            self.next_tag += 1;
            line.extend_from_slice(format!("{tag} {c}\r\n").as_bytes());
            tags.push(tag);
        }
        self.write(&line).await?;
        let mut done: Vec<Option<Status>> = vec![None; tags.len()];
        let mut pending = tags.len();
        let mut untagged = Vec::new();
        while pending > 0 {
            match self.read().await? {
                Response::Tagged { tag, status } => {
                    if let Some(i) = tags.iter().position(|t| *t == tag) {
                        if done[i].is_none() {
                            pending -= 1;
                        }
                        done[i] = Some(status);
                    }
                }
                Response::Untagged(Untagged::Status(st)) if st.kind == StatusKind::Bye => {
                    self.io = None;
                    return Err(status_error(name, &st));
                }
                Response::Untagged(Untagged::Capability(c)) => self.caps = Caps::from_list(&c),
                Response::Untagged(u) => untagged.push(u),
                Response::Continue(_) => {
                    self.io = None;
                    return Err(Error::Network(format!(
                        "unexpected continuation during {name}"
                    )));
                }
            }
        }
        Ok((untagged, done.into_iter().flatten().collect()))
    }

    /// Several `UID FETCH <set> <items>` in one round trip ([`Self::pipeline`];
    /// FETCHes don't depend on each other). Returns the merged items of all
    /// of them (only those with a UID). Any command's NO/BAD is an error,
    /// reported after the others finish so the connection stays in step.
    pub async fn uid_fetch_pipelined(
        &mut self,
        requests: &[(String, String)],
    ) -> Result<Vec<FetchItem>> {
        let requests: Vec<&(String, String)> =
            requests.iter().filter(|(set, _)| !set.is_empty()).collect();
        if requests.len() <= 1 {
            return match requests.first() {
                Some((set, items)) => Ok(self.uid_fetch(set, items, None).await?.0),
                None => Ok(Vec::new()),
            };
        }
        let commands: Vec<String> = requests
            .iter()
            .map(|(set, items)| format!("UID FETCH {set} {items}"))
            .collect();
        let (untagged, done) = self.pipeline("FETCH", &commands).await?;
        if let Some(st) = done.iter().find(|st| st.kind != StatusKind::Ok) {
            return Err(status_error("FETCH", st));
        }
        let fetched = untagged
            .into_iter()
            .filter_map(|u| match u {
                Untagged::Fetch { seq, items } => Some(decode_fetch(seq, items)),
                _ => None,
            })
            .collect();
        Ok(merge_fetches(fetched))
    }

    /// `STATUS <mailbox> (<items>)` for several mailboxes in one round trip
    /// ([`Self::pipeline`]): mailbox wire name → item (uppercase) → number.
    /// A mailbox the server refuses (gone, no access) is just missing from
    /// the answer. Names that would need a literal aren't pipelined: the
    /// caller gets nothing for them either. Never for the selected mailbox
    /// (RFC 3501 §6.3.10).
    pub async fn status_many(
        &mut self,
        mailboxes: &[&str],
        items: &str,
    ) -> Result<HashMap<String, HashMap<String, u64>>> {
        let names: Vec<&str> = mailboxes
            .iter()
            .copied()
            .filter(|m| proto::quotable(m.as_bytes()))
            .filter(|m| self.selected.as_ref().is_none_or(|s| s.name != *m))
            .collect();
        let mut out = HashMap::new();
        if names.is_empty() {
            return Ok(out);
        }
        let commands: Vec<String> = names
            .iter()
            .map(|m| {
                format!(
                    "STATUS {} ({items})",
                    String::from_utf8_lossy(&proto::quote(m.as_bytes()))
                )
            })
            .collect();
        let (untagged, _) = self.pipeline("STATUS", &commands).await?;
        for u in untagged {
            if let Untagged::MailboxStatus { name, items } = u {
                let Ok(name) = String::from_utf8(name) else {
                    continue;
                };
                let entry: &mut HashMap<String, u64> = out.entry(name).or_default();
                let mut it = items.into_iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    if let (Some(k), Some(v)) = (k.as_atom(), v.as_u64()) {
                        entry.insert(k.to_ascii_uppercase(), v);
                    }
                }
            }
        }
        Ok(out)
    }

    /// `UID STORE <set> <op> <list>`, e.g. `+FLAGS.SILENT (\Seen)`.
    pub async fn uid_store(&mut self, set: &str, op: &str, list: Vec<Arg>) -> Result<()> {
        if set.is_empty() {
            return Ok(());
        }
        let mut args = vec![Arg::raw("STORE"), Arg::raw(set), Arg::raw(op)];
        // The list is spliced as `(a b c)`, each element encoded.
        let mut inner: Vec<u8> = vec![b'('];
        let mut literal_args = Vec::new();
        for (i, a) in list.into_iter().enumerate() {
            if i > 0 {
                inner.push(b' ');
            }
            match a {
                Arg::Raw(s) => inner.extend_from_slice(s.as_bytes()),
                Arg::Str(s) if proto::quotable(&s) => inner.extend_from_slice(&proto::quote(&s)),
                Arg::Str(s) => literal_args.push(s),
            }
        }
        if literal_args.is_empty() {
            inner.push(b')');
            args.push(Arg::Raw(String::from_utf8_lossy(&inner).into_owned()));
        } else {
            // Rare (non-ASCII Gmail label names arrive mUTF-7, so ASCII):
            // store one literal label at a time.
            inner.push(b')');
            if inner.len() > 2 {
                args.push(Arg::Raw(String::from_utf8_lossy(&inner).into_owned()));
                self.run("UID", args).await?;
            }
            for s in literal_args {
                let args = vec![Arg::raw("STORE"), Arg::raw(set), Arg::raw(op), Arg::Str(s)];
                self.run("UID", args).await?;
            }
            return Ok(());
        }
        self.run("UID", args).await?;
        Ok(())
    }

    /// Copy UIDs to `dest` (a wire name); COPYUID when the server has UIDPLUS.
    pub async fn uid_copy(&mut self, set: &str, dest: &str) -> Result<Option<CopyUid>> {
        let done = self
            .run(
                "UID",
                vec![
                    Arg::raw("COPY"),
                    Arg::raw(set),
                    Arg::string(dest.as_bytes()),
                ],
            )
            .await?;
        Ok(done.status.as_ref().and_then(copyuid))
    }

    /// Move UIDs to `dest`: MOVE (RFC 6851) when offered, else COPY +
    /// \Deleted + UID EXPUNGE (or EXPUNGE without UIDPLUS).
    pub async fn uid_move(&mut self, set: &str, dest: &str) -> Result<Option<CopyUid>> {
        if set.is_empty() {
            return Ok(None);
        }
        if self.caps.has("MOVE") {
            let done = self
                .run(
                    "UID",
                    vec![
                        Arg::raw("MOVE"),
                        Arg::raw(set),
                        Arg::string(dest.as_bytes()),
                    ],
                )
                .await?;
            // COPYUID arrives in an untagged OK before the expunges, or on
            // the tagged OK.
            let from_untagged = done.untagged.iter().find_map(|u| match u {
                Untagged::Status(st) => copyuid(st),
                _ => None,
            });
            return Ok(from_untagged.or_else(|| done.status.as_ref().and_then(copyuid)));
        }
        let copied = self.uid_copy(set, dest).await?;
        self.uid_store(set, "+FLAGS.SILENT", vec![Arg::raw("\\Deleted")])
            .await?;
        self.expunge_uids(set).await?;
        Ok(copied)
    }

    /// Expunge exactly `set` (UIDPLUS) or everything \Deleted (fallback).
    pub async fn expunge_uids(&mut self, set: &str) -> Result<()> {
        if self.caps.has("UIDPLUS") {
            self.run("UID", vec![Arg::raw("EXPUNGE"), Arg::raw(set)])
                .await?;
        } else {
            self.run("EXPUNGE", vec![]).await?;
        }
        Ok(())
    }

    /// APPEND a message; (uidvalidity, uid) when the server has UIDPLUS.
    pub async fn append(
        &mut self,
        mailbox: &str,
        flags: &str,
        message: &[u8],
    ) -> Result<Option<(u32, u32)>> {
        // The message always goes as a literal (never a quoted string), so
        // this doesn't use `run`.
        let tag = format!("p{:04}", self.next_tag);
        self.next_tag += 1;
        let mut line = format!("{tag} APPEND ").into_bytes();
        if !proto::quotable(mailbox.as_bytes()) {
            return Err(Error::Other("mailbox name can't be sent".into()));
        }
        line.extend_from_slice(&proto::quote(mailbox.as_bytes()));
        if !flags.is_empty() {
            line.extend_from_slice(format!(" ({flags})").as_bytes());
        }
        if self.caps.has("LITERAL+") {
            line.extend_from_slice(format!(" {{{}+}}\r\n", message.len()).as_bytes());
            self.write(&line).await?;
        } else {
            line.extend_from_slice(format!(" {{{}}}\r\n", message.len()).as_bytes());
            self.write(&line).await?;
            loop {
                match self.read().await? {
                    Response::Continue(_) => break,
                    Response::Tagged { status, .. } => return Err(status_error("APPEND", &status)),
                    Response::Untagged(_) => {}
                }
            }
        }
        let mut body = message.to_vec();
        body.extend_from_slice(b"\r\n");
        self.write(&body).await?;
        let done = self.collect(&tag, "APPEND").await?;
        Ok(appenduid(done.status.as_ref()))
    }

    /// `STATUS <mailbox> (<items>)` → item name (uppercase) → number.
    pub async fn status(&mut self, mailbox: &str, items: &str) -> Result<HashMap<String, u64>> {
        let done = self
            .run(
                "STATUS",
                vec![
                    Arg::string(mailbox.as_bytes()),
                    Arg::raw(format!("({items})")),
                ],
            )
            .await?;
        let mut out = HashMap::new();
        for u in done.untagged {
            if let Untagged::MailboxStatus { items, .. } = u {
                let mut it = items.into_iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    if let (Some(k), Some(v)) = (k.as_atom(), v.as_u64()) {
                        out.insert(k.to_ascii_uppercase(), v);
                    }
                }
            }
        }
        Ok(out)
    }

    pub async fn create(&mut self, mailbox: &str) -> Result<()> {
        self.run("CREATE", vec![Arg::string(mailbox.as_bytes())])
            .await?;
        // Best effort: some clients only show subscribed folders.
        let _ = self
            .run("SUBSCRIBE", vec![Arg::string(mailbox.as_bytes())])
            .await;
        Ok(())
    }

    pub async fn noop(&mut self) -> Result<Vec<Untagged>> {
        Ok(self.run("NOOP", vec![]).await?.untagged)
    }

    pub async fn logout(mut self) {
        if self.io.is_some() {
            let _ = tokio::time::timeout(Duration::from_secs(5), self.run("LOGOUT", vec![])).await;
        }
    }

    /// IDLE on the selected mailbox until it changes or `wait` elapses.
    /// Needs the IDLE capability (callers check).
    pub async fn idle(&mut self, wait: Duration) -> Result<IdleEvent> {
        let tag = format!("p{:04}", self.next_tag);
        self.next_tag += 1;
        self.write(format!("{tag} IDLE\r\n").as_bytes()).await?;
        loop {
            match self.read().await? {
                Response::Continue(_) => break,
                Response::Tagged { status, .. } => return Err(status_error("IDLE", &status)),
                Response::Untagged(Untagged::Status(st)) if st.kind == StatusKind::Bye => {
                    self.io = None;
                    return Err(status_error("IDLE", &st));
                }
                Response::Untagged(_) => {}
            }
        }
        let deadline = tokio::time::Instant::now() + wait;
        let event = loop {
            let io = self.io()?;
            let next = tokio::time::timeout_at(deadline, proto::read_response(io)).await;
            match next {
                Err(_) => break IdleEvent::Timeout,
                Ok(Err(e)) => {
                    self.io = None;
                    return Err(Error::Network(format!("connection lost: {e}")));
                }
                Ok(Ok(bytes)) => match proto::parse_response(&bytes) {
                    Ok(Response::Untagged(
                        Untagged::Exists(_)
                        | Untagged::Expunge(_)
                        | Untagged::Fetch { .. }
                        | Untagged::Vanished { .. },
                    )) => break IdleEvent::Changed,
                    Ok(Response::Untagged(Untagged::Status(st))) if st.kind == StatusKind::Bye => {
                        self.io = None;
                        return Err(status_error("IDLE", &st));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.io = None;
                        return Err(Error::Network(e.to_string()));
                    }
                },
            }
        };
        self.write(b"DONE\r\n").await?;
        self.collect(&tag, "IDLE").await?;
        Ok(event)
    }
}

fn appenduid(status: Option<&Status>) -> Option<(u32, u32)> {
    let (code, args) = status?.code.as_ref()?;
    if code != "APPENDUID" {
        return None;
    }
    let uidvalidity = args.first()?.as_u64()? as u32;
    let uid = args.get(1)?.as_text()?.parse().ok()?;
    Some((uidvalidity, uid))
}

/// Decode a list of X-GM-LABELS values to text (`\Inbox` stays as is).
pub fn gm_label_names(raw: &[Vec<u8>]) -> Vec<String> {
    raw.iter().map(|b| utf7::decode(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_keys() {
        assert_eq!(
            section_key("BODY[HEADER.FIELDS (MESSAGE-ID DATE)]"),
            "BODY[HEADER.FIELDS]"
        );
        assert_eq!(section_key("body[]<0>"), "BODY[]");
        assert_eq!(section_key("BODY[1.2]"), "BODY[1.2]");
        assert_eq!(section_key("RFC822"), "BODY[]");
        assert_eq!(section_key("BODY[1.MIME]"), "BODY[1.MIME]");
    }

    #[test]
    fn internal_dates() {
        assert_eq!(
            parse_internal_date(" 7-Feb-2026 09:10:11 +0100"),
            Some(1_770_451_811_000)
        );
        assert_eq!(
            parse_internal_date("17-Jul-1996 02:44:25 -0700"),
            Some(837_596_665_000)
        );
        assert_eq!(parse_internal_date("garbage"), None);
        assert_eq!(search_date(1_770_451_811_000), "7-Feb-2026");
    }

    #[test]
    fn fetches_merge_by_uid() {
        let a = FetchItem {
            uid: Some(4),
            flags: Some(vec!["\\Seen".into()]),
            ..Default::default()
        };
        let b = FetchItem {
            uid: Some(4),
            size: Some(9),
            ..Default::default()
        };
        let orphan = FetchItem {
            uid: None,
            ..Default::default()
        };
        let merged = merge_fetches(vec![a, b, orphan]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].size, Some(9));
        assert!(merged[0].has_flag("\\seen"));
    }

    #[test]
    fn errors_are_classified() {
        let st = |kind, code: Option<&str>, text: &str| Status {
            kind,
            code: code.map(|c| (c.to_string(), vec![])),
            text: text.into(),
        };
        assert!(matches!(
            status_error(
                "LOGIN",
                &st(StatusKind::No, Some("AUTHENTICATIONFAILED"), "bad")
            ),
            Error::NeedsReauth(_)
        ));
        assert!(matches!(
            status_error(
                "FETCH",
                &st(StatusKind::No, None, "Account exceeded bandwidth limits")
            ),
            Error::RateLimited
        ));
        assert!(matches!(
            status_error("SELECT", &st(StatusKind::No, Some("NONEXISTENT"), "no")),
            Error::NotFound(_)
        ));
        assert!(matches!(
            status_error("X", &st(StatusKind::Bye, None, "bye")),
            Error::Network(_)
        ));
        assert!(matches!(
            status_error("X", &st(StatusKind::No, Some("THROTTLED"), "slow down")),
            Error::RateLimited
        ));
    }

    #[test]
    fn copy_and_append_uids() {
        let st = Status {
            kind: StatusKind::Ok,
            code: Some((
                "COPYUID".into(),
                vec![
                    Value::Atom("9".into()),
                    Value::Atom("3:4".into()),
                    Value::Atom("10:11".into()),
                ],
            )),
            text: String::new(),
        };
        assert_eq!(
            copyuid(&st),
            Some(CopyUid {
                uidvalidity: 9,
                pairs: vec![(3, 10), (4, 11)]
            })
        );
        let st = Status {
            kind: StatusKind::Ok,
            code: Some((
                "APPENDUID".into(),
                vec![Value::Atom("9".into()), Value::Atom("77".into())],
            )),
            text: String::new(),
        };
        assert_eq!(appenduid(Some(&st)), Some((9, 77)));
    }
}
