//! Cross-device state: this device's copy of the synced records
//! (penguin-sync's model; design in docs/CROSS-DEVICE-STATE.md).
//!
//! `sync_records` holds one row per record: the record (JSON), the server
//! change tag it is based on, the time the server stored this exact version
//! (None while it only exists here) and whether it still has to be sent.
//! `sync_state` is one row: this install's device id, its hybrid logical
//! clock, the change-feed token and when a pull last completed.
//!
//! Records are always kept, whether or not a transport is connected: the
//! app's own tables write their record in the same transaction as the
//! change (`snooze_threads`, `dismiss_follow_ups`, …), so turning sync on
//! later is only pushing what is pending. Rows that existed before this
//! table did get their record once, on open ([`seed`]).
//!
//! Remote records come in through [`penguin_sync::LocalStore`] (implemented
//! here for `Store`): each is merged with the local copy and, when it wins,
//! projected onto the app's tables in the same transaction (`project`).
//! Records for accounts this device doesn't have are kept but not projected
//! until the account is added (`project_account`).

use penguin_sync::settings::Change;
use penguin_sync::{
    merge, record_name, split_thread_key, Applied, Clock, Hlc, Kind, Outgoing, Record,
    RemoteRecord, ServerVersion, Winner,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::{outbox, snooze, triage, Store};
use crate::{Error, Result};

pub(super) const SCHEMA_SYNC: &str = r#"
CREATE TABLE sync_records(
    name TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    record TEXT NOT NULL,
    base_tag TEXT,
    server_ms INTEGER,
    dirty INTEGER NOT NULL DEFAULT 0,
    changed INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE INDEX sync_records_pending ON sync_records(changed) WHERE dirty = 1;
CREATE INDEX sync_records_key ON sync_records(kind, key);
CREATE TABLE sync_state(
    id INTEGER PRIMARY KEY CHECK (id = 1),
    device TEXT NOT NULL,
    hlc_wall INTEGER NOT NULL DEFAULT 0,
    hlc_count INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 0,
    token TEXT,
    last_pull_at INTEGER,
    seeded INTEGER NOT NULL DEFAULT 0
);
INSERT INTO sync_state(id, device) VALUES (1, lower(hex(randomblob(8))));
ALTER TABLE snoozes ADD COLUMN remote_pending INTEGER NOT NULL DEFAULT 0;
ALTER TABLE snoozes ADD COLUMN wake_undo TEXT;
"#;

/// What the CloudKit transport keeps (docs/CROSS-DEVICE-STATE.md, "Moving
/// records: CKSyncEngine"): CKSyncEngine's own state, the user's "Sync with
/// iCloud" switch (NULL: never touched, which is on), whether settings were
/// carried over on the first sync, and when records last went out.
pub(super) const SCHEMA_SYNC_CLOUD: &str = r#"
ALTER TABLE sync_state ADD COLUMN engine_state BLOB;
ALTER TABLE sync_state ADD COLUMN wanted INTEGER;
ALTER TABLE sync_state ADD COLUMN settings_seeded INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sync_state ADD COLUMN last_send_at INTEGER;
"#;

/// Called (on the writing thread, after the commit, with the writer free)
/// when a write queued records to send. Keep it short: wake the transport,
/// don't sync from inside it.
pub type PendingListener = std::sync::Arc<dyn Fn() + Send + Sync>;

thread_local! {
    /// Set when this thread queued a record in the write it is running
    /// (`Store::write` clears it before and reads it after).
    static QUEUED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread queued records since the last call (and clear it).
pub(super) fn take_queued() -> bool {
    QUEUED.with(|q| q.replace(false))
}

fn queued() {
    QUEUED.with(|q| q.set(true));
}

fn to_sync(e: Error) -> penguin_sync::Error {
    penguin_sync::Error::Store(e.to_string())
}

/// This device's clock, read from `sync_state`.
fn clock(c: &Connection) -> Result<Clock> {
    Ok(
        c.prepare_cached("SELECT device, hlc_wall, hlc_count FROM sync_state WHERE id = 1")?
            .query_row([], |r| {
                Ok(Clock::new(
                    r.get::<_, String>(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? as u32,
                ))
            })?,
    )
}

fn save_clock(c: &Connection, clock: &Clock) -> Result<()> {
    c.prepare_cached("UPDATE sync_state SET hlc_wall = ?1, hlc_count = ?2 WHERE id = 1")?
        .execute(params![clock.wall, clock.count as i64])?;
    Ok(())
}

/// A stamp for a local event at `now_ms`.
pub(super) fn tick(c: &Connection, now_ms: i64) -> Result<Hlc> {
    let mut k = clock(c)?;
    let h = k.tick(now_ms);
    save_clock(c, &k)?;
    Ok(h)
}

/// A stored record, with the time the server stored this version (None
/// while it only exists here).
pub(super) struct Stored {
    pub record: Record,
    pub server_ms: Option<i64>,
}

pub(super) fn load(c: &Connection, kind: Kind, key: &str) -> Result<Option<Stored>> {
    load_named(c, &record_name(kind, key))
}

fn load_named(c: &Connection, name: &str) -> Result<Option<Stored>> {
    let row: Option<(String, Option<i64>)> = c
        .prepare_cached("SELECT record, server_ms FROM sync_records WHERE name = ?1")?
        .query_row([name], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((json, server_ms)) = row else {
        return Ok(None);
    };
    Ok(Some(Stored {
        record: serde_json::from_str(&json)?,
        server_ms,
    }))
}

/// Store a record written on this device: queued to send, based on
/// whatever server version the old one was based on.
fn put_local(c: &Connection, rec: &Record) -> Result<()> {
    queued();
    c.prepare_cached(
        "INSERT INTO sync_records(name, kind, key, record, base_tag, server_ms, dirty, changed)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, 1, (SELECT coalesce(max(changed), 0) + 1 FROM sync_records))
         ON CONFLICT(name) DO UPDATE SET record = excluded.record, server_ms = NULL, dirty = 1,
           changed = excluded.changed",
    )?
    .execute(params![
        rec.name(),
        rec.kind.tag(),
        rec.key,
        serde_json::to_string(rec)?
    ])?;
    Ok(())
}

/// A new user intent for (kind, key): a new generation.
pub(super) fn emit_intent(
    c: &Connection,
    kind: Kind,
    key: &str,
    body: Option<Value>,
    now_ms: i64,
) -> Result<Record> {
    let stamp = tick(c, now_ms)?;
    let rec = Record::intent(kind, key, stamp, body);
    put_local(c, &rec)?;
    Ok(rec)
}

/// A system event moving the current record of (kind, key) on to `phase`
/// (same generation). `body` gets the current record and returns the new
/// body, or None to leave the record alone.
pub(super) fn emit_phase(
    c: &Connection,
    kind: Kind,
    key: &str,
    phase: u8,
    now_ms: i64,
    body: impl FnOnce(&Record) -> Option<Value>,
) -> Result<Option<Record>> {
    let Some(current) = load(c, kind, key)? else {
        return Ok(None);
    };
    let Some(new_body) = body(&current.record) else {
        return Ok(None);
    };
    let stamp = tick(c, now_ms)?;
    let rec = current.record.advanced(phase, stamp, Some(new_body));
    put_local(c, &rec)?;
    Ok(Some(rec))
}

pub(super) fn account_exists(c: &Connection, account_id: &str) -> Result<bool> {
    Ok(
        c.prepare_cached("SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1)")?
            .query_row([account_id], |r| r.get(0))?,
    )
}

/// Make the app's tables reflect `rec`, which just won a merge.
fn project(c: &Connection, rec: &Record, now_ms: i64) -> Result<()> {
    let Some((account, thread)) = split_thread_key(&rec.key) else {
        return Ok(());
    };
    match rec.kind {
        Kind::Snooze | Kind::FollowUpDismissal | Kind::ScheduledSend | Kind::Reminder
            if !account_exists(c, account)? =>
        {
            Ok(())
        }
        Kind::Snooze => snooze::project_remote(c, account, thread, rec, now_ms),
        Kind::FollowUpDismissal => triage::project_remote(c, account, thread, rec),
        // The key's second half is the schedule's or reminder's id.
        Kind::ScheduledSend | Kind::Reminder => outbox::project_remote(c, account, thread, rec),
        // Settings, snippets and signatures live in the app's settings file,
        // which the app updates from `sync_records(kind)`. Kept here either
        // way.
        _ => Ok(()),
    }
}

/// Project every stored record of a newly added account.
pub(super) fn project_account(c: &Connection, account_id: &str, now_ms: i64) -> Result<()> {
    let prefix = penguin_sync::thread_key(account_id, "");
    let records: Vec<String> = c
        .prepare_cached(
            "SELECT record FROM sync_records
             WHERE kind IN ('snooze', 'followUpDismissal', 'scheduledSend', 'reminder')
             AND substr(key, 1, length(?1)) = ?1",
        )?
        .query_map([&prefix], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for json in records {
        let rec: Record = serde_json::from_str(&json)?;
        project(c, &rec, now_ms)?;
    }
    Ok(())
}

/// Give rows from before sync existed their records, once.
/// (On open, before anything else uses the connection.)
pub(super) fn seed(c: &Connection) -> Result<()> {
    let tx = c.unchecked_transaction()?;
    seed_in(&tx)?;
    outbox::seed(&tx, triage::now_ms())?;
    tx.commit()?;
    take_queued();
    Ok(())
}

/// This install's device id.
pub(super) fn device(c: &Connection) -> Result<String> {
    Ok(c
        .prepare_cached("SELECT device FROM sync_state WHERE id = 1")?
        .query_row([], |r| r.get(0))?)
}

/// Whether records move (a transport is connected).
pub(super) fn enabled(c: &Connection) -> Result<bool> {
    Ok(c
        .prepare_cached("SELECT enabled FROM sync_state WHERE id = 1")?
        .query_row([], |r| r.get(0))?)
}

/// Where the record `name` stands with the server: this exact version
/// stored (nothing to send), and whether any version ever was.
pub(super) fn on_server(c: &Connection, name: &str) -> Result<penguin_sync::outbox::OnServer> {
    let row: Option<(bool, Option<String>, Option<i64>)> = c
        .prepare_cached("SELECT dirty, base_tag, server_ms FROM sync_records WHERE name = ?1")?
        .query_row([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    Ok(match row {
        None => penguin_sync::outbox::OnServer::default(),
        Some((dirty, tag, server_ms)) => penguin_sync::outbox::OnServer {
            stored: !dirty && server_ms.is_some(),
            ever: tag.is_some() || server_ms.is_some(),
        },
    })
}

fn seed_in(c: &Connection) -> Result<()> {
    let seeded: bool = c.query_row("SELECT seeded FROM sync_state WHERE id = 1", [], |r| {
        r.get(0)
    })?;
    if seeded {
        return Ok(());
    }
    let now = triage::now_ms();
    snooze::seed(c, now)?;
    triage::seed(c, now)?;
    c.execute("UPDATE sync_state SET seeded = 1 WHERE id = 1", [])?;
    Ok(())
}

impl Store {
    /// This install's device id (random, made with the database).
    pub fn sync_device(&self) -> Result<String> {
        self.read(|c| {
            Ok(
                c.query_row("SELECT device FROM sync_state WHERE id = 1", [], |r| {
                    r.get(0)
                })?,
            )
        })
    }

    /// Whether a transport is connected (the app sets it when the user
    /// turns sync on and iCloud is available).
    pub fn sync_enabled(&self) -> Result<bool> {
        self.read(|c| {
            Ok(
                c.query_row("SELECT enabled FROM sync_state WHERE id = 1", [], |r| {
                    r.get(0)
                })?,
            )
        })
    }

    pub fn set_sync_enabled(&self, enabled: bool) -> Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE sync_state SET enabled = ?1 WHERE id = 1", [enabled])?;
            Ok(())
        })
    }

    /// Whether this device has seen every change made before `at_ms`: sync
    /// is off (nothing to see), or a pull completed at or after it.
    pub fn sync_pull_fresh(&self, at_ms: i64) -> Result<bool> {
        self.read(|c| pull_fresh(c, at_ms))
    }

    /// Every stored record of `kind` (tombstones included), e.g. for the app
    /// to update its settings file from `Setting` / `Snippet` records.
    pub fn sync_records(&self, kind: Kind) -> Result<Vec<Record>> {
        self.read(|c| {
            let rows: Vec<String> = c
                .prepare_cached("SELECT record FROM sync_records WHERE kind = ?1 ORDER BY key")?
                .query_map([kind.tag()], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            rows.iter()
                .map(|j| serde_json::from_str(j).map_err(Error::from))
                .collect()
        })
    }

    /// Record a user change to a setting, snippet or signature (the app
    /// calls this with `penguin_sync::settings::changes` of each write).
    pub fn sync_record_change(
        &self,
        kind: Kind,
        key: &str,
        body: Option<Value>,
        now_ms: i64,
    ) -> Result<()> {
        self.write(|tx| emit_intent(tx, kind, key, body, now_ms).map(|_| ()))
    }

    /// Record a write of the app's settings file: each of
    /// `penguin_sync::settings::changes(old, new)`, in one transaction.
    /// `partial`: this app keeps only part of some settings (the iPhone),
    /// so each new value (a setting, or one snippet or signature) is laid
    /// over the last synced one (`settings::overlay`) and the fields it
    /// doesn't have survive (a snippet's files, which the iPhone doesn't
    /// keep).
    pub fn sync_record_settings(
        &self,
        changes: &[Change],
        partial: bool,
        now_ms: i64,
    ) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            for (kind, key, body) in changes {
                let body = match (body, partial) {
                    (Some(v), true) => {
                        let base = load(tx, *kind, key)?.and_then(|s| s.record.body);
                        Some(penguin_sync::settings::overlay(base.as_ref(), v))
                    }
                    (b, _) => b.clone(),
                };
                emit_intent(tx, *kind, key, body, now_ms)?;
            }
            Ok(())
        })
    }

    /// Whether this device's settings were carried over (once, after the
    /// first complete pull; see [`Store::sync_seed_settings`]).
    pub fn sync_settings_seeded(&self) -> Result<bool> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT settings_seeded FROM sync_state WHERE id = 1",
                [],
                |r| r.get(0),
            )?)
        })
    }

    /// Carry this device's settings over to sync, once: each change (from
    /// `settings::seed_changes`) that has no record yet becomes a seeded
    /// record (`Record::seeded`), which any real edit outranks. Run after
    /// the first complete pull, so settings another device already synced
    /// are kept rather than replaced. Returns how many were recorded.
    pub fn sync_seed_settings(&self, changes: &[Change], now_ms: i64) -> Result<usize> {
        self.write(|tx| {
            let seeded: bool = tx.query_row(
                "SELECT settings_seeded FROM sync_state WHERE id = 1",
                [],
                |r| r.get(0),
            )?;
            if seeded {
                return Ok(0);
            }
            let mut n = 0;
            for (kind, key, body) in changes {
                if load(tx, *kind, key)?.is_some() {
                    continue;
                }
                let stamp = tick(tx, now_ms)?;
                put_local(tx, &Record::seeded(*kind, key.clone(), stamp, body.clone()))?;
                n += 1;
            }
            tx.execute("UPDATE sync_state SET settings_seeded = 1 WHERE id = 1", [])?;
            Ok(n)
        })
    }

    /// Every record of a kind kept in the settings file (settings,
    /// snippets, signatures), tombstones included.
    pub fn sync_settings_records(&self) -> Result<Vec<Record>> {
        let mut out = Vec::new();
        for kind in [Kind::Setting, Kind::Snippet, Kind::Signature] {
            out.extend(self.sync_records(kind)?);
        }
        Ok(out)
    }

    /// Tell `listener` whenever a write queues records to send, so the
    /// transport wakes up and sends them. None stops it.
    pub fn set_sync_listener(&self, listener: Option<PendingListener>) {
        *self
            .inner
            .sync_listener
            .write()
            .unwrap_or_else(|p| p.into_inner()) = listener;
    }

    pub(super) fn notify_sync_pending(&self) {
        let listener = self
            .inner
            .sync_listener
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(l) = listener {
            l();
        }
    }

    /// "Sync with iCloud": on unless the user turned it off.
    pub fn sync_wanted(&self) -> Result<bool> {
        self.read(|c| {
            let v: Option<bool> =
                c.query_row("SELECT wanted FROM sync_state WHERE id = 1", [], |r| {
                    r.get(0)
                })?;
            Ok(v.unwrap_or(true))
        })
    }

    pub fn set_sync_wanted(&self, on: bool) -> Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE sync_state SET wanted = ?1 WHERE id = 1", [on])?;
            Ok(())
        })
    }

    /// The transport's own saved state (CKSyncEngine's serialization).
    pub fn sync_engine_state(&self) -> Result<Option<Vec<u8>>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT engine_state FROM sync_state WHERE id = 1",
                [],
                |r| r.get(0),
            )?)
        })
    }

    pub fn set_sync_engine_state(&self, state: Option<&[u8]>) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE sync_state SET engine_state = ?1 WHERE id = 1",
                [state],
            )?;
            Ok(())
        })
    }

    /// Names of the records waiting to be sent, oldest change first.
    pub fn sync_pending_names(&self, limit: usize) -> Result<Vec<String>> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT name FROM sync_records WHERE dirty = 1 ORDER BY changed LIMIT ?1",
            )?
            .query_map([limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn sync_pending_count(&self) -> Result<u32> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM sync_records WHERE dirty = 1",
                [],
                |r| r.get(0),
            )?)
        })
    }

    /// The records among `names` that still have to be sent, with the
    /// server version each is based on (for a transport that asks record
    /// by record, like CKSyncEngine's batches).
    pub fn sync_outgoing(&self, names: &[String]) -> Result<Vec<Outgoing>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT record, base_tag FROM sync_records WHERE name = ?1 AND dirty = 1",
            )?;
            let mut out = Vec::new();
            for name in names {
                let row: Option<(String, Option<String>)> = stmt
                    .query_row([name], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                if let Some((json, base_tag)) = row {
                    out.push(Outgoing {
                        record: serde_json::from_str(&json)?,
                        base_tag,
                    });
                }
            }
            Ok(out)
        })
    }

    /// The server no longer has these records (CloudKit `unknownItem`, or
    /// their zone is gone): send them again as new.
    pub fn sync_clear_base(&self, names: &[String]) -> Result<()> {
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "UPDATE sync_records SET base_tag = NULL, server_ms = NULL, dirty = 1,
                   changed = (SELECT coalesce(max(changed), 0) + 1 FROM sync_records)
                 WHERE name = ?1",
            )?;
            for name in names {
                if stmt.execute([name])? > 0 {
                    queued();
                }
            }
            Ok(())
        })
    }

    /// Start over with an empty server (signed out of iCloud, another
    /// account, the zone deleted or reset): every record is sent again
    /// from this device's copy, and the next pull starts from the
    /// beginning. Local state is kept.
    pub fn sync_reset_server(&self) -> Result<()> {
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE sync_records SET base_tag = NULL, server_ms = NULL, dirty = 1",
                [],
            )?;
            tx.execute(
                "UPDATE sync_state SET token = NULL, engine_state = NULL, last_pull_at = NULL
                 WHERE id = 1",
                [],
            )?;
            if n > 0 {
                queued();
            }
            Ok(())
        })
    }

    /// When a pull last completed and records last went out (unix ms).
    pub fn sync_times(&self) -> Result<(Option<i64>, Option<i64>)> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT last_pull_at, last_send_at FROM sync_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
    }

    pub fn set_sync_last_send(&self, at_ms: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE sync_state SET last_send_at = ?1 WHERE id = 1",
                [at_ms],
            )?;
            Ok(())
        })
    }
}

pub(super) fn pull_fresh(c: &Connection, at_ms: i64) -> Result<bool> {
    let (enabled, last): (bool, Option<i64>) = c.query_row(
        "SELECT enabled, last_pull_at FROM sync_state WHERE id = 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(!enabled || last.is_some_and(|l| l >= at_ms))
}

impl penguin_sync::LocalStore for Store {
    fn sync_token(&self) -> penguin_sync::Result<Option<String>> {
        self.read(|c| {
            Ok(
                c.query_row("SELECT token FROM sync_state WHERE id = 1", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .map_err(to_sync)
    }

    fn set_sync_token(&self, token: &str) -> penguin_sync::Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE sync_state SET token = ?1 WHERE id = 1", [token])?;
            Ok(())
        })
        .map_err(to_sync)
    }

    fn apply_remote(&self, remote: &RemoteRecord, now_ms: i64) -> penguin_sync::Result<Applied> {
        self.write(|tx| apply_remote(tx, remote, now_ms))
            .map_err(to_sync)
    }

    fn forget(&self, name: &str) -> penguin_sync::Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM sync_records WHERE name = ?1 AND dirty = 0",
                [name],
            )?;
            Ok(())
        })
        .map_err(to_sync)
    }

    fn pending(&self, limit: usize) -> penguin_sync::Result<Vec<Outgoing>> {
        self.read(|c| {
            let rows: Vec<(String, Option<String>)> = c
                .prepare_cached(
                    "SELECT record, base_tag FROM sync_records WHERE dirty = 1 ORDER BY changed LIMIT ?1",
                )?
                .query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            rows.into_iter()
                .map(|(json, base_tag)| {
                    Ok(Outgoing {
                        record: serde_json::from_str(&json)?,
                        base_tag,
                    })
                })
                .collect()
        })
        .map_err(to_sync)
    }

    fn saved(&self, sent: &Outgoing, version: &ServerVersion) -> penguin_sync::Result<()> {
        self.write(|tx| {
            let name = sent.record.name();
            let Some(current) = load_named(tx, &name)? else {
                return Ok(());
            };
            if current.record == sent.record {
                tx.execute(
                    "UPDATE sync_records SET base_tag = ?2, server_ms = ?3, dirty = 0 WHERE name = ?1",
                    params![name, version.tag, version.modified_ms],
                )?;
            } else {
                // Changed again while it was on the way: send that too.
                tx.execute(
                    "UPDATE sync_records SET base_tag = ?2 WHERE name = ?1",
                    params![name, version.tag],
                )?;
            }
            Ok(())
        })
        .map_err(to_sync)
    }

    fn pull_completed(&self, at_ms: i64) -> penguin_sync::Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE sync_state SET last_pull_at = ?1 WHERE id = 1",
                [at_ms],
            )?;
            Ok(())
        })
        .map_err(to_sync)
    }
}

fn apply_remote(c: &Connection, remote: &RemoteRecord, now_ms: i64) -> Result<Applied> {
    let stored_at = Some(remote.version.modified_ms);
    let mut k = clock(c)?;
    k.observe(&remote.record.gen.clamped(stored_at), now_ms);
    k.observe(&remote.record.at.clamped(stored_at), now_ms);
    save_clock(c, &k)?;

    let name = remote.record.name();
    let local = load_named(c, &name)?;
    let winner = match &local {
        None => Winner::Remote,
        Some(l) => merge((&l.record, l.server_ms), (&remote.record, stored_at)),
    };
    let json = serde_json::to_string(&remote.record)?;
    match winner {
        Winner::Remote => {
            c.prepare_cached(
                "INSERT INTO sync_records(name, kind, key, record, base_tag, server_ms, dirty, changed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0)
                 ON CONFLICT(name) DO UPDATE SET record = excluded.record, base_tag = excluded.base_tag,
                   server_ms = excluded.server_ms, dirty = 0",
            )?
            .execute(params![
                name,
                remote.record.kind.tag(),
                remote.record.key,
                json,
                remote.version.tag,
                remote.version.modified_ms
            ])?;
            project(c, &remote.record, now_ms)?;
            Ok(Applied::Took)
        }
        Winner::Local => {
            queued();
            // Ours is newer: send it again, on top of the server's version.
            c.execute(
                "UPDATE sync_records SET base_tag = ?2, dirty = 1 WHERE name = ?1",
                params![name, remote.version.tag],
            )?;
            Ok(Applied::Kept)
        }
        Winner::Same => {
            c.execute(
                "UPDATE sync_records SET base_tag = ?2, server_ms = ?3, dirty = 0 WHERE name = ?1",
                params![name, remote.version.tag, remote.version.modified_ms],
            )?;
            Ok(Applied::Same)
        }
    }
}

#[cfg(test)]
#[path = "store_sync_tests.rs"]
mod tests;
