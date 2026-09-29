//! What the Penguin app does for an agent over the agent socket
//! (agent/ipc.rs): drafts, sends, attachment downloads and share links
//! (agent/sharing.rs), each checked against the agent level
//! (agent/permission.rs) at the moment it arrives.
//!
//! - **Drafts** go through the same path as the composer's
//!   (`outgoing::save_draft`): the provider's real Drafts, mirrored locally
//!   so Penguin's Drafts view shows them at once. Each one is recorded as
//!   the agent's (`agent_drafts`, local only).
//! - **Scope:** an agent can update, delete and send only drafts an agent
//!   created. Your own drafts are never changed, sent or deleted by an
//!   agent (deleting a draft can't be undone, and a draft you left
//!   half-written isn't meant to go out).
//! - **Replies** are threaded like the composer's: the draft names the
//!   replied-to message, the provider adds In-Reply-To/References from it
//!   and keeps the thread; they're sent from the account that received the
//!   original (another account can't reply in that thread).
//! - **Sends** never go straight out. The draft is saved, then scheduled in
//!   the outbox `sendDelaySeconds` from now (a minute by default), exactly
//!   like Send later: the user gets a notification and a toast with Cancel,
//!   the draft stays visible in Drafts until then, and lowering the agent
//!   level cancels every send an agent queued. With "only people I've
//!   emailed" (on by default) every recipient must be someone the user has
//!   sent mail to before, or one of their own accounts.
//!
//! Every request is audit-logged (agent/audit.rs) with counts and ids only.
//! Tauri-free: the app implements [`WriteHost`]; tests use the fake
//! provider.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use penguin_core::store::AgentDraft;
use penguin_core::{Account, Address, Message, Store};
use penguin_provider::compose::{Draft, OutgoingAttachment};
use penguin_provider::{async_trait, MailProvider};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::attach::{self, AttachPolicy, AttachmentArg};
use super::audit::{AppAuditEntry, AuditLog};
use super::output::{envelope, iso, AttachmentOut};
use super::{ipc, markdown, permission};
use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops::{self, Paths};
use crate::settings::{AgentAccess, Settings};
use crate::state::blocking;

/// Bounds on what an agent can put in one draft.
const MAX_RECIPIENTS: usize = 100;
const MAX_SUBJECT_CHARS: usize = 998;
const MAX_NAME_CHARS: usize = 200;
/// Largest attachment `fetch_attachment` hands to a client.
pub const MAX_FETCH_BYTES: u64 = 25 * 1024 * 1024;

/// What the handlers need from the app.
#[async_trait]
pub trait WriteHost: Send + Sync + 'static {
    fn store(&self) -> &Store;
    fn paths(&self) -> &Paths;
    /// The live settings (in memory: a change applies to the next request).
    fn settings(&self) -> Settings;
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>>;
    /// Serializes draft saves with the composer's autosaves.
    fn drafts_lock(&self) -> &tokio::sync::Mutex<()>;
    fn mail_changed(&self, account_id: &str, thread_ids: Vec<String>);
    /// An agent's send was queued: tell the user (notification, toast).
    fn send_queued(&self, queued: &QueuedSend);
    fn audit(&self) -> &AuditLog;
    /// The share-link state (settings, Keychain secret, upload records), for
    /// `create_share_link` (agent/sharing.rs).
    fn share(&self) -> Arc<crate::share::Share>;
    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }
    fn log_dir(&self) -> Option<PathBuf> {
        super::log_dir()
    }
    fn now_ms(&self) -> i64 {
        ops::now_ms()
    }
}

/// `penguin://agent-send-queued`: an agent's send is waiting in the outbox.
/// Mirrored in types.ts (`AgentSendQueued`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedSend {
    pub schedule_id: String,
    pub account_id: String,
    pub draft_id: String,
    pub thread_id: String,
    pub send_at: i64,
    pub delay_seconds: u32,
    /// Every recipient's address (To, Cc, Bcc).
    pub to: Vec<String>,
    pub subject: String,
}

// ---------- arguments (shared by the MCP tools and the CLI) ----------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub enum BodyFormat {
    /// Sent as written.
    #[default]
    Text,
    /// Rendered to HTML (bold, italic, links, lists, headings, quotes,
    /// code); the Markdown itself is the plain-text part.
    Markdown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftArgs {
    /// Recipients, each "Name <address>" or a bare address. A reply with no
    /// `to` goes to the original's sender (its Reply-To when set).
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Default for a reply: "Re: " + the original's subject.
    pub subject: Option<String>,
    /// The message text: plain text, or Markdown with format "markdown".
    #[serde(default)]
    pub body: String,
    /// "text" (default) or "markdown".
    pub format: Option<BodyFormat>,
    /// The account (email address) it's from. Default: the account that
    /// received `replyToMessageId`, or the only account.
    pub from_account: Option<String>,
    /// Reply to this message (a messageId from search or get_thread): the
    /// draft joins its thread with In-Reply-To/References set.
    pub reply_to_message_id: Option<String>,
    /// Files to attach (25 MB in all, at most 20).
    #[serde(default)]
    pub attachments: Vec<AttachmentArg>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateDraftArgs {
    /// Optional when the draft id is unique across accounts.
    pub account_id: Option<String>,
    pub draft_id: String,
    /// Each field given replaces the draft's; omitted fields are kept.
    pub to: Option<Vec<String>>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub subject: Option<String>,
    pub body: Option<String>,
    pub format: Option<BodyFormat>,
    /// Replaces every attachment; omit to keep them.
    pub attachments: Option<Vec<AttachmentArg>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftIdArgs {
    /// Optional when the draft id is unique across accounts.
    pub account_id: Option<String>,
    pub draft_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListDraftsArgs {
    /// Limit to one account (email address).
    pub account: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct FetchAttachmentArgs {
    pub account_id: String,
    pub message_id: String,
    pub attachment_id: String,
}

// ---------- results ----------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftFileOut {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct QueuedSendOut {
    pub schedule_id: String,
    pub send_at: i64,
    pub send_at_iso: String,
}

/// `draft` (create_draft, update_draft) and each of `drafts`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftOut {
    pub account_id: String,
    pub draft_id: String,
    pub message_id: String,
    pub thread_id: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub attachments: Vec<DraftFileOut>,
    /// The message it replies to, if any.
    pub reply_to_message_id: Option<String>,
    /// `mcp` or `cli`.
    pub created_by: String,
    pub updated_at: i64,
    pub updated_at_iso: String,
    /// Set while a send an agent queued is waiting in the outbox.
    pub queued_send: Option<QueuedSendOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftsOut {
    /// Drafts agents created that still exist, newest first. Drafts the
    /// user wrote are not listed (list_threads view "drafts" reads those).
    pub drafts: Vec<DraftOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftDeletedOut {
    pub account_id: String,
    pub draft_id: String,
}

/// `sendQueued` (send_draft, send_message).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct SendQueuedOut {
    pub account_id: String,
    pub draft_id: String,
    pub thread_id: String,
    pub schedule_id: String,
    /// When it goes (unix ms); the user can cancel it until then.
    pub send_at: i64,
    pub send_at_iso: String,
    pub delay_seconds: u32,
    pub recipient_count: usize,
    /// "queued": it waits in the outbox; it is not sent yet.
    pub status: String,
}

/// `fetch_attachment` (not an MCP tool: the MCP server's get_attachment
/// and `penguin-cli attachment` use it for what isn't cached).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentBytesOut {
    pub account_id: String,
    pub message_id: String,
    pub attachment: AttachmentOut,
    pub data_base64: String,
}

// ---------- the service ----------

pub struct AgentService<H: WriteHost> {
    host: Arc<H>,
}

impl<H: WriteHost> AgentService<H> {
    pub fn new(host: Arc<H>) -> Self {
        AgentService { host }
    }
}

fn client_name(client: &str) -> &'static str {
    match client {
        "mcp" => "mcp",
        "cli" => "cli",
        _ => "other",
    }
}

fn parse<T: serde::de::DeserializeOwned>(tool: &str, args: Value) -> CmdResult<T> {
    let args = if args.is_null() {
        Value::Object(Default::default())
    } else {
        args
    };
    serde_json::from_value(args).map_err(|e| CmdError::invalid(format!("{tool}: {e}")))
}

fn to_value<T: Serialize>(v: T) -> CmdResult<Value> {
    serde_json::to_value(v).map_err(|e| CmdError::other(e.to_string()))
}

#[async_trait]
impl<H: WriteHost> ipc::Handler for AgentService<H> {
    async fn handle(&self, client: &str, tool: &str, args: Value) -> CmdResult<Value> {
        let started = Instant::now();
        let mut audit = AppAuditEntry {
            tool: permission::required(tool)
                .map(|_| tool.to_string())
                .unwrap_or_else(|| "unknown".into()),
            via: client_name(client).into(),
            ..AppAuditEntry::default()
        };
        let result = self
            .dispatch(client_name(client), tool, args, &mut audit)
            .await;
        audit.ts = iso(self.host.now_ms());
        audit.ok = result.is_ok();
        audit.error_code = result.as_ref().err().map(|e| e.code.as_str().to_string());
        audit.ms = started.elapsed().as_secs_f64() * 1000.0;
        self.host.audit().record(&audit);
        if let Err(e) = &result {
            tracing::info!(tool = %audit.tool, via = %audit.via, code = e.code.as_str(), "agent request refused or failed");
        }
        result
    }
}

impl<H: WriteHost> AgentService<H> {
    async fn dispatch(
        &self,
        client: &'static str,
        tool: &str,
        args: Value,
        audit: &mut AppAuditEntry,
    ) -> CmdResult<Value> {
        // The gate, on the app's live settings.
        permission::check(self.host.settings().mcp.access, tool)?;
        match tool {
            "create_draft" => {
                let a: DraftArgs = parse(tool, args)?;
                let (out, _) = self.create(client, a, audit, false).await?;
                to_value(envelope("draft", out))
            }
            "update_draft" => {
                let a: UpdateDraftArgs = parse(tool, args)?;
                to_value(envelope("draft", self.update(client, a, audit).await?))
            }
            "list_drafts" => {
                let a: ListDraftsArgs = parse(tool, args)?;
                let out = self.list(a.account.as_deref()).await?;
                audit.result_count = out.drafts.len();
                to_value(envelope("drafts", out))
            }
            "delete_draft" => {
                let a: DraftIdArgs = parse(tool, args)?;
                to_value(envelope("draftDeleted", self.delete(a, audit).await?))
            }
            "send_draft" => {
                let a: DraftIdArgs = parse(tool, args)?;
                to_value(envelope("sendQueued", self.send_draft(a, audit).await?))
            }
            "send_message" => {
                let a: DraftArgs = parse(tool, args)?;
                let (out, draft) = self.create(client, a, audit, true).await?;
                let q = self
                    .queue(
                        &out.account_id,
                        &out.draft_id,
                        &out.thread_id,
                        &draft,
                        audit,
                    )
                    .await?;
                to_value(envelope("sendQueued", q))
            }
            "fetch_attachment" => {
                let a: FetchAttachmentArgs = parse(tool, args)?;
                to_value(self.fetch_attachment(a, audit).await?)
            }
            "create_share_link" => {
                let a: super::sharing::ShareLinkArgs = parse(tool, args)?;
                let out = super::sharing::create(self.host.as_ref(), a, audit).await?;
                to_value(envelope("shareLink", out))
            }
            other => Err(CmdError::invalid(format!(
                "{other} isn't something Penguin does for agents"
            ))),
        }
    }

    fn store(&self) -> Store {
        self.host.store().clone()
    }

    async fn accounts(&self) -> CmdResult<Vec<Account>> {
        let store = self.store();
        blocking(move || Ok(store.list_accounts()?)).await
    }

    fn attach_policy(&self) -> AttachPolicy {
        let canon = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
        let paths = self.host.paths();
        let mut protected = vec![
            canon(paths.data_dir.clone()),
            canon(paths.config_dir.clone()),
            canon(paths.cache_dir.clone()),
        ];
        protected.extend(self.host.log_dir().map(canon));
        AttachPolicy {
            home: self.host.home_dir().map(canon),
            protected,
        }
    }

    /// Build and save a new agent draft. With `for_send`, the recipients are
    /// checked for sending before anything is saved.
    async fn create(
        &self,
        client: &'static str,
        a: DraftArgs,
        audit: &mut AppAuditEntry,
        for_send: bool,
    ) -> CmdResult<(DraftOut, Draft)> {
        let settings = self.host.settings();
        let accounts = self.accounts().await?;
        let (account, parent) = self.resolve_from(&accounts, &a).await?;
        audit.account = Some(account.id.clone());

        let own: HashSet<String> = accounts.iter().map(|x| x.email.to_lowercase()).collect();
        let mut to = parse_addresses("to", &a.to)?;
        let cc = parse_addresses("cc", &a.cc)?;
        let bcc = parse_addresses("bcc", &a.bcc)?;
        let mut subject = a.subject.as_deref().map(one_line).unwrap_or_default();
        if let Some(p) = &parent {
            if to.is_empty() && a.to.is_empty() {
                to = reply_recipients(p, &own);
            }
            if a.subject.is_none() {
                subject = reply_subject(&p.subject);
            }
        }
        let (body_text, body_html) = body(&a.body, a.format.unwrap_or_default());
        let policy = self.attach_policy();
        let files = a.attachments.clone();
        let attachments = blocking(move || attach::load_all(&files, &policy)).await?;
        let draft = Draft {
            account_id: account.id.clone(),
            to,
            cc,
            bcc,
            subject,
            body_text,
            body_html,
            reply_to_thread_id: parent.as_ref().map(|p| p.thread_id.clone()),
            reply_to_message_id: parent.as_ref().map(|p| p.id.clone()),
            attachments,
            request_read_receipt: None,
        };
        let draft = crate::commands::with_receipt_choice(draft, settings.request_read_receipts);
        audit.recipient_count = Some(recipient_count(&draft));
        audit.attachment_count = Some(draft.attachments.len());
        if for_send {
            self.check_recipients(&draft, &accounts).await?;
        }
        check_size(&draft)?;

        let provider = self.host.provider(&account.id).await?;
        let from = ops::from_address(&account);
        let saved = {
            let _serial = self.host.drafts_lock().lock().await;
            crate::outgoing::save_draft(
                self.host.paths(),
                provider.as_ref(),
                &Default::default(),
                &from,
                &draft,
                None,
            )
            .await?
        };
        audit.draft_id = Some(saved.draft_id.clone());
        let now = self.host.now_ms();
        {
            let (store, acct, id, reply) = (
                self.store(),
                account.id.clone(),
                saved.draft_id.clone(),
                draft.reply_to_message_id.clone(),
            );
            blocking(move || {
                Ok(store.put_agent_draft(&acct, &id, client, reply.as_deref(), now)?)
            })
            .await?;
        }
        self.host
            .mail_changed(&account.id, vec![saved.thread_id.clone()]);
        tracing::info!(account = %account.id, draft = %saved.draft_id, via = client, "agent draft saved");
        let out = DraftOut {
            account_id: account.id.clone(),
            draft_id: saved.draft_id,
            message_id: saved.message_id,
            thread_id: saved.thread_id,
            to: draft.to.iter().map(show_address).collect(),
            cc: draft.cc.iter().map(show_address).collect(),
            bcc: draft.bcc.iter().map(show_address).collect(),
            subject: draft.subject.clone(),
            attachments: draft.attachments.iter().map(file_out).collect(),
            reply_to_message_id: draft.reply_to_message_id.clone(),
            created_by: client.to_string(),
            updated_at: now,
            updated_at_iso: iso(now),
            queued_send: None,
        };
        Ok((out, draft))
    }

    /// The sending account and, for a reply, the stored original.
    async fn resolve_from(
        &self,
        accounts: &[Account],
        a: &DraftArgs,
    ) -> CmdResult<(Account, Option<Message>)> {
        let named = match a
            .from_account
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(f) => Some(find_account(accounts, f)?),
            None => None,
        };
        let parent = match a
            .reply_to_message_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => None,
            Some(id) => {
                let (store, id, within) = (
                    self.store(),
                    id.to_string(),
                    match &named {
                        Some(acct) => vec![acct.id.clone()],
                        None => accounts.iter().map(|x| x.id.clone()).collect(),
                    },
                );
                let found = blocking(move || {
                    let mut hits = Vec::new();
                    for acct in within {
                        if let Some(m) = store.get_message(&acct, &id)? {
                            hits.push(m);
                        }
                    }
                    Ok(hits)
                })
                .await?;
                let id = a.reply_to_message_id.as_deref().unwrap_or_default().trim();
                match found.len() {
                    0 => {
                        return Err(CmdError::not_found(match &named {
                            Some(acct) => format!(
                                "no message {id} in {}; a reply is sent from the account that received the original",
                                acct.email
                            ),
                            None => format!("no message {id} in any account"),
                        }))
                    }
                    1 => found.into_iter().next(),
                    _ => {
                        return Err(CmdError::invalid(format!(
                            "message {id} is in more than one account; say which with fromAccount"
                        )))
                    }
                }
            }
        };
        let account = match (named, &parent) {
            (Some(acct), _) => acct,
            (None, Some(p)) => find_account(accounts, &p.account_id)?,
            (None, None) => match accounts {
                [] => return Err(CmdError::not_found("Penguin has no accounts yet")),
                [only] => only.clone(),
                _ => {
                    return Err(CmdError::invalid(format!(
                        "fromAccount is required when there's more than one account: {}",
                        accounts
                            .iter()
                            .map(|x| x.email.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )))
                }
            },
        };
        Ok((account, parent))
    }

    /// The agent draft `draft_id` (in `account` when given). Drafts the user
    /// wrote are "not found" to an agent: it can only act on its own.
    async fn own_draft(&self, account: Option<&str>, draft_id: &str) -> CmdResult<AgentDraft> {
        let (store, acct, id) = (
            self.store(),
            account
                .map(|a| a.trim().to_lowercase())
                .filter(|a| !a.is_empty()),
            draft_id.trim().to_string(),
        );
        if id.is_empty() {
            return Err(CmdError::invalid("draftId is required"));
        }
        let rows = blocking(move || {
            Ok(match &acct {
                Some(a) => store.get_agent_draft(a, &id)?.into_iter().collect(),
                None => store.find_agent_drafts(&id)?,
            })
        })
        .await?;
        let row = match rows.len() {
            0 => {
                return Err(CmdError::not_found(format!(
                    "no draft {draft_id} created by an agent; agents can only change, send or delete drafts an agent created"
                )))
            }
            1 => rows.into_iter().next().expect("one row"),
            _ => {
                return Err(CmdError::invalid(format!(
                    "draft {draft_id} exists in more than one account; say which with accountId"
                )))
            }
        };
        if row.message_id.is_none() {
            let (store, a, d) = (self.store(), row.account_id.clone(), row.draft_id.clone());
            blocking(move || Ok(store.remove_agent_draft(&a, &d)?)).await?;
            return Err(CmdError::not_found(format!(
                "draft {draft_id} no longer exists (it was sent or deleted)"
            )));
        }
        Ok(row)
    }

    /// The outbox schedule an agent queued for `row`, while it's pending.
    async fn pending_send(
        &self,
        row: &AgentDraft,
    ) -> CmdResult<Option<penguin_core::ScheduledSend>> {
        let Some(id) = row.send_schedule_id.clone() else {
            return Ok(None);
        };
        let (store, acct) = (self.store(), row.account_id.clone());
        blocking(move || Ok(store.get_scheduled_send(&acct, &id)?)).await
    }

    async fn update(
        &self,
        client: &'static str,
        a: UpdateDraftArgs,
        audit: &mut AppAuditEntry,
    ) -> CmdResult<DraftOut> {
        let row = self.own_draft(a.account_id.as_deref(), &a.draft_id).await?;
        audit.account = Some(row.account_id.clone());
        audit.draft_id = Some(row.draft_id.clone());
        if let Some(s) = self.pending_send(&row).await? {
            return Err(CmdError::invalid(format!(
                "draft {} is queued to send at {}; it can't be changed now (the user can cancel the send in Penguin)",
                row.draft_id,
                iso(s.send_at)
            )));
        }
        if a.format.is_some() && a.body.is_none() {
            return Err(CmdError::invalid("format needs a body"));
        }
        let accounts = self.accounts().await?;
        let account = find_account(&accounts, &row.account_id)?;
        let provider = self.host.provider(&account.id).await?;
        let Some(opened) = provider.open_draft(Some(&row.draft_id), None).await? else {
            let (store, acct, id) = (self.store(), row.account_id.clone(), row.draft_id.clone());
            blocking(move || Ok(store.remove_agent_draft(&acct, &id)?)).await?;
            return Err(CmdError::not_found(format!(
                "draft {} no longer exists (it was sent or deleted)",
                row.draft_id
            )));
        };
        let mut draft = opened.draft;
        draft.account_id = account.id.clone();
        // Keep it threaded whatever the provider's reopened copy remembers.
        if draft.reply_to_message_id.is_none() {
            if let Some(parent_id) = row.reply_to_message_id.clone() {
                let (store, acct) = (self.store(), account.id.clone());
                let pid = parent_id.clone();
                let parent = blocking(move || Ok(store.get_message(&acct, &pid)?)).await?;
                draft.reply_to_thread_id = parent.map(|p| p.thread_id);
                draft.reply_to_message_id = Some(parent_id);
            }
        }
        if let Some(v) = &a.to {
            draft.to = parse_addresses("to", v)?;
        }
        if let Some(v) = &a.cc {
            draft.cc = parse_addresses("cc", v)?;
        }
        if let Some(v) = &a.bcc {
            draft.bcc = parse_addresses("bcc", v)?;
        }
        if let Some(s) = &a.subject {
            draft.subject = one_line(s);
        }
        if let Some(b) = &a.body {
            let (text, html) = body(b, a.format.unwrap_or_default());
            draft.body_text = text;
            draft.body_html = html;
        }
        if let Some(files) = a.attachments.clone() {
            let policy = self.attach_policy();
            draft.attachments = blocking(move || attach::load_all(&files, &policy)).await?;
        }
        audit.recipient_count = Some(recipient_count(&draft));
        audit.attachment_count = Some(draft.attachments.len());
        check_size(&draft)?;
        let from = ops::from_address(&account);
        let saved = {
            let _serial = self.host.drafts_lock().lock().await;
            crate::outgoing::save_draft(
                self.host.paths(),
                provider.as_ref(),
                &Default::default(),
                &from,
                &draft,
                Some(&row.draft_id),
            )
            .await?
        };
        let now = self.host.now_ms();
        {
            // A draft gone from the server is recreated under a new id.
            let (store, acct, old, new) = (
                self.store(),
                account.id.clone(),
                row.draft_id.clone(),
                saved.draft_id.clone(),
            );
            let (created_by, reply) = (row.client.clone(), draft.reply_to_message_id.clone());
            blocking(move || {
                if old != new {
                    store.remove_agent_draft(&acct, &old)?;
                }
                Ok(store.put_agent_draft(&acct, &new, &created_by, reply.as_deref(), now)?)
            })
            .await?;
        }
        audit.draft_id = Some(saved.draft_id.clone());
        self.host
            .mail_changed(&account.id, vec![saved.thread_id.clone()]);
        tracing::info!(account = %account.id, draft = %saved.draft_id, via = client, "agent draft updated");
        Ok(DraftOut {
            account_id: account.id.clone(),
            draft_id: saved.draft_id,
            message_id: saved.message_id,
            thread_id: saved.thread_id,
            to: draft.to.iter().map(show_address).collect(),
            cc: draft.cc.iter().map(show_address).collect(),
            bcc: draft.bcc.iter().map(show_address).collect(),
            subject: draft.subject.clone(),
            attachments: draft.attachments.iter().map(file_out).collect(),
            reply_to_message_id: draft.reply_to_message_id.clone(),
            created_by: row.client,
            updated_at: now,
            updated_at_iso: iso(now),
            queued_send: None,
        })
    }

    async fn list(&self, account: Option<&str>) -> CmdResult<DraftsOut> {
        let (store, acct) = (self.store(), account.map(|a| a.trim().to_lowercase()));
        let drafts = blocking(move || {
            store.prune_agent_drafts()?;
            let mut out = Vec::new();
            for row in store.list_agent_drafts(acct.as_deref())? {
                let Some(mid) = &row.message_id else { continue };
                let Some(m) = store.get_message(&row.account_id, mid)? else {
                    continue;
                };
                let queued = match &row.send_schedule_id {
                    Some(id) => {
                        store
                            .get_scheduled_send(&row.account_id, id)?
                            .map(|s| QueuedSendOut {
                                schedule_id: s.id,
                                send_at: s.send_at,
                                send_at_iso: iso(s.send_at),
                            })
                    }
                    None => None,
                };
                out.push(DraftOut {
                    account_id: row.account_id.clone(),
                    draft_id: row.draft_id.clone(),
                    message_id: m.id.clone(),
                    thread_id: m.thread_id.clone(),
                    to: m.to.iter().map(show_address).collect(),
                    cc: m.cc.iter().map(show_address).collect(),
                    bcc: m.bcc.iter().map(show_address).collect(),
                    subject: m.subject.clone(),
                    attachments: m
                        .attachments
                        .iter()
                        .filter(|a| !a.inline)
                        .map(|a| DraftFileOut {
                            filename: a.filename.clone(),
                            mime_type: a.mime_type.clone(),
                            size: a.size,
                        })
                        .collect(),
                    reply_to_message_id: row.reply_to_message_id.clone(),
                    created_by: row.client.clone(),
                    updated_at: row.updated_at,
                    updated_at_iso: iso(row.updated_at),
                    queued_send: queued,
                });
            }
            Ok(out)
        })
        .await?;
        Ok(DraftsOut { drafts })
    }

    async fn delete(
        &self,
        a: DraftIdArgs,
        audit: &mut AppAuditEntry,
    ) -> CmdResult<DraftDeletedOut> {
        let row = self.own_draft(a.account_id.as_deref(), &a.draft_id).await?;
        audit.account = Some(row.account_id.clone());
        audit.draft_id = Some(row.draft_id.clone());
        let provider = self.host.provider(&row.account_id).await?;
        let thread = {
            let _serial = self.host.drafts_lock().lock().await;
            provider.delete_draft(&row.draft_id).await?
        };
        let (store, acct, id) = (self.store(), row.account_id.clone(), row.draft_id.clone());
        let unscheduled = blocking(move || {
            let n = store.delete_scheduled_sends_for_draft(&acct, &id)?;
            store.remove_agent_draft(&acct, &id)?;
            Ok(n)
        })
        .await?;
        if unscheduled > 0 {
            crate::outbox::schedule_changed();
        }
        if let Some(t) = thread {
            self.host.mail_changed(&row.account_id, vec![t]);
        }
        tracing::info!(account = %row.account_id, draft = %row.draft_id, "agent draft deleted");
        Ok(DraftDeletedOut {
            account_id: row.account_id,
            draft_id: row.draft_id,
        })
    }

    async fn send_draft(
        &self,
        a: DraftIdArgs,
        audit: &mut AppAuditEntry,
    ) -> CmdResult<SendQueuedOut> {
        let row = self.own_draft(a.account_id.as_deref(), &a.draft_id).await?;
        audit.account = Some(row.account_id.clone());
        audit.draft_id = Some(row.draft_id.clone());
        let provider = self.host.provider(&row.account_id).await?;
        // What the server holds is what will go out: check that.
        let Some(opened) = provider.open_draft(Some(&row.draft_id), None).await? else {
            return Err(CmdError::not_found(format!(
                "draft {} no longer exists (it was sent or deleted)",
                row.draft_id
            )));
        };
        audit.recipient_count = Some(recipient_count(&opened.draft));
        audit.attachment_count = Some(opened.draft.attachments.len());
        if let Some(s) = self.pending_send(&row).await? {
            // Already queued: the same answer again, nothing new.
            return Ok(SendQueuedOut {
                account_id: row.account_id.clone(),
                draft_id: row.draft_id.clone(),
                thread_id: opened.thread_id.clone(),
                schedule_id: s.id,
                send_at: s.send_at,
                send_at_iso: iso(s.send_at),
                delay_seconds: self.host.settings().mcp.send_delay_seconds,
                recipient_count: recipient_count(&opened.draft),
                status: "queued".into(),
            });
        }
        let accounts = self.accounts().await?;
        self.check_recipients(&opened.draft, &accounts).await?;
        self.queue(
            &row.account_id,
            &row.draft_id,
            &opened.thread_id,
            &opened.draft,
            audit,
        )
        .await
    }

    /// Recipients present, and (with "only people I've emailed") each one
    /// someone I've sent mail to, or one of my own accounts.
    async fn check_recipients(&self, draft: &Draft, accounts: &[Account]) -> CmdResult<()> {
        let all: Vec<String> = draft
            .to
            .iter()
            .chain(&draft.cc)
            .chain(&draft.bcc)
            .map(|a| a.email.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        if all.is_empty() {
            return Err(CmdError::invalid(
                "add at least one recipient before sending",
            ));
        }
        if !self.host.settings().mcp.send_known_only {
            return Ok(());
        }
        let own: HashSet<String> = accounts.iter().map(|a| a.email.to_lowercase()).collect();
        let store = self.store();
        let unknown = blocking(move || {
            let mut out = Vec::new();
            for e in all {
                if !own.contains(&e) && store.sent_to_count(&e)? == 0 && !out.contains(&e) {
                    out.push(e);
                }
            }
            Ok(out)
        })
        .await?;
        if unknown.is_empty() {
            return Ok(());
        }
        Err(CmdError::denied(format!(
            "Agents may only send to people you've emailed before, and you haven't emailed {}. \
             Save it as a draft (create_draft) for the user to review and send, or the user can turn off \
             \"Only people I've emailed\" in Penguin → Settings → Developer → Agents.",
            unknown.join(", ")
        )))
    }

    /// Put a saved agent draft in the outbox, `sendDelaySeconds` from now.
    async fn queue(
        &self,
        account_id: &str,
        draft_id: &str,
        thread_id: &str,
        draft: &Draft,
        audit: &mut AppAuditEntry,
    ) -> CmdResult<SendQueuedOut> {
        let delay = self.host.settings().mcp.send_delay_seconds;
        let now = self.host.now_ms();
        let send_at = now + i64::from(delay) * 1000;
        let s = penguin_provider::outbox::schedule_send(
            self.host.store(),
            account_id,
            draft_id,
            send_at,
            None,
            now,
        )
        .await
        .map_err(|e| CmdError::invalid(e.to_string()))?;
        {
            let (store, acct, id, sid) = (
                self.store(),
                account_id.to_string(),
                draft_id.to_string(),
                s.id.clone(),
            );
            blocking(move || Ok(store.set_agent_send(&acct, &id, Some(&sid))?)).await?;
        }
        // A downgrade that landed while this request was running wins. (One
        // that lands from here on finds the schedule recorded above and
        // cancels it: `cancel_agent_sends`.)
        if self.host.settings().mcp.access < AgentAccess::Send {
            let (store, acct, id, sid) = (
                self.store(),
                account_id.to_string(),
                draft_id.to_string(),
                s.id.clone(),
            );
            blocking(move || {
                store.delete_scheduled_send(&acct, &sid)?;
                Ok(store.set_agent_send(&acct, &id, None)?)
            })
            .await?;
            return Err(
                permission::check(self.host.settings().mcp.access, "send_draft")
                    .err()
                    .unwrap_or_else(|| CmdError::denied("sending was turned off")),
            );
        }
        crate::outbox::schedule_changed();
        audit.send_at = Some(iso(send_at));
        let to: Vec<String> = draft
            .to
            .iter()
            .chain(&draft.cc)
            .chain(&draft.bcc)
            .map(|a| a.email.clone())
            .collect();
        self.host.send_queued(&QueuedSend {
            schedule_id: s.id.clone(),
            account_id: account_id.to_string(),
            draft_id: draft_id.to_string(),
            thread_id: thread_id.to_string(),
            send_at,
            delay_seconds: delay,
            to: to.clone(),
            subject: draft.subject.clone(),
        });
        tracing::info!(account = %account_id, draft = %draft_id, recipients = to.len(), delay_s = delay, "agent send queued");
        Ok(SendQueuedOut {
            account_id: account_id.to_string(),
            draft_id: draft_id.to_string(),
            thread_id: thread_id.to_string(),
            schedule_id: s.id,
            send_at,
            send_at_iso: iso(send_at),
            delay_seconds: delay,
            recipient_count: to.len(),
            status: "queued".into(),
        })
    }

    async fn fetch_attachment(
        &self,
        a: FetchAttachmentArgs,
        audit: &mut AppAuditEntry,
    ) -> CmdResult<AttachmentBytesOut> {
        let acct = a.account_id.trim().to_lowercase();
        audit.account = Some(acct.clone());
        let (store, ac, m, id) = (
            self.store(),
            acct.clone(),
            a.message_id.clone(),
            a.attachment_id.clone(),
        );
        let meta = blocking(move || crate::attachments::find(&store, &ac, &m, &id)).await?;
        if meta.size > MAX_FETCH_BYTES {
            return Err(CmdError::invalid(format!(
                "{} is {:.1} MB; attachments over {} MB aren't handed to agents",
                meta.filename,
                meta.size as f64 / (1024.0 * 1024.0),
                MAX_FETCH_BYTES >> 20
            )));
        }
        let provider = self.host.provider(&acct).await?;
        let bytes = crate::attachments::bytes(
            self.host.paths(),
            provider.as_ref(),
            &acct,
            &a.message_id,
            &meta,
        )
        .await?;
        audit.result_count = 1;
        use base64::Engine;
        Ok(AttachmentBytesOut {
            account_id: acct,
            message_id: a.message_id,
            attachment: AttachmentOut::from(&meta),
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }
}

// ---------- helpers ----------

fn find_account(accounts: &[Account], email: &str) -> CmdResult<Account> {
    let want = email.trim().to_lowercase();
    accounts
        .iter()
        .find(|a| a.id == want || a.email.to_lowercase() == want)
        .cloned()
        .ok_or_else(|| {
            CmdError::not_found(format!(
                "unknown account {email}; accounts: {}",
                accounts
                    .iter()
                    .map(|a| a.email.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

fn one_line(s: &str) -> String {
    s.split(['\r', '\n'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_SUBJECT_CHARS)
        .collect()
}

fn reply_subject(original: &str) -> String {
    let s = original.trim();
    if s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("re:")) {
        s.to_string()
    } else {
        format!("Re: {s}")
    }
}

/// Who a reply goes to by default: the original's Reply-To, else its
/// sender; for a message I sent, its recipients.
fn reply_recipients(parent: &Message, own: &HashSet<String>) -> Vec<Address> {
    if own.contains(&parent.from.email.to_lowercase()) {
        return parent
            .to
            .iter()
            .filter(|a| !own.contains(&a.email.to_lowercase()))
            .cloned()
            .collect();
    }
    if !parent.reply_to.is_empty() {
        return parent.reply_to.clone();
    }
    vec![parent.from.clone()]
}

fn body(text: &str, format: BodyFormat) -> (String, Option<String>) {
    let text = text.replace("\r\n", "\n");
    match format {
        BodyFormat::Text => (text, None),
        BodyFormat::Markdown => {
            let html = markdown::to_html(&text);
            (text, Some(html))
        }
    }
}

fn recipient_count(d: &Draft) -> usize {
    d.to.len() + d.cc.len() + d.bcc.len()
}

fn check_size(d: &Draft) -> CmdResult<()> {
    if recipient_count(d) > MAX_RECIPIENTS {
        return Err(CmdError::invalid(format!(
            "at most {MAX_RECIPIENTS} recipients per message"
        )));
    }
    Ok(())
}

fn show_address(a: &Address) -> String {
    match a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} <{}>", a.email),
        None => a.email.clone(),
    }
}

fn file_out(a: &OutgoingAttachment) -> DraftFileOut {
    let size = match a {
        OutgoingAttachment::File { data_base64, .. } => {
            let pad = data_base64.bytes().rev().take_while(|&b| b == b'=').count();
            (data_base64.len() / 4 * 3).saturating_sub(pad) as u64
        }
        OutgoingAttachment::Gmail { size, .. } => *size,
    };
    DraftFileOut {
        filename: a.filename().to_string(),
        mime_type: a.mime_type().to_string(),
        size,
    }
}

/// `"Name <a@b.example>"` or `"a@b.example"` (a quoted name is unquoted).
pub fn parse_address(raw: &str) -> CmdResult<Address> {
    let s = raw.trim();
    let bad = |why: &str| CmdError::invalid(format!("{s:?} isn't an email address ({why})"));
    let (name, email) = match s.rfind('<') {
        Some(open) => {
            if !s.ends_with('>') {
                return Err(bad("unclosed <"));
            }
            let name = s[..open].trim().trim_matches('"').trim();
            (name, s[open + 1..s.len() - 1].trim())
        }
        None => ("", s),
    };
    if email.is_empty() || email.chars().count() > 254 {
        return Err(bad("empty or too long"));
    }
    if email
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "<>()[],;:\"\\".contains(c))
    {
        return Err(bad("contains a character addresses can't have"));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(bad("no @"));
    };
    if local.is_empty()
        || local.len() > 64
        || domain.contains('@')
        || !domain.contains('.')
        || domain.split('.').any(str::is_empty)
    {
        return Err(bad("not user@domain"));
    }
    if name.chars().any(char::is_control) || name.chars().count() > MAX_NAME_CHARS {
        return Err(bad("bad display name"));
    }
    Ok(Address {
        name: (!name.is_empty()).then(|| name.to_string()),
        email: email.to_string(),
    })
}

fn parse_addresses(field: &str, list: &[String]) -> CmdResult<Vec<Address>> {
    if list.len() > MAX_RECIPIENTS {
        return Err(CmdError::invalid(format!(
            "{field}: at most {MAX_RECIPIENTS} recipients"
        )));
    }
    list.iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            parse_address(s).map_err(|e| {
                CmdError::new(ErrorCode::InvalidInput, format!("{field}: {}", e.message))
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "writes_tests.rs"]
pub(crate) mod tests;
