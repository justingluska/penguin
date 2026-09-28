//! Providers in the store: the migration that adds `Account.provider` and
//! moves Gmail's cursor columns into the opaque provider state, provider
//! configs, provider-owned tables and moving messages between threads.

use rusqlite::{params, Connection};

use super::super::MIGRATIONS;
use crate::types::*;
use crate::Store;

const A: &str = "ada@penguin.example";
const B: &str = "bea@penguin.example";
const T0: i64 = 1_760_000_000_000;

fn temp_db(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "penguin-providers-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mail.db");
    (dir, path)
}

/// Index of the first provider migration in `MIGRATIONS`: a database at
/// this version is what every build before providers wrote.
fn pre_provider_version() -> usize {
    MIGRATIONS
        .iter()
        .position(|m| *m == super::SCHEMA_ACCOUNT_PROVIDER)
        .expect("provider migration is registered")
}

/// A database as the last pre-provider build left it: two Gmail accounts,
/// one with a full cursor (history id past 2^53 to catch float rounding),
/// one whose cursor has no history id or page token yet.
fn old_database(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    let v = pre_provider_version();
    for sql in &MIGRATIONS[..v] {
        conn.execute_batch(sql).unwrap();
    }
    conn.pragma_update(None, "user_version", v as i64).unwrap();
    conn.execute(
        "INSERT INTO accounts(id, email, display_name, nickname, color, added_at) VALUES (?1, ?1, 'Ada', 'Work', '#123456', 5)",
        [A],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO accounts(id, email, display_name, nickname, color, added_at) VALUES (?1, ?1, NULL, NULL, '#654321', 6)",
        [B],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sync_cursors(account_id, history_id, backfill_page_token, backfill_done, failed_message_ids, window)
         VALUES (?1, ?2, 'page-7', 0, '[\"bad1\",\"bad2\"]', ?3)",
        params![
            A,
            9_007_199_254_740_993_i64,
            r#"{"fullSinceMs":1700000000000,"fillPageToken":"fill-tok","olderMode":"headers","olderDone":true}"#
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sync_cursors(account_id, history_id, backfill_page_token, backfill_done, failed_message_ids)
         VALUES (?1, NULL, NULL, 1, '[]')",
        [B],
    )
    .unwrap();
}

#[test]
fn old_database_migrates_accounts_to_gmail_and_keeps_cursors() {
    let (dir, path) = temp_db("migrate");
    old_database(&path);
    {
        let s = Store::open(&path).unwrap();
        let accounts = s.list_accounts().unwrap();
        assert_eq!(accounts.len(), 2);
        for a in &accounts {
            assert_eq!(a.provider, AccountProvider::Gmail);
            assert_eq!(a.provider_config, ProviderConfig::default());
            assert_eq!(a.capabilities, AccountProvider::Gmail.capabilities());
        }
        assert_eq!(accounts[0].nickname.as_deref(), Some("Work"));
        assert_eq!(accounts[0].display_name.as_deref(), Some("Ada"));

        let a = s.get_sync_cursor(A).unwrap();
        let state: serde_json::Value = serde_json::from_str(&a.provider_state).unwrap();
        assert_eq!(state["historyId"].as_u64(), Some(9_007_199_254_740_993));
        assert_eq!(state["backfillPageToken"], "page-7");
        assert!(!a.backfill_done);
        assert_eq!(a.failed_message_ids, vec!["bad1", "bad2"]);
        assert_eq!(a.window.full_since_ms, Some(1_700_000_000_000));
        assert_eq!(a.window.fill_page_token.as_deref(), Some("fill-tok"));
        assert_eq!(a.window.older_mode.as_deref(), Some("headers"));
        assert!(a.window.older_done);

        // Nothing recorded yet stays "nothing recorded".
        let b = s.get_sync_cursor(B).unwrap();
        assert_eq!(b.provider_state, "");
        assert!(b.backfill_done);
        assert_eq!(b.window, WindowCursor::default());
    }
    // The Gmail columns are gone; the schema is current.
    let conn = Connection::open(&path).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('sync_cursors')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!cols
        .iter()
        .any(|c| c == "history_id" || c == "backfill_page_token"));
    assert!(cols.iter().any(|c| c == "provider_state"));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, MIGRATIONS.len() as i64);
    drop(conn);
    let _ = std::fs::remove_dir_all(dir);
}

fn account(id: &str, provider: AccountProvider) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        color: "#123456".into(),
        added_at: 1,
        provider,
        ..Account::default()
    }
}

fn imap_config() -> ProviderConfig {
    ProviderConfig {
        auth: Some(AuthMethod::AppPassword),
        imap: Some(ServerSettings {
            host: "imap.mail.example".into(),
            port: 993,
            security: MailSecurity::Tls,
            username: "ada".into(),
        }),
        smtp: Some(ServerSettings {
            host: "smtp.mail.example".into(),
            port: 587,
            security: MailSecurity::Starttls,
            username: A.into(),
        }),
        host: Some("icloud".into()),
        ..ProviderConfig::default()
    }
}

#[test]
fn provider_and_config_round_trip_and_capabilities_are_derived() {
    let s = Store::open_in_memory().unwrap();
    let mut imap = account(A, AccountProvider::Imap);
    imap.provider_config = imap_config();
    // Whatever the caller passes, capabilities come from the provider.
    imap.capabilities = AccountProvider::Gmail.capabilities();
    s.upsert_account(&imap).unwrap();
    s.upsert_account(&account(B, AccountProvider::Microsoft))
        .unwrap();
    let got = s.list_accounts().unwrap();
    assert_eq!(got[0].provider, AccountProvider::Imap);
    assert_eq!(got[0].provider_config, imap_config());
    assert_eq!(got[0].capabilities, AccountProvider::Imap.capabilities());
    assert!(got[0].capabilities.folders && !got[0].capabilities.calendar);
    assert_eq!(got[1].provider, AccountProvider::Microsoft);

    // Signing in again replaces the config (new server settings) but keeps
    // the nickname, as for Gmail.
    s.update_account(A, Some(Some("Home")), None).unwrap();
    let mut again = account(A, AccountProvider::Imap);
    again.provider_config.imap = Some(ServerSettings {
        port: 143,
        security: MailSecurity::Starttls,
        ..imap_config().imap.unwrap()
    });
    s.upsert_account(&again).unwrap();
    let a = &s.list_accounts().unwrap()[0];
    assert_eq!(a.nickname.as_deref(), Some("Home"));
    assert_eq!(a.provider_config.imap.as_ref().unwrap().port, 143);
    assert_eq!(a.provider_config.smtp, None);
}

#[test]
fn account_json_is_camel_case_and_old_json_reads_as_gmail() {
    let mut a = account(A, AccountProvider::Imap);
    a.provider_config = imap_config();
    a.capabilities = a.provider.capabilities();
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["provider"], "imap");
    assert_eq!(v["providerConfig"]["imap"]["security"], "tls");
    assert_eq!(v["providerConfig"]["auth"], "appPassword");
    assert_eq!(v["capabilities"]["serverSearch"], true);
    assert_eq!(v["capabilities"]["inboxCategories"], false);
    let old: Account = serde_json::from_str(
        r##"{"id":"a@x.example","email":"a@x.example","displayName":null,"color":"#000000","addedAt":1}"##,
    )
    .unwrap();
    assert_eq!(old.provider, AccountProvider::Gmail);
    for p in AccountProvider::ALL {
        assert_eq!(AccountProvider::parse(p.as_str()), Some(p));
        assert_eq!(serde_json::to_value(p).unwrap(), p.as_str());
    }
}

#[test]
fn unknown_provider_in_the_database_is_an_error_not_gmail() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A, AccountProvider::Gmail))
        .unwrap();
    s.provider_write(|tx| {
        tx.execute("UPDATE accounts SET provider = 'jmap'", [])?;
        Ok(())
    })
    .unwrap();
    let e = s.list_accounts().unwrap_err().to_string();
    assert!(e.contains("unknown provider"), "{e}");
}

#[test]
fn provider_tables_migrate_once_and_follow_account_removal() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A, AccountProvider::Imap))
        .unwrap();
    s.upsert_account(&account(B, AccountProvider::Imap))
        .unwrap();
    let v1 = "CREATE TABLE imap_locations(account_id TEXT NOT NULL, message_id TEXT NOT NULL, folder TEXT NOT NULL, uid INTEGER NOT NULL);";
    let v2 = "CREATE INDEX imap_locations_msg ON imap_locations(account_id, message_id);";
    s.migrate_provider_schema("imap", &[v1], &["imap_locations"])
        .unwrap();
    // Re-running is a no-op; appending runs only the new one.
    s.migrate_provider_schema("imap", &[v1], &["imap_locations"])
        .unwrap();
    s.migrate_provider_schema("imap", &[v1, v2], &["imap_locations"])
        .unwrap();
    // A build that knows fewer migrations refuses the newer schema.
    assert!(s
        .migrate_provider_schema("imap", &[v1], &["imap_locations"])
        .is_err());
    // Names are checked: tables must carry the provider's prefix.
    assert!(s
        .migrate_provider_schema("graph", &[], &["imap_locations"])
        .is_err());
    assert!(s.migrate_provider_schema("Bad Name", &[], &[]).is_err());

    s.provider_write(|tx| {
        for (acct, uid) in [(A, 1), (A, 2), (B, 3)] {
            tx.execute(
                "INSERT INTO imap_locations VALUES (?1, 'm', 'INBOX', ?2)",
                params![acct, uid],
            )?;
        }
        Ok(())
    })
    .unwrap();
    s.remove_account(A).unwrap();
    let left: Vec<String> = s
        .provider_read(|c| {
            Ok(c.prepare("SELECT account_id FROM imap_locations")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(left, vec![B.to_string()]);
}

fn msg(id: &str, thread: &str, date: i64) -> Message {
    Message {
        account_id: A.into(),
        id: id.into(),
        thread_id: thread.into(),
        date,
        from: Address {
            name: Some("Cy Quill".into()),
            email: "cy@quill.example".into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: format!("subject {id}"),
        snippet: String::new(),
        body_text: "hello".into(),
        body_html: None,
        label_ids: vec!["INBOX".into(), "UNREAD".into()],
        attachments: vec![],
        message_id_header: Some(format!("<{id}@quill.example>")),
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: false,
    }
}

#[test]
fn set_message_thread_merges_threads_and_drops_the_empty_one() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A, AccountProvider::Imap))
        .unwrap();
    s.upsert_messages(&[msg("m1", "t:a", T0), msg("m2", "t:b", T0 + 1000)])
        .unwrap();
    let changed = s
        .set_message_thread(A, &["m2".into(), "nope".into()], "t:a")
        .unwrap();
    assert_eq!(changed, vec!["t:a".to_string(), "t:b".to_string()]);
    let t = s.get_thread(A, "t:a").unwrap().unwrap();
    assert_eq!(
        t.messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["m1", "m2"]
    );
    assert!(s.get_thread(A, "t:b").unwrap().is_none());
    let inbox = s
        .list_threads(&ListQuery {
            view: MailboxView::Inbox,
            tab: None,
            account_id: Some(A.into()),
            account_ids: None,
            limit: 10,
            before: None,
            unread_only: false,
            split: None,
        })
        .unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].message_count, 2);
    assert_eq!(inbox[0].last_date, T0 + 1000);
    // Moving into a brand-new thread splits it out again; a no-op move
    // leaves no empty thread behind.
    assert_eq!(
        s.set_message_thread(A, &["m1".into()], "t:a").unwrap(),
        Vec::<String>::new()
    );
    s.set_message_thread(A, &["m1".into()], "t:c").unwrap();
    assert_eq!(s.get_thread(A, "t:c").unwrap().unwrap().messages.len(), 1);
    assert_eq!(s.get_thread(A, "t:a").unwrap().unwrap().messages.len(), 1);
    s.set_message_thread(A, &["missing".into()], "t:z").unwrap();
    assert!(s.get_thread(A, "t:z").unwrap().is_none());
}
