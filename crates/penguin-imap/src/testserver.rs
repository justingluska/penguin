//! A scripted, in-memory IMAP + SMTP server for tests (plain TCP on
//! 127.0.0.1, which the client allows for loopback). It speaks enough of
//! IMAP4rev1 and the extensions Penguin uses (UIDPLUS, MOVE, CONDSTORE,
//! QRESYNC, OBJECTID, IDLE, SPECIAL-USE, and a Gmail mode with X-GM-EXT-1)
//! to drive the real client code end to end. Capabilities are chosen per
//! test so the fallbacks (no MOVE, no CONDSTORE, no UIDPLUS) run too.
//!
//! Gmail mode keeps every message in `[Gmail]/All Mail` with X-GM-LABELS;
//! INBOX, Drafts and Sent Mail are views of it by label (sharing All Mail's
//! UIDs), Trash and Spam are real folders, as on Gmail.
//!
//! Every FETCH that would set \Seen (BODY[...] or RFC822 without PEEK) is
//! recorded in `State::non_peek`, so tests can assert sync never does it.

use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};

use mail_parser::{MessageParser, MimeHeaders, PartType};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use crate::proto::{seqset, utf7, Tokens, Value};

pub const USER: &str = "sam@mail.example";
pub const PASS: &str = "app-pass-1234";

#[derive(Debug, Clone)]
pub struct Msg {
    pub uid: u32,
    pub flags: BTreeSet<String>,
    /// Shared so FETCH doesn't copy every message of a view per command.
    pub raw: Arc<Vec<u8>>,
    pub date_ms: i64,
    pub modseq: u64,
    pub emailid: String,
    pub gm_msgid: u64,
    pub gm_thrid: u64,
    pub gm_labels: BTreeSet<String>,
    /// Its BODYSTRUCTURE and part offsets, worked out once on first use (a
    /// real server keeps these in its index; re-parsing per FETCH made the
    /// test server, not the client, the bottleneck in benchmarks).
    pub parsed: Arc<OnceLock<Parsed>>,
}

#[derive(Debug)]
pub struct Parsed {
    bodystructure: String,
    /// Part path (`1`, `1.2`, …) → the byte range of its encoded body.
    parts: HashMap<String, (usize, usize)>,
}

impl Msg {
    pub fn parsed(&self) -> &Parsed {
        self.parsed.get_or_init(|| Parsed {
            bodystructure: bodystructure(&self.raw),
            parts: part_offsets(&self.raw),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Mailbox {
    pub name: String,
    pub attrs: Vec<String>,
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub msgs: Vec<Msg>,
    /// (uid, modseq) of expunged messages, for VANISHED.
    pub vanished: Vec<(u32, u64)>,
}

#[derive(Debug, Default)]
pub struct State {
    pub caps: Vec<String>,
    pub gmail: bool,
    pub mailboxes: Vec<Mailbox>,
    pub modseq: u64,
    next_id: u64,
    /// FETCHes without .PEEK (must stay empty).
    pub non_peek: Vec<String>,
    /// Command names received, in order.
    pub commands: Vec<String>,
    /// Messages received over SMTP: (from, recipients, data).
    pub smtp_sent: Vec<(String, Vec<String>, Vec<u8>)>,
    /// SMTP files the Sent copy itself (Gmail/iCloud behavior).
    pub smtp_saves_sent: bool,
    /// Refuse every login (revoked app password).
    pub refuse_login: bool,
    /// The next N commands with this name ("*" = any, "UID FETCH" for
    /// UID commands) get no answer: the connection just closes, as when
    /// the network drops or the server resets it mid-command.
    pub drop: Option<(String, u32)>,
    /// Held before every command's reply: a network round trip (benchmarks).
    pub latency: std::time::Duration,
    /// Reply bytes written to clients, before any COMPRESS.
    pub bytes_out: u64,
    /// Bytes that actually went out on the sockets (after COMPRESS).
    pub wire_out: Arc<std::sync::atomic::AtomicU64>,
    /// Link speed in bytes/s for replies (benchmarks; 0 = unlimited).
    pub bandwidth: u64,
}

/// Server-side changes wake idling clients.
pub struct TestServer {
    pub imap: SocketAddr,
    pub smtp: SocketAddr,
    pub state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    reap: Arc<Notify>,
}

pub const GENERIC_CAPS: &[&str] = &[
    "IMAP4rev1",
    "AUTH=PLAIN",
    "SASL-IR",
    "LITERAL+",
    "UIDPLUS",
    "MOVE",
    "IDLE",
    "ENABLE",
    "CONDSTORE",
    "QRESYNC",
    "OBJECTID",
    "SPECIAL-USE",
];

/// A minimal server: no MOVE, UIDPLUS, CONDSTORE, OBJECTID, LITERAL+.
pub const BASIC_CAPS: &[&str] = &["IMAP4rev1", "IDLE"];

impl TestServer {
    pub async fn start(caps: &[&str], gmail: bool) -> TestServer {
        let mut state = State {
            caps: caps.iter().map(|s| s.to_string()).collect(),
            gmail,
            modseq: 1,
            next_id: 0x1000,
            ..State::default()
        };
        if gmail {
            state.caps.push("X-GM-EXT-1".into());
        }
        let boxes: Vec<(&str, &[&str])> = if gmail {
            vec![
                ("INBOX", &[]),
                ("[Gmail]", &["\\Noselect"]),
                ("[Gmail]/All Mail", &["\\All"]),
                ("[Gmail]/Drafts", &["\\Drafts"]),
                ("[Gmail]/Sent Mail", &["\\Sent"]),
                ("[Gmail]/Spam", &["\\Junk"]),
                ("[Gmail]/Trash", &["\\Trash"]),
                ("Receipts", &[]),
            ]
        } else {
            vec![
                ("INBOX", &[]),
                ("Sent", &["\\Sent"]),
                ("Drafts", &["\\Drafts"]),
                ("Trash", &["\\Trash"]),
                ("Junk", &["\\Junk"]),
                ("Archive", &["\\Archive"]),
                ("Receipts", &[]),
            ]
        };
        for (i, (name, attrs)) in boxes.into_iter().enumerate() {
            state.mailboxes.push(Mailbox {
                name: name.into(),
                attrs: attrs.iter().map(|s| s.to_string()).collect(),
                uidvalidity: 1000 + i as u32,
                uidnext: 1,
                msgs: Vec::new(),
                vanished: Vec::new(),
            });
        }
        let state = Arc::new(Mutex::new(state));
        let changed = Arc::new(Notify::new());
        let reap = Arc::new(Notify::new());
        let imap = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let smtp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (imap_addr, smtp_addr) = (imap.local_addr().unwrap(), smtp.local_addr().unwrap());
        {
            let (state, changed, reap) = (state.clone(), changed.clone(), reap.clone());
            tokio::spawn(async move {
                while let Ok((sock, _)) = imap.accept().await {
                    // Each reply is written on its own; with Nagle a burst
                    // of small replies to pipelined commands would wait on
                    // the client's delayed ACKs (a real server buffers them).
                    let _ = sock.set_nodelay(true);
                    let (state, changed, reap) = (state.clone(), changed.clone(), reap.clone());
                    tokio::spawn(async move {
                        let _ = Conn::new(state, changed, reap).serve(sock).await;
                    });
                }
            });
        }
        {
            let (state, changed) = (state.clone(), changed.clone());
            tokio::spawn(async move {
                while let Ok((sock, _)) = smtp.accept().await {
                    let (state, changed) = (state.clone(), changed.clone());
                    tokio::spawn(async move {
                        let _ = serve_smtp(state, changed, sock).await;
                    });
                }
            });
        }
        TestServer {
            imap: imap_addr,
            smtp: smtp_addr,
            state,
            changed,
            reap,
        }
    }

    /// Log out every connection waiting for a command, as Yahoo does after
    /// 5 idle minutes: a plain `* BYE` even on a COMPRESS connection, then
    /// close.
    pub fn reap_idle(&self) {
        self.reap.notify_waiters();
    }

    /// A new, empty mailbox (as another client would CREATE it).
    pub fn add_folder(&self, name: &str) {
        let mut st = self.state.lock().unwrap();
        let n = st.mailboxes.len() as u32;
        st.mailboxes.push(Mailbox {
            name: name.into(),
            attrs: vec![],
            uidvalidity: 7000 + n,
            uidnext: 1,
            msgs: vec![],
            vanished: vec![],
        });
    }

    pub fn imap_settings(&self) -> penguin_core::ServerSettings {
        penguin_core::ServerSettings {
            host: "127.0.0.1".into(),
            port: self.imap.port(),
            security: penguin_core::MailSecurity::Plain,
            username: USER.into(),
        }
    }

    pub fn smtp_settings(&self) -> penguin_core::ServerSettings {
        penguin_core::ServerSettings {
            host: "127.0.0.1".into(),
            port: self.smtp.port(),
            security: penguin_core::MailSecurity::Plain,
            username: USER.into(),
        }
    }

    /// Deliver a message into `mailbox` (Gmail mode: into All Mail with
    /// the labels of that view). Returns its UID.
    pub fn deliver(&self, mailbox: &str, raw: &[u8], flags: &[&str], date_ms: i64) -> u32 {
        let uid = {
            let mut st = self.state.lock().unwrap();
            let flags: BTreeSet<String> = flags.iter().map(|s| s.to_string()).collect();
            st.insert(mailbox, raw.to_vec(), flags, date_ms)
        };
        self.changed.notify_waiters();
        uid
    }

    pub fn set_flags(&self, mailbox: &str, uid: u32, flags: &[&str]) {
        let mut st = self.state.lock().unwrap();
        st.modseq += 1;
        let m = st.modseq;
        let mb = st.real_mailbox(mailbox);
        let b = st.mailboxes.iter_mut().find(|b| b.name == mb).unwrap();
        let msg = b.msgs.iter_mut().find(|x| x.uid == uid).unwrap();
        msg.flags = flags.iter().map(|s| s.to_string()).collect();
        msg.modseq = m;
        drop(st);
        self.changed.notify_waiters();
    }

    /// Remove a message the way another client would (expunge).
    pub fn expunge(&self, mailbox: &str, uid: u32) {
        let mut st = self.state.lock().unwrap();
        st.modseq += 1;
        let m = st.modseq;
        let b = st.mailboxes.iter_mut().find(|b| b.name == mailbox).unwrap();
        b.msgs.retain(|x| x.uid != uid);
        b.vanished.push((uid, m));
        drop(st);
        self.changed.notify_waiters();
    }

    /// Move like another client: new UID in `to`, same message.
    pub fn move_message(&self, from: &str, uid: u32, to: &str) -> u32 {
        let mut st = self.state.lock().unwrap();
        let b = st.mailboxes.iter_mut().find(|b| b.name == from).unwrap();
        let pos = b.msgs.iter().position(|x| x.uid == uid).unwrap();
        let msg = b.msgs.remove(pos);
        st.modseq += 1;
        let m = st.modseq;
        let b = st.mailboxes.iter_mut().find(|b| b.name == from).unwrap();
        b.vanished.push((uid, m));
        let new = st.place(to, msg);
        drop(st);
        self.changed.notify_waiters();
        new
    }

    /// The server lost its UIDs (UIDVALIDITY changes, all UIDs renumbered).
    pub fn reset_uidvalidity(&self, mailbox: &str) {
        let mut st = self.state.lock().unwrap();
        let b = st.mailboxes.iter_mut().find(|b| b.name == mailbox).unwrap();
        b.uidvalidity += 100;
        b.vanished.clear();
        let mut next = 1;
        for m in &mut b.msgs {
            m.uid = next;
            next += 1;
        }
        b.uidnext = next;
    }

    pub fn messages(&self, mailbox: &str) -> Vec<Msg> {
        let st = self.state.lock().unwrap();
        st.view(mailbox)
    }
}

fn internal_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%d-%b-%Y %H:%M:%S +0000")
        .to_string()
}

impl State {
    fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c.eq_ignore_ascii_case(cap))
    }

    /// Gmail: label views live in All Mail.
    fn real_mailbox(&self, name: &str) -> String {
        if self.gmail
            && matches!(
                name,
                "INBOX" | "[Gmail]/Drafts" | "[Gmail]/Sent Mail" | "Receipts"
            )
        {
            "[Gmail]/All Mail".into()
        } else {
            name.to_string()
        }
    }

    fn view_label(&self, name: &str) -> Option<&'static str> {
        if !self.gmail {
            return None;
        }
        match name {
            "INBOX" => Some("\\Inbox"),
            "[Gmail]/Drafts" => Some("\\Draft"),
            "[Gmail]/Sent Mail" => Some("\\Sent"),
            "Receipts" => Some("Receipts"),
            _ => None,
        }
    }

    pub fn view(&self, name: &str) -> Vec<Msg> {
        let real = self.real_mailbox(name);
        let label = self.view_label(name);
        self.mailboxes
            .iter()
            .find(|b| b.name == real)
            .map(|b| {
                b.msgs
                    .iter()
                    .filter(|m| label.is_none_or(|l| m.gm_labels.contains(l)))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn insert(
        &mut self,
        mailbox: &str,
        raw: Vec<u8>,
        flags: BTreeSet<String>,
        date_ms: i64,
    ) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        let mut labels = BTreeSet::new();
        if let Some(l) = self.view_label(mailbox) {
            labels.insert(l.to_string());
        }
        // Gmail threads: by References root, else own id.
        let thrid = {
            let env = crate::mime::envelope(&raw);
            let root = env.references.first().cloned().or(env.in_reply_to.clone());
            root.and_then(|r| {
                self.mailboxes.iter().flat_map(|b| &b.msgs).find_map(|m| {
                    (crate::mime::envelope(&m.raw).message_id.as_deref() == Some(r.as_str()))
                        .then_some(m.gm_thrid)
                })
            })
            .unwrap_or(id)
        };
        let msg = Msg {
            uid: 0,
            flags,
            raw: Arc::new(raw),
            date_ms,
            modseq: 0,
            emailid: format!("M{id:x}"),
            gm_msgid: id,
            gm_thrid: thrid,
            gm_labels: labels,
            parsed: Arc::default(),
        };
        self.place(mailbox, msg)
    }

    fn place(&mut self, mailbox: &str, mut msg: Msg) -> u32 {
        let real = self.real_mailbox(mailbox);
        if let Some(l) = self.view_label(mailbox) {
            msg.gm_labels.insert(l.to_string());
        }
        self.modseq += 1;
        msg.modseq = self.modseq;
        let b = self
            .mailboxes
            .iter_mut()
            .find(|b| b.name == real)
            .expect("mailbox");
        msg.uid = b.uidnext;
        b.uidnext += 1;
        let uid = msg.uid;
        b.msgs.push(msg);
        uid
    }
}

impl Mailbox {
    /// Highest mod-sequence of its messages and expunges (at least 1).
    pub fn highestmodseq(&self) -> u64 {
        self.msgs
            .iter()
            .map(|m| m.modseq)
            .max()
            .unwrap_or(0)
            .max(self.vanished.iter().map(|v| v.1).max().unwrap_or(0))
            .max(1)
    }
}

struct Conn {
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    reap: Arc<Notify>,
    selected: Option<String>,
    readonly: bool,
    qresync: bool,
}

type Io = BufReader<Box<dyn crate::net::Io>>;

/// A socket that counts what is written to it (`State::wire_out`).
struct Counted {
    sock: TcpStream,
    out: Arc<std::sync::atomic::AtomicU64>,
}

impl tokio::io::AsyncRead for Counted {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.sock).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for Counted {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        data: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = std::pin::Pin::new(&mut self.sock).poll_write(cx, data);
        if let std::task::Poll::Ready(Ok(n)) = &polled {
            self.out
                .fetch_add(*n as u64, std::sync::atomic::Ordering::Relaxed);
        }
        polled
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.sock).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.sock).poll_shutdown(cx)
    }
}

async fn write(io: &mut Io, bytes: &[u8]) -> std::io::Result<()> {
    io.get_mut().write_all(bytes).await?;
    io.get_mut().flush().await
}

/// Read a command line; synchronizing literals get a `+` first.
async fn read_command(io: &mut Io) -> std::io::Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    loop {
        let start = out.len();
        if io.read_until(b'\n', &mut out).await? == 0 {
            return Ok(None);
        }
        let line = out[start..]
            .strip_suffix(b"\r\n")
            .unwrap_or(&out[start..])
            .to_vec();
        match crate::proto::trailing_literal(&line) {
            Some(n) => {
                if !line.ends_with(b"+}") {
                    write(io, b"+ go ahead\r\n").await?;
                }
                let at = out.len();
                out.resize(at + n, 0);
                io.read_exact(&mut out[at..]).await?;
            }
            None => return Ok(Some(out)),
        }
    }
}

fn lit(bytes: &[u8]) -> Vec<u8> {
    let mut v = format!("{{{}}}\r\n", bytes.len()).into_bytes();
    v.extend_from_slice(bytes);
    v
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Conn {
    fn new(state: Arc<Mutex<State>>, changed: Arc<Notify>, reap: Arc<Notify>) -> Conn {
        Conn {
            state,
            changed,
            reap,
            selected: None,
            readonly: false,
            qresync: false,
        }
    }

    async fn serve(mut self, sock: TcpStream) -> std::io::Result<()> {
        let out = self.state.lock().unwrap().wire_out.clone();
        // A handle on the bare socket, for the uncompressed idle BYE.
        let sock = sock.into_std()?;
        let plain = sock.try_clone()?;
        let sock = TcpStream::from_std(sock)?;
        let mut io: Io = BufReader::new(Box::new(Counted { sock, out }));
        let caps = self.state.lock().unwrap().caps.join(" ");
        write(
            &mut io,
            format!("* OK [CAPABILITY {caps}] test server ready\r\n").as_bytes(),
        )
        .await?;
        let mut authed = false;
        let mut compressed = false;
        let mut arrived = tokio::time::Instant::now();
        loop {
            // A command already buffered came in with the previous read
            // (pipelined): it has been waiting since then.
            let buffered = !io.buffer().is_empty();
            let reap = self.reap.clone();
            let line = tokio::select! {
                line = read_command(&mut io) => line?,
                _ = reap.notified(), if !buffered => {
                    use std::io::Write;
                    (&plain).write_all(b"* BYE IMAP4rev1 Server logging out\r\n")?;
                    return Ok(());
                }
            };
            let Some(line) = line else {
                break;
            };
            if !buffered {
                arrived = tokio::time::Instant::now();
            }
            let mut t = Tokens::new(&line);
            let Ok(values) = t.values() else {
                write(&mut io, b"* BAD parse\r\n").await?;
                continue;
            };
            let (Some(tag), Some(cmd)) = (
                values.first().and_then(Value::as_text),
                values.get(1).and_then(Value::as_text),
            ) else {
                continue;
            };
            let mut cmd = cmd.to_ascii_uppercase();
            let mut args: Vec<Value> = values[2..].to_vec();
            let uid = cmd == "UID";
            if uid {
                cmd = args
                    .first()
                    .and_then(Value::as_text)
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                args.remove(0);
            }
            let name = if uid {
                format!("UID {cmd}")
            } else {
                cmd.clone()
            };
            let dropped = {
                let mut st = self.state.lock().unwrap();
                st.commands.push(name.clone());
                match st.drop.as_mut() {
                    Some((only, n)) if *n > 0 && (only == "*" || *only == name) => {
                        *n -= 1;
                        true
                    }
                    _ => false,
                }
            };
            if dropped {
                return Ok(());
            }
            if !authed
                && !matches!(
                    cmd.as_str(),
                    "CAPABILITY" | "LOGIN" | "AUTHENTICATE" | "LOGOUT" | "NOOP"
                )
            {
                write(
                    &mut io,
                    format!("{tag} NO not authenticated\r\n").as_bytes(),
                )
                .await?;
                continue;
            }
            let reply: Vec<u8> = match cmd.as_str() {
                "CAPABILITY" => {
                    let caps = self.state.lock().unwrap().caps.join(" ");
                    format!("* CAPABILITY {caps}\r\n{tag} OK done\r\n").into_bytes()
                }
                "LOGIN" | "AUTHENTICATE" => {
                    let (user, pass) = if cmd == "LOGIN" {
                        (
                            args.first().and_then(Value::as_text).unwrap_or_default(),
                            args.get(1).and_then(Value::as_text).unwrap_or_default(),
                        )
                    } else {
                        use base64::Engine;
                        let token = match args.get(1).and_then(Value::as_text) {
                            Some(t) => t,
                            None => {
                                write(&mut io, b"+ \r\n").await?;
                                let mut l = String::new();
                                io.read_line(&mut l).await?;
                                l.trim().to_string()
                            }
                        };
                        let raw = base64::engine::general_purpose::STANDARD
                            .decode(token)
                            .unwrap_or_default();
                        let parts: Vec<String> = raw
                            .split(|b| *b == 0)
                            .map(|p| String::from_utf8_lossy(p).into_owned())
                            .collect();
                        (
                            parts.get(1).cloned().unwrap_or_default(),
                            parts.get(2).cloned().unwrap_or_default(),
                        )
                    };
                    let refuse = self.state.lock().unwrap().refuse_login;
                    if user == USER && pass == PASS && !refuse {
                        authed = true;
                        let caps = self.state.lock().unwrap().caps.join(" ");
                        format!("{tag} OK [CAPABILITY {caps}] logged in\r\n").into_bytes()
                    } else {
                        format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n")
                            .into_bytes()
                    }
                }
                "ENABLE" => {
                    let what: Vec<String> = args.iter().filter_map(Value::as_text).collect();
                    let mut enabled = Vec::new();
                    for w in what {
                        if w.eq_ignore_ascii_case("QRESYNC")
                            && self.state.lock().unwrap().has("QRESYNC")
                        {
                            self.qresync = true;
                            enabled.push("QRESYNC");
                        }
                        if w.eq_ignore_ascii_case("CONDSTORE")
                            && self.state.lock().unwrap().has("CONDSTORE")
                        {
                            enabled.push("CONDSTORE");
                        }
                    }
                    format!("* ENABLED {}\r\n{tag} OK enabled\r\n", enabled.join(" ")).into_bytes()
                }
                "LIST" => {
                    let st = self.state.lock().unwrap();
                    let mut out = String::new();
                    for b in &st.mailboxes {
                        out.push_str(&format!(
                            "* LIST ({}) \"/\" {}\r\n",
                            b.attrs.join(" "),
                            q(&utf7::encode(&b.name))
                        ));
                    }
                    out.push_str(&format!("{tag} OK listed\r\n"));
                    out.into_bytes()
                }
                "SELECT" | "EXAMINE" => {
                    let name =
                        utf7::decode(args.first().and_then(Value::as_bytes).unwrap_or_default());
                    let st = self.state.lock().unwrap();
                    let real = st.real_mailbox(&name);
                    match st
                        .mailboxes
                        .iter()
                        .find(|b| b.name == real && !b.attrs.iter().any(|a| a == "\\Noselect"))
                    {
                        None => format!("{tag} NO [NONEXISTENT] no such mailbox\r\n").into_bytes(),
                        Some(b) => {
                            let exists = st.view(&name).len();
                            let hms = b.highestmodseq();
                            let mut out = format!(
                                "* {exists} EXISTS\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT {}] ok\r\n* FLAGS (\\Seen \\Flagged \\Deleted \\Draft)\r\n",
                                b.uidvalidity, b.uidnext
                            );
                            if st.has("CONDSTORE") {
                                out.push_str(&format!("* OK [HIGHESTMODSEQ {hms}] ok\r\n"));
                            }
                            self.selected = Some(name.clone());
                            self.readonly = cmd == "EXAMINE";
                            let rw = if self.readonly {
                                "READ-ONLY"
                            } else {
                                "READ-WRITE"
                            };
                            out.push_str(&format!("{tag} OK [{rw}] selected\r\n"));
                            out.into_bytes()
                        }
                    }
                }
                "STATUS" => {
                    let name =
                        utf7::decode(args.first().and_then(Value::as_bytes).unwrap_or_default());
                    let st = self.state.lock().unwrap();
                    let n = st.view(&name).len();
                    let real = st.real_mailbox(&name);
                    match st.mailboxes.iter().find(|b| b.name == real) {
                        None => format!("{tag} NO [NONEXISTENT] no such mailbox\r\n").into_bytes(),
                        Some(b) => {
                            let asked_modseq = matches!(args.get(1), Some(Value::List(l)) if l.iter().any(|v| v.is_atom("HIGHESTMODSEQ")));
                            let modseq = if st.has("CONDSTORE") && asked_modseq {
                                format!(" HIGHESTMODSEQ {}", b.highestmodseq())
                            } else {
                                String::new()
                            };
                            format!(
                                "* STATUS {} (MESSAGES {n} UIDNEXT {} UIDVALIDITY {}{modseq})\r\n{tag} OK status\r\n",
                                q(&utf7::encode(&name)),
                                b.uidnext,
                                b.uidvalidity
                            )
                            .into_bytes()
                        }
                    }
                }
                "CREATE" => {
                    let name =
                        utf7::decode(args.first().and_then(Value::as_bytes).unwrap_or_default());
                    let mut st = self.state.lock().unwrap();
                    let n = st.mailboxes.len() as u32;
                    st.mailboxes.push(Mailbox {
                        name,
                        attrs: vec![],
                        uidvalidity: 5000 + n,
                        uidnext: 1,
                        msgs: vec![],
                        vanished: vec![],
                    });
                    format!("{tag} OK created\r\n").into_bytes()
                }
                "SUBSCRIBE" | "NOOP" | "CHECK" => format!("{tag} OK done\r\n").into_bytes(),
                "COMPRESS" => {
                    let deflate = args
                        .first()
                        .and_then(Value::as_text)
                        .is_some_and(|a| a.eq_ignore_ascii_case("DEFLATE"));
                    if !self.state.lock().unwrap().has("COMPRESS=DEFLATE") || !deflate {
                        format!("{tag} BAD not supported\r\n").into_bytes()
                    } else if compressed {
                        format!("{tag} NO [COMPRESSIONACTIVE] already\r\n").into_bytes()
                    } else {
                        // RFC 4978: the OK goes out plain, everything after
                        // it (both ways) is DEFLATE.
                        write(&mut io, format!("{tag} OK deflating\r\n").as_bytes()).await?;
                        let leftover = io.buffer().to_vec();
                        io = BufReader::new(Box::new(crate::compress::Deflate::new(
                            io.into_inner(),
                            leftover,
                        )));
                        compressed = true;
                        continue;
                    }
                }
                "LOGOUT" => {
                    write(
                        &mut io,
                        format!("* BYE bye\r\n{tag} OK logged out\r\n").as_bytes(),
                    )
                    .await?;
                    return Ok(());
                }
                "IDLE" => {
                    write(&mut io, b"+ idling\r\n").await?;
                    let changed = self.changed.clone();
                    let mut l = String::new();
                    tokio::select! {
                        _ = changed.notified() => {
                            write(&mut io, b"* 1 EXISTS\r\n").await?;
                            io.read_line(&mut l).await?;
                        }
                        r = io.read_line(&mut l) => { r?; }
                    }
                    format!("{tag} OK idle done\r\n").into_bytes()
                }
                "SEARCH" if uid => self.search(&tag, &args),
                "FETCH" if uid => self.fetch(&tag, &args),
                "STORE" if uid => self.store(&tag, &args),
                "MOVE" | "COPY" if uid => self.copy(&tag, &args, cmd == "MOVE"),
                "EXPUNGE" => self.expunge(
                    &tag,
                    if uid {
                        args.first().and_then(Value::as_text)
                    } else {
                        None
                    },
                ),
                "APPEND" => self.append(&tag, &args),
                _ => format!("{tag} BAD unknown command\r\n").into_bytes(),
            };
            let (latency, bandwidth, wire) = {
                let mut st = self.state.lock().unwrap();
                st.bytes_out += reply.len() as u64;
                (st.latency, st.bandwidth, st.wire_out.clone())
            };
            if !latency.is_zero() {
                // The reply leaves one round trip after the command arrived.
                tokio::time::sleep_until(arrived + latency).await;
            }
            let before = wire.load(std::sync::atomic::Ordering::Relaxed);
            write(&mut io, &reply).await?;
            if bandwidth > 0 {
                // The time those bytes would take on a link this fast.
                let sent = wire.load(std::sync::atomic::Ordering::Relaxed) - before;
                tokio::time::sleep(std::time::Duration::from_secs_f64(
                    sent as f64 / bandwidth as f64,
                ))
                .await;
            }
            if matches!(
                cmd.as_str(),
                "STORE" | "MOVE" | "COPY" | "EXPUNGE" | "APPEND"
            ) {
                self.changed.notify_waiters();
            }
        }
        Ok(())
    }

    fn sel(&self) -> Option<String> {
        self.selected.clone()
    }

    fn search(&self, tag: &str, args: &[Value]) -> Vec<u8> {
        let Some(name) = self.sel() else {
            return format!("{tag} BAD no mailbox\r\n").into_bytes();
        };
        let st = self.state.lock().unwrap();
        let msgs = st.view(&name);
        let mut args = args.to_vec();
        if args.first().is_some_and(|a| a.is_atom("CHARSET")) {
            args.drain(..2);
        }
        let max = msgs.iter().map(|m| m.uid).max().unwrap_or(0);
        let hits: Vec<u32> = msgs
            .iter()
            .filter(|m| {
                let mut it = args.iter();
                let mut ok = true;
                while it.len() > 0 {
                    ok &= eval(&mut it, m, max);
                }
                ok
            })
            .map(|m| m.uid)
            .collect();
        let list: Vec<String> = hits.iter().map(u32::to_string).collect();
        format!("* SEARCH {}\r\n{tag} OK searched\r\n", list.join(" ")).into_bytes()
    }

    fn fetch(&self, tag: &str, args: &[Value]) -> Vec<u8> {
        let Some(name) = self.sel() else {
            return format!("{tag} BAD no mailbox\r\n").into_bytes();
        };
        let set = args.first().and_then(Value::as_text).unwrap_or_default();
        let items: Vec<String> = match args.get(1) {
            Some(Value::List(l)) => l.iter().filter_map(Value::as_text).collect(),
            Some(v) => v.as_text().into_iter().collect(),
            None => vec![],
        };
        let mut changed_since = None;
        let mut vanished = false;
        if let Some(Value::List(m)) = args.get(2) {
            let mut it = m.iter();
            while let Some(k) = it.next() {
                if k.is_atom("CHANGEDSINCE") {
                    changed_since = it.next().and_then(Value::as_u64);
                } else if k.is_atom("VANISHED") {
                    vanished = true;
                }
            }
        }
        let mut st = self.state.lock().unwrap();
        let msgs = st.view(&name);
        let max = msgs.iter().map(|m| m.uid).max().unwrap_or(0);
        let wanted = seqset::parse(&set, max, 1_000_000).unwrap_or_default();
        let mut out: Vec<u8> = Vec::new();
        if vanished && self.qresync {
            if let Some(since) = changed_since {
                let real = st.real_mailbox(&name);
                let b = st.mailboxes.iter().find(|b| b.name == real).unwrap();
                let gone: Vec<u32> = b
                    .vanished
                    .iter()
                    .filter(|(u, m)| *m > since && wanted.contains(u))
                    .map(|(u, _)| *u)
                    .collect();
                if !gone.is_empty() {
                    out.extend(
                        format!("* VANISHED (EARLIER) {}\r\n", seqset::format(&gone)).into_bytes(),
                    );
                }
            }
        }
        for item in &items {
            let u = item.to_ascii_uppercase();
            if (u.starts_with("BODY[") || u == "RFC822") && !u.starts_with("BODY.PEEK") {
                st.non_peek.push(item.clone());
            }
        }
        for (seq, m) in msgs.iter().enumerate() {
            if !wanted.contains(&m.uid) {
                continue;
            }
            if changed_since.is_some_and(|c| m.modseq <= c) {
                continue;
            }
            let mut parts: Vec<Vec<u8>> = vec![format!("UID {}", m.uid).into_bytes()];
            for item in &items {
                let u = item.to_ascii_uppercase();
                match u.as_str() {
                    "UID" => {}
                    "FLAGS" => parts.push(
                        format!(
                            "FLAGS ({})",
                            m.flags.iter().cloned().collect::<Vec<_>>().join(" ")
                        )
                        .into_bytes(),
                    ),
                    "INTERNALDATE" => parts.push(
                        format!("INTERNALDATE \"{}\"", internal_date(m.date_ms)).into_bytes(),
                    ),
                    "RFC822.SIZE" => {
                        parts.push(format!("RFC822.SIZE {}", m.raw.len()).into_bytes())
                    }
                    "EMAILID" if st.has("OBJECTID") => {
                        parts.push(format!("EMAILID ({})", m.emailid).into_bytes())
                    }
                    "X-GM-MSGID" if st.gmail => {
                        parts.push(format!("X-GM-MSGID {}", m.gm_msgid).into_bytes())
                    }
                    "X-GM-THRID" if st.gmail => {
                        parts.push(format!("X-GM-THRID {}", m.gm_thrid).into_bytes())
                    }
                    "X-GM-LABELS" if st.gmail => parts.push(
                        format!(
                            "X-GM-LABELS ({})",
                            m.gm_labels
                                .iter()
                                .map(|l| if l.starts_with('\\') {
                                    l.clone()
                                } else {
                                    q(&utf7::encode(l))
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                        .into_bytes(),
                    ),
                    "BODYSTRUCTURE" => parts
                        .push(format!("BODYSTRUCTURE {}", m.parsed().bodystructure).into_bytes()),
                    "MODSEQ" => parts.push(format!("MODSEQ ({})", m.modseq).into_bytes()),
                    _ if u.starts_with("BODY.PEEK[") || u.starts_with("BODY[") => {
                        let (key, bytes) = section(m, item);
                        let mut p = format!("{key} ").into_bytes();
                        p.extend(lit(&bytes));
                        parts.push(p);
                    }
                    _ => {}
                }
            }
            if changed_since.is_some() {
                parts.push(format!("MODSEQ ({})", m.modseq).into_bytes());
            }
            let mut line = format!("* {} FETCH (", seq + 1).into_bytes();
            line.extend(parts.join(&b' '));
            line.extend_from_slice(b")\r\n");
            out.extend(line);
        }
        out.extend(format!("{tag} OK fetched\r\n").into_bytes());
        out
    }

    fn store(&self, tag: &str, args: &[Value]) -> Vec<u8> {
        let Some(name) = self.sel() else {
            return format!("{tag} BAD no mailbox\r\n").into_bytes();
        };
        if self.readonly {
            return format!("{tag} NO read-only\r\n").into_bytes();
        }
        let set = args.first().and_then(Value::as_text).unwrap_or_default();
        let op = args
            .get(1)
            .and_then(Value::as_text)
            .unwrap_or_default()
            .to_ascii_uppercase();
        let vals: Vec<String> = match args.get(2) {
            Some(Value::List(l)) => l
                .iter()
                .map(|v| utf7::decode(v.as_bytes().unwrap_or_default()))
                .collect(),
            Some(v) => vec![utf7::decode(v.as_bytes().unwrap_or_default())],
            None => vec![],
        };
        let mut st = self.state.lock().unwrap();
        let real = st.real_mailbox(&name);
        let label = st.view_label(&name).map(str::to_string);
        st.modseq += 1;
        let modseq = st.modseq;
        let b = st.mailboxes.iter_mut().find(|b| b.name == real).unwrap();
        let max = b.msgs.iter().map(|m| m.uid).max().unwrap_or(0);
        let wanted = seqset::parse(&set, max, 1_000_000).unwrap_or_default();
        for m in b.msgs.iter_mut().filter(|m| wanted.contains(&m.uid)) {
            if label.as_ref().is_some_and(|l| !m.gm_labels.contains(l)) {
                continue;
            }
            let target = if op.contains("X-GM-LABELS") {
                &mut m.gm_labels
            } else {
                &mut m.flags
            };
            let norm = |v: &String| -> String {
                if v.eq_ignore_ascii_case("\\Inbox") {
                    "\\Inbox".into()
                } else {
                    v.clone()
                }
            };
            if op.starts_with('+') {
                target.extend(vals.iter().map(norm));
            } else if op.starts_with('-') {
                for v in &vals {
                    target.remove(&norm(v));
                }
            } else {
                *target = vals.iter().map(norm).collect();
            }
            m.modseq = modseq;
        }
        format!("{tag} OK stored\r\n").into_bytes()
    }

    fn copy(&self, tag: &str, args: &[Value], is_move: bool) -> Vec<u8> {
        let Some(name) = self.sel() else {
            return format!("{tag} BAD no mailbox\r\n").into_bytes();
        };
        let set = args.first().and_then(Value::as_text).unwrap_or_default();
        let dest = utf7::decode(args.get(1).and_then(Value::as_bytes).unwrap_or_default());
        let mut st = self.state.lock().unwrap();
        if is_move && !st.has("MOVE") {
            return format!("{tag} BAD no MOVE\r\n").into_bytes();
        }
        let real_dest = st.real_mailbox(&dest);
        if !st.mailboxes.iter().any(|b| b.name == real_dest) {
            return format!("{tag} NO [TRYCREATE] no such mailbox\r\n").into_bytes();
        }
        let msgs = st.view(&name);
        let max = msgs.iter().map(|m| m.uid).max().unwrap_or(0);
        let wanted = seqset::parse(&set, max, 1_000_000).unwrap_or_default();
        let picked: Vec<Msg> = msgs
            .into_iter()
            .filter(|m| wanted.contains(&m.uid))
            .collect();
        let real_src = st.real_mailbox(&name);
        let gmail = st.gmail;
        let mut src = Vec::new();
        let mut dst = Vec::new();
        for m in &picked {
            src.push(m.uid);
            let mut copy = m.clone();
            if gmail && is_move {
                // Leaving Trash/Spam or moving into them: labels reset.
                if matches!(dest.as_str(), "[Gmail]/Trash" | "[Gmail]/Spam") {
                    copy.gm_labels.clear();
                }
            }
            if gmail && real_dest == real_src && real_dest == "[Gmail]/All Mail" {
                // A label change in the same store.
                let b = st
                    .mailboxes
                    .iter_mut()
                    .find(|b| b.name == real_src)
                    .unwrap();
                let x = b.msgs.iter_mut().find(|x| x.uid == m.uid).unwrap();
                if let Some(l) = st_view_label(&dest) {
                    x.gm_labels.insert(l.to_string());
                }
                if is_move {
                    if let Some(l) = st_view_label(&name) {
                        x.gm_labels.remove(l);
                    }
                }
                dst.push(m.uid);
                continue;
            }
            dst.push(st.place(&dest, copy));
        }
        let (uv_src, uv_dst) = (
            st.mailboxes
                .iter()
                .find(|b| b.name == real_src)
                .unwrap()
                .uidvalidity,
            st.mailboxes
                .iter()
                .find(|b| b.name == real_dest)
                .unwrap()
                .uidvalidity,
        );
        let _ = uv_src;
        let mut out = String::new();
        let code = if st.has("UIDPLUS") {
            format!(
                "[COPYUID {uv_dst} {} {}] ",
                seqset::format(&src),
                seqset::format(&dst)
            )
        } else {
            String::new()
        };
        if is_move && !(gmail && real_dest == real_src) {
            out.push_str(&format!("* OK {code}moved\r\n"));
            st.modseq += 1;
            let m = st.modseq;
            let b = st
                .mailboxes
                .iter_mut()
                .find(|b| b.name == real_src)
                .unwrap();
            for u in &src {
                if let Some(pos) = b.msgs.iter().position(|x| x.uid == *u) {
                    b.msgs.remove(pos);
                    b.vanished.push((*u, m));
                    out.push_str(&format!("* {} EXPUNGE\r\n", pos + 1));
                }
            }
            out.push_str(&format!("{tag} OK done\r\n"));
        } else {
            out.push_str(&format!("{tag} OK {code}done\r\n"));
        }
        out.into_bytes()
    }

    fn expunge(&self, tag: &str, set: Option<String>) -> Vec<u8> {
        let Some(name) = self.sel() else {
            return format!("{tag} BAD no mailbox\r\n").into_bytes();
        };
        let mut st = self.state.lock().unwrap();
        let real = st.real_mailbox(&name);
        st.modseq += 1;
        let modseq = st.modseq;
        let b = st.mailboxes.iter_mut().find(|b| b.name == real).unwrap();
        let max = b.msgs.iter().map(|m| m.uid).max().unwrap_or(0);
        let only = set.map(|s| seqset::parse(&s, max, 1_000_000).unwrap_or_default());
        let mut out = String::new();
        let mut i = 0;
        while i < b.msgs.len() {
            let m = &b.msgs[i];
            if m.flags.contains("\\Deleted") && only.as_ref().is_none_or(|o| o.contains(&m.uid)) {
                b.vanished.push((m.uid, modseq));
                b.msgs.remove(i);
                out.push_str(&format!("* {} EXPUNGE\r\n", i + 1));
            } else {
                i += 1;
            }
        }
        out.push_str(&format!("{tag} OK expunged\r\n"));
        out.into_bytes()
    }

    fn append(&self, tag: &str, args: &[Value]) -> Vec<u8> {
        let name = utf7::decode(args.first().and_then(Value::as_bytes).unwrap_or_default());
        let mut flags = BTreeSet::new();
        let mut raw = Vec::new();
        for a in &args[1..] {
            match a {
                Value::List(l) => flags.extend(l.iter().filter_map(Value::as_text)),
                Value::Str(s) => raw = s.clone(),
                _ => {}
            }
        }
        let mut st = self.state.lock().unwrap();
        let real = st.real_mailbox(&name);
        let Some(uv) = st
            .mailboxes
            .iter()
            .find(|b| b.name == real)
            .map(|b| b.uidvalidity)
        else {
            return format!("{tag} NO [TRYCREATE] no such mailbox\r\n").into_bytes();
        };
        let uid = st.insert(&name, raw, flags, 1_760_000_000_000);
        if st.has("UIDPLUS") {
            format!("{tag} OK [APPENDUID {uv} {uid}] appended\r\n").into_bytes()
        } else {
            format!("{tag} OK appended\r\n").into_bytes()
        }
    }
}

fn st_view_label(name: &str) -> Option<&'static str> {
    match name {
        "INBOX" => Some("\\Inbox"),
        "[Gmail]/Drafts" => Some("\\Draft"),
        "[Gmail]/Sent Mail" => Some("\\Sent"),
        "Receipts" => Some("Receipts"),
        _ => None,
    }
}

/// Evaluate one search key (consuming its arguments) against a message.
fn eval(it: &mut std::slice::Iter<Value>, m: &Msg, max: u32) -> bool {
    let Some(k) = it.next() else { return true };
    if let Value::List(inner) = k {
        let mut i = inner.iter();
        let mut ok = true;
        while i.len() > 0 {
            ok &= eval(&mut i, m, max);
        }
        return ok;
    }
    let key = k.as_text().unwrap_or_default().to_ascii_uppercase();
    let mut arg = || it.next().and_then(Value::as_text).unwrap_or_default();
    let headers = crate::mime::header_fields(&m.raw);
    let header = |n: &str| {
        headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let day = |s: &str| {
        chrono::NaiveDate::parse_from_str(s, "%d-%b-%Y")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis())
            .unwrap_or(0)
    };
    match key.as_str() {
        "ALL" => true,
        "UID" => {
            let set = arg();
            seqset::parse(&set, max, 1_000_000)
                .unwrap_or_default()
                .contains(&m.uid)
        }
        "SINCE" => m.date_ms >= day(&arg()),
        "BEFORE" => m.date_ms < day(&arg()),
        "UNSEEN" => !m.flags.contains("\\Seen"),
        "SEEN" => m.flags.contains("\\Seen"),
        "FLAGGED" => m.flags.contains("\\Flagged"),
        "UNFLAGGED" => !m.flags.contains("\\Flagged"),
        "DELETED" => m.flags.contains("\\Deleted"),
        "NOT" => !eval(it, m, max),
        "OR" => {
            let a = eval(it, m, max);
            let b = eval(it, m, max);
            a || b
        }
        "HEADER" => {
            let (name, value) = (arg(), arg().to_lowercase());
            header(&name).contains(&value)
        }
        "FROM" | "TO" | "CC" | "BCC" | "SUBJECT" => {
            let v = arg().to_lowercase();
            header(&key).contains(&v)
        }
        "TEXT" | "BODY" => {
            let v = arg().to_lowercase();
            String::from_utf8_lossy(&m.raw).to_lowercase().contains(&v)
        }
        "EMAILID" => arg() == m.emailid,
        "X-GM-MSGID" => arg().parse::<u64>().ok() == Some(m.gm_msgid),
        "X-GM-RAW" => {
            let q = arg();
            match q.strip_prefix("rfc822msgid:") {
                Some(id) => header("Message-ID").contains(&id.to_lowercase()),
                None => String::from_utf8_lossy(&m.raw)
                    .to_lowercase()
                    .contains(&q.to_lowercase()),
            }
        }
        _ => false,
    }
}

/// `BODY[...]` for a fetch item: (response key, bytes).
fn section(m: &Msg, item: &str) -> (String, Vec<u8>) {
    let raw: &[u8] = &m.raw;
    let open = item.find('[').unwrap_or(0);
    let close = item.rfind(']').unwrap_or(item.len());
    let spec = &item[open + 1..close];
    let partial = item[close + 1..]
        .trim_start_matches('<')
        .trim_end_matches('>');
    let key = format!("BODY[{spec}]");
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(raw.len(), |i| i + 4);
    let upper = spec.to_ascii_uppercase();
    let mut bytes = if spec.is_empty() {
        raw.to_vec()
    } else if upper == "HEADER" {
        raw[..header_end].to_vec()
    } else if upper == "TEXT" {
        raw[header_end..].to_vec()
    } else if upper.starts_with("HEADER.FIELDS") {
        let names: Vec<String> = spec
            [spec.find('(').unwrap_or(0) + 1..spec.rfind(')').unwrap_or(spec.len())]
            .split_whitespace()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        let mut out = String::new();
        for (n, v) in crate::mime::header_fields(raw) {
            if names.contains(&n.to_ascii_lowercase()) {
                out.push_str(&format!("{n}: {v}\r\n"));
            }
        }
        out.push_str("\r\n");
        out.into_bytes()
    } else {
        m.parsed()
            .parts
            .get(spec)
            .and_then(|&(start, end)| raw.get(start..end))
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    };
    let mut key = key;
    if let Some((start, count)) = partial.split_once('.') {
        let (s, c): (usize, usize) = (start.parse().unwrap_or(0), count.parse().unwrap_or(0));
        bytes = bytes
            .get(s.min(bytes.len())..(s + c).min(bytes.len()))
            .unwrap_or_default()
            .to_vec();
        key = format!("{key}<{s}>");
    }
    (key, bytes)
}

/// Where each part's encoded body sits: `1`, `1.2`, … through the
/// multipart tree (a single-part message's body is part 1).
fn part_offsets(raw: &[u8]) -> HashMap<String, (usize, usize)> {
    fn walk(
        msg: &mail_parser::Message,
        id: usize,
        path: String,
        out: &mut HashMap<String, (usize, usize)>,
    ) {
        let Some(p) = msg.parts.get(id) else { return };
        if let PartType::Multipart(ch) = &p.body {
            for (i, c) in ch.iter().enumerate() {
                let child = if path.is_empty() {
                    (i + 1).to_string()
                } else {
                    format!("{path}.{}", i + 1)
                };
                walk(msg, *c as usize, child, out);
            }
        }
        let key = if path.is_empty() {
            "1".to_string()
        } else {
            path
        };
        out.entry(key)
            .or_insert((p.offset_body as usize, p.offset_end as usize));
    }
    let mut out = HashMap::new();
    if let Some(msg) = MessageParser::default().parse(raw) {
        if matches!(
            msg.parts.first().map(|p| &p.body),
            Some(PartType::Multipart(_))
        ) {
            walk(&msg, 0, String::new(), &mut out);
        } else if let Some(p) = msg.parts.first() {
            out.insert("1".into(), (p.offset_body as usize, p.offset_end as usize));
        }
    }
    out
}

/// A BODYSTRUCTURE for `raw` (enough of RFC 3501 for the client).
fn bodystructure(raw: &[u8]) -> String {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return "(\"text\" \"plain\" NIL NIL NIL \"7bit\" 0 0)".into();
    };
    fn node(msg: &mail_parser::Message, id: usize) -> String {
        let p = &msg.parts[id];
        if let PartType::Multipart(ch) = &p.body {
            let kids: String = ch.iter().map(|c| node(msg, *c as usize)).collect();
            let sub = p
                .content_type()
                .and_then(|c| c.subtype())
                .unwrap_or("mixed");
            return format!("({kids} {})", q(sub));
        }
        let (ty, sub) = p
            .content_type()
            .map(|c| {
                (
                    c.ctype().to_string(),
                    c.subtype().unwrap_or("plain").to_string(),
                )
            })
            .unwrap_or(("text".into(), "plain".into()));
        let mut params = Vec::new();
        if let Some(cs) = p.content_type().and_then(|c| c.attribute("charset")) {
            params.push(format!("\"charset\" {}", q(cs)));
        }
        if let Some(n) = p.content_type().and_then(|c| c.attribute("name")) {
            params.push(format!("\"name\" {}", q(n)));
        }
        let params = if params.is_empty() {
            "NIL".to_string()
        } else {
            format!("({})", params.join(" "))
        };
        let cid = p
            .content_id()
            .map(|c| q(&format!("<{}>", c.trim_matches(['<', '>']))))
            .unwrap_or("NIL".into());
        let enc = p.content_transfer_encoding().unwrap_or("7bit");
        let size = p.offset_end.saturating_sub(p.offset_body);
        let disp = match p.content_disposition() {
            Some(d) => {
                let fname = d
                    .attribute("filename")
                    .map(|f| format!("(\"filename\" {})", q(f)))
                    .unwrap_or("NIL".into());
                format!("({} {fname})", q(d.ctype()))
            }
            None => "NIL".into(),
        };
        let lines = if ty.eq_ignore_ascii_case("text") {
            " 1".to_string()
        } else {
            String::new()
        };
        format!(
            "({} {} {params} {cid} NIL {} {size}{lines} NIL {disp} NIL)",
            q(&ty),
            q(&sub),
            q(enc)
        )
    }
    node(&msg, 0)
}

async fn serve_smtp(
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    sock: TcpStream,
) -> std::io::Result<()> {
    let out = state.lock().unwrap().wire_out.clone();
    let mut io: Io = BufReader::new(Box::new(Counted { sock, out }));
    write(&mut io, b"220 test smtp\r\n").await?;
    let mut from = String::new();
    let mut rcpts = Vec::new();
    let mut authed = false;
    loop {
        let mut line = String::new();
        if io.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let l = line.trim_end().to_string();
        let upper = l.to_ascii_uppercase();
        if upper.starts_with("EHLO") {
            write(
                &mut io,
                b"250-test\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n250 SIZE 1000000\r\n",
            )
            .await?;
        } else if upper.starts_with("AUTH PLAIN ") {
            use base64::Engine;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(l[11..].trim())
                .unwrap_or_default();
            let parts: Vec<String> = raw
                .split(|b| *b == 0)
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .collect();
            let refuse = state.lock().unwrap().refuse_login;
            if parts.get(1).map(String::as_str) == Some(USER)
                && parts.get(2).map(String::as_str) == Some(PASS)
                && !refuse
            {
                authed = true;
                write(&mut io, b"235 ok\r\n").await?;
            } else {
                write(&mut io, b"535 5.7.8 Username and Password not accepted\r\n").await?;
            }
        } else if upper.starts_with("MAIL FROM:") {
            if !authed {
                write(&mut io, b"530 auth first\r\n").await?;
                continue;
            }
            from = l[10..]
                .split('>')
                .next()
                .unwrap_or("")
                .trim_start_matches('<')
                .to_string();
            write(&mut io, b"250 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            rcpts.push(
                l[8..]
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string(),
            );
            write(&mut io, b"250 ok\r\n").await?;
        } else if upper == "DATA" {
            write(&mut io, b"354 go\r\n").await?;
            let mut data = Vec::new();
            loop {
                let mut dl = Vec::new();
                io.read_until(b'\n', &mut dl).await?;
                if dl == b".\r\n" {
                    break;
                }
                if dl.starts_with(b"..") {
                    dl.remove(0);
                }
                data.extend(dl);
            }
            {
                let mut st = state.lock().unwrap();
                if st.smtp_saves_sent {
                    let sent = if st.gmail {
                        "[Gmail]/Sent Mail"
                    } else {
                        "Sent"
                    };
                    let mut flags = BTreeSet::new();
                    flags.insert("\\Seen".to_string());
                    st.insert(sent, data.clone(), flags, 1_760_000_100_000);
                }
                st.smtp_sent.push((from.clone(), rcpts.clone(), data));
            }
            changed.notify_waiters();
            rcpts.clear();
            write(&mut io, b"250 2.0.0 queued\r\n").await?;
        } else if upper == "QUIT" {
            write(&mut io, b"221 bye\r\n").await?;
            return Ok(());
        } else {
            write(&mut io, b"502 unknown\r\n").await?;
        }
    }
}
