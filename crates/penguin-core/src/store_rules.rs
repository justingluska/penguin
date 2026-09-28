//! Rules & automations: persistence only. OWNER: rules agent.
//! Rule definitions live in `<config dir>/rules.json` (the app's
//! `rules` module); the database keeps what must survive restarts and be
//! transactional with the mail:
//!
//! - `rule_queue`: messages the sync engine reported (new via history, or a
//!   label added) that rules haven't looked at yet. Written before the sync
//!   cursor moves, so a crash replays rather than loses them.
//! - `rule_applications`: (rule, message) pairs a rule acted on. A rule
//!   never fires twice for the same message.
//! - `rule_log`: per-rule history for the UI (ids, subject and sender for
//!   display; never bodies).
//! - `rule_schedule`: when each scheduled rule last ran.

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::Store;
use crate::Result;

/// Log rows kept per rule (oldest pruned first).
const MAX_LOG_PER_RULE: i64 = 1000;
/// Applications older than this are forgotten (history never re-delivers a
/// message as new, so only a label trigger could fire again after this).
const APPLICATION_RETENTION_MS: i64 = 365 * 24 * 3600 * 1000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RuleEventKind {
    /// Stored from history.list (incremental sync), never from backfill.
    NewMessage,
    /// Gained labels (history labelsAdded).
    LabelAdded,
}

impl RuleEventKind {
    fn as_str(self) -> &'static str {
        match self {
            RuleEventKind::NewMessage => "new",
            RuleEventKind::LabelAdded => "label",
        }
    }
    fn parse(s: &str) -> RuleEventKind {
        match s {
            "label" => RuleEventKind::LabelAdded,
            _ => RuleEventKind::NewMessage,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleEvent {
    pub message_id: String,
    pub kind: RuleEventKind,
    /// LabelAdded: the label ids that were added.
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueuedRuleEvent {
    pub id: i64,
    pub account_id: String,
    pub message_id: String,
    pub kind: RuleEventKind,
    pub labels: Vec<String>,
    pub queued_at: i64,
}

/// A (rule, message) claim. Inserted as `pending` before any action runs.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleApplication {
    pub rule_id: String,
    pub account_id: String,
    pub message_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationStatus {
    Pending,
    Done,
    Failed,
    /// The app stopped between claiming and finishing. Not retried: actions
    /// such as forward or a webhook must not run twice.
    Interrupted,
}

impl ApplicationStatus {
    fn as_str(self) -> &'static str {
        match self {
            ApplicationStatus::Pending => "pending",
            ApplicationStatus::Done => "done",
            ApplicationStatus::Failed => "failed",
            ApplicationStatus::Interrupted => "interrupted",
        }
    }
}

/// One row of a rule's history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuleLogEntry {
    pub id: i64,
    pub ts: i64,
    pub rule_id: String,
    /// "newMessage" | "labelAdded" | "schedule" | "manual"
    pub trigger: String,
    pub dry_run: bool,
    pub ok: bool,
    pub account_id: Option<String>,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    /// For display in the history list (the local DB already holds it).
    pub subject: Option<String>,
    pub from_email: Option<String>,
    /// Matches covered by this row (1 per message; a scheduled run's
    /// aggregate row counts all of them).
    pub matched: u32,
    /// What each action did: `[{action, ok, detail}]` (app-defined JSON).
    pub outcomes: serde_json::Value,
    pub undone: bool,
}

/// Aggregates for the rules list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleStats {
    pub rule_id: String,
    /// Messages acted on (live runs).
    pub applied: u64,
    /// Log rows from dry runs.
    pub dry_run_matches: u64,
    pub last_run_at: Option<i64>,
    pub last_error_at: Option<i64>,
    pub last_error: Option<String>,
}

fn join_labels(labels: &[String]) -> String {
    labels.join("\u{1f}")
}

fn split_labels(s: &str) -> Vec<String> {
    s.split('\u{1f}')
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

const LOG_COLS: &str = "id, ts, rule_id, trigger, dry_run, ok, account_id, thread_id, message_id, subject, from_email, matched, outcomes, undone";

fn log_row(r: &Row) -> rusqlite::Result<RuleLogEntry> {
    let outcomes: String = r.get(12)?;
    Ok(RuleLogEntry {
        id: r.get(0)?,
        ts: r.get(1)?,
        rule_id: r.get(2)?,
        trigger: r.get(3)?,
        dry_run: r.get::<_, i64>(4)? != 0,
        ok: r.get::<_, i64>(5)? != 0,
        account_id: r.get(6)?,
        thread_id: r.get(7)?,
        message_id: r.get(8)?,
        subject: r.get(9)?,
        from_email: r.get(10)?,
        matched: r.get::<_, i64>(11)? as u32,
        outcomes: serde_json::from_str(&outcomes).unwrap_or(serde_json::Value::Null),
        undone: r.get::<_, i64>(13)? != 0,
    })
}

impl Store {
    // ----- queue -----

    /// Queue sync events for the rules engine. Cheap (one small
    /// transaction), so the sync engine can call it inline.
    pub fn enqueue_rule_events(
        &self,
        account_id: &str,
        events: &[RuleEvent],
        now: i64,
    ) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO rule_queue(account_id, message_id, kind, labels, queued_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for e in events {
                stmt.execute(params![
                    account_id,
                    e.message_id,
                    e.kind.as_str(),
                    join_labels(&e.labels),
                    now
                ])?;
            }
            Ok(())
        })
    }

    /// Oldest first.
    pub fn pending_rule_events(&self, limit: usize) -> Result<Vec<QueuedRuleEvent>> {
        self.read(|c| {
            let rows = c
                .prepare_cached(
                    "SELECT id, account_id, message_id, kind, labels, queued_at FROM rule_queue ORDER BY id LIMIT ?1",
                )?
                .query_map([limit as i64], |r| {
                    Ok(QueuedRuleEvent {
                        id: r.get(0)?,
                        account_id: r.get(1)?,
                        message_id: r.get(2)?,
                        kind: RuleEventKind::parse(&r.get::<_, String>(3)?),
                        labels: split_labels(&r.get::<_, String>(4)?),
                        queued_at: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// In one transaction: drop the handled queue rows and claim each
    /// (rule, message) as `pending`. Returns, per claim, whether it was new
    /// (false = that rule already fired for that message).
    pub fn claim_rule_applications(
        &self,
        handled_queue_ids: &[i64],
        claims: &[RuleApplication],
        now: i64,
    ) -> Result<Vec<bool>> {
        self.write(|tx| {
            {
                let mut del = tx.prepare_cached("DELETE FROM rule_queue WHERE id = ?1")?;
                for id in handled_queue_ids {
                    del.execute([id])?;
                }
            }
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO rule_applications(rule_id, account_id, message_id, thread_id, applied_at, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
            )?;
            let mut out = Vec::with_capacity(claims.len());
            for a in claims {
                let n = ins.execute(params![
                    a.rule_id,
                    a.account_id,
                    a.message_id,
                    a.thread_id,
                    now
                ])?;
                out.push(n > 0);
            }
            Ok(out)
        })
    }

    pub fn set_rule_application_status(
        &self,
        rule_id: &str,
        account_id: &str,
        message_ids: &[String],
        status: ApplicationStatus,
    ) -> Result<()> {
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "UPDATE rule_applications SET status = ?4 WHERE rule_id = ?1 AND account_id = ?2 AND message_id = ?3",
            )?;
            for m in message_ids {
                stmt.execute(params![rule_id, account_id, m, status.as_str()])?;
            }
            Ok(())
        })
    }

    /// At startup: claims left `pending` by a previous run become
    /// `interrupted` (never retried). Returns how many.
    pub fn interrupt_pending_rule_applications(&self) -> Result<usize> {
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE rule_applications SET status = 'interrupted' WHERE status = 'pending'",
                [],
            )?)
        })
    }

    /// Which of `pairs` (account id, message id) `rule_id` already fired for.
    pub fn applied_rule_messages(
        &self,
        rule_id: &str,
        pairs: &[(String, String)],
    ) -> Result<std::collections::HashSet<(String, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT 1 FROM rule_applications WHERE rule_id = ?1 AND account_id = ?2 AND message_id = ?3",
            )?;
            let mut out = std::collections::HashSet::new();
            for (a, m) in pairs {
                if stmt
                    .query_row(params![rule_id, a, m], |_| Ok(()))
                    .optional()?
                    .is_some()
                {
                    out.insert((a.clone(), m.clone()));
                }
            }
            Ok(out)
        })
    }

    // ----- log -----

    /// Append history rows (their `id` is ignored); returns the new ids.
    pub fn add_rule_log(&self, entries: &[RuleLogEntry]) -> Result<Vec<i64>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO rule_log(ts, rule_id, trigger, dry_run, ok, account_id, thread_id, message_id, subject, from_email, matched, outcomes, undone)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            let mut ids = Vec::with_capacity(entries.len());
            for e in entries {
                stmt.execute(params![
                    e.ts,
                    e.rule_id,
                    e.trigger,
                    e.dry_run as i64,
                    e.ok as i64,
                    e.account_id,
                    e.thread_id,
                    e.message_id,
                    e.subject,
                    e.from_email,
                    e.matched as i64,
                    e.outcomes.to_string(),
                    e.undone as i64
                ])?;
                ids.push(tx.last_insert_rowid());
            }
            Ok(ids)
        })
    }

    /// Newest first; `rule_id` None = every rule. `before_id` pages back.
    pub fn rule_log(
        &self,
        rule_id: Option<&str>,
        limit: usize,
        before_id: Option<i64>,
    ) -> Result<Vec<RuleLogEntry>> {
        self.read(|c| {
            let rows = c
                .prepare_cached(&format!(
                    "SELECT {LOG_COLS} FROM rule_log
                     WHERE (?1 IS NULL OR rule_id = ?1) AND (?2 IS NULL OR id < ?2)
                     ORDER BY id DESC LIMIT ?3"
                ))?
                .query_map(params![rule_id, before_id, limit as i64], log_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    pub fn rule_log_entry(&self, id: i64) -> Result<Option<RuleLogEntry>> {
        self.read(|c| {
            Ok(
                c.prepare_cached(&format!("SELECT {LOG_COLS} FROM rule_log WHERE id = ?1"))?
                    .query_row([id], log_row)
                    .optional()?,
            )
        })
    }

    pub fn mark_rule_log_undone(&self, id: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE rule_log SET undone = 1 WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Per-rule aggregates from the log.
    pub fn rule_stats(&self) -> Result<Vec<RuleStats>> {
        self.read(|c| {
            let mut out: Vec<RuleStats> = c
                .prepare_cached(
                    "SELECT rule_id,
                            sum(CASE WHEN dry_run = 0 AND ok = 1 THEN matched ELSE 0 END),
                            sum(CASE WHEN dry_run = 1 THEN matched ELSE 0 END),
                            max(ts),
                            max(CASE WHEN ok = 0 THEN ts END)
                     FROM rule_log GROUP BY rule_id",
                )?
                .query_map([], |r| {
                    Ok(RuleStats {
                        rule_id: r.get(0)?,
                        applied: r.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                        dry_run_matches: r.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64,
                        last_run_at: r.get(3)?,
                        last_error_at: r.get(4)?,
                        last_error: None,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            let mut err = c.prepare_cached(
                "SELECT outcomes FROM rule_log WHERE rule_id = ?1 AND ok = 0 ORDER BY id DESC LIMIT 1",
            )?;
            for s in out.iter_mut().filter(|s| s.last_error_at.is_some()) {
                let outcomes: Option<String> = err
                    .query_row([&s.rule_id], |r| r.get(0))
                    .optional()?;
                s.last_error = outcomes
                    .and_then(|o| serde_json::from_str::<serde_json::Value>(&o).ok())
                    .and_then(|v| {
                        v.as_array()?
                            .iter()
                            .find(|o| o["ok"] == serde_json::Value::Bool(false))
                            .and_then(|o| o["detail"].as_str().map(str::to_string))
                    });
            }
            Ok(out)
        })
    }

    // ----- schedule -----

    pub fn rule_last_scheduled(&self, rule_id: &str) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(
                c.prepare_cached("SELECT last_run FROM rule_schedule WHERE rule_id = ?1")?
                    .query_row([rule_id], |r| r.get(0))
                    .optional()?,
            )
        })
    }

    pub fn set_rule_last_scheduled(&self, rule_id: &str, at: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO rule_schedule(rule_id, last_run) VALUES (?1, ?2)
                 ON CONFLICT(rule_id) DO UPDATE SET last_run = excluded.last_run",
                params![rule_id, at],
            )?;
            Ok(())
        })
    }

    // ----- housekeeping -----

    /// A rule was deleted: forget its history, claims and schedule.
    pub fn delete_rule_data(&self, rule_id: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM rule_applications WHERE rule_id = ?1",
                [rule_id],
            )?;
            tx.execute("DELETE FROM rule_log WHERE rule_id = ?1", [rule_id])?;
            tx.execute("DELETE FROM rule_schedule WHERE rule_id = ?1", [rule_id])?;
            Ok(())
        })
    }

    /// Bound the log per rule and drop year-old claims.
    pub fn prune_rule_data(&self, now: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM rule_applications WHERE applied_at < ?1",
                [now - APPLICATION_RETENTION_MS],
            )?;
            tx.execute(
                "DELETE FROM rule_log WHERE id IN (
                   SELECT id FROM (
                     SELECT id, row_number() OVER (PARTITION BY rule_id ORDER BY id DESC) AS n FROM rule_log
                   ) WHERE n > ?1)",
                [MAX_LOG_PER_RULE],
            )?;
            Ok(())
        })
    }
}

/// Part of `Store::remove_account`'s transaction.
pub(super) fn remove_account_rule_data(tx: &rusqlite::Transaction, account_id: &str) -> Result<()> {
    tx.execute("DELETE FROM rule_queue WHERE account_id = ?1", [account_id])?;
    tx.execute(
        "DELETE FROM rule_applications WHERE account_id = ?1",
        [account_id],
    )?;
    tx.execute("DELETE FROM rule_log WHERE account_id = ?1", [account_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(rule: &str, msg: &str) -> RuleApplication {
        RuleApplication {
            rule_id: rule.into(),
            account_id: "a@x.example".into(),
            message_id: msg.into(),
            thread_id: "t1".into(),
        }
    }

    #[test]
    fn queue_round_trip_and_claims_are_idempotent() {
        let s = Store::open_in_memory().unwrap();
        s.enqueue_rule_events(
            "a@x.example",
            &[
                RuleEvent {
                    message_id: "m1".into(),
                    kind: RuleEventKind::NewMessage,
                    labels: vec![],
                },
                RuleEvent {
                    message_id: "m2".into(),
                    kind: RuleEventKind::LabelAdded,
                    labels: vec!["Label_1".into(), "STARRED".into()],
                },
            ],
            5,
        )
        .unwrap();
        let q = s.pending_rule_events(10).unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q[1].labels, vec!["Label_1", "STARRED"]);
        assert_eq!(q[1].kind, RuleEventKind::LabelAdded);

        let fresh = s
            .claim_rule_applications(&[q[0].id], &[claim("r1", "m1"), claim("r2", "m1")], 6)
            .unwrap();
        assert_eq!(fresh, vec![true, true]);
        assert_eq!(s.pending_rule_events(10).unwrap().len(), 1);
        // The same rule never claims the same message twice.
        let again = s
            .claim_rule_applications(&[q[1].id], &[claim("r1", "m1"), claim("r1", "m2")], 7)
            .unwrap();
        assert_eq!(again, vec![false, true]);
        assert!(s.pending_rule_events(10).unwrap().is_empty());

        // Restart: pending claims become interrupted and stay claimed.
        s.set_rule_application_status("r1", "a@x.example", &["m1".into()], ApplicationStatus::Done)
            .unwrap();
        assert_eq!(s.interrupt_pending_rule_applications().unwrap(), 2);
        let applied = s
            .applied_rule_messages(
                "r1",
                &[
                    ("a@x.example".into(), "m1".into()),
                    ("a@x.example".into(), "m3".into()),
                ],
            )
            .unwrap();
        assert_eq!(applied.len(), 1);

        s.remove_account("a@x.example").unwrap();
        assert!(s
            .applied_rule_messages("r1", &[("a@x.example".into(), "m1".into())])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn log_stats_and_prune() {
        let s = Store::open_in_memory().unwrap();
        let entry = |ok: bool, dry: bool| RuleLogEntry {
            id: 0,
            ts: 10,
            rule_id: "r1".into(),
            trigger: "newMessage".into(),
            dry_run: dry,
            ok,
            account_id: Some("a@x.example".into()),
            thread_id: Some("t".into()),
            message_id: Some("m".into()),
            subject: Some("Receipt".into()),
            from_email: Some("billing@shop.example".into()),
            matched: 1,
            outcomes: serde_json::json!([{"action": "archive", "ok": ok, "detail": if ok { "" } else { "label missing" }}]),
            undone: false,
        };
        let ids = s
            .add_rule_log(&[entry(true, false), entry(true, true), entry(false, false)])
            .unwrap();
        assert_eq!(ids.len(), 3);
        let stats = s.rule_stats().unwrap();
        assert_eq!(stats[0].applied, 1);
        assert_eq!(stats[0].dry_run_matches, 1);
        assert_eq!(stats[0].last_error.as_deref(), Some("label missing"));
        let log = s.rule_log(Some("r1"), 2, None).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].id, ids[2]);
        s.mark_rule_log_undone(ids[0]).unwrap();
        assert!(s.rule_log_entry(ids[0]).unwrap().unwrap().undone);
        s.set_rule_last_scheduled("r1", 99).unwrap();
        assert_eq!(s.rule_last_scheduled("r1").unwrap(), Some(99));
        s.prune_rule_data(100).unwrap();
        s.delete_rule_data("r1").unwrap();
        assert!(s.rule_log(None, 10, None).unwrap().is_empty());
        assert_eq!(s.rule_last_scheduled("r1").unwrap(), None);
    }
}
