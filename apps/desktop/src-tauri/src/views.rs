//! App-layer types that exist only at the Tauri boundary. Mirrored in
//! `apps/desktop/src/lib/types.ts` (MessageView, ThreadView, ThreadAction,
//! ThreadRef, OAuthClientStatus, MailChangedEvent, SignInLink) — change both together.

use std::collections::HashMap;

use penguin_core::unsubscribe::UnsubscribeOffer;
use penguin_core::{Address, AttachmentMeta, Message, Otp, ThreadDetail};
use penguin_render::{render_html, render_text, RenderOptions, TrackerRemoved};
use serde::{Deserialize, Serialize};

use crate::settings::RemoteImageDecision;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClientStatus {
    pub configured: bool,
    pub path: String,
    /// iOS OAuth client id enabling the macOS sign-in sheet; null = browser sign-in.
    pub ios_client_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRef {
    pub account_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThreadAction {
    Archive,
    MoveToInbox,
    Trash,
    Untrash,
    MarkRead,
    MarkUnread,
    Star,
    Unstar,
    AddLabel {
        #[serde(rename = "labelId")]
        label_id: String,
    },
    RemoveLabel {
        #[serde(rename = "labelId")]
        label_id: String,
    },
    /// Reply Later: add the account's Reply Later label, archive, mark read
    /// (one delta, so IMAP does a single move). Sent by `reply_later`,
    /// which knows each account's label id.
    ReplyLater {
        #[serde(rename = "labelId")]
        label_id: String,
    },
    /// Report spam: into Spam and out of the inbox (Gmail +SPAM −INBOX,
    /// IMAP a move to the Junk folder, Graph a move to junkemail).
    ReportSpam,
    /// Not spam: out of Spam and back to the inbox.
    NotSpam,
}

impl ThreadAction {
    /// Local label delta applied optimistically: (add, remove).
    /// Trash/untrash mirror what Gmail does to the labels server-side
    /// (trash drops INBOX; untrash restores the thread to the inbox).
    pub fn local_delta(&self) -> (Vec<String>, Vec<String>) {
        let s = |v: &str| v.to_string();
        match self {
            ThreadAction::Archive => (vec![], vec![s("INBOX")]),
            ThreadAction::MoveToInbox => (vec![s("INBOX")], vec![]),
            ThreadAction::Trash => (vec![s("TRASH")], vec![s("INBOX")]),
            ThreadAction::Untrash => (vec![s("INBOX")], vec![s("TRASH")]),
            ThreadAction::MarkRead => (vec![], vec![s("UNREAD")]),
            ThreadAction::MarkUnread => (vec![s("UNREAD")], vec![]),
            ThreadAction::Star => (vec![s("STARRED")], vec![]),
            ThreadAction::Unstar => (vec![], vec![s("STARRED")]),
            ThreadAction::AddLabel { label_id } => (vec![label_id.clone()], vec![]),
            ThreadAction::RemoveLabel { label_id } => (vec![], vec![label_id.clone()]),
            ThreadAction::ReplyLater { label_id } => {
                (vec![label_id.clone()], vec![s("INBOX"), s("UNREAD")])
            }
            ThreadAction::ReportSpam => (vec![s("SPAM")], vec![s("INBOX")]),
            ThreadAction::NotSpam => (vec![s("INBOX")], vec![s("SPAM")]),
        }
    }

    /// Short verb for error messages ("Couldn't archive …").
    pub fn verb(&self) -> &'static str {
        match self {
            ThreadAction::Archive => "archive",
            ThreadAction::MoveToInbox => "move to inbox",
            ThreadAction::Trash => "trash",
            ThreadAction::Untrash => "restore",
            ThreadAction::MarkRead => "mark as read",
            ThreadAction::MarkUnread => "mark as unread",
            ThreadAction::Star => "star",
            ThreadAction::Unstar => "unstar",
            ThreadAction::AddLabel { .. } => "label",
            ThreadAction::RemoveLabel { .. } => "remove the label from",
            ThreadAction::ReplyLater { .. } => "move to Reply Later",
            ThreadAction::ReportSpam => "report as spam",
            ThreadAction::NotSpam => "move out of Spam",
        }
    }
}

/// Payload of `penguin://mail-changed`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailChangedEvent {
    pub account_id: String,
    pub thread_ids: Vec<String>,
}

/// The browser sign-in waiting for the user: `sign_in_link`'s result and the
/// payload of `penguin://sign-in-url`. `url` is Google's authorization URL
/// (client id, loopback redirect, PKCE challenge, state, login hint: nothing
/// that redeems a token on its own); never logged. `error`: why Penguin
/// couldn't open the browser with it, null when it opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInLink {
    pub url: String,
    pub error: Option<String>,
}

/// Payload of `penguin://action-failed`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionFailedEvent {
    pub message: String,
}

/// Payload of `penguin://body-fetch-failed`: opening a thread started a
/// download of its headers-only messages, and it failed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyFetchFailedEvent {
    pub account_id: String,
    pub message_ids: Vec<String>,
    /// Why, worded for the user.
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub account_id: String,
    pub id: String,
    pub thread_id: String,
    pub date: i64,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: String,
    pub snippet: String,
    pub body_text: String,
    /// Sanitized full HTML document for a sandboxed iframe srcdoc.
    pub html: String,
    pub blocked_remote_images: u32,
    pub trackers_removed: u32,
    /// Every tracker found (host + path, never the query) with what happened
    /// to it, for the privacy details dialog. Capped
    /// (penguin_render::MAX_TRACKER_ENTRIES).
    pub trackers: Vec<TrackerRemoved>,
    /// Trackers found but not removed ("Block tracking pixels" off): held
    /// back with the remote images, or loaded with them.
    pub trackers_allowed: u32,
    /// Links whose tracking parameters were removed ("Remove tracking from
    /// links" on).
    pub links_cleaned: u32,
    /// Names of the removed link parameters (never their values).
    pub link_params: Vec<String>,
    /// Links that go through a click-tracking redirect.
    pub tracked_links: u32,
    pub label_ids: Vec<String>,
    pub attachments: Vec<AttachmentMeta>,
    pub unread: bool,
    pub starred: bool,
    /// The sender is on the "always load images" list, but this message's
    /// sender couldn't be verified, so images were not loaded automatically.
    pub trusted_sender_unverified: bool,
    /// Only headers are stored (mail older than the sync window); the body
    /// is being downloaded and a mail-changed follows when it lands.
    pub body_pending: bool,
    /// Gmail's MX authenticated the From address (DMARC pass or aligned
    /// DKIM). The UI passes it to avatar_lookup: brand logos only show on
    /// authenticated messages.
    pub sender_authenticated: bool,
    /// Verification code / sign-in link in this message (penguin_core::otp).
    pub otp: Option<Otp>,
    /// What the Unsubscribe button does (penguin_core::unsubscribe);
    /// `unsubscribed` is filled by `unsubscribe::annotate`.
    pub unsubscribe: Option<UnsubscribeOffer>,
    /// Read receipts that came back for this message you sent, oldest
    /// first (filled by `receipts::annotate`; see docs/PRIVACY.md).
    pub read_receipts: Vec<penguin_core::store::ReadReceipt>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
    pub label_ids: Vec<String>,
    pub messages: Vec<MessageView>,
}

/// The tracking protections a render applies (Settings → Privacy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Protection {
    pub block_tracking_pixels: bool,
    pub strip_link_tracking: bool,
}

impl Default for Protection {
    /// Penguin's defaults: pixels blocked, links left as sent.
    fn default() -> Self {
        Protection {
            block_tracking_pixels: true,
            strip_link_tracking: false,
        }
    }
}

/// Render one message. The raw `body_html` never leaves this function.
pub fn message_view(
    m: Message,
    allow_remote_images: bool,
    protection: Protection,
    cid_map: HashMap<String, String>,
) -> MessageView {
    let rendered = match m.body_html.as_deref() {
        Some(html) if !html.trim().is_empty() => render_html(
            html,
            &RenderOptions {
                allow_remote_images,
                cid_map,
                allow_tracking_pixels: !protection.block_tracking_pixels,
                strip_link_tracking: protection.strip_link_tracking,
            },
        ),
        _ => render_text(&m.body_text),
    };
    let unread = m.is_unread();
    let starred = m.is_starred();
    let sender_authenticated = m.sender_authenticated;
    let otp = penguin_core::otp::detect_message(&m);
    // The Unsubscribe button acts on the link as the sender wrote it, even
    // when the shown one had its tracking parameters removed.
    let unsubscribe = penguin_core::unsubscribe::plan(&m, || {
        penguin_render::unsubscribe::find_unsubscribe_link(&rendered.html)
            .map(|link| rendered.original_link(&link))
    })
    .map(|p| p.offer);
    MessageView {
        account_id: m.account_id,
        id: m.id,
        thread_id: m.thread_id,
        date: m.date,
        from: m.from,
        to: m.to,
        cc: m.cc,
        bcc: m.bcc,
        reply_to: m.reply_to,
        subject: m.subject,
        snippet: m.snippet,
        body_text: m.body_text,
        html: rendered.html,
        blocked_remote_images: rendered.blocked_remote_images,
        trackers_removed: rendered.trackers_removed,
        trackers: rendered.trackers,
        trackers_allowed: rendered.trackers_allowed,
        links_cleaned: rendered.links_cleaned,
        link_params: rendered.link_params,
        tracked_links: rendered.tracked_links,
        label_ids: m.label_ids,
        attachments: m.attachments,
        unread,
        starred,
        trusted_sender_unverified: false,
        body_pending: false,
        sender_authenticated,
        otp,
        unsubscribe,
        read_receipts: Vec::new(),
    }
}

/// `cid_maps` is keyed by message id. `image_policy` decides per message
/// whether remote images load on first render (the user's setting).
pub fn thread_view(
    t: ThreadDetail,
    mut cid_maps: HashMap<String, HashMap<String, String>>,
    image_policy: impl Fn(&Message) -> RemoteImageDecision,
    protection: Protection,
) -> ThreadView {
    ThreadView {
        account_id: t.account_id,
        thread_id: t.thread_id,
        subject: t.subject,
        label_ids: t.label_ids,
        messages: t
            .messages
            .into_iter()
            .map(|m| {
                let cids = cid_maps.remove(&m.id).unwrap_or_default();
                let decision = image_policy(&m);
                let mut view = message_view(
                    m,
                    decision == RemoteImageDecision::Load,
                    protection,
                    cids,
                );
                view.trusted_sender_unverified =
                    decision == RemoteImageDecision::BlockUnverifiedSender;
                view
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_action_wire_format() {
        let a: ThreadAction =
            serde_json::from_str(r#"{"kind":"addLabel","labelId":"Label_1"}"#).unwrap();
        assert_eq!(
            a,
            ThreadAction::AddLabel {
                label_id: "Label_1".into()
            }
        );
        let a: ThreadAction = serde_json::from_str(r#"{"kind":"moveToInbox"}"#).unwrap();
        assert_eq!(a, ThreadAction::MoveToInbox);
        let r: ThreadRef =
            serde_json::from_str(r#"{"accountId":"a@x.example","threadId":"t1"}"#).unwrap();
        assert_eq!(r.thread_id, "t1");
    }

    #[test]
    fn reply_later_is_label_archive_and_read_in_one_delta() {
        let a: ThreadAction =
            serde_json::from_str(r#"{"kind":"replyLater","labelId":"f:Reply%20Later"}"#).unwrap();
        assert_eq!(
            a.local_delta(),
            (
                vec!["f:Reply%20Later".to_string()],
                vec!["INBOX".to_string(), "UNREAD".to_string()]
            )
        );
        assert_eq!(a.verb(), "move to Reply Later");
    }

    #[test]
    fn spam_actions_move_between_spam_and_the_inbox() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let a: ThreadAction = serde_json::from_str(r#"{"kind":"reportSpam"}"#).unwrap();
        assert_eq!(a, ThreadAction::ReportSpam);
        assert_eq!(a.local_delta(), (s(&["SPAM"]), s(&["INBOX"])));
        let a: ThreadAction = serde_json::from_str(r#"{"kind":"notSpam"}"#).unwrap();
        assert_eq!(a, ThreadAction::NotSpam);
        assert_eq!(a.local_delta(), (s(&["INBOX"]), s(&["SPAM"])));
        assert_eq!(a.verb(), "move out of Spam");
    }
}
