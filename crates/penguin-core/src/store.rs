//! Local mail store: SQLite (WAL) + FTS5. OWNER: store/search agent.
//!
//! Contract: `Store` is `Clone + Send + Sync`; all methods are blocking and
//! must be called from `spawn_blocking` (or a dedicated thread) in async code.
//! Writes are transactional and keep the FTS index consistent with the rows.
//!
//! Layout (see `SCHEMA_V1`):
//! - `messages` holds compact metadata only; bodies live in `message_bodies`
//!   so scans and rowid lookups during ranking stay cache-friendly.
//! - A message's rowid encodes its date (`date_ms * 1024 + seq`). Date filters
//!   become rowid ranges that FTS5 applies inside its doclist walk, and a
//!   "newest first" scan is a plain reverse rowid walk.
//! - `threads` + `thread_views` are per-thread aggregates materialized on every
//!   write, so `list_threads` is one index range scan for every view and tab.
//! - `messages_fts` is a contentless FTS5 table (text is stored once, in
//!   `message_bodies`); `attachments_fts` is a small trigram index over
//!   filenames for substring matches like `filename:2041`.
//!
//! Connections: one writer behind a mutex plus a pool of read-only
//! connections, so searches never queue behind a sync batch (WAL readers see
//! the last committed snapshot).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::types::Value;
use rusqlite::{
    params, params_from_iter, Connection, OpenFlags, OptionalExtension, Transaction,
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};

use crate::text;
use crate::types::*;
use crate::{Error, Result};

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Db(format!("json: {e}"))
    }
}

// ----- message flag bits (denormalized from labels/headers/attachments) -----
pub(crate) const F_UNREAD: i64 = 1;
pub(crate) const F_STARRED: i64 = 1 << 1;
pub(crate) const F_INBOX: i64 = 1 << 2;
pub(crate) const F_SENT: i64 = 1 << 3;
pub(crate) const F_DRAFT: i64 = 1 << 4;
pub(crate) const F_TRASH: i64 = 1 << 5;
pub(crate) const F_SPAM: i64 = 1 << 6;
pub(crate) const F_IMPORTANT: i64 = 1 << 7;
/// List-Unsubscribe header or a Gmail CATEGORY_{PROMOTIONS,UPDATES,FORUMS,SOCIAL} label.
pub(crate) const F_NEWSLETTER: i64 = 1 << 8;
/// Has at least one non-inline attachment.
pub(crate) const F_ATTACH: i64 = 1 << 9;
pub(crate) const F_PDF: i64 = 1 << 10;
pub(crate) const F_IMAGE: i64 = 1 << 11;
pub(crate) const F_DOC: i64 = 1 << 12;
pub(crate) const F_SHEET: i64 = 1 << 13;
pub(crate) const F_PRES: i64 = 1 << 14;
/// Raw List-Unsubscribe presence, kept so label deltas can recompute F_NEWSLETTER.
pub(crate) const F_LIST_UNSUB: i64 = 1 << 15;
/// `Message::sender_authenticated` (DMARC/aligned-DKIM pass per Gmail's MX).
/// Rows written before this bit existed read back as false.
pub(crate) const F_AUTH: i64 = 1 << 16;
/// Only headers, labels and the snippet are stored; the body hasn't been
/// downloaded (older than the sync window) or was dropped to free space.
/// The FTS row indexes the snippet in place of the body. A full upsert
/// clears it. Must match the `messages_body_pending` partial index.
pub(crate) const F_BODY_PENDING: i64 = 1 << 17;
/// One of your account addresses is among the recipients (To/Cc/Bcc), for
/// `to:me`. Exact at insert; see store_contacts.rs for backfill and account
/// changes.
pub(crate) const F_TO_ME: i64 = 1 << 18;

/// Labels represented by flag bits; everything else goes to `message_labels`.
const FLAG_LABELS: &[(&str, i64)] = &[
    ("UNREAD", F_UNREAD),
    ("STARRED", F_STARRED),
    ("INBOX", F_INBOX),
    ("SENT", F_SENT),
    ("DRAFT", F_DRAFT),
    ("TRASH", F_TRASH),
    ("SPAM", F_SPAM),
    ("IMPORTANT", F_IMPORTANT),
];
const NEWSLETTER_CATEGORIES: &[&str] = &[
    "CATEGORY_PROMOTIONS",
    "CATEGORY_UPDATES",
    "CATEGORY_FORUMS",
    "CATEGORY_SOCIAL",
];

/// Messages sharing a millisecond get distinct rowids via this many slots.
pub(crate) const ROWID_SLOTS: i64 = 1024;

/// Attachment rowid = message rowid * ATTACHMENT_SLOTS + ordinal, so the
/// filename index is date-ordered too. Attachments past this many on one
/// message are not stored (real mail never gets close).
pub(crate) const ATTACHMENT_SLOTS: i64 = 256;

/// Max bytes of authored / quoted body text fed to the index per message.
const MAX_INDEXED_BODY: usize = 128 * 1024;
const MAX_INDEXED_QUOTED: usize = 32 * 1024;

// Synthetic view keys (Gmail label ids never start with '~').
const V_ALL: &str = "~all";
const V_DONE: &str = "~done";
const V_INBOX_IMPORTANT: &str = "~inbox:important";
const V_INBOX_OTHER: &str = "~inbox:other";
const V_INBOX_NEWS: &str = "~inbox:news";
const V_SNOOZED: &str = "~snoozed";

const SCHEMA_V1: &str = r#"
CREATE TABLE accounts(
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    display_name TEXT,
    color TEXT NOT NULL,
    added_at INTEGER NOT NULL
);
CREATE TABLE labels(
    account_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    color TEXT,
    unread_count INTEGER,
    PRIMARY KEY(account_id, id)
) WITHOUT ROWID;
CREATE TABLE threads(
    rowid INTEGER PRIMARY KEY,
    account_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    subject TEXT NOT NULL DEFAULT '',
    snippet TEXT NOT NULL DEFAULT '',
    participants TEXT NOT NULL DEFAULT '[]',
    message_count INTEGER NOT NULL DEFAULT 0,
    flags INTEGER NOT NULL DEFAULT 0,
    labels TEXT NOT NULL DEFAULT '',
    last_date INTEGER NOT NULL DEFAULT 0,
    UNIQUE(account_id, thread_id)
);
-- rowid = date_ms * 1024 + seq (see ROWID_SLOTS).
CREATE TABLE messages(
    rowid INTEGER PRIMARY KEY,
    account_id TEXT NOT NULL,
    id TEXT NOT NULL,
    thread_rowid INTEGER NOT NULL,
    date INTEGER NOT NULL,
    flags INTEGER NOT NULL,
    labels TEXT NOT NULL,
    from_name TEXT,
    from_email TEXT NOT NULL,
    subject TEXT NOT NULL,
    snippet TEXT NOT NULL,
    UNIQUE(account_id, id)
);
CREATE INDEX messages_thread ON messages(thread_rowid);
CREATE TABLE message_bodies(
    rowid INTEGER PRIMARY KEY,
    -- zstd-compressed UTF-8 (BLOB); plain TEXT rows are also accepted on read.
    body_text BLOB NOT NULL,
    body_html BLOB,
    extra TEXT NOT NULL
);
-- Non-flag labels (user labels, categories) for label: filters.
CREATE TABLE message_labels(
    label TEXT NOT NULL,
    msg INTEGER NOT NULL,
    PRIMARY KEY(label, msg)
) WITHOUT ROWID;
CREATE TABLE attachments(
    rowid INTEGER PRIMARY KEY,
    message_rowid INTEGER NOT NULL,
    ord INTEGER NOT NULL,
    att_id TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    content_id TEXT,
    inline INTEGER NOT NULL,
    kind INTEGER NOT NULL
);
CREATE INDEX attachments_msg ON attachments(message_rowid);
-- One row per (thread, view) the thread appears in; last_date is the latest
-- message of the thread within that view.
CREATE TABLE thread_views(
    thread_rowid INTEGER NOT NULL,
    view TEXT NOT NULL,
    account_id TEXT NOT NULL,
    last_date INTEGER NOT NULL,
    unread INTEGER NOT NULL,
    PRIMARY KEY(thread_rowid, view)
) WITHOUT ROWID;
CREATE INDEX thread_views_all ON thread_views(view, last_date);
CREATE INDEX thread_views_acct ON thread_views(account_id, view, last_date);
CREATE INDEX thread_views_unread ON thread_views(account_id, view) WHERE unread = 1;
-- Correspondents, for people results, from: resolution and ranking boosts.
CREATE TABLE people(
    email TEXT PRIMARY KEY,
    name TEXT,
    search_key TEXT NOT NULL,
    from_count INTEGER NOT NULL DEFAULT 0,
    sent_to_count INTEGER NOT NULL DEFAULT 0,
    last_date INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE TABLE sync_cursors(
    account_id TEXT PRIMARY KEY,
    history_id INTEGER,
    backfill_page_token TEXT,
    backfill_done INTEGER NOT NULL DEFAULT 0,
    failed_message_ids TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE message_counts(
    account_id TEXT PRIMARY KEY,
    n INTEGER NOT NULL
);
-- Contentless: the text lives in message_bodies; snippets are built in Rust.
CREATE VIRTUAL TABLE messages_fts USING fts5(
    subject, sender, recipients, cc, body, quoted, filenames,
    content='', contentless_delete=1,
    tokenize='unicode61 remove_diacritics 2',
    prefix='3'
);
CREATE VIRTUAL TABLE attachments_fts USING fts5(
    filename,
    content='', contentless_delete=1,
    tokenize='trigram'
);
"#;

/// Gmail draft id ↔ current draft message id (see store_drafts.rs).
const SCHEMA_V2: &str = r#"
CREATE TABLE drafts(
    account_id TEXT NOT NULL,
    draft_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    PRIMARY KEY(account_id, draft_id)
) WITHOUT ROWID;
CREATE INDEX drafts_by_message ON drafts(account_id, message_id);
"#;

/// User-set account nickname (Account.nickname).
const SCHEMA_V3: &str = "ALTER TABLE accounts ADD COLUMN nickname TEXT;";

/// Sync window: the windowed-backfill cursor (`SyncCursor.window`, JSON)
/// and an index over headers-only messages (`F_BODY_PENDING` = 131072).
const SCHEMA_WINDOW: &str = r#"
ALTER TABLE sync_cursors ADD COLUMN window TEXT NOT NULL DEFAULT '{}';
CREATE INDEX messages_body_pending ON messages(account_id, date) WHERE flags & 131072 != 0;
"#;

/// Outbox: send-later schedule and remind-if-no-reply (see store_outbox.rs).
const SCHEMA_OUTBOX: &str = r#"
CREATE TABLE scheduled_sends(
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    draft_id TEXT NOT NULL,
    send_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    remind_after_ms INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
CREATE INDEX scheduled_sends_due ON scheduled_sends(send_at);
CREATE INDEX scheduled_sends_draft ON scheduled_sends(account_id, draft_id);
CREATE TABLE reminders(
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    since INTEGER NOT NULL,
    remind_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX reminders_due ON reminders(remind_at);
"#;

/// Reminders remember which sent message they wait on (the `since` column
/// holds `Reminder::sent_at`).
const SCHEMA_OUTBOX_SENT_MESSAGE: &str = "ALTER TABLE reminders ADD COLUMN sent_message_id TEXT;";

/// Person card: "messages from X" without a table scan (see store_people.rs).
const SCHEMA_PEOPLE_FROM: &str =
    "CREATE INDEX messages_from ON messages(from_email COLLATE NOCASE, date);";

/// Rules & automations: event queue, idempotency claims, history, schedule
/// (see store_rules.rs).
const SCHEMA_RULES: &str = r#"
CREATE TABLE rule_queue(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    labels TEXT NOT NULL DEFAULT '',
    queued_at INTEGER NOT NULL
);
CREATE TABLE rule_applications(
    rule_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    applied_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    PRIMARY KEY(rule_id, account_id, message_id)
) WITHOUT ROWID;
CREATE INDEX rule_applications_age ON rule_applications(applied_at);
CREATE TABLE rule_log(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    rule_id TEXT NOT NULL,
    trigger TEXT NOT NULL,
    dry_run INTEGER NOT NULL,
    ok INTEGER NOT NULL,
    account_id TEXT,
    thread_id TEXT,
    message_id TEXT,
    subject TEXT,
    from_email TEXT,
    matched INTEGER NOT NULL DEFAULT 1,
    outcomes TEXT NOT NULL DEFAULT '[]',
    undone INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX rule_log_rule ON rule_log(rule_id, id);
CREATE TABLE rule_schedule(
    rule_id TEXT PRIMARY KEY,
    last_run INTEGER NOT NULL
);
"#;

/// Label.hidden: Gmail's `labelListVisibility == "labelHide"`.
const SCHEMA_LABEL_HIDDEN: &str =
    "ALTER TABLE labels ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;";

/// Detected verification code / sign-in link (`otp.rs`) as `Otp` JSON.
/// messages.otp: NULL = not scanned yet (rows from before this migration;
/// `backfill_otp` fills recent ones), '' = scanned, nothing found.
/// threads.otp: the newest live message's value.
/// Invitations get their own attachment kind (`KIND_CALENDAR`) so
/// `has:invite` and its attachment panel are an indexed kind check.
const SCHEMA_ATTACH_CALENDAR: &str = "UPDATE attachments SET kind = 6
    WHERE lower(mime_type) IN ('text/calendar', 'application/ics') OR lower(filename) LIKE '%.ics';";

const SCHEMA_OTP: &str =
    "ALTER TABLE messages ADD COLUMN otp TEXT; ALTER TABLE threads ADD COLUMN otp TEXT;";

/// The list's Unread filter: newest-first unread threads of a view, as the
/// all/per-account pair of `thread_views_all`/`_acct` restricted to unread
/// rows. The per-account one also serves the unread counts, so it replaces
/// the old (account_id, view) partial index.
const SCHEMA_UNREAD_LIST: &str = r#"
CREATE INDEX thread_views_unread_all ON thread_views(view, last_date) WHERE unread = 1;
CREATE INDEX thread_views_unread_acct ON thread_views(account_id, view, last_date) WHERE unread = 1;
DROP INDEX thread_views_unread;
"#;

/// Unread messages, for search's `is:unread` prefilter (search.rs
/// `UNREAD_SET`; the WHERE must match it exactly for SQLite to use it).
const SCHEMA_UNREAD_MESSAGES: &str =
    "CREATE INDEX messages_unread ON messages(date) WHERE flags & 1 != 0;";

/// FTS5 merges index segments once 8 of them are on one level instead of
/// 4: during a backfill every batch adds a segment, and each merge level
/// rewrites the whole index once, so fewer levels is less rewriting.
/// Searches meanwhile see a few more segments; the merge after a backfill
/// (`Store::optimize`) still ends in one. Stored in the FTS table's
/// config, so it is set once.
const SCHEMA_FTS_AUTOMERGE: &str =
    "INSERT INTO messages_fts(messages_fts, rank) VALUES('automerge', 8);";

/// The words of each person's `search_key`, so finding people by a typed
/// word or prefix is an index range instead of a `LIKE '% word%'` scan of
/// every correspondent (several per search keystroke). `people_delta`, the
/// only writer of `search_key`, keeps it current (not triggers: any trigger
/// on `people` made ingest ~40% slower, as every message upserts several
/// people); `rebuild_people` empties both. The backfill splits `search_key`
/// (folded tokens between spaces) with a recursive CTE.
const SCHEMA_PEOPLE_WORDS: &str = r#"
CREATE TABLE people_words(
    word TEXT NOT NULL,
    email TEXT NOT NULL,
    PRIMARY KEY(word, email)
) WITHOUT ROWID;
CREATE INDEX people_words_email ON people_words(email);
WITH RECURSIVE split(email, word, rest) AS (
    SELECT email, '', trim(search_key) || ' ' FROM people
    UNION ALL
    SELECT email, substr(rest, 1, instr(rest, ' ') - 1), ltrim(substr(rest, instr(rest, ' ') + 1))
    FROM split WHERE rest != ''
)
INSERT OR IGNORE INTO people_words(word, email) SELECT word, email FROM split WHERE word != '';
"#;

const MIGRATIONS: &[&str] = &[
    SCHEMA_V1,
    SCHEMA_V2,
    SCHEMA_V3,
    SCHEMA_WINDOW,
    SCHEMA_OUTBOX,
    SCHEMA_OUTBOX_SENT_MESSAGE,
    SCHEMA_PEOPLE_FROM,
    SCHEMA_RULES,
    calendar::SCHEMA_CALENDAR,
    SCHEMA_LABEL_HIDDEN,
    SCHEMA_OTP,
    SCHEMA_ATTACH_CALENDAR,
    unsubscribes::SCHEMA_UNSUBSCRIBES,
    SCHEMA_UNREAD_LIST,
    snooze::SCHEMA_SNOOZES,
    providers::SCHEMA_ACCOUNT_PROVIDER,
    providers::SCHEMA_SYNC_STATE,
    providers::SCHEMA_PROVIDER_TABLES,
    contacts::SCHEMA_FIRST_CONTACTS,
    // Already shipped: keep it before the ones added after it.
    triage::SCHEMA_FOLLOW_UP,
    SCHEMA_UNREAD_MESSAGES,
    SCHEMA_FTS_AUTOMERGE,
    invites::SCHEMA_INVITES,
    summaries::SCHEMA_SUMMARIES,
    extracted::SCHEMA_EXTRACTED,
    smart::SCHEMA_SMART_VIEWS,
    smart::SCHEMA_FILES_COVERING,
    SCHEMA_PEOPLE_WORDS,
    contacts::SCHEMA_TO_ME_PLUS,
    inline_repair::SCHEMA_INLINE_REPAIR,
];

#[path = "store_drafts.rs"]
mod drafts;

#[path = "store_checkpoint.rs"]
mod checkpoint;

#[path = "store_readonly.rs"]
mod readonly;

#[path = "store_outbox.rs"]
mod outbox;

#[path = "store_window.rs"]
mod window;
pub use window::BodyCoverage;

#[path = "store_senders.rs"]
mod senders;

#[path = "store_calendar.rs"]
mod calendar;
pub(crate) use calendar::search_events;
pub use calendar::{CalendarCounts, CalendarCursor};

#[path = "store_people.rs"]
mod people;
pub use people::{PersonAccount, PersonSummary, PersonThread};

#[path = "store_rules.rs"]
mod rules;

#[path = "store_otp.rs"]
mod otp;

#[path = "store_inline_repair.rs"]
mod inline_repair;

#[path = "store_unsubscribe.rs"]
mod unsubscribes;

#[path = "store_snooze.rs"]
mod snooze;

#[path = "store_providers.rs"]
mod providers;

#[path = "store_contacts.rs"]
mod contacts;

#[path = "store_triage.rs"]
mod triage;

#[path = "store_semantic.rs"]
mod semantic;
pub use semantic::{SemanticRow, SemanticText};

#[path = "store_summaries.rs"]
mod summaries;

#[path = "store_invites.rs"]
mod invites;

#[path = "store_receipts.rs"]
mod receipts;
pub use receipts::{ReadReceipt, ReceiptCandidate, RECEIPTS_SCHEMA};

#[path = "store_extracted.rs"]
mod extracted;

#[path = "store_smart.rs"]
mod smart;
pub(crate) use smart::{recurring_charges, Recurring};

#[path = "store_split.rs"]
mod split;
pub use extracted::ExtractProgress;
pub(crate) use extracted::{extract_now, read_fact, StoredFact, FACT_COLS};
pub use invites::{
    answerable, effective_response, is_series, shown_time, InviteCandidate, StoredInvite,
};
pub use rules::{
    ApplicationStatus, QueuedRuleEvent, RuleApplication, RuleEvent, RuleEventKind, RuleLogEntry,
    RuleStats,
};
pub use snooze::{DueSnooze, LocalWake, WAKE_LABELS};
pub use triage::is_automated_address;

/// Extra headers not needed for listing/search, stored as JSON with the body.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Extra {
    #[serde(default)]
    to: Vec<Address>,
    #[serde(default)]
    cc: Vec<Address>,
    #[serde(default)]
    bcc: Vec<Address>,
    #[serde(default)]
    reply_to: Vec<Address>,
    #[serde(default)]
    message_id_header: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default)]
    list_unsubscribe: Option<String>,
    /// None = stored before this field existed (unknown).
    #[serde(default)]
    list_unsubscribe_post: Option<bool>,
}

/// email → number of messages you sent them, keyed by write generation.
pub(crate) type SentToMap = Arc<HashMap<String, u32>>;

#[derive(Clone)]
pub struct Store {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    writer: Mutex<Connection>,
    /// Checkpoints the WAL in the background (file-backed stores).
    checkpointer: std::sync::OnceLock<Arc<checkpoint::Checkpointer>>,
    /// Threads waiting for `writer` (see `lock_writer`); index maintenance
    /// hands the writer over to them between its steps.
    writer_waiters: AtomicUsize,
    /// None for in-memory stores (reads go through the writer).
    readers: Option<ReaderPool>,
    /// Bumped on every committed write; keys read-side caches.
    pub(crate) generation: AtomicU64,
    pub(crate) sent_to_cache: Mutex<Option<(u64, SentToMap)>>,
}

struct ReaderPool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

const MAX_IDLE_READERS: usize = 4;

/// FTS5 index pages (about 4 KB each) one maintenance step merges: tens of
/// milliseconds of work, so a write queued behind a step waits about that
/// long (see `Store::optimize`).
pub(crate) const MERGE_STEP_PAGES: i64 = 256;
/// Longest a maintenance step keeps waiting for queued writers to take the
/// writer before it continues anyway.
const MAX_HANDOFF_WAIT: Duration = Duration::from_millis(500);

/// The locked writer connection. Dropping it tells the checkpointer
/// there may be new WAL frames to copy.
pub(crate) struct WriterGuard<'a> {
    conn: MutexGuard<'a, Connection>,
    inner: &'a Inner,
}

impl std::ops::Deref for WriterGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl std::ops::DerefMut for WriterGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl Drop for WriterGuard<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.inner.checkpointer.get() {
            c.written();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(c) = self.checkpointer.get() {
            c.stop();
        }
    }
}

impl Inner {
    /// The writer connection, counted in `writer_waiters` while waiting.
    fn lock_writer(&self) -> WriterGuard<'_> {
        self.writer_waiters.fetch_add(1, Ordering::AcqRel);
        let conn = lock(&self.writer);
        self.writer_waiters.fetch_sub(1, Ordering::AcqRel);
        WriterGuard { conn, inner: self }
    }

    /// Let writers queued behind a maintenance step go first. std's mutex
    /// isn't fair: a thread that unlocks and immediately locks again usually
    /// wins, so without this a waiting write could sit out every step.
    fn hand_off_writer(&self) {
        let until = Instant::now() + MAX_HANDOFF_WAIT;
        while self.writer_waiters.load(Ordering::Acquire) > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock leaves the connection usable: rusqlite
    // rolls back the open transaction when it is dropped during unwinding.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Store {
    /// Open or create the database at `path`, running migrations.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| Error::Db(format!("create {}: {e}", dir.display())))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        configure_writer(&conn)?;
        migrate(&conn)?;
        let store = Store::from_parts(
            conn,
            Some(ReaderPool {
                path: path.to_path_buf(),
                idle: Mutex::new(Vec::new()),
            }),
        );
        let checkpointer = checkpoint::Checkpointer::start(path)?;
        let _ = store.inner.checkpointer.set(checkpointer);
        Ok(store)
    }

    /// In-memory store for tests and benchmarks.
    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        configure_writer(&conn)?;
        migrate(&conn)?;
        Ok(Store::from_parts(conn, None))
    }

    fn from_parts(conn: Connection, readers: Option<ReaderPool>) -> Store {
        Store {
            inner: Arc::new(Inner {
                writer: Mutex::new(conn),
                checkpointer: std::sync::OnceLock::new(),
                writer_waiters: AtomicUsize::new(0),
                readers,
                generation: AtomicU64::new(1),
                sent_to_cache: Mutex::new(None),
            }),
        }
    }

    /// Background WAL checkpoints run so far, and whether the checkpointer
    /// has nothing left to do (file-backed stores).
    #[cfg(test)]
    pub(crate) fn checkpoint_progress(&self) -> (u64, bool) {
        self.inner
            .checkpointer
            .get()
            .map_or((0, true), |c| c.progress())
    }

    /// Run `f` on a read-only connection (a pooled reader when file-backed).
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let Some(pool) = &self.inner.readers else {
            return f(&self.inner.lock_writer());
        };
        let conn = lock(&pool.idle).pop();
        let conn = match conn {
            Some(c) => c,
            None => open_reader(&pool.path)?,
        };
        let out = f(&conn);
        let mut idle = lock(&pool.idle);
        if idle.len() < MAX_IDLE_READERS {
            idle.push(conn);
        }
        out
    }

    /// Run `f` inside an IMMEDIATE transaction on the writer connection.
    fn write<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.inner.lock_writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        self.inner.generation.fetch_add(1, Ordering::Release);
        Ok(out)
    }

    /// Merge the full-text indexes into one segment each and refresh the
    /// planner's statistics. Call after a large backfill completes; it makes
    /// later queries faster and is safe to run at any time, alongside sync
    /// and user actions: the merge runs in short steps (see `merge_fts`), so
    /// other writes wait tens of milliseconds, not the whole merge (FTS5's
    /// one-statement 'optimize' held the writer for 84 s at 300k messages).
    /// FTS5 only stops a step between terms, so the steps that reach the
    /// most common terms run long: at 300k messages 99% of writes issued
    /// during the merge waited under ~130 ms, the worst 0.8-1.4 s.
    /// Interrupted (app quit), it resumes where it stopped on the next call.
    pub fn optimize(&self) -> Result<()> {
        self.merge_fts("messages_fts", MERGE_STEP_PAGES)?;
        self.merge_fts("attachments_fts", MERGE_STEP_PAGES)?;
        self.inner.lock_writer().execute_batch("PRAGMA optimize;")?;
        Ok(())
    }

    /// FTS5's 'optimize' for `table`, in steps. Each step is FTS5's 'merge'
    /// command with a budget of `pages` pages, its own write transaction; between steps, writes queued behind it go first. The
    /// first step's budget is negative, which puts every segment on one
    /// level so the merge ends in a single segment, as 'optimize' does. The
    /// rest are positive: they continue that merge where it stopped, where
    /// a negative one would fold the half-written output back into the
    /// inputs and start over whenever a write in between added a segment.
    /// Segments written meanwhile are left to automerge. Done when a step
    /// changes nothing (fewer than two rows, per the FTS5 docs). Returns
    /// the number of steps that did work.
    pub(crate) fn merge_fts(&self, table: &str, pages: i64) -> Result<usize> {
        let step = |pages: i64| -> Result<bool> {
            let conn = self.inner.lock_writer();
            let before = conn.total_changes();
            conn.prepare_cached(&format!(
                "INSERT INTO {table}({table}, rank) VALUES('merge', ?1)"
            ))?
            .execute([pages])?;
            Ok(conn.total_changes() - before >= 2)
        };
        let mut steps = 0;
        let mut worked = step(-pages)?;
        while worked {
            steps += 1;
            self.inner.hand_off_writer();
            worked = step(pages)?;
        }
        Ok(steps)
    }

    // ----- accounts -----
    /// Insert or refresh an account after sign-in. The nickname is written
    /// only on insert: signing in again never clears it (update_account owns
    /// it). The provider and its config are replaced (a reconnect may change
    /// server settings); `capabilities` is derived, never stored.
    pub fn upsert_account(&self, account: &Account) -> Result<()> {
        let config = serde_json::to_string(&account.provider_config)?;
        self.write(|tx| {
            let before: Option<String> = tx
                .prepare_cached("SELECT email FROM accounts WHERE id = ?1")?
                .query_row([&account.id], |r| r.get(0))
                .optional()?;
            tx.execute(
                "INSERT INTO accounts(id, email, display_name, nickname, color, added_at, provider, provider_config)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET email = excluded.email, display_name = excluded.display_name,
                   color = excluded.color, provider = excluded.provider, provider_config = excluded.provider_config",
                params![
                    account.id,
                    account.email,
                    account.display_name,
                    account.nickname,
                    account.color,
                    account.added_at,
                    account.provider.as_str(),
                    config
                ],
            )?;
            // A new address (or a changed one) changes what is "to me".
            if !before.is_some_and(|e| e.eq_ignore_ascii_case(&account.email)) {
                contacts::refresh_to_me(tx, &account.email)?;
            }
            Ok(())
        })
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, email, display_name, nickname, color, added_at, provider, provider_config
                 FROM accounts ORDER BY added_at, id",
            )?;
            type Row = (Account, String, String);
            let rows = stmt.query_map([], |r| -> rusqlite::Result<Row> {
                Ok((
                    Account {
                        id: r.get(0)?,
                        email: r.get(1)?,
                        display_name: r.get(2)?,
                        nickname: r.get(3)?,
                        color: r.get(4)?,
                        added_at: r.get(5)?,
                        ..Account::default()
                    },
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (mut account, provider, config) = row?;
                account.provider = AccountProvider::parse(&provider).ok_or_else(|| {
                    Error::Db(format!(
                        "account {} has an unknown provider {provider:?} (newer build?)",
                        account.id
                    ))
                })?;
                account.provider_config = serde_json::from_str(&config)?;
                account.capabilities = account.provider.capabilities();
                out.push(account);
            }
            Ok(out)
        })
    }

    /// Change an account's nickname (`Some(None)` clears it) and/or color.
    /// Values are stored as given; callers validate them. NotFound if the
    /// account doesn't exist.
    pub fn update_account(
        &self,
        account_id: &str,
        nickname: Option<Option<&str>>,
        color: Option<&str>,
    ) -> Result<Account> {
        self.write(|tx| {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1)",
                [account_id],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(Error::NotFound(format!("account {account_id}")));
            }
            if let Some(nick) = nickname {
                tx.execute(
                    "UPDATE accounts SET nickname = ?2 WHERE id = ?1",
                    params![account_id, nick],
                )?;
            }
            if let Some(color) = color {
                tx.execute(
                    "UPDATE accounts SET color = ?2 WHERE id = ?1",
                    params![account_id, color],
                )?;
            }
            Ok(())
        })?;
        self.list_accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| Error::NotFound(format!("account {account_id}")))
    }

    /// Deletes the account and ALL of its local data.
    pub fn remove_account(&self, account_id: &str) -> Result<()> {
        self.write(|tx| {
            let email: Option<String> = tx
                .prepare_cached("SELECT email FROM accounts WHERE id = ?1")?
                .query_row([account_id], |r| r.get(0))
                .optional()?;
            let rowids: Vec<i64> = {
                let mut stmt = tx.prepare("SELECT rowid FROM messages WHERE account_id = ?1")?;
                let rows = stmt.query_map([account_id], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            {
                let mut del_fts = tx.prepare_cached("DELETE FROM messages_fts WHERE rowid = ?1")?;
                let mut del_att_fts =
                    tx.prepare_cached("DELETE FROM attachments_fts WHERE rowid = ?1")?;
                let mut atts =
                    tx.prepare_cached("SELECT rowid FROM attachments WHERE message_rowid = ?1")?;
                for &rowid in &rowids {
                    del_fts.execute([rowid])?;
                    let att_rows: Vec<i64> = atts
                        .query_map([rowid], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    for a in att_rows {
                        del_att_fts.execute([a])?;
                    }
                }
            }
            tx.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS doomed(rowid INTEGER PRIMARY KEY);
                 DELETE FROM temp.doomed;",
            )?;
            tx.execute(
                "INSERT INTO temp.doomed SELECT rowid FROM messages WHERE account_id = ?1",
                [account_id],
            )?;
            tx.execute_batch(
                "DELETE FROM attachments WHERE message_rowid IN (SELECT rowid FROM temp.doomed);
                 DELETE FROM message_labels WHERE msg IN (SELECT rowid FROM temp.doomed);
                 DELETE FROM sent_recipients WHERE msg IN (SELECT rowid FROM temp.doomed);
                 DELETE FROM message_bodies WHERE rowid IN (SELECT rowid FROM temp.doomed);
                 DELETE FROM temp.doomed;",
            )?;
            tx.execute("DELETE FROM messages WHERE account_id = ?1", [account_id])?;
            tx.execute(
                "DELETE FROM thread_views WHERE account_id = ?1",
                [account_id],
            )?;
            tx.execute("DELETE FROM threads WHERE account_id = ?1", [account_id])?;
            tx.execute("DELETE FROM labels WHERE account_id = ?1", [account_id])?;
            tx.execute(
                "DELETE FROM sync_cursors WHERE account_id = ?1",
                [account_id],
            )?;
            tx.execute(
                "DELETE FROM message_counts WHERE account_id = ?1",
                [account_id],
            )?;
            tx.execute("DELETE FROM drafts WHERE account_id = ?1", [account_id])?;
            tx.execute(
                "DELETE FROM scheduled_sends WHERE account_id = ?1",
                [account_id],
            )?;
            tx.execute("DELETE FROM reminders WHERE account_id = ?1", [account_id])?;
            calendar::remove_account(tx, account_id)?;
            rules::remove_account_rule_data(tx, account_id)?;
            unsubscribes::remove_account(tx, account_id)?;
            snooze::remove_account(tx, account_id)?;
            triage::remove_account(tx, account_id)?;
            invites::remove_account(tx, account_id)?;
            summaries::remove_account(tx, account_id)?;
            providers::remove_account(tx, account_id)?;
            tx.execute("DELETE FROM accounts WHERE id = ?1", [account_id])?;
            rebuild_people(tx)?;
            contacts::rebuild(tx)?;
            if let Some(email) = email {
                contacts::refresh_to_me(tx, &email)?;
            }
            Ok(())
        })
    }

    // ----- labels -----
    /// Replaces the full label set for the account.
    pub fn replace_labels(&self, account_id: &str, labels: &[Label]) -> Result<()> {
        self.write(|tx| {
            tx.execute("DELETE FROM labels WHERE account_id = ?1", [account_id])?;
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO labels(account_id, id, name, kind, color, unread_count, hidden) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for l in labels {
                stmt.execute(params![account_id, l.id, l.name, l.kind, l.color, l.unread_count, l.hidden])?;
            }
            Ok(())
        })
    }

    /// Writes one label's name/color/hidden (after a Gmail labels.patch).
    /// Inserts it when missing; the stored unread count is left alone.
    pub fn upsert_label(&self, label: &Label) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO labels(account_id, id, name, kind, color, unread_count, hidden) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(account_id, id) DO UPDATE SET name = excluded.name, kind = excluded.kind,
                     color = excluded.color, hidden = excluded.hidden",
                params![label.account_id, label.id, label.name, label.kind, label.color, label.unread_count, label.hidden],
            )?;
            Ok(())
        })
    }

    /// Removes a label (after a Gmail labels.delete): its row, and its id from
    /// every message and thread of the account. Returns the touched thread ids.
    pub fn delete_label(&self, account_id: &str, label_id: &str) -> Result<Vec<String>> {
        let remove = [label_id.to_string()];
        self.write(|tx| {
            tx.execute(
                "DELETE FROM labels WHERE account_id = ?1 AND id = ?2",
                params![account_id, label_id],
            )?;
            // Flag labels (system ids) never reach message_labels, so scan the
            // denormalized column for those; user labels use the index.
            let sql = if is_flag_label(label_id) {
                "SELECT rowid FROM messages WHERE account_id = ?1 AND instr(labels, ' ' || ?2 || ' ') > 0"
            } else {
                "SELECT m.rowid FROM message_labels ml JOIN messages m ON m.rowid = ml.msg
                 WHERE ml.label = ?2 AND m.account_id = ?1"
            };
            let rowids: Vec<i64> = tx
                .prepare(sql)?
                .query_map(params![account_id, label_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut touched = HashSet::new();
            for rowid in rowids {
                apply_label_delta(tx, rowid, &[], &remove, &mut touched)?;
            }
            let mut threads = Vec::with_capacity(touched.len());
            for t in touched {
                threads.push(tx.query_row("SELECT thread_id FROM threads WHERE rowid = ?1", [t], |r| r.get(0))?);
                refresh_thread(tx, t)?;
            }
            Ok(threads)
        })
    }

    /// `account_id = None` → all accounts. `unread_count` is computed locally
    /// (unread threads carrying the label), so optimistic actions show up at once.
    pub fn list_labels(&self, account_id: Option<&str>) -> Result<Vec<Label>> {
        self.list_labels_in(account_scope(account_id, None).as_deref())
    }

    /// Labels of a set of accounts (None = all; an empty set = none).
    pub fn list_labels_in(&self, accounts: Option<&[AccountId]>) -> Result<Vec<Label>> {
        if accounts.is_some_and(|a| a.is_empty()) {
            return Ok(Vec::new());
        }
        let (filter, args) = account_in("account_id", accounts);
        self.read(|c| {
            let mut unread: HashMap<(String, String), u32> = HashMap::new();
            {
                let mut stmt = c.prepare_cached(&format!(
                    "SELECT account_id, view, count(*) FROM thread_views WHERE unread = 1{filter}
                     GROUP BY account_id, view"
                ))?;
                let rows = stmt.query_map(params_from_iter(&args), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u32>(2)?,
                    ))
                })?;
                for row in rows {
                    let (a, v, n) = row?;
                    unread.insert((a, v), n);
                }
            }
            let mut stmt = c.prepare_cached(&format!(
                "SELECT account_id, id, name, kind, color, hidden FROM labels WHERE 1{filter}
                 ORDER BY account_id, kind DESC, name COLLATE NOCASE"
            ))?;
            let rows = stmt.query_map(params_from_iter(&args), |r| {
                Ok(Label {
                    account_id: r.get(0)?,
                    id: r.get(1)?,
                    name: r.get(2)?,
                    kind: r.get(3)?,
                    color: r.get(4)?,
                    unread_count: None,
                    hidden: r.get(5)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                let mut l = row?;
                l.unread_count = Some(
                    unread
                        .get(&(l.account_id.clone(), l.id.clone()))
                        .copied()
                        .unwrap_or(0),
                );
                out.push(l);
            }
            Ok(out)
        })
    }

    // ----- messages -----
    /// Insert or replace messages (and their FTS rows) in one transaction.
    pub fn upsert_messages(&self, messages: &[Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            let mut touched = HashSet::new();
            let mut added: HashMap<&str, i64> = HashMap::new();
            let order = ingest_order(messages);
            // Stored copies go first, so the batch's full-text deletes and
            // inserts each run in rowid order (see `ingest_order`) instead
            // of alternating.
            let mut replaced = Vec::with_capacity(order.len());
            for &i in &order {
                replaced.push(remove_stored(tx, &messages[i], &mut touched)?);
            }
            for (&i, prior) in order.iter().zip(replaced) {
                let m = &messages[i];
                if insert_message_after(tx, m, prior, false, &mut touched)? {
                    *added.entry(m.account_id.as_str()).or_default() += 1;
                }
            }
            for (acct, n) in added {
                bump_count(tx, acct, n)?;
            }
            for t in touched {
                refresh_thread(tx, t)?;
            }
            Ok(())
        })
    }

    pub fn delete_messages(&self, account_id: &str, message_ids: &[String]) -> Result<()> {
        if message_ids.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            let mut touched = HashSet::new();
            let mut removed = 0;
            for id in message_ids {
                let found: Option<(i64, i64)> = tx
                    .prepare_cached("SELECT rowid, thread_rowid FROM messages WHERE account_id = ?1 AND id = ?2")?
                    .query_row(params![account_id, id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                if let Some((rowid, thread)) = found {
                    purge_message(tx, rowid)?;
                    touched.insert(thread);
                    removed += 1;
                }
            }
            bump_count(tx, account_id, -removed)?;
            for t in touched {
                refresh_thread(tx, t)?;
            }
            Ok(())
        })
    }

    /// Apply label deltas to specific messages (history labelsAdded/Removed).
    pub fn modify_message_labels(
        &self,
        account_id: &str,
        message_ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        if message_ids.is_empty() || (add.is_empty() && remove.is_empty()) {
            return Ok(());
        }
        self.write(|tx| {
            let mut touched = HashSet::new();
            for id in message_ids {
                let found: Option<i64> = tx
                    .prepare_cached("SELECT rowid FROM messages WHERE account_id = ?1 AND id = ?2")?
                    .query_row(params![account_id, id], |r| r.get(0))
                    .optional()?;
                if let Some(rowid) = found {
                    apply_label_delta(tx, rowid, add, remove, &mut touched)?;
                }
            }
            for t in touched {
                refresh_thread(tx, t)?;
            }
            Ok(())
        })
    }

    /// Apply label deltas to every message in a thread (optimistic UI actions).
    pub fn modify_thread_labels(
        &self,
        account_id: &str,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            let rowids: Vec<i64> = tx
                .prepare_cached(
                    "SELECT m.rowid FROM threads t JOIN messages m ON m.thread_rowid = t.rowid
                     WHERE t.account_id = ?1 AND t.thread_id = ?2",
                )?
                .query_map(params![account_id, thread_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut touched = HashSet::new();
            for rowid in rowids {
                apply_label_delta(tx, rowid, add, remove, &mut touched)?;
            }
            for t in touched {
                refresh_thread(tx, t)?;
            }
            Ok(())
        })
    }

    /// Which of `ids` are already stored (to skip re-fetching during backfill).
    pub fn known_message_ids(&self, account_id: &str, ids: &[String]) -> Result<HashSet<String>> {
        self.read(|c| {
            let mut out = HashSet::new();
            for chunk in ids.chunks(500) {
                let placeholders = vec!["?"; chunk.len()].join(",");
                let sql = format!(
                    "SELECT id FROM messages WHERE account_id = ? AND id IN ({placeholders})"
                );
                let mut stmt = c.prepare(&sql)?;
                let args = std::iter::once(account_id).chain(chunk.iter().map(|s| s.as_str()));
                let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
                for r in rows {
                    out.insert(r?);
                }
            }
            Ok(out)
        })
    }

    pub fn get_message(&self, account_id: &str, message_id: &str) -> Result<Option<Message>> {
        self.read(|c| {
            let rowid: Option<i64> = c
                .prepare_cached("SELECT rowid FROM messages WHERE account_id = ?1 AND id = ?2")?
                .query_row(params![account_id, message_id], |r| r.get(0))
                .optional()?;
            match rowid {
                Some(r) => Ok(load_messages(c, &[r])?.pop()),
                None => Ok(None),
            }
        })
    }

    pub fn count_messages(&self, account_id: Option<&str>) -> Result<u64> {
        self.read(|c| count_messages(c, account_id))
    }

    // ----- threads -----
    /// Newest-first threads of one view. The account scope (`account_id`
    /// intersected with `account_ids`) is served from the per-account index:
    /// with several accounts, each account's newest `limit` rows are read by
    /// an index range scan and merged, so the cost is O(accounts × limit)
    /// whatever the mailbox size (a plain `account_id IN (..) ORDER BY` would
    /// sort every matching row). Only the merged top `limit` join `threads`.
    /// `unread_only` walks the same shapes over the partial unread indexes, so
    /// it costs the same however much read mail the view holds.
    pub fn list_threads(&self, query: &ListQuery) -> Result<Vec<ThreadSummary>> {
        let view = view_key(&query.view, query.tab);
        let limit = if query.limit == 0 {
            50
        } else {
            query.limit.min(1000)
        };
        let before = query.before.unwrap_or(i64::MAX);
        let scope = account_scope(query.account_id.as_deref(), query.account_ids.as_deref());
        match &query.view {
            MailboxView::Snoozed => return self.list_snoozed(scope.as_deref(), query),
            MailboxView::ReplyLater => return self.list_reply_later(scope.as_deref(), query),
            // One page holds them all (like Snoozed): `before` pages are empty.
            MailboxView::FollowUp if query.before.is_some() => return Ok(Vec::new()),
            MailboxView::FollowUp => {
                return self.list_follow_ups(
                    scope.as_deref(),
                    crate::types::FOLLOW_UP_DAYS_DEFAULT,
                    triage::now_ms(),
                    query.unread_only,
                )
            }
            MailboxView::Smart(id) => {
                return self.list_smart(
                    id,
                    scope.as_deref(),
                    query,
                    triage::now_ms(),
                    smart::local_offset_secs(),
                )
            }
            MailboxView::Query(q) => return self.list_saved_search(q, scope.as_deref(), query),
            MailboxView::Inbox => {
                if let Some(split) = &query.split {
                    return self.list_split(split, scope.as_deref(), query);
                }
            }
            _ => {}
        }
        // snoozed_until: a primary-key probe per returned row.
        const COLS: &str = "t.account_id, t.thread_id, t.subject, t.snippet, t.participants, t.message_count, t.flags, t.labels, t.otp,
            (SELECT s.wake_at FROM snoozes s WHERE s.account_id = t.account_id AND s.thread_id = t.thread_id AND s.woke_at IS NULL)";
        let mut args: Vec<Value> = vec![
            Value::Text(view),
            Value::Integer(before),
            Value::Integer(limit.into()),
        ];
        // The partial indexes only serve queries that say `unread = 1`.
        let (all_idx, acct_idx, unread) = if query.unread_only {
            (
                "thread_views_unread_all",
                "thread_views_unread_acct",
                " AND unread = 1",
            )
        } else {
            ("thread_views_all", "thread_views_acct", "")
        };
        let sql = match scope.as_deref() {
            None => format!(
                "SELECT {COLS}, v.last_date
                 FROM thread_views v INDEXED BY {all_idx} JOIN threads t ON t.rowid = v.thread_rowid
                 WHERE v.view = ?1 AND v.last_date < ?2{unread}
                 ORDER BY v.last_date DESC LIMIT ?3"
            ),
            Some([]) => return Ok(Vec::new()),
            Some([one]) => {
                args.push(Value::Text(one.clone()));
                format!(
                    "SELECT {COLS}, v.last_date
                     FROM thread_views v INDEXED BY {acct_idx} JOIN threads t ON t.rowid = v.thread_rowid
                     WHERE v.account_id = ?4 AND v.view = ?1 AND v.last_date < ?2{unread}
                     ORDER BY v.last_date DESC LIMIT ?3"
                )
            }
            Some(many) => {
                let parts: Vec<String> = (0..many.len())
                    .map(|i| {
                        format!(
                            "SELECT * FROM (SELECT thread_rowid, last_date FROM thread_views INDEXED BY {acct_idx}
                             WHERE account_id = ?{} AND view = ?1 AND last_date < ?2{unread} ORDER BY last_date DESC LIMIT ?3)",
                            i + 4
                        )
                    })
                    .collect();
                args.extend(many.iter().map(|a| Value::Text(a.clone())));
                format!(
                    "WITH top(thread_rowid, last_date) AS ({})
                     SELECT {COLS}, top.last_date FROM top JOIN threads t ON t.rowid = top.thread_rowid
                     ORDER BY top.last_date DESC LIMIT ?3",
                    parts.join(" UNION ALL ")
                )
            }
        };
        self.read(|c| {
            let mut stmt = c.prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(&args), summary_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// `MailboxView::Snoozed`: see `snooze::snoozed_list_sql`.
    fn list_snoozed(
        &self,
        scope: Option<&[AccountId]>,
        query: &ListQuery,
    ) -> Result<Vec<ThreadSummary>> {
        if query.before.is_some() || scope.is_some_and(|s| s.is_empty()) {
            return Ok(Vec::new());
        }
        let sql = snooze::snoozed_list_sql(scope, query.unread_only);
        let mut args: Vec<Value> = vec![Value::Integer(1000)];
        args.extend(scope.unwrap_or(&[]).iter().map(|a| Value::Text(a.clone())));
        self.read(|c| {
            let mut stmt = c.prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(&args), summary_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn get_thread(&self, account_id: &str, thread_id: &str) -> Result<Option<ThreadDetail>> {
        self.read(|c| {
            let row: Option<(i64, String, String)> = c
                .prepare_cached("SELECT rowid, subject, labels FROM threads WHERE account_id = ?1 AND thread_id = ?2")?
                .query_row(params![account_id, thread_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            let Some((trow, subject, labels)) = row else { return Ok(None) };
            let rowids: Vec<i64> = c
                .prepare_cached("SELECT rowid FROM messages WHERE thread_rowid = ?1 ORDER BY date, rowid")?
                .query_map([trow], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if rowids.is_empty() {
                return Ok(None);
            }
            let messages = load_messages(c, &rowids)?;
            Ok(Some(ThreadDetail {
                account_id: account_id.to_string(),
                thread_id: thread_id.to_string(),
                subject,
                label_ids: split_labels(&labels),
                messages,
            }))
        })
    }

    // ----- search -----
    /// Keyword search (no search by meaning): the response says `semantic: off`.
    pub fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        crate::search::search(self, request, None, &crate::HybridParams::default())
    }

    /// Search with the meaning index when there is one: hybrid ranking where
    /// the query calls for it (docs/SEARCH-RANKING.md), else exactly
    /// [`Store::search`]. `semantic` None, or an index still being built,
    /// is keyword search, and the response's `semantic` says which.
    pub fn search_hybrid(
        &self,
        request: &SearchRequest,
        semantic: Option<&crate::SemanticHandles>,
    ) -> Result<SearchResponse> {
        crate::search::search(self, request, semantic, &crate::HybridParams::default())
    }

    /// The query hybrid search runs for `raw`: dates in words taken out as a
    /// soft window, misspelled words respelled (`hybrid::rewrite`). For
    /// evaluation tools that must embed exactly what search embeds.
    #[doc(hidden)]
    pub fn hybrid_rewrite(&self, raw: &str, now_ms: i64) -> Result<crate::hybrid::Rewrite> {
        self.read(|c| crate::hybrid::rewrite(c, raw, now_ms))
    }

    /// [`Store::search_hybrid`] with explicit ranking parameters (evaluation
    /// and tuning; the app uses the defaults).
    #[doc(hidden)]
    pub fn search_hybrid_with(
        &self,
        request: &SearchRequest,
        semantic: Option<&crate::SemanticHandles>,
        params: &crate::HybridParams,
    ) -> Result<SearchResponse> {
        crate::search::search(self, request, semantic, params)
    }

    // ----- sync cursor -----
    /// The account's sync position; the default (nothing recorded) when it
    /// has none yet. `provider_state` comes back exactly as the provider
    /// saved it.
    pub fn get_sync_cursor(&self, account_id: &str) -> Result<SyncCursor> {
        self.read(|c| {
            type Row = (String, bool, String, String);
            let row: Option<Row> = c
                .prepare_cached(
                    "SELECT provider_state, backfill_done, failed_message_ids, window FROM sync_cursors
                     WHERE account_id = ?1",
                )?
                .query_row([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .optional()?;
            let Some((provider_state, backfill_done, failed, window)) = row else {
                return Ok(SyncCursor::default());
            };
            Ok(SyncCursor {
                provider_state,
                backfill_done,
                failed_message_ids: serde_json::from_str(&failed)?,
                window: serde_json::from_str(&window)?,
            })
        })
    }

    pub fn set_sync_cursor(&self, account_id: &str, cursor: &SyncCursor) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO sync_cursors(account_id, provider_state, backfill_done, failed_message_ids, window)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(account_id) DO UPDATE SET provider_state = excluded.provider_state,
                   backfill_done = excluded.backfill_done,
                   failed_message_ids = excluded.failed_message_ids, window = excluded.window",
                params![
                    account_id,
                    cursor.provider_state,
                    cursor.backfill_done,
                    serde_json::to_string(&cursor.failed_message_ids)?,
                    serde_json::to_string(&cursor.window)?
                ],
            )?;
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// connection setup

fn configure_writer(conn: &Connection) -> Result<()> {
    #[cfg(feature = "sql-profile")]
    crate::sqlprof::install(conn);
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -65536;
         PRAGMA mmap_size = 1073741824;
         PRAGMA foreign_keys = OFF;
         PRAGMA busy_timeout = 5000;",
    )?;
    // The WAL is checkpointed in the background; the writer's own
    // checkpoint is only a backstop (store_checkpoint.rs). A restarted WAL
    // file is cut back to the same 64 MiB.
    let backstop = checkpoint::WAL_BACKSTOP_PAGES;
    conn.pragma_update(None, "wal_autocheckpoint", backstop)?;
    conn.pragma_update(None, "journal_size_limit", backstop * 4096)?;
    Ok(())
}

/// Readers map up to 1 GiB of the file. Measured on the Linux benchmark
/// VM: the map makes a cold launch slow there, because a page fault on a
/// file that isn't in the OS cache reads the device's whole readahead
/// window around the page, and that VM's disk reads ahead 8 MiB (334 MiB
/// read for the first Inbox page of 100k messages, ~320 ms; with pread,
/// 15 ms). But without the map every page SQLite's 32 MiB cache doesn't
/// hold costs a syscall, and searches that read many message rows got
/// twice as slow at 300k messages (`account:personal invoice` 53 -> 113
/// ms), every time rather than once per boot. With a typical 128 KiB
/// readahead a cold first page reads at most ~10 MiB through the map, so it
/// stays. macOS clusters page-ins differently; measure a cold start there
/// with `sudo purge` before changing this.
fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    #[cfg(feature = "sql-profile")]
    crate::sqlprof::install(&conn);
    conn.execute_batch(
        "PRAGMA cache_size = -32768;
         PRAGMA mmap_size = 1073741824;
         PRAGMA temp_store = MEMORY;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let version = version as usize;
    if version > MIGRATIONS.len() {
        return Err(Error::Db(format!(
            "database schema v{version} is newer than this build (v{})",
            MIGRATIONS.len()
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// write helpers

pub(crate) fn label_flags(labels: &[String]) -> i64 {
    let mut f = 0;
    for l in labels {
        if let Some((_, bit)) = FLAG_LABELS.iter().find(|(name, _)| name == l) {
            f |= bit;
        }
    }
    let news = labels
        .iter()
        .any(|l| NEWSLETTER_CATEGORIES.contains(&l.as_str()));
    if news {
        f |= F_NEWSLETTER;
    }
    f
}

fn is_flag_label(l: &str) -> bool {
    FLAG_LABELS.iter().any(|(name, _)| *name == l)
}

/// Attachment category: 1 pdf, 2 image, 3 doc, 4 spreadsheet, 5 presentation, 0 other.
/// `attachments.kind` of an invitation (.ics / text/calendar); no flag bit.
pub(crate) const KIND_CALENDAR: i64 = 6;

pub(crate) fn attachment_kind(mime: &str, filename: &str) -> i64 {
    let mime = mime.to_ascii_lowercase();
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if mime == "text/calendar" || mime == "application/ics" || ext == "ics" {
        KIND_CALENDAR
    } else if mime == "application/pdf" || ext == "pdf" {
        1
    } else if mime.starts_with("image/")
        || matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp" | "bmp" | "tif" | "tiff" | "svg"
        )
    {
        2
    } else if mime.contains("spreadsheet")
        || mime.contains("excel")
        || mime == "text/csv"
        || matches!(
            ext.as_str(),
            "xls" | "xlsx" | "xlsm" | "csv" | "ods" | "numbers" | "tsv"
        )
    {
        4
    } else if mime.contains("presentation")
        || mime.contains("powerpoint")
        || matches!(ext.as_str(), "ppt" | "pptx" | "key" | "odp")
    {
        5
    } else if mime.contains("msword")
        || mime.contains("wordprocessing")
        || mime.contains("opendocument.text")
        || mime == "application/rtf"
        || matches!(
            ext.as_str(),
            "doc" | "docx" | "odt" | "rtf" | "pages" | "txt" | "md"
        )
    {
        3
    } else {
        0
    }
}

pub(crate) fn kind_flag(kind: i64) -> i64 {
    match kind {
        1 => F_PDF,
        2 => F_IMAGE,
        3 => F_DOC,
        4 => F_SHEET,
        5 => F_PRES,
        _ => 0,
    }
}

fn join_labels(labels: &[String]) -> String {
    // Space-delimited with sentinels so `instr(labels, ' X ')` is exact.
    let mut s = String::from(" ");
    for l in labels {
        s.push_str(l);
        s.push(' ');
    }
    s
}

pub(crate) fn split_labels(s: &str) -> Vec<String> {
    s.split(' ')
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

fn format_addr(a: &Address, out: &mut String) {
    if let Some(n) = &a.name {
        out.push_str(n);
        out.push(' ');
    }
    out.push_str(&a.email);
    out.push_str(", ");
}

fn truncate_at(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// How many following milliseconds a full millisecond may spill into.
const ROWID_SPILL_LIMIT: i64 = 1_000_000;
/// Latest date whose rowid (and its attachments' rowids, `* ATTACHMENT_SLOTS`)
/// fits in an i64 with room to spill: ~year 3084.
const MAX_ROWID_DATE: i64 = i64::MAX / ATTACHMENT_SLOTS / ROWID_SLOTS - ROWID_SPILL_LIMIT - 1;

/// Allocate a date-ordered rowid (`date * 1024 + next slot`). The rowid only
/// orders rows; the true date lives in the `date` column. Out-of-range dates
/// (negative, or absurd future internalDates) are clamped so the arithmetic
/// can't overflow. When a millisecond's 1024 slots are taken (bulk imports
/// stamp thousands of messages with one internalDate) the message spills
/// into the next free millisecond, so ordering stays within a few ms of true.
fn alloc_rowid(tx: &Connection, date: i64) -> Result<i64> {
    let first = date.clamp(0, MAX_ROWID_DATE);
    let mut stmt =
        tx.prepare_cached("SELECT max(rowid) FROM messages WHERE rowid >= ?1 AND rowid < ?2")?;
    for bucket in first..=first + ROWID_SPILL_LIMIT {
        let base = bucket * ROWID_SLOTS;
        let max: Option<i64> = stmt.query_row(params![base, base + ROWID_SLOTS], |r| r.get(0))?;
        match max {
            None => return Ok(base),
            Some(m) if m + 1 < base + ROWID_SLOTS => return Ok(m + 1),
            Some(_) => continue,
        }
    }
    Err(Error::Db(format!(
        "no free rowid within {ROWID_SPILL_LIMIT} ms of timestamp {date}"
    )))
}

fn ensure_thread(tx: &Connection, account_id: &str, thread_id: &str) -> Result<i64> {
    let existing: Option<i64> = tx
        .prepare_cached("SELECT rowid FROM threads WHERE account_id = ?1 AND thread_id = ?2")?
        .query_row(params![account_id, thread_id], |r| r.get(0))
        .optional()?;
    if let Some(r) = existing {
        return Ok(r);
    }
    tx.prepare_cached("INSERT INTO threads(account_id, thread_id) VALUES (?1, ?2)")?
        .execute(params![account_id, thread_id])?;
    Ok(tx.last_insert_rowid())
}

/// The order a batch of messages is written in: oldest first, unless the
/// batch names a message twice (then as given, so the last copy wins).
///
/// Rowids follow dates, and FTS5 starts a new index segment whenever a row
/// arrives with a lower rowid than the one before it in the transaction.
/// Written as they arrive (newest first from Gmail, any order from IMAP or
/// a backfill's pages), a 100-message batch left dozens of one-message
/// segments, each flushed and later merged again: most of ingest's cost.
/// In rowid order a batch is one segment.
fn ingest_order(messages: &[Message]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..messages.len()).collect();
    let mut seen = HashSet::with_capacity(messages.len());
    if messages
        .iter()
        .all(|m| seen.insert((m.account_id.as_str(), m.id.as_str())))
    {
        // Stable: messages of the same millisecond keep their order.
        order.sort_by_key(|&i| messages[i].date);
    }
    order
}

/// A stored copy of a message, removed so a new copy can take its place.
struct Replaced {
    rowid: i64,
    date: i64,
    /// Its attachments, for [`keep_attachment_id`].
    attachments: Vec<AttachmentMeta>,
}

/// Remove the stored copy of `m`, if any (see [`purge_message`]).
fn remove_stored(
    tx: &Connection,
    m: &Message,
    touched: &mut HashSet<i64>,
) -> Result<Option<Replaced>> {
    let existing: Option<(i64, i64, i64)> = tx
        .prepare_cached(
            "SELECT rowid, thread_rowid, date FROM messages WHERE account_id = ?1 AND id = ?2",
        )?
        .query_row(params![m.account_id, m.id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let Some((rowid, thread, date)) = existing else {
        return Ok(None);
    };
    let attachments = stored_attachment_ids(tx, rowid)?;
    purge_message(tx, rowid)?;
    touched.insert(thread);
    Ok(Some(Replaced {
        rowid,
        date,
        attachments,
    }))
}

/// Insert (or replace) one message. Returns true when it was new.
/// `body_pending`: `m` carries headers only (see F_BODY_PENDING).
fn insert_message(
    tx: &Connection,
    m: &Message,
    body_pending: bool,
    touched: &mut HashSet<i64>,
) -> Result<bool> {
    let prior = remove_stored(tx, m, touched)?;
    insert_message_after(tx, m, prior, body_pending, touched)
}

/// [`insert_message`] when the caller already removed the stored copy
/// (`prior`). With `prior` None it still checks, since a batch may name
/// the same message twice.
fn insert_message_after(
    tx: &Connection,
    m: &Message,
    prior: Option<Replaced>,
    body_pending: bool,
    touched: &mut HashSet<i64>,
) -> Result<bool> {
    let prior = match prior {
        Some(p) => Some(p),
        None => remove_stored(tx, m, touched)?,
    };
    let is_new = prior.is_none();
    let (existing, mut kept_ids) = match prior {
        Some(p) => (Some((p.rowid, p.date)), p.attachments),
        None => (None, Vec::new()),
    };
    let thread_rowid = ensure_thread(tx, &m.account_id, &m.thread_id)?;
    touched.insert(thread_rowid);
    let rowid = match existing {
        Some((rowid, date)) if date == m.date => rowid,
        _ => alloc_rowid(tx, m.date)?,
    };

    let mut flags = label_flags(&m.label_ids);
    if contacts::addressed_to_me(tx, m.to.iter().chain(&m.cc).chain(&m.bcc))? {
        flags |= F_TO_ME;
    }
    if m.list_unsubscribe.is_some() {
        flags |= F_LIST_UNSUB | F_NEWSLETTER;
    }
    if m.sender_authenticated {
        flags |= F_AUTH;
    }
    if body_pending {
        flags |= F_BODY_PENDING;
    }
    let mut filenames = String::new();
    let mut att_bytes: i64 = 0;
    for a in &m.attachments {
        if a.inline {
            continue;
        }
        att_bytes = att_bytes.saturating_add(a.size.min(i64::MAX as u64) as i64);
        flags |= F_ATTACH | kind_flag(attachment_kind(&a.mime_type, &a.filename));
        filenames.push_str(&a.filename);
        filenames.push(' ');
    }
    let (authored, quoted) = if body_pending {
        (m.snippet.clone(), String::new())
    } else {
        text::split_quoted(&m.body_text)
    };
    // Only recent mail can hold a usable code; older rows stay NULL
    // ("unscanned") and backfill_otp covers them on demand.
    let otp = if m.date >= otp_ingest_cutoff() {
        Some(otp_column(
            crate::otp::detect_authored(m, &authored).as_ref(),
        )?)
    } else {
        None
    };
    tx.prepare_cached(
        "INSERT INTO messages(rowid, account_id, id, thread_rowid, date, flags, labels, from_name, from_email, subject, snippet, otp, att_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?
    .execute(params![
        rowid,
        m.account_id,
        m.id,
        thread_rowid,
        m.date,
        flags,
        join_labels(&m.label_ids),
        m.from.name,
        m.from.email,
        m.subject,
        m.snippet,
        otp,
        att_bytes
    ])?;
    let extra = Extra {
        to: m.to.clone(),
        cc: m.cc.clone(),
        bcc: m.bcc.clone(),
        reply_to: m.reply_to.clone(),
        message_id_header: m.message_id_header.clone(),
        in_reply_to: m.in_reply_to.clone(),
        references: m.references.clone(),
        list_unsubscribe: m.list_unsubscribe.clone(),
        list_unsubscribe_post: m.list_unsubscribe_post,
    };
    tx.prepare_cached(
        "INSERT INTO message_bodies(rowid, body_text, body_html, extra) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        rowid,
        compress_body(&m.body_text)?,
        m.body_html.as_deref().map(compress_body).transpose()?,
        serde_json::to_string(&extra)?
    ])?;

    {
        let mut ins = tx.prepare_cached(
            "INSERT INTO attachments(rowid, message_rowid, ord, att_id, filename, mime_type, size, content_id, inline, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        let mut ins_fts =
            tx.prepare_cached("INSERT INTO attachments_fts(rowid, filename) VALUES (?1, ?2)")?;
        if m.attachments.len() > ATTACHMENT_SLOTS as usize {
            tracing::warn!(
                count = m.attachments.len(),
                "message has too many attachments; storing the first {ATTACHMENT_SLOTS}"
            );
        }
        for (i, a) in m
            .attachments
            .iter()
            .enumerate()
            .take(ATTACHMENT_SLOTS as usize)
        {
            let att_rowid = rowid * ATTACHMENT_SLOTS + i as i64;
            let id = keep_attachment_id(&mut kept_ids, a).unwrap_or_else(|| a.id.clone());
            ins.execute(params![
                att_rowid,
                rowid,
                i as i64,
                id,
                a.filename,
                a.mime_type,
                a.size as i64,
                a.content_id,
                a.inline,
                attachment_kind(&a.mime_type, &a.filename)
            ])?;
            if !a.inline {
                ins_fts.execute(params![att_rowid, a.filename])?;
            }
        }
    }
    {
        let mut ins =
            tx.prepare_cached("INSERT OR IGNORE INTO message_labels(label, msg) VALUES (?1, ?2)")?;
        for l in m.label_ids.iter().filter(|l| !is_flag_label(l)) {
            ins.execute(params![l, rowid])?;
        }
    }

    // Full-text row.
    let mut sender = String::new();
    format_addr(&m.from, &mut sender);
    let mut recipients = String::new();
    for a in m.to.iter().chain(&m.bcc) {
        format_addr(a, &mut recipients);
    }
    let mut cc = String::new();
    for a in &m.cc {
        format_addr(a, &mut cc);
    }
    tx.prepare_cached(
        "INSERT INTO messages_fts(rowid, subject, sender, recipients, cc, body, quoted, filenames) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?
    .execute(params![
        rowid,
        m.subject,
        sender,
        recipients,
        cc,
        truncate_at(&authored, MAX_INDEXED_BODY),
        truncate_at(&quoted, MAX_INDEXED_QUOTED),
        filenames
    ])?;

    people_delta(
        tx,
        &m.from,
        m.to.iter().chain(&m.cc).chain(&m.bcc),
        flags & F_SENT != 0,
        m.date,
        1,
    )?;
    contacts::add(
        tx,
        rowid,
        flags,
        &m.from.email,
        m.to.iter().chain(&m.cc).chain(&m.bcc),
    )?;
    Ok(is_new)
}

/// Attachment prefix for parts Gmail inlined in the message (see
/// penguin-gmail's `PART_ID_PREFIX`): addressed by partId, already stable.
const PART_ID_PREFIX: &str = "part:";

/// A stored message's attachments, as candidates for [`keep_attachment_id`].
fn stored_attachment_ids(tx: &Connection, rowid: i64) -> Result<Vec<AttachmentMeta>> {
    Ok(tx
        .prepare_cached(
            "SELECT att_id, filename, mime_type, size, content_id, inline FROM attachments
             WHERE message_rowid = ?1 ORDER BY ord",
        )?
        .query_map([rowid], |r| {
            Ok(AttachmentMeta {
                id: r.get(0)?,
                filename: r.get(1)?,
                mime_type: r.get(2)?,
                size: r.get::<_, i64>(3)? as u64,
                content_id: r.get(4)?,
                inline: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

/// The id already stored for the same attachment of the same message, if
/// any (consumed so two identical files keep two ids).
///
/// Gmail issues a new attachmentId for every part on every messages.get,
/// and the earlier ids stay valid. Re-storing a message (its body fetched on
/// open, a window fill, a retry) would otherwise change every attachment's
/// id under whoever still holds the old one: an open thread or preview,
/// search results, the person card, the attachment cache. Their
/// save/preview then found no such attachment. A Gmail message is
/// immutable, so the same name, type, size and Content-ID within one
/// message id is the same part. (If Gmail ever rejects a kept id,
/// penguin-gmail's `get_attachment` re-matches it against a fresh fetch.)
fn keep_attachment_id(kept: &mut Vec<AttachmentMeta>, a: &AttachmentMeta) -> Option<String> {
    if a.id.starts_with(PART_ID_PREFIX) {
        return None;
    }
    let at = kept.iter().position(|k| {
        !k.id.starts_with(PART_ID_PREFIX)
            && k.filename == a.filename
            && k.mime_type == a.mime_type
            && k.size == a.size
            && k.content_id == a.content_id
            && k.inline == a.inline
    })?;
    Some(kept.remove(at).id)
}

/// Remove a message and everything derived from it (FTS, attachments,
/// labels, people counts). The caller refreshes the thread.
fn purge_message(tx: &Connection, rowid: i64) -> Result<()> {
    let row: Option<(i64, Option<String>, String, i64, String)> = tx
        .prepare_cached(
            "SELECT m.flags, m.from_name, m.from_email, m.date, b.extra FROM messages m
             JOIN message_bodies b ON b.rowid = m.rowid WHERE m.rowid = ?1",
        )?
        .query_row([rowid], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .optional()?;
    if let Some((flags, from_name, from_email, date, extra)) = row {
        let extra: Extra = serde_json::from_str(&extra).unwrap_or_default();
        let from = Address {
            name: from_name,
            email: from_email,
        };
        people_delta(
            tx,
            &from,
            extra.to.iter().chain(&extra.cc).chain(&extra.bcc),
            flags & F_SENT != 0,
            date,
            -1,
        )?;
        contacts::remove(
            tx,
            rowid,
            flags,
            &from.email,
            extra.to.iter().chain(&extra.cc).chain(&extra.bcc),
        )?;
    }
    tx.prepare_cached("DELETE FROM messages_fts WHERE rowid = ?1")?
        .execute([rowid])?;
    let att_rows: Vec<i64> = tx
        .prepare_cached("SELECT rowid FROM attachments WHERE message_rowid = ?1")?
        .query_map([rowid], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for a in att_rows {
        tx.prepare_cached("DELETE FROM attachments_fts WHERE rowid = ?1")?
            .execute([a])?;
    }
    tx.prepare_cached("DELETE FROM attachments WHERE message_rowid = ?1")?
        .execute([rowid])?;
    tx.prepare_cached("DELETE FROM message_labels WHERE msg = ?1")?
        .execute([rowid])?;
    tx.prepare_cached("DELETE FROM message_bodies WHERE rowid = ?1")?
        .execute([rowid])?;
    tx.prepare_cached("DELETE FROM messages WHERE rowid = ?1")?
        .execute([rowid])?;
    Ok(())
}

fn apply_label_delta(
    tx: &Connection,
    rowid: i64,
    add: &[String],
    remove: &[String],
    touched: &mut HashSet<i64>,
) -> Result<()> {
    let (thread, flags, labels): (i64, i64, String) = tx
        .prepare_cached("SELECT thread_rowid, flags, labels FROM messages WHERE rowid = ?1")?
        .query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let mut set = split_labels(&labels);
    set.retain(|l| !remove.contains(l));
    for a in add {
        if !set.contains(a) {
            set.push(a.clone());
        }
    }
    let keep = flags
        & (F_ATTACH
            | F_PDF
            | F_IMAGE
            | F_DOC
            | F_SHEET
            | F_PRES
            | F_LIST_UNSUB
            | F_AUTH
            | F_BODY_PENDING
            | F_TO_ME);
    let mut new_flags = label_flags(&set) | keep;
    if flags & F_LIST_UNSUB != 0 {
        new_flags |= F_NEWSLETTER;
    }
    tx.prepare_cached("UPDATE messages SET flags = ?1, labels = ?2 WHERE rowid = ?3")?
        .execute(params![new_flags, join_labels(&set), rowid])?;
    for l in remove.iter().filter(|l| !is_flag_label(l)) {
        tx.prepare_cached("DELETE FROM message_labels WHERE label = ?1 AND msg = ?2")?
            .execute(params![l, rowid])?;
    }
    for l in add.iter().filter(|l| !is_flag_label(l)) {
        tx.prepare_cached("INSERT OR IGNORE INTO message_labels(label, msg) VALUES (?1, ?2)")?
            .execute(params![l, rowid])?;
    }
    // A message entering/leaving SENT moves its recipients' "you write to
    // them" count; SENT, DRAFT and SPAM decide whether it is a first contact.
    if (flags ^ new_flags) & (F_SENT | F_DRAFT | F_SPAM) != 0 {
        let (from_name, from_email, date, extra): (Option<String>, String, i64, String) = tx
            .prepare_cached(
                "SELECT m.from_name, m.from_email, m.date, b.extra FROM messages m JOIN message_bodies b ON b.rowid = m.rowid WHERE m.rowid = ?1",
            )?
            .query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        let extra: Extra = serde_json::from_str(&extra).unwrap_or_default();
        let from = Address {
            name: from_name,
            email: from_email,
        };
        let recips = || extra.to.iter().chain(&extra.cc).chain(&extra.bcc);
        if (flags ^ new_flags) & F_SENT != 0 {
            people_delta(tx, &from, recips(), flags & F_SENT != 0, date, -1)?;
            people_delta(tx, &from, recips(), new_flags & F_SENT != 0, date, 1)?;
        }
        contacts::remove(tx, rowid, flags, &from.email, recips())?;
        contacts::add(tx, rowid, new_flags, &from.email, recips())?;
    }
    touched.insert(thread);
    Ok(())
}

pub(crate) fn person_key(name: Option<&str>, email: &str) -> String {
    let mut key = String::from(" ");
    for (_, _, tok) in text::tokens(name.unwrap_or(""))
        .into_iter()
        .chain(text::tokens(email))
    {
        key.push_str(&tok);
        key.push(' ');
    }
    key
}

/// The `people_words` range for a typed token: the word itself, or with
/// `prefix` every word that starts with it (what `search_key LIKE '% tok%'`
/// matched, as an index range). Tokens are folded like the stored words.
pub(crate) fn word_range(tok: &str, prefix: bool) -> [rusqlite::types::Value; 2] {
    let hi = if prefix {
        format!("{tok}{}", char::MAX)
    } else {
        tok.to_string()
    };
    [
        rusqlite::types::Value::Text(tok.to_string()),
        rusqlite::types::Value::Text(hi),
    ]
}

/// People with a word in the range `?{n}`..`?{n+1}` (see `word_range`).
pub(crate) fn has_word(n: usize) -> String {
    format!(
        "email IN (SELECT email FROM people_words WHERE word BETWEEN ?{} AND ?{})",
        n,
        n + 1
    )
}

/// Adjust people counts for one message: its sender (received mail) or its
/// recipients (mail you sent).
fn people_delta<'a>(
    tx: &Connection,
    from: &Address,
    recipients: impl Iterator<Item = &'a Address>,
    sent: bool,
    date: i64,
    sign: i64,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO people(email, name, search_key, from_count, sent_to_count, last_date) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(email) DO UPDATE SET
           from_count = from_count + excluded.from_count,
           sent_to_count = sent_to_count + excluded.sent_to_count,
           name = CASE WHEN excluded.name IS NOT NULL AND excluded.last_date >= last_date THEN excluded.name ELSE name END,
           search_key = CASE WHEN excluded.name IS NOT NULL AND excluded.last_date >= last_date THEN excluded.search_key ELSE search_key END,
           last_date = max(last_date, excluded.last_date)",
    )?;
    let mut prev =
        tx.prepare_cached("SELECT search_key, last_date FROM people WHERE email = ?1")?;
    let mut upsert = |a: &Address, from_n: i64, sent_n: i64| -> Result<()> {
        let email = a.email.trim().to_lowercase();
        if email.is_empty() {
            return Ok(());
        }
        let name = if sign > 0 {
            a.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
        } else {
            None
        };
        let date = if sign > 0 { date } else { 0 };
        let key = person_key(name, &email);
        let before: Option<(String, i64)> = prev
            .query_row([&email], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        stmt.execute(params![email, name, key, from_n, sent_n, date])?;
        // The upsert's CASE for search_key, mirrored: a new person, or a
        // name at least as recent as the stored one that changes the key.
        match before {
            None => set_person_words(tx, &email, None, &key)?,
            Some((old, last)) if name.is_some() && date >= last && old != key => {
                set_person_words(tx, &email, Some(&old), &key)?
            }
            Some(_) => {}
        }
        Ok(())
    };
    if sent {
        let mut seen = HashSet::new();
        for r in recipients {
            if seen.insert(r.email.to_lowercase()) {
                upsert(r, 0, sign)?;
            }
        }
    } else {
        upsert(from, sign, 0)?;
    }
    Ok(())
}

/// `people_words` for a person whose `search_key` became `key` (from
/// `old`, or a new person).
fn set_person_words(tx: &Connection, email: &str, old: Option<&str>, key: &str) -> Result<()> {
    if old.is_some() {
        tx.prepare_cached("DELETE FROM people_words WHERE email = ?1")?
            .execute([email])?;
    }
    let mut ins =
        tx.prepare_cached("INSERT OR IGNORE INTO people_words(word, email) VALUES (?1, ?2)")?;
    for word in key.split(' ').filter(|w| !w.is_empty()) {
        ins.execute([word, email])?;
    }
    Ok(())
}

fn rebuild_people(tx: &Connection) -> Result<()> {
    tx.execute("DELETE FROM people", [])?;
    tx.execute("DELETE FROM people_words", [])?;
    let mut stmt = tx.prepare(
        "SELECT m.flags, m.from_name, m.from_email, m.date, b.extra FROM messages m JOIN message_bodies b ON b.rowid = m.rowid",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let flags: i64 = r.get(0)?;
        let from = Address {
            name: r.get(1)?,
            email: r.get(2)?,
        };
        let date: i64 = r.get(3)?;
        let extra: Extra = serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default();
        people_delta(
            tx,
            &from,
            extra.to.iter().chain(&extra.cc).chain(&extra.bcc),
            flags & F_SENT != 0,
            date,
            1,
        )?;
    }
    Ok(())
}

fn bump_count(tx: &Connection, account_id: &str, n: i64) -> Result<()> {
    if n == 0 {
        return Ok(());
    }
    tx.prepare_cached(
        "INSERT INTO message_counts(account_id, n) VALUES (?1, ?2) ON CONFLICT(account_id) DO UPDATE SET n = n + excluded.n",
    )?
    .execute(params![account_id, n])?;
    Ok(())
}

/// ` AND <col> IN (?,..)` plus its bound values; empty for "all accounts".
fn account_in(col: &str, accounts: Option<&[AccountId]>) -> (String, Vec<Value>) {
    match accounts {
        None => (String::new(), Vec::new()),
        Some(ids) => (
            format!(" AND {col} IN ({})", vec!["?"; ids.len()].join(",")),
            ids.iter().map(|a| Value::Text(a.clone())).collect(),
        ),
    }
}

pub(crate) fn count_messages(c: &Connection, account_id: Option<&str>) -> Result<u64> {
    let n: i64 = c
        .prepare_cached(
            "SELECT coalesce(sum(n), 0) FROM message_counts WHERE ?1 IS NULL OR account_id = ?1",
        )?
        .query_row([account_id], |r| r.get(0))?;
    Ok(n.max(0) as u64)
}

struct ThreadMsg {
    date: i64,
    flags: i64,
    labels: Vec<String>,
    from: Address,
    subject: String,
    snippet: String,
    otp: Option<String>,
}

/// Recompute a thread's aggregate row and its view memberships.
fn refresh_thread(tx: &Connection, trow: i64) -> Result<()> {
    let mut msgs: Vec<ThreadMsg> = tx
        .prepare_cached("SELECT date, flags, labels, from_name, from_email, subject, snippet, otp FROM messages WHERE thread_rowid = ?1")?
        .query_map([trow], |r| {
            Ok(ThreadMsg {
                date: r.get(0)?,
                flags: r.get(1)?,
                labels: split_labels(&r.get::<_, String>(2)?),
                from: Address { name: r.get(3)?, email: r.get(4)? },
                subject: r.get(5)?,
                snippet: r.get(6)?,
                otp: r.get::<_, Option<String>>(7)?.filter(|o| !o.is_empty()),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    tx.prepare_cached("DELETE FROM thread_views WHERE thread_rowid = ?1")?
        .execute([trow])?;
    let Some((account_id, thread_id)): Option<(String, String)> = tx
        .prepare_cached("SELECT account_id, thread_id FROM threads WHERE rowid = ?1")?
        .query_row([trow], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
    else {
        return Ok(());
    };
    if msgs.is_empty() {
        tx.prepare_cached("DELETE FROM threads WHERE rowid = ?1")?
            .execute([trow])?;
        snooze::thread_deleted(tx, &account_id, &thread_id)?;
        summaries::thread_deleted(tx, &account_id, &thread_id)?;
        return Ok(());
    }
    msgs.sort_by_key(|m| m.date);

    // Aggregates ignore trashed/spam messages unless that is all there is.
    let live: Vec<&ThreadMsg> = msgs
        .iter()
        .filter(|m| m.flags & (F_TRASH | F_SPAM) == 0)
        .collect();
    let agg: Vec<&ThreadMsg> = if live.is_empty() {
        msgs.iter().collect()
    } else {
        live.clone()
    };
    let subject = agg
        .iter()
        .find(|m| !m.subject.trim().is_empty())
        .map_or("", |m| m.subject.as_str());
    let latest = agg.last().expect("non-empty");
    let mut participants: Vec<Address> = Vec::new();
    for m in agg.iter().rev() {
        if !participants
            .iter()
            .any(|p| p.email.eq_ignore_ascii_case(&m.from.email))
        {
            participants.push(m.from.clone());
        }
    }
    let mut flags = 0;
    let mut labels: Vec<String> = Vec::new();
    for m in &agg {
        flags |= m.flags;
        for l in &m.labels {
            if !labels.contains(l) {
                labels.push(l.clone());
            }
        }
    }
    tx.prepare_cached(
        "UPDATE threads SET subject = ?1, snippet = ?2, participants = ?3, message_count = ?4, flags = ?5, labels = ?6, last_date = ?7, otp = ?9
         WHERE rowid = ?8",
    )?
    .execute(params![
        subject,
        latest.snippet,
        serde_json::to_string(&participants)?,
        agg.len() as i64,
        flags,
        join_labels(&labels),
        latest.date,
        trow,
        latest.otp
    ])?;

    // View memberships: view -> (last_date, unread).
    let mut views: HashMap<&str, (i64, bool)> = HashMap::new();
    let put = |views: &mut HashMap<&str, (i64, bool)>, key: &'static str, m: &ThreadMsg| {
        let e = views.entry(key).or_insert((i64::MIN, false));
        e.0 = e.0.max(m.date);
        e.1 |= m.flags & F_UNREAD != 0;
    };
    let mut label_views: HashMap<&str, (i64, bool)> = HashMap::new();
    for m in &msgs {
        let dead = m.flags & (F_TRASH | F_SPAM) != 0;
        for l in &m.labels {
            if l == "UNREAD" {
                continue;
            }
            if dead && l != "TRASH" && l != "SPAM" {
                continue;
            }
            let e = label_views.entry(l.as_str()).or_insert((i64::MIN, false));
            e.0 = e.0.max(m.date);
            e.1 |= m.flags & F_UNREAD != 0;
        }
        if !dead {
            put(&mut views, V_ALL, m);
        }
    }
    let in_inbox = live.iter().any(|m| m.flags & F_INBOX != 0);
    // A thread woken from snooze sorts at its wake time in the inbox views.
    let live_flags: Vec<i64> = live.iter().map(|m| m.flags).collect();
    let woke_at = snooze::reconcile(tx, &account_id, &thread_id, &live_flags)?;
    if !in_inbox {
        for m in live.iter().filter(|m| m.flags & F_DRAFT == 0) {
            put(&mut views, V_DONE, m);
        }
    } else {
        let inbox_msgs: Vec<&&ThreadMsg> = live.iter().filter(|m| m.flags & F_INBOX != 0).collect();
        let newest = inbox_msgs.last().expect("in inbox");
        let tab = if newest.flags & F_NEWSLETTER != 0 {
            V_INBOX_NEWS
        } else if inbox_msgs.iter().any(|m| m.flags & F_IMPORTANT != 0) {
            V_INBOX_IMPORTANT
        } else {
            V_INBOX_OTHER
        };
        for m in inbox_msgs {
            put(&mut views, tab, m);
        }
        if let Some(at) = woke_at {
            for v in [views.get_mut(tab), label_views.get_mut("INBOX")]
                .into_iter()
                .flatten()
            {
                v.0 = v.0.max(at);
            }
        }
    }
    let mut ins = tx.prepare_cached("INSERT INTO thread_views(thread_rowid, view, account_id, last_date, unread) VALUES (?1, ?2, ?3, ?4, ?5)")?;
    for (view, (date, unread)) in views.iter().chain(label_views.iter()) {
        ins.execute(params![trow, view, account_id, date, unread])?;
    }
    Ok(())
}

/// Mail dated before this (now − 48 h) isn't scanned for codes at ingest.
fn otp_ingest_cutoff() -> i64 {
    const WINDOW_MS: i64 = 48 * 3_600_000;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    now - WINDOW_MS
}

/// `messages.otp` value: the JSON, or '' for "scanned, nothing found".
pub(crate) fn otp_column(otp: Option<&Otp>) -> Result<String> {
    Ok(match otp {
        Some(o) => serde_json::to_string(o)?,
        None => String::new(),
    })
}

/// The thread summaries of `rowids`, keyed by thread rowid, from one
/// statement (a rowid list in JSON) rather than one per thread: lists built
/// in Rust (Follow up, smart views) have hundreds of rows. `last_date` is
/// left 0 for the caller to set.
pub(crate) fn summaries_by_rowid(
    c: &Connection,
    rowids: &[i64],
) -> Result<HashMap<i64, ThreadSummary>> {
    if rowids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = serde_json::to_string(rowids)?;
    let mut stmt = c.prepare_cached(
        "SELECT t.account_id, t.thread_id, t.subject, t.snippet, t.participants, t.message_count, t.flags, t.labels, t.otp,
            (SELECT s.wake_at FROM snoozes s WHERE s.account_id = t.account_id AND s.thread_id = t.thread_id AND s.woke_at IS NULL),
            0, t.rowid
         FROM threads t WHERE t.rowid IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt.query_map([ids], |r| Ok((r.get::<_, i64>(11)?, summary_row(r)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A `ThreadSummary` from `t.account_id, t.thread_id, t.subject, t.snippet,
/// t.participants, t.message_count, t.flags, t.labels, t.otp, <snoozed
/// until>, <last date>`.
fn summary_row(r: &rusqlite::Row) -> rusqlite::Result<ThreadSummary> {
    let flags: i64 = r.get(6)?;
    Ok(ThreadSummary {
        account_id: r.get(0)?,
        thread_id: r.get(1)?,
        subject: r.get(2)?,
        snippet: r.get(3)?,
        participants: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        message_count: r.get(5)?,
        unread: flags & F_UNREAD != 0,
        starred: flags & F_STARRED != 0,
        has_attachments: flags & F_ATTACH != 0,
        label_ids: split_labels(&r.get::<_, String>(7)?),
        otp: r
            .get::<_, Option<String>>(8)?
            .and_then(|j| serde_json::from_str(&j).ok()),
        snoozed_until: r.get(9)?,
        last_date: r.get(10)?,
        invite: None,
        smart: None,
    })
}

pub(crate) fn view_key(view: &MailboxView, tab: Option<InboxTab>) -> String {
    match view {
        MailboxView::Inbox => match tab.unwrap_or(InboxTab::All) {
            InboxTab::All => "INBOX".into(),
            InboxTab::Important => V_INBOX_IMPORTANT.into(),
            InboxTab::Other => V_INBOX_OTHER.into(),
            InboxTab::Newsletters => V_INBOX_NEWS.into(),
        },
        MailboxView::Starred => "STARRED".into(),
        MailboxView::Sent => "SENT".into(),
        MailboxView::Drafts => "DRAFT".into(),
        MailboxView::Done => V_DONE.into(),
        MailboxView::Trash => "TRASH".into(),
        MailboxView::Spam => "SPAM".into(),
        MailboxView::All => V_ALL.into(),
        MailboxView::Label(id) => id.clone(),
        // Served from `snoozes` (list_threads), never from thread_views.
        MailboxView::Snoozed => V_SNOOZED.into(),
        // Served by store_triage.rs (list_threads), never under these keys.
        MailboxView::ReplyLater => "~replylater".into(),
        MailboxView::FollowUp => "~followup".into(),
        // Served by store_smart.rs (list_threads).
        MailboxView::Smart(_) => "~smart".into(),
        MailboxView::Query(_) => "~query".into(),
    }
}

/// zstd level for stored bodies: ~4-6x on mail text/HTML at >300 MB/s.
const BODY_ZSTD_LEVEL: i32 = 3;

fn compress_body(s: &str) -> Result<Vec<u8>> {
    zstd::bulk::compress(s.as_bytes(), BODY_ZSTD_LEVEL)
        .map_err(|e| Error::Db(format!("compress body: {e}")))
}

/// A stored body: zstd-compressed BLOB (current) or plain TEXT (accepted so a
/// database written before compression still reads correctly).
pub(crate) struct Body(pub(crate) String);

impl rusqlite::types::FromSql for Body {
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        use rusqlite::types::{FromSqlError, ValueRef};
        match v {
            ValueRef::Text(t) => Ok(Body(String::from_utf8_lossy(t).into_owned())),
            ValueRef::Blob(b) => {
                let raw = zstd::decode_all(b).map_err(|e| FromSqlError::Other(Box::new(e)))?;
                String::from_utf8(raw)
                    .map(Body)
                    .map_err(|e| FromSqlError::Other(Box::new(e)))
            }
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

/// Load full messages for the given rowids, in the given order.
pub(crate) fn load_messages(c: &Connection, rowids: &[i64]) -> Result<Vec<Message>> {
    let mut stmt = c.prepare_cached(
        "SELECT m.account_id, m.id, t.thread_id, m.date, m.from_name, m.from_email, m.subject, m.snippet, m.labels,
                b.body_text, b.body_html, b.extra, m.flags
         FROM messages m JOIN threads t ON t.rowid = m.thread_rowid JOIN message_bodies b ON b.rowid = m.rowid
         WHERE m.rowid = ?1",
    )?;
    let mut att = c.prepare_cached(
        "SELECT att_id, filename, mime_type, size, content_id, inline FROM attachments WHERE message_rowid = ?1 ORDER BY ord",
    )?;
    let mut out = Vec::with_capacity(rowids.len());
    for &rowid in rowids {
        let row = stmt
            .query_row([rowid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, Body>(9)?.0,
                    r.get::<_, Option<Body>>(10)?.map(|b| b.0),
                    r.get::<_, String>(11)?,
                    r.get::<_, i64>(12)?,
                ))
            })
            .optional()?;
        let Some((
            account_id,
            id,
            thread_id,
            date,
            from_name,
            from_email,
            subject,
            snippet,
            labels,
            body_text,
            body_html,
            extra,
            flags,
        )) = row
        else {
            continue;
        };
        let extra: Extra = serde_json::from_str(&extra)?;
        let attachments = att
            .query_map([rowid], |r| {
                Ok(AttachmentMeta {
                    id: r.get(0)?,
                    filename: r.get(1)?,
                    mime_type: r.get(2)?,
                    size: r.get::<_, i64>(3)? as u64,
                    content_id: r.get(4)?,
                    inline: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.push(Message {
            account_id,
            id,
            thread_id,
            date,
            from: Address {
                name: from_name,
                email: from_email,
            },
            to: extra.to,
            cc: extra.cc,
            bcc: extra.bcc,
            reply_to: extra.reply_to,
            subject,
            snippet,
            body_text,
            body_html,
            label_ids: split_labels(&labels),
            attachments,
            message_id_header: extra.message_id_header,
            in_reply_to: extra.in_reply_to,
            references: extra.references,
            list_unsubscribe: extra.list_unsubscribe,
            list_unsubscribe_post: extra.list_unsubscribe_post,
            sender_authenticated: flags & F_AUTH != 0,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod people_words_migration_tests {
    use super::*;

    /// The migration splits every stored `search_key` (spaces at the ends,
    /// runs of spaces) into `people_words`.
    #[test]
    fn backfill_splits_search_keys() {
        let conn = Connection::open_in_memory().unwrap();
        let v = MIGRATIONS
            .iter()
            .position(|m| *m == SCHEMA_PEOPLE_WORDS)
            .expect("registered");
        for sql in &MIGRATIONS[..v] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", v as i64).unwrap();
        for (email, key) in [
            ("ana@ruiz.example", " ana ruiz ana ruiz example "),
            ("x@y.example", "  x   y example"),
            ("blank@z.example", " "),
        ] {
            conn.execute(
                "INSERT INTO people(email, search_key, from_count) VALUES (?1, ?2, 1)",
                [email, key],
            )
            .unwrap();
        }
        migrate(&conn).unwrap();
        let got: Vec<String> = conn
            .prepare("SELECT email || ':' || word FROM people_words ORDER BY email, word")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            got,
            [
                "ana@ruiz.example:ana",
                "ana@ruiz.example:example",
                "ana@ruiz.example:ruiz",
                "x@y.example:example",
                "x@y.example:x",
                "x@y.example:y",
            ]
        );
    }
}
