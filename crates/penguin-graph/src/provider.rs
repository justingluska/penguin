//! [`GraphProvider`]: one Microsoft account's `MailProvider`
//! (docs/PROVIDERS-IMPL.md §2.1). Label deltas become Graph operations
//! (§4): UNREAD → `isRead`, STARRED → `flag`, `c:` → `categories`, and
//! folder labels (INBOX, SPAM, TRASH, `f:<id>`, removing INBOX = archive)
//! → moves. After a move the location table and the message's folder
//! label are fixed locally at once; the sync the app pokes afterwards
//! converges everything else.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use penguin_core::query::ParsedQuery;
use penguin_core::{AccountProvider, Address, AttachmentMeta, Message, SearchHit, SentRef};
use penguin_provider::compose::{AttachmentBytes, Draft};
use penguin_provider::ids::system;
use penguin_provider::window::window_start_ms;
use penguin_provider::{
    async_trait, DraftRef, Error, MailProvider, MessageMetadata, OpenedDraft, Result, ServerSearch,
};
use serde_json::{json, Map, Value};

use crate::client::{enc, Fetched, GraphClient};
use crate::convert::format_date;
use crate::folders::{category_of_label, folder_id_of_label, is_folder_label, FolderMap, Role};
use crate::http::{is_gone, Body, Method, Retry};
use crate::locations;
use crate::wire::Created;

/// Messages listed per server search.
const SEARCH_LIST: u32 = 100;

/// One Microsoft account through the provider seam.
#[derive(Clone)]
pub struct GraphProvider {
    client: Arc<GraphClient>,
}

/// Where a stored message is, as far as moves are concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Place {
    System(&'static str),
    Archive,
    User(String),
    Unknown,
}

fn place_of(message: &Message, folder_id: Option<&String>, map: &FolderMap) -> Place {
    if let Some(f) = folder_id.and_then(|id| map.get(id)) {
        return match &f.role {
            Role::System(l) => Place::System(l),
            Role::Archive => Place::Archive,
            Role::User => Place::User(f.id.clone()),
            Role::Skip => Place::Unknown,
        };
    }
    for l in &message.label_ids {
        match l.as_str() {
            system::INBOX => return Place::System(system::INBOX),
            system::SENT => return Place::System(system::SENT),
            system::DRAFT => return Place::System(system::DRAFT),
            system::TRASH => return Place::System(system::TRASH),
            system::SPAM => return Place::System(system::SPAM),
            other => {
                if let Some(id) = folder_id_of_label(other) {
                    return Place::User(id);
                }
            }
        }
    }
    Place::Archive
}

fn is_mine(message: &Message, account_id: &str) -> bool {
    message.from.email.eq_ignore_ascii_case(account_id)
}

impl GraphProvider {
    pub fn new(client: Arc<GraphClient>) -> GraphProvider {
        GraphProvider { client }
    }

    fn acct(&self) -> &str {
        self.client.account_id()
    }

    /// The thread's stored messages with their locations (NotFound when
    /// the thread isn't stored).
    async fn thread(&self, thread_id: &str) -> Result<Vec<(Message, Option<String>)>> {
        let (acct, t) = (self.acct().to_string(), thread_id.to_string());
        let found = self
            .client
            .db(move |s| {
                let Some(detail) = s.get_thread(&acct, &t)? else {
                    return Ok(None);
                };
                let ids: Vec<String> = detail.messages.iter().map(|m| m.id.clone()).collect();
                let locs = locations::get_many(s, &acct, &ids)?;
                Ok(Some(
                    detail
                        .messages
                        .into_iter()
                        .map(|m| {
                            let loc = locs.get(&m.id).cloned();
                            (m, loc)
                        })
                        .collect::<Vec<_>>(),
                ))
            })
            .await?;
        match found {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Err(Error::NotFound(format!(
                "conversation {thread_id} not found"
            ))),
        }
    }

    /// The archive's destination id: the well-known Archive, a root folder
    /// named Archive, or one created now (older mailboxes have none).
    async fn archive_destination(&self) -> Result<String> {
        let map = self.client.folders().await?;
        if map.well_known("archive").is_some() {
            return Ok("archive".into());
        }
        if let Some(f) = map.synced().find(|f| f.role == Role::Archive) {
            return Ok(f.id.clone());
        }
        let created: Created = self
            .client
            .api
            .call(
                Method::Post,
                "/me/mailFolders",
                &[],
                Body::Json(json!({ "displayName": "Archive" })),
                &[],
                Retry::OnlyIfRejected,
            )
            .await?
            .json("Archive folder")?;
        self.client.refresh_folders().await?;
        Ok(created.id)
    }

    /// Move messages to `destination` (a well-known name or folder id),
    /// then record their new place locally. Messages already gone are
    /// skipped; the first other failure is returned after the rest ran.
    async fn move_all(&self, moves: Vec<(Message, String)>) -> Result<()> {
        if moves.is_empty() {
            return Ok(());
        }
        let results = join_all(
            moves
                .iter()
                .map(|(m, dest)| self.client.move_to(&m.id, dest)),
        )
        .await;
        let map = self.client.folders().await?;
        let mut first_err = None;
        let mut located = Vec::new();
        let mut relabel: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
        for ((m, dest), r) in moves.iter().zip(results) {
            match r {
                Ok(landed) => {
                    let folder = landed.or_else(|| map.resolve(dest));
                    let new_label = folder
                        .as_deref()
                        .and_then(|f| map.get(f))
                        .and_then(|f| f.label());
                    let remove: Vec<String> = m
                        .label_ids
                        .iter()
                        .filter(|l| is_folder_label(l) && Some(*l) != new_label.as_ref())
                        .cloned()
                        .collect();
                    relabel.push((m.id.clone(), new_label.into_iter().collect(), remove));
                    if let Some(f) = folder {
                        located.push((m.id.clone(), f));
                    }
                }
                Err(e) if is_gone(&e) => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        let acct = self.acct().to_string();
        self.client
            .db(move |s| {
                locations::set(s, &acct, &located)?;
                for (id, add, remove) in relabel {
                    s.modify_message_labels(&acct, std::slice::from_ref(&id), &add, &remove)?;
                }
                Ok(())
            })
            .await?;
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn patch_all(&self, patches: Vec<(String, Value)>) -> Result<()> {
        let results = join_all(
            patches
                .iter()
                .map(|(id, body)| self.client.patch(id, body.clone())),
        )
        .await;
        for r in results {
            match r {
                Ok(_) => {}
                Err(e) if is_gone(&e) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[async_trait]
impl MailProvider for GraphProvider {
    fn account_id(&self) -> &str {
        self.acct()
    }

    fn provider(&self) -> AccountProvider {
        AccountProvider::Microsoft
    }

    async fn fetch_messages(&self, ids: &[String]) -> Vec<(String, Result<Option<Message>>)> {
        self.client
            .fetch_full_each(ids)
            .await
            .into_iter()
            .map(|(id, r)| (id, r.map(|f| f.map(|f| f.message))))
            .collect()
    }

    async fn fetch_pending_bodies(&self, ids: &[String]) -> Result<Vec<String>> {
        let acct = self.acct().to_string();
        let claim = self.client.claim_bodies(ids);
        let claimed = claim.ids.clone();
        if claimed.is_empty() {
            return Ok(vec![]);
        }
        let result = async {
            let (a, probe) = (acct.clone(), claimed.clone());
            let pending: Vec<String> = self
                .client
                .db(move |s| {
                    let mut out = Vec::new();
                    for id in probe {
                        if s.is_body_pending(&a, &id)? == Some(true) {
                            out.push(id);
                        }
                    }
                    Ok(out)
                })
                .await?;
            if pending.is_empty() {
                return Ok(vec![]);
            }
            let mut fetched = Vec::new();
            let mut last_err = None;
            for (id, r) in self.client.fetch_full_each(&pending).await {
                match r {
                    Ok(Some(f)) => fetched.push(f),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(account = %acct, message = %id, error = %e, "could not download a message body");
                        last_err = Some(e);
                    }
                }
            }
            let threads = self.client.store_full(fetched).await?;
            match (threads.is_empty(), last_err) {
                (true, Some(e)) => Err(e),
                _ => Ok(threads),
            }
        }
        .await;
        drop(claim);
        result
    }

    async fn get_attachment(
        &self,
        message_id: &str,
        attachment: &AttachmentMeta,
    ) -> Result<Vec<u8>> {
        match self
            .client
            .api
            .call(
                Method::Get,
                &format!(
                    "/me/messages/{}/attachments/{}/$value",
                    enc(message_id),
                    enc(&attachment.id)
                ),
                &[],
                Body::None,
                &[],
                Retry::Idempotent,
            )
            .await
        {
            Ok(resp) => Ok(resp.body),
            Err(e) if is_gone(&e) => Err(Error::NotFound(
                "Outlook no longer has this attachment".into(),
            )),
            Err(e) => Err(e),
        }
    }

    async fn get_message_metadata(&self, message_id: &str) -> Result<Option<MessageMetadata>> {
        Ok(self
            .client
            .headers(message_id)
            .await?
            .map(|(headers, size_estimate)| MessageMetadata {
                headers,
                size_estimate,
            }))
    }

    async fn get_message_raw(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        self.client.raw(message_id).await
    }

    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let messages = self.thread(thread_id).await?;
        let map = self.client.folders().await?;
        let acct = self.acct().to_string();

        // Flags and categories: every message of the thread (as Gmail does),
        // so the optimistic local state and the server agree.
        let has = |list: &[String], l: &str| list.iter().any(|x| x == l);
        let cat_add: Vec<String> = add.iter().filter_map(|l| category_of_label(l)).collect();
        let cat_remove: Vec<String> = remove.iter().filter_map(|l| category_of_label(l)).collect();
        let mut patches = Vec::new();
        for (m, _) in &messages {
            let mut body = Map::new();
            if has(add, system::UNREAD) {
                body.insert("isRead".into(), Value::Bool(false));
            } else if has(remove, system::UNREAD) {
                body.insert("isRead".into(), Value::Bool(true));
            }
            if has(add, system::STARRED) {
                body.insert("flag".into(), json!({ "flagStatus": "flagged" }));
            } else if has(remove, system::STARRED) {
                body.insert("flag".into(), json!({ "flagStatus": "notFlagged" }));
            }
            if !cat_add.is_empty() || !cat_remove.is_empty() {
                // The local labels already carry the optimistic change.
                let mut cats: BTreeSet<String> = m
                    .label_ids
                    .iter()
                    .filter_map(|l| category_of_label(l))
                    .collect();
                cats.extend(cat_add.iter().cloned());
                for c in &cat_remove {
                    cats.remove(c);
                }
                body.insert(
                    "categories".into(),
                    Value::Array(cats.into_iter().map(Value::String).collect()),
                );
            }
            if !body.is_empty() {
                patches.push((m.id.clone(), Value::Object(body)));
            }
        }
        self.patch_all(patches).await?;

        // Moves. An explicit destination wins over the archive a removal implies.
        let dest_label = add.iter().find(|l| {
            matches!(l.as_str(), system::INBOX | system::SPAM | system::TRASH)
                || l.starts_with("f:")
        });
        let dest = match dest_label {
            Some(l) => Some(map.destination_for_label(l).ok_or_else(|| {
                Error::NotFound(format!("folder {l} no longer exists in Outlook"))
            })?),
            None => None,
        };
        // Placeholder for the archive, resolved (and for a mailbox without
        // one, created) only when a message actually goes there.
        const ARCHIVE: &str = "\u{0}archive";
        let mut moves = Vec::new();
        for (m, loc) in &messages {
            let place = place_of(m, loc.as_ref(), &map);
            let outgoing = matches!(
                place,
                Place::System(system::SENT) | Place::System(system::DRAFT)
            );
            let target = if let (Some(d), Some(label)) = (&dest, dest_label) {
                let here = match &place {
                    Place::System(l) => *l == label.as_str(),
                    Place::User(id) => folder_id_of_label(label).as_deref() == Some(id.as_str()),
                    _ => false,
                };
                if here || (outgoing && label != system::TRASH) {
                    None
                } else {
                    Some(d.clone())
                }
            } else {
                let leaving = match &place {
                    Place::System(l) => has(remove, l),
                    Place::User(id) => remove
                        .iter()
                        .any(|l| folder_id_of_label(l).as_deref() == Some(id.as_str())),
                    _ => false,
                };
                match (&place, leaving) {
                    (Place::System(system::SPAM), true) | (Place::System(system::TRASH), true) => {
                        Some("inbox".to_string())
                    }
                    (_, true) if !outgoing => Some(ARCHIVE.to_string()),
                    _ => None,
                }
            };
            if let Some(t) = target {
                moves.push((m.clone(), t));
            }
        }
        if moves.iter().any(|(_, t)| t == ARCHIVE) {
            let archive = self.archive_destination().await?;
            for (_, t) in moves.iter_mut().filter(|(_, t)| t == ARCHIVE) {
                *t = archive.clone();
            }
        }
        let ignored: Vec<&String> = add
            .iter()
            .chain(remove)
            .filter(|l| matches!(l.as_str(), system::SENT | system::DRAFT | system::IMPORTANT))
            .collect();
        if !ignored.is_empty() {
            tracing::debug!(account = %acct, ?ignored, "label changes with no Outlook equivalent ignored");
        }
        self.move_all(moves).await
    }

    /// A category, which needs no call: Outlook shows a category once a
    /// message carries one (adding it to the master list would need
    /// MailboxSettings.ReadWrite). Categories don't move mail, so Reply
    /// Later pairs this with an archive.
    async fn ensure_label(&self, name: &str) -> Result<penguin_core::Label> {
        Ok(penguin_core::Label {
            account_id: self.acct().to_string(),
            id: crate::folders::category_label_id(name),
            name: name.to_string(),
            kind: "user".into(),
            color: None,
            unread_count: None,
            hidden: false,
        })
    }

    async fn trash_thread(&self, thread_id: &str) -> Result<()> {
        let messages = self.thread(thread_id).await?;
        let map = self.client.folders().await?;
        let moves = messages
            .into_iter()
            .filter(|(m, loc)| place_of(m, loc.as_ref(), &map) != Place::System(system::TRASH))
            .map(|(m, _)| (m, "deleteditems".to_string()))
            .collect();
        self.move_all(moves).await
    }

    /// Out of Deleted Items: your own messages back to Sent Items,
    /// everything else to the inbox (Outlook doesn't remember the old folder).
    async fn untrash_thread(&self, thread_id: &str) -> Result<()> {
        let messages = self.thread(thread_id).await?;
        let map = self.client.folders().await?;
        let acct = self.acct().to_string();
        let moves = messages
            .into_iter()
            .filter(|(m, loc)| place_of(m, loc.as_ref(), &map) == Place::System(system::TRASH))
            .map(|(m, _)| {
                let dest = if is_mine(&m, &acct) {
                    "sentitems"
                } else {
                    "inbox"
                };
                (m, dest.to_string())
            })
            .collect();
        self.move_all(moves).await
    }

    async fn send_raw(&self, raw: &[u8], thread_id: Option<&str>) -> Result<SentRef> {
        self.client.send_mime(raw, thread_id).await
    }

    async fn respond_to_invitation(
        &self,
        message_id: &str,
        answer: &penguin_provider::InvitationAnswer,
    ) -> Result<()> {
        self.client.respond_to_invitation(message_id, answer).await
    }

    async fn save_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: Option<&str>,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        self.client
            .save_draft(draft, from, draft_id, attachments)
            .await
    }

    async fn send_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: &str,
        attachments: &[AttachmentBytes],
    ) -> Result<DraftRef> {
        self.client
            .send_draft(draft, from, draft_id, attachments)
            .await
    }

    async fn send_saved_draft(&self, draft_id: &str) -> Result<SentRef> {
        self.client.send_saved_draft(draft_id).await
    }

    async fn delete_draft(&self, draft_id: &str) -> Result<Option<String>> {
        self.client.delete_draft(draft_id).await
    }

    async fn open_draft(
        &self,
        draft_id: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<Option<OpenedDraft>> {
        self.client.open_draft(draft_id, message_id).await
    }

    async fn server_search(&self, query: &ParsedQuery, fetch_cap: usize) -> Result<ServerSearch> {
        let Some(kql) = crate::search::to_kql(query) else {
            return Ok(ServerSearch::default());
        };
        let page = self
            .client
            .list_messages(
                None,
                &[
                    ("$search", format!("\"{kql}\"")),
                    ("$top", SEARCH_LIST.to_string()),
                ],
                &[],
            )
            .await?;
        let map = self.client.folders().await?;
        let listed: Vec<_> = page
            .value
            .into_iter()
            .filter(|w| w.removed.is_none() && !w.id.is_empty())
            .filter(|w| {
                w.parent_folder_id
                    .as_deref()
                    .is_none_or(|f| map.get(f).is_none() || map.is_synced(f))
            })
            .collect();
        let ids: Vec<String> = listed.iter().map(|w| w.id.clone()).collect();
        let (acct, probe) = (self.acct().to_string(), ids.clone());
        let known = self
            .client
            .db(move |s| s.known_message_ids(&acct, &probe))
            .await?;
        let mut headers: Vec<Fetched> = Vec::new();
        for w in listed
            .iter()
            .filter(|w| !known.contains(&w.id))
            .take(fetch_cap)
        {
            match self.client.convert(w, false).await {
                Ok(f) => headers.push(f),
                Err(e) => {
                    tracing::warn!(account = %self.acct(), error = %e, "server search: skipped a result")
                }
            }
        }
        let (fetched, _) = self.client.store_headers(headers).await?;
        let (acct, want) = (self.acct().to_string(), ids);
        let mut messages: Vec<Message> = self
            .client
            .db(move |s| {
                let mut out = Vec::new();
                for id in &want {
                    if let Some(m) = s.get_message(&acct, id)? {
                        out.push(m);
                    }
                }
                Ok(out)
            })
            .await?;
        messages.sort_by_key(|m| std::cmp::Reverse(m.date));
        let estimate = messages.len() as u64;
        let mut hits: Vec<SearchHit> = Vec::new();
        let mut seen = HashSet::new();
        for m in messages {
            if !seen.insert(m.thread_id.clone()) {
                if let Some(h) = hits.iter_mut().find(|h| h.thread_id == m.thread_id) {
                    h.match_count += 1;
                }
                continue;
            }
            let mut snippet_html = String::new();
            penguin_core::text::escape_html(&m.snippet, &mut snippet_html);
            hits.push(SearchHit {
                account_id: m.account_id.clone(),
                thread_id: m.thread_id.clone(),
                message_id: m.id.clone(),
                subject: m.subject.clone(),
                from: m.from.clone(),
                date: m.date,
                snippet_html,
                match_count: 1,
                has_attachments: m.attachments.iter().any(|a| !a.inline),
                unread: m.is_unread(),
                label_ids: m.label_ids,
                score: 0.0,
                matched_by: Vec::new(),
                passage: None,
            });
        }
        Ok(ServerSearch {
            hits,
            fetched: fetched as u32,
            estimate,
        })
    }

    async fn window_estimate(&self, months: u32, now_ms: i64) -> Result<u64> {
        const TTL: Duration = Duration::from_secs(30 * 60);
        let cached = self
            .client
            .estimates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&months)
            .copied();
        if let Some((n, at)) = cached {
            if at.elapsed() < TTL {
                return Ok(n);
            }
        }
        let n = if months == 0 {
            let map = self.client.refresh_folders().await?;
            map.synced().map(|f| f.total).sum()
        } else {
            let start = window_start_ms(now_ms, months);
            let page = self
                .client
                .api
                .get_json::<crate::wire::Page<crate::wire::WireMessage>>(
                    "/me/messages",
                    &[
                        (
                            "$filter",
                            format!("receivedDateTime ge {}", format_date(start)),
                        ),
                        ("$count", "true".into()),
                        ("$top", "1".into()),
                        ("$select", "id".into()),
                    ],
                    &[],
                    "message count",
                )
                .await?;
            page.count
                .ok_or_else(|| Error::Other("Outlook didn't report a message count".into()))?
        };
        self.client
            .estimates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(months, (n, Instant::now()));
        Ok(n)
    }

    async fn profile_photo(&self) -> Result<Option<Vec<u8>>> {
        match self
            .client
            .api
            .call(
                Method::Get,
                "/me/photo/$value",
                &[],
                Body::None,
                &[],
                Retry::Idempotent,
            )
            .await
        {
            Ok(resp) if !resp.body.is_empty() => Ok(Some(resp.body)),
            Ok(_) => Ok(None),
            Err(e) if is_gone(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
