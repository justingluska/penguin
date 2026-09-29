//! The Penguin app's side of agent access (the logic is Tauri-free in
//! agent/): it serves the agent socket (agent/ipc.rs) with the app's own
//! providers, announces queued agent sends, and backs Settings → Developer
//! → Agents: raising the level to send (after the confirmation), recent
//! agent activity from the audit log, and the sends waiting to go.

use std::path::PathBuf;
use std::sync::Arc;

use penguin_core::Store;
use penguin_provider::{async_trait, MailProvider};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::agent::audit::{self, AuditLog};
use crate::agent::ipc;
use crate::agent::writes::{AgentService, QueuedSend, WriteHost};
use crate::error::{CmdError, CmdResult};
use crate::ops::Paths;
use crate::settings::{AgentAccess, Settings};
use crate::state::{blocking, AppState};

/// Payload `QueuedSend`: an agent's send is waiting in the outbox.
pub const EVENT_AGENT_SEND_QUEUED: &str = "penguin://agent-send-queued";

/// What the user types to let agents send (Settings shows it).
pub const SEND_ACKNOWLEDGEMENT: &str = "I understand";

struct AppHost {
    state: Arc<AppState>,
    app: AppHandle,
    audit: AuditLog,
    share: Arc<crate::share::Share>,
}

#[async_trait]
impl WriteHost for AppHost {
    fn store(&self) -> &Store {
        &self.state.store
    }
    fn paths(&self) -> &Paths {
        &self.state.paths
    }
    fn settings(&self) -> Settings {
        self.state.settings.get()
    }
    async fn provider(&self, account_id: &str) -> CmdResult<Arc<dyn MailProvider>> {
        self.state.provider(account_id).await
    }
    fn drafts_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.state.drafts_lock
    }
    fn mail_changed(&self, account_id: &str, thread_ids: Vec<String>) {
        self.state.emit_mail_changed(account_id, thread_ids);
        self.state.poke(account_id);
    }
    fn send_queued(&self, q: &QueuedSend) {
        if let Err(e) = self.app.emit(EVENT_AGENT_SEND_QUEUED, q.clone()) {
            tracing::warn!(error = %e, "could not emit agent-send-queued");
        }
        let who = match q.to.as_slice() {
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} and {} more", rest.len()),
            [] => "no one".into(),
        };
        let when = match q.delay_seconds {
            s if s < 60 => format!("{s} seconds"),
            60 => "a minute".into(),
            s => format!("{} minutes", s / 60),
        };
        crate::notify::show(
            &self.app,
            crate::notify::Note {
                title: format!("An agent is sending to {who}"),
                body: format!(
                    "“{}” goes in {when}. Open Penguin to cancel it.",
                    if q.subject.is_empty() {
                        "(no subject)"
                    } else {
                        &q.subject
                    }
                ),
                open: Some(crate::notify::NotificationOpen {
                    account_id: q.account_id.clone(),
                    thread_id: Some(q.thread_id.clone()),
                }),
            },
        );
    }
    fn audit(&self) -> &AuditLog {
        &self.audit
    }
    fn share(&self) -> Arc<crate::share::Share> {
        self.share.clone()
    }
}

/// At startup: the agent tables, the sends a lowered level must not let
/// go (the level may have changed while Penguin was closed), and the
/// socket. Failing to bind only disables agent writes; the app runs on.
/// `share` answers `create_share_link` (agent/sharing.rs).
pub fn start(app: &AppHandle, state: Arc<AppState>, share: Arc<crate::share::Share>) {
    if let Err(e) = state.store.migrate_agent() {
        tracing::error!(error = %e, "could not set up the agent tables; agent drafts are off");
        return;
    }
    if state.settings.get().mcp.access < AgentAccess::Send {
        cancel_queued_sends(&state.store);
    }
    let listener = match ipc::Listener::bind(&state.paths) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "agent socket not started; penguin-cli can't draft or send");
            return;
        }
    };
    tracing::info!("agent socket ready");
    let host = AppHost {
        state,
        app: app.clone(),
        audit: AuditLog::open(crate::agent::log_dir().as_deref()),
        share,
    };
    let service = Arc::new(AgentService::new(Arc::new(host)));
    tauri::async_runtime::spawn(async move {
        if let Err(e) = listener.serve(service).await {
            tracing::warn!(error = %e, "agent socket stopped");
        }
    });
}

fn cancel_queued_sends(store: &Store) -> usize {
    match store.cancel_agent_sends() {
        Ok(cancelled) => {
            if !cancelled.is_empty() {
                tracing::info!(count = cancelled.len(), "cancelled queued agent sends");
                crate::outbox::schedule_changed();
            }
            cancelled.len()
        }
        Err(e) => {
            tracing::error!(error = %e, "could not cancel queued agent sends");
            0
        }
    }
}

/// After a settings change: lowering the level below send cancels every
/// send an agent queued (the drafts stay in Drafts). Everything else about
/// a downgrade needs nothing: each request checks the live level.
pub async fn level_changed(state: &Arc<AppState>, before: AgentAccess, after: AgentAccess) {
    if before != after {
        tracing::info!(
            from = before.as_str(),
            to = after.as_str(),
            "agent level changed"
        );
    }
    if before == AgentAccess::Send && after < AgentAccess::Send {
        let store = state.store.clone();
        let _ = blocking(move || Ok(cancel_queued_sends(&store))).await;
    }
}

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Whether the confirmation's text is the acknowledgement (any case, outer
/// spaces ignored).
pub fn acknowledged(typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case(SEND_ACKNOWLEDGEMENT)
}

/// Settings → Developer → Agents → "Read, draft and send", after the
/// confirmation: `acknowledgement` is what the user typed into it, and it
/// must say "I understand". A patch through `update_settings` can't do this.
#[tauri::command]
pub async fn enable_agent_send(
    state: AppStateRef<'_>,
    acknowledgement: String,
) -> CmdResult<Settings> {
    if !acknowledged(&acknowledgement) {
        return Err(CmdError::invalid(format!(
            "Type “{SEND_ACKNOWLEDGEMENT}” to let agents send"
        )));
    }
    let st = state.inner().clone();
    let saved = blocking(move || Ok(st.settings.grant_agent_send()?)).await?;
    tracing::info!("agents may send (confirmed in Settings)");
    let saved = crate::commands::settings_view(&state, saved).await?;
    state.emit_settings_changed(saved.clone());
    Ok(saved)
}

/// One line of Settings → Developer → "Recent agent activity". Mirrored in
/// types.ts (`AgentActivity`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivity {
    pub ts: String,
    pub tool: String,
    /// `mcp` or `cli` (reads answered by the MCP server say `mcp`).
    pub via: String,
    pub ok: bool,
    pub error_code: Option<String>,
    pub account: Option<String>,
    pub recipient_count: Option<u64>,
    pub attachment_count: Option<u64>,
    pub draft_id: Option<String>,
    pub send_at: Option<String>,
    pub result_count: Option<u64>,
    /// A read's query or question (what the agent asked), cut short.
    pub detail: Option<String>,
}

fn activity_from(v: &serde_json::Value) -> Option<AgentActivity> {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64());
    let detail = v.get("args").and_then(|a| {
        ["query", "question", "view"]
            .iter()
            .find_map(|k| a.get(*k).and_then(|x| x.as_str()))
            .map(|q| q.chars().take(80).collect())
    });
    Some(AgentActivity {
        ts: s("ts")?,
        tool: s("tool")?,
        via: s("via").unwrap_or_else(|| "mcp".into()),
        ok: v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
        error_code: s("errorCode"),
        account: s("account"),
        recipient_count: n("recipientCount"),
        attachment_count: n("attachmentCount"),
        draft_id: s("draftId"),
        send_at: s("sendAt"),
        result_count: n("resultCount"),
        detail,
    })
}

pub fn read_activity(log: Option<PathBuf>, limit: usize) -> Vec<AgentActivity> {
    log.map(|p| audit::read_recent(&p, limit))
        .unwrap_or_default()
        .iter()
        .filter_map(activity_from)
        .collect()
}

/// The newest agent calls, newest first (at most 100).
#[tauri::command]
pub async fn agent_activity(limit: Option<usize>) -> CmdResult<Vec<AgentActivity>> {
    let limit = limit.unwrap_or(30).min(100);
    let log = crate::agent::log_dir().map(|d| d.join(audit::AUDIT_FILE));
    blocking(move || Ok(read_activity(log, limit))).await
}

/// A send an agent queued that hasn't gone yet. Mirrored in types.ts.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPendingSend {
    pub schedule_id: String,
    pub account_id: String,
    pub draft_id: String,
    pub thread_id: Option<String>,
    pub send_at: i64,
    pub to: Vec<String>,
    pub subject: String,
}

pub fn pending_sends(store: &Store) -> CmdResult<Vec<AgentPendingSend>> {
    let mut out = Vec::new();
    for row in store.list_agent_drafts(None)? {
        let Some(id) = &row.send_schedule_id else {
            continue;
        };
        let Some(s) = store.get_scheduled_send(&row.account_id, id)? else {
            continue;
        };
        let m = match &row.message_id {
            Some(mid) => store.get_message(&row.account_id, mid)?,
            None => None,
        };
        out.push(AgentPendingSend {
            schedule_id: s.id,
            account_id: row.account_id.clone(),
            draft_id: row.draft_id.clone(),
            thread_id: m.as_ref().map(|m| m.thread_id.clone()),
            send_at: s.send_at,
            to: m
                .as_ref()
                .map(|m| {
                    m.to.iter()
                        .chain(&m.cc)
                        .chain(&m.bcc)
                        .map(|a| a.email.clone())
                        .collect()
                })
                .unwrap_or_default(),
            subject: m.map(|m| m.subject).unwrap_or_default(),
        });
    }
    out.sort_by_key(|p| p.send_at);
    Ok(out)
}

/// Sends agents queued that are still waiting, soonest first. Cancel one
/// with `cancel_scheduled_send`.
#[tauri::command]
pub async fn agent_pending_sends(state: AppStateRef<'_>) -> CmdResult<Vec<AgentPendingSend>> {
    let store = state.store.clone();
    blocking(move || pending_sends(&store)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::audit::{AppAuditEntry, AuditEntry};

    #[test]
    fn activity_reads_both_kinds_of_line_newest_first() {
        let dir = std::env::temp_dir().join(format!("pg-activity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = AuditLog::open(Some(&dir));
        log.record(&AuditEntry {
            ts: "2026-01-01T00:00:00Z".into(),
            tool: "search",
            args: serde_json::json!({"query": "from:bo walrus"}),
            ok: true,
            result_count: 3,
            error_code: None,
            ms: 1.0,
        });
        log.record(&AppAuditEntry {
            ts: "2026-01-01T00:00:01Z".into(),
            tool: "send_message".into(),
            via: "cli".into(),
            ok: false,
            error_code: Some("permissionDenied".into()),
            account: Some("ada@penguin.example".into()),
            recipient_count: Some(2),
            ..AppAuditEntry::default()
        });
        std::fs::write(dir.join("junk"), "not json\n").unwrap();
        let rows = read_activity(Some(dir.join(audit::AUDIT_FILE)), 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tool, "send_message");
        assert_eq!(rows[0].via, "cli");
        assert_eq!(rows[0].error_code.as_deref(), Some("permissionDenied"));
        assert_eq!(rows[0].recipient_count, Some(2));
        assert_eq!(rows[1].tool, "search");
        assert_eq!(rows[1].via, "mcp");
        assert_eq!(rows[1].detail.as_deref(), Some("from:bo walrus"));
        assert!(read_activity(None, 10).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sending_needs_the_typed_acknowledgement() {
        assert!(acknowledged("I understand"));
        assert!(acknowledged("  i understand "));
        for no in ["", "yes", "I understand!", "understand", "true"] {
            assert!(!acknowledged(no), "{no:?}");
        }
    }

    #[test]
    fn pending_sends_are_the_agents_queued_ones() {
        let store = Store::open_in_memory().unwrap();
        store.migrate_agent().unwrap();
        let a = "ada@penguin.example";
        store.put_agent_draft(a, "d1", "mcp", None, 1).unwrap();
        store
            .upsert_scheduled_send(&penguin_core::ScheduledSend {
                id: "s1".into(),
                account_id: a.into(),
                draft_id: "d1".into(),
                send_at: 99,
                created_at: 1,
                remind_after_ms: None,
                attempts: 0,
                last_error: None,
            })
            .unwrap();
        assert!(
            pending_sends(&store).unwrap().is_empty(),
            "not recorded as the agent's yet"
        );
        store.set_agent_send(a, "d1", Some("s1")).unwrap();
        let p = pending_sends(&store).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].schedule_id.as_str(), p[0].send_at), ("s1", 99));
        store.cancel_agent_sends().unwrap();
        assert!(pending_sends(&store).unwrap().is_empty());
    }
}
