//! Drafts an AI agent created through `penguin-cli` / its MCP server
//! (apps/desktop/src-tauri/src/agent/writes.rs). Agents may change, delete
//! or send only drafts listed here, and the app cancels the sends an agent
//! queued when the user lowers the agent level. Local only: nothing marks
//! the draft on the provider.
//!
//! Its own schema (`agent`, through [`Store::migrate_provider_schema`]) so
//! it never reorders the store's migrations; rows go with their account.

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::Store;
use crate::Result;

pub const AGENT_SCHEMA: &str = "agent";

/// `send_schedule_id`: the scheduled send (store_outbox.rs) an agent queued
/// for this draft, so a downgrade cancels exactly the sends agents queued
/// and never one the user scheduled from the composer.
const AGENT_V1: &str = r#"
CREATE TABLE agent_drafts(
    account_id TEXT NOT NULL,
    draft_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    client TEXT NOT NULL,
    reply_to_message_id TEXT,
    send_schedule_id TEXT,
    PRIMARY KEY(account_id, draft_id)
) WITHOUT ROWID;
"#;

/// One agent-created draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDraft {
    pub account_id: String,
    pub draft_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// `mcp` or `cli`.
    pub client: String,
    /// The message it replies to (same account), kept here so an update
    /// re-threads it whatever the provider's reopened draft remembers.
    pub reply_to_message_id: Option<String>,
    /// The scheduled send an agent queued, while it is still pending.
    pub send_schedule_id: Option<String>,
    /// The draft's current message id (from the drafts mapping); None when
    /// the draft is gone (sent, or deleted in Gmail).
    pub message_id: Option<String>,
}

const COLS: &str = "a.account_id, a.draft_id, a.created_at, a.updated_at, a.client, a.reply_to_message_id, a.send_schedule_id, d.message_id";
const FROM: &str =
    "agent_drafts a LEFT JOIN drafts d ON d.account_id = a.account_id AND d.draft_id = a.draft_id";

fn row(r: &Row) -> rusqlite::Result<AgentDraft> {
    Ok(AgentDraft {
        account_id: r.get(0)?,
        draft_id: r.get(1)?,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
        client: r.get(4)?,
        reply_to_message_id: r.get(5)?,
        send_schedule_id: r.get(6)?,
        message_id: r.get(7)?,
    })
}

impl Store {
    /// Create or update the agent tables. Call once at startup (writes).
    pub fn migrate_agent(&self) -> Result<()> {
        self.migrate_provider_schema(AGENT_SCHEMA, &[AGENT_V1], &["agent_drafts"])
    }

    /// Record a draft an agent created (or touch one it updated; its
    /// client and reply are kept unless a reply is given).
    pub fn put_agent_draft(
        &self,
        account_id: &str,
        draft_id: &str,
        client: &str,
        reply_to_message_id: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO agent_drafts(account_id, draft_id, created_at, updated_at, client, reply_to_message_id)
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5)
                 ON CONFLICT(account_id, draft_id) DO UPDATE SET updated_at = excluded.updated_at,
                   reply_to_message_id = coalesce(excluded.reply_to_message_id, reply_to_message_id)",
                params![account_id, draft_id, now_ms, client, reply_to_message_id],
            )?;
            Ok(())
        })
    }

    pub fn get_agent_draft(&self, account_id: &str, draft_id: &str) -> Result<Option<AgentDraft>> {
        self.read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {COLS} FROM {FROM} WHERE a.account_id = ?1 AND a.draft_id = ?2"
            ))?
            .query_row(params![account_id, draft_id], row)
            .optional()?)
        })
    }

    /// Agent drafts whose id is `draft_id`, in any account (a caller that
    /// didn't name the account).
    pub fn find_agent_drafts(&self, draft_id: &str) -> Result<Vec<AgentDraft>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLS} FROM {FROM} WHERE a.draft_id = ?1 ORDER BY a.account_id"
            ))?;
            let rows = stmt.query_map([draft_id], row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Newest first; `account_id` None = every account. Includes drafts
    /// that are gone (`message_id` None) until [`Store::prune_agent_drafts`].
    pub fn list_agent_drafts(&self, account_id: Option<&str>) -> Result<Vec<AgentDraft>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLS} FROM {FROM} WHERE ?1 IS NULL OR a.account_id = ?1
                 ORDER BY a.updated_at DESC, a.draft_id"
            ))?;
            let rows = stmt.query_map([account_id], row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn remove_agent_draft(&self, account_id: &str, draft_id: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM agent_drafts WHERE account_id = ?1 AND draft_id = ?2",
                params![account_id, draft_id],
            )?;
            Ok(())
        })
    }

    /// Forget agent drafts that no longer exist (sent, or deleted outside
    /// Penguin). Returns how many rows went.
    pub fn prune_agent_drafts(&self) -> Result<usize> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM agent_drafts WHERE NOT EXISTS (SELECT 1 FROM drafts d
                   WHERE d.account_id = agent_drafts.account_id AND d.draft_id = agent_drafts.draft_id)",
                [],
            )?)
        })
    }

    /// Remember (or clear) the send an agent queued for its draft.
    pub fn set_agent_send(
        &self,
        account_id: &str,
        draft_id: &str,
        schedule_id: Option<&str>,
    ) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE agent_drafts SET send_schedule_id = ?3 WHERE account_id = ?1 AND draft_id = ?2",
                params![account_id, draft_id, schedule_id],
            )?;
            Ok(())
        })
    }

    /// Cancel every send an agent queued that hasn't gone yet (the user
    /// lowered the agent level). Sends the user scheduled themselves are
    /// untouched: only schedules whose id an agent recorded are removed.
    /// Returns the cancelled schedules as (account id, draft id).
    pub fn cancel_agent_sends(&self) -> Result<Vec<(String, String)>> {
        self.write(|tx| {
            let pending: Vec<(String, String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT a.account_id, a.draft_id, a.send_schedule_id FROM agent_drafts a
                     JOIN scheduled_sends s ON s.id = a.send_schedule_id",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (_, _, id) in &pending {
                tx.execute("DELETE FROM scheduled_sends WHERE id = ?1", [id])?;
            }
            tx.execute(
                "UPDATE agent_drafts SET send_schedule_id = NULL WHERE send_schedule_id IS NOT NULL",
                [],
            )?;
            Ok(pending.into_iter().map(|(a, d, _)| (a, d)).collect())
        })
    }

    /// Messages I've sent to `email` (lowercased match), from any account.
    /// Indexed (`people` is keyed by email).
    pub fn sent_to_count(&self, email: &str) -> Result<u32> {
        let email = email.trim().to_lowercase();
        self.read(|c| {
            Ok(
                c.prepare_cached("SELECT sent_to_count FROM people WHERE email = ?1")?
                    .query_row([email], |r| r.get::<_, u32>(0))
                    .optional()?
                    .unwrap_or(0),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Account, ScheduledSend, Store};

    const A: &str = "ada@penguin.example";

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&Account {
            id: A.into(),
            email: A.into(),
            ..Account::default()
        })
        .unwrap();
        s.migrate_agent().unwrap();
        s.migrate_agent().unwrap(); // idempotent
        s
    }

    fn schedule(s: &Store, id: &str, draft: &str) {
        s.upsert_scheduled_send(&ScheduledSend {
            id: id.into(),
            account_id: A.into(),
            draft_id: draft.into(),
            send_at: 10,
            created_at: 1,
            remind_after_ms: None,
            attempts: 0,
            last_error: None,
        })
        .unwrap();
    }

    #[test]
    fn records_lists_and_prunes_agent_drafts() {
        let s = store();
        s.set_draft(A, "d1", "m1").unwrap();
        s.put_agent_draft(A, "d1", "mcp", Some("parent"), 5)
            .unwrap();
        s.put_agent_draft(A, "d2", "cli", None, 6).unwrap();
        // An update keeps the creation time, client and reply.
        s.put_agent_draft(A, "d1", "cli", None, 9).unwrap();
        let d1 = s.get_agent_draft(A, "d1").unwrap().unwrap();
        assert_eq!(
            (d1.created_at, d1.updated_at, d1.client.as_str()),
            (5, 9, "mcp")
        );
        assert_eq!(d1.reply_to_message_id.as_deref(), Some("parent"));
        assert_eq!(d1.message_id.as_deref(), Some("m1"));
        assert!(s
            .get_agent_draft("other@x.example", "d1")
            .unwrap()
            .is_none());
        assert_eq!(s.find_agent_drafts("d1").unwrap().len(), 1);
        let all = s.list_agent_drafts(None).unwrap();
        assert_eq!(
            all.iter().map(|d| d.draft_id.as_str()).collect::<Vec<_>>(),
            ["d1", "d2"]
        );
        // d2 has no drafts mapping (gone): pruned.
        assert_eq!(s.prune_agent_drafts().unwrap(), 1);
        assert_eq!(s.list_agent_drafts(Some(A)).unwrap().len(), 1);
        // Removing the account removes its rows.
        s.remove_account(A).unwrap();
        assert!(s.list_agent_drafts(None).unwrap().is_empty());
    }

    #[test]
    fn cancelling_agent_sends_leaves_the_users_own_schedules() {
        let s = store();
        s.put_agent_draft(A, "d1", "mcp", None, 1).unwrap();
        s.put_agent_draft(A, "d2", "mcp", None, 1).unwrap();
        schedule(&s, "s-agent", "d1");
        s.set_agent_send(A, "d1", Some("s-agent")).unwrap();
        // The user scheduled the agent's other draft from the composer.
        schedule(&s, "s-user", "d2");
        // And a draft of their own.
        schedule(&s, "s-mine", "d9");
        let cancelled = s.cancel_agent_sends().unwrap();
        assert_eq!(cancelled, vec![(A.to_string(), "d1".to_string())]);
        let left: Vec<String> = s
            .list_scheduled_sends(None)
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert_eq!(left.len(), 2);
        assert!(left.contains(&"s-user".to_string()) && left.contains(&"s-mine".to_string()));
        assert!(s
            .get_agent_draft(A, "d1")
            .unwrap()
            .unwrap()
            .send_schedule_id
            .is_none());
        assert!(s.cancel_agent_sends().unwrap().is_empty());
    }

    #[test]
    fn sent_to_count_is_zero_for_strangers() {
        let s = store();
        assert_eq!(s.sent_to_count("nobody@x.example").unwrap(), 0);
    }
}
