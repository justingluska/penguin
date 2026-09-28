# Search

Penguin searches the index on your Mac (SQLite + FTS5). Nothing is sent anywhere, results update on every keystroke, and every operator below is answered from local data. Nobody needs this page to search: plain words, names and plain English work, with suggestions and chips (the experience and its sources: `docs/SEARCH-UX.md`). For those who like operators, `⌘/` in the search overlay shows this reference with clickable examples, and a typed operator completes its values (people, labels, accounts, dates, sizes).

Parser: `crates/penguin-core/src/query.rs` (operators, grouping) and `dates.rs` (date phrases). Execution: `search.rs`. Plain English: `apps/desktop/src/features/search/nl.ts`. Rules, `penguin-cli search` and the MCP server use the same language (plain English is a search-box feature only).

## Words and phrases

| Query | Matches |
|---|---|
| `lease renewal` | Both words, anywhere (subject, people, body, attachment names). The last word matches as a prefix while you type. |
| `"early termination"` | The exact phrase. Typographic quotes (`“…”`, `„…“`) from a paste or smart quotes work too |
| `-newsletter`, `NOT newsletter` | Leaves a word out. Works on operators and groups too: `-from:ana`, `-has:pdf`, `NOT (a OR b)`. `NOT` must be uppercase (Outlook, Apple Mail) |
| `a OR b` | Either. `OR` must be uppercase and binds tighter than the implicit AND: `from:ana OR from:bob invoice` = (from ana or bob) and invoice |
| `a AND b` | Same as `a b` (uppercase AND is accepted for Gmail habits) |
| `(a OR b) c` | Grouping. Groups nest, and `-( … )` excludes a whole group: `-(is:newsletter OR category:promotions)` |
| `{a b} c` | Gmail's braces: any of the terms inside, `{from:ana from:bob} invoice`; `-{a b}` is none of them |
| `subject:(dinner movie)`, `from:(ana OR bob)` | The operator applies to every term in the parentheses (Gmail) |
| `invoic*` | A prefix: invoice, invoices, invoicing (a trailing `*`, as in Outlook, Fastmail, notmuch and mu; also `subject:renew*`) |
| `rent AROUND 5 june` | Both words at most 5 words apart (Gmail's `AROUND`; `NEAR` is the same; without a number, 10) |

Common words ("the", "about", "sent", "email") are optional when the query has other content, including one still being typed at the end ("what did grace send"). So is the decoration of a pasted subject or link: `RE:`, `Fwd:`, `AW:`, `RV:` and other reply/forward prefixes, a tag like `[External]`, a bare `https://`.

### Word forms and formats

A word also finds the other forms your mail uses, as long as some message contains them (each is looked up in the index first, `crates/penguin-core/src/lexicon.rs`). Quoted phrases and `-exclusions` stay exact.

| You type | Also finds |
|---|---|
| `invoices`, `ships`, `approve` | invoice; shipped, shipping; approved (plurals, -ed, -ing, possessives without the apostrophe: `sofias party`) |
| `color`, `organize`, `center`, `license`, `catalog`, `canceled`, `gray` | colour, organise, centre, licence, catalogue, cancelled, grey (and back) |
| `wi-fi`, `bike pick up`, `oncall`, `leaserenewal` | wifi, pickup, on-call, lease renewal |
| `odegard`, `hauptstrasse`, `mueller` | Ødegård, Hauptstraße, Müller (letters the index doesn't fold) |
| `INV20417`, `SK2210`, `1Z4F8A62…` | INV-20417, SK 2210, a tracking number printed in groups |
| `2450`, `$2,450.00`, `1234.56` | $2,450; €1.234,56 |
| `4155550138`, `+1 415 555 0138`, `020 7946 0958` | (415) 555-0138; +44 20 7946 0958 |
| `2.14.3` | v2.14.3 |
| `rob kowalski`, `bob` | Robert Kowalski: a given name also finds its common short forms and back, when someone you have mail with has that name |
| `invioce`, `crestlnie` | invoice, crestline: a word no message contains is corrected to the indexed word one edit away that occurs with your other words |

Messages that match the words as typed are ranked on those words alone; the other forms only bring in mail that uses them.

Other mail apps' spellings are accepted: `deliveredto:` (Gmail) is `to:`, `participants:` (Outlook) is `with:`, `attachment:` / `attachmentnames:` are `filename:`, and `hasattachment:yes` / `hasattachments:true` is `has:attachment`.

## People

| Operator | Matches |
|---|---|
| `from:mike` | Sender name or address (`from:mike@acme.example`, `from:@acme.example`) |
| `to:ana` | Any recipient: To, Cc or Bcc (`bcc:` is the same) |
| `cc:ana` | Cc only |
| `with:priya` | Sender or any recipient (`participant:` is the same) |
| `domain:acme.example` | Anyone at a domain, as sender or recipient (`@` optional) |
| `from:me` | Mail you sent or drafted (any of your accounts) |
| `to:me` | Mail with one of your addresses among the recipients, `+tag` copies included (`you+shopping@…`) |
| `is:new-sender` / `from:new` | The first message you ever received from that address. With a date, "whose first contact happened then": `is:new-sender date:"last week"` |
| `is:first-outbound` / `to:new` | A message you sent that was the first you ever sent to at least one of its recipients |
| `is:known-sender` / `from:known` | Received from an address you have written to (the People you know view as an operator). `-is:known-sender` keeps people out of an auto-archive rule |

First contacts count only mail stored on this Mac (see Settings → Sync: with a sync window and no older headers, "first" means first within what is downloaded). Spam doesn't count as a first contact, and drafts don't count as sent.

## Replies and threads

| Operator | Matches |
|---|---|
| `is:unanswered` | Received (not bulk mail), and nothing was sent in its thread after it. `is:unanswered from:nick date:"this month"` = replies you owe Nick |
| `is:replied` | Received, and you sent something in its thread afterwards |
| `is:awaiting` | You sent it and nothing arrived (or was sent) in its thread since: you're waiting on them |
| `is:reply` | Not the first message of its thread |
| `messages:>5` | Threads by length: `>5`, `>=5`, `5+`, `<3`, `<=3`, `3`, `2..4` |

## Dates

Free text is never a date: "february" searches the word (the box offers "Use 'february' as a date?"). Dates come from these operators, in your timezone:

| Operator | Matches |
|---|---|
| `date:february` | The most recent February (months and weekdays mean the latest past one; `last february` the one before) |
| `date:"august 27"`, `date:aug27` | One day |
| `date:"last week"` | Also `today`, `yesterday`, `this week`, `this month`, `last month`, `this year`, `last spring`, `past 2 weeks`, `3 days ago`, `late february`, `the week of feb 10`, `since march`, `between jan and march` |
| `date:aug1..aug15` | A range (`..`, `to` or `-`): `date:2026-08-01..2026-08-15`, `date:"jan 5 to jan 20"` |
| `date:2026-02`, `date:2026-02-10`, `date:2025` | Numeric month, day or year |
| `date:"18 august 2026"`, `date:"18th of august"`, `date:"aug. 18"` | Day first, and abbreviations with a period |
| `on:8/18/26`, `on:18/08/2026`, `on:18.08.2026` | Month first like Gmail, unless only day first is a real date; dotted dates are day first (Germany); two-digit years are 20yy |
| `date:"two weeks ago"`, `date:"a couple of weeks ago"`, `date:"a week ago"` | Numbers in words |
| `date:"last weekend"`, `date:"this weekend"` | Saturday and Sunday of last week or this week |
| `date:"last quarter"`, `date:"this quarter"`, `date:q2`, `date:"q2 2025"` | Calendar quarters (`q2` alone: the most recent Q2) |
| `after:1790000000` | Unix seconds: an exact instant (Gmail) |
| `on:2026-08-27` | Same as `date:` |
| `before:2026-08-27`, `before:"aug 27"` | Before the start of that day (numeric dates or any date phrase) |
| `after:2026-08-01`, `since:"last week"` | From the start of it (inclusive, like Gmail's `after:`) |
| `until:2026-08-27` | Up to and including it |
| `newer_than:3d`, `older_than:1y` | Relative: `h`, `d`, `w`, `m` (months), `y` |
| `day:monday`, `day:weekend`, `day:weekday`, `day:mon,wed` | Messages sent on those weekdays (any week) |

An unreadable `date:` shows a "Couldn't read that date" chip and constrains nothing.

## Files and content

| Operator | Matches |
|---|---|
| `has:attachment` | Any attached file (inline images don't count) |
| `has:pdf`, `has:image`, `has:doc`, `has:spreadsheet`, `has:presentation` | A file of that kind |
| `has:invite` | A calendar invitation (text/calendar part or .ics) |
| `filename:invoice` | Words in an attachment name (3+ characters match anywhere in the name); `filename:"q3 report"` needs each word in one name, whatever joins them (Q3_report.xlsx) |
| `larger:5M`, `smaller:200K`, `size:>5M`, `size:<1M` | Total size of the attached files (`K`, `M`, `G`; bare numbers are bytes, as in Gmail). Bodies aren't counted |
| `subject:"q3 plan"` | Words in the subject |
| `has:link` | `http`, `https` or `www` in the message's own text |
| `has:otp` | A detected verification code or sign-in link (scanned for recent mail) |
| `has:unsubscribe` | A List-Unsubscribe header |

## Status and place

| Operator | Matches |
|---|---|
| `is:unread`, `is:read`, `is:starred`, `is:unstarred`, `is:important` | Message state |
| `is:snoozed`, `in:snoozed` | In a snoozed conversation |
| `is:newsletter` | Bulk mail: List-Unsubscribe, or the Promotions, Updates or Forums category |
| `in:inbox`, `in:sent`, `in:drafts`, `in:done` (archived), `in:starred`, `in:important` | Folders (`folder:` is the same; `is:sent`, `is:draft`, `is:inbox` too) |
| `in:trash`, `in:spam`, `in:anywhere` | Trash and Spam are left out unless asked for |
| `label:finance` | A label by name, nested name (`clients/acme` or `clients-acme`), its last segments (`atlas/finance` for Work/Atlas/Finance), or, when nothing has that name, a word of it (`label:fernhill`). `label:tax docs 2025` without quotes reads the words as the label when they name one |
| `category:promotions` | `primary`, `promotions`, `social`, `updates`, `forums` |
| `account:work` | One of your accounts (address, prefix, nickname or name) |
| `type:event`, `is:event`, `in:calendar` | Calendar events only (dates apply to the event's start) |

## Plain English

Typing a phrase instead of operators works when it reads clearly as one. The filters it understood appear as chips under the box ("From Mike Delgado", "This year", "Has PDF"); removing a chip removes the words that made it, and **Just the words** goes back to a plain word search for that query. The operator query below is what runs; the box keeps your words.

| You type | Runs |
|---|---|
| new senders last week | `is:new-sender date:"last week"` |
| emails I sent on august 27 | `in:sent date:"august 27"` |
| `date:"august 27"` sent | `date:"august 27" in:sent` |
| unanswered from nick this month | `is:unanswered from:nick date:"this month"` |
| pdfs from acme.example | `has:pdf from:acme.example` |
| people I emailed for the first time last month | `is:first-outbound date:"last month"` |
| waiting for a reply | `is:awaiting` |
| awaiting reply to priya | `is:awaiting to:priya` |
| large attachments from dana | `larger:5M from:dana` |
| pdfs or spreadsheets from dana | `has:pdf OR has:spreadsheet from:dana` |
| unread newsletters | `is:unread is:newsletter` |
| long threads with ana | `messages:>5 with:ana` |
| emails on weekends from nick | `day:weekend from:nick` |
| mail from nick last week | `from:nick date:"last week"` |

It is conservative: a phrase is rewritten only with a clear cue (a state such as "unanswered" or "new senders", "I sent", a plural file type, "unread"…) or a person together with a date or a file type. A date alone ("february"), a name alone (unless it's someone you correspond with), or "the pdf mike sent" (Mike's mail, not yours) stay plain searches. Queries with `OR` or parentheses are never rewritten; operators, quotes and exclusions you typed pass through. Questions ("when did I last email Priya?") go to Ask instead.

## Search by meaning

When search by meaning is set up, the same box also finds mail by what it says, not only by the words in it: "plane tickets to portugal" finds the airline's "Your trip is booked: Boston to Lisbon", and "that thing about ending the lease early" finds the landlord's note on early termination. It runs on your Mac with a local model; nothing is sent anywhere.

- Keyword and meaning results are one list, ranked together. How much meaning counts depends on the query. Codes, numbers, addresses and single words lean on the exact words. Descriptions and questions lean on meaning. Names of people you correspond with, quoted phrases, `OR`/`( )` and filter-only queries use the exact words only, as before.
- Every operator filters meaning results exactly as it filters word results (`from:`, dates, labels, `-word`, Trash and Spam left out…).
- A result found by meaning shows its best-matching passage, with any of your words that occur in it highlighted.
- A date typed as plain words ranks instead of filtering: "dentist last spring" shows dentist mail from last spring first, then the rest. `date:"last spring"` still filters.
- A misspelled word that no message contains is matched as the closest word your mail does contain ("leese renewal" finds "lease renewal").
- While the meaning index is still being built, results are word results only, and the response says how far indexing has got.

How it ranks, the sources behind it, and measurements: [SEARCH-RANKING.md](SEARCH-RANKING.md).

## Also search Gmail

"Also search Gmail" (`⌘⇧↵`) translates the query to Gmail's syntax for mail that isn't downloaded. `from:me`, `to:me`, `larger:`/`smaller:`, `with:`/`domain:` (as `{from: to: cc: bcc:}`), groups, dates, folders, labels and file types translate; the local-only operators (`is:new-sender`, `to:new`, `is:known-sender`, `is:unanswered`, `is:replied`, `is:awaiting`, `is:reply`, `is:newsletter`, `is:snoozed`, `has:link`, `has:otp`, `has:unsubscribe`, `messages:`, `day:`) are dropped and the Gmail results are marked approximate.

## Speed

Every operator is an indexed lookup or a bounded walk; none scans message bodies. First contacts come from a derived table, `first_contacts(email, dir, msg)` (plus `sent_recipients(email, msg)` for recomputing), maintained on every insert, delete and relabel (`crates/penguin-core/src/store_contacts.rs`). Reply states check the thread through the `messages_thread` index. Timings at 300,000 messages are in `cargo run -p penguin-core --release --example bench_search`. Relevance (does the right conversation come first?) is measured by `crates/penguin-eval`: see `docs/SEARCH-EVAL.md`.

## Not supported (yet)

- `list:` (mailing-list id): Penguin doesn't store the List-Id header. `is:newsletter` covers most uses.
- `deliveredto:`, `rfc822msgid:`, `has:drive`/`has:youtube`, `is:muted`, `is:chat`.
- Message size including the body (`larger:` counts attachments).

## Design notes

What other tools do, and what Penguin took:

- **Gmail**: the operator vocabulary (`from:`, `has:`, `is:`, `in:`, `larger:`, `older_than:`, `OR`, `{ }`, `( )`, `-`). Penguin keeps its spelling so muscle memory transfers, and adds the relationship operators Gmail lacks.
- **Outlook (KQL)** and **Fastmail**: explicit `AND`/`OR`/`NOT` and grouping, relative dates, size ranges with `..`. Penguin accepts `AND`, `( )`, `-( )` and `a..b` ranges.
- **Superhuman**, **Spark smart search**, **Apple Mail** and **HEY**: natural phrases ("attachments from John last week") turned into visible filters/tokens the user can remove. Penguin does the same deterministically and shows the operator query, so there's nothing hidden to trust.
- **Linear**, **GitHub** and **Slack**: `key:value` filters with autocomplete of both keys and values (people, labels), negation with `-`, `is:`/`has:` families, a filter cheat sheet one key away. Penguin's autocomplete and `⌘/` tips follow that.
- **Raycast**: everything reachable from the keyboard, hints inline. Tab takes the first suggestion, a completion or a date hint.
