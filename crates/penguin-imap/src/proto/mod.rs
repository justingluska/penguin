//! IMAP4rev1 wire format (RFC 3501 and the extensions Penguin uses): reading
//! one complete server response (a line plus any literals), tokenizing it
//! into [`Value`]s, and classifying it as a [`Response`].
//!
//! The tokenizer is deliberately generic: FETCH items, codes and extension
//! responses (EMAILID, X-GM-*, VANISHED, MODSEQ, ESEARCH, …) all come out as
//! atoms, strings and lists, and the session picks what it needs by name.
//! An extension this code has never heard of can't break parsing.

pub mod seqset;
pub mod utf7;

use std::fmt;

/// Largest single literal accepted from a server (a message body).
pub const MAX_LITERAL: usize = 200 * 1024 * 1024;
/// Largest line (outside literals) accepted: a SEARCH answer for a huge folder.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

/// One IMAP data item.
#[derive(Clone, PartialEq, Eq)]
pub enum Value {
    /// An atom or number (`\Seen`, `FETCH`, `42`, `BODY[HEADER]<0>`).
    Atom(String),
    /// A quoted string or a literal (raw bytes; mailbox names are mUTF-7).
    Str(Vec<u8>),
    List(Vec<Value>),
    Nil,
}

impl fmt::Debug for Value {
    // Values can hold message content: never print strings in full.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Atom(a) => write!(f, "{a}"),
            Value::Str(s) => write!(f, "<{} bytes>", s.len()),
            Value::List(l) => f.debug_list().entries(l).finish(),
            Value::Nil => write!(f, "NIL"),
        }
    }
}

impl Value {
    pub fn as_atom(&self) -> Option<&str> {
        match self {
            Value::Atom(a) => Some(a),
            _ => None,
        }
    }

    /// Atom text or string bytes.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Atom(a) => Some(a.as_bytes()),
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Atom or string as text (lossy UTF-8); None for NIL and lists.
    pub fn as_text(&self) -> Option<String> {
        self.as_bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Atom(a) => a.parse().ok(),
            Value::Str(s) => std::str::from_utf8(s).ok()?.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Atom equal to `name`, ignoring ASCII case.
    pub fn is_atom(&self, name: &str) -> bool {
        self.as_atom().is_some_and(|a| a.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed IMAP response: {}", self.0)
    }
}

fn err(what: &str) -> ParseError {
    ParseError(what.to_string())
}

/// Tokenizer over one response's bytes (literals inline, as they arrived).
pub struct Tokens<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Tokens<'a> {
    pub fn new(buf: &'a [u8]) -> Tokens<'a> {
        Tokens { buf, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(b' ')) {
            self.pos += 1;
        }
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), None | Some(b'\r') | Some(b'\n'))
    }

    /// The rest of the current line, as text (status response text).
    pub fn rest_text(&mut self) -> String {
        self.skip_spaces();
        let start = self.pos;
        while !self.at_end() {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.buf[start..self.pos]).into_owned()
    }

    /// Every remaining value on the response.
    pub fn values(&mut self) -> Result<Vec<Value>, ParseError> {
        let mut out = Vec::new();
        loop {
            self.skip_spaces();
            if self.at_end() {
                return Ok(out);
            }
            out.push(self.value()?);
        }
    }

    /// One value.
    pub fn value(&mut self) -> Result<Value, ParseError> {
        self.skip_spaces();
        match self.peek() {
            None => Err(err("unexpected end")),
            Some(b'(') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_spaces();
                    match self.peek() {
                        Some(b')') => {
                            self.pos += 1;
                            return Ok(Value::List(items));
                        }
                        None | Some(b'\r') | Some(b'\n') => return Err(err("unclosed list")),
                        _ => items.push(self.value()?),
                    }
                }
            }
            Some(b'"') => self.quoted().map(Value::Str),
            Some(b'{') => self.literal().map(Value::Str),
            Some(b'~') if self.buf.get(self.pos + 1) == Some(&b'{') => {
                self.pos += 1;
                self.literal().map(Value::Str)
            }
            Some(b')') => Err(err("unexpected ')'")),
            Some(_) => {
                let atom = self.atom()?;
                if atom.eq_ignore_ascii_case("NIL") {
                    Ok(Value::Nil)
                } else {
                    Ok(Value::Atom(atom))
                }
            }
        }
    }

    fn quoted(&mut self) -> Result<Vec<u8>, ParseError> {
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None | Some(b'\r') | Some(b'\n') => return Err(err("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    let next = *self
                        .buf
                        .get(self.pos + 1)
                        .ok_or_else(|| err("bad escape"))?;
                    out.push(next);
                    self.pos += 2;
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn literal(&mut self) -> Result<Vec<u8>, ParseError> {
        self.pos += 1; // '{'
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        let n: usize = std::str::from_utf8(&self.buf[start..self.pos])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| err("bad literal length"))?;
        if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
            self.pos += 1;
        }
        if self.peek() != Some(b'}') {
            return Err(err("bad literal"));
        }
        self.pos += 1;
        if self.buf.get(self.pos..self.pos + 2) != Some(b"\r\n") {
            return Err(err("literal without CRLF"));
        }
        self.pos += 2;
        let end = self.pos.checked_add(n).ok_or_else(|| err("literal size"))?;
        let bytes = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| err("short literal"))?
            .to_vec();
        self.pos = end;
        Ok(bytes)
    }

    /// An atom; `[...]` sections (with spaces and lists inside) and a
    /// trailing `<origin>` stay part of it (`BODY[HEADER.FIELDS (DATE)]<0>`).
    fn atom(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        let mut depth = 0usize;
        while let Some(c) = self.peek() {
            match c {
                b'[' => depth += 1,
                b']' if depth > 0 => depth -= 1,
                b'\r' | b'\n' => break,
                b' ' | b'(' | b')' | b'"' | b'{' if depth == 0 => break,
                _ => {}
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(err("empty atom"));
        }
        Ok(String::from_utf8_lossy(&self.buf[start..self.pos]).into_owned())
    }

    /// A number (for `* 12 EXISTS`, tags) or None without consuming.
    fn number(&mut self) -> Option<u32> {
        self.skip_spaces();
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        let n = std::str::from_utf8(&self.buf[start..self.pos])
            .ok()?
            .parse()
            .ok();
        if n.is_none() {
            self.pos = start;
        }
        n
    }
}

/// Status of a tagged or untagged OK/NO/BAD/BYE/PREAUTH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Ok,
    No,
    Bad,
    Bye,
    Preauth,
}

impl StatusKind {
    fn parse(s: &str) -> Option<StatusKind> {
        Some(match s.to_ascii_uppercase().as_str() {
            "OK" => StatusKind::Ok,
            "NO" => StatusKind::No,
            "BAD" => StatusKind::Bad,
            "BYE" => StatusKind::Bye,
            "PREAUTH" => StatusKind::Preauth,
            _ => return None,
        })
    }
}

/// resp-text: an optional `[CODE args]` and human text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub kind: StatusKind,
    /// The code atom (uppercased) and its arguments, e.g. `UIDVALIDITY 3`.
    pub code: Option<(String, Vec<Value>)>,
    pub text: String,
}

impl Status {
    pub fn code_is(&self, name: &str) -> bool {
        self.code
            .as_ref()
            .is_some_and(|(c, _)| c.eq_ignore_ascii_case(name))
    }

    /// The first argument of code `name` as a number (UIDVALIDITY, UIDNEXT, …).
    pub fn code_num(&self, name: &str) -> Option<u64> {
        match &self.code {
            Some((c, args)) if c.eq_ignore_ascii_case(name) => args.first()?.as_u64(),
            _ => None,
        }
    }
}

/// One classified server response.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// `+ text`: the server waits for a literal or an AUTHENTICATE step.
    Continue(String),
    Tagged {
        tag: String,
        status: Status,
    },
    Untagged(Untagged),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Untagged {
    Status(Status),
    Capability(Vec<String>),
    Exists(u32),
    Recent(u32),
    Expunge(u32),
    /// `* <seq> FETCH (<items>)`: the item list as name/value pairs.
    Fetch {
        seq: u32,
        items: Vec<Value>,
    },
    /// `* SEARCH 1 2 3 [(MODSEQ n)]`.
    Search(Vec<u32>),
    /// `* LIST (<attrs>) <delim> <name>`.
    List {
        attrs: Vec<String>,
        delimiter: Option<String>,
        name: Vec<u8>,
    },
    /// `* STATUS <mailbox> (<items>)`.
    MailboxStatus {
        name: Vec<u8>,
        items: Vec<Value>,
    },
    Flags(Vec<Value>),
    Enabled(Vec<String>),
    /// `* VANISHED [(EARLIER)] <uid set>` (QRESYNC).
    Vanished {
        earlier: bool,
        uids: String,
    },
    /// Anything else, keyword and values (ID, NAMESPACE, ESEARCH, …).
    Other(String, Vec<Value>),
}

fn parse_status(kind: StatusKind, t: &mut Tokens) -> Result<Status, ParseError> {
    t.skip_spaces();
    let mut code = None;
    if t.peek() == Some(b'[') {
        let start = t.pos + 1;
        let close = t.buf[start..]
            .iter()
            .position(|&c| c == b']' || c == b'\n')
            .map(|i| start + i)
            .ok_or_else(|| err("unclosed response code"))?;
        if t.buf[close] == b']' {
            let mut inner = Tokens::new(&t.buf[start..close]);
            let name = inner.atom().unwrap_or_default().to_ascii_uppercase();
            let args = inner.values().unwrap_or_default();
            code = Some((name, args));
            t.pos = close + 1;
        }
    }
    let text = t.rest_text();
    Ok(Status { kind, code, text })
}

/// Classify one complete response (as read by [`read_response`]).
pub fn parse_response(buf: &[u8]) -> Result<Response, ParseError> {
    let mut t = Tokens::new(buf);
    match t.peek() {
        Some(b'+') => {
            t.pos += 1;
            Ok(Response::Continue(t.rest_text()))
        }
        Some(b'*') => {
            t.pos += 1;
            parse_untagged(&mut t).map(Response::Untagged)
        }
        Some(_) => {
            let tag = t.atom()?;
            t.skip_spaces();
            let word = t.atom()?;
            let kind =
                StatusKind::parse(&word).ok_or_else(|| err("tagged response without status"))?;
            Ok(Response::Tagged {
                tag,
                status: parse_status(kind, &mut t)?,
            })
        }
        None => Err(err("empty response")),
    }
}

fn atoms(values: Vec<Value>) -> Vec<String> {
    values.into_iter().filter_map(|v| v.as_text()).collect()
}

fn parse_untagged(t: &mut Tokens) -> Result<Untagged, ParseError> {
    if let Some(n) = t.number() {
        t.skip_spaces();
        let word = t.atom()?.to_ascii_uppercase();
        return Ok(match word.as_str() {
            "EXISTS" => Untagged::Exists(n),
            "RECENT" => Untagged::Recent(n),
            "EXPUNGE" => Untagged::Expunge(n),
            "FETCH" => {
                let v = t.value()?;
                match v {
                    Value::List(items) => Untagged::Fetch { seq: n, items },
                    _ => return Err(err("FETCH without a list")),
                }
            }
            other => Untagged::Other(other.to_string(), t.values()?),
        });
    }
    t.skip_spaces();
    let word = t.atom()?;
    let upper = word.to_ascii_uppercase();
    if let Some(kind) = StatusKind::parse(&upper) {
        return Ok(Untagged::Status(parse_status(kind, t)?));
    }
    Ok(match upper.as_str() {
        "CAPABILITY" => Untagged::Capability(atoms(t.values()?)),
        "ENABLED" => Untagged::Enabled(atoms(t.values()?)),
        "FLAGS" => Untagged::Flags(match t.value()? {
            Value::List(l) => l,
            _ => Vec::new(),
        }),
        "SEARCH" => {
            let mut uids = Vec::new();
            for v in t.values()? {
                // A trailing (MODSEQ n) with CONDSTORE is ignored.
                if let Some(n) = v.as_u64() {
                    uids.push(n as u32);
                }
            }
            Untagged::Search(uids)
        }
        "LIST" | "LSUB" | "XLIST" => {
            let attrs = match t.value()? {
                Value::List(l) => atoms(l),
                _ => Vec::new(),
            };
            let delimiter = match t.value()? {
                Value::Nil => None,
                v => v.as_text(),
            };
            let name = t
                .value()?
                .as_bytes()
                .map(<[u8]>::to_vec)
                .ok_or_else(|| err("LIST without a name"))?;
            Untagged::List {
                attrs,
                delimiter,
                name,
            }
        }
        "STATUS" => {
            let name = t
                .value()?
                .as_bytes()
                .map(<[u8]>::to_vec)
                .ok_or_else(|| err("STATUS without a name"))?;
            let items = match t.value()? {
                Value::List(l) => l,
                _ => Vec::new(),
            };
            Untagged::MailboxStatus { name, items }
        }
        "VANISHED" => {
            let mut values = t.values()?;
            let earlier = matches!(values.first(), Some(Value::List(l)) if l.iter().any(|v| v.is_atom("EARLIER")));
            if earlier {
                values.remove(0);
            }
            Untagged::Vanished {
                earlier,
                uids: values.first().and_then(Value::as_text).unwrap_or_default(),
            }
        }
        _ => Untagged::Other(upper, t.values().unwrap_or_default()),
    })
}

/// The literal announced at the end of a line (`{123}` / `{123+}` /
/// `~{123}`), if any. `line` excludes the CRLF.
pub fn trailing_literal(line: &[u8]) -> Option<usize> {
    if line.last() != Some(&b'}') {
        return None;
    }
    let open = line.iter().rposition(|&c| c == b'{')?;
    let inner = &line[open + 1..line.len() - 1];
    let inner = inner
        .strip_suffix(b"+")
        .or_else(|| inner.strip_suffix(b"-"))
        .unwrap_or(inner);
    if inner.is_empty() || !inner.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(inner).ok()?.parse().ok()
}

/// Read one complete response: a line, and for every literal it announces,
/// the literal's bytes and the line that continues after it.
pub async fn read_response<R>(r: &mut R) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut out = Vec::new();
    loop {
        let start = out.len();
        let n = r.read_until(b'\n', &mut out).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed by the server",
            ));
        }
        if out.len() > MAX_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "server response too long",
            ));
        }
        let line = &out[start..];
        let body = line
            .strip_suffix(b"\r\n")
            .or_else(|| line.strip_suffix(b"\n"))
            .unwrap_or(line);
        match trailing_literal(body) {
            Some(len) if len <= MAX_LITERAL => {
                // Normalize a bare LF before the literal to CRLF so the
                // tokenizer sees `{n}\r\n`.
                if !line.ends_with(b"\r\n") {
                    out.pop();
                    out.extend_from_slice(b"\r\n");
                }
                let at = out.len();
                out.resize(at + len, 0);
                r.read_exact(&mut out[at..]).await?;
            }
            Some(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "server literal too large",
                ))
            }
            None => return Ok(out),
        }
    }
}

/// A command argument as it goes on the wire.
#[derive(Debug, Clone)]
pub enum Arg {
    /// Sent as is (keywords, sequence sets, flag lists, search keys).
    Raw(String),
    /// A string: quoted when safe, else a literal.
    Str(Vec<u8>),
}

impl Arg {
    pub fn raw(s: impl Into<String>) -> Arg {
        Arg::Raw(s.into())
    }
    pub fn string(s: impl AsRef<[u8]>) -> Arg {
        Arg::Str(s.as_ref().to_vec())
    }
    /// A mailbox name, encoded as modified UTF-7.
    pub fn mailbox(name: &str) -> Arg {
        Arg::Str(utf7::encode(name).into_bytes())
    }
}

/// Whether `s` can go on the wire as a quoted string.
pub fn quotable(s: &[u8]) -> bool {
    s.len() < 1024 && s.iter().all(|&c| (0x20..0x7f).contains(&c))
}

/// `"s"` with `\` and `"` escaped (caller checked [`quotable`]).
pub fn quote(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'"');
    for &c in s {
        if c == b'"' || c == b'\\' {
            out.push(b'\\');
        }
        out.push(c);
    }
    out.push(b'"');
    out
}

#[cfg(test)]
mod tests;
