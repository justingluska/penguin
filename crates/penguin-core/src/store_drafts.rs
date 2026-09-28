//! Gmail draft id ↔ draft message id mapping. Gmail gives a draft a new
//! message id on every update, while the draft id stays fixed; the UI works
//! with message ids (Drafts view) and the API with draft ids.

use rusqlite::{params, OptionalExtension};

use super::Store;
use crate::Result;

impl Store {
    /// Record (or move) a draft to its current message id.
    pub fn set_draft(&self, account_id: &str, draft_id: &str, message_id: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO drafts(account_id, draft_id, message_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, draft_id) DO UPDATE SET message_id = excluded.message_id",
                params![account_id, draft_id, message_id],
            )?;
            Ok(())
        })
    }

    pub fn draft_for_message(&self, account_id: &str, message_id: &str) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT draft_id FROM drafts WHERE account_id = ?1 AND message_id = ?2",
            )?
            .query_row(params![account_id, message_id], |r| r.get(0))
            .optional()?)
        })
    }

    pub fn message_for_draft(&self, account_id: &str, draft_id: &str) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.prepare_cached(
                "SELECT message_id FROM drafts WHERE account_id = ?1 AND draft_id = ?2",
            )?
            .query_row(params![account_id, draft_id], |r| r.get(0))
            .optional()?)
        })
    }

    pub fn remove_draft(&self, account_id: &str, draft_id: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM drafts WHERE account_id = ?1 AND draft_id = ?2",
                params![account_id, draft_id],
            )?;
            Ok(())
        })
    }

    /// Replace the account's whole mapping with a fresh `drafts.list` result
    /// (draft id, message id).
    pub fn replace_drafts(&self, account_id: &str, drafts: &[(String, String)]) -> Result<()> {
        self.write(|tx| {
            tx.execute("DELETE FROM drafts WHERE account_id = ?1", [account_id])?;
            let mut stmt = tx.prepare_cached("INSERT OR REPLACE INTO drafts(account_id, draft_id, message_id) VALUES (?1, ?2, ?3)")?;
            for (draft_id, message_id) in drafts {
                stmt.execute(params![account_id, draft_id, message_id])?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Account, Store};

    #[test]
    fn draft_mapping_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let a = "ada@x.example";
        s.set_draft(a, "r1", "m1").unwrap();
        assert_eq!(s.message_for_draft(a, "r1").unwrap().as_deref(), Some("m1"));
        assert_eq!(s.draft_for_message(a, "m1").unwrap().as_deref(), Some("r1"));

        // An update moves the draft to a new message id.
        s.set_draft(a, "r1", "m2").unwrap();
        assert_eq!(s.message_for_draft(a, "r1").unwrap().as_deref(), Some("m2"));
        assert_eq!(s.draft_for_message(a, "m1").unwrap(), None);

        // Accounts are isolated.
        assert_eq!(s.message_for_draft("bo@x.example", "r1").unwrap(), None);

        s.replace_drafts(a, &[("r2".into(), "m9".into())]).unwrap();
        assert_eq!(s.message_for_draft(a, "r1").unwrap(), None);
        assert_eq!(s.draft_for_message(a, "m9").unwrap().as_deref(), Some("r2"));

        s.remove_draft(a, "r2").unwrap();
        assert_eq!(s.draft_for_message(a, "m9").unwrap(), None);
    }

    #[test]
    fn v1_database_migrates_to_v2_and_mapping_persists() {
        let dir = std::env::temp_dir().join(format!(
            "penguin-drafts-migrate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mail.db");
        {
            // A database written before drafts existed.
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::super::SCHEMA_V1).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        {
            let s = Store::open(&path).unwrap();
            s.set_draft("ada@x.example", "r1", "m1").unwrap();
        }
        let s = Store::open(&path).unwrap();
        // Lookups go through the file-backed reader pool.
        assert_eq!(
            s.message_for_draft("ada@x.example", "r1")
                .unwrap()
                .as_deref(),
            Some("m1")
        );
        let version: i64 = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        // Migrated through v2 (and any later migrations).
        assert_eq!(version, super::super::MIGRATIONS.len() as i64);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn removing_account_drops_its_drafts() {
        let s = Store::open_in_memory().unwrap();
        let acct = Account {
            id: "ada@x.example".into(),
            email: "ada@x.example".into(),
            display_name: None,
            nickname: None,
            color: "#000".into(),
            added_at: 0,
            ..Account::default()
        };
        s.upsert_account(&acct).unwrap();
        s.set_draft(&acct.id, "r1", "m1").unwrap();
        s.remove_account(&acct.id).unwrap();
        assert_eq!(s.message_for_draft(&acct.id, "r1").unwrap(), None);
    }
}
