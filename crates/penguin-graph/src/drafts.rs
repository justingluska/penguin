//! Drafts on Graph (docs/PROVIDERS-IMPL.md §2.1).
//!
//! A draft is an Outlook message in Drafts, edited in place with PATCH, so
//! the **draft id is the message id** (immutable) and never changes across
//! saves. Replies start from `createReply` on the parent, which gives the
//! draft the conversation and the In-Reply-To/References headers (Graph
//! can't set those directly). Attachments are synced on each save: kept
//! when the composer still lists them (a ref to this draft's own
//! attachment), deleted when dropped, uploaded when new (≤ 3 MB in the
//! request, larger through an upload session). Inline images (an attachment
//! with a `content_id` the HTML shows, [`penguin_provider::inline::plan`])
//! are uploaded with `isInline` and `contentId`, which is how Outlook
//! resolves the body's `cid:` references.
//!
//! Sending a draft moves it to Sent Items under a new id, so after
//! `send_saved_draft` the draft id no longer names a draft: a second call
//! is `NotFound` (checked before sending), which keeps retries from
//! sending twice.

use base64::Engine;
use penguin_core::{Address, AttachmentMeta, Message, SentRef};
use penguin_provider::compose::{AttachmentBytes, Draft, OutgoingAttachment};
use penguin_provider::ids::system;
use penguin_provider::{DraftRef, Error, OpenedDraft, Result};
use serde_json::{json, Value};

use crate::client::{enc, GraphClient};
use crate::convert::snippet;
use crate::http::{is_gone, Body, Method, Retry};
use crate::locations;
use crate::wire::{Attachment, Created, Page, UploadSession};

/// Attachments up to this size go inline in one request; larger ones use
/// an upload session (Graph's limit for the inline form is 3 MB).
const INLINE_UPLOAD_MAX: usize = 3 * 1024 * 1024;
/// Upload-session chunk: a multiple of 320 KiB, as Graph requires.
const UPLOAD_CHUNK: usize = 10 * 320 * 1024;

fn recipients(list: &[Address]) -> Value {
    Value::Array(
        list.iter()
            .filter(|a| !a.email.trim().is_empty())
            .map(|a| {
                let mut e = json!({ "address": a.email.trim() });
                if let Some(n) = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    e["name"] = Value::String(n.to_string());
                }
                json!({ "emailAddress": e })
            })
            .collect(),
    )
}

/// Which of `draft`'s attachments are inline images, and its HTML.
fn inline_plan(draft: &Draft) -> penguin_provider::inline::Plan {
    let parts: Vec<(Option<&str>, &str)> = draft
        .attachments
        .iter()
        .map(|a| (a.content_id(), a.mime_type()))
        .collect();
    penguin_provider::inline::plan(draft.body_html.as_deref(), &parts)
}

/// The PATCH/POST body for a draft's content.
pub(crate) fn draft_body(draft: &Draft) -> Value {
    let body = match inline_plan(draft).html {
        Some(html) => json!({ "contentType": "html", "content": html }),
        None => json!({ "contentType": "text", "content": draft.body_text }),
    };
    json!({
        "subject": draft.subject,
        "body": body,
        "toRecipients": recipients(&draft.to),
        "ccRecipients": recipients(&draft.cc),
        "bccRecipients": recipients(&draft.bcc),
        // Graph's form of Disposition-Notification-To (penguin_provider::compose).
        "isReadReceiptRequested": draft.request_read_receipt == Some(true),
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl GraphClient {
    async fn create_draft(&self, draft: &Draft) -> Result<Created> {
        // A reply: start from createReply on the parent so Outlook threads it.
        if let Some(parent) = draft.reply_to_message_id.as_deref() {
            match self
                .api
                .call(
                    Method::Post,
                    &format!("/me/messages/{}/createReply", enc(parent)),
                    &[],
                    Body::Json(json!({})),
                    &[],
                    Retry::OnlyIfRejected,
                )
                .await
            {
                Ok(resp) => {
                    let created: Created = resp.json("createReply")?;
                    self.patch(&created.id, draft_body(draft)).await?;
                    return Ok(created);
                }
                // The parent is gone: save it as a new message instead.
                Err(e) if is_gone(&e) => {}
                Err(e) => return Err(e),
            }
        }
        let resp = self
            .api
            .call(
                Method::Post,
                "/me/messages",
                &[],
                Body::Json(draft_body(draft)),
                &[],
                Retry::OnlyIfRejected,
            )
            .await?;
        resp.json("draft")
    }

    /// A draft's attachments, with the Content-ID of the inline ones
    /// (`contentId` is on fileAttachment only, so `$select` on the list
    /// can't ask for it).
    async fn list_attachments(&self, message_id: &str) -> Result<Vec<Attachment>> {
        let base = format!("/me/messages/{}/attachments", enc(message_id));
        let page: Page<Attachment> = self
            .api
            .get_json(
                &base,
                &[("$select", "id,name,contentType,size,isInline".into())],
                &[],
                "attachments",
            )
            .await?;
        let mut list = page.value;
        for a in list
            .iter_mut()
            .filter(|a| a.is_inline && a.content_id.is_none())
        {
            match self
                .api
                .get_json::<Attachment>(&format!("{base}/{}", enc(&a.id)), &[], &[], "attachment")
                .await
            {
                Ok(one) => a.content_id = one.content_id,
                Err(e) if is_gone(&e) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(list)
    }

    /// Upload one attachment; `content_id` makes it an inline image.
    async fn upload_attachment(
        &self,
        message_id: &str,
        a: &AttachmentBytes,
        content_id: Option<&str>,
    ) -> Result<String> {
        let base = format!("/me/messages/{}/attachments", enc(message_id));
        if a.bytes.len() <= INLINE_UPLOAD_MAX {
            let mut body = json!({
                "@odata.type": "#microsoft.graph.fileAttachment",
                "name": a.filename,
                "contentType": a.mime_type,
                "contentBytes": base64::engine::general_purpose::STANDARD.encode(&a.bytes),
            });
            if let Some(cid) = content_id {
                body["isInline"] = Value::Bool(true);
                body["contentId"] = Value::String(cid.to_string());
            }
            let resp = self
                .api
                .call(
                    Method::Post,
                    &base,
                    &[],
                    Body::Json(body),
                    &[],
                    Retry::OnlyIfRejected,
                )
                .await?;
            let created: Attachment = resp.json("attachment")?;
            return Ok(created.id);
        }
        let session: UploadSession = self
            .api
            .call(
                Method::Post,
                &format!("{base}/createUploadSession"),
                &[],
                Body::Json(json!({
                    "AttachmentItem": match content_id {
                        Some(cid) => json!({
                            "attachmentType": "file",
                            "name": a.filename,
                            "size": a.bytes.len(),
                            "contentType": a.mime_type,
                            "isInline": true,
                            "contentId": cid,
                        }),
                        None => json!({
                            "attachmentType": "file",
                            "name": a.filename,
                            "size": a.bytes.len(),
                            "contentType": a.mime_type,
                        }),
                    }
                })),
                &[],
                Retry::OnlyIfRejected,
            )
            .await?
            .json("upload session")?;
        let total = a.bytes.len();
        let mut last = None;
        for (i, chunk) in a.bytes.chunks(UPLOAD_CHUNK).enumerate() {
            let start = i * UPLOAD_CHUNK;
            let end = start + chunk.len() - 1;
            let resp = self
                .api
                .put_unauthenticated(
                    &session.upload_url,
                    chunk.to_vec(),
                    vec![
                        ("Content-Type".into(), "application/octet-stream".into()),
                        (
                            "Content-Range".into(),
                            format!("bytes {start}-{end}/{total}"),
                        ),
                    ],
                )
                .await?;
            last = Some(resp);
        }
        // The last chunk's answer carries the attachment's location; its id
        // is the last path segment of `Location` (…/attachments('<id>')).
        let id = last
            .as_ref()
            .and_then(|r| r.header("Location"))
            .and_then(|loc| {
                let tail = loc.rsplit("attachments(").next()?;
                Some(
                    tail.trim_start_matches('\'')
                        .trim_end_matches(")")
                        .trim_end_matches('\'')
                        .to_string(),
                )
            })
            .filter(|s| !s.is_empty());
        match id {
            Some(id) => Ok(id),
            // No Location: find it by name and size.
            None => self
                .list_attachments(message_id)
                .await?
                .into_iter()
                .rev()
                .find(|x| {
                    x.name.as_deref() == Some(a.filename.as_str())
                        && x.size as usize >= total.min(1)
                })
                .map(|x| x.id)
                .ok_or_else(|| {
                    Error::Other(format!("attachment {} uploaded but not found", a.filename))
                }),
        }
    }

    /// Make the draft's attachments match `draft.attachments`; returns them
    /// as refs to this draft, in order. An attachment is kept when the
    /// composer still points at it and it's still the same kind (inline
    /// with the same Content-ID, or a file); everything else on the draft
    /// is deleted, inline parts included (the body is ours, so an inline
    /// part it no longer shows is an orphan).
    async fn sync_attachments(
        &self,
        draft_id: &str,
        draft: &Draft,
        attachments: &[AttachmentBytes],
        existing: Vec<Attachment>,
    ) -> Result<(Vec<OutgoingAttachment>, Vec<AttachmentMeta>)> {
        if attachments.len() != draft.attachments.len() {
            return Err(Error::Other(format!(
                "{} attachments on the draft but {} resolved",
                draft.attachments.len(),
                attachments.len()
            )));
        }
        let inline = inline_plan(draft).inline;
        let cid_of = |i: usize| {
            inline[i]
                .then(|| draft.attachments[i].content_id())
                .flatten()
                .map(|c| {
                    c.trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string()
                })
        };
        let mut keep: Vec<Option<String>> = Vec::with_capacity(draft.attachments.len());
        for (i, a) in draft.attachments.iter().enumerate() {
            let want = cid_of(i).map(|c| penguin_render::normalize_cid(&c));
            let kept = match a {
                OutgoingAttachment::Gmail {
                    message_id,
                    attachment_id,
                    ..
                } if message_id == draft_id
                    && !keep.iter().flatten().any(|k| k == attachment_id)
                    && existing.iter().any(|e| {
                        e.id == *attachment_id
                            && e.is_inline == want.is_some()
                            && (want.is_none()
                                || e.content_id.as_deref().map(penguin_render::normalize_cid)
                                    == want)
                    }) =>
                {
                    Some(attachment_id.clone())
                }
                _ => None,
            };
            keep.push(kept);
        }
        for e in existing.iter() {
            if !keep.iter().flatten().any(|k| *k == e.id) {
                match self
                    .api
                    .call(
                        Method::Delete,
                        &format!("/me/messages/{}/attachments/{}", enc(draft_id), enc(&e.id)),
                        &[],
                        Body::None,
                        &[],
                        Retry::Idempotent,
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(err) if is_gone(&err) => {}
                    Err(err) => return Err(err),
                }
            }
        }
        let mut refs = Vec::with_capacity(attachments.len());
        let mut metas = Vec::with_capacity(attachments.len());
        for (i, bytes) in attachments.iter().enumerate() {
            let content_id = cid_of(i);
            let id = match &keep[i] {
                Some(id) => id.clone(),
                None => {
                    self.upload_attachment(draft_id, bytes, content_id.as_deref())
                        .await?
                }
            };
            let size = existing
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.size)
                .unwrap_or(bytes.bytes.len() as u64);
            refs.push(OutgoingAttachment::Gmail {
                message_id: draft_id.to_string(),
                attachment_id: id.clone(),
                filename: bytes.filename.clone(),
                mime_type: bytes.mime_type.clone(),
                size,
                account_id: None,
                content_id: content_id.clone(),
            });
            metas.push(AttachmentMeta {
                id,
                filename: bytes.filename.clone(),
                mime_type: bytes.mime_type.clone(),
                size,
                inline: content_id.is_some(),
                content_id,
            });
        }
        Ok((refs, metas))
    }

    /// Create or update a draft on the server and mirror it locally.
    pub(crate) async fn save_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: Option<&str>,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        let acct = self.account_id().to_string();
        let (id, thread_id, fresh) = match draft_id {
            Some(id) => match self.patch(id, draft_body(draft)).await {
                Ok(updated) => {
                    let meta = match updated.conversation_id {
                        Some(_) => Some(updated),
                        None => self.draft_meta(id).await?,
                    };
                    match meta {
                        Some(m) if m.is_draft != Some(false) => (
                            id.to_string(),
                            m.conversation_id.unwrap_or_else(|| id.to_string()),
                            false,
                        ),
                        // Sent meanwhile (or gone): start a new draft.
                        _ => {
                            let c = self.create_draft(draft).await?;
                            let t = c.conversation_id.clone().unwrap_or_else(|| c.id.clone());
                            (c.id, t, true)
                        }
                    }
                }
                Err(e) if is_gone(&e) => {
                    let c = self.create_draft(draft).await?;
                    let t = c.conversation_id.clone().unwrap_or_else(|| c.id.clone());
                    (c.id, t, true)
                }
                Err(e) => return Err(e),
            },
            None => {
                let c = self.create_draft(draft).await?;
                let t = c.conversation_id.clone().unwrap_or_else(|| c.id.clone());
                (c.id, t, true)
            }
        };
        let existing =
            if fresh && draft.reply_to_message_id.is_none() && draft.attachments.is_empty() {
                Vec::new()
            } else {
                self.list_attachments(&id).await?
            };
        let (refs, metas) = self
            .sync_attachments(&id, draft, attachments, existing)
            .await?;

        // Local mirror: the Drafts view shows it before sync does.
        let (in_reply_to, references) = self.reply_headers(draft).await?;
        let local = Message {
            account_id: acct.clone(),
            id: id.clone(),
            thread_id: thread_id.clone(),
            date: now_ms(),
            from: from.clone(),
            to: draft.to.clone(),
            cc: draft.cc.clone(),
            bcc: draft.bcc.clone(),
            reply_to: Vec::new(),
            subject: draft.subject.clone(),
            snippet: snippet(&draft.body_text),
            body_text: draft.body_text.clone(),
            body_html: draft.body_html.clone().filter(|h| !h.trim().is_empty()),
            label_ids: vec![system::DRAFT.to_string()],
            attachments: metas,
            message_id_header: None,
            in_reply_to,
            references,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        };
        let drafts_folder = self
            .folders()
            .await?
            .well_known("drafts")
            .map(str::to_string);
        let previous_draft = draft_id.map(str::to_string);
        let (new_id, new_draft) = (id.clone(), id.clone());
        self.db(move |s| {
            if let Some(old) = previous_draft.filter(|old| *old != new_id) {
                if let Some(old_msg) = s.message_for_draft(&acct, &old)? {
                    s.delete_messages(&acct, std::slice::from_ref(&old_msg))?;
                    locations::delete(s, &acct, &[old_msg])?;
                }
                s.remove_draft(&acct, &old)?;
            }
            s.upsert_messages(std::slice::from_ref(&local))?;
            if let Some(f) = drafts_folder {
                locations::set(s, &acct, &[(new_id.clone(), f)])?;
            }
            s.set_draft(&acct, &new_draft, &new_id)
        })
        .await?;
        Ok(DraftRef {
            draft_id: id.clone(),
            message_id: id,
            thread_id,
            attachments: refs,
        })
    }

    /// In-Reply-To and References for the local copy of a reply draft.
    async fn reply_headers(&self, draft: &Draft) -> Result<(Option<String>, Vec<String>)> {
        let Some(parent) = draft.reply_to_message_id.clone() else {
            return Ok((None, Vec::new()));
        };
        let acct = draft.account_id.clone();
        let parent = self.db(move |s| s.get_message(&acct, &parent)).await?;
        Ok(match parent {
            Some(p) => {
                let mut refs = p.references.clone();
                if let Some(mid) = &p.message_id_header {
                    if refs.last() != Some(mid) {
                        refs.push(mid.clone());
                    }
                }
                (p.message_id_header, refs)
            }
            None => (None, Vec::new()),
        })
    }

    /// Whether `id` is still a draft (and its conversation): None = gone.
    async fn draft_meta(&self, id: &str) -> Result<Option<Created>> {
        match self
            .api
            .get_json::<Created>(
                &format!("/me/messages/{}", enc(id)),
                &[("$select", "id,conversationId,isDraft,parentFolderId".into())],
                &[],
                "draft",
            )
            .await
        {
            Ok(c) => Ok(Some(c)),
            Err(e) if is_gone(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn send_existing(&self, id: &str) -> Result<()> {
        self.api
            .call(
                Method::Post,
                &format!("/me/messages/{}/send", enc(id)),
                &[],
                Body::None,
                &[],
                Retry::OnlyIfRejected,
            )
            .await?;
        Ok(())
    }

    /// Drop a draft's local copy, mapping and location; returns its thread.
    pub(crate) async fn forget_draft(&self, draft_id: &str) -> Result<Option<String>> {
        let (acct, d) = (self.account_id().to_string(), draft_id.to_string());
        self.db(move |s| {
            let mut thread = None;
            let mid = s.message_for_draft(&acct, &d)?.unwrap_or_else(|| d.clone());
            if let Some(m) = s.get_message(&acct, &mid)? {
                thread = Some(m.thread_id);
                s.delete_messages(&acct, std::slice::from_ref(&mid))?;
            }
            locations::delete(s, &acct, &[mid])?;
            s.remove_draft(&acct, &d)?;
            Ok(thread)
        })
        .await
    }

    pub(crate) async fn send_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: &str,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            return Err(Error::InvalidInput("Add at least one recipient.".into()));
        }
        let saved = self
            .save_draft(draft, from, Some(draft_id), attachments)
            .await?;
        self.send_existing(&saved.draft_id).await?;
        self.forget_draft(&saved.draft_id).await?;
        if saved.draft_id != draft_id {
            self.forget_draft(draft_id).await?;
        }
        Ok(DraftRef {
            draft_id: draft_id.to_string(),
            message_id: saved.message_id,
            thread_id: saved.thread_id,
            attachments: Vec::new(),
        })
    }

    pub(crate) async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef> {
        let meta = self.draft_meta(draft_id).await?;
        let Some(meta) = meta.filter(|m| m.is_draft != Some(false)) else {
            return Err(Error::NotFound(format!(
                "draft {draft_id} no longer exists"
            )));
        };
        match self.send_existing(draft_id).await {
            Ok(()) => {}
            Err(e) if is_gone(&e) => {
                return Err(Error::NotFound(format!(
                    "draft {draft_id} no longer exists"
                )))
            }
            Err(e) => return Err(e),
        }
        self.forget_draft(draft_id).await?;
        Ok(SentRef {
            message_id: draft_id.to_string(),
            thread_id: meta.conversation_id.unwrap_or_else(|| draft_id.to_string()),
        })
    }

    pub(crate) async fn delete_draft(&self, draft_id: &str) -> Result<Option<String>> {
        // Only delete what is still a draft: an id that now names a sent
        // message must never be deleted from here.
        if let Some(meta) = self.draft_meta(draft_id).await? {
            if meta.is_draft != Some(false) {
                match self
                    .api
                    .call(
                        Method::Delete,
                        &format!("/me/messages/{}", enc(draft_id)),
                        &[],
                        Body::None,
                        &[],
                        Retry::Idempotent,
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(e) if is_gone(&e) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.forget_draft(draft_id).await
    }

    pub(crate) async fn open_draft(
        &self,
        draft_id: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<Option<OpenedDraft>> {
        let acct = self.account_id().to_string();
        let id = match (draft_id, message_id) {
            (Some(d), _) => {
                let (a, dd) = (acct.clone(), d.to_string());
                self.db(move |s| s.message_for_draft(&a, &dd))
                    .await?
                    .unwrap_or_else(|| d.to_string())
            }
            (None, Some(m)) => m.to_string(),
            (None, None) => {
                return Err(Error::InvalidInput(
                    "Opening a draft needs a draft id or a message id".into(),
                ))
            }
        };
        let Some(w) = self.get_full_wire(&id).await? else {
            return Ok(None);
        };
        if w.is_draft == Some(false) {
            return Ok(None);
        }
        let fetched = self.convert(&w, true).await?;
        let message = fetched.message.clone();
        // Keep the local copy current (and mapped) for the Drafts view.
        let (a, mid, folder) = (acct.clone(), message.id.clone(), fetched.folder_id.clone());
        let stored = message.clone();
        self.db(move |s| {
            s.upsert_messages(std::slice::from_ref(&stored))?;
            if let Some(f) = folder {
                locations::set(s, &a, &[(mid.clone(), f)])?;
            }
            s.set_draft(&a, &mid, &mid)
        })
        .await?;
        let (a, t) = (acct.clone(), message.thread_id.clone());
        let thread = self.db(move |s| s.get_thread(&a, &t)).await?;
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
                    .find(|m| m.message_id_header.as_deref() == Some(irt))
            })
            .map(|m| m.id.clone());
        let reply_to_thread_id =
            (parent.is_some() || !others.is_empty()).then(|| message.thread_id.clone());
        // The HTML through the editor's allowlist with the inline images it
        // shows; their parts come back as refs next to the files.
        let (body_html, attachments) = penguin_provider::inline::reopen(&message);
        Ok(Some(OpenedDraft {
            draft_id: message.id.clone(),
            message_id: message.id.clone(),
            thread_id: message.thread_id.clone(),
            draft: Draft {
                request_read_receipt: None,
                account_id: acct,
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

    /// Send complete MIME bytes: create a draft from them (Graph parses the
    /// MIME, Bcc included, and keeps it out of what recipients get), then
    /// send it, so the conversation id is known. A draft whose send fails
    /// is deleted again.
    pub(crate) async fn send_mime(&self, raw: &[u8], thread_hint: Option<&str>) -> Result<SentRef> {
        let body = base64::engine::general_purpose::STANDARD.encode(raw);
        let created: Created = self
            .api
            .call(
                Method::Post,
                "/me/messages",
                &[],
                Body::Bytes(body.into_bytes(), "text/plain"),
                &[],
                Retry::OnlyIfRejected,
            )
            .await?
            .json("MIME draft")?;
        if let Err(e) = self.send_existing(&created.id).await {
            if let Err(cleanup) = self
                .api
                .call(
                    Method::Delete,
                    &format!("/me/messages/{}", enc(&created.id)),
                    &[],
                    Body::None,
                    &[],
                    Retry::Idempotent,
                )
                .await
            {
                tracing::warn!(account = %self.account_id(), error = %cleanup, "could not remove the unsent draft");
            }
            return Err(e);
        }
        let thread_id = created
            .conversation_id
            .filter(|c| !c.is_empty())
            .or_else(|| thread_hint.map(str::to_string))
            .unwrap_or_else(|| created.id.clone());
        Ok(SentRef {
            message_id: created.id,
            thread_id,
        })
    }
}
