//! An in-memory Microsoft Graph (and token endpoint) behind the
//! [`Transport`] seam: a stateful mailbox answering the requests Penguin
//! makes, with Graph's JSON shapes, per-folder delta (initial rounds,
//! `$deltatoken` rounds, `@removed` entries, 410 for expired links),
//! immutable ids that survive moves but change when a draft is sent,
//! throttling on demand, and rotating refresh tokens.
//!
//! Handwritten from Graph's documented shapes; every person and domain is
//! fictional (`.example`). It checks what a real server would refuse:
//! a missing bearer token (401) and a missing `Prefer: IdType="ImmutableId"`
//! (counted, and the tests assert it never happens).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use base64::Engine;
use penguin_provider::{async_trait, Result};
use serde_json::{json, Value};

use crate::http::{Endpoints, Method, Request, Response, Transport};

pub(crate) const GRAPH: &str = "https://graph.fake.example/v1.0";
const TOKEN_URL: &str = "https://login.fake.example/common/oauth2/v2.0/token";
const AUTHORIZE_URL: &str = "https://login.fake.example/common/oauth2/v2.0/authorize";
const UPLOAD_HOST: &str = "https://upload.fake.example/";
pub(crate) const ME: &str = "sam@outlook.example";

pub(crate) fn endpoints() -> Endpoints {
    Endpoints {
        graph: GRAPH.into(),
        authorize: AUTHORIZE_URL.into(),
        token: TOKEN_URL.into(),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FAtt {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub inline: bool,
    pub content_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FMsg {
    pub id: String,
    pub conversation: String,
    pub folder: String,
    pub received_ms: i64,
    pub subject: String,
    pub body_html: String,
    pub from: (String, String),
    pub to: Vec<(String, String)>,
    pub cc: Vec<(String, String)>,
    pub bcc: Vec<(String, String)>,
    pub is_read: bool,
    pub flagged: bool,
    pub categories: Vec<String>,
    pub is_draft: bool,
    pub imid: String,
    pub headers: Vec<(String, String)>,
    pub attachments: Vec<FAtt>,
}

#[derive(Debug, Clone)]
struct FFolder {
    id: String,
    name: String,
    parent: String,
    well_known: Option<&'static str>,
}

/// A scripted failure: requests whose path contains `pattern` (or is
/// exactly it, when it ends in `$`) answer
/// `status` (with `Retry-After` when given), `times` times.
#[derive(Debug, Clone)]
pub(crate) struct Failure {
    pub pattern: String,
    pub status: u16,
    pub retry_after: Option<u64>,
    pub times: u32,
}

#[derive(Default)]
pub(crate) struct State {
    folders: Vec<FFolder>,
    pub messages: BTreeMap<String, FMsg>,
    /// (seq, folder id, message id): every change a delta round reports.
    changes: Vec<(u64, String, String)>,
    seq: u64,
    next: u64,
    /// Paged listings: token → entries (message id, removed?), plus the
    /// deltaLink to hand out at the end.
    snapshots: HashMap<String, (Vec<(String, bool)>, String)>,
    pub expired: HashSet<String>,
    pub refuse_ordered_delta: bool,
    /// Every request: "METHOD /path" (no query).
    pub requests: Vec<String>,
    pub missing_immutable: u32,
    pub failures: VecDeque<Failure>,
    pub access_tokens: HashSet<String>,
    pub refresh_token: String,
    pub reject_refresh: bool,
    pub token_calls: u32,
    pub categories: Vec<String>,
    pub photo: Option<Vec<u8>>,
    uploads: HashMap<String, (String, String, Vec<u8>)>,
    /// Upload sessions' AttachmentItem: session → (contentId, contentType).
    upload_meta: HashMap<String, (Option<String>, String)>,
    /// Messages sent (their Sent Items ids).
    pub sent: Vec<String>,
    /// Meeting invitations: message id → the event id it links to.
    pub meeting_events: HashMap<String, String>,
    /// Event actions answered: (event id, action, body).
    pub meeting_actions: Vec<(String, String, Value)>,
}

pub(crate) struct FakeGraph {
    pub state: Mutex<State>,
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn date(ms: i64) -> String {
    crate::convert::format_date(ms)
}

fn rcpt((name, addr): &(String, String)) -> Value {
    json!({ "emailAddress": { "name": name, "address": addr } })
}

fn rcpts(list: &[(String, String)]) -> Value {
    Value::Array(list.iter().map(rcpt).collect())
}

fn from_rcpts(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|r| {
                    let e = &r["emailAddress"];
                    (
                        e["name"].as_str().unwrap_or("").to_string(),
                        e["address"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn graph_error(status: u16, code: &str) -> Response {
    Response {
        status,
        headers: vec![],
        body: serde_json::to_vec(&json!({ "error": { "code": code, "message": "fake" } })).unwrap(),
    }
}

fn ok(v: Value) -> Response {
    Response {
        status: 200,
        headers: vec![],
        body: serde_json::to_vec(&v).unwrap(),
    }
}

fn status(code: u16) -> Response {
    Response {
        status: code,
        headers: vec![],
        body: vec![],
    }
}

impl FMsg {
    fn light(&self) -> Value {
        json!({
            "id": self.id,
            "conversationId": self.conversation,
            "receivedDateTime": date(self.received_ms),
            "subject": self.subject,
            "bodyPreview": crate::convert::snippet(&penguin_core::text::html_to_text(&self.body_html)),
            "from": rcpt(&self.from),
            "sender": rcpt(&self.from),
            "toRecipients": rcpts(&self.to),
            "ccRecipients": rcpts(&self.cc),
            "isRead": self.is_read,
            "isDraft": self.is_draft,
            "flag": { "flagStatus": if self.flagged { "flagged" } else { "notFlagged" } },
            "categories": self.categories,
            "parentFolderId": self.folder,
            "internetMessageId": format!("<{}>", self.imid),
            "hasAttachments": self.attachments.iter().any(|a| !a.inline),
        })
    }

    fn full(&self, with_size: bool) -> Value {
        let mut v = self.light();
        v["body"] = json!({ "contentType": "html", "content": self.body_html });
        v["bccRecipients"] = rcpts(&self.bcc);
        v["replyTo"] = json!([]);
        v["sentDateTime"] = json!(date(self.received_ms));
        v["internetMessageHeaders"] = Value::Array(
            self.headers
                .iter()
                .map(|(k, val)| json!({ "name": k, "value": val }))
                .collect(),
        );
        // Like Graph with $select on the base type: no contentId here.
        v["attachments"] = Value::Array(
            self.attachments
                .iter()
                .map(|a| {
                    json!({
                        "@odata.type": "#microsoft.graph.fileAttachment",
                        "id": a.id, "name": a.name, "contentType": a.content_type,
                        "size": a.bytes.len(), "isInline": a.inline,
                    })
                })
                .collect(),
        );
        if with_size {
            v["singleValueExtendedProperties"] = json!([{ "id": "Integer 0xe08", "value": (2048 + self.body_html.len()).to_string() }]);
        }
        v
    }

    fn mime(&self) -> Vec<u8> {
        let mut s = format!(
            "From: {} <{}>\r\nTo: {}\r\nSubject: {}\r\nMessage-ID: <{}>\r\n",
            self.from.0,
            self.from.1,
            self.to
                .iter()
                .map(|t| t.1.clone())
                .collect::<Vec<_>>()
                .join(", "),
            self.subject,
            self.imid
        );
        for (k, v) in &self.headers {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
        s.push_str("Content-Type: text/html; charset=utf-8\r\n\r\n");
        s.push_str(&self.body_html);
        s.into_bytes()
    }
}

impl FakeGraph {
    /// A mailbox with the well-known folders, a user folder under the inbox
    /// ("Receipts") and one at the root ("Clients").
    pub fn new() -> Arc<FakeGraph> {
        let mut s = State {
            refresh_token: "rt-0".into(),
            categories: vec!["Red category".into()],
            ..State::default()
        };
        for (id, name, parent, wk) in [
            ("FLD-inbox", "Inbox", "ROOT", Some("inbox")),
            ("FLD-sent", "Sent Items", "ROOT", Some("sentitems")),
            ("FLD-drafts", "Drafts", "ROOT", Some("drafts")),
            ("FLD-trash", "Deleted Items", "ROOT", Some("deleteditems")),
            ("FLD-junk", "Junk Email", "ROOT", Some("junkemail")),
            ("FLD-archive", "Archive", "ROOT", Some("archive")),
            ("FLD-outbox", "Outbox", "ROOT", Some("outbox")),
            ("FLD-receipts", "Receipts", "FLD-inbox", None),
            ("FLD-clients", "Clients", "ROOT", None),
        ] {
            s.folders.push(FFolder {
                id: id.into(),
                name: name.into(),
                parent: parent.into(),
                well_known: wk,
            });
        }
        Arc::new(FakeGraph {
            state: Mutex::new(s),
        })
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Put a message in `folder` (well-known name or id); returns its id.
    pub fn add(&self, folder: &str, mut m: FMsg) -> String {
        let mut s = self.lock();
        let folder = s.folder_id(folder).expect("known folder");
        if m.id.is_empty() {
            m.id = s.new_id();
        }
        if m.conversation.is_empty() {
            m.conversation = format!("CONV-{}", m.id);
        }
        if m.imid.is_empty() {
            m.imid = format!("{}@mail.outlook.example", m.id.to_lowercase());
        }
        m.folder = folder.clone();
        let id = m.id.clone();
        s.messages.insert(id.clone(), m);
        s.change(&folder, &id);
        id
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.lock())
    }

    pub fn fail(&self, pattern: &str, status: u16, retry_after: Option<u64>, times: u32) {
        self.lock().failures.push_back(Failure {
            pattern: pattern.into(),
            status,
            retry_after,
            times,
        });
    }
}

impl State {
    fn new_id(&mut self) -> String {
        self.next += 1;
        // Immutable-id-like: base64url characters and padding.
        format!("AAMkADfake{:06}-Aa_=", self.next)
    }

    fn change(&mut self, folder: &str, id: &str) {
        self.seq += 1;
        let seq = self.seq;
        self.changes.push((seq, folder.to_string(), id.to_string()));
    }

    pub fn folder_id(&self, name_or_id: &str) -> Option<String> {
        self.folders
            .iter()
            .find(|f| f.id == name_or_id || f.well_known == Some(name_or_id))
            .map(|f| f.id.clone())
    }

    pub fn move_message(&mut self, id: &str, folder: &str) {
        let dest = self.folder_id(folder).expect("folder");
        let old = self.messages.get(id).expect("message").folder.clone();
        self.messages.get_mut(id).unwrap().folder = dest.clone();
        self.change(&old, id);
        self.change(&dest, id);
    }

    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut FMsg)) {
        let m = self.messages.get_mut(id).expect("message");
        f(m);
        let folder = m.folder.clone();
        self.change(&folder, id);
    }

    pub fn delete(&mut self, id: &str) {
        if let Some(m) = self.messages.remove(id) {
            self.change(&m.folder, id);
        }
    }

    pub fn add_folder(&mut self, id: &str, name: &str, parent: &str) {
        self.folders.push(FFolder {
            id: id.into(),
            name: name.into(),
            parent: parent.into(),
            well_known: None,
        });
    }

    /// Move a folder under another (deleting a folder moves it to Deleted Items).
    pub fn move_folder(&mut self, id: &str, new_parent: &str) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.parent = new_parent.into();
        }
    }

    pub fn remove_folder(&mut self, id: &str) {
        self.folders.retain(|f| f.id != id);
    }

    pub fn expire_delta_links(&mut self, folder: &str) {
        let f = self.folder_id(folder).expect("folder");
        self.expired.insert(f);
    }

    fn folder_json(&self, f: &FFolder) -> Value {
        let children = self.folders.iter().filter(|c| c.parent == f.id).count();
        let total = self.messages.values().filter(|m| m.folder == f.id).count();
        json!({
            "id": f.id, "displayName": f.name, "parentFolderId": f.parent,
            "childFolderCount": children, "totalItemCount": total, "unreadItemCount": 0,
        })
    }

    /// Hand out one page of a listing, storing the rest under a token.
    fn page(
        &mut self,
        entries: Vec<(String, bool)>,
        size: usize,
        delta_link: String,
        base: &str,
    ) -> Response {
        let (now, rest): (Vec<_>, Vec<_>) = if entries.len() > size {
            let mut e = entries;
            let rest = e.split_off(size);
            (e, rest)
        } else {
            (entries, vec![])
        };
        let value: Vec<Value> = now
            .iter()
            .map(|(id, removed)| match (removed, self.messages.get(id)) {
                (false, Some(m)) => m.light(),
                _ => json!({ "id": id, "@removed": { "reason": "deleted" } }),
            })
            .collect();
        if rest.is_empty() {
            return ok(json!({ "value": value, "@odata.deltaLink": delta_link }));
        }
        self.next += 1;
        let token = format!("snap{}", self.next);
        self.snapshots.insert(token.clone(), (rest, delta_link));
        ok(json!({ "value": value, "@odata.nextLink": format!("{base}?$skiptoken={token}") }))
    }

    fn delta(&mut self, folder: &str, q: &HashMap<String, String>, size: usize) -> Response {
        let base = format!("{GRAPH}/me/mailFolders/{folder}/messages/delta");
        if let Some(tok) = q.get("$skiptoken") {
            let Some((entries, link)) = self.snapshots.remove(tok) else {
                return graph_error(410, "SyncStateNotFound");
            };
            return self.page(entries, size, link, &base);
        }
        let start_seq = self.seq;
        let link = format!("{base}?$deltatoken={start_seq}");
        if let Some(tok) = q.get("$deltatoken") {
            if self.expired.contains(folder) {
                return graph_error(410, "SyncStateNotFound");
            }
            let since: u64 = tok.parse().unwrap_or(0);
            let mut ids: Vec<String> = Vec::new();
            for (seq, f, id) in &self.changes {
                if *seq > since && f == folder && !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
            let entries = ids
                .into_iter()
                .map(|id| {
                    let here = self.messages.get(&id).is_some_and(|m| m.folder == folder);
                    (id, !here)
                })
                .collect();
            return self.page(entries, size, link, &base);
        }
        // An initial round.
        self.expired.remove(folder);
        if q.contains_key("$orderby") && self.refuse_ordered_delta {
            return graph_error(400, "ErrorInvalidParameter");
        }
        let mut msgs: Vec<&FMsg> = self
            .messages
            .values()
            .filter(|m| m.folder == folder)
            .collect();
        if q.contains_key("$orderby") {
            msgs.sort_by_key(|m| std::cmp::Reverse(m.received_ms));
        }
        let entries = msgs.into_iter().map(|m| (m.id.clone(), false)).collect();
        self.page(entries, size, link, &base)
    }

    /// `/me/messages` or `/me/mailFolders/{id}/messages`: `$filter` on
    /// receivedDateTime, newest first, `$top`/`$skip`; full messages when
    /// `$select` asks for the body (as Graph returns what's selected).
    fn list(&mut self, folder: Option<&str>, q: &HashMap<String, String>) -> Response {
        let mut msgs: Vec<&FMsg> = self
            .messages
            .values()
            .filter(|m| folder.is_none_or(|f| m.folder == f))
            .collect();
        if let Some(filter) = q.get("$filter") {
            for part in filter.split(" and ") {
                let mut t = part.split_whitespace();
                let (Some(_), Some(op), Some(val)) = (t.next(), t.next(), t.next()) else {
                    continue;
                };
                let bound = crate::convert::parse_date_ms(val).unwrap_or(0);
                // Graph compares whole seconds.
                let secs = |ms: i64| ms.div_euclid(1000);
                msgs.retain(|m| match op {
                    "ge" => secs(m.received_ms) >= secs(bound),
                    "gt" => secs(m.received_ms) > secs(bound),
                    "lt" => secs(m.received_ms) < secs(bound),
                    "le" => secs(m.received_ms) <= secs(bound),
                    _ => true,
                });
            }
        }
        if let Some(search) = q.get("$search") {
            let words: Vec<String> = search
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() > 2 && !["AND", "OR"].contains(w))
                .map(|w| w.to_lowercase())
                .collect();
            msgs.retain(|m| {
                let hay = format!("{} {}", m.subject, m.body_html).to_lowercase();
                words.iter().any(|w| hay.contains(w))
            });
        }
        msgs.sort_by_key(|m| std::cmp::Reverse(m.received_ms));
        let count = msgs.len();
        let skip: usize = q.get("$skip").and_then(|s| s.parse().ok()).unwrap_or(0);
        let top: usize = q.get("$top").and_then(|s| s.parse().ok()).unwrap_or(10);
        let full = q
            .get("$select")
            .is_some_and(|s| s.split(',').any(|f| f == "body"));
        let page: Vec<Value> = msgs
            .iter()
            .skip(skip)
            .take(top)
            .map(|m| if full { m.full(false) } else { m.light() })
            .collect();
        let mut v = json!({ "value": page });
        if q.get("$count").map(String::as_str) == Some("true") {
            v["@odata.count"] = json!(count);
        }
        if skip + top < count {
            let base = match folder {
                Some(f) => format!("{GRAPH}/me/mailFolders/{f}/messages"),
                None => format!("{GRAPH}/me/messages"),
            };
            let mut next = url::Url::parse(&base).unwrap();
            {
                let mut qp = next.query_pairs_mut();
                for (k, val) in q {
                    if k != "$skip" {
                        qp.append_pair(k, val);
                    }
                }
                qp.append_pair("$skip", &(skip + top).to_string());
            }
            v["@odata.nextLink"] = json!(next.to_string());
        }
        ok(v)
    }

    fn new_draft(&mut self, v: &Value) -> FMsg {
        let id = self.new_id();
        let drafts = self.folder_id("drafts").unwrap();
        FMsg {
            id: id.clone(),
            conversation: format!("CONV-{id}"),
            folder: drafts,
            received_ms: 1_760_000_000_000 + self.next as i64,
            subject: v["subject"].as_str().unwrap_or("").into(),
            body_html: v["body"]["content"].as_str().unwrap_or("").into(),
            from: ("Sam Rivers".into(), ME.into()),
            to: from_rcpts(v.get("toRecipients")),
            cc: from_rcpts(v.get("ccRecipients")),
            bcc: from_rcpts(v.get("bccRecipients")),
            is_read: true,
            is_draft: true,
            imid: format!("{}@mail.outlook.example", id.to_lowercase()),
            ..FMsg::default()
        }
    }

    fn apply_patch(m: &mut FMsg, v: &Value) {
        if let Some(r) = v.get("isRead").and_then(Value::as_bool) {
            m.is_read = r;
        }
        if let Some(f) = v.get("flag") {
            m.flagged = f["flagStatus"] == "flagged";
        }
        if let Some(c) = v.get("categories").and_then(Value::as_array) {
            m.categories = c
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect();
        }
        if let Some(s) = v.get("subject").and_then(Value::as_str) {
            m.subject = s.into();
        }
        if let Some(b) = v.get("body") {
            m.body_html = b["content"].as_str().unwrap_or("").into();
        }
        if v.get("toRecipients").is_some() {
            m.to = from_rcpts(v.get("toRecipients"));
            m.cc = from_rcpts(v.get("ccRecipients"));
            m.bcc = from_rcpts(v.get("bccRecipients"));
        }
    }

    fn message_route(
        &mut self,
        method: Method,
        id: &str,
        rest: &[String],
        req: &Request,
        q: &HashMap<String, String>,
    ) -> Response {
        let exists = self.messages.contains_key(id);
        if !exists && !(method == Method::Post && rest.is_empty()) {
            if !id.starts_with("AAMk") {
                return graph_error(400, "ErrorInvalidIdMalformed");
            }
            return graph_error(404, "ErrorItemNotFound");
        }
        let body_json = || -> Value {
            req.body
                .as_deref()
                .and_then(|b| serde_json::from_slice(b).ok())
                .unwrap_or(Value::Null)
        };
        match (
            method,
            rest.iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice(),
        ) {
            (Method::Get, []) if q.get("$expand").is_some_and(|e| e.contains("eventMessage")) => {
                match self.meeting_events.get(id) {
                    Some(ev) => ok(json!({ "id": id, "event": { "id": ev } })),
                    None => ok(json!({ "id": id })),
                }
            }
            (Method::Get, []) => {
                let with_size = q
                    .get("$expand")
                    .is_some_and(|e| e.contains("singleValueExtendedProperties"));
                ok(self.messages[id].full(with_size))
            }
            (Method::Get, ["$value"]) => Response {
                status: 200,
                headers: vec![],
                body: self.messages[id].mime(),
            },
            (Method::Patch, []) => {
                let v = body_json();
                let m = self.messages.get_mut(id).unwrap();
                Self::apply_patch(m, &v);
                let (folder, out) = (m.folder.clone(), m.full(false));
                self.change(&folder, id);
                ok(out)
            }
            (Method::Delete, []) => {
                self.delete(id);
                status(204)
            }
            (Method::Post, ["move"]) => {
                let dest = body_json()["destinationId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let Some(dest) = self.folder_id(&dest) else {
                    return graph_error(404, "ErrorItemNotFound");
                };
                self.move_message(id, &dest);
                ok(self.messages[id].light())
            }
            (Method::Post, ["send"]) => {
                if !self.messages[id].is_draft {
                    return graph_error(404, "ErrorItemNotFound");
                }
                // Sending moves the draft to Sent Items under a new id.
                let mut m = self.messages.remove(id).unwrap();
                let drafts = m.folder.clone();
                self.change(&drafts, id);
                m.id = self.new_id();
                m.is_draft = false;
                m.folder = self.folder_id("sentitems").unwrap();
                let (sent_id, folder) = (m.id.clone(), m.folder.clone());
                self.messages.insert(sent_id.clone(), m);
                self.change(&folder, &sent_id);
                self.sent.push(sent_id);
                status(202)
            }
            (Method::Post, ["createReply"]) => {
                let parent = self.messages[id].clone();
                let mut d =
                    self.new_draft(&json!({ "subject": format!("RE: {}", parent.subject) }));
                d.conversation = parent.conversation.clone();
                d.to = vec![parent.from.clone()];
                d.headers = vec![
                    ("In-Reply-To".into(), format!("<{}>", parent.imid)),
                    ("References".into(), format!("<{}>", parent.imid)),
                ];
                let (did, folder, out) = (d.id.clone(), d.folder.clone(), d.light());
                self.messages.insert(did.clone(), d);
                self.change(&folder, &did);
                Response {
                    status: 201,
                    ..ok(out)
                }
            }
            (Method::Get, ["attachments"]) => {
                let v: Vec<Value> = self.messages[id]
                    .attachments
                    .iter()
                    .map(|a| json!({ "@odata.type": "#microsoft.graph.fileAttachment", "id": a.id, "name": a.name, "contentType": a.content_type, "size": a.bytes.len(), "isInline": a.inline }))
                    .collect();
                ok(json!({ "value": v }))
            }
            (Method::Get, ["attachments", aid]) => {
                match self.messages[id].attachments.iter().find(|a| a.id == *aid) {
                    Some(a) => ok(json!({
                        "@odata.type": "#microsoft.graph.fileAttachment",
                        "id": a.id, "name": a.name, "contentType": a.content_type, "size": a.bytes.len(),
                        "isInline": a.inline, "contentId": a.content_id,
                        "contentBytes": base64::engine::general_purpose::STANDARD.encode(&a.bytes),
                    })),
                    None => graph_error(404, "ErrorItemNotFound"),
                }
            }
            (Method::Get, ["attachments", aid, "$value"]) => {
                match self.messages[id].attachments.iter().find(|a| a.id == *aid) {
                    Some(a) => Response {
                        status: 200,
                        headers: vec![],
                        body: a.bytes.clone(),
                    },
                    None => graph_error(404, "ErrorItemNotFound"),
                }
            }
            (Method::Post, ["attachments"]) => {
                let v = body_json();
                self.next += 1;
                let aid = format!("ATT{}", self.next);
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(v["contentBytes"].as_str().unwrap_or(""))
                    .unwrap_or_default();
                self.messages.get_mut(id).unwrap().attachments.push(FAtt {
                    id: aid.clone(),
                    name: v["name"].as_str().unwrap_or("").into(),
                    content_type: v["contentType"].as_str().unwrap_or("").into(),
                    bytes,
                    inline: v["isInline"].as_bool().unwrap_or(false),
                    content_id: v["contentId"].as_str().map(str::to_string),
                });
                Response {
                    status: 201,
                    ..ok(json!({ "id": aid, "name": v["name"] }))
                }
            }
            (Method::Post, ["attachments", "createUploadSession"]) => {
                let v = body_json();
                self.next += 1;
                let session = format!("s{}", self.next);
                let item = &v["AttachmentItem"];
                self.uploads.insert(
                    session.clone(),
                    (
                        id.to_string(),
                        item["name"].as_str().unwrap_or("").to_string(),
                        Vec::new(),
                    ),
                );
                self.upload_meta.insert(
                    session.clone(),
                    (
                        item["contentId"].as_str().map(str::to_string),
                        item["contentType"]
                            .as_str()
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                    ),
                );
                ok(json!({ "uploadUrl": format!("{UPLOAD_HOST}{session}") }))
            }
            (Method::Delete, ["attachments", aid]) => {
                self.messages
                    .get_mut(id)
                    .unwrap()
                    .attachments
                    .retain(|a| a.id != *aid);
                status(204)
            }
            _ => graph_error(400, "ErrorInvalidRequest"),
        }
    }

    fn upload(&mut self, req: &Request) -> Response {
        let session = req.url.trim_start_matches(UPLOAD_HOST).to_string();
        let range = req.header("Content-Range").unwrap_or("").to_string();
        let Some((mid, name, buf)) = self.uploads.get_mut(&session) else {
            return status(404);
        };
        buf.extend_from_slice(req.body.as_deref().unwrap_or(&[]));
        let total: usize = range
            .rsplit('/')
            .next()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        if buf.len() < total {
            return status(200);
        }
        let (mid, name, bytes) = (mid.clone(), name.clone(), std::mem::take(buf));
        self.uploads.remove(&session);
        let (content_id, content_type) = self
            .upload_meta
            .remove(&session)
            .unwrap_or((None, "application/octet-stream".into()));
        self.next += 1;
        let aid = format!("ATT{}", self.next);
        if let Some(m) = self.messages.get_mut(&mid) {
            m.attachments.push(FAtt {
                id: aid.clone(),
                name,
                content_type,
                bytes,
                inline: content_id.is_some(),
                content_id,
            });
        }
        Response {
            status: 201,
            headers: vec![(
                "Location".into(),
                format!("{GRAPH}/me/messages('{mid}')/attachments('{aid}')"),
            )],
            body: vec![],
        }
    }

    fn token(&mut self, req: &Request) -> Response {
        self.token_calls += 1;
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(req.body.as_deref().unwrap_or(&[]))
                .into_owned()
                .collect();
        let bad = |code: u64, error: &str| {
            Response {
            status: 400,
            headers: vec![],
            body: serde_json::to_vec(&json!({
                "error": error,
                "error_description": format!("AADSTS{code}: something about sam@outlook.example. Trace ID: x"),
                "error_codes": [code],
            }))
            .unwrap(),
        }
        };
        let issue = |s: &mut State, id_token: Option<String>| {
            s.next += 1;
            let at = format!("at-{}", s.next);
            let rt = format!("rt-{}", s.next);
            s.access_tokens.insert(at.clone());
            s.refresh_token = rt.clone();
            let mut v = json!({
                "access_token": at, "refresh_token": rt, "expires_in": 3600,
                "scope": "https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/User.Read openid profile email",
            });
            if let Some(t) = id_token {
                v["id_token"] = json!(t);
            }
            ok(v)
        };
        match form.get("grant_type").map(String::as_str) {
            Some("refresh_token") => {
                if self.reject_refresh || form.get("refresh_token") != Some(&self.refresh_token) {
                    return bad(70008, "invalid_grant");
                }
                issue(self, None)
            }
            Some("authorization_code") => {
                if form.get("code").map(String::as_str) != Some("good-code")
                    || form.get("code_verifier").is_none()
                {
                    return bad(70000, "invalid_grant");
                }
                let claims = json!({ "preferred_username": ME, "name": "Sam Rivers", "tid": "9188040d-6c67-4c5b-b112-36a304b66dad", "oid": "00000000-0000-0000-aaaa-000000000001" });
                let payload =
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
                issue(self, Some(format!("e30.{payload}.sig")))
            }
            _ => bad(900144, "invalid_request"),
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        if req.url.starts_with(TOKEN_URL) {
            return self.token(&req);
        }
        if req.url.starts_with(UPLOAD_HOST) {
            if req.header("Authorization").is_some() {
                return status(400);
            }
            return self.upload(&req);
        }
        let url = url::Url::parse(&req.url).expect("valid url");
        let path = url.path().trim_start_matches("/v1.0").to_string();
        self.requests
            .push(format!("{} {}", req.method.as_str(), path));
        let bearer = req
            .header("Authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
            .unwrap_or("");
        if !self.access_tokens.contains(bearer) {
            return graph_error(401, "InvalidAuthenticationToken");
        }
        if !req
            .header("Prefer")
            .is_some_and(|p| p.contains("IdType=\"ImmutableId\""))
        {
            self.missing_immutable += 1;
        }
        if let Some(i) = self
            .failures
            .iter()
            .position(|f| match f.pattern.strip_suffix('$') {
                // `…$`: this exact path only.
                Some(exact) => path == exact,
                None => path.contains(&f.pattern),
            })
        {
            let f = &mut self.failures[i];
            f.times -= 1;
            let (st, ra) = (f.status, f.retry_after);
            if f.times == 0 {
                self.failures.remove(i);
            }
            let mut r = graph_error(
                st,
                if st == 429 {
                    "TooManyRequests"
                } else {
                    "ServiceUnavailable"
                },
            );
            if let Some(s) = ra {
                r.headers.push(("Retry-After".into(), s.to_string()));
            }
            return r;
        }
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let size: usize = req
            .header("Prefer")
            .and_then(|p| p.split("odata.maxpagesize=").nth(1))
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let segs: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(pct_decode)
            .collect();
        let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
        match (req.method, segs.as_slice()) {
            (Method::Get, ["me"]) => {
                if q.get("$select")
                    .is_some_and(|s| s.contains("proxyAddresses"))
                {
                    ok(
                        json!({ "proxyAddresses": ["SMTP:sam@outlook.example", "smtp:sam.rivers@alias.example"] }),
                    )
                } else {
                    ok(
                        json!({ "displayName": "Sam Rivers", "mail": null, "userPrincipalName": ME }),
                    )
                }
            }
            (Method::Get, ["me", "mailFolders"]) => {
                let v: Vec<Value> = self
                    .folders
                    .iter()
                    .filter(|f| f.parent == "ROOT")
                    .map(|f| self.folder_json(f))
                    .collect();
                ok(json!({ "value": v }))
            }
            (Method::Post, ["me", "mailFolders"]) => {
                let name = serde_json::from_slice::<Value>(req.body.as_deref().unwrap_or(b"{}"))
                    .unwrap_or_default()["displayName"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                self.next += 1;
                let id = format!("FLD-new{}", self.next);
                self.add_folder(&id, &name, "ROOT");
                Response {
                    status: 201,
                    ..ok(json!({ "id": id, "displayName": name }))
                }
            }
            (Method::Get, ["me", "mailFolders", f]) => match self
                .folders
                .iter()
                .find(|x| x.id == *f || x.well_known == Some(*f))
            {
                Some(x) => ok(self.folder_json(x)),
                None => graph_error(404, "ErrorItemNotFound"),
            },
            (Method::Get, ["me", "mailFolders", f, "childFolders"]) => {
                let v: Vec<Value> = self
                    .folders
                    .iter()
                    .filter(|c| c.parent == *f)
                    .map(|c| self.folder_json(c))
                    .collect();
                ok(json!({ "value": v }))
            }
            (Method::Get, ["me", "mailFolders", f, "messages", "delta"]) => {
                let Some(fid) = self.folder_id(f) else {
                    return graph_error(404, "ErrorItemNotFound");
                };
                self.delta(&fid, &q, size)
            }
            (Method::Get, ["me", "outlook", "masterCategories"]) => {
                let v: Vec<Value> = self
                    .categories
                    .iter()
                    .map(|c| json!({ "displayName": c, "color": "preset0" }))
                    .collect();
                ok(json!({ "value": v }))
            }
            (Method::Get, ["me", "photo", "$value"]) => match &self.photo {
                Some(p) => Response {
                    status: 200,
                    headers: vec![],
                    body: p.clone(),
                },
                None => graph_error(404, "ImageNotFound"),
            },
            (Method::Get, ["me", "messages"]) => self.list(None, &q),
            (Method::Get, ["me", "mailFolders", f, "messages"]) => {
                let Some(fid) = self.folder_id(f) else {
                    return graph_error(404, "ErrorItemNotFound");
                };
                self.list(Some(&fid), &q)
            }
            (Method::Post, ["me", "messages"]) => {
                let is_mime = req.header("Content-Type") == Some("text/plain");
                let d = if is_mime {
                    let raw = base64::engine::general_purpose::STANDARD
                        .decode(req.body.as_deref().unwrap_or(&[]))
                        .unwrap_or_default();
                    let headers = crate::convert::raw_headers(&raw);
                    let get = |k: &str| {
                        headers
                            .iter()
                            .find(|(h, _)| h.eq_ignore_ascii_case(k))
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default()
                    };
                    let split = |v: String| -> Vec<(String, String)> {
                        v.split(',')
                            .filter(|s| !s.trim().is_empty())
                            .map(|s| {
                                let a = s.trim();
                                let addr = a
                                    .rsplit('<')
                                    .next()
                                    .unwrap_or(a)
                                    .trim_end_matches('>')
                                    .trim()
                                    .to_string();
                                (String::new(), addr)
                            })
                            .collect()
                    };
                    let mut d = self.new_draft(&json!({ "subject": get("Subject") }));
                    d.to = split(get("To"));
                    d.bcc = split(get("Bcc"));
                    let irt = get("In-Reply-To");
                    let irt = irt.trim_matches(|c| c == '<' || c == '>');
                    if let Some(parent) = self
                        .messages
                        .values()
                        .find(|m| !irt.is_empty() && m.imid == irt)
                    {
                        d.conversation = parent.conversation.clone();
                    }
                    d
                } else {
                    let v: Value = serde_json::from_slice(req.body.as_deref().unwrap_or(b"{}"))
                        .unwrap_or_default();
                    self.new_draft(&v)
                };
                let (id, folder, out) = (d.id.clone(), d.folder.clone(), d.light());
                self.messages.insert(id.clone(), d);
                self.change(&folder, &id);
                Response {
                    status: 201,
                    ..ok(out)
                }
            }
            (Method::Post, ["me", "events", ev, action]) => {
                let v: Value = serde_json::from_slice(req.body.as_deref().unwrap_or(b"{}"))
                    .unwrap_or_default();
                self.meeting_actions
                    .push((ev.to_string(), action.to_string(), v));
                status(202)
            }
            (m, ["me", "messages", id, rest @ ..]) => {
                let id = id.to_string();
                let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
                self.message_route(m, &id, &rest, &req, &q)
            }
            _ => graph_error(400, "ErrorInvalidRequest"),
        }
    }
}

#[async_trait]
impl Transport for FakeGraph {
    async fn send(&self, request: Request) -> Result<Response> {
        Ok(self.lock().handle(request))
    }
}
