//! First contacts: for every address, the first message you ever received
//! from it (`dir` 0) and the first message you ever sent to it (`dir` 1).
//! Backs the `is:new-sender` (`from:new`) and `is:first-outbound` (`to:new`)
//! search operators, which then cost one indexed lookup per candidate
//! message instead of a scan of everything the address ever sent.
//!
//! "Received" = neither SENT nor DRAFT, and not in Spam (a spammer using an
//! address first shouldn't hide the real first message). "Sent" = SENT and
//! not a draft; its recipients are To, Cc and Bcc. Only mail stored locally
//! counts: with a sync window and no older headers, "first" means first
//! within what this Mac has.
//!
//! The same module keeps `F_TO_ME` (one of your addresses is a recipient,
//! for `to:me`): exact at insert, backfilled by the migration and refreshed
//! when an account is added or removed through the FTS recipients/cc
//! columns (a phrase match of the address, so the whole index isn't read).
//!
//! Maintenance: inserting a message lowers the stored minimum (`msg` is a
//! message rowid, which is date-ordered); removing the message a row points
//! at (or relabelling it out of the counted set) recomputes that address
//! from what's left: the `messages_from` index for received mail, and
//! `sent_recipients` (every sent message's recipients) for sent mail.

use rusqlite::{params, Connection, OptionalExtension};

use super::{F_DRAFT, F_SENT, F_SPAM, F_TO_ME};
use crate::types::Address;
use crate::Result;

/// Schema and backfill of the tables from what is already stored, plus
/// two search helpers that ride along: `messages.att_bytes` (total size of
/// the attached files, for larger:/smaller:) and a partial index over
/// messages with a detected code (has:otp).
pub(super) const SCHEMA_FIRST_CONTACTS: &str = r#"
ALTER TABLE messages ADD COLUMN att_bytes INTEGER NOT NULL DEFAULT 0;
UPDATE messages SET att_bytes = (
    SELECT coalesce(sum(a.size), 0) FROM attachments a
    WHERE a.message_rowid = messages.rowid AND a.inline = 0)
WHERE flags & 512 != 0;
CREATE INDEX messages_otp ON messages(date) WHERE otp IS NOT NULL AND otp != '';
CREATE TABLE sent_recipients(
    email TEXT NOT NULL,
    msg INTEGER NOT NULL,
    PRIMARY KEY(email, msg)
) WITHOUT ROWID;
CREATE TABLE first_contacts(
    email TEXT NOT NULL,
    dir INTEGER NOT NULL,
    msg INTEGER NOT NULL,
    PRIMARY KEY(email, dir)
) WITHOUT ROWID;
CREATE INDEX first_contacts_msg ON first_contacts(msg);
INSERT OR IGNORE INTO sent_recipients(email, msg)
    SELECT lower(trim(json_extract(r.value, '$.email'))), m.rowid
    FROM messages m JOIN message_bodies b ON b.rowid = m.rowid,
         json_each(CASE WHEN json_valid(b.extra) THEN b.extra ELSE '{}' END) f,
         json_each(CASE WHEN f.type = 'array' THEN f.value ELSE '[]' END) r
    WHERE (m.flags & 24) = 8 AND f.key IN ('to', 'cc', 'bcc')
      AND r.type = 'object'
      AND trim(coalesce(json_extract(r.value, '$.email'), '')) != '';
INSERT INTO first_contacts(email, dir, msg)
    SELECT lower(trim(from_email)), 0, min(rowid) FROM messages
    WHERE (flags & 88) = 0 AND trim(from_email) != ''
    GROUP BY lower(trim(from_email));
INSERT INTO first_contacts(email, dir, msg)
    SELECT email, 1, min(msg) FROM sent_recipients GROUP BY email;
UPDATE messages SET flags = flags | 262144 WHERE rowid IN (
    SELECT f.rowid FROM (
        SELECT '{recipients cc} : (' || group_concat('"' || replace(email, '"', '') || '"', ' OR ') || ')' AS q
        FROM accounts
    ) a JOIN messages_fts f ON f.messages_fts MATCH a.q
    WHERE a.q IS NOT NULL);
"#;

/// Backfill of `F_TO_ME` (262144) for mail sent to a `+tag` of one of your
/// addresses, which `to:me` missed before `addressed_to_me` knew about
/// plus-addressing. Candidates come from the index (local part near the
/// domain), and each is checked exactly against the stored To/Cc/Bcc.
pub(super) const SCHEMA_TO_ME_PLUS: &str = r#"
UPDATE messages SET flags = flags | 262144 WHERE flags & 262144 = 0 AND rowid IN (
    SELECT m.rowid
    FROM accounts a
    JOIN messages_fts f ON f.messages_fts MATCH (
        '{recipients cc} : NEAR("'
        || replace(substr(lower(trim(a.email)), 1, instr(trim(a.email), '@') - 1), '"', '')
        || '" "'
        || replace(substr(lower(trim(a.email)), instr(trim(a.email), '@') + 1), '"', '')
        || '", 3)')
    JOIN messages m ON m.rowid = f.rowid
    JOIN message_bodies b ON b.rowid = m.rowid,
    json_each(CASE WHEN json_valid(b.extra) THEN b.extra ELSE '{}' END) k,
    json_each(CASE WHEN k.type = 'array' THEN k.value ELSE '[]' END) r
    WHERE instr(trim(a.email), '@') > 1
      AND k.key IN ('to', 'cc', 'bcc') AND r.type = 'object'
      AND instr(lower(trim(json_extract(r.value, '$.email'))), '+') > 1
      AND substr(lower(trim(json_extract(r.value, '$.email'))), 1,
                 instr(lower(trim(json_extract(r.value, '$.email'))), '+') - 1)
          || substr(lower(trim(json_extract(r.value, '$.email'))),
                    instr(lower(trim(json_extract(r.value, '$.email'))), '@'))
          = lower(trim(a.email))
);
"#;

/// Flags that keep a message out of the received side (SENT | DRAFT | SPAM = 88).
const NOT_RECEIVED: i64 = F_SENT | F_DRAFT | F_SPAM;

fn counts_as_received(flags: i64) -> bool {
    flags & NOT_RECEIVED == 0
}

fn counts_as_sent(flags: i64) -> bool {
    flags & (F_SENT | F_DRAFT) == F_SENT
}

fn norm(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

/// Distinct, normalized, non-empty recipient addresses.
fn recipient_emails<'a>(recipients: impl Iterator<Item = &'a Address>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in recipients {
        let e = norm(&a.email);
        if !e.is_empty() && !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// Record message `rowid` (with these flags, sender and recipients).
pub(super) fn add<'a>(
    tx: &Connection,
    rowid: i64,
    flags: i64,
    from_email: &str,
    recipients: impl Iterator<Item = &'a Address>,
) -> Result<()> {
    let mut lower = tx.prepare_cached(
        "INSERT INTO first_contacts(email, dir, msg) VALUES (?1, ?2, ?3)
         ON CONFLICT(email, dir) DO UPDATE SET msg = min(msg, excluded.msg)",
    )?;
    if counts_as_received(flags) {
        let e = norm(from_email);
        if !e.is_empty() {
            lower.execute(params![e, 0, rowid])?;
        }
    }
    if counts_as_sent(flags) {
        let mut ins =
            tx.prepare_cached("INSERT OR IGNORE INTO sent_recipients(email, msg) VALUES (?1, ?2)")?;
        for e in recipient_emails(recipients) {
            ins.execute(params![e, rowid])?;
            lower.execute(params![e, 1, rowid])?;
        }
    }
    Ok(())
}

/// Forget message `rowid` (still present in `messages`; the caller deletes
/// or relabels it next). Addresses whose first contact it was are
/// recomputed from the rest.
pub(super) fn remove<'a>(
    tx: &Connection,
    rowid: i64,
    flags: i64,
    from_email: &str,
    recipients: impl Iterator<Item = &'a Address>,
) -> Result<()> {
    if counts_as_received(flags) {
        let e = norm(from_email);
        if first_is(tx, &e, 0, rowid)? {
            // messages_from is (from_email COLLATE NOCASE, date): walk this
            // sender's mail oldest first to the first one that still counts.
            let next: Option<i64> = tx
                .prepare_cached(
                    "SELECT rowid FROM messages INDEXED BY messages_from
                     WHERE from_email = ?1 COLLATE NOCASE AND rowid != ?2 AND (flags & ?3) = 0
                     ORDER BY from_email COLLATE NOCASE, date, rowid LIMIT 1",
                )?
                .query_row(params![e, rowid, NOT_RECEIVED], |r| r.get(0))
                .optional()?;
            set_first(tx, &e, 0, next)?;
        }
    }
    if counts_as_sent(flags) {
        for e in recipient_emails(recipients) {
            tx.prepare_cached("DELETE FROM sent_recipients WHERE email = ?1 AND msg = ?2")?
                .execute(params![e, rowid])?;
            if first_is(tx, &e, 1, rowid)? {
                let next: Option<i64> = tx
                    .prepare_cached("SELECT min(msg) FROM sent_recipients WHERE email = ?1")?
                    .query_row([&e], |r| r.get(0))?;
                set_first(tx, &e, 1, next)?;
            }
        }
    }
    Ok(())
}

fn first_is(tx: &Connection, email: &str, dir: i64, rowid: i64) -> Result<bool> {
    let cur: Option<i64> = tx
        .prepare_cached("SELECT msg FROM first_contacts WHERE email = ?1 AND dir = ?2")?
        .query_row(params![email, dir], |r| r.get(0))
        .optional()?;
    Ok(cur == Some(rowid))
}

fn set_first(tx: &Connection, email: &str, dir: i64, msg: Option<i64>) -> Result<()> {
    match msg {
        Some(m) => tx
            .prepare_cached("UPDATE first_contacts SET msg = ?3 WHERE email = ?1 AND dir = ?2")?
            .execute(params![email, dir, m])?,
        None => tx
            .prepare_cached("DELETE FROM first_contacts WHERE email = ?1 AND dir = ?2")?
            .execute(params![email, dir])?,
    };
    Ok(())
}

/// `user+tag@domain` → `user@domain` (plus-addressing, "subaddressing" in
/// RFC 5233): mail to a tagged address is mail to that account.
fn untagged(e: &str) -> Option<String> {
    let (local, domain) = e.split_once('@')?;
    let (base, _) = local.split_once('+')?;
    (!base.is_empty()).then(|| format!("{base}@{domain}"))
}

/// Is one of these addresses one of your accounts (or a `+tag` of one)?
pub(super) fn addressed_to_me<'a>(
    tx: &Connection,
    recipients: impl Iterator<Item = &'a Address>,
) -> Result<bool> {
    let mut own =
        tx.prepare_cached("SELECT EXISTS (SELECT 1 FROM accounts WHERE lower(trim(email)) = ?1)")?;
    for e in recipient_emails(recipients) {
        for cand in std::iter::once(e.clone()).chain(untagged(&e)) {
            if own.query_row([&cand], |r| r.get::<_, bool>(0))? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// An FTS match for an account address in the recipient columns, tagged
/// copies included: the address as a phrase, or its local part within three
/// words of its domain (`alex+shop@mail.example`; the tokenizer drops `+`).
fn address_match(email: &str) -> String {
    let phrase = |e: &str| format!("\"{}\"", e.replace('"', ""));
    match email.split_once('@') {
        Some((local, domain))
            if local.chars().any(char::is_alphanumeric) && domain.chars().any(char::is_alphanumeric) =>
        {
            format!("{} OR NEAR({} {}, 3)", phrase(email), phrase(local), phrase(domain))
        }
        _ => phrase(email),
    }
}

/// Recompute `F_TO_ME` for stored mail whose recipients mention `email`
/// (an account that was just added, re-addressed or removed). Tagged copies
/// (`user+tag@`) are found by proximity, so for them this refresh is
/// approximate; inserts are exact (`addressed_to_me`).
pub(super) fn refresh_to_me(tx: &Connection, email: &str) -> Result<()> {
    if !email.chars().any(char::is_alphanumeric) {
        return Ok(());
    }
    let touched = format!("{{recipients cc}} : ({})", address_match(email));
    let own: Vec<String> = tx
        .prepare_cached("SELECT email FROM accounts")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let own: Vec<String> = own
        .iter()
        .filter(|e| e.chars().any(char::is_alphanumeric))
        .map(|e| format!("({})", address_match(e)))
        .collect();
    tx.prepare_cached(&format!(
        "UPDATE messages SET flags = flags & ~{F_TO_ME} WHERE rowid IN
         (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1)"
    ))?
    .execute([&touched])?;
    if !own.is_empty() {
        let mine = format!("{{recipients cc}} : ({})", own.join(" OR "));
        tx.prepare_cached(&format!(
            "UPDATE messages SET flags = flags | {F_TO_ME} WHERE rowid IN
             (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1)
             AND rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?2)"
        ))?
        .execute([&touched, &mine])?;
    }
    Ok(())
}

/// Recompute `first_contacts` from scratch (after an account's mail is
/// removed wholesale; `sent_recipients` must already be pruned).
pub(super) fn rebuild(tx: &Connection) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM first_contacts;
         INSERT INTO first_contacts(email, dir, msg)
             SELECT lower(trim(from_email)), 0, min(rowid) FROM messages
             WHERE (flags & 88) = 0 AND trim(from_email) != ''
             GROUP BY lower(trim(from_email));
         INSERT INTO first_contacts(email, dir, msg)
             SELECT email, 1, min(msg) FROM sent_recipients GROUP BY email;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Account, Message};
    use crate::Store;

    fn msg(id: &str, date: i64, from: &str, to: &[&str], labels: &[&str]) -> Message {
        let addr = |e: &str| Address {
            name: None,
            email: e.to_string(),
        };
        Message {
            account_id: "me@work.example".into(),
            id: id.into(),
            thread_id: format!("t-{id}"),
            date,
            from: addr(from),
            to: to.iter().map(|e| addr(e)).collect(),
            cc: vec![],
            bcc: vec![addr("hidden@bcc.example")],
            reply_to: vec![],
            subject: id.into(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: None,
            label_ids: labels.iter().map(|l| l.to_string()).collect(),
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    type Snapshot = (
        Vec<(String, i64, i64)>,
        Vec<(String, i64)>,
        Vec<(i64, i64, i64)>,
    );

    fn snapshot(s: &Store) -> Snapshot {
        s.read(|c| {
            let firsts = c
                .prepare("SELECT email, dir, msg FROM first_contacts ORDER BY email, dir")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let sent = c
                .prepare("SELECT email, msg FROM sent_recipients ORDER BY email, msg")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let rows = c
                .prepare("SELECT rowid, flags, att_bytes FROM messages ORDER BY rowid")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((firsts, sent, rows))
        })
        .unwrap()
    }

    /// The migration's SQL backfill (run over existing mail) must agree
    /// with what incremental maintenance builds, and so must `rebuild`.
    #[test]
    fn backfill_matches_incremental_maintenance() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&Account {
            id: "me@work.example".into(),
            email: "me@work.example".into(),
            color: "#123456".into(),
            ..Account::default()
        })
        .unwrap();
        let day = 86_400_000;
        let me = "me@work.example";
        let mut with_files = msg(
            "f1",
            6 * day,
            "bo@files.example",
            &["team@x.example"],
            &["INBOX"],
        );
        for (i, size) in [(0, 1500u64), (1, 2500)] {
            with_files.attachments.push(crate::types::AttachmentMeta {
                id: format!("a{i}"),
                filename: format!("f{i}.pdf"),
                mime_type: "application/pdf".into(),
                size,
                content_id: None,
                inline: false,
            });
        }
        s.upsert_messages(&[with_files]).unwrap();
        s.upsert_messages(&[
            msg("r2", 5 * day, "Ana@Ruiz.example", &[me], &["INBOX"]),
            msg("r1", 3 * day, "ana@ruiz.example", &[me], &["INBOX"]),
            msg("spam", day, "ana@ruiz.example", &[me], &["SPAM"]),
            msg(
                "s1",
                4 * day,
                me,
                &["X@one.example", "y@two.example"],
                &["SENT"],
            ),
            msg("s2", 2 * day, me, &["x@one.example"], &["SENT"]),
            msg("d1", day, me, &["z@three.example"], &["DRAFT"]),
        ])
        .unwrap();
        let incremental = snapshot(&s);
        // ana (r1, not the spam copy) and bo; x, y, hidden@bcc as recipients.
        assert_eq!(incremental.0.len(), 5, "{incremental:?}");
        assert!(incremental.1.iter().any(|(e, _)| e == "hidden@bcc.example"));
        assert!(!incremental.1.iter().any(|(e, _)| e == "z@three.example"));
        // to:me on the three received messages addressed to the account;
        // att_bytes on the one with files.
        let to_me = incremental.2.iter().filter(|r| r.1 & F_TO_ME != 0).count();
        assert_eq!(to_me, 3, "{incremental:?}");
        assert!(incremental.2.iter().any(|r| r.2 == 4000));

        s.write(|tx| {
            tx.execute_batch(&format!(
                "DROP TABLE first_contacts; DROP TABLE sent_recipients;
                 DROP INDEX messages_otp; ALTER TABLE messages DROP COLUMN att_bytes;
                 UPDATE messages SET flags = flags & ~{F_TO_ME};"
            ))?;
            tx.execute_batch(SCHEMA_FIRST_CONTACTS)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(snapshot(&s), incremental);

        s.write(|tx| rebuild(tx)).unwrap();
        assert_eq!(snapshot(&s), incremental);
    }

    /// The plus-addressing backfill sets `F_TO_ME` exactly where inserting
    /// the mail now would.
    #[test]
    fn plus_address_backfill_matches_insert() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&Account {
            id: "me@work.example".into(),
            email: "me@work.example".into(),
            color: "#123456".into(),
            ..Account::default()
        })
        .unwrap();
        let day = 86_400_000;
        s.upsert_messages(&[
            msg("tag", 3 * day, "a@x.example", &["Me+Shop@Work.example"], &["INBOX"]),
            msg("plain", 2 * day, "a@x.example", &["me@work.example"], &["INBOX"]),
            msg("near", day, "a@x.example", &["me+x@work.example.org", "me@elsewhere.example"], &["INBOX"]),
        ])
        .unwrap();
        let to_me = |s: &Store| -> Vec<(i64, bool)> {
            snapshot(s).2.iter().map(|r| (r.0, r.1 & F_TO_ME != 0)).collect()
        };
        let inserted = to_me(&s);
        assert_eq!(inserted.iter().filter(|r| r.1).count(), 2, "{inserted:?}");
        s.write(|tx| {
            tx.execute_batch(&format!("UPDATE messages SET flags = flags & ~{F_TO_ME};"))?;
            tx.execute_batch(SCHEMA_TO_ME_PLUS)?;
            tx.execute_batch(&format!(
                "UPDATE messages SET flags = flags | {F_TO_ME} WHERE subject = 'plain';"
            ))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(to_me(&s), inserted);
    }
}
