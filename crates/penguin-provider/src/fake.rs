//! An in-memory provider and backend: the reference implementation of the
//! seam for tests (the app's dispatch tests, the conformance suite's own
//! tests) and a worked example for new providers. Enabled by the `fake`
//! feature. Its "server" is a map of messages with Gmail-style labels.
//!
//! Every call is recorded (`calls()`), and failures can be scripted per
//! method (`fail_next`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use penguin_core::query::ParsedQuery;
use penguin_core::{
    Account, AccountProvider, Address, AttachmentMeta, Label, Message, SearchHit, SentRef, Store,
    SyncPhase, SyncStatus,
};

use crate::compose::{build_rfc822_draft_with_attachments, AttachmentBytes, Draft};
use crate::provider::{
    Backend, DraftRef, LabelUpdate, MailProvider, MessageMetadata, OpenedDraft, ServerSearch,
    SyncHandle, SyncObserver, SyncTask,
};
use crate::window::{window_start_ms, WindowPolicy};
use crate::{Error, Result};

/// The fake server's state for one account.
#[derive(Default)]
pub struct FakeServer {
    pub messages: BTreeMap<String, Message>,
    /// draft id → message id of its current version.
    pub drafts: BTreeMap<String, String>,
    pub attachments: HashMap<(String, String), Vec<u8>>,
    pub labels: Vec<Label>,
    /// Scripted failures: (method name, error), consumed in order by the
    /// first call of that method.
    pub fail_next: VecDeque<(String, Error)>,
    pub calls: Vec<String>,
    /// Every MIME message that left, in order (send_raw, send_draft,
    /// send_saved_draft), exactly as the provider would hand it over.
    pub outgoing: Vec<Vec<u8>>,
    /// draft id → the MIME of its current version (what a send later sends).
    pub draft_mime: BTreeMap<String, Vec<u8>>,
    next: u64,
}

impl FakeServer {
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn call(&mut self, method: &str, detail: &str) -> Result<()> {
        self.calls
            .push(format!("{method} {detail}").trim_end().to_string());
        if let Some(i) = self.fail_next.iter().position(|(m, _)| m == method) {
            let (_, e) = self.fail_next.remove(i).expect("position is valid");
            return Err(e);
        }
        Ok(())
    }
}

/// A [`MailProvider`] over a [`FakeServer`].
#[derive(Clone)]
pub struct FakeProvider {
    account_id: String,
    provider: AccountProvider,
    store: Store,
    pub server: Arc<Mutex<FakeServer>>,
}

impl FakeProvider {
    pub fn new(account_id: &str, provider: AccountProvider, store: Store) -> FakeProvider {
        FakeProvider {
            account_id: account_id.to_string(),
            provider,
            store,
            server: Arc::default(),
        }
    }

    fn server(&self) -> std::sync::MutexGuard<'_, FakeServer> {
        self.server.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Put a message on the server (not in the store).
    pub fn seed(&self, message: Message) {
        self.server().messages.insert(message.id.clone(), message);
    }

    pub fn seed_attachment(&self, message_id: &str, attachment_id: &str, bytes: &[u8]) {
        self.server().attachments.insert(
            (message_id.to_string(), attachment_id.to_string()),
            bytes.to_vec(),
        );
    }

    /// Make the next call of `method` (e.g. "modify_thread") fail.
    pub fn fail_next(&self, method: &str, error: Error) {
        self.server()
            .fail_next
            .push_back((method.to_string(), error));
    }

    pub fn calls(&self) -> Vec<String> {
        self.server().calls.clone()
    }

    /// Every MIME message sent so far, oldest first.
    pub fn outgoing(&self) -> Vec<Vec<u8>> {
        self.server().outgoing.clone()
    }

    /// In-Reply-To and References for a reply draft, from the stored parent
    /// (what Gmail's drafts code does).
    async fn reply_headers(&self, draft: &Draft) -> Result<(Option<String>, Vec<String>)> {
        let Some(parent) = draft.reply_to_message_id.clone() else {
            return Ok((None, Vec::new()));
        };
        let acct = self.account_id.clone();
        let parent = self.db(move |s| s.get_message(&acct, &parent)).await?;
        Ok(parent
            .map(|p| (p.message_id_header.clone(), p.references.clone()))
            .unwrap_or_default())
    }

    pub fn message(&self, id: &str) -> Option<Message> {
        self.server().messages.get(id).cloned()
    }

    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> penguin_core::Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|e| Error::Other(format!("store task failed: {e}")))?
            .map_err(Error::from)
    }

    fn draft_message(&self, id: &str, thread_id: &str, draft: &Draft, from: &Address) -> Message {
        Message {
            account_id: self.account_id.clone(),
            id: id.to_string(),
            thread_id: thread_id.to_string(),
            date: now_ms(),
            from: from.clone(),
            to: draft.to.clone(),
            cc: draft.cc.clone(),
            bcc: draft.bcc.clone(),
            reply_to: vec![],
            subject: draft.subject.clone(),
            snippet: draft.body_text.chars().take(100).collect(),
            body_text: draft.body_text.clone(),
            body_html: draft.body_html.clone(),
            label_ids: vec!["DRAFT".into()],
            attachments: vec![],
            message_id_header: Some(format!("<{id}@fake.example>")),
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn apply(labels: &mut Vec<String>, add: &[String], remove: &[String]) {
    labels.retain(|l| !remove.contains(l));
    for a in add {
        if !labels.contains(a) {
            labels.push(a.clone());
        }
    }
}

#[async_trait]
impl MailProvider for FakeProvider {
    fn account_id(&self) -> &str {
        &self.account_id
    }

    fn provider(&self) -> AccountProvider {
        self.provider
    }

    async fn fetch_messages(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        let mut s = self.server();
        ids.iter()
            .map(|id| {
                let r = s
                    .call("fetch_messages", id)
                    .map(|()| s.messages.get(id).cloned());
                (id.clone(), r)
            })
            .collect()
    }

    async fn fetch_pending_bodies(&self, ids: &[String]) -> Result<Vec<String>> {
        let (acct, want) = (self.account_id.clone(), ids.to_vec());
        let pending: Vec<String> = self
            .db(move |s| {
                let mut out = Vec::new();
                for id in want {
                    if s.is_body_pending(&acct, &id)? == Some(true) {
                        out.push(id);
                    }
                }
                Ok(out)
            })
            .await?;
        let mut messages = Vec::new();
        for (_, r) in self.fetch_messages(&pending).await {
            messages.extend(r?);
        }
        let threads: Vec<String> = messages.iter().map(|m| m.thread_id.clone()).collect();
        self.db(move |s| s.upsert_messages(&messages)).await?;
        Ok(threads)
    }

    async fn get_attachment(
        &self,
        message_id: &str,
        attachment: &AttachmentMeta,
    ) -> Result<Vec<u8>> {
        let mut s = self.server();
        s.call("get_attachment", message_id)?;
        s.attachments
            .get(&(message_id.to_string(), attachment.id.clone()))
            .cloned()
            .ok_or_else(|| Error::NotFound("attachment not found".into()))
    }

    async fn get_message_metadata(&self, message_id: &str) -> Result<Option<MessageMetadata>> {
        let mut s = self.server();
        s.call("get_message_metadata", message_id)?;
        Ok(s.messages.get(message_id).map(|m| MessageMetadata {
            headers: vec![
                ("From".into(), m.from.email.clone()),
                ("Subject".into(), m.subject.clone()),
                (
                    "Message-ID".into(),
                    m.message_id_header.clone().unwrap_or_default(),
                ),
            ],
            size_estimate: Some(m.body_text.len() as u64),
        }))
    }

    async fn get_message_raw(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        let mut s = self.server();
        s.call("get_message_raw", message_id)?;
        Ok(s.messages.get(message_id).map(|m| {
            format!(
                "Message-ID: {}\r\nFrom: {}\r\nSubject: {}\r\n\r\n{}",
                m.message_id_header.clone().unwrap_or_default(),
                m.from.email,
                m.subject,
                m.body_text
            )
            .into_bytes()
        }))
    }

    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let mut s = self.server();
        s.call(
            "modify_thread",
            &format!("{thread_id} +{} -{}", add.join(","), remove.join(",")),
        )?;
        let mut found = false;
        for m in s.messages.values_mut().filter(|m| m.thread_id == thread_id) {
            apply(&mut m.label_ids, add, remove);
            found = true;
        }
        if !found {
            return Err(Error::NotFound(format!("thread {thread_id}")));
        }
        Ok(())
    }

    async fn trash_thread(&self, thread_id: &str) -> Result<()> {
        self.server().call("trash_thread", thread_id)?;
        self.modify_thread(thread_id, &["TRASH".into()], &["INBOX".into()])
            .await
    }

    async fn untrash_thread(&self, thread_id: &str) -> Result<()> {
        self.server().call("untrash_thread", thread_id)?;
        self.modify_thread(thread_id, &["INBOX".into()], &["TRASH".into()])
            .await
    }

    async fn update_label(&self, label_id: &str, update: &LabelUpdate) -> Result<Label> {
        let mut s = self.server();
        s.call("update_label", label_id)?;
        let label = s
            .labels
            .iter_mut()
            .find(|l| l.id == label_id)
            .ok_or_else(|| Error::NotFound(format!("label {label_id}")))?;
        if let Some(n) = &update.name {
            label.name = n.clone();
        }
        if let Some(c) = &update.color {
            label.color = c.as_ref().map(|c| c.background.clone());
        }
        if let Some(h) = update.hidden {
            label.hidden = h;
        }
        Ok(label.clone())
    }

    async fn delete_label(&self, label_id: &str) -> Result<()> {
        let mut s = self.server();
        s.call("delete_label", label_id)?;
        s.labels.retain(|l| l.id != label_id);
        Ok(())
    }

    async fn ensure_label(&self, name: &str) -> Result<Label> {
        let mut s = self.server();
        s.call("ensure_label", name)?;
        if let Some(l) = s.labels.iter().find(|l| l.kind == "user" && l.name == name) {
            return Ok(l.clone());
        }
        let label = Label {
            account_id: self.account_id.clone(),
            id: crate::ids::label_id_for_name("f:", name),
            name: name.to_string(),
            kind: "user".into(),
            color: None,
            unread_count: None,
            hidden: false,
        };
        s.labels.push(label.clone());
        Ok(label)
    }

    async fn send_raw(&self, raw: &[u8], thread_id: Option<&str>) -> Result<SentRef> {
        let mut s = self.server();
        s.call("send_raw", &format!("{} bytes", raw.len()))?;
        s.outgoing.push(raw.to_vec());
        let id = s.id("sent-");
        let thread = thread_id.map(str::to_string).unwrap_or_else(|| s.id("t-"));
        let mut m = self.draft_message(&id, &thread, &empty_draft(&self.account_id), &me());
        m.label_ids = vec!["SENT".into()];
        s.messages.insert(id.clone(), m);
        Ok(SentRef {
            message_id: id,
            thread_id: thread,
        })
    }

    async fn save_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: Option<&str>,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        let (in_reply_to, references) = self.reply_headers(draft).await?;
        let raw = build_rfc822_draft_with_attachments(
            draft,
            from,
            in_reply_to.as_deref(),
            &references,
            attachments,
        )?;
        let (r, local, old) = {
            let mut s = self.server();
            s.call("save_draft", draft_id.unwrap_or("new"))?;
            // A draft gone from the server is recreated.
            let draft_id = match draft_id.filter(|d| s.drafts.contains_key(*d)) {
                Some(d) => d.to_string(),
                None => s.id("d-"),
            };
            let message_id = s.id("dm-");
            let thread = draft
                .reply_to_thread_id
                .clone()
                .unwrap_or_else(|| format!("t-{draft_id}"));
            let old = s.drafts.insert(draft_id.clone(), message_id.clone());
            if let Some(old) = &old {
                s.messages.remove(old);
            }
            let mut m = self.draft_message(&message_id, &thread, draft, from);
            // Like Gmail, the saved draft's attachments get new ids on the
            // new draft message; inline images keep their Content-ID.
            let parts: Vec<(Option<&str>, &str)> = attachments
                .iter()
                .map(|a| (a.content_id.as_deref(), a.mime_type.as_str()))
                .collect();
            let plan = crate::inline::plan(draft.body_html.as_deref(), &parts);
            for (a, inline) in attachments.iter().zip(plan.inline) {
                let id = s.id("da-");
                s.attachments
                    .insert((message_id.clone(), id.clone()), a.bytes.clone());
                m.attachments.push(AttachmentMeta {
                    id,
                    filename: a.filename.clone(),
                    mime_type: a.mime_type.clone(),
                    size: a.bytes.len() as u64,
                    content_id: a.content_id.clone().filter(|_| inline),
                    inline,
                });
            }
            let refs = crate::inline::saved_refs(&message_id, &m.attachments, attachments);
            s.messages.insert(message_id.clone(), m.clone());
            s.draft_mime.insert(draft_id.clone(), raw);
            let r = DraftRef {
                draft_id,
                message_id,
                thread_id: thread,
                attachments: refs,
            };
            (r, m, old)
        };
        let (acct, rr) = (self.account_id.clone(), r.clone());
        self.db(move |s| {
            if let Some(old) = old {
                s.delete_messages(&acct, &[old])?;
            }
            s.upsert_messages(std::slice::from_ref(&local))?;
            s.set_draft(&acct, &rr.draft_id, &rr.message_id)
        })
        .await?;
        Ok(r)
    }

    async fn send_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: &str,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        let (in_reply_to, references) = self.reply_headers(draft).await?;
        let raw = crate::compose::build_rfc822_with_attachments(
            draft,
            from,
            in_reply_to.as_deref(),
            &references,
            attachments,
        )?;
        // drafts.send with the final content: the draft is updated, then sent.
        self.server().draft_mime.insert(draft_id.to_string(), raw);
        let sent = self.send_saved_draft(draft_id).await?;
        Ok(DraftRef {
            draft_id: draft_id.to_string(),
            message_id: sent.message_id,
            thread_id: sent.thread_id,
            attachments: vec![],
        })
    }

    async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef> {
        let sent = {
            let mut s = self.server();
            s.call("send_saved_draft", draft_id)?;
            let mid = s
                .drafts
                .remove(draft_id)
                .ok_or_else(|| Error::NotFound(format!("draft {draft_id}")))?;
            if let Some(raw) = s.draft_mime.remove(draft_id) {
                s.outgoing.push(raw);
            }
            let mut m = s
                .messages
                .remove(&mid)
                .ok_or_else(|| Error::NotFound(format!("draft message {mid}")))?;
            let id = s.id("sent-");
            m.id = id.clone();
            m.label_ids = vec!["SENT".into()];
            let r = SentRef {
                message_id: id.clone(),
                thread_id: m.thread_id.clone(),
            };
            s.messages.insert(id, m);
            r
        };
        let (acct, d) = (self.account_id.clone(), draft_id.to_string());
        self.db(move |s| {
            if let Some(mid) = s.message_for_draft(&acct, &d)? {
                s.delete_messages(&acct, &[mid])?;
            }
            s.remove_draft(&acct, &d)
        })
        .await?;
        Ok(sent)
    }

    async fn delete_draft(&self, draft_id: &str) -> Result<Option<String>> {
        {
            let mut s = self.server();
            s.call("delete_draft", draft_id)?;
            if let Some(mid) = s.drafts.remove(draft_id) {
                s.messages.remove(&mid);
            }
        }
        let (acct, d) = (self.account_id.clone(), draft_id.to_string());
        self.db(move |s| {
            let mut thread = None;
            if let Some(mid) = s.message_for_draft(&acct, &d)? {
                thread = s.get_message(&acct, &mid)?.map(|m| m.thread_id);
                s.delete_messages(&acct, &[mid])?;
            }
            s.remove_draft(&acct, &d)?;
            Ok(thread)
        })
        .await
    }

    async fn open_draft(
        &self,
        draft_id: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<Option<OpenedDraft>> {
        let (draft_id, message_id) = {
            let mut s = self.server();
            s.call("open_draft", draft_id.or(message_id).unwrap_or(""))?;
            match (draft_id, message_id) {
                (Some(d), _) => match s.drafts.get(d) {
                    Some(m) => (d.to_string(), m.clone()),
                    None => return Ok(None),
                },
                (None, Some(m)) => match s.drafts.iter().find(|(_, v)| v.as_str() == m) {
                    Some((d, _)) => (d.clone(), m.to_string()),
                    None => return Ok(None),
                },
                (None, None) => {
                    return Err(Error::InvalidInput(
                        "open_draft needs a draftId or a messageId".into(),
                    ))
                }
            }
        };
        let Some(m) = self.message(&message_id) else {
            return Ok(None);
        };
        // Like the real providers: the HTML through the editor's allowlist
        // with the inline images it shows, which come back as refs too.
        let (body_html, attachments) = crate::inline::reopen(&m);
        Ok(Some(OpenedDraft {
            draft_id,
            message_id: m.id.clone(),
            thread_id: m.thread_id.clone(),
            draft: Draft {
                request_read_receipt: None,
                account_id: self.account_id.clone(),
                to: m.to.clone(),
                cc: m.cc.clone(),
                bcc: m.bcc.clone(),
                subject: m.subject.clone(),
                body_text: m.body_text.clone(),
                body_html,
                reply_to_thread_id: None,
                reply_to_message_id: None,
                attachments,
            },
        }))
    }

    async fn server_search(&self, query: &ParsedQuery, fetch_cap: usize) -> Result<ServerSearch> {
        // Subject contains every free-text word; other operators are ignored.
        let words: Vec<String> = query
            .text_terms()
            .map(|(t, _, _)| t.to_lowercase())
            .collect();
        let matches: Vec<Message> = {
            let mut s = self.server();
            s.call("server_search", "")?;
            s.messages
                .values()
                .filter(|m| {
                    let subject = m.subject.to_lowercase();
                    !words.is_empty() && words.iter().all(|w| subject.contains(w.as_str()))
                })
                .cloned()
                .collect()
        };
        let ids: Vec<String> = matches.iter().map(|m| m.id.clone()).collect();
        let (acct, probe) = (self.account_id.clone(), ids.clone());
        let known = self.db(move |s| s.known_message_ids(&acct, &probe)).await?;
        let unknown: Vec<Message> = matches
            .iter()
            .filter(|m| !known.contains(&m.id))
            .take(fetch_cap)
            .cloned()
            .collect();
        let fetched = unknown.len() as u32;
        self.db(move |s| s.insert_header_messages(&unknown)).await?;
        let hits = matches
            .iter()
            .map(|m| SearchHit {
                account_id: m.account_id.clone(),
                thread_id: m.thread_id.clone(),
                message_id: m.id.clone(),
                subject: m.subject.clone(),
                from: m.from.clone(),
                date: m.date,
                snippet_html: String::new(),
                match_count: 1,
                label_ids: m.label_ids.clone(),
                has_attachments: false,
                unread: m.is_unread(),
                score: 0.0,
                matched_by: Vec::new(),
                passage: None,
            })
            .collect();
        Ok(ServerSearch {
            hits,
            fetched,
            estimate: matches.len() as u64,
        })
    }

    async fn window_estimate(&self, months: u32, now_ms: i64) -> Result<u64> {
        let start = window_start_ms(now_ms, months);
        let mut s = self.server();
        s.call("window_estimate", &months.to_string())?;
        Ok(s.messages.values().filter(|m| m.date >= start).count() as u64)
    }
}

fn empty_draft(account_id: &str) -> Draft {
    Draft {
        request_read_receipt: None,
        account_id: account_id.to_string(),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        subject: String::new(),
        body_text: String::new(),
        body_html: None,
        reply_to_thread_id: None,
        reply_to_message_id: None,
        attachments: vec![],
    }
}

fn me() -> Address {
    Address {
        name: None,
        email: "me@fake.example".into(),
    }
}

/// A sync task that only records pokes.
pub struct FakeTask {
    pub pokes: Mutex<u32>,
    stopped: AtomicBool,
}

impl SyncTask for FakeTask {
    fn poke(&self) {
        *self.pokes.lock().unwrap_or_else(|p| p.into_inner()) += 1;
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
    fn is_running(&self) -> bool {
        !self.stopped.load(Ordering::SeqCst)
    }
}

/// A [`Backend`] of [`FakeProvider`]s. Sync "runs" by reporting Idle
/// through the observer; credentials are a set of account ids.
pub struct FakeBackend {
    provider: AccountProvider,
    store: Store,
    observer: Arc<dyn SyncObserver>,
    pub credentials: Mutex<Vec<String>>,
    pub clients: Mutex<HashMap<String, FakeProvider>>,
    pub tasks: Mutex<HashMap<String, Arc<FakeTask>>>,
    pub statuses: Mutex<HashMap<String, SyncStatus>>,
    pub policy: Mutex<Option<WindowPolicy>>,
    pub signed_out: Mutex<Vec<String>>,
}

impl FakeBackend {
    pub fn new(
        provider: AccountProvider,
        store: Store,
        observer: Arc<dyn SyncObserver>,
    ) -> FakeBackend {
        FakeBackend {
            provider,
            store,
            observer,
            credentials: Mutex::default(),
            clients: Mutex::default(),
            tasks: Mutex::default(),
            statuses: Mutex::default(),
            policy: Mutex::default(),
            signed_out: Mutex::default(),
        }
    }

    /// The client `client()` hands out for `account_id` (created on demand).
    pub fn fake_client(&self, account_id: &str) -> FakeProvider {
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(account_id.to_string())
            .or_insert_with(|| FakeProvider::new(account_id, self.provider, self.store.clone()))
            .clone()
    }

    fn report(&self, account_id: &str, phase: SyncPhase) {
        let status = SyncStatus {
            indexed: self.store.count_messages(Some(account_id)).unwrap_or(0),
            last_synced_at: Some(now_ms()),
            ..SyncStatus::new(account_id, phase)
        };
        self.statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account_id.to_string(), status.clone());
        self.observer.status(status);
    }
}

#[async_trait]
impl Backend for FakeBackend {
    fn provider(&self) -> AccountProvider {
        self.provider
    }

    fn client(&self, account: &Account) -> Result<Arc<dyn MailProvider>> {
        if account.provider != self.provider {
            return Err(Error::Other(format!(
                "{} is not a {:?} account",
                account.id, self.provider
            )));
        }
        Ok(Arc::new(self.fake_client(&account.id)))
    }

    fn has_credentials(&self, account: &Account) -> bool {
        self.credentials
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&account.id)
    }

    async fn sign_out(&self, account: &Account) -> Result<()> {
        self.credentials
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|a| a != &account.id);
        self.signed_out
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(account.id.clone());
        Ok(())
    }

    fn start_sync(&self, account: &Account) -> SyncHandle {
        let task = {
            let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
            match tasks.get(&account.id).filter(|t| t.is_running()) {
                Some(t) => return SyncHandle::new(t.clone()),
                None => {
                    let t = Arc::new(FakeTask {
                        pokes: Mutex::new(0),
                        stopped: AtomicBool::new(false),
                    });
                    tasks.insert(account.id.clone(), t.clone());
                    t
                }
            }
        };
        self.report(&account.id, SyncPhase::Idle);
        SyncHandle::new(task)
    }

    fn retry_sync(&self, account: &Account) -> SyncHandle {
        self.report(&account.id, SyncPhase::Idle);
        let handle = self.start_sync(account);
        handle.poke();
        handle
    }

    fn sync_status(&self, account_id: &str) -> Option<SyncStatus> {
        self.statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account_id)
            .cloned()
    }

    fn set_window_policy(&self, policy: WindowPolicy) {
        *self.policy.lock().unwrap_or_else(|p| p.into_inner()) = Some(policy);
    }
}

/// An observer that records what it's told.
#[derive(Default)]
pub struct RecordingObserver {
    pub statuses: Mutex<Vec<SyncStatus>>,
    pub changed: Mutex<Vec<(String, Vec<String>)>>,
    pub added: Mutex<Vec<(String, Vec<String>)>>,
}

impl SyncObserver for RecordingObserver {
    fn status(&self, status: SyncStatus) {
        self.statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(status);
    }
    fn mail_changed(&self, account_id: &str, thread_ids: Vec<String>) {
        self.changed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((account_id.to_string(), thread_ids));
    }
    fn messages_added(&self, account_id: &str, message_ids: Vec<String>) {
        self.added
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((account_id.to_string(), message_ids));
    }
}
