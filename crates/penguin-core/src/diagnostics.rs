//! Read-only database statistics for the app's diagnostics screen. Counts,
//! sizes and names only: nothing here reads message content.

use std::collections::BTreeMap;

use rusqlite::params;
use serde::Serialize;

use crate::{Result, Store};

/// Cheap page-level numbers plus counts from counter tables and the thread
/// b-tree (no scans of message rows).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DbStats {
    pub page_size: u64,
    pub page_count: u64,
    pub freelist_count: u64,
    pub total_messages: u64,
    pub total_threads: u64,
    /// account id → messages stored (from `message_counts`).
    pub messages_by_account: BTreeMap<String, u64>,
}

/// One b-tree's size on disk.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableSize {
    pub name: String,
    /// "table" | "index" | "fts" (FTS5 shadow tables are summed under their
    /// virtual table's name).
    pub kind: String,
    pub bytes: u64,
}

impl Store {
    pub fn db_stats(&self) -> Result<DbStats> {
        self.read(|c| {
            let pragma = |name: &str| -> Result<u64> {
                Ok(c.query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, i64>(0))? as u64)
            };
            let mut by_account = BTreeMap::new();
            let mut stmt = c.prepare_cached("SELECT account_id, n FROM message_counts")?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (id, n) = row?;
                by_account.insert(id, n.max(0) as u64);
            }
            // count(*) on a rowid table walks only interior/leaf page headers
            // (sqlite3BtreeCount), not the row payloads.
            let total_threads: i64 =
                c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?;
            Ok(DbStats {
                page_size: pragma("page_size")?,
                page_count: pragma("page_count")?,
                freelist_count: pragma("freelist_count")?,
                total_messages: by_account.values().sum(),
                total_threads: total_threads.max(0) as u64,
                messages_by_account: by_account,
            })
        })
    }

    /// Per-table and per-index bytes via the `dbstat` virtual table (compiled
    /// into the bundled SQLite). This visits every page, so it is O(db size):
    /// call it on demand, not on a hot path. Largest first.
    pub fn table_sizes(&self) -> Result<Vec<TableSize>> {
        self.read(|c| {
            let mut kinds: BTreeMap<String, String> = BTreeMap::new();
            let mut fts: Vec<String> = Vec::new();
            {
                let mut stmt =
                    c.prepare("SELECT name, type, coalesce(sql, '') FROM sqlite_schema")?;
                for row in stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })? {
                    let (name, kind, sql) = row?;
                    if kind == "table" && sql.to_ascii_uppercase().contains("USING FTS5") {
                        fts.push(name.clone());
                    }
                    kinds.insert(name, kind);
                }
            }
            let mut sizes: BTreeMap<(String, String), u64> = BTreeMap::new();
            let mut stmt = c.prepare("SELECT name, sum(pgsize) FROM dbstat GROUP BY name")?;
            for row in stmt.query_map(params![], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })? {
                let (name, bytes) = row?;
                // FTS5 shadow tables (x_data, x_idx, x_docsize, x_config,
                // x_content) and their autoindexes roll up under x.
                let owner = fts.iter().find(|f| {
                    name.strip_prefix(f.as_str())
                        .is_some_and(|rest| rest.starts_with('_'))
                        || name
                            .strip_prefix("sqlite_autoindex_")
                            .and_then(|r| r.strip_prefix(f.as_str()))
                            .is_some_and(|rest| rest.starts_with('_'))
                });
                let key = match owner {
                    Some(f) => (f.clone(), "fts".to_string()),
                    None => {
                        let kind = kinds.get(&name).cloned().unwrap_or_else(|| {
                            if name.starts_with("sqlite_autoindex_") {
                                "index".into()
                            } else {
                                "table".into()
                            }
                        });
                        (name, kind)
                    }
                };
                *sizes.entry(key).or_default() += bytes.max(0) as u64;
            }
            let mut out: Vec<TableSize> = sizes
                .into_iter()
                .map(|((name, kind), bytes)| TableSize { name, kind, bytes })
                .collect();
            out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_and_sizes_on_empty_store() {
        let store = Store::open_in_memory().unwrap();
        let stats = store.db_stats().unwrap();
        assert_eq!(stats.total_messages, 0);
        assert_eq!(stats.total_threads, 0);
        assert!(stats.page_size > 0 && stats.page_count > 0);

        let sizes = store.table_sizes().unwrap();
        let find = |n: &str| sizes.iter().find(|t| t.name == n).cloned();
        assert_eq!(find("messages").unwrap().kind, "table");
        assert_eq!(find("messages_fts").unwrap().kind, "fts");
        assert_eq!(find("thread_views_all").unwrap().kind, "index");
        // Shadow tables are folded into their FTS table.
        assert!(find("messages_fts_data").is_none());
        assert!(sizes.windows(2).all(|w| w[0].bytes >= w[1].bytes));
    }

    fn message(account: &str, id: &str, thread: &str) -> crate::Message {
        let who = crate::Address {
            name: None,
            email: "bo@x.example".into(),
        };
        crate::Message {
            account_id: account.into(),
            id: id.into(),
            thread_id: thread.into(),
            date: 1_700_000_000_000,
            from: who,
            to: vec![crate::Address {
                name: None,
                email: account.into(),
            }],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Hello".into(),
            snippet: "hi".into(),
            body_text: "hello there".into(),
            body_html: None,
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    #[test]
    fn stats_on_a_file_backed_store() {
        let dir = std::env::temp_dir().join(format!("penguin-core-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir.join("penguin.db")).unwrap();
        let msgs: Vec<_> = (0..30)
            .map(|i| {
                message(
                    if i % 3 == 0 {
                        "b@x.example"
                    } else {
                        "a@x.example"
                    },
                    &format!("m{i}"),
                    &format!("t{}", i / 2),
                )
            })
            .collect();
        store.upsert_messages(&msgs).unwrap();

        let stats = store.db_stats().unwrap();
        assert_eq!(stats.total_messages, 30);
        assert_eq!(stats.messages_by_account["a@x.example"], 20);
        assert_eq!(stats.messages_by_account["b@x.example"], 10);
        // Threads are per account: t0..t14 split across both accounts.
        assert_eq!(stats.total_threads, 25);

        let sizes = store.table_sizes().unwrap();
        let total: u64 = sizes.iter().map(|t| t.bytes).sum();
        assert!(total > 0 && total <= stats.page_count * stats.page_size);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
