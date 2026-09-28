//! Inline (`cid:`) images in outgoing mail, provider-neutral.
//!
//! An attachment with a `content_id` is an inline image when the draft's
//! HTML shows it (`<img src="cid:…">`). Outgoing MIME ([`crate::compose`])
//! then puts it in a `multipart/related` part next to the HTML; the Graph
//! provider uploads it with `isInline` and `contentId`. One that the HTML
//! doesn't show goes out as an ordinary attachment, so nothing the user
//! attached is ever dropped silently.
//!
//! Security: the HTML only ever references `cid:` ids of parts the message
//! itself carries ([`penguin_render::CidMap`]); remote, `data:` and relative
//! image URLs never survive. Ids written into HTML and `Content-ID` headers
//! are [`penguin_render::is_safe_cid`]; anything else gets a fresh id.
//!
//! Also here: re-pointing a saved draft's attachments at the stored copy
//! ([`saved_refs`]), reopening a draft with its images ([`reopen`]) and the
//! images of an original a reply or forward quotes ([`quote_images`]).

use std::collections::HashSet;

use penguin_core::{AttachmentMeta, Message};
use penguin_render::{
    cid_map, is_safe_cid, keep_cids, normalize_cid, referenced_cids,
    sanitize_compose_html_with_images, sanitize_outgoing_html_with_images,
    sanitize_quoted_html_with_images, CidMap,
};

use crate::compose::{AttachmentBytes, OutgoingAttachment};

/// At most this many bytes of a quoted original's inline images ride along
/// with a reply or forward; images past it are left out of the quote (as
/// remote images always are), so a long newsletter can't push the message
/// over the 25 MB limit by itself.
pub const MAX_QUOTED_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// A fresh Content-ID for an inline image (random; nothing about the
/// machine or the sender).
pub fn new_content_id() -> String {
    format!(
        "img-{:016x}{:08x}@penguin",
        fastrand::u64(..),
        fastrand::u32(..)
    )
}

/// Image types sent and shown inline (what penguin-render displays as
/// `data:` URLs); anything else goes as a file.
pub fn inline_image_type(mime: &str) -> bool {
    matches!(
        mime.trim().to_ascii_lowercase().as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/avif"
    )
}

fn bare(cid: &str) -> &str {
    cid.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
}

/// A usable inline id for an attachment of this type, or None.
fn usable(cid: Option<&str>, mime: &str) -> Option<String> {
    let cid = bare(cid?);
    (is_safe_cid(cid) && inline_image_type(mime)).then(|| cid.to_string())
}

/// How a draft goes out: its HTML part (sanitized, keeping the images it
/// may show) and, per attachment, whether it's an inline part.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub html: Option<String>,
    pub inline: Vec<bool>,
}

/// The plan for `body_html` and attachments given as (content id, MIME
/// type), in order. An attachment is inline when it has a usable id the
/// sanitized HTML references (the first one with that id, if several do).
pub fn plan(body_html: Option<&str>, parts: &[(Option<&str>, &str)]) -> Plan {
    let Some(raw) = body_html.filter(|h| !h.trim().is_empty()) else {
        return Plan {
            html: None,
            inline: vec![false; parts.len()],
        };
    };
    let ids: Vec<Option<String>> = parts.iter().map(|(c, m)| usable(*c, m)).collect();
    let html = sanitize_outgoing_html_with_images(raw, &keep_cids(ids.iter().flatten()));
    let shown: HashSet<String> = referenced_cids(&html).into_iter().collect();
    let mut used = HashSet::new();
    let inline = ids
        .iter()
        .map(|id| match id {
            Some(id) => shown.contains(id) && used.insert(id.clone()),
            None => false,
        })
        .collect();
    Plan {
        html: Some(html),
        inline,
    }
}

/// A saved draft's attachments as the provider stored them (`remote`, of
/// message `message_id`), as refs in the order the composer sent them
/// (`sent`): inline parts matched by Content-ID, files in order. Empty
/// when they don't line up, so the composer keeps what it has (and the
/// next save sends the bytes again).
pub fn saved_refs(
    message_id: &str,
    remote: &[AttachmentMeta],
    sent: &[AttachmentBytes],
) -> Vec<OutgoingAttachment> {
    let mut taken = vec![false; remote.len()];
    let mut files = remote
        .iter()
        .enumerate()
        .filter(|(_, a)| !a.inline)
        .map(|(i, _)| i)
        .collect::<Vec<_>>()
        .into_iter();
    let mut out = Vec::with_capacity(sent.len());
    for s in sent {
        let by_cid = s.content_id.as_deref().and_then(|cid| {
            let want = normalize_cid(cid);
            remote.iter().enumerate().position(|(i, a)| {
                !taken[i]
                    && a.inline
                    && a.content_id.as_deref().map(normalize_cid).as_deref() == Some(want.as_str())
            })
        });
        let i = match by_cid {
            Some(i) => i,
            None => match files.by_ref().find(|&i| !taken[i]) {
                Some(i) => i,
                None => return Vec::new(),
            },
        };
        taken[i] = true;
        let a = &remote[i];
        out.push(OutgoingAttachment::Gmail {
            message_id: message_id.to_string(),
            attachment_id: a.id.clone(),
            filename: s.filename.clone(),
            mime_type: s.mime_type.clone(),
            size: a.size,
            account_id: None,
            content_id: by_cid.and(s.content_id.clone()),
        });
    }
    out
}

/// A stored draft reopened for the composer: its HTML through the editor's
/// allowlist keeping the inline images it shows, and its attachments as
/// refs (files, then those images with their Content-IDs, in stored order).
/// Inline parts the HTML doesn't show are left out, as before; an image
/// whose id isn't [`is_safe_cid`] gets a fresh one.
pub fn reopen(message: &Message) -> (Option<String>, Vec<OutgoingAttachment>) {
    let ids: Vec<Option<(String, String)>> = message
        .attachments
        .iter()
        .map(|a| {
            if !a.inline || !inline_image_type(&a.mime_type) {
                return None;
            }
            let orig = bare(a.content_id.as_deref()?).to_string();
            let out = if is_safe_cid(&orig) {
                orig.clone()
            } else {
                new_content_id()
            };
            Some((orig, out))
        })
        .collect();
    let map = cid_map(ids.iter().flatten().cloned());
    let html = message
        .body_html
        .as_deref()
        .map(|h| sanitize_compose_html_with_images(h, &map))
        .filter(|h| !h.trim().is_empty());
    let shown: HashSet<String> = html
        .as_deref()
        .map(referenced_cids)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let refs = message
        .attachments
        .iter()
        .zip(&ids)
        .filter_map(|(a, id)| {
            let content_id = match id {
                Some((_, out)) if shown.contains(out) => Some(out.clone()),
                Some(_) => return None,
                None if a.inline => return None,
                None => None,
            };
            Some(OutgoingAttachment::Gmail {
                message_id: message.id.clone(),
                attachment_id: a.id.clone(),
                filename: a.filename.clone(),
                mime_type: a.mime_type.clone(),
                size: a.size,
                account_id: None,
                content_id,
            })
        })
        .collect();
    (html, refs)
}

/// The inline images of `original` a reply or forward quotes: the map that
/// rewrites its `cid:` references to fresh ids (for
/// [`penguin_render::sanitize_quoted_html_with_images`]), and refs to the
/// original's parts under those ids (`account_id` = the account it's stored
/// in, for sending from another one). Only images its HTML shows, in the
/// order it shows them, up to [`MAX_QUOTED_IMAGE_BYTES`].
pub fn quote_images(
    original: &Message,
    account_id: Option<&str>,
) -> (CidMap, Vec<OutgoingAttachment>) {
    let Some(raw) = original
        .body_html
        .as_deref()
        .filter(|h| !h.trim().is_empty())
    else {
        return (CidMap::new(), Vec::new());
    };
    // Every image part with a Content-ID, under a fresh id.
    let mut fresh: Vec<(String, String, &AttachmentMeta)> = Vec::new();
    for a in &original.attachments {
        let Some(cid) = a.content_id.as_deref().map(bare).filter(|c| !c.is_empty()) else {
            continue;
        };
        if !inline_image_type(&a.mime_type)
            || fresh
                .iter()
                .any(|(o, _, _)| normalize_cid(o) == normalize_cid(cid))
        {
            continue;
        }
        fresh.push((cid.to_string(), new_content_id(), a));
    }
    if fresh.is_empty() {
        return (CidMap::new(), Vec::new());
    }
    let all = cid_map(fresh.iter().map(|(o, n, _)| (o.as_str(), n.clone())));
    let shown = referenced_cids(&sanitize_quoted_html_with_images(raw, &all));
    let mut budget = MAX_QUOTED_IMAGE_BYTES;
    let mut kept = Vec::new();
    for id in shown {
        let Some((orig, new, a)) = fresh.iter().find(|(_, n, _)| *n == id) else {
            continue;
        };
        if a.size > budget {
            continue;
        }
        budget -= a.size;
        kept.push((orig.clone(), new.clone(), *a));
    }
    let map = cid_map(kept.iter().map(|(o, n, _)| (o.as_str(), n.clone())));
    let refs = kept
        .into_iter()
        .map(|(_, new, a)| OutgoingAttachment::Gmail {
            message_id: original.id.clone(),
            attachment_id: a.id.clone(),
            filename: a.filename.clone(),
            mime_type: a.mime_type.clone(),
            size: a.size,
            account_id: account_id.map(str::to_string),
            content_id: Some(new),
        })
        .collect();
    (map, refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, cid: Option<&str>, inline: bool, mime: &str, size: u64) -> AttachmentMeta {
        AttachmentMeta {
            id: id.into(),
            filename: format!("{id}.bin"),
            mime_type: mime.into(),
            size,
            content_id: cid.map(str::to_string),
            inline,
        }
    }

    fn bytes(name: &str, cid: Option<&str>) -> AttachmentBytes {
        AttachmentBytes {
            filename: name.into(),
            mime_type: "image/png".into(),
            bytes: vec![1, 2, 3],
            content_id: cid.map(str::to_string),
        }
    }

    fn message(html: &str, atts: Vec<AttachmentMeta>) -> Message {
        Message {
            account_id: "me@penguin.example".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 0,
            from: penguin_core::Address {
                name: None,
                email: "dana@acme.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "s".into(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: Some(html.into()),
            label_ids: vec![],
            attachments: atts,
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    #[test]
    fn plan_marks_only_shown_images_inline() {
        let html = r#"<p><img src="cid:a@x"><img src="cid:a@x"><img src="cid:gone@x"></p>"#;
        let p = plan(
            Some(html),
            &[
                (Some("<a@x>"), "image/png"),
                (Some("a@x"), "image/png"),
                (Some("b@x"), "image/png"),
                (None, "application/pdf"),
                (Some("svg@x"), "image/svg+xml"),
            ],
        );
        assert_eq!(p.inline, vec![true, false, false, false, false]);
        let html = p.html.unwrap();
        assert_eq!(html.matches("cid:a@x").count(), 2, "{html}");
        assert!(!html.contains("gone"), "{html}");
        // No HTML part: nothing is inline.
        assert_eq!(
            plan(None, &[(Some("a@x"), "image/png")]).inline,
            vec![false]
        );
    }

    #[test]
    fn saved_refs_match_inline_by_id_and_files_in_order() {
        let remote = vec![
            meta("r-img2", Some("<B@X>"), true, "image/png", 30),
            meta("r-img1", Some("a@x"), true, "image/png", 20),
            meta("r-f1", None, false, "application/pdf", 10),
            meta("r-f2", None, false, "text/plain", 11),
        ];
        let sent = vec![
            bytes("f1.pdf", None),
            bytes("one.png", Some("a@x")),
            bytes("f2.txt", None),
            bytes("two.png", Some("b@x")),
        ];
        let refs = saved_refs("dm-1", &remote, &sent);
        let ids: Vec<(String, Option<String>)> = refs
            .iter()
            .map(|r| match r {
                OutgoingAttachment::Gmail {
                    attachment_id,
                    content_id,
                    ..
                } => (attachment_id.clone(), content_id.clone()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                ("r-f1".into(), None),
                ("r-img1".into(), Some("a@x".into())),
                ("r-f2".into(), None),
                ("r-img2".into(), Some("b@x".into())),
            ]
        );
        // An image sent as a file (its HTML no longer showed it) takes the
        // next file slot; one too many is "can't line up".
        let refs = saved_refs(
            "dm-1",
            &remote[2..],
            &[bytes("x.png", Some("z@x")), bytes("y", None)],
        );
        assert_eq!(refs.len(), 2);
        assert!(
            saved_refs("dm-1", &remote[2..3], &[bytes("a", None), bytes("b", None)]).is_empty()
        );
    }

    #[test]
    fn reopen_keeps_shown_images_and_renames_unsafe_ids() {
        let m = message(
            r#"<p>hi <img src="cid:ok@x"><img src="cid:we%20ird@x"><img src="https://t.example/p.gif"></p>"#,
            vec![
                meta("a1", Some("ok@x"), true, "image/png", 5),
                meta("a2", Some("we ird@x"), true, "image/jpeg", 6),
                meta("a3", Some("unused@x"), true, "image/png", 7),
                meta("a4", None, false, "application/pdf", 8),
            ],
        );
        let (html, refs) = reopen(&m);
        let html = html.unwrap();
        assert!(html.contains(r#"<img src="cid:ok@x">"#), "{html}");
        assert!(
            !html.contains("we%20ird") && !html.contains("https:"),
            "{html}"
        );
        let cids = referenced_cids(&html);
        assert_eq!(cids.len(), 2);
        assert!(cids[1].starts_with("img-") && cids[1].ends_with("@penguin"));
        let got: Vec<(String, Option<String>)> = refs
            .iter()
            .map(|r| match r {
                OutgoingAttachment::Gmail {
                    attachment_id,
                    content_id,
                    ..
                } => (attachment_id.clone(), content_id.clone()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("a1".into(), Some("ok@x".into())),
                ("a2".into(), Some(cids[1].clone())),
                ("a4".into(), None),
            ]
        );
    }

    #[test]
    fn quote_images_rewrites_to_fresh_ids_within_the_budget() {
        let big = MAX_QUOTED_IMAGE_BYTES - 100;
        let m = message(
            r#"<img src="cid:big@x"><img src="cid:logo@x"><img src="cid:small@x"><img src="cid:doc@x">"#,
            vec![
                meta("b", Some("big@x"), true, "image/png", big),
                meta("l", Some("<logo@x>"), false, "image/gif", 200),
                meta("s", Some("small@x"), true, "image/png", 50),
                meta("d", Some("doc@x"), true, "application/pdf", 5),
                meta("n", Some("notshown@x"), true, "image/png", 5),
            ],
        );
        let (map, refs) = quote_images(&m, Some("dana@acme.example"));
        // big fits, logo (200) doesn't after it, small (50) does.
        let kept: Vec<&str> = refs
            .iter()
            .map(|r| match r {
                OutgoingAttachment::Gmail { attachment_id, .. } => attachment_id.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(kept, vec!["b", "s"]);
        assert_eq!(map.len(), 2);
        let q = sanitize_quoted_html_with_images(m.body_html.as_deref().unwrap(), &map);
        assert_eq!(q.matches("<img").count(), 2, "{q}");
        assert!(!q.contains("big@x") && !q.contains("small@x"), "{q}");
        for r in &refs {
            let OutgoingAttachment::Gmail {
                content_id,
                account_id,
                message_id,
                ..
            } = r
            else {
                unreachable!()
            };
            assert!(q.contains(&format!("cid:{}", content_id.as_deref().unwrap())));
            assert_eq!(account_id.as_deref(), Some("dana@acme.example"));
            assert_eq!(message_id, "m1");
        }
    }
}
