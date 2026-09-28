//! IMAP's own tables (Store::migrate_provider_schema; never MIGRATIONS):
//!
//! - `imap_locations`: where each stored message lives on the server,
//!   `(folder, uid)` → message id, with the copy's flags (and Gmail labels).
//!   UIDs are locations, not identities: a move changes the row, not the id.
//! - `imap_msgids`: Message-ID → thread, for JWZ-style threading.
//! - `imap_subjects`: base subject → thread, for replies that lost their
//!   References (merged only within a time window).
//!
//! Blocking; callers go through `spawn_blocking`.

use std::collections::{HashMap, HashSet};

use penguin_core::rusqlite::{params, params_from_iter, OptionalExtension};
use penguin_core::{Result, Store};

pub const NAME: &str = "imap";
pub const TABLES: &[&str] = &["imap_locations", "imap_msgids", "imap_subjects"];

/// Append-only: never edit an entry that has shipped.
pub const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE imap_locations(
    account_id TEXT NOT NULL,
    folder TEXT NOT NULL,
    uid INTEGER NOT NULL,
    uidvalidity INTEGER NOT NULL,
    message_id TEXT NOT NULL,
    flags TEXT NOT NULL DEFAULT '',
    gm_labels TEXT,
    PRIMARY KEY(account_id, folder, uid)
) WITHOUT ROWID;
CREATE INDEX imap_locations_msg ON imap_locations(account_id, message_id);
CREATE TABLE imap_msgids(
    account_id TEXT NOT NULL,
    msgid TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    PRIMARY KEY(account_id, msgid)
) WITHOUT ROWID;
CREATE INDEX imap_msgids_thread ON imap_msgids(account_id, thread_id);
CREATE TABLE imap_subjects(
    account_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    last_date INTEGER NOT NULL,
    PRIMARY KEY(account_id, subject)
) WITHOUT ROWID;
"#];

pub fn migrate(store: &Store) -> Result<()> {
    store.migrate_provider_schema(NAME, MIGRATIONS, TABLES)
}

/// The flags Penguin tracks on one copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub seen: bool,
    pub flagged: bool,
    pub draft: bool,
}

impl Flags {
    pub fn from_imap(flags: &[String]) -> Flags {
        let has = |f: &str| flags.iter().any(|x| x.eq_ignore_ascii_case(f));
        Flags {
            seen: has("\\Seen"),
            flagged: has("\\Flagged"),
            draft: has("\\Draft"),
        }
    }
    fn encode(self) -> String {
        let mut s = String::new();
        if self.seen {
            s.push('S');
        }
        if self.flagged {
            s.push('F');
        }
        if self.draft {
            s.push('D');
        }
        s
    }
    fn decode(s: &str) -> Flags {
        Flags {
            seen: s.contains('S'),
            flagged: s.contains('F'),
            draft: s.contains('D'),
        }
    }
}

/// One server copy of a stored message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Wire name of the folder.
    pub folder: String,
    pub uid: u32,
    pub uidvalidity: u32,
    pub message_id: String,
    pub flags: Flags,
    /// Gmail: the copy's labels as Penguin label ids.
    pub gm_labels: Option<Vec<String>>,
}

fn row(r: &penguin_core::rusqlite::Row) -> penguin_core::rusqlite::Result<Location> {
    let gm: Option<String> = r.get(5)?;
    Ok(Location {
        folder: r.get(0)?,
        uid: r.get::<_, i64>(1)? as u32,
        uidvalidity: r.get::<_, i64>(2)? as u32,
        message_id: r.get(3)?,
        flags: Flags::decode(&r.get::<_, String>(4)?),
        gm_labels: gm.map(|s| s.split_whitespace().map(str::to_string).collect()),
    })
}

const COLS: &str = "folder, uid, uidvalidity, message_id, flags, gm_labels";

/// Every copy of each message id (ids without copies are absent).
pub fn locations(
    store: &Store,
    account: &str,
    ids: &[String],
) -> Result<HashMap<String, Vec<Location>>> {
    store.provider_read(|c| {
        let mut out: HashMap<String, Vec<Location>> = HashMap::new();
        for chunk in ids.chunks(400) {
            let ph = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT {COLS} FROM imap_locations WHERE account_id = ? AND message_id IN ({ph}) ORDER BY folder, uid"
            );
            let mut stmt = c.prepare(&sql)?;
            let args = std::iter::once(account).chain(chunk.iter().map(String::as_str));
            for l in stmt.query_map(params_from_iter(args), row)? {
                let l = l?;
                out.entry(l.message_id.clone()).or_default().push(l);
            }
        }
        Ok(out)
    })
}

/// The copy at `(folder, uid)`.
pub fn location_at(
    store: &Store,
    account: &str,
    folder: &str,
    uid: u32,
) -> Result<Option<Location>> {
    store.provider_read(|c| {
        Ok(c.prepare_cached(&format!(
            "SELECT {COLS} FROM imap_locations WHERE account_id = ?1 AND folder = ?2 AND uid = ?3"
        ))?
        .query_row(params![account, folder, uid as i64], row)
        .optional()?)
    })
}

/// Every copy in `folder` (uid → location).
pub fn folder_locations(
    store: &Store,
    account: &str,
    folder: &str,
) -> Result<HashMap<u32, Location>> {
    store.provider_read(|c| {
        let mut stmt = c.prepare_cached(&format!(
            "SELECT {COLS} FROM imap_locations WHERE account_id = ?1 AND folder = ?2"
        ))?;
        let rows = stmt.query_map(params![account, folder], row)?;
        let mut out = HashMap::new();
        for l in rows {
            let l = l?;
            out.insert(l.uid, l);
        }
        Ok(out)
    })
}

/// How many copies `folder` holds below `uid_limit`.
pub fn count_below(store: &Store, account: &str, folder: &str, uid_limit: u32) -> Result<u64> {
    store.provider_read(|c| {
        Ok(c.prepare_cached(
            "SELECT count(*) FROM imap_locations WHERE account_id = ?1 AND folder = ?2 AND uid < ?3",
        )?
        .query_row(params![account, folder, uid_limit as i64], |r| r.get::<_, i64>(0))? as u64)
    })
}

/// Insert or replace copies.
pub fn put(store: &Store, account: &str, locs: &[Location]) -> Result<()> {
    if locs.is_empty() {
        return Ok(());
    }
    store.provider_write(|tx| {
        let mut stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO imap_locations(account_id, folder, uid, uidvalidity, message_id, flags, gm_labels)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for l in locs {
            stmt.execute(params![
                account,
                l.folder,
                l.uid as i64,
                l.uidvalidity as i64,
                l.message_id,
                l.flags.encode(),
                l.gm_labels.as_ref().map(|g| g.join(" ")),
            ])?;
        }
        Ok(())
    })
}

/// Remove copies at `uids` of `folder`; returns the message ids they held.
pub fn remove(store: &Store, account: &str, folder: &str, uids: &[u32]) -> Result<Vec<String>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    store.provider_write(|tx| {
        let mut ids = HashSet::new();
        for uid in uids {
            let id: Option<String> = tx
                .prepare_cached(
                    "DELETE FROM imap_locations WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 RETURNING message_id",
                )?
                .query_row(params![account, folder, *uid as i64], |r| r.get(0))
                .optional()?;
            ids.extend(id);
        }
        Ok(ids.into_iter().collect())
    })
}

/// Remove every copy in `folder` (UIDVALIDITY changed); returns their ids.
pub fn clear_folder(store: &Store, account: &str, folder: &str) -> Result<Vec<String>> {
    store.provider_write(|tx| {
        let ids: Vec<String> = tx
            .prepare_cached(
                "DELETE FROM imap_locations WHERE account_id = ?1 AND folder = ?2 RETURNING message_id",
            )?
            .query_map(params![account, folder], |r| r.get(0))?
            .collect::<penguin_core::rusqlite::Result<_>>()?;
        let set: HashSet<String> = ids.into_iter().collect();
        Ok(set.into_iter().collect())
    })
}

/// Remove every copy of these message ids (they're gone locally).
pub fn forget_messages(store: &Store, account: &str, ids: &[String]) -> Result<()> {
    store.provider_write(|tx| {
        for id in ids {
            tx.prepare_cached(
                "DELETE FROM imap_locations WHERE account_id = ?1 AND message_id = ?2",
            )?
            .execute(params![account, id])?;
        }
        Ok(())
    })
}

/// Update one copy's flags / Gmail labels (unknown copies are ignored).
pub fn set_flags(
    store: &Store,
    account: &str,
    changes: &[(String, u32, Flags, Option<Vec<String>>)],
) -> Result<()> {
    store.provider_write(|tx| {
        for (folder, uid, flags, gm) in changes {
            match gm {
                Some(g) => tx
                    .prepare_cached(
                        "UPDATE imap_locations SET flags = ?4, gm_labels = ?5 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
                    )?
                    .execute(params![account, folder, *uid as i64, flags.encode(), g.join(" ")])?,
                None => tx
                    .prepare_cached(
                        "UPDATE imap_locations SET flags = ?4 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
                    )?
                    .execute(params![account, folder, *uid as i64, flags.encode()])?,
            };
        }
        Ok(())
    })
}

/// Of `ids`, those with no copy left on the server.
pub fn orphans(store: &Store, account: &str, ids: &[String]) -> Result<Vec<String>> {
    let located = locations(store, account, ids)?;
    Ok(ids
        .iter()
        .filter(|id| !located.contains_key(*id))
        .cloned()
        .collect())
}

// ---------- threading tables ----------

/// Known threads of these normalized Message-IDs.
pub fn threads_of(
    store: &Store,
    account: &str,
    msgids: &[String],
) -> Result<HashMap<String, String>> {
    store.provider_read(|c| {
        let mut out = HashMap::new();
        let mut stmt = c.prepare_cached(
            "SELECT thread_id FROM imap_msgids WHERE account_id = ?1 AND msgid = ?2",
        )?;
        for m in msgids {
            if let Some(t) = stmt
                .query_row(params![account, m], |r| r.get::<_, String>(0))
                .optional()?
            {
                out.insert(m.clone(), t);
            }
        }
        Ok(out)
    })
}

/// Point these Message-IDs at `thread`, and every Message-ID of the
/// `merged` threads too.
pub fn set_threads(
    store: &Store,
    account: &str,
    msgids: &[String],
    thread: &str,
    merged: &[String],
) -> Result<()> {
    store.provider_write(|tx| {
        for t in merged {
            tx.prepare_cached(
                "UPDATE imap_msgids SET thread_id = ?3 WHERE account_id = ?1 AND thread_id = ?2",
            )?
            .execute(params![account, t, thread])?;
            tx.prepare_cached(
                "UPDATE imap_subjects SET thread_id = ?3 WHERE account_id = ?1 AND thread_id = ?2",
            )?
            .execute(params![account, t, thread])?;
        }
        let mut stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO imap_msgids(account_id, msgid, thread_id) VALUES (?1, ?2, ?3)",
        )?;
        for m in msgids {
            stmt.execute(params![account, m, thread])?;
        }
        Ok(())
    })
}

/// The thread a base subject was last seen in, if at or after `since_ms`.
pub fn subject_thread(
    store: &Store,
    account: &str,
    key: &str,
    since_ms: i64,
) -> Result<Option<String>> {
    store.provider_read(|c| {
        Ok(c.prepare_cached(
            "SELECT thread_id FROM imap_subjects WHERE account_id = ?1 AND subject = ?2 AND last_date >= ?3",
        )?
        .query_row(params![account, key, since_ms], |r| r.get(0))
        .optional()?)
    })
}

pub fn put_subject(
    store: &Store,
    account: &str,
    key: &str,
    thread: &str,
    date_ms: i64,
) -> Result<()> {
    store.provider_write(|tx| {
        tx.execute(
            "INSERT INTO imap_subjects(account_id, subject, thread_id, last_date) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, subject) DO UPDATE SET
               thread_id = excluded.thread_id,
               last_date = max(last_date, excluded.last_date)
             WHERE excluded.last_date >= imap_subjects.last_date OR imap_subjects.thread_id = excluded.thread_id",
            params![account, key, thread, date_ms],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "sam@mail.example";

    fn loc(folder: &str, uid: u32, id: &str) -> Location {
        Location {
            folder: folder.into(),
            uid,
            uidvalidity: 7,
            message_id: id.into(),
            flags: Flags {
                seen: true,
                ..Flags::default()
            },
            gm_labels: None,
        }
    }

    #[test]
    fn locations_round_trip_and_orphans() {
        let store = Store::open_in_memory().unwrap();
        migrate(&store).unwrap();
        migrate(&store).unwrap();
        put(
            &store,
            A,
            &[
                loc("INBOX", 1, "h:a"),
                loc("Archive", 9, "h:a"),
                loc("INBOX", 2, "h:b"),
            ],
        )
        .unwrap();
        let all = locations(&store, A, &["h:a".into(), "h:b".into(), "h:c".into()]).unwrap();
        assert_eq!(all["h:a"].len(), 2);
        assert!(all["h:a"][0].flags.seen);
        assert!(!all.contains_key("h:c"));
        assert_eq!(count_below(&store, A, "INBOX", 3).unwrap(), 2);
        assert_eq!(count_below(&store, A, "INBOX", 2).unwrap(), 1);
        assert_eq!(
            remove(&store, A, "INBOX", &[1, 5]).unwrap(),
            vec!["h:a".to_string()]
        );
        assert!(orphans(&store, A, &["h:a".into()]).unwrap().is_empty());
        let mut cleared = clear_folder(&store, A, "INBOX").unwrap();
        cleared.sort();
        assert_eq!(cleared, vec!["h:b".to_string()]);
        assert_eq!(
            orphans(&store, A, &["h:a".into(), "h:b".into()]).unwrap(),
            vec!["h:b".to_string()]
        );
        set_flags(
            &store,
            A,
            &[(
                "Archive".into(),
                9,
                Flags {
                    flagged: true,
                    ..Flags::default()
                },
                Some(vec!["INBOX".into(), "f:X".into()]),
            )],
        )
        .unwrap();
        let l = location_at(&store, A, "Archive", 9).unwrap().unwrap();
        assert!(l.flags.flagged && !l.flags.seen);
        assert_eq!(
            l.gm_labels,
            Some(vec!["INBOX".to_string(), "f:X".to_string()])
        );
        // Removing the account empties the tables.
        store
            .upsert_account(&penguin_core::Account {
                id: A.into(),
                email: A.into(),
                ..Default::default()
            })
            .unwrap();
        set_threads(&store, A, &["a@x.example".into()], "t:1", &[]).unwrap();
        store.remove_account(A).unwrap();
        assert!(folder_locations(&store, A, "Archive").unwrap().is_empty());
        assert!(threads_of(&store, A, &["a@x.example".into()])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn thread_merges_move_every_msgid() {
        let store = Store::open_in_memory().unwrap();
        migrate(&store).unwrap();
        set_threads(&store, A, &["a@x".into(), "b@x".into()], "t:1", &[]).unwrap();
        set_threads(&store, A, &["c@x".into()], "t:2", &[]).unwrap();
        put_subject(&store, A, "k", "t:2", 100).unwrap();
        set_threads(&store, A, &["d@x".into()], "t:1", &["t:2".into()]).unwrap();
        let t = threads_of(&store, A, &["a@x".into(), "c@x".into(), "d@x".into()]).unwrap();
        assert!(t.values().all(|v| v == "t:1"), "{t:?}");
        assert_eq!(
            subject_thread(&store, A, "k", 50).unwrap().as_deref(),
            Some("t:1")
        );
        assert_eq!(subject_thread(&store, A, "k", 150).unwrap(), None);
    }
}
