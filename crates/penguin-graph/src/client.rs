//! One account's Graph mailbox: the typed calls the provider and the sync
//! engine share, the folder map (cached; refreshed when an unknown folder
//! shows up) and store access for messages and their locations.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::future::join_all;
use penguin_core::{Message, Store};
use penguin_provider::{Error, Result};
use serde_json::json;
use tokio::sync::Mutex;

use crate::convert::{self, to_message};
use crate::folders::FolderMap;
use crate::http::{is_gone, Body, GraphApi, Method, Retry};
use crate::locations;
use crate::wire::{
    Category, Created, MailFolder, Page, WireMessage, EXPAND_ATTACHMENTS, SELECT_FULL, SELECT_LIGHT,
};

/// Folders listed per mailbox at most (a runaway tree stops here).
const MAX_FOLDERS: usize = 2000;

/// A message as fetched, with the folder it's in.
#[derive(Debug, Clone)]
pub(crate) struct Fetched {
    pub message: Message,
    pub folder_id: Option<String>,
}

pub struct GraphClient {
    pub(crate) api: GraphApi,
    pub(crate) store: Store,
    account_id: String,
    folders: Mutex<Option<Arc<FolderMap>>>,
}

pub(crate) fn enc(id: &str) -> String {
    // Graph ids are base64 (`+`, `/`, `=`, `-`, `_`); `/` must not split
    // the path. Immutable ids use the URL-safe alphabet, so this is a no-op
    // for them.
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

impl GraphClient {
    pub fn new(api: GraphApi, store: Store) -> GraphClient {
        GraphClient {
            account_id: api.account_id().to_string(),
            api,
            store,
            folders: Mutex::new(None),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(crate) async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> penguin_core::Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|e| Error::Other(format!("store task failed: {e}")))?
            .map_err(Error::from)
    }

    // ----- folders -----

    /// The cached folder map, loading it on first use.
    pub(crate) async fn folders(&self) -> Result<Arc<FolderMap>> {
        let mut cached = self.folders.lock().await;
        if let Some(m) = cached.as_ref() {
            return Ok(m.clone());
        }
        let map = Arc::new(self.load_folders().await?);
        *cached = Some(map.clone());
        Ok(map)
    }

    /// Re-read the folder tree and categories.
    pub(crate) async fn refresh_folders(&self) -> Result<Arc<FolderMap>> {
        let mut cached = self.folders.lock().await;
        let map = Arc::new(self.load_folders().await?);
        *cached = Some(map.clone());
        Ok(map)
    }

    async fn load_folders(&self) -> Result<FolderMap> {
        let mut well_known = HashMap::new();
        for name in crate::folders::WELL_KNOWN {
            match self
                .api
                .get_json::<MailFolder>(
                    &format!("/me/mailFolders/{name}"),
                    &[("$select", "id".into())],
                    &[],
                    "mail folder",
                )
                .await
            {
                Ok(f) => {
                    well_known.insert(name.to_string(), f.id);
                }
                Err(e) if is_gone(&e) => {}
                Err(e) => return Err(e),
            }
        }
        let select =
            "id,displayName,parentFolderId,childFolderCount,totalItemCount,unreadItemCount";
        let mut all: Vec<MailFolder> = Vec::new();
        let mut truncated = false;
        let mut queue: Vec<String> = vec!["/me/mailFolders".into()];
        while let Some(path) = queue.pop() {
            let mut next: Option<String> = None;
            loop {
                let page: Page<MailFolder> = match &next {
                    Some(link) => self.api.get_json(link, &[], &[], "mail folders").await?,
                    None => {
                        self.api
                            .get_json(
                                &path,
                                &[
                                    ("$select", select.into()),
                                    ("$top", "100".into()),
                                    ("includeHiddenFolders", "false".into()),
                                ],
                                &[],
                                "mail folders",
                            )
                            .await?
                    }
                };
                for f in page.value {
                    if f.child_folder_count > 0 {
                        queue.push(format!("/me/mailFolders/{}/childFolders", enc(&f.id)));
                    }
                    all.push(f);
                }
                if all.len() > MAX_FOLDERS {
                    tracing::warn!(account = %self.account_id, "more than {MAX_FOLDERS} mail folders; the rest aren't synced");
                    queue.clear();
                    truncated = true;
                    break;
                }
                match page.next_link {
                    Some(l) => next = Some(l),
                    None => break,
                }
            }
        }
        let categories = match self
            .api
            .get_json::<Page<Category>>("/me/outlook/masterCategories", &[], &[], "categories")
            .await
        {
            Ok(p) => p.value.into_iter().map(|c| c.display_name).collect(),
            // Some mailboxes (and older tenants) refuse this; categories
            // then come only from the messages themselves.
            Err(e)
                if matches!(
                    e,
                    Error::Http {
                        status: 403 | 404 | 400,
                        ..
                    }
                ) =>
            {
                tracing::info!(account = %self.account_id, error = %e, "master categories unavailable");
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let mut map = FolderMap::build(all, well_known, categories);
        map.truncated = truncated;
        Ok(map)
    }

    /// The folder label of a message, refreshing the folder map once when
    /// its folder is new. Returns (label, folder id, whether the folder is
    /// synced).
    pub(crate) async fn place(
        &self,
        w: &WireMessage,
    ) -> Result<(Option<String>, Option<String>, bool)> {
        let mut map = self.folders().await?;
        let Some(fid) = w.parent_folder_id.clone() else {
            return Ok((None, None, true));
        };
        if map.get(&fid).is_none() {
            map = self.refresh_folders().await?;
        }
        match map.get(&fid) {
            Some(f) => Ok((f.label(), Some(fid.clone()), map.is_synced(&fid))),
            // Still unknown (a hidden folder): treat as not synced.
            None => Ok((None, Some(fid), false)),
        }
    }

    /// A wire message as a stored message plus its folder.
    pub(crate) async fn convert(&self, w: &WireMessage, full: bool) -> Result<Fetched> {
        let (label, folder_id, _) = self.place(w).await?;
        Ok(Fetched {
            message: to_message(&self.account_id, w, label, full)?,
            folder_id,
        })
    }

    // ----- messages -----

    /// One message in full (body, headers, attachments); None when gone.
    pub(crate) async fn get_full_wire(&self, id: &str) -> Result<Option<WireMessage>> {
        let path = format!("/me/messages/{}", enc(id));
        let mut w: WireMessage = match self
            .api
            .get_json(
                &path,
                &[
                    ("$select", SELECT_FULL.into()),
                    ("$expand", EXPAND_ATTACHMENTS.into()),
                ],
                &["outlook.body-content-type=\"html\""],
                "message",
            )
            .await
        {
            Ok(w) => w,
            Err(e) if is_gone(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        self.inline_content_ids(&mut w).await?;
        Ok(Some(w))
    }

    /// contentId lives on fileAttachment only (not selectable through the
    /// base type): read it for inline parts, which `cid:` links need.
    async fn inline_content_ids(&self, w: &mut WireMessage) -> Result<()> {
        let path = format!("/me/messages/{}", enc(&w.id));
        if let Some(atts) = w.attachments.as_mut() {
            for a in atts
                .iter_mut()
                .filter(|a| a.is_inline && a.content_id.is_none())
            {
                let one: crate::wire::Attachment = match self
                    .api
                    .get_json(
                        &format!("{path}/attachments/{}", enc(&a.id)),
                        &[],
                        &[],
                        "attachment",
                    )
                    .await
                {
                    Ok(one) => one,
                    Err(e) if is_gone(&e) => continue,
                    Err(e) => return Err(e),
                };
                a.content_id = one.content_id;
            }
        }
        Ok(())
    }

    /// The messages of one folder received in `[from_ms, to_ms]`, in full
    /// (the same `$select`/`$expand` as [`Self::get_full_wire`]), newest
    /// first: `top` per request, at most `max_pages` requests. A backfill
    /// page's bodies in one request instead of one GET each: Outlook limits
    /// requests (10,000 per 10 minutes per mailbox), not bytes.
    pub(crate) async fn list_full_range(
        &self,
        folder_id: &str,
        from_ms: i64,
        to_ms: i64,
        top: usize,
        max_pages: usize,
    ) -> Result<Vec<WireMessage>> {
        const PREFER: &[&str] = &["outlook.body-content-type=\"html\""];
        let path = format!("/me/mailFolders/{}/messages", enc(folder_id));
        let query = [
            ("$select", SELECT_FULL.to_string()),
            ("$expand", EXPAND_ATTACHMENTS.to_string()),
            (
                "$filter",
                format!(
                    "receivedDateTime ge {} and receivedDateTime lt {}",
                    convert::format_date(from_ms),
                    // Whole seconds on the wire: the second after the last.
                    convert::format_date(to_ms + 1_000)
                ),
            ),
            ("$orderby", "receivedDateTime desc".to_string()),
            ("$top", top.to_string()),
        ];
        let mut page: Page<WireMessage> =
            self.api.get_json(&path, &query, PREFER, "messages").await?;
        let mut out = std::mem::take(&mut page.value);
        let mut pages = 1;
        while let Some(next) = page.next_link.take() {
            if pages >= max_pages {
                break;
            }
            page = self.api.get_json(&next, &[], PREFER, "messages").await?;
            out.append(&mut page.value);
            pages += 1;
        }
        // Concurrently, as one-by-one fetches do theirs (the mailbox's
        // 4-request cap applies inside).
        futures_util::future::try_join_all(out.iter_mut().map(|w| self.inline_content_ids(w)))
            .await?;
        Ok(out)
    }

    pub(crate) async fn fetch_full(&self, id: &str) -> Result<Option<Fetched>> {
        match self.get_full_wire(id).await? {
            Some(w) => Ok(Some(self.convert(&w, true).await?)),
            None => Ok(None),
        }
    }

    /// Several messages in full, one outcome per id in input order (the
    /// mailbox concurrency cap applies inside).
    pub(crate) async fn fetch_full_each(
        &self,
        ids: &[String],
    ) -> Vec<(String, Result<Option<Fetched>>)> {
        let results = join_all(ids.iter().map(|id| self.fetch_full(id))).await;
        ids.iter().cloned().zip(results).collect()
    }

    /// Store fetched messages (full) and their locations; returns thread ids.
    pub(crate) async fn store_full(&self, fetched: Vec<Fetched>) -> Result<Vec<String>> {
        if fetched.is_empty() {
            return Ok(vec![]);
        }
        let acct = self.account_id.clone();
        let threads = fetched
            .iter()
            .map(|f| f.message.thread_id.clone())
            .collect();
        self.db(move |s| {
            let locs: Vec<(String, String)> = fetched
                .iter()
                .filter_map(|f| Some((f.message.id.clone(), f.folder_id.clone()?)))
                .collect();
            let messages: Vec<Message> = fetched.into_iter().map(|f| f.message).collect();
            s.upsert_messages(&messages)?;
            locations::set(s, &acct, &locs)
        })
        .await?;
        Ok(threads)
    }

    /// Store headers-only messages (existing rows are left alone) and
    /// their locations; returns (inserted count, thread ids).
    pub(crate) async fn store_headers(
        &self,
        fetched: Vec<Fetched>,
    ) -> Result<(usize, Vec<String>)> {
        if fetched.is_empty() {
            return Ok((0, vec![]));
        }
        let acct = self.account_id.clone();
        let threads = fetched
            .iter()
            .map(|f| f.message.thread_id.clone())
            .collect();
        let n = self
            .db(move |s| {
                let locs: Vec<(String, String)> = fetched
                    .iter()
                    .filter_map(|f| Some((f.message.id.clone(), f.folder_id.clone()?)))
                    .collect();
                let messages: Vec<Message> = fetched.into_iter().map(|f| f.message).collect();
                let n = s.insert_header_messages(&messages)?;
                locations::set(s, &acct, &locs)?;
                Ok(n)
            })
            .await?;
        Ok((n, threads))
    }

    /// Delete messages locally with their locations; returns thread ids.
    pub(crate) async fn delete_local(&self, ids: Vec<String>) -> Result<Vec<String>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let acct = self.account_id.clone();
        self.db(move |s| {
            let mut threads = Vec::new();
            for id in &ids {
                if let Some(m) = s.get_message(&acct, id)? {
                    threads.push(m.thread_id);
                }
            }
            s.delete_messages(&acct, &ids)?;
            locations::delete(s, &acct, &ids)?;
            Ok(threads)
        })
        .await
    }

    /// PATCH a message; Graph answers with the updated message (its id,
    /// conversation and whether it's still a draft are read from it).
    pub(crate) async fn patch(&self, id: &str, body: serde_json::Value) -> Result<Created> {
        let resp = self
            .api
            .call(
                Method::Patch,
                &format!("/me/messages/{}", enc(id)),
                &[],
                Body::Json(body),
                &[],
                Retry::Idempotent,
            )
            .await?;
        if resp.body.is_empty() {
            return Ok(Created::default());
        }
        resp.json("updated message")
    }

    /// Move a message; returns the folder id it landed in (Graph answers
    /// with the moved message; its immutable id doesn't change).
    pub(crate) async fn move_to(&self, id: &str, destination: &str) -> Result<Option<String>> {
        let resp = self
            .api
            .call(
                Method::Post,
                &format!("/me/messages/{}/move", enc(id)),
                &[],
                Body::Json(json!({ "destinationId": destination })),
                &[],
                Retry::Idempotent,
            )
            .await?;
        let moved: Created = resp.json("move")?;
        if !moved.id.is_empty() && moved.id != id {
            tracing::warn!(account = %self.account_id, "Graph changed a message id on move despite immutable ids");
        }
        Ok(moved.parent_folder_id)
    }

    /// A light listing page of `/me/messages` (search, ranges, counts).
    pub(crate) async fn list_messages(
        &self,
        link: Option<&str>,
        query: &[(&str, String)],
        prefer: &[&str],
    ) -> Result<Page<WireMessage>> {
        match link {
            Some(l) => self.api.get_json(l, &[], prefer, "messages").await,
            None => {
                let mut q: Vec<(&str, String)> = vec![("$select", SELECT_LIGHT.into())];
                q.extend(query.iter().cloned());
                self.api
                    .get_json("/me/messages", &q, prefer, "messages")
                    .await
            }
        }
    }

    /// Headers of a message in server order, or None when gone.
    pub(crate) async fn headers(
        &self,
        id: &str,
    ) -> Result<Option<(Vec<(String, String)>, Option<u64>)>> {
        let path = format!("/me/messages/{}", enc(id));
        let w: WireMessage = match self
            .api
            .get_json(
                &path,
                &[
                    ("$select", "id,internetMessageHeaders".into()),
                    (
                        "$expand",
                        format!(
                            "singleValueExtendedProperties($filter=id eq '{}')",
                            crate::wire::SIZE_PROPERTY
                        ),
                    ),
                ],
                &[],
                "message headers",
            )
            .await
        {
            Ok(w) => w,
            Err(e) if is_gone(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let size = w
            .single_value_extended_properties
            .iter()
            .flatten()
            // Graph echoes the id normalized ("Integer 0xe08").
            .find(|p| {
                let id = p.id.to_ascii_lowercase().replace("0x0e08", "0xe08");
                id.split_whitespace().collect::<Vec<_>>() == ["integer", "0xe08"]
            })
            .and_then(|p| p.value.trim().parse().ok());
        let headers: Vec<(String, String)> = w
            .internet_message_headers
            .unwrap_or_default()
            .into_iter()
            .map(|h| (h.name, h.value))
            .collect();
        if !headers.is_empty() {
            return Ok(Some((headers, size)));
        }
        // Sent items and drafts made in Outlook carry no internet headers:
        // read them from the MIME source.
        match self.raw(id).await? {
            Some(raw) => {
                let size = size.or(Some(raw.len() as u64));
                Ok(Some((convert::raw_headers(&raw), size)))
            }
            None => Ok(None),
        }
    }

    /// The MIME source, or None when gone.
    pub(crate) async fn raw(&self, id: &str) -> Result<Option<Vec<u8>>> {
        match self
            .api
            .call(
                Method::Get,
                &format!("/me/messages/{}/$value", enc(id)),
                &[],
                Body::None,
                &[],
                Retry::Idempotent,
            )
            .await
        {
            Ok(resp) => Ok(Some(resp.body)),
            Err(e) if is_gone(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
