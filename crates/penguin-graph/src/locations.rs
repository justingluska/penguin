//! Where each stored message lives on the server: `graph_locations
//! (account_id, message_id, folder_id)`, a provider-owned table
//! (`Store::migrate_provider_schema`, emptied with the account).
//!
//! Per-folder delta reports a move as "removed" from the old folder and
//! "added" to the new one, in whichever order the folders are polled. The
//! location decides whether a removal is a deletion (the message is still
//! recorded in that folder) or the old half of a move (it's recorded
//! elsewhere already). Moves made from Penguin update it immediately.

use std::collections::HashMap;

use penguin_core::rusqlite::{params, OptionalExtension};
use penguin_core::Store;

pub(crate) const SCHEMA_NAME: &str = "graph";
const GRAPH_V1: &str = r#"
CREATE TABLE graph_locations(
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    folder_id TEXT NOT NULL,
    PRIMARY KEY(account_id, message_id)
) WITHOUT ROWID;
CREATE INDEX graph_locations_folder ON graph_locations(account_id, folder_id);
"#;
const TABLES: [&str; 1] = ["graph_locations"];

/// Create or update Penguin's Graph tables (cheap when current).
pub fn migrate(store: &Store) -> penguin_core::Result<()> {
    store.migrate_provider_schema(SCHEMA_NAME, &[GRAPH_V1], &TABLES)
}

pub(crate) fn set(
    store: &Store,
    account_id: &str,
    entries: &[(String, String)],
) -> penguin_core::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    store.provider_write(|tx| {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO graph_locations(account_id, message_id, folder_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, message_id) DO UPDATE SET folder_id = excluded.folder_id",
        )?;
        for (m, f) in entries {
            stmt.execute(params![account_id, m, f])?;
        }
        Ok(())
    })
}

#[cfg(test)]
pub(crate) fn get(
    store: &Store,
    account_id: &str,
    message_id: &str,
) -> penguin_core::Result<Option<String>> {
    store.provider_read(|c| {
        Ok(c.prepare_cached(
            "SELECT folder_id FROM graph_locations WHERE account_id = ?1 AND message_id = ?2",
        )?
        .query_row(params![account_id, message_id], |r| r.get(0))
        .optional()?)
    })
}

pub(crate) fn get_many(
    store: &Store,
    account_id: &str,
    message_ids: &[String],
) -> penguin_core::Result<HashMap<String, String>> {
    store.provider_read(|c| {
        let mut stmt = c.prepare_cached(
            "SELECT folder_id FROM graph_locations WHERE account_id = ?1 AND message_id = ?2",
        )?;
        let mut out = HashMap::new();
        for id in message_ids {
            if let Some(f) = stmt
                .query_row(params![account_id, id], |r| r.get::<_, String>(0))
                .optional()?
            {
                out.insert(id.clone(), f);
            }
        }
        Ok(out)
    })
}

pub(crate) fn in_folder(
    store: &Store,
    account_id: &str,
    folder_id: &str,
) -> penguin_core::Result<Vec<String>> {
    store.provider_read(|c| {
        let rows = c
            .prepare_cached(
                "SELECT message_id FROM graph_locations WHERE account_id = ?1 AND folder_id = ?2",
            )?
            .query_map(params![account_id, folder_id], |r| r.get(0))?
            .collect::<penguin_core::rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    })
}

/// Every folder id that holds messages for the account.
pub(crate) fn folders(store: &Store, account_id: &str) -> penguin_core::Result<Vec<String>> {
    store.provider_read(|c| {
        let rows = c
            .prepare_cached("SELECT DISTINCT folder_id FROM graph_locations WHERE account_id = ?1")?
            .query_map(params![account_id], |r| r.get(0))?
            .collect::<penguin_core::rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    })
}

pub(crate) fn delete(
    store: &Store,
    account_id: &str,
    message_ids: &[String],
) -> penguin_core::Result<()> {
    if message_ids.is_empty() {
        return Ok(());
    }
    store.provider_write(|tx| {
        let mut stmt = tx.prepare_cached(
            "DELETE FROM graph_locations WHERE account_id = ?1 AND message_id = ?2",
        )?;
        for id in message_ids {
            stmt.execute(params![account_id, id])?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_are_per_account_and_removed_with_it() {
        let store = Store::open_in_memory().unwrap();
        migrate(&store).unwrap();
        migrate(&store).unwrap();
        set(
            &store,
            "a@x.example",
            &[("m1".into(), "F1".into()), ("m2".into(), "F1".into())],
        )
        .unwrap();
        set(&store, "b@x.example", &[("m1".into(), "F9".into())]).unwrap();
        set(&store, "a@x.example", &[("m2".into(), "F2".into())]).unwrap();
        assert_eq!(
            get(&store, "a@x.example", "m2").unwrap().as_deref(),
            Some("F2")
        );
        assert_eq!(in_folder(&store, "a@x.example", "F1").unwrap(), vec!["m1"]);
        assert_eq!(
            get_many(&store, "b@x.example", &["m1".into(), "zz".into()])
                .unwrap()
                .len(),
            1
        );
        let mut fs = folders(&store, "a@x.example").unwrap();
        fs.sort();
        assert_eq!(fs, vec!["F1", "F2"]);
        delete(&store, "a@x.example", &["m1".into()]).unwrap();
        assert_eq!(get(&store, "a@x.example", "m1").unwrap(), None);
        store.remove_account("b@x.example").unwrap();
        assert_eq!(get(&store, "b@x.example", "m1").unwrap(), None);
        assert!(get(&store, "a@x.example", "m2").unwrap().is_some());
    }
}
