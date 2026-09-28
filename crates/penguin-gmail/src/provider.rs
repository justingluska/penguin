//! Gmail behind the provider seam: [`GmailProvider`] (one account's
//! `MailProvider`) and [`GmailBackend`] (the kind's `Backend`: clients,
//! Keychain, sync tasks). Both only delegate to the existing Gmail code
//! (`api`, `drafts`, `details`, `sync`), so Gmail behaves exactly as it did
//! when the app called that code directly.

use std::sync::Arc;

use penguin_core::query::ParsedQuery;
use penguin_core::{
    Account, AccountProvider, Address, AttachmentMeta, Label, Message, SentRef, Store, SyncStatus,
};
use penguin_provider::compose::{AttachmentBytes, Draft};
use penguin_provider::window::WindowPolicy;
use penguin_provider::{
    async_trait, Backend, DraftRef, LabelUpdate, MailProvider, MessageMetadata, OpenedDraft,
    ServerSearch, SyncHandle,
};

use crate::api::GmailClient;
use crate::auth::AuthManager;
use crate::sync::{self as gsync, SyncEngine, WindowApi};
use crate::{drafts, Error};

type PResult<T> = penguin_provider::Result<T>;

/// One Gmail account through the provider seam.
#[derive(Clone)]
pub struct GmailProvider {
    account_id: String,
    client: GmailClient,
    store: Store,
}

impl GmailProvider {
    pub fn new(account_id: &str, client: GmailClient, store: Store) -> GmailProvider {
        GmailProvider {
            account_id: account_id.to_string(),
            client,
            store,
        }
    }

    /// The underlying Gmail client (for Gmail-only features).
    pub fn client(&self) -> &GmailClient {
        &self.client
    }
}

/// Gmail's label-name rejections as user-facing input errors (409: the name
/// is taken or clashes with a system label; 400: e.g. a reserved name).
fn label_error(e: Error) -> penguin_provider::Error {
    match e {
        Error::Http { status: 409, .. } => {
            penguin_provider::Error::InvalidInput("A label with that name already exists.".into())
        }
        Error::Http { status: 400, body } => penguin_provider::Error::InvalidInput(format!(
            "Gmail didn't accept that change: {body}"
        )),
        e => e.into(),
    }
}

#[async_trait]
impl MailProvider for GmailProvider {
    fn account_id(&self) -> &str {
        &self.account_id
    }

    fn provider(&self) -> AccountProvider {
        AccountProvider::Gmail
    }

    async fn fetch_messages(&self, ids: &[String]) -> Vec<(String, PResult<Option<Message>>)> {
        WindowApi::full_now(&self.client, ids)
            .await
            .into_iter()
            .map(|(id, r)| (id, r.map_err(Into::into)))
            .collect()
    }

    async fn fetch_pending_bodies(&self, ids: &[String]) -> PResult<Vec<String>> {
        Ok(gsync::fetch_pending_bodies(&self.client, &self.store, &self.account_id, ids).await?)
    }

    async fn get_attachment(
        &self,
        message_id: &str,
        attachment: &AttachmentMeta,
    ) -> PResult<Vec<u8>> {
        Ok(self.client.get_attachment(message_id, attachment).await?)
    }

    async fn get_message_metadata(&self, message_id: &str) -> PResult<Option<MessageMetadata>> {
        Ok(self.client.get_message_metadata(message_id).await?)
    }

    async fn get_message_raw(&self, message_id: &str) -> PResult<Option<Vec<u8>>> {
        Ok(self.client.get_message_raw(message_id).await?)
    }

    async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> PResult<()> {
        Ok(self.client.modify_thread(thread_id, add, remove).await?)
    }

    async fn trash_thread(&self, thread_id: &str) -> PResult<()> {
        Ok(self.client.trash_thread(thread_id).await?)
    }

    /// threads.untrash, then add INBOX (untrash alone restores the labels
    /// the thread had, which may not include the inbox).
    async fn untrash_thread(&self, thread_id: &str) -> PResult<()> {
        self.client.untrash_thread(thread_id).await?;
        Ok(self
            .client
            .modify_thread(thread_id, &["INBOX".to_string()], &[])
            .await?)
    }

    async fn update_label(&self, label_id: &str, update: &LabelUpdate) -> PResult<Label> {
        let color = update
            .color
            .as_ref()
            .map(|c| c.as_ref().map(|c| (c.background.as_str(), c.text.as_str())));
        self.client
            .patch_label(label_id, update.name.as_deref(), color, update.hidden)
            .await
            .map_err(label_error)
    }

    async fn delete_label(&self, label_id: &str) -> PResult<()> {
        self.client
            .delete_label(label_id)
            .await
            .map_err(label_error)
    }

    async fn ensure_label(&self, name: &str) -> PResult<Label> {
        let find = |labels: Vec<Label>| {
            labels
                .into_iter()
                .find(|l| l.kind == "user" && l.name.eq_ignore_ascii_case(name))
        };
        if let Some(l) = find(self.client.list_labels().await?) {
            return Ok(l);
        }
        match self.client.create_label(name).await {
            Ok(l) => Ok(l),
            // Created meanwhile (another client, or our own call landing
            // before a lost response).
            Err(Error::Http { status: 409, .. }) => find(self.client.list_labels().await?)
                .ok_or_else(|| {
                    penguin_provider::Error::InvalidInput(format!(
                        "Gmail has a label that conflicts with \"{name}\"."
                    ))
                }),
            Err(e) => Err(label_error(e)),
        }
    }

    async fn send_raw(&self, raw: &[u8], thread_id: Option<&str>) -> PResult<SentRef> {
        let sent = self.client.send_raw(raw, thread_id).await?;
        Ok(SentRef {
            message_id: sent.id,
            thread_id: sent.thread_id,
        })
    }

    async fn save_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: Option<&str>,
        attachments: &[AttachmentBytes],
    ) -> PResult<DraftRef> {
        Ok(drafts::save(
            &self.client,
            &self.store,
            draft,
            from,
            draft_id,
            attachments,
        )
        .await?)
    }

    async fn send_draft(
        &self,
        draft: &Draft,
        from: &Address,
        draft_id: &str,
        attachments: &[AttachmentBytes],
    ) -> PResult<DraftRef> {
        Ok(drafts::send(
            &self.client,
            &self.store,
            draft,
            from,
            draft_id,
            attachments,
        )
        .await?)
    }

    async fn send_saved_draft(&self, draft_id: &str) -> PResult<SentRef> {
        Ok(self.client.send_saved_draft(draft_id).await?)
    }

    async fn delete_draft(&self, draft_id: &str) -> PResult<Option<String>> {
        Ok(drafts::delete(&self.client, &self.store, &self.account_id, draft_id).await?)
    }

    async fn open_draft(
        &self,
        draft_id: Option<&str>,
        message_id: Option<&str>,
    ) -> PResult<Option<OpenedDraft>> {
        Ok(drafts::open(
            &self.client,
            &self.store,
            &self.account_id,
            draft_id,
            message_id,
        )
        .await?)
    }

    /// The parsed query as Gmail `q` (`to_gmail_query`), then messages.list.
    async fn server_search(&self, query: &ParsedQuery, fetch_cap: usize) -> PResult<ServerSearch> {
        let (q, _) = gsync::to_gmail_query(query);
        Ok(
            gsync::search_server(&self.client, &self.store, &self.account_id, &q, fetch_cap)
                .await?,
        )
    }

    async fn window_estimate(&self, months: u32, now_ms: i64) -> PResult<u64> {
        Ok(gsync::window_estimate(&self.client, &self.account_id, months, now_ms).await?)
    }
}

/// The Gmail kind: one OAuth client (`AuthManager`) and one sync engine
/// for every Gmail account. Rebuilt by the app when the Google client
/// changes.
#[derive(Clone)]
pub struct GmailBackend {
    auth: AuthManager,
    engine: SyncEngine,
    store: Store,
}

impl GmailBackend {
    pub fn new(auth: AuthManager, engine: SyncEngine, store: Store) -> GmailBackend {
        GmailBackend {
            auth,
            engine,
            store,
        }
    }

    /// Google sign-in, tokens and scopes (calendar, contacts, account photo).
    pub fn auth(&self) -> &AuthManager {
        &self.auth
    }

    pub fn engine(&self) -> &SyncEngine {
        &self.engine
    }

    /// A Gmail client for `account` (Gmail-only features).
    pub fn gmail_client(&self, account: &Account) -> GmailClient {
        GmailClient::new(self.auth.clone(), &account.email)
    }
}

#[async_trait]
impl Backend for GmailBackend {
    fn provider(&self) -> AccountProvider {
        AccountProvider::Gmail
    }

    fn client(&self, account: &Account) -> PResult<Arc<dyn MailProvider>> {
        if account.provider != AccountProvider::Gmail {
            return Err(penguin_provider::Error::Other(format!(
                "{} is not a Gmail account",
                account.id
            )));
        }
        Ok(Arc::new(GmailProvider::new(
            &account.id,
            self.gmail_client(account),
            self.store.clone(),
        )))
    }

    fn has_credentials(&self, account: &Account) -> bool {
        self.auth.has_credentials(&account.email)
    }

    async fn sign_out(&self, account: &Account) -> PResult<()> {
        Ok(self.auth.sign_out(&account.email).await?)
    }

    fn start_sync(&self, account: &Account) -> SyncHandle {
        SyncHandle::new(Arc::new(self.engine.start_account(&account.id)))
    }

    fn retry_sync(&self, account: &Account) -> SyncHandle {
        SyncHandle::new(Arc::new(self.engine.retry_account(&account.id)))
    }

    fn sync_status(&self, account_id: &str) -> Option<SyncStatus> {
        self.engine.status(account_id)
    }

    fn set_window_policy(&self, policy: WindowPolicy) {
        self.engine.set_window_policy(policy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::GmailCursor;

    #[test]
    fn gmail_cursor_reads_what_the_store_migration_wrote() {
        // SCHEMA_SYNC_STATE's json_object('historyId', …, 'backfillPageToken', …).
        let c =
            GmailCursor::parse(r#"{"historyId":9007199254740993,"backfillPageToken":"page-7"}"#)
                .unwrap();
        assert_eq!(c.history_id, Some(9_007_199_254_740_993));
        assert_eq!(c.backfill_page_token.as_deref(), Some("page-7"));
        let c = GmailCursor::parse(r#"{"historyId":5,"backfillPageToken":null}"#).unwrap();
        assert_eq!(c.history_id, Some(5));
        assert_eq!(c.backfill_page_token, None);
        assert_eq!(GmailCursor::parse(r#"{"historyId":5}"#).unwrap(), c);
        assert_eq!(GmailCursor::parse(&c.to_state()).unwrap(), c);
        // Nothing recorded stays "".
        assert_eq!(GmailCursor::parse("").unwrap(), GmailCursor::default());
        assert_eq!(GmailCursor::default().to_state(), "");
        assert!(GmailCursor::parse("{not json").is_err());
    }

    #[test]
    fn errors_cross_the_seam_with_their_messages() {
        let cases = vec![
            Error::NotConfigured("x".into()),
            Error::NeedsReauth("ada@x.example".into()),
            Error::OAuth("bad".into()),
            Error::SignInCancelled,
            Error::Keychain("denied".into()),
            Error::Http {
                status: 503,
                body: "later".into(),
            },
            Error::HistoryExpired,
            Error::RateLimited,
            Error::Network("offline".into()),
            Error::Other("odd".into()),
        ];
        for e in cases {
            let text = e.to_string();
            let p: penguin_provider::Error = e.into();
            assert_eq!(p.to_string(), text);
        }
        let p: penguin_provider::Error = Error::Http {
            status: 404,
            body: String::new(),
        }
        .into();
        assert!(p.is_not_found());
    }

    #[test]
    fn label_rejections_read_as_input_errors() {
        let e = label_error(Error::Http {
            status: 409,
            body: "exists".into(),
        });
        assert!(matches!(e, penguin_provider::Error::InvalidInput(_)));
        assert_eq!(e.to_string(), "A label with that name already exists.");
        let e = label_error(Error::Http {
            status: 400,
            body: "Invalid label name".into(),
        });
        assert_eq!(
            e.to_string(),
            "Gmail didn't accept that change: Invalid label name"
        );
        assert!(matches!(
            label_error(Error::Network("x".into())),
            penguin_provider::Error::Network(_)
        ));
    }
}
