//! In-memory doubles: a server that behaves like a CloudKit private
//! database zone (change tags checked on every save, a change feed with a
//! token, server timestamps), a transport onto it that can be switched
//! offline, and a local store that keeps records without projecting them
//! anywhere. Used by this crate's tests and by penguin-core's sync tests.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::engine::{
    Applied, LocalStore, Outgoing, Pulled, PushOutcome, RemoteRecord, ServerVersion, Transport,
};
use crate::hlc::{Clock, Hlc};
use crate::record::{merge, record_name, Kind, Record, Winner};
use crate::{Error, Result};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct ServerState {
    /// name → (record, the change sequence it was stored at).
    records: BTreeMap<String, (RemoteRecord, u64)>,
    /// (sequence, name) of records removed.
    deleted: Vec<(u64, String)>,
    seq: u64,
    tags: u64,
    now_ms: i64,
    page_size: usize,
}

/// The shared server. Clone it to hand the same server to several devices.
#[derive(Clone)]
pub struct MemoryServer {
    state: Arc<Mutex<ServerState>>,
}

impl Default for MemoryServer {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryServer {
    pub fn new() -> MemoryServer {
        MemoryServer {
            state: Arc::new(Mutex::new(ServerState {
                page_size: usize::MAX,
                ..ServerState::default()
            })),
        }
    }

    /// The server's clock (what it stamps as `modified_ms`).
    pub fn set_now(&self, now_ms: i64) {
        lock(&self.state).now_ms = now_ms;
    }

    /// Records per pull page (default: all).
    pub fn set_page_size(&self, n: usize) {
        lock(&self.state).page_size = n.max(1);
    }

    pub fn transport(&self) -> MemoryTransport {
        MemoryTransport {
            server: self.clone(),
            offline: false,
        }
    }

    /// The stored record named `name`.
    pub fn get(&self, name: &str) -> Option<RemoteRecord> {
        lock(&self.state).records.get(name).map(|(r, _)| r.clone())
    }

    pub fn len(&self) -> usize {
        lock(&self.state).records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove tombstones stored before `before_ms` (the GC a device runs
    /// on the server; see the design doc's tombstone section).
    pub fn collect_tombstones(&self, before_ms: i64) -> usize {
        let mut s = lock(&self.state);
        let doomed: Vec<String> = s
            .records
            .iter()
            .filter(|(_, (r, _))| r.record.is_deleted() && r.version.modified_ms < before_ms)
            .map(|(n, _)| n.clone())
            .collect();
        for name in &doomed {
            s.records.remove(name);
            s.seq += 1;
            let seq = s.seq;
            s.deleted.push((seq, name.clone()));
        }
        doomed.len()
    }
}

/// One device's connection to a [`MemoryServer`].
pub struct MemoryTransport {
    server: MemoryServer,
    /// While true every call fails as `Unavailable`.
    pub offline: bool,
}

impl Transport for MemoryTransport {
    fn pull(&mut self, token: Option<&str>) -> Result<Pulled> {
        if self.offline {
            return Err(Error::Unavailable("offline".into()));
        }
        let s = lock(&self.server.state);
        let since: u64 = match token {
            None => 0,
            Some(t) => t
                .parse()
                .map_err(|_| Error::Unavailable(format!("bad token {t}")))?,
        };
        let mut changes: Vec<(u64, Option<RemoteRecord>, String)> = s
            .records
            .iter()
            .filter(|(_, (_, seq))| *seq > since)
            .map(|(n, (r, seq))| (*seq, Some(r.clone()), n.clone()))
            .chain(
                s.deleted
                    .iter()
                    .filter(|(seq, _)| *seq > since)
                    .map(|(seq, n)| (*seq, None, n.clone())),
            )
            .collect();
        changes.sort_by_key(|c| c.0);
        let more = changes.len() > s.page_size;
        changes.truncate(s.page_size);
        let token = changes.last().map_or(since, |c| c.0);
        let mut out = Pulled {
            token: token.to_string(),
            more,
            ..Pulled::default()
        };
        for (_, r, name) in changes {
            match r {
                Some(r) => out.records.push(r),
                None => out.deleted.push(name),
            }
        }
        Ok(out)
    }

    fn push(&mut self, batch: &[Outgoing]) -> Result<Vec<PushOutcome>> {
        if self.offline {
            return Err(Error::Unavailable("offline".into()));
        }
        let mut s = lock(&self.server.state);
        let mut out = Vec::with_capacity(batch.len());
        for o in batch {
            let name = o.record.name();
            if let Some((current, _)) = s.records.get(&name) {
                if o.base_tag.as_deref() != Some(current.version.tag.as_str()) {
                    out.push(PushOutcome::Conflict(current.clone()));
                    continue;
                }
            }
            s.tags += 1;
            s.seq += 1;
            let version = ServerVersion {
                tag: format!("t{}", s.tags),
                modified_ms: s.now_ms,
            };
            let seq = s.seq;
            s.records.insert(
                name,
                (
                    RemoteRecord {
                        record: o.record.clone(),
                        version: version.clone(),
                    },
                    seq,
                ),
            );
            out.push(PushOutcome::Saved(version));
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct Entry {
    record: Record,
    base_tag: Option<String>,
    server_ms: Option<i64>,
    dirty: bool,
    order: u64,
}

struct LocalState {
    clock: Clock,
    records: HashMap<String, Entry>,
    token: Option<String>,
    last_pull: Option<i64>,
    order: u64,
}

/// A device's records, merged with [`merge`] and kept in memory.
pub struct MemoryStore {
    state: Mutex<LocalState>,
}

impl MemoryStore {
    pub fn new(device: &str) -> MemoryStore {
        MemoryStore {
            state: Mutex::new(LocalState {
                clock: Clock::new(device, 0, 0),
                records: HashMap::new(),
                token: None,
                last_pull: None,
                order: 0,
            }),
        }
    }

    /// A user edit: a new generation for (kind, key) at wall time `now_ms`.
    pub fn edit(
        &self,
        kind: Kind,
        key: &str,
        body: Option<serde_json::Value>,
        now_ms: i64,
    ) -> Record {
        let mut s = lock(&self.state);
        let stamp = s.clock.tick(now_ms);
        let rec = Record::intent(kind, key, stamp, body);
        put_local(&mut s, rec.clone());
        rec
    }

    /// A system event moving the current record of (kind, key) to `phase`.
    /// None when there is no record.
    pub fn advance(
        &self,
        kind: Kind,
        key: &str,
        phase: u8,
        body: Option<serde_json::Value>,
        now_ms: i64,
    ) -> Option<Record> {
        let mut s = lock(&self.state);
        let current = s.records.get(&record_name(kind, key))?.record.clone();
        let stamp = s.clock.tick(now_ms);
        let rec = current.advanced(phase, stamp, body);
        put_local(&mut s, rec.clone());
        Some(rec)
    }

    pub fn get(&self, kind: Kind, key: &str) -> Option<Record> {
        lock(&self.state)
            .records
            .get(&record_name(kind, key))
            .map(|e| e.record.clone())
    }

    /// Every record, by name.
    pub fn snapshot(&self) -> BTreeMap<String, Record> {
        lock(&self.state)
            .records
            .iter()
            .map(|(n, e)| (n.clone(), e.record.clone()))
            .collect()
    }

    pub fn clock(&self) -> Clock {
        lock(&self.state).clock.clone()
    }

    pub fn last_pull(&self) -> Option<i64> {
        lock(&self.state).last_pull
    }
}

fn put_local(s: &mut LocalState, rec: Record) {
    s.order += 1;
    let order = s.order;
    let name = rec.name();
    // A local edit keeps the server tag it is based on; its content has
    // not been stored yet, so it has no server time.
    let base_tag = s.records.get(&name).and_then(|e| e.base_tag.clone());
    s.records.insert(
        name,
        Entry {
            record: rec,
            base_tag,
            server_ms: None,
            dirty: true,
            order,
        },
    );
}

impl LocalStore for MemoryStore {
    fn sync_token(&self) -> Result<Option<String>> {
        Ok(lock(&self.state).token.clone())
    }

    fn set_sync_token(&self, token: &str) -> Result<()> {
        lock(&self.state).token = Some(token.to_string());
        Ok(())
    }

    fn apply_remote(&self, remote: &RemoteRecord, now_ms: i64) -> Result<Applied> {
        let mut s = lock(&self.state);
        let stored = Some(remote.version.modified_ms);
        s.clock.observe(&remote.record.gen.clamped(stored), now_ms);
        s.clock.observe(&remote.record.at.clamped(stored), now_ms);
        let name = remote.record.name();
        let Some(local) = s.records.get_mut(&name) else {
            s.order += 1;
            let order = s.order;
            s.records.insert(
                name,
                Entry {
                    record: remote.record.clone(),
                    base_tag: Some(remote.version.tag.clone()),
                    server_ms: Some(remote.version.modified_ms),
                    dirty: false,
                    order,
                },
            );
            return Ok(Applied::Took);
        };
        let w = merge(
            (&local.record, local.server_ms),
            (&remote.record, Some(remote.version.modified_ms)),
        );
        local.base_tag = Some(remote.version.tag.clone());
        Ok(match w {
            Winner::Remote => {
                local.record = remote.record.clone();
                local.server_ms = Some(remote.version.modified_ms);
                local.dirty = false;
                Applied::Took
            }
            Winner::Local => {
                local.dirty = true;
                Applied::Kept
            }
            Winner::Same => {
                local.server_ms = Some(remote.version.modified_ms);
                local.dirty = false;
                Applied::Same
            }
        })
    }

    fn forget(&self, name: &str) -> Result<()> {
        let mut s = lock(&self.state);
        if s.records.get(name).is_some_and(|e| !e.dirty) {
            s.records.remove(name);
        }
        Ok(())
    }

    fn pending(&self, limit: usize) -> Result<Vec<Outgoing>> {
        let s = lock(&self.state);
        let mut dirty: Vec<&Entry> = s.records.values().filter(|e| e.dirty).collect();
        dirty.sort_by_key(|e| e.order);
        Ok(dirty
            .into_iter()
            .take(limit)
            .map(|e| Outgoing {
                record: e.record.clone(),
                base_tag: e.base_tag.clone(),
            })
            .collect())
    }

    fn saved(&self, sent: &Outgoing, version: &ServerVersion) -> Result<()> {
        let mut s = lock(&self.state);
        if let Some(e) = s.records.get_mut(&sent.record.name()) {
            e.base_tag = Some(version.tag.clone());
            if e.record == sent.record {
                e.dirty = false;
                e.server_ms = Some(version.modified_ms);
            }
        }
        Ok(())
    }

    fn pull_completed(&self, at_ms: i64) -> Result<()> {
        lock(&self.state).last_pull = Some(at_ms);
        Ok(())
    }
}

/// Convenience for tests: a stamp.
pub fn stamp(wall: i64, count: u32, device: &str) -> Hlc {
    Hlc {
        wall,
        count,
        device: device.into(),
    }
}
