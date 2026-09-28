//! Gmail drafts: create / update / delete / send, plus the local mirror that
//! makes a saved draft show up in the Drafts view immediately.
//!
//! Gmail gives a draft a NEW message id on every update while the draft id
//! stays fixed, so the store keeps a draft id ↔ message id mapping
//! (`Store::set_draft` & co). Drafts made elsewhere (Gmail web, phone) have
//! no mapping until someone needs it; `resolve_*` then refreshes the whole
//! mapping from drafts.list.
//!
//! The flows are generic over [`DraftApi`] so they can be tested without
//! Google; [`GmailClient`] is the real implementation.

use penguin_core::{Address, Message, SentRef, Store};
use penguin_provider::text::snippet;
use reqwest::Method;
use serde::Deserialize;

use crate::api::{Cost, GmailClient, Priority, Retry, CHEAP};

/// drafts.create/update/send are unmeasured; charged like messages.send.
const UNMEASURED_WRITE: Cost = Cost::Fixed(100.0);
use crate::compose::{
    build_rfc822_draft_with_attachments, build_rfc822_with_attachments, AttachmentBytes, Draft,
};
use crate::convert::B64URL;
use crate::{Error, Result};

use base64::Engine;

/// Gmail's identifiers for one draft (or, after send, the sent message),
/// and a draft reopened for editing: the provider seam's types.
pub use penguin_provider::{DraftRef, OpenedDraft};

/// The drafts endpoints the flows need.
#[allow(async_fn_in_trait)]
pub trait DraftApi {
    async fn create_draft(&self, raw: &[u8], thread_id: Option<&str>) -> Result<DraftRef>;
    async fn update_draft(
        &self,
        draft_id: &str,
        raw: &[u8],
        thread_id: Option<&str>,
    ) -> Result<DraftRef>;
    /// Missing drafts are not an error (already sent or deleted elsewhere).
    async fn delete_draft(&self, draft_id: &str) -> Result<()>;
    /// drafts.send with the final content; returns the SENT message's ids
    /// (`draft_id` echoes the input).
    async fn send_draft(
        &self,
        draft_id: &str,
        raw: &[u8],
        thread_id: Option<&str>,
    ) -> Result<DraftRef>;
    async fn list_drafts(&self) -> Result<Vec<DraftRef>>;
    /// Fetch one message (for drafts not synced locally yet).
    async fn fetch_message(&self, message_id: &str) -> Result<Option<Message>>;
}

// ---------- Gmail implementation ----------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct MessageIdsWire {
    id: String,
    thread_id: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct DraftWire {
    id: String,
    message: MessageIdsWire,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct DraftListWire {
    drafts: Vec<DraftWire>,
    next_page_token: Option<String>,
}

impl From<DraftWire> for DraftRef {
    fn from(d: DraftWire) -> Self {
        DraftRef {
            draft_id: d.id,
            message_id: d.message.id,
            thread_id: d.message.thread_id,
            attachments: Vec::new(),
        }
    }
}

fn message_body(raw: &[u8], thread_id: Option<&str>) -> serde_json::Value {
    let mut message = serde_json::json!({ "raw": B64URL.encode(raw) });
    if let Some(t) = thread_id {
        message["threadId"] = serde_json::Value::String(t.to_string());
    }
    message
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| Error::Other(format!("{what}: bad json: {e}")))
}

impl DraftApi for GmailClient {
    async fn create_draft(&self, raw: &[u8], thread_id: Option<&str>) -> Result<DraftRef> {
        let body = serde_json::json!({ "message": message_body(raw, thread_id) });
        // Not idempotent: a blind retry could leave a duplicate draft.
        let bytes = self
            .call(
                Method::POST,
                "/drafts",
                &[],
                Some(&body),
                UNMEASURED_WRITE,
                Priority::Interactive,
                Retry::OnlyIfRejected,
            )
            .await?;
        Ok(parse::<DraftWire>(&bytes, "drafts.create")?.into())
    }

    async fn update_draft(
        &self,
        draft_id: &str,
        raw: &[u8],
        thread_id: Option<&str>,
    ) -> Result<DraftRef> {
        let body = serde_json::json!({ "id": draft_id, "message": message_body(raw, thread_id) });
        let path = format!("/drafts/{draft_id}");
        // Safe to repeat: a retried update of the same draft id with the
        // same content just yields a newer message id, which we use.
        let bytes = self
            .call(
                Method::PUT,
                &path,
                &[],
                Some(&body),
                UNMEASURED_WRITE,
                Priority::Interactive,
                Retry::Idempotent,
            )
            .await?;
        Ok(parse::<DraftWire>(&bytes, "drafts.update")?.into())
    }

    async fn delete_draft(&self, draft_id: &str) -> Result<()> {
        let path = format!("/drafts/{draft_id}");
        match self
            .call(
                Method::DELETE,
                &path,
                &[],
                None,
                Cost::Fixed(10.0),
                Priority::Interactive,
                Retry::Idempotent,
            )
            .await
        {
            Ok(_) | Err(Error::Http { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn send_draft(
        &self,
        draft_id: &str,
        raw: &[u8],
        thread_id: Option<&str>,
    ) -> Result<DraftRef> {
        let body = serde_json::json!({ "id": draft_id, "message": message_body(raw, thread_id) });
        let bytes = self
            .call(
                Method::POST,
                "/drafts/send",
                &[],
                Some(&body),
                UNMEASURED_WRITE,
                Priority::Interactive,
                Retry::OnlyIfRejected,
            )
            .await?;
        let sent: MessageIdsWire = parse(&bytes, "drafts.send")?;
        Ok(DraftRef {
            draft_id: draft_id.to_string(),
            message_id: sent.id,
            thread_id: sent.thread_id,
            attachments: Vec::new(),
        })
    }

    async fn list_drafts(&self) -> Result<Vec<DraftRef>> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", "500".to_string())];
            if let Some(t) = &page_token {
                query.push(("pageToken", t.clone()));
            }
            let page: DraftListWire = self
                .get_json("/drafts", &query, CHEAP, Priority::Interactive)
                .await?;
            out.extend(page.drafts.into_iter().map(DraftRef::from));
            match page.next_page_token.filter(|t| !t.is_empty()) {
                Some(t) => page_token = Some(t),
                None => return Ok(out),
            }
        }
    }

    async fn fetch_message(&self, message_id: &str) -> Result<Option<Message>> {
        Ok(self.get_messages(&[message_id.to_string()]).await?.pop())
    }
}

impl GmailClient {
    /// `drafts.send` of a saved draft as-is, by id only (send later): edits
    /// saved before now, HTML and attachments go out exactly as stored.
    /// A draft that's gone is HTTP 404.
    pub async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef> {
        let body = serde_json::json!({ "id": draft_id });
        // Not idempotent: only retried when Google certainly rejected it.
        let bytes = self
            .call(
                Method::POST,
                "/drafts/send",
                &[],
                Some(&body),
                Cost::Fixed(100.0),
                Priority::Interactive,
                Retry::OnlyIfRejected,
            )
            .await?;
        let sent: MessageIdsWire = parse(&bytes, "drafts.send")?;
        Ok(SentRef {
            message_id: sent.id,
            thread_id: sent.thread_id,
        })
    }
}

// ---------- flows ----------

async fn blocking<T: Send + 'static>(
    store: &Store,
    f: impl FnOnce(&Store) -> penguin_core::Result<T> + Send + 'static,
) -> Result<T> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| Error::Other(format!("store task failed: {e}")))?
        .map_err(Error::from)
}

fn bare(id: &str) -> &str {
    id.trim().trim_start_matches('<').trim_end_matches('>')
}

/// In-Reply-To and References for a reply to the stored message
/// `reply_to_message_id` (build_rfc822 appends the parent id itself).
async fn reply_headers(store: &Store, draft: &Draft) -> Result<(Option<String>, Vec<String>)> {
    let Some(parent_id) = draft.reply_to_message_id.clone() else {
        return Ok((None, Vec::new()));
    };
    let account = draft.account_id.clone();
    let parent = blocking(store, move |s| s.get_message(&account, &parent_id)).await?;
    Ok(parent
        .map(|p| (p.message_id_header, p.references))
        .unwrap_or_default())
}

/// The local copy of a just-saved draft, so the Drafts view and the thread
/// show it before sync fetches Gmail's version (which then replaces it).
pub fn local_draft_message(
    r: &DraftRef,
    draft: &Draft,
    from: &Address,
    in_reply_to: Option<&str>,
    references: &[String],
    now_ms: i64,
) -> Message {
    Message {
        account_id: draft.account_id.clone(),
        id: r.message_id.clone(),
        thread_id: r.thread_id.clone(),
        date: now_ms,
        from: from.clone(),
        to: draft.to.clone(),
        cc: draft.cc.clone(),
        bcc: draft.bcc.clone(),
        reply_to: Vec::new(),
        subject: draft.subject.clone(),
        snippet: snippet(&draft.body_text),
        body_text: draft.body_text.clone(),
        body_html: draft.body_html.clone().filter(|h| !h.trim().is_empty()),
        label_ids: vec!["DRAFT".to_string()],
        attachments: Vec::new(),
        message_id_header: None,
        in_reply_to: in_reply_to.map(|s| bare(s).to_string()),
        references: references.to_vec(),
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: false,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Create (no `draft_id`) or update a draft, then mirror it locally.
/// Updating a draft that no longer exists on Gmail (sent or deleted from
/// another device) creates a new one instead of losing the text.
///
/// `attachments` are `draft.attachments` resolved to bytes, in order. With
/// attachments, the new draft message is re-read so the local mirror lists
/// them and the returned [`DraftRef::attachments`] point at it (inline
/// images matched by Content-ID, files in order).
pub async fn save<A: DraftApi>(
    api: &A,
    store: &Store,
    draft: &Draft,
    from: &Address,
    draft_id: Option<&str>,
    attachments: &[AttachmentBytes],
) -> Result<DraftRef> {
    let (in_reply_to, references) = reply_headers(store, draft).await?;
    let raw = build_rfc822_draft_with_attachments(
        draft,
        from,
        in_reply_to.as_deref(),
        &references,
        attachments,
    )?;
    let thread = draft.reply_to_thread_id.as_deref();
    let account = draft.account_id.clone();

    let previous = match draft_id {
        Some(id) => {
            let (a, d) = (account.clone(), id.to_string());
            blocking(store, move |s| s.message_for_draft(&a, &d)).await?
        }
        None => None,
    };
    let mut saved = match draft_id {
        Some(id) => match api.update_draft(id, &raw, thread).await {
            Err(Error::Http { status: 404, .. }) => {
                let (a, d) = (account.clone(), id.to_string());
                blocking(store, move |s| s.remove_draft(&a, &d)).await?;
                api.create_draft(&raw, thread).await?
            }
            other => other?,
        },
        None => api.create_draft(&raw, thread).await?,
    };

    let mut local = local_draft_message(
        &saved,
        draft,
        from,
        in_reply_to.as_deref(),
        &references,
        now_ms(),
    );
    if !attachments.is_empty() {
        match api.fetch_message(&saved.message_id).await {
            Ok(Some(remote)) => {
                saved.attachments = penguin_provider::inline::saved_refs(
                    &saved.message_id,
                    &remote.attachments,
                    attachments,
                );
                local.attachments = remote.attachments;
            }
            // The draft is saved; the refs just can't be re-pointed yet. The
            // composer keeps what it has and the next save retries.
            Ok(None) => {
                tracing::warn!(message = %saved.message_id, "saved draft message not found to list its attachments")
            }
            Err(e) => {
                tracing::warn!(message = %saved.message_id, error = %e, "could not re-read saved draft attachments")
            }
        }
    }
    let r = saved.clone();
    blocking(store, move |s| {
        if let Some(old) = previous.filter(|old| *old != r.message_id) {
            s.delete_messages(&local.account_id, &[old])?;
        }
        s.upsert_messages(std::slice::from_ref(&local))?;
        s.set_draft(&local.account_id, &r.draft_id, &r.message_id)
    })
    .await?;
    Ok(saved)
}

/// Send a saved draft with its final content (drafts.send), so the draft
/// disappears from Drafts. Returns the sent message's ids. The sent copy
/// itself arrives through history sync.
/// `attachments` are `draft.attachments` resolved to bytes, in order.
pub async fn send<A: DraftApi>(
    api: &A,
    store: &Store,
    draft: &Draft,
    from: &Address,
    draft_id: &str,
    attachments: &[AttachmentBytes],
) -> Result<DraftRef> {
    let (in_reply_to, references) = reply_headers(store, draft).await?;
    let raw = build_rfc822_with_attachments(
        draft,
        from,
        in_reply_to.as_deref(),
        &references,
        attachments,
    )?;
    let sent = api
        .send_draft(draft_id, &raw, draft.reply_to_thread_id.as_deref())
        .await?;
    forget_local(store, &draft.account_id, draft_id).await?;
    Ok(sent)
}

/// Delete on Gmail, then locally. Returns the thread the draft was in, if
/// it was stored locally.
pub async fn delete<A: DraftApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    draft_id: &str,
) -> Result<Option<String>> {
    api.delete_draft(draft_id).await?;
    forget_local(store, account_id, draft_id).await
}

/// Drop the local draft message and mapping; returns its thread id.
async fn forget_local(store: &Store, account_id: &str, draft_id: &str) -> Result<Option<String>> {
    let (a, d) = (account_id.to_string(), draft_id.to_string());
    blocking(store, move |s| {
        let mut thread = None;
        if let Some(mid) = s.message_for_draft(&a, &d)? {
            thread = s.get_message(&a, &mid)?.map(|m| m.thread_id);
            s.delete_messages(&a, &[mid])?;
        }
        s.remove_draft(&a, &d)?;
        Ok(thread)
    })
    .await
}

/// Refresh the account's whole mapping from drafts.list.
async fn refresh_mapping<A: DraftApi>(api: &A, store: &Store, account_id: &str) -> Result<()> {
    let pairs: Vec<(String, String)> = api
        .list_drafts()
        .await?
        .into_iter()
        .map(|d| (d.draft_id, d.message_id))
        .collect();
    let a = account_id.to_string();
    blocking(store, move |s| s.replace_drafts(&a, &pairs)).await
}

/// Draft id for a DRAFT message, refreshing from Gmail when unmapped.
pub async fn resolve_draft_id<A: DraftApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    message_id: &str,
) -> Result<Option<String>> {
    let (a, m) = (account_id.to_string(), message_id.to_string());
    if let Some(id) = blocking(store, move |s| s.draft_for_message(&a, &m)).await? {
        return Ok(Some(id));
    }
    refresh_mapping(api, store, account_id).await?;
    let (a, m) = (account_id.to_string(), message_id.to_string());
    blocking(store, move |s| s.draft_for_message(&a, &m)).await
}

/// Current message id of a draft, refreshing from Gmail when unmapped.
pub async fn resolve_message_id<A: DraftApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    draft_id: &str,
) -> Result<Option<String>> {
    let (a, d) = (account_id.to_string(), draft_id.to_string());
    if let Some(id) = blocking(store, move |s| s.message_for_draft(&a, &d)).await? {
        return Ok(Some(id));
    }
    refresh_mapping(api, store, account_id).await?;
    let (a, d) = (account_id.to_string(), draft_id.to_string());
    blocking(store, move |s| s.message_for_draft(&a, &d)).await
}

/// Reopen a draft by draft id or by its message id (Drafts view). `None`
/// when Gmail has no such draft.
pub async fn open<A: DraftApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    draft_id: Option<&str>,
    message_id: Option<&str>,
) -> Result<Option<OpenedDraft>> {
    let (draft_id, message_id) = match (draft_id, message_id) {
        (Some(d), _) => match resolve_message_id(api, store, account_id, d).await? {
            Some(m) => (d.to_string(), m),
            None => return Ok(None),
        },
        (None, Some(m)) => match resolve_draft_id(api, store, account_id, m).await? {
            Some(d) => (d, m.to_string()),
            None => return Ok(None),
        },
        (None, None) => {
            return Err(Error::Other(
                "get_draft needs a draftId or a messageId".into(),
            ))
        }
    };

    let (a, m) = (account_id.to_string(), message_id.clone());
    let mut message = blocking(store, move |s| s.get_message(&a, &m)).await?;
    if message.is_none() {
        // Created elsewhere and not synced yet.
        message = api.fetch_message(&message_id).await?;
        if let Some(m) = message.clone() {
            blocking(store, move |s| s.upsert_messages(&[m])).await?;
        }
    }
    let Some(message) = message else {
        return Ok(None);
    };

    // Reply context: the thread message whose Message-ID the draft answers.
    let (a, t) = (account_id.to_string(), message.thread_id.clone());
    let thread = blocking(store, move |s| s.get_thread(&a, &t)).await?;
    let others: Vec<&Message> = thread
        .as_ref()
        .map(|t| t.messages.iter().filter(|m| m.id != message.id).collect())
        .unwrap_or_default();
    let parent = message
        .in_reply_to
        .as_deref()
        .and_then(|irt| {
            others
                .iter()
                .find(|m| m.message_id_header.as_deref().map(bare) == Some(bare(irt)))
        })
        .map(|m| m.id.clone());
    let reply_to_thread_id =
        (parent.is_some() || !others.is_empty()).then(|| message.thread_id.clone());
    // Stored HTML is untrusted (a draft may hold forwarded mail): it reaches
    // the composer only through the editor's strict allowlist, with the
    // inline images it shows (their parts come back as refs).
    let (body_html, attachments) = penguin_provider::inline::reopen(&message);

    Ok(Some(OpenedDraft {
        draft_id,
        message_id: message.id.clone(),
        thread_id: message.thread_id.clone(),
        draft: Draft {
            request_read_receipt: None,
            account_id: account_id.to_string(),
            to: message.to.clone(),
            cc: message.cc.clone(),
            bcc: message.bcc.clone(),
            subject: message.subject.clone(),
            body_text: message.body_text.clone(),
            body_html,
            reply_to_thread_id,
            reply_to_message_id: parent,
            attachments,
        },
    }))
}

#[cfg(test)]
#[path = "drafts_tests.rs"]
mod tests;
