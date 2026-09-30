//! `create_share_link` (MCP) and `penguin-cli share-link`: an agent asks the
//! running app for a share link to one attachment of one message
//! (docs/SHARE-LINKS.md). The app answers over the agent socket
//! (agent/ipc.rs) through `share::share_attachment` with `Caller::Agent`,
//! with its own storage settings and Keychain secret; the CLI process never
//! touches the storage or its secret.
//!
//! Two gates, both checked by the app on every request:
//! - the agent level is at least "Read, organize and draft" (agent/permission.rs,
//!   `SHARE_TOOLS`: publishing a file is a write);
//! - share links are set up and "Let agents (CLI and MCP) create share
//!   links" is on (`share::agent_gate`, Settings → Share links).
//!
//! Agents name attachments, embedded (cid:) pictures included, never a
//! picture by its web address. The audit line holds the tool, the account
//! and the outcome: never the link (a bearer credential), the object key or
//! the file's name.

use std::fmt;
use std::sync::Arc;

use penguin_core::Store;
use penguin_provider::{async_trait, MailProvider};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::audit::AppAuditEntry;
use super::files;
use super::output::iso;
use super::writes::WriteHost;
use crate::error::{CmdError, CmdResult};
use crate::ops::Paths;
use crate::share::{self, Caller, ShareRequest, ShareSource};
use crate::state::blocking;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ShareLinkArgs {
    /// Account email the message belongs to (accountId in search results).
    pub account_id: String,
    pub message_id: String,
    /// Attachment id from list_attachments or get_thread, or an embedded
    /// picture's Content-ID ("cid:…"). Never a web address: remote pictures
    /// can't be shared by agents.
    pub attachment_id: String,
}

/// `shareLink` (create_share_link, `penguin-cli share-link --json`).
#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ShareLinkOut {
    /// The link. Anyone who has it can download the file until it expires:
    /// treat it like a password to that one file.
    pub url: String,
    /// Unix ms when the link stops working.
    pub expires_at: i64,
    pub expires_at_iso: String,
    /// The file's name (what a download is called).
    pub name: String,
    /// Bytes.
    pub size: u64,
}

impl fmt::Debug for ShareLinkOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShareLinkOut")
            .field("url", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// The agent host as the share seam's byte source (its store, attachment
/// cache and providers).
struct HostSource<'a, H>(&'a H);

#[async_trait]
impl<H: WriteHost> ShareSource for HostSource<'_, H> {
    fn store(&self) -> &Store {
        self.0.store()
    }
    fn paths(&self) -> &Paths {
        self.0.paths()
    }
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>> {
        self.0.provider(account_id).await
    }
}

/// A picture by its address (https, http, data:) instead of an attachment id.
fn is_web_address(id: &str) -> bool {
    let id = id.trim().to_ascii_lowercase();
    id.contains("://") || id.starts_with("data:") || id.starts_with("//")
}

/// Share one attachment (or embedded picture) for an agent. The caller
/// (`AgentService::dispatch`) has already checked the agent level.
pub(crate) async fn create<H: WriteHost>(
    host: &H,
    args: ShareLinkArgs,
    audit: &mut AppAuditEntry,
) -> CmdResult<ShareLinkOut> {
    let account = args.account_id.trim().to_lowercase();
    audit.account = Some(account.clone());
    let share = host.share();
    // The share-link switch first: nothing is looked up for an agent the
    // user hasn't allowed to share.
    share::agent_gate(&share.config())?;
    if is_web_address(&args.attachment_id) {
        return Err(CmdError::denied(
            "Agents can share a message's attachments and embedded (cid:) pictures, never a picture by its web address. \
             list_attachments shows the ids.",
        ));
    }
    // By id, or an embedded picture by its Content-ID (as get_attachment).
    let (store, a, m, id) = (
        host.store().clone(),
        account.clone(),
        args.message_id.clone(),
        args.attachment_id.clone(),
    );
    let meta = blocking(move || files::find_in(&store, &a, &m, &id)).await?;
    let link = share::share_attachment(
        &HostSource(host),
        &share,
        Caller::Agent,
        ShareRequest::Attachment {
            account_id: account,
            message_id: args.message_id,
            attachment_id: meta.id,
        },
    )
    .await?;
    audit.result_count = 1;
    Ok(ShareLinkOut {
        url: link.url,
        expires_at: link.expires_at,
        expires_at_iso: iso(link.expires_at),
        name: link.name,
        size: link.size,
    })
}

#[cfg(test)]
mod tests {
    //! The app's side of create_share_link against the fake provider and the
    //! in-process S3 stub (share/stub.rs): the gate matrix, remote pictures,
    //! embedded pictures, and what the audit log keeps.

    use serde_json::{json, Value};

    use super::*;
    use crate::agent::ipc::Handler;
    use crate::agent::permission;
    use crate::agent::writes::tests::{TestHost, ADA};
    use crate::agent::writes::AgentService;
    use crate::error::ErrorCode;
    use crate::settings::AgentAccess;
    use crate::share::config::StoredConfig;
    use crate::share::stub::Stub;

    const LEVELS: [AgentAccess; 4] = [
        AgentAccess::Off,
        AgentAccess::Read,
        AgentAccess::Draft,
        AgentAccess::Send,
    ];
    const PDF: &[u8] = b"%PDF-1.7 fictional walrus budget";

    /// m1 gets a PDF (cached, as if previewed) and an embedded picture (on
    /// the fake server only).
    fn with_files(host: &TestHost) {
        let mut m = host.store.get_message(ADA, "m1").unwrap().unwrap();
        m.attachments = vec![
            penguin_core::AttachmentMeta {
                id: "att-pdf".into(),
                filename: "Walrus budget.pdf".into(),
                mime_type: "application/pdf".into(),
                size: PDF.len() as u64,
                content_id: None,
                inline: false,
            },
            penguin_core::AttachmentMeta {
                id: "att-img".into(),
                filename: "chart.png".into(),
                mime_type: "image/png".into(),
                size: 4,
                content_id: Some("<chart@acme.example>".into()),
                inline: true,
            },
        ];
        host.store.upsert_messages(&[m]).unwrap();
        crate::attachments::put_cached(&host.paths, ADA, "m1", "att-pdf", PDF).unwrap();
        host.fake().seed_attachment("m1", "att-img", b"\x89PNG");
    }

    async fn call(host: &Arc<TestHost>, args: Value) -> CmdResult<Value> {
        AgentService::new(host.clone())
            .handle("mcp", "create_share_link", args)
            .await
    }

    fn pdf_args() -> Value {
        json!({"accountId": ADA, "messageId": "m1", "attachmentId": "att-pdf"})
    }

    /// Every level × set up or not × agent switch on or off: only "Read
    /// and draft" or higher with both switches shares; everything else is
    /// a permission error that names what to turn on, and uploads nothing.
    #[tokio::test]
    async fn the_gate_matrix() {
        let stub = Stub::start().await;
        let host = TestHost::new("share-gate", AgentAccess::Off);
        with_files(&host);
        let mut uploads = 0;
        for level in LEVELS {
            for configured in [false, true] {
                for allow in [false, true] {
                    host.set_level(level);
                    host.set_share(configured.then_some(stub.endpoint.as_str()), allow);
                    let r = call(&host, pdf_args()).await;
                    let want = level >= AgentAccess::Draft && configured && allow;
                    let case = format!("{level:?} configured={configured} allow={allow}");
                    assert_eq!(
                        permission::allows(level, "create_share_link") && configured && allow,
                        want,
                        "{case}"
                    );
                    match r {
                        Ok(v) => {
                            assert!(want, "{case}: shared");
                            assert_eq!(v["kind"], "shareLink");
                            uploads += 1;
                        }
                        Err(e) => {
                            assert!(!want, "{case}: {e:?}");
                            assert_eq!(e.code, ErrorCode::PermissionDenied, "{case}");
                            let where_ = if level < AgentAccess::Draft {
                                "Settings → Developer → Agents"
                            } else {
                                "Settings → Share links"
                            };
                            assert!(e.message.contains(where_), "{case}: {}", e.message);
                        }
                    }
                    assert_eq!(stub.objects.lock().unwrap().len(), uploads, "{case}");
                }
            }
        }
        assert_eq!(uploads, 2, "Draft and Send, both switches on");
    }

    #[tokio::test]
    async fn a_link_downloads_the_file_and_the_audit_keeps_no_link_or_name() {
        let stub = Stub::start().await;
        let host = TestHost::new("share-ok", AgentAccess::Draft);
        with_files(&host);
        host.set_share(Some(&stub.endpoint), true);
        let v = call(&host, pdf_args()).await.unwrap();
        let d = &v["data"];
        assert_eq!(d["name"], "Walrus budget.pdf");
        assert_eq!(d["size"], PDF.len());
        let expires = d["expiresAt"].as_i64().unwrap();
        assert!(expires > crate::ops::now_ms());
        assert_eq!(d["expiresAtIso"], iso(expires));
        let url = d["url"].as_str().unwrap().to_string();
        // Uploaded once, and the link downloads it (from the cache: no
        // provider download).
        let got = reqwest::get(&url).await.unwrap();
        assert_eq!(got.status(), 200);
        assert_eq!(got.bytes().await.unwrap().as_ref(), PDF);
        assert!(host
            .fake()
            .calls()
            .iter()
            .all(|c| !c.starts_with("get_attachment")));
        let put = stub.seen().into_iter().find(|s| s.method == "PUT").unwrap();
        let key = put.path.trim_start_matches("/penguin-shares/").to_string();

        // The audit line: the tool, who asked, the account, the outcome.
        let line = host.audit_lines().pop().unwrap();
        assert_eq!(line["tool"], "create_share_link");
        assert_eq!(line["via"], "mcp");
        assert_eq!(line["account"], ADA);
        assert_eq!(line["ok"], true);
        let text = line.to_string();
        for secret in [
            url.as_str(),
            key.as_str(),
            "Walrus budget",
            "Walrus-budget",
            "X-Amz",
        ] {
            assert!(!text.contains(secret), "audit line has {secret:?}: {text}");
        }
        let _ = std::fs::remove_dir_all(host.root.join("logs"));
    }

    #[tokio::test]
    async fn an_embedded_picture_is_shared_by_its_content_id() {
        let stub = Stub::start().await;
        let host = TestHost::new("share-cid", AgentAccess::Draft);
        with_files(&host);
        host.set_share(Some(&stub.endpoint), true);
        let v = call(
            &host,
            json!({"accountId": ADA, "messageId": "m1", "attachmentId": "cid:chart@acme.example"}),
        )
        .await
        .unwrap();
        assert_eq!(v["data"]["name"], "chart.png");
        // Not cached: the app downloaded it with its own provider.
        let got = reqwest::get(v["data"]["url"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(got.bytes().await.unwrap().as_ref(), b"\x89PNG");
        // An id that isn't on the message: not found, nothing uploaded.
        let before = stub.objects.lock().unwrap().len();
        let e = call(
            &host,
            json!({"accountId": ADA, "messageId": "m1", "attachmentId": "cid:other@acme.example"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert_eq!(stub.objects.lock().unwrap().len(), before);
    }

    /// Remote pictures are never shared for an agent: not by address as an
    /// id, not through the seam's picture request, and no extra argument
    /// sneaks one in. Nothing is fetched or uploaded.
    #[tokio::test]
    async fn a_remote_picture_is_refused() {
        let stub = Stub::start().await;
        let host = TestHost::new("share-remote", AgentAccess::Send);
        with_files(&host);
        host.set_share(Some(&stub.endpoint), true);
        for id in [
            "https://pictures.acme.example/chart.png",
            "HTTP://pictures.acme.example/chart.png",
            "//pictures.acme.example/chart.png",
            "data:image/png;base64,iVBORw0KGgo=",
        ] {
            let e = call(
                &host,
                json!({"accountId": ADA, "messageId": "m1", "attachmentId": id}),
            )
            .await
            .unwrap_err();
            assert_eq!(e.code, ErrorCode::PermissionDenied, "{id}");
            assert!(e.message.contains("web address"), "{id}: {}", e.message);
        }
        // The arguments have no field for a picture's address.
        let e = call(
            &host,
            json!({"accountId": ADA, "messageId": "m1", "attachmentId": "att-pdf", "src": "https://pictures.acme.example/x.png"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidInput);
        // And the seam itself refuses an agent's picture request.
        let e = share::share_attachment(
            &HostSource(host.as_ref()),
            &host.share(),
            Caller::Agent,
            ShareRequest::Picture {
                account_id: ADA.into(),
                message_id: "m1".into(),
                src: "https://pictures.acme.example/chart.png".into(),
                name: "chart.png".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert!(stub.seen().is_empty(), "nothing reached the storage");
        assert!(stub.objects.lock().unwrap().is_empty());
    }

    #[test]
    fn web_addresses_are_told_apart_from_ids() {
        for yes in [
            "https://a.example/x.png",
            " http://a.example/x.png",
            "//a.example/x",
            "DATA:image/png;base64,AA",
            "ftp://a.example/x",
        ] {
            assert!(is_web_address(yes), "{yes}");
        }
        for no in ["att-1", "cid:chart@acme.example", "ANGjdJ9_x-Y", "0.1"] {
            assert!(!is_web_address(no), "{no}");
        }
    }

    #[test]
    fn a_link_never_shows_in_debug() {
        let out = ShareLinkOut {
            url: "https://storage.example/k?X-Amz-Signature=abc".into(),
            expires_at: 1,
            expires_at_iso: iso(1),
            name: "a.pdf".into(),
            size: 1,
        };
        let shown = format!("{out:?}");
        assert!(
            !shown.contains("X-Amz") && !shown.contains("a.pdf"),
            "{shown}"
        );
        // A config the user never touched lets no agent share.
        assert!(!share::agents_allowed(&StoredConfig::default()));
    }
}
