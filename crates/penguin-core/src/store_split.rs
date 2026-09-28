//! Split Inbox: the inbox divided into tabs by search queries.
//!
//! The UI keeps an ordered list of splits (`Settings.inboxSplits`), each a
//! query in the search language. A conversation in the inbox belongs to the
//! first split whose query matches one of its inbox messages (so a split
//! claims only what an earlier one didn't); the last tab, "Other", holds
//! every conversation no split claims. A list request carries one split as
//! [`SplitFilter`]: its own query (`include`, None for Other) and the queries
//! before it (`exclude`).
//!
//! Matching is per conversation but only over its messages in the inbox:
//! what came in since it was last archived decides where it goes, as the
//! rest of the thread was already dealt with.
//!
//! Cost. Everything is driven by the inbox's own `thread_views` rows, never
//! by a scan of the mailbox: a page walks the inbox newest first in chunks
//! and tests each chunk's messages against the split queries
//! (`search::inbox_threads_matching`, one statement per query per chunk,
//! over the `messages_thread` index; a query's full-text part is looked up
//! once per call as a set of message rowids), stopping once the page is
//! full. At 100k messages a tab's first page is 3–20 ms and the counts for
//! four splits 28 ms (examples/bench.rs, "Split:" rows). A
//! split whose query has a full-text part (`from:`, words) first asks the
//! full-text index for its inbox matches: when there are few (the usual
//! case: a VIP split), the page is read straight from them instead.

use std::collections::HashSet;

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};

use super::{summaries_by_rowid, triage, Store};
use crate::search::{
    compile_split, inbox_threads_matching, inbox_threads_of_matches, prepare_split, SplitPlan,
};
use crate::types::{AccountId, ListQuery, SplitCount, SplitCounts, SplitFilter, ThreadSummary};
use crate::Result;

/// Inbox conversations tested per round of the walk.
const CHUNK: usize = 400;
/// A full-text split reads its page from its matches when it has at most
/// this many inbox messages.
const CANDIDATE_CAP: usize = 2000;
/// `split_counts` stops after this many conversations (`more`).
const COUNT_CAP: usize = 3000;

/// The inbox's thread rows (rowid, last date) newest first, before
/// `before`, at most `limit`, over the per-account indexes (as
/// `list_threads` reads them).
fn inbox_page(
    c: &Connection,
    scope: Option<&[AccountId]>,
    before: i64,
    limit: usize,
    unread_only: bool,
) -> Result<Vec<(i64, i64)>> {
    let (all_idx, acct_idx, unread) = if unread_only {
        (
            "thread_views_unread_all",
            "thread_views_unread_acct",
            " AND unread = 1",
        )
    } else {
        ("thread_views_all", "thread_views_acct", "")
    };
    let mut args: Vec<Value> = vec![Value::Integer(before), Value::Integer(limit as i64)];
    let sql = match scope {
        None => format!(
            "SELECT thread_rowid, last_date FROM thread_views INDEXED BY {all_idx}
             WHERE view = 'INBOX' AND last_date < ?1{unread} ORDER BY last_date DESC LIMIT ?2"
        ),
        Some([]) => return Ok(Vec::new()),
        Some(many) => {
            let parts: Vec<String> = (0..many.len())
                .map(|i| {
                    format!(
                        "SELECT * FROM (SELECT thread_rowid, last_date FROM thread_views INDEXED BY {acct_idx}
                         WHERE account_id = ?{} AND view = 'INBOX' AND last_date < ?1{unread}
                         ORDER BY last_date DESC LIMIT ?2)",
                        i + 3
                    )
                })
                .collect();
            args.extend(many.iter().map(|a| Value::Text(a.clone())));
            format!(
                "SELECT * FROM ({}) ORDER BY last_date DESC LIMIT ?2",
                parts.join(" UNION ALL ")
            )
        }
    };
    let mut stmt = c.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The inbox rows of `threads`, newest first, before `before`, in scope.
fn inbox_rows_of(
    c: &Connection,
    threads: &HashSet<i64>,
    scope: Option<&[AccountId]>,
    before: i64,
    unread_only: bool,
) -> Result<Vec<(i64, i64)>> {
    if threads.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = threads.iter().copied().collect();
    let unread = if unread_only { " AND unread = 1" } else { "" };
    let mut stmt = c.prepare_cached(&format!(
        "SELECT thread_rowid, last_date, account_id FROM thread_views
         WHERE view = 'INBOX' AND thread_rowid IN (SELECT value FROM json_each(?1)) AND last_date < ?2{unread}
         ORDER BY last_date DESC"
    ))?;
    let rows = stmt.query_map(
        params_from_iter([
            Value::Text(serde_json::to_string(&ids)?),
            Value::Integer(before),
        ]),
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (t, d, a) = row?;
        if scope.is_none_or(|s| s.contains(&a)) {
            out.push((t, d));
        }
    }
    Ok(out)
}

/// The compiled queries of a split: its own (None = Other) and the earlier
/// splits'. An earlier split with an unreadable query claims nothing (its own
/// tab says why); this split's own unreadable query is the error.
struct Compiled {
    include: Option<SplitPlan>,
    /// The split's own query can't match anything: an empty list.
    nothing: bool,
    exclude: Vec<SplitPlan>,
}

fn compile(
    c: &Connection,
    split: &SplitFilter,
    scope: Option<&[AccountId]>,
    now: i64,
) -> Result<Compiled> {
    let (include, nothing) = match split.include.as_deref() {
        None => (None, false),
        Some(q) => match compile_split(c, q, scope, now)? {
            Some(p) => (Some(p), false),
            None => (None, true),
        },
    };
    let exclude = split
        .exclude
        .iter()
        .filter_map(|q| compile_split(c, q, scope, now).ok().flatten())
        .map(prepare_split)
        .collect::<Vec<_>>();
    let include = include.map(prepare_split);
    Ok(Compiled {
        include,
        nothing,
        exclude,
    })
}

/// Of `rows`, the ones this split shows: matched by its query (when it has
/// one) and by none of the earlier ones. Keeps the order.
fn keep(c: &Connection, q: &Compiled, rows: &[(i64, i64)]) -> Result<Vec<(i64, i64)>> {
    let mut left: Vec<(i64, i64)> = rows.to_vec();
    if let Some(p) = &q.include {
        let ids: Vec<i64> = left.iter().map(|r| r.0).collect();
        let hit = inbox_threads_matching(c, p, &ids)?;
        left.retain(|r| hit.contains(&r.0));
    }
    keep_excluding(c, &q.exclude, &left)
}

/// Of `rows`, the ones no earlier split claims. Keeps the order.
fn keep_excluding(
    c: &Connection,
    exclude: &[SplitPlan],
    rows: &[(i64, i64)],
) -> Result<Vec<(i64, i64)>> {
    let mut left: Vec<(i64, i64)> = rows.to_vec();
    for p in exclude {
        if left.is_empty() {
            break;
        }
        let ids: Vec<i64> = left.iter().map(|r| r.0).collect();
        let hit = inbox_threads_matching(c, p, &ids)?;
        left.retain(|r| !hit.contains(&r.0));
    }
    Ok(left)
}

impl Store {
    /// `MailboxView::Inbox` with a split: that split's conversations, newest
    /// first, paged by `before` like the inbox. An unreadable split query is
    /// `InvalidQuery`.
    pub(crate) fn list_split(
        &self,
        split: &SplitFilter,
        scope: Option<&[AccountId]>,
        query: &ListQuery,
    ) -> Result<Vec<ThreadSummary>> {
        let limit = if query.limit == 0 {
            50
        } else {
            query.limit.min(1000)
        } as usize;
        let before = query.before.unwrap_or(i64::MAX);
        if scope.is_some_and(<[AccountId]>::is_empty) {
            return Ok(Vec::new());
        }
        let now = triage::now_ms();
        self.read(|c| {
            let q = compile(c, split, scope, now)?;
            if q.nothing {
                return Ok(Vec::new());
            }
            let mut picked: Vec<(i64, i64)> = Vec::new();
            // Few inbox matches for a full-text query: read them directly.
            let direct = match &q.include {
                Some(p) => inbox_threads_of_matches(c, &p.plan, CANDIDATE_CAP)?,
                None => None,
            };
            if let Some(threads) = direct {
                let rows = inbox_rows_of(c, &threads, scope, before, query.unread_only)?;
                for chunk in rows.chunks(CHUNK) {
                    // The candidates matched; only the earlier splits remain to test.
                    picked.extend(keep_excluding(c, &q.exclude, chunk)?);
                    if picked.len() >= limit {
                        break;
                    }
                }
            } else {
                let mut cursor = before;
                loop {
                    let page = inbox_page(c, scope, cursor, CHUNK, query.unread_only)?;
                    let Some(&(_, last)) = page.last() else { break };
                    picked.extend(keep(c, &q, &page)?);
                    if picked.len() >= limit || page.len() < CHUNK || last >= cursor {
                        break;
                    }
                    cursor = last;
                }
            }
            picked.truncate(limit);
            let ids: Vec<i64> = picked.iter().map(|r| r.0).collect();
            let mut rows = summaries_by_rowid(c, &ids)?;
            Ok(picked
                .into_iter()
                .filter_map(|(t, d)| {
                    rows.remove(&t).map(|mut s| {
                        s.last_date = d;
                        s
                    })
                })
                .collect())
        })
    }

    /// How many conversations, and how many unread, each split of
    /// `queries` holds (in order: a conversation counts for the first split
    /// that matches it), then Other. One walk over the inbox, newest first;
    /// it stops after `COUNT_CAP` conversations (`more`). A split with an
    /// unreadable query counts nothing.
    pub fn split_counts(
        &self,
        queries: &[String],
        scope: Option<&[AccountId]>,
    ) -> Result<SplitCounts> {
        let mut out = SplitCounts {
            splits: vec![SplitCount::default(); queries.len() + 1],
            more: false,
        };
        if scope.is_some_and(<[AccountId]>::is_empty) {
            return Ok(out);
        }
        let now = triage::now_ms();
        self.read(|c| {
            let plans: Vec<Option<SplitPlan>> = queries
                .iter()
                .map(|q| {
                    compile_split(c, q, scope, now)
                        .ok()
                        .flatten()
                        .map(prepare_split)
                })
                .collect();
            let mut cursor = i64::MAX;
            let mut seen = 0usize;
            loop {
                let page = inbox_page(c, scope, cursor, CHUNK, false)?;
                let Some(&(_, last)) = page.last() else { break };
                let unread = unread_of(c, &page)?;
                let mut left: Vec<i64> = page.iter().map(|r| r.0).collect();
                let mut tally = |i: usize, threads: &mut dyn Iterator<Item = i64>| {
                    for t in threads {
                        out.splits[i].total += 1;
                        out.splits[i].unread += u32::from(unread.contains(&t));
                    }
                };
                for (i, p) in plans.iter().enumerate() {
                    let Some(p) = p else { continue };
                    if left.is_empty() {
                        break;
                    }
                    let hit = inbox_threads_matching(c, p, &left)?;
                    tally(i, &mut left.iter().copied().filter(|t| hit.contains(t)));
                    left.retain(|t| !hit.contains(t));
                }
                tally(queries.len(), &mut left.into_iter());
                seen += page.len();
                if page.len() < CHUNK || last >= cursor {
                    break;
                }
                if seen >= COUNT_CAP {
                    out.more = true;
                    break;
                }
                cursor = last;
            }
            Ok(out)
        })
    }
}

/// Which of `rows` are unread in the inbox.
fn unread_of(c: &Connection, rows: &[(i64, i64)]) -> Result<HashSet<i64>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let mut stmt = c.prepare_cached(
        "SELECT thread_rowid FROM thread_views
         WHERE view = 'INBOX' AND thread_rowid IN (SELECT value FROM json_each(?1)) AND unread = 1",
    )?;
    let rows = stmt.query_map([serde_json::to_string(&ids)?], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
#[path = "store_split_tests.rs"]
mod tests;
