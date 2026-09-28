//! Read-only store for agent access (`penguin-cli mcp`, CLI reads): every
//! connection is opened `SQLITE_OPEN_READ_ONLY` with `query_only`, so a bug
//! or a hostile tool call can't change the mailbox, and the file is never
//! created or migrated.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};

use super::{ReaderPool, Store, MIGRATIONS};
use crate::{Error, Result};

impl Store {
    /// Open an existing database for reading only. Fails if the file is
    /// missing or its schema isn't exactly this build's (an older database
    /// must be upgraded by opening Penguin, which owns migrations).
    pub fn open_read_only(path: &Path) -> Result<Store> {
        if !path.is_file() {
            return Err(Error::NotFound(format!(
                "no Penguin database at {}",
                path.display()
            )));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch(
            // Reader pragmas only (see open_reader); journal_mode is never touched.
            "PRAGMA query_only = ON;
             PRAGMA cache_size = -32768;
             PRAGMA mmap_size = 1073741824;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 5000;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let expected = MIGRATIONS.len() as i64;
        if version < expected {
            return Err(Error::Db(format!(
                "database schema v{version} is older than this build (v{expected}): open Penguin once to upgrade the database"
            )));
        }
        if version > expected {
            return Err(Error::Db(format!(
                "database schema v{version} is newer than this build (v{expected})"
            )));
        }
        Ok(Store::from_parts(
            conn,
            Some(ReaderPool {
                path: path.to_path_buf(),
                idle: Mutex::new(Vec::new()),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Account, Address, Message, SearchRequest, Store};

    fn temp_db(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("penguin-ro-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("mail.db")
    }

    fn account() -> Account {
        Account {
            id: "ada@x.example".into(),
            email: "ada@x.example".into(),
            display_name: None,
            nickname: None,
            color: "#000000".into(),
            added_at: 0,
            ..Account::default()
        }
    }

    fn message() -> Message {
        Message {
            account_id: "ada@x.example".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 1_700_000_000_000,
            from: Address {
                name: Some("Bo".into()),
                email: "bo@x.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Walrus migration".into(),
            snippet: "walrus".into(),
            body_text: "The walrus plan is ready".into(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    #[test]
    fn reads_work_and_writes_are_impossible() {
        let path = temp_db("rw");
        {
            let s = Store::open(&path).unwrap();
            s.upsert_account(&account()).unwrap();
            s.upsert_messages(&[message()]).unwrap();
        }
        // The writer is still open elsewhere in real life; keep one here too.
        let writer = Store::open(&path).unwrap();
        let before = std::fs::read(&path).unwrap();

        let ro = Store::open_read_only(&path).unwrap();
        let hits = ro
            .search(&SearchRequest {
                query: "walrus".into(),
                account_id: None,
                account_ids: None,
                limit: 10,
            })
            .unwrap()
            .hits;
        assert_eq!(hits.len(), 1);
        assert_eq!(ro.list_accounts().unwrap().len(), 1);

        assert!(ro
            .upsert_account(&Account {
                id: "evil@x.example".into(),
                ..account()
            })
            .is_err());
        assert!(ro
            .upsert_messages(&[Message {
                id: "m2".into(),
                ..message()
            }])
            .is_err());
        assert!(ro.delete_messages("ada@x.example", &["m1".into()]).is_err());
        assert!(ro.remove_account("ada@x.example").is_err());
        assert!(ro.set_draft("ada@x.example", "r1", "m1").is_err());
        assert!(ro.optimize().is_err());
        assert!(matches!(
            ro.upsert_account(&account()),
            Err(crate::Error::Db(_))
        ));

        assert_eq!(ro.count_messages(None).unwrap(), 1);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "database file unchanged"
        );
        drop(writer);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The app quit cleanly: its last connection checkpointed and removed
    /// -wal/-shm. A read-only open must still read (SQLite handles WAL
    /// read-only without -shm when the directory is writable).
    #[test]
    fn reads_after_writer_closed_and_checkpointed() {
        let path = temp_db("closed");
        {
            let s = Store::open(&path).unwrap();
            s.upsert_account(&account()).unwrap();
            s.upsert_messages(&[message()]).unwrap();
        }
        let wal = path.with_file_name("mail.db-wal");
        assert!(!wal.exists(), "clean close removes the WAL");
        let before = std::fs::read(&path).unwrap();

        let ro = Store::open_read_only(&path).unwrap();
        let r = ro
            .search(&SearchRequest {
                query: "walrus".into(),
                account_id: None,
                account_ids: None,
                limit: 10,
            })
            .unwrap();
        assert_eq!(r.hits.len(), 1);
        assert!(ro.get_thread("ada@x.example", "t1").unwrap().is_some());
        assert!(ro.upsert_account(&account()).is_err());
        drop(ro);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "main db file unchanged"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_database_is_not_created() {
        let path = temp_db("missing");
        std::fs::remove_file(&path).ok();
        assert!(Store::open_read_only(&path).is_err());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn outdated_schema_is_refused_not_migrated() {
        let path = temp_db("old");
        {
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(super::super::SCHEMA_V1).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
        }
        let err = Store::open_read_only(&path).err().expect("refused");
        assert!(err.to_string().contains("open Penguin once"), "{err}");
        let version: i64 = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
