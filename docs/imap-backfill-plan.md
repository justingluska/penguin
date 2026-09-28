# IMAP bulk backfill (parked plan)

Status: **not built; no-go for now** (decision 2026-09-24). Revisit only if the
Gmail API quota raise to 60,000 units/min/user is denied after the Sept 26
resubmission. With the raise, the REST API alone backfills ~300k messages in
~5.5 h per account and wins on sustained throughput.

## Why it was considered

REST backfill is bounded by the per-mailbox "Total Query Cost" budget (6,000
units/min) and `messages.get(format=full)` costs ~60 units, so ~95
messages/min per account (~2.2 days for 300k). IMAP `FETCH` isn't metered in
API units, only by Gmail's IMAP download bandwidth cap. Mailspring syncs Gmail
entirely over IMAP; GYB is REST-only.

## Design

1. **Auth.** XOAUTH2 over `imaps://imap.gmail.com:993` (rustls), with an access
   token carrying `https://mail.google.com/` (restricted scope; incremental
   auth + a one-time reconnect per account). IMAP must be enabled on each
   Workspace domain: probe `CAPABILITY` first, fall back to REST if absent.
   External + Testing projects expire refresh tokens after 7 days.
2. **Mailbox.** `LIST` with SPECIAL-USE `\All` (the name is localized),
   `EXAMINE` (read-only), and only `BODY.PEEK[...]` fetches. A plain
   `RFC822`/`BODY[]` fetch sets `\Seen` and would mark the whole mailbox read;
   a test must assert no non-PEEK fetch is ever sent. All Mail excludes Spam
   and Trash, matching REST backfill.
3. **Batches, newest first** (UIDs rise with arrival), 200 UIDs descending:
   - `UID FETCH lo:hi (UID X-GM-MSGID X-GM-THRID X-GM-LABELS FLAGS INTERNALDATE RFC822.SIZE)`
     (~250 B/message). API message id = `format!("{:x}", X-GM-MSGID)`, thread
     id = hex of `X-GM-THRID`: the same ids the REST API uses, so rows merge
     without a mapping table. Dedup with `Store::known_message_ids` (and
     parked ids) before any body bytes move.
   - Bodies for unknown ids: `RFC822.SIZE ≤ 64 KB` → `BODY.PEEK[]`, parsed with
     mail-parser; larger → `BODYSTRUCTURE` + `BODY.PEEK[HEADER]` + only the
     text/plain and text/html sections with `<0.262144>` partial fetches.
     Attachment bytes are never downloaded; metadata comes from BODYSTRUCTURE.
4. **Conversion** reuses `convert.rs` (address/RFC 2047 parsing, Message-ID and
   References, List-Unsubscribe, the Google-MX Authentication-Results check,
   charset decoding, `html_to_text`). Differences: snippet generated from
   body_text; `date` = INTERNALDATE (second precision; the first REST touch
   replaces it with ms); attachment ids stored with a `resolve:` marker that
   `get_attachment` resolves via one `messages.get` + the filename/size
   rematch.
5. **Labels.** `X-GM-LABELS` system flags `\Inbox \Important \Sent \Starred
   \Draft` → INBOX/IMPORTANT/SENT/STARRED/DRAFT; UNREAD = no `\Seen`;
   `\Flagged` → STARRED. User labels arrive as names (modified UTF-7 when
   non-ASCII; needs a decoder) and map to ids via `labels.list`. CATEGORY_*
   aren't exposed: per batch run `UID SEARCH X-GM-RAW "category:promotions"`
   (and updates/social/forums) over the same UID range.
6. **REST stays authoritative.** Record the history id before the IMAP pass
   (as today); history polling runs throughout. Ids touched by history while
   a batch is in flight get `get_message_labels` re-checked after that
   batch's upsert, so an IMAP snapshot can't overwrite a newer label change.
   REST backfill may run in parallel; both skip known ids.
7. **Limits.** Gmail's IMAP download cap (~2,500 MB/day per account) is
   shared with every other IMAP client on the mailbox (e.g. a phone), and
   exceeding it blocks IMAP for up to ~24 h. Budget 1,500 MB/day per account,
   count bytes client-side and persist them with the cursor; at most 2
   connections per account (Gmail allows ~15, shared). On `[OVERQUOTA]` /
   "Account exceeded bandwidth limits" / `BYE`, stop IMAP for that account
   until tomorrow and let REST continue.

## Estimates (per account; text-only IMAP ~12 KB/message, to be measured)

| Path | Rate | 100k | 300k |
|---|---|---|---|
| REST, 6,000 u/min, ~60 u/get | ~95/min | ~18 h | ~2.2 days |
| REST after 60k raise | ~950/min | ~1.8 h | ~5.5 h |
| IMAP (1.5 GB/day budget) | ~125k in the first 10–20 min, then ~125k/day | ~15 min | ~2.5 days |
| IMAP + REST in parallel (6,000 u/min) | ~260k/day | ~15 min | ~1.2 days |

Fetching HTML during backfill (~30 KB/message) cuts IMAP to ~50k/day; the
plan keeps HTML for messages > 64 KB lazy (fetched on open via REST), which
needs a "body_html not fetched" store flag and thread-view support.

## Cost of building it

~2.5–3 days: `imap.rs` (connection, XOAUTH2, special-use, UID batching,
PEEK-only fetches, BODYSTRUCTURE walk, modified-UTF-7 decoder, category
searches, bandwidth ledger), sync wiring, the scope in auth, an IMAP section
in `SyncCursor` (uidvalidity, next UID, bytes today: a types.rs/types.ts
contract change), the lazy-HTML flag, and tests against a scripted in-process
IMAP server (dedup, PEEK-only, labels/categories, UIDVALIDITY change,
bandwidth stop, the history race). New dependency: async-imap (tokio).
