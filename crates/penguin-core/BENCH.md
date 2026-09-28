# penguin-core search benchmark

Reproduce:

```sh
source ~/.cargo/env
cargo run -p penguin-core --release --example bench_search          # builds a 300k db (~2.5 min) and runs everything
PENGUIN_BENCH_REUSE=1 cargo run -p penguin-core --release --example bench_search   # re-run queries on the existing db
cargo run -p penguin-core --release --example probe_search -- <db> "query"         # one query, per-stage timings
```

Machine: Apple M5 Pro (18 cores, 64 GB), macOS 26.6, SQLite 3.50.2 (bundled), release build.
Date: 2026-09-23. All numbers below are real output from `bench_search`. Nothing is estimated.

## Corpus

The corpus is synthetic and deterministic (`examples/support/corpus.rs`, seed 42). It has 300,003 messages across 3 accounts (60/30/10 split) and covers 8 years, with volume rising toward the present. There are 4,000 fictional correspondents on `.example` domains, weighted by a power law so a few correspondents dominate.

- 45% are newsletters and notifications: 150–1,050 words each, List-Unsubscribe, and a CATEGORY_* label. Their HTML is marketing-style, with a big `<style>` block, nested layout tables, verbose inline styles, image rows, preheader padding and unique per-recipient tracking URLs. That is typically 10–40 KB each, like real newsletters.
- 55% are conversations of 1–8 messages with Gmail-style quoted history, 60% of which carry a light HTML part.
- 12% have attachments (invoices `Invoice_INV-NNNNN.pdf`, contracts, photos, xlsx/docx/pptx), and 15% of threads carry a user label. There are 20 user labels plus the system labels per account.
- The vocabulary has 40k words drawn from a Zipf distribution (s = 1.07). The top ~600 are real English and email words, so words like "invoice", "lease" and "meeting" are *much* more frequent here than in real mail. That makes this a pessimistic test for latency and a poor test for ranking quality (see caveats).
- Input totals: **581 MiB plain text** (subject + body_text) and **1,948 MiB raw HTML** (6.8 KB per message on average).
- Three hand-written known items are planted: the lease PDF from Mike, invoice INV-20417, and the cabin wifi password.

## Build and disk

| metric | value |
|---|---|
| messages | 300,003 |
| insert + index (batches of 500, excl. corpus generation) | 123.8 s (**2,424 msg/s**, including zstd) |
| FTS `optimize` after backfill | 7.2 s |
| db file on disk after checkpoint | **2,406 MiB** |

### Where the bytes go: real on-disk pages, from SQLite `dbstat`

| table / index | MiB total | **MiB per 100k msgs** | bytes / msg |
|---|---:|---:|---:|
| `message_bodies` (zstd body_text + zstd body_html + header JSON) | 1,057.5 | **352.5** | 3,696 |
| `messages_fts` data (the full-text index) | 1,056.6 | **352.2** | 3,693 |
| `messages` (metadata) + its unique index | 108.9 | 36.3 | 381 |
| `threads` + its unique index | 56.7 | 18.9 | 198 |
| `thread_views` + 3 indexes (list views, unread counts) | 51.2 | 17.1 | 179 |
| `attachments` + index + trigram filename index | 19.9 | 6.6 | 70 |
| `messages_fts` docsize/idx | 8.7 | 2.9 | 30 |
| `messages_thread` index, `message_labels`, `people`, rest | 8.7 | 2.9 | 30 |
| **total** | **2,368.2** | **789.4** | **8,277** |

**About 790 MiB per 100k messages**, so a mailbox of roughly 300k messages across 3 accounts comes to about 2.4 GB.

- **Bodies are zstd level 3.** Raw HTML shrinks 1,948 → 473 MiB (4.1x) and body_text 581 → 319 MiB (1.8x). Before compression, this layout stored bodies at 1,579 MiB for a corpus with 2.6x *less* HTML. Decompression happens only in `get_message`/`get_thread` and for the ≤50 hits a search hydrates for snippets. The latency tables below include it.
- **HTML is never indexed.** The FTS index holds subject, sender, recipients, cc, authored body, quoted body (weight 0.15, capped at 32 KB per message) and attachment filenames. It is about 1.8x the plain text, including a 3-character prefix index and full positions for phrase search.
- Reading tolerates plain-TEXT body rows, so databases written before compression still open correctly (tested).

Indexing at about 2,400 msg/s is roughly 25–50x what the Gmail API delivers per account, so the store is never the backfill bottleneck.

### FTS index options measured (300k messages, subject/sender/body columns)

| prefix option | index size | `"th"*` | `"the"*` | `"in"*` | `"inv"*` |
|---|---:|---:|---:|---:|---:|
| none | 611 MiB | 744 ms | 352 ms | 63 ms | 5.9 ms |
| `prefix='3'` (**chosen**) | 1,054 MiB | 274 ms | 7.9 ms | 49 ms | 2.6 ms |
| `prefix='2 3'` | 1,378 MiB | 8.3 ms | 7.8 ms | 6.0 ms | 2.6 ms |

We chose `prefix='3'` and only prefix-match the final word once it has 3 or more characters. A 1–2 character fragment is matched as an exact word, and the People panel (which does prefix-match) covers "mi" → Mike. This saves 24% of the index compared with `'2 3'` and loses nothing a user would notice. With no prefix index at all, common stems like `the*` merge dozens of doclists and take hundreds of ms.

## list_threads (50 rows, warm, 200 runs each)

| view | p50 ms | p95 ms | p99 ms |
|---|---:|---:|---:|
| Inbox (unified, 3 accounts) | 0.03 | 0.04 | 0.06 |
| Inbox / Important | 0.04 | 0.04 | 0.05 |
| Inbox / Newsletters | 0.03 | 0.03 | 0.04 |
| Inbox (one account) | 0.03 | 0.04 | 0.05 |
| All mail | 0.03 | 0.04 | 0.06 |
| All mail, page 20 (`before` cursor) | 0.03 | 0.04 | 0.05 |
| Done (archived) | 0.03 | 0.03 | 0.05 |
| Sent | 0.03 | 0.04 | 0.04 |
| Label_3 | 0.03 | 0.04 | 0.06 |
| Starred | 0.03 | 0.04 | 0.05 |

Target was < 5 ms; the actual time is about 40 µs. Every view and tab is a single range scan over `thread_views(view, last_date)`, a table materialized on every write.

`list_labels(None)` with **locally computed unread counts** for 96 labels across 3 accounts: **p50 0.15 ms, p95 0.16 ms**. The counts come from a partial index `thread_views(account_id, view) WHERE unread = 1`, so they reflect optimistic mark-read immediately. Gmail's labels.list doesn't return these counts.

## search (limit 50, warm, 60 runs each)

Timings are end to end through `Store::search`: parse, people panel, retrieval and ranking, hydration of 50 hits (zstd decompression plus snippets), and the attachment panel.

| query | kind | p50 ms | p95 ms | p99 ms | threads | top hit |
|---|---|---:|---:|---:|---:|---|
| `meeting` | single common word | 6.2 | 6.3 | 6.6 | 50 | |
| `invoice` | single mid-frequency word | 8.0 | 8.1 | 8.7 | 50 | |
| `escrow` | single rare word | 6.3 | 6.5 | 6.6 | 50 | |
| `the` | very common word | 11.0 | 11.2 | 11.3 | 50 | |
| `lease renewal` | two words | 5.5 | 5.7 | 5.9 | 50 | planted lease ✓ |
| `time people` | two common words | 16.2 | 17.3 | 17.5 | 50 | |
| `quarterly report deadline` | three words | 5.6 | 5.7 | 5.8 | 50 | |
| `in` | 2 chars (exact, no prefix) | 7.9 | 8.1 | 8.1 | 50 | |
| `inv` | prefix 3 chars | 7.0 | 7.1 | 7.2 | 50 | |
| `invo` | prefix 4 chars | 8.0 | 8.2 | 8.4 | 50 | |
| `ther` | prefix 4 chars, very common stem | 21.4 | 21.8 | 21.9 | 50 | |
| `invoice` | prefix, complete word | 8.0 | 8.3 | 8.4 | 50 | |
| `lease ren` | prefix on 2nd word | 5.8 | 6.1 | 6.2 | 50 | |
| `from:mike` | from: name | 2.4 | 2.4 | 2.5 | 50 | |
| `from:mike.delgado@realty.example` | from: email | 4.8 | 4.9 | 5.0 | 1 | planted lease ✓ |
| `from:mike lease` | from: + word | 4.1 | 4.2 | 4.2 | 50 | |
| `has:pdf date:"last spring"` | filter + `date:` | 0.5 | 0.7 | 0.7 | 50 | |
| `lease has:pdf date:"last spring"` | word + filter + `date:` | 2.5 | 3.0 | 3.4 | 50 | planted lease ✓ |
| `INV-20417` | rare identifier | 0.9 | 1.0 | 1.0 | 2 | planted invoice ✓ |
| `20417` | identifier digits only | 0.7 | 0.8 | 0.8 | 2 | planted invoice ✓ |
| `filename:20417` | trigram substring | 0.0 | 0.0 | 0.0 | 1 | planted invoice ✓ |
| `"early termination clause"` | phrase | 0.0 | 0.0 | 0.0 | 1 | planted lease ✓ |
| `"to the"` | phrase of common words | 6.0 | 6.2 | 6.4 | 50 | |
| `the pdf mike sent about the lease date:"last spring"` | natural language + `date:` | 3.1 | 3.5 | 3.5 | 2 | planted lease ✓ |
| `invoice -stripe` | exclusion | 6.7 | 6.8 | 7.1 | 50 | |
| `lease OR mortgage` | OR | 7.5 | 7.6 | 7.7 | 50 | |
| `is:unread` | filter only | 0.5 | 0.6 | 0.6 | 50 | |
| `in:sent budget` | folder + word | 12.1 | 12.9 | 13.0 | 50 | |
| `label:Label_3 meeting` | label + word | 6.4 | 6.6 | 7.1 | 38 | |
| `account:personal invoice` | account + word | 13.1 | 13.7 | 14.4 | 50 | |
| `password in:anywhere` | scope + word | 6.0 | 6.2 | 6.2 | 50 | |
| `tax older_than:2y` | date + word | 6.0 | 6.2 | 6.5 | 50 | |
| `zzqxjv` | no results | 0.7 | 0.7 | 0.7 | 0 | |

- **All queries pooled: p50 6.0 ms, p95 16.2 ms, p99 21.4 ms.** Every query's own p95 is under the 30 ms target.
- Dates in search are explicit: only `date:` values become date filters, and "last spring" typed bare is two search words. The three `date:` rows above come from a 2026-09-24 re-run made while other agents were compiling (load average about 7). That run was uniformly 15–25% slower on every query, including ones with no dates (pooled p95 18.7 ms, and `ther` p95 30.8 ms). Parsing the query itself takes at most 2 µs, so the query-language change does not affect latency. The other rows are from the earlier quiet-machine run.
- **As-you-type**, every prefix of `invoice from:mike` searched separately: p50 8.1 ms, p95 25.8 ms, max 28.2 ms.
- First query after reopening the store (fresh connection pool, OS page cache warm): 10.3 ms. A true cold start after reboot was not measured.

## html_to_text (runs on every synced message)

| | throughput | 255 KB marketing email |
|---|---:|---:|
| `penguin_core::text::html_to_text` (hand-rolled streaming) | **1,128 MiB/s** | 0.22 ms |
| `html2text` 0.15 (html5ever DOM + layout) | 30 MiB/s (38x slower) | 8.09 ms |

Measured on 2,000 corpus HTML bodies (20 MiB). `html2text` produces formatted layout text: link footnotes, wrapped tables, and about 86x the output bytes here. Indexing only needs the visible words, so the streaming converter is the right tool. It drops `<head>`, `<style>`, `<script>` and comments, decodes named and numeric entities (including cp1252 mis-encodings), removes zero-width preheader padding, and recovers from an unclosed `<head>`.

## How the latency is achieved (and where it would break)

1. **Date-ordered rowids.** `rowid = date_ms * 1024 + seq` for messages and `message_rowid * 256 + ord` for attachments. FTS5 walks doclists newest-first natively (`ORDER BY rowid DESC`), and date filters become rowid ranges that FTS5 applies inside the doclist walk.
2. **Bounded scoring.** Free-text queries score at most the 5,000 newest matches. When that cap is hit, a second bounded pass (2,000 rows) restricted to subject, sender and filenames adds older mail whose strongest fields match. This is skipped for noise-only queries like "the". Without the cap, `the` costs O(matches), about 250 ms at 300k.
3. **Phrases of common words.** bm25's IDF for a phrase requires evaluating the phrase on every match (61 ms for `"to the"`). A bounded rowid-only precount (about 2 ms) detects that case and ranks by recency instead, because such a phrase has no discriminating IDF anyway.
4. **Hydration and panels are bounded** to 50 hits and 24 attachments. Hydration reads (and decompresses) from a separate `message_bodies` table, so ranking scans stay on small metadata rows.

**Honest caveats.**

- **Truncation.** For an unspecific query matching more than 5,000 messages, an old message that matches *only in its body* can fall outside the scored window. The user sees the best of the newest 5,000 matches plus older subject, sender and filename matches, and adding one more word narrows the set. That is the price of flat latency, and we accept it for known-item search, where recency is a strong prior. Tantivy with block-max WAND would remove this truncation.
- **Ranking weights are starting points.** Ranking quality on this corpus is not meaningful: synthetic Zipf text makes "lease" as common as "people". The weights (subject 4, sender 2.5, filenames 3, body 1, recipients 0.5, cc 0.3, quoted 0.15; recency 0.35·1/(1+age/90d); starred +0.10, unread +0.03, sender-you-write-to up to +0.12, typed-name sender +0.30) must be tuned against real re-finding tasks.
- **Scale.** At 1M messages, retrieval cost stays roughly flat because of the caps. Doclist-merge costs grow linearly: prefix stems like `ther*` and common-word IDF could approach 60–80 ms. If a 1M benchmark confirms that, the Tantivy path applies: SQLite stays canonical, a Tantivy index sits behind `Store::search` with the same query AST (`query.rs` is engine-agnostic), and block-max WAND top-k removes the need for the candidate cap. At 300k, FTS5 meets the target with margin, so we did not take that path.
