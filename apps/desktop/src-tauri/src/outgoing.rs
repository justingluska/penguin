//! Outgoing mail, provider-neutral: resolving a draft's attachments to bytes,
//! sending (directly or through its saved draft), saving drafts, and the
//! originals a reply or forward quotes (`quote_sources`, with their inline
//! images). Inline images are attachments with a `content_id` that the HTML
//! shows (`penguin_provider::inline`); they resolve to bytes like any other
//! attachment. The commands in
//! commands.rs and the rule forward call these; they take the store, paths
//! and provider directly so tests run them against the fake provider.

use std::collections::HashMap;
use std::sync::Arc;

use penguin_core::{Address, AttachmentMeta, SentRef, Store};
use penguin_provider::compose::{
    build_rfc822_with_attachments, decode_file_attachment, AttachmentBytes, Draft,
    OutgoingAttachment, MAX_ATTACHMENTS_BYTES,
};
use penguin_provider::{DraftRef, MailProvider};
use serde::Serialize;

use crate::attachments;
use crate::error::{CmdError, CmdResult};
use crate::ops::{self, Paths};
use crate::state::blocking;

/// Providers of the other accounts a draft's attachments come from (a
/// forward sent from a different account than the one the mail is in), by
/// account id.
pub type OtherAccounts = HashMap<String, Arc<dyn MailProvider>>;

/// Bytes for every attachment on `draft`, in order: `file`s are decoded,
/// stored-message refs (`gmail` on the wire) come from the attachment cache
/// or the provider of the account they're stored in (`provider` for the
/// sending account, else `others`). The 25 MB limit (files and inline
/// images together) is checked from the declared sizes first, so an
/// oversized draft fails before anything is downloaded. A file
/// that can't be fetched fails the whole send or save, naming the file (and
/// logged with ids only): nothing goes out without it.
pub async fn resolve_attachments(
    paths: &Paths,
    provider: &dyn MailProvider,
    others: &OtherAccounts,
    account_id: &str,
    draft: &Draft,
) -> CmdResult<Vec<AttachmentBytes>> {
    let declared: u64 = draft
        .attachments
        .iter()
        .map(|a| match a {
            OutgoingAttachment::File { data_base64, .. } => data_base64.len() as u64 / 4 * 3,
            OutgoingAttachment::Gmail { size, .. } => *size,
        })
        .sum();
    if declared > MAX_ATTACHMENTS_BYTES {
        let images = draft.attachments.iter().any(|a| a.content_id().is_some());
        return Err(CmdError::invalid(format!(
            "{} total {:.1} MB; the limit is {} MB",
            if images {
                "Attachments and images"
            } else {
                "Attachments"
            },
            declared as f64 / (1024.0 * 1024.0),
            MAX_ATTACHMENTS_BYTES / (1024 * 1024)
        )));
    }
    let mut out = Vec::with_capacity(draft.attachments.len());
    for a in &draft.attachments {
        match a {
            OutgoingAttachment::File { .. } => {
                let decoded =
                    decode_file_attachment(a).map_err(|e| CmdError::invalid(e.to_string()))?;
                out.extend(decoded);
            }
            OutgoingAttachment::Gmail {
                message_id,
                attachment_id,
                filename,
                mime_type,
                size,
                account_id: stored_in,
                content_id,
            } => {
                let (acct, source) = match stored_in.as_deref().filter(|a| *a != account_id) {
                    None => (account_id, provider),
                    Some(other) => match others.get(other) {
                        Some(p) => (other, p.as_ref()),
                        None => {
                            return Err(CmdError::invalid(format!(
                                "Couldn't attach “{filename}”: {other} isn't connected"
                            )))
                        }
                    },
                };
                let meta = AttachmentMeta {
                    id: attachment_id.clone(),
                    filename: filename.clone(),
                    mime_type: mime_type.clone(),
                    size: *size,
                    content_id: None,
                    inline: false,
                };
                let bytes = attachments::bytes(paths, source, acct, message_id, &meta)
                    .await
                    .map_err(|e| {
                        tracing::warn!(account = %acct, message = %message_id, attachment = %attachment_id, code = ?e.code, "outgoing attachment could not be fetched");
                        CmdError::new(e.code, format!("Couldn't attach “{filename}”: {}", e.message))
                    })?;
                out.push(AttachmentBytes {
                    filename: filename.clone(),
                    mime_type: mime_type.clone(),
                    bytes,
                    content_id: content_id.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// Send `draft` now: through its saved draft when there is one (Gmail
/// drafts.send with the final content, which removes the draft), else as a
/// new message with In-Reply-To/References from the stored parent.
pub async fn send(
    store: &Store,
    paths: &Paths,
    provider: &dyn MailProvider,
    others: &OtherAccounts,
    from: &Address,
    draft: &Draft,
    draft_id: Option<&str>,
) -> CmdResult<SentRef> {
    let account_id = draft.account_id.as_str();
    let files = resolve_attachments(paths, provider, others, account_id, draft).await?;
    if let Some(draft_id) = draft_id {
        let sent = provider.send_draft(draft, from, draft_id, &files).await?;
        tracing::info!(account = %account_id, message = %sent.message_id, attachments = files.len(), "draft sent");
        return Ok(SentRef {
            message_id: sent.message_id,
            thread_id: sent.thread_id,
        });
    }
    let (in_reply_to, references) = match draft.reply_to_message_id.clone() {
        Some(parent_id) => {
            let (store, acct) = (store.clone(), account_id.to_string());
            let parent = blocking(move || Ok(store.get_message(&acct, &parent_id)?)).await?;
            parent.as_ref().map(ops::reply_headers).unwrap_or_default()
        }
        None => (None, Vec::new()),
    };
    let raw =
        build_rfc822_with_attachments(draft, from, in_reply_to.as_deref(), &references, &files)?;
    let sent = provider
        .send_raw(&raw, draft.reply_to_thread_id.as_deref())
        .await?;
    tracing::info!(account = %account_id, message = %sent.message_id, attachments = files.len(), "message sent");
    Ok(sent)
}

/// Create (`draft_id` None) or update a server draft with every attachment.
/// The provider re-ids a saved draft's attachments; their bytes are seeded
/// into the cache under the new ids so the next save or send doesn't
/// download them again.
pub async fn save_draft(
    paths: &Paths,
    provider: &dyn MailProvider,
    others: &OtherAccounts,
    from: &Address,
    draft: &Draft,
    draft_id: Option<&str>,
) -> CmdResult<DraftRef> {
    let account_id = draft.account_id.as_str();
    let files = resolve_attachments(paths, provider, others, account_id, draft).await?;
    let saved = provider.save_draft(draft, from, draft_id, &files).await?;
    if saved.attachments.len() == files.len() {
        for (r, f) in saved.attachments.iter().zip(&files) {
            if let OutgoingAttachment::Gmail {
                message_id,
                attachment_id,
                ..
            } = r
            {
                if let Err(e) =
                    attachments::put_cached(paths, account_id, message_id, attachment_id, &f.bytes)
                {
                    tracing::warn!(account = %account_id, error = %e, "could not cache a draft attachment");
                }
            }
        }
    }
    Ok(saved)
}

// ---------- quoting ----------

/// An original a reply or forward quotes (`quote_sources`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuoteSource {
    pub message_id: String,
    /// Its HTML body through `penguin_render::sanitize_quoted_html`
    /// (formatting kept, nothing active or remote); null for text-only mail
    /// or when `with_body` was false.
    pub html: Option<String>,
    /// Its text body (from the HTML when it has no text part); null when
    /// `with_body` was false.
    pub text: Option<String>,
    /// Its attachments, inline ones included (the composer forwards the
    /// others).
    pub attachments: Vec<AttachmentMeta>,
    /// The inline images `html` still shows, as refs to the original's
    /// parts under the fresh Content-IDs `html` now uses (the account they're
    /// stored in set, so a send from another account can fetch them). The
    /// composer attaches them with the quote. Empty without `with_body`.
    pub inline_images: Vec<OutgoingAttachment>,
}

/// Most originals one call loads (a long conversation's forward asks for
/// every message's attachments).
pub const MAX_QUOTE_SOURCES: usize = 500;

/// The stored originals `message_ids` (in that order), downloading the
/// bodies of headers-only ones first (mail older than the sync window has no
/// body or attachment list until then). Messages with a stored body never
/// touch the network. Returns the threads whose bodies arrived, so the
/// caller can emit mail-changed for them.
pub async fn quote_sources(
    store: &Store,
    provider: Option<&dyn MailProvider>,
    account_id: &str,
    message_ids: &[String],
    with_body: bool,
) -> CmdResult<(Vec<QuoteSource>, Vec<String>)> {
    if message_ids.len() > MAX_QUOTE_SOURCES {
        return Err(CmdError::invalid(format!(
            "At most {MAX_QUOTE_SOURCES} messages at once"
        )));
    }
    let pending = {
        let (store, acct, ids) = (store.clone(), account_id.to_string(), message_ids.to_vec());
        blocking(move || {
            let mut out = Vec::new();
            for id in ids {
                if store.is_body_pending(&acct, &id)? == Some(true) {
                    out.push(id);
                }
            }
            Ok(out)
        })
        .await?
    };
    let mut fetched_threads = Vec::new();
    if !pending.is_empty() {
        let provider = provider.ok_or_else(|| {
            CmdError::new(
                crate::error::ErrorCode::Network,
                "This account isn't connected, so the original can't be downloaded",
            )
        })?;
        fetched_threads = provider.fetch_pending_bodies(&pending).await.map_err(|e| {
            let e = CmdError::from(e);
            tracing::warn!(account = %account_id, messages = ?pending, code = ?e.code, error = %e.message, "could not download the original to quote");
            e
        })?;
    }
    let (store, acct, ids) = (store.clone(), account_id.to_string(), message_ids.to_vec());
    let out = blocking(move || {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(m) = store.get_message(&acct, &id)? else {
                return Err(CmdError::not_found("The original message is no longer here"));
            };
            if store.is_body_pending(&acct, &id)? == Some(true) {
                tracing::warn!(account = %acct, message = %id, "original still has no body after downloading it");
                return Err(CmdError::not_found(
                    "The original message is no longer on the server",
                ));
            }
            let mut inline_images = Vec::new();
            let (html, text) = if with_body {
                // Inline (cid:) images come along under fresh ids; remote
                // ones are dropped (docs/SECURITY.md → Rich-text composer).
                let (cids, refs) = penguin_provider::inline::quote_images(&m, Some(&acct));
                inline_images = refs;
                let html = m
                    .body_html
                    .as_deref()
                    .filter(|h| !h.trim().is_empty())
                    .map(|h| penguin_render::sanitize_quoted_html_with_images(h, &cids));
                let text = if m.body_text.trim().is_empty() {
                    m.body_html
                        .as_deref()
                        .map(penguin_core::text::html_to_text)
                        .unwrap_or_default()
                } else {
                    m.body_text.clone()
                };
                (html, Some(text))
            } else {
                (None, None)
            };
            out.push(QuoteSource {
                message_id: m.id,
                html,
                text,
                attachments: m.attachments,
                inline_images,
            });
        }
        Ok(out)
    })
    .await?;
    Ok((out, fetched_threads))
}

#[cfg(test)]
#[path = "outgoing_tests.rs"]
mod tests;
