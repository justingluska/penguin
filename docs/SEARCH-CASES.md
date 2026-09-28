# What people type into mail search: edge cases

This page lists the kinds of queries people type into mail search, and the edge cases inside each kind. It draws on:
- the query languages of Gmail, Outlook, Apple Mail, Fastmail and Proton;
- the query languages and test suites of open-source clients;
- research on email search logs;
- forum threads where a search failed.

Each case is a judged query in `crates/penguin-eval` (`src/edge_queries.rs`, targets in `src/corpus/edge.rs`). Methodology is in [SEARCH-EVAL.md](SEARCH-EVAL.md). The fixes these queries led to, and what is still out of reach, are at the end.

`[Sn]` refers to the source list at the bottom.

## What the research says people type

- **Email queries are short and aimed at one item.**
  - Outlook web logs (1.4 M sessions, 2.0 M queries): 1.59 words on average, against 3.16 on the web. 84.6% of surveyed searches had a specific need, and 68.9% were after one specific email. People remember the content (62.9%), the sender (52.1%), the subject (27.6%) and a rough time (20.3%) [S25].
  - Yahoo Mail: 69.5% of queries are one term, 21.1% two, and 9.4% three or more [S30]. About 55% are contact queries [S31]. Kuzi et al. split queries into person ≈40%, company ≈10% and content ≈50% [S29].
  - Thunderbird users (47 people over 4 months): 94% of queries were single words, "many were only partial words" (`jen`, `virt`, `mar`) [S27].
- **Operators are a minority, and mostly `from:`.** 18% of Outlook queries use advanced syntax: `from:` in 13.6%, `to:` in 2.7% [S25]. Kim et al. counted `from:` at 72% of operator uses [S26]. A later lab study saw operators in under 1% of queries [S28].
- **Queries repeat, and typos are common.** 38–45% of queries are repeated later [S25][S27][S31]. Fixing a typo accounts for about 19% of reformulations [S28]. More than 15% of surveyed searches failed [S25].
- **Longer queries do better, and time order fails on old mail.** MRR rises with query length, and ranking by date misses more as the target gets older [S28]. Relevance ranking beats pure time ranking in about twice as many cases as the reverse [S30].

## The taxonomy

Each group below is one category of the eval (`search-eval run` reports per category). The count is the number of judged queries.

### Syntax (36): logic, quotes, grouping, prefixes, malformed input

| Case | Examples in the eval | Expected, and who defines it |
|---|---|---|
| OR precedence and grouping | `from:marco OR from:julia invoice`, `(from:marco OR from:julia) invoice`, `((lease OR parking) (renewal OR spot)) from:mike` | OR binds tighter than the implicit AND (Gmail [S1]) |
| Gmail braces | `{from:marco from:julia} invoice`, `invoice {bianchi fernhill}` | `{ }` is OR [S1] |
| Operator groups | `subject:(lease renewal)`, `from:(marco OR julia) invoice` | The operator applies to each term [S1][S5] |
| Negation forms | `invoice -(crestline OR bianchi)`, `invoice NOT crestline`, `-(from:theo OR from:marco) invoice`, `from:mike -delgado` | `-` [S1]; uppercase `NOT` [S5][S6][S9] |
| Case of logic words | `lease or renewal`, `lease and renewal`, `invoice AND crestline` | Lowercase `or`/`and` are words; uppercase is logic [S5][S6][S11] |
| Phrases | `"renewal agreement" "12 months"`, `"renewal lease"` (expect nothing), `“early termination”` | Adjacent and in order [S12]; typographic quotes from a paste |
| Exact-word `+` | `+lease +renewal` | Gmail `+word` [S1] |
| Trailing wildcard | `invoic* crestline`, `deskcr*` | Prefix `*` [S5][S6][S11][S14][S16] |
| Proximity | `rent AROUND 5 june` | Gmail `AROUND`, KQL `NEAR` [S1][S5] |
| Malformed input | `(lease renewal`, `lease renewal)`, `"early termination`, `lease from:`, `lease renewal OR`, `OR lease renewal`, `from: theo past due`, `FROM:theo SUBJECT:invoice` | Recover and search what can be read, as mu does (`a and (b` → `a AND b`, trailing operator dropped) [S17] |

### Place (44): labels, folders, states, categories, accounts, Trash and Spam

| Case | Examples | Expected |
|---|---|---|
| Labels with spaces | `label:"tax docs 2025"`, `label:tax-docs-2025`, `label:tax_docs_2025`, `label:tax docs 2025` (no quotes) | Gmail's hyphen form (third-party sources only, unverified [labnol]); the unquoted form is what people type |
| Nested labels | `label:"clients/fernhill bakery"`, `label:clients-fernhill-bakery`, `label:"fernhill bakery"`, `label:fernhill`, `label:work/atlas/finance`, `label:atlas/finance`, `label:finance` | Full path or unique leaf (Fastmail `in:` [S11]) |
| Hyphen in the label's name | `label:follow-up`, `label:"follow up"` | Collides with Gmail's space→hyphen convention |
| Folders and states | `in:inbox from:priya`, `is:unread`, `in:starred`, `is:starred budget`, `in:sent to:julia`, `is:sent to:julia`, `-in:inbox lease renewal`, `in:done lease renewal` | [S1][S11] |
| Trash and Spam | `in:trash comeback`, `in:bin comeback`, `in:anywhere comeback offer`, `in:spam gift card`, `in:junk prize`, `"comeback offer"` and `"prize claim form"` (expect nothing) | Left out unless asked for (`in:anywhere` [S1], `in:*` [S11]) |
| Categories | `category:social trailmates`, `category:promotions skyward`, `category:updates skyward`, `category:primary jess` | [S1] |
| Accounts | `account:personal flight`, `account:hello@morenostudio.example julia`, `account:work atlas budget` | |

### Attachment (33)

| Case | Examples | Expected |
|---|---|---|
| Kinds | `has:presentation`, `has:doc from:kowalski`, `has:document will draft`, `has:image washing machine` (a HEIC), `has:spreadsheet ledgerly` (a CSV), `has:pdf from:marco` (a PDF sent as application/octet-stream) | [S1][S11][S16] |
| File names | `filename:pptx`, `filename:Q3_report_final_v2.xlsx`, `filename:"q3 report"`, `filename:q3`, `filename:"Scan 2026-03-14"`, `filename:INVITATION-LETTER-TOKYO.PDF`, `filename:.docx`, `filename:heic` | Extension or name part [S1]. mu matches `custer.jpg` but not `custer` [S17] |
| Sizes | `larger:25M`, `larger:10mb`, `size:>10M`, `size:30000000`, `smaller:100K has:pdf from:ledgerly`, `larger:15M zip` | Gmail `size:` is bytes [S1]; units vary by product [S7][S16][S17] |
| Other products' spellings | `hasattachment:yes lease`, `attachment:roadmap` | Outlook/KQL [S6][S7] |

### Date (37)

| Case | Examples | Expected |
|---|---|---|
| Formats | `after:YYYY/MM/DD before:…`, `on:MM/DD/YYYY`, `date:YYYY-MM-DD`, `date:"August 18, 2026"`, `date:"18 August 2026"`, `date:"18th of August"`, `date:"Aug. 18"`, `date:8/18/26` | Gmail YYYY/MM/DD and MM/DD/YYYY [S1]; himalaya and notmuch read day-first forms [S14][S19] |
| Day-first ambiguity | `on:18/08/2026`, `on:18.08.2026` | Gmail reads month first, himalaya day first [S19]; a date only one reading allows is unambiguous |
| Local midnight | `kestrel on:<day>` for mail at 23:50 and 00:10, `after:`, `before:`, `until:` | Local time here. Gmail's API uses PST midnight [S2]; Purview and KQL use UTC [S5][S6] |
| Unix seconds | `kestrel after:<epoch>` | Gmail's documented workaround for its time zone [S2] |
| Relative | `newer_than:Nd`, `older_than:30d`, `since:"6 weeks ago"`, `date:"last 43 days"`, `date:"past 6 weeks"` | [S1][S11] |
| Numbers in words | `date:"two weeks ago"`, `date:"a couple of weeks ago"`, `compost two weeks ago` | notmuch parses "two mo" [S15] |
| Weekends and quarters | `date:"last weekend"`, `date:"last quarter"`, `date:"q2 2026"` | AQS "this week", "past month" [S8] |
| Open windows | `from:swiftcab since:july`, `movers older_than:<Nd>` (expect nothing) | |

### People (47)

| Case | Examples | Expected |
|---|---|---|
| Diacritics | `jose nunez`, `José Núñez`, `Jose Núñez`, `núñez`, `from:nunez`, `siobhán`, `siobhan` | Folded: mu `creme fraiche` = `crème fraîche` [S17], gloda `paris` = `París` [S21] |
| Letters that don't decompose | `soren odegard`, `odegard` (Ødegård), `hauptstrasse 12` (Straße), `juergen mueller` (Jürgen Müller) | ø and ß have no Unicode decomposition |
| CJK | `田中`, `from:田中`, `misaki tanaka` | Every CJK bigram should match (gloda [S21]) |
| Apostrophes | `o'brien`, `obrien`, `O’Brien`, `from:o'brien` | Exchange indexes O'Brien as a unit [S50]; a client bug strips it [S51] |
| Hyphens, and names joined | `anne-marie`, `anne marie dubois`, `annemarie`, `atelier-lune.example` | |
| Nicknames | `bob kowalski`, `robert kowalski`, `rob kowalski` | No product documents nickname matching |
| Display name vs address | `revans`, `dr evans`, `Dr. Ruth Evans` | [S15] |
| Plus-addressing | `to:alexmoreno+shopping@mailbox.example`, `alexmoreno+shopping`, `to:+shopping`, `deliveredto:…`, `to:me lumen lamps` | Gmail's `to:me` includes plus tags [S38]; `deliveredto:` [S1] |
| Domains and subdomains | `domain:ledgerly.example`, `from:@notify.ledgerly.example`, `ledgerly.example`, `domain:northwind.example tokyo`, `müller-bau.example` (IDN) | Fastmail: `from:@x` is exact, `from:x` includes subdomains [S11]; Gmail includes subdomains [S39] |
| Emoji and symbols | `🎉`, `🎉 party`, `birch & bloom`, `birch and bloom`, `birchandbloom`, `birch&bloom`, `thistle & page` | Emoji subjects can't be searched in Gmail [S42]; `A&B` [S17] |

### Format (40): identifiers typed another way

| Case | Examples | Expected |
|---|---|---|
| Tracking numbers | `1Z4F8A620311872290` (printed in groups), `1Z 4F8 A62 03 1187 2290`, `9400 1118 9956 2537 8663 61` (printed as one run) | [S41][S52] |
| Codes without punctuation | `INV20417`, `inv 20417`, `#20417`, `DC55120`, `order #DC-55120`, `SK2210`, `NA883104` | Outlook can't find `24681` in `PO24681` [S48] |
| Amounts | `2450`, `$2450`, `$2,450.00`, `18400`, `$18,400.00`, `$1.2M atlas`, `9% expansion` | Gmail ignores currency symbols [S37] |
| European amounts | `1600 euros`, `€1,600`, `1234.56`, `1.234,56 €`, `1,234.56` | |
| Phone numbers | `4155550138`, `415-555-0138`, `415.555.0138`, `+1 415 555 0138`, `020 7946 0958`, `+44 20 7946 0958`, `915550123` | Indexed "as entered"; a country code doesn't match (AQS [S8]) |
| URLs, versions, case numbers | `https://portal.fjordsoft.example/v2/migration-guide`, `portal.fjordsoft.example`, `fjordsoft.example/v2`, `migration-guide`, `2.14.3`, `v2.14.3`, `CX-2026/0419`, `cx 2026 0419`, `security@northwind.example` | A link behind link text [S36] |

### Morphology (38): word forms

| Case | Examples | Expected |
|---|---|---|
| Plurals and inflections | `crestline invoices`, `receipt swiftcab`, `harbor mobile bills`, `cedar valley statements`, `ledgerly verification codes`, `deskcraft orders`, `lamp ships`, `gym cancel`, `approve atlas budget`, `refunding headphones`, `lab result` | notmuch and Fastmail stem English (`detailed` = `detail`, `bus` = `buses`) [S11][S14]; HN reports Gmail's "invoice" missing "invoices" [S52] |
| Derivation | `renewing lease` (the mail says "renewal") | Left to search by meaning |
| British vs American | `order canceled`, `favorite titles`, `autumn catalog`, `color proofs`, `gray flour bag`, `organize a call julia`, `driving license renewal`, `licensing center`, `theater tickets`, `neighbor dana`, `favorite sunset` | No product documents this equivalence |
| Compounds and hyphens | `oncall handoff`, `lisbon checkin`, `wi-fi password`, `bike pick-up`, `bike pick up`, `laptop setup`, `phishing e-mails` | notmuch: `a-list-of-words` = `"a list of words"` [S14]; mu `kata-containers` [S17] |
| Possessives without the apostrophe | `sofias party`, `bens wedding` | |
| CJK inside a run | `会議室`, `東京オフィス`, `会議室 room b` | gloda indexes bigrams [S21]; unicode61 doesn't |

### Messy (35)

| Case | Examples | Expected |
|---|---|---|
| Very short | `vet`, `pto`, `msa`, `cpa` | Short known-item queries dominate [S25][S27] |
| Very long or pasted | a whole sentence from the lease, a rambling description, a question as a sentence | KQL caps length at 2,048 characters [S5] |
| Pasted subjects | `Re: Invoice MS-2026-007 — Bianchi Wines`, `Fwd: Signed MSA attached`, `RE: FW: Past due: invoice INV-20417`, `[External] Past due: …`, `AW: …` (German), `RV: …` (Spanish), `Re: Re: Re: …` | mu's `subject:Re: Learning LISP…` is a known-hard case [S17] |
| Punctuation and case | `lease renewal!!!`, `lease renewal???`, `...lease renewal...`, `lease/renewal`, `lease, renewal;`, `lease-renewal`, `LEASE RENEWAL`, `LeaseRenewal`, extra spaces, a tab, `lease renewal 🙏`, `lease renewal https://`, `lease has:` | `foo & bar` → `foo AND bar` [S17] |
| Typos | transpositions (`crestlnie`, `plumebr`), adjacent keys (`invoixe`), doubled letters (`retreeat`), a missing space (`leaserenewal`), a split word (`lea se renewal`) | About 19% of reformulations fix a typo [S28] |
| Apostrophes | `can't wait to celebrate`, `can’t wait to celebrate` | |

### Mixed (31): operators with plain English, and conflicting filters

| Case | Examples | Expected |
|---|---|---|
| Operator plus description | `from:mike the pdf about the lease`, `has:pdf what did grace send`, `in:sent the logo files i sent julia`, `from:jess photos last summer`, `label:"tax docs 2025" refund amount`, `account:work what's my asset tag`, `from:kwame money approved for the project`, `account:personal plane tickets to portugal`, `hotel in lisbon -nestaway` | Carmel's `from:<person> <content word>` pattern [S30] |
| Filter across languages | `from:carmen recipe`, `from:carmen receta` | |
| Conflicting or impossible filters (expect nothing) | `in:sent from:theo`, `has:pdf -has:attachment`, `lease before:2015-01-01`, `invoice after:<future>`, `older_than:30d newer_than:10d`, `is:unread is:read`, two date windows that don't overlap, `from:priya from:kwame budget`, `label:nonexistent invoice`, `in:trash lease renewal`, `"quarterly unicorn review"`, `from:zelda invoice`, `account:school budget`, `larger:1G` | Nothing. mu swaps a reversed date range [S17]; Gmail doesn't document its behaviour. A search by meaning must not leak results past a filter that nothing passes |

### Recall (29): descriptions, questions, other languages

Examples:
- **Descriptions:** "rent going up" (it once suggested the sender UPS), "that email where mike said the rent was going up", "the one with the door code", "the email about the boiler".
- **Questions:** "who is our contact at nordlys?", "what's the tracking number for the lamp?", "when are the movers coming?".
- **Other languages:** "photos from the cadiz trip" (Spanish mail), "reserva de sala de reuniones en tokio" (Japanese mail), "réservation hôtel madrid", "delivery address in munich" (German mail), "cuánto cuesta el piso en ruzafa".

These follow the survey's "what people remember": content first, then the sender [S25]. They follow Apple's "search by what you mean" examples too [S9][S10].

## Results

See [SEARCH-EVAL.md](SEARCH-EVAL.md#baseline-and-the-edge-case-loop) for the before/after tables by category. They cover keyword search, hybrid search with EmbeddingGemma and with the hash stand-in, and Ask. Latency is there too.

## What was fixed, and why it failed

Each fix has unit tests, and search-eval measured each one. Every fix is a separate commit.

| Fix | Root cause |
|---|---|
| Gmail braces, `key:(…)` groups, uppercase `NOT`, `AROUND`/`NEAR`, a trailing `*`, typographic quotes, `deliveredto:` and other products' operator names (`query.rs`) | The parser only knew `( )`, `OR`, `-` and straight quotes. Anything else became plain words that matched nothing |
| A noise word at the end is optional | `has:pdf what did grace send` required the prefix `send*`: the word still being typed was never dropped |
| Word forms (`lexicon.rs`): inflections, British/American spellings, compounds and hyphens, letters that don't fold, numbers and codes in another format, typos in keyword search, nicknames | FTS5 `unicode61` has no stemming and no compound handling. It doesn't fold ø or ß, and it indexes a number or code exactly as the mail printed it. Each variant is checked with one seek in the index |
| Matches on the typed words are scored first (`Plan::fts_exact`), and variant-only matches weigh 0.8 | bm25 adds up every matching alternative, so a message with both "report" and "reports" outranked the one described exactly |
| No bases under four letters from -ed/-ing | "wedding" gave "wed", the weekday in every calendar invitation |
| Dates: day first, dotted, two-digit years, numbers in words, weekends, quarters, Unix seconds (`dates.rs`) | The grammar had none of these, and an unreadable `date:` constrains nothing |
| Labels: trailing segments, a word of the name, names typed without quotes | Only the exact name or the last segment resolved |
| Pasted-subject decoration (`RE:`, `Fwd:`, `AW:`, `[External]`, `https://`) is optional | The noise check compared the whole text `fwd:` with the word `fwd` |
| `to:me` covers `+tag` copies, with a migration to backfill stored mail | `F_TO_ME` compared recipients with account addresses exactly |
| `filename:` with several words | The trigram index matched the space literally ("q3 report" vs Q3_report) |
| With meaning on, a rare co-occurrence of all the words gets +0.15 | A keyword match that only 1–3 conversations share tied the model's nearest neighbours, and lost to them on recency |
| Hybrid respelling leaves joins and folds to the lexicon | "sour dough" was respelled "your rough" before the lexicon could join it |
| Natural-language chips: a word that reads as a date is never a person, and pasted prose isn't a command | "…send it back by May 15" became `from:May` |
| A one-letter word being typed is looked up exactly | A one-letter prefix query read every term starting with that letter (500 ms at 300k messages) |
| A CJK word never panics | Respelling sliced a string by bytes |

## What remains, and why

These are measured by the eval and not fixed:
- **CJK words inside a run** (`会議室`, `東京オフィス` except as the last word typed). `unicode61` indexes a whole run of CJK characters as one token. The fix is to index CJK as bigrams, like gloda [S21], which means re-tokenizing every stored message: a migration that rebuilds `messages_fts` from `message_bodies`. With meaning on, the model finds the Japanese mail from an English description.
- **An emoji alone** (`🎉`). `unicode61` drops symbols, so the query has nothing to search. Emoji would have to be indexed as tokens, which also needs a re-index. Gmail can't do this either [S42].
- **Derivations** (`renewing` for "renewal") and **real-word typos** (`fishing emails` for phishing). Both change the word, not its form, so they are left to search by meaning. The model gets `renewing lease` right, but misses `fishing emails`.
- **Long descriptions and questions in keyword-only search.** Every content word is required, so a rambling sentence finds nothing without meaning. Relaxing that ("most words must match") would change what every multi-word query means. With a model, these route to meaning.
- **Dates written as plain words in keyword-only search** (`compost two weeks ago`). Free text is never a date filter (SEARCH.md). With meaning on, they become a soft window, and the UI offers "Use … as a date?".
- **Date-only queries** (`on:2026-08-18`) score 0.67. They list the day newest first, as specified, so the planted message isn't always first.
- **Transliteration beyond single letters** (`soeren oedegaard`) and nicknames outside the table.
- **Ambiguous `from <word>` in plain English.** `photos from cádiz` becomes `from:cádiz`, because the grammar can't tell a place from an unknown sender. The chip shows the reading, and one click undoes it.
- **Ranking with meaning** on some paraphrases, questions and Spanish queries (`company offsite`, `when am I on call?`, `factura de la luz`). This is the model, not the grammar: see [SEMANTIC.md](SEMANTIC.md).

## Sources

The research was done on 2026-09-27. Pages that couldn't be read are marked.

**Product documentation**
- [S1] Gmail search operators: https://support.google.com/mail/answer/7190
- [S2] Gmail API, filtering (dates are "midnight … in the PST timezone"; epoch seconds): https://developers.google.com/workspace/gmail/api/guides/filtering
- [S3] Stack Overflow 33912834 (API vs UI dates): https://stackoverflow.com/questions/33912834
- [S4] Stack Overflow 70447366 (time zones): https://stackoverflow.com/questions/70447366
- [S5] SharePoint KQL syntax reference: https://learn.microsoft.com/en-us/sharepoint/dev/general-development/keyword-query-language-kql-syntax-reference
- [S6] Microsoft Purview keyword queries: https://learn.microsoft.com/en-us/purview/ediscovery-keyword-queries-and-search-conditions
- [S7] Outlook, narrow your search: https://support.microsoft.com/en-us/office/learn-to-narrow-your-search-criteria-for-better-searches-in-outlook-d824d1e9-a255-4c8a-8553-276fb895a8da
- [S8] Windows Advanced Query Syntax: https://learn.microsoft.com/en-us/windows/win32/lwef/-search-2x-wds-aqsreference
- [S9] Apple Mail for Mac, search: https://support.apple.com/guide/mail/search-for-emails-mlhlp1003/mac (and its older version for the natural-language examples)
- [S10] Apple Mail for iPhone, search: https://support.apple.com/guide/iphone/search-for-email-iphb2eab8035/ios
- [S11] Fastmail, searching your mail: https://www.fastmail.help/hc/en-us/articles/360060591213-Searching-your-mail
- [S12] Proton search: https://proton.me/support/search
- [S13] Proton message content search: https://proton.me/support/search-message-content

**Open-source query languages and their tests**
- [S14] notmuch-search-terms(7): https://notmuchmail.org/doc/latest/man7/notmuch-search-terms.html
- [S15] notmuch tests (T080, T081, T100, T110, T120, T300, T490, T500, T650, T660, T740): https://github.com/notmuch/notmuch/tree/master/test
- [S16] mu-query(7): https://github.com/djcb/mu/blob/master/man/mu-query.7.org
- [S17] mu query parser and tests (`lib/mu-query-parser.cc`, `lib/mu-query-processor.cc`, `mu/tests/test-mu-query.cc`, `lib/tests/test-mu-store-query.cc`): https://github.com/djcb/mu
- [S18] aerc-search(1): https://git.sr.ht/~rjarry/aerc/tree/master/item/doc/aerc-search.1.scd
- [S19] himalaya / pimalaya search grammar and tests: https://github.com/pimalaya/core/blob/master/email/src/email/search_query/filter/grammar.abnf
- [S20] meli search (`melib/src/search.rs`, `meli.1`): https://github.com/meli/meli
- [S21] Thunderbird gloda tokenizer and internationalization tests (`test_intl.js`, `test_fts3_tokenizer.js`): https://github.com/mozilla/releases-comm-central/tree/master/mailnews/db/gloda/test/unit
- [S22] Thunderbird quick filter: https://github.com/mozilla/releases-comm-central/blob/master/mail/modules/QuickFilterManager.sys.mjs

**Research**
- [S23] Craswell, de Vries & Soboroff, Overview of the TREC-2005 Enterprise Track: https://trec.nist.gov/pubs/trec14/papers/ENTERPRISE.OVERVIEW.pdf
- [S24] Soboroff, de Vries & Craswell, Overview of the TREC 2006 Enterprise Track: https://trec.nist.gov/pubs/trec15/papers/ENT06.OVERVIEW.pdf
- [S25] Ai, Dumais, Craswell & Liebling, Characterizing Email Search using Large-scale Behavioral Logs and Surveys, WWW 2017: https://doi.org/10.1145/3038912.3052615
- [S26] Kim, Craswell, Dumais, Radlinski & Liu, Understanding and Modeling Success in Email Search, SIGIR 2017: https://doi.org/10.1145/3077136.3080837
- [S27] Harvey & Elsweiler, Exploring Query Patterns in Email Search, ECIR 2012: https://epub.uni-regensburg.de/22703/1/ecir2012_email.pdf
- [S28] Mackenzie, Gupta, Kiseleva et al., Exploring User Behavior in Email Re-Finding Tasks, WWW 2019: https://www.microsoft.com/en-us/research/wp-content/uploads/2019/02/Mackenzie-www19.pdf
- [S29] Kuzi, Carmel, Libov & Raviv, Query Expansion for Email Search, SIGIR 2017: https://doi.org/10.1145/3077136.3080660
- [S30] Carmel, Lewin-Eytan, Libov, Maarek & Raviv, Promoting Relevant Results in Time-Ranked Mail Search, WWW 2017: https://doi.org/10.1145/3038912.3052659
- [S31] Carmel et al., The Demographics of Mail Search and their Application to Query Suggestion, WWW 2017: https://doi.org/10.1145/3038912.3052658
- [S32] Bendersky, Metzler, Najork & Wang, a survey of personal search, 2024: https://arxiv.org/html/2412.12330v1
- [S33] Wang, Bendersky, Metzler & Najork, Learning to Rank with Selection Bias in Personal Search, SIGIR 2016: https://doi.org/10.1145/2911451.2911537
- [S34] Wang et al., Position Bias Estimation for Unbiased Learning to Rank in Personal Search, WSDM 2018: https://doi.org/10.1145/3159652.3159732
- Not read in full: Carmel et al., Rank by Time or by Relevance?, CIKM 2015 (https://doi.org/10.1145/2806416.2806471), cited through [S29] and [S30]; Narang et al., CHIIR 2017.

**Forums and issue trackers**
- [S35] Gmail substring search: https://webapps.stackexchange.com/questions/21619
- [S36] A URL behind link text: https://webapps.stackexchange.com/questions/158301
- [S37] Special characters in Gmail: https://webapps.stackexchange.com/questions/31322
- [S38] Plus-addressing and `to:`: https://webapps.stackexchange.com/questions/133440
- [S39] Domains and subdomains: https://webapps.stackexchange.com/questions/40841
- [S40] Words inside an address: https://webapps.stackexchange.com/questions/101510
- [S41] Tracking numbers: https://webapps.stackexchange.com/questions/1245
- [S42] Emoji and "via" senders: https://webapps.stackexchange.com/questions/89119, https://webapps.stackexchange.com/questions/142706
- [S43] Operators lost in filters: https://webapps.stackexchange.com/questions/127207
- [S44] Pure negation: https://webapps.stackexchange.com/questions/29626
- [S45] Outlook prefix-only matching: https://superuser.com/questions/626596
- [S47] Outlook top-bar search vs Advanced Find: https://learn.microsoft.com/en-us/answers/questions/5829960/
- [S48] Outlook numbers inside codes: https://learn.microsoft.com/en-us/answers/questions/5584697/
- [S49] Outlook partial numbers: https://learn.microsoft.com/en-us/answers/questions/4707493/
- [S50] How Exchange indexes apostrophes: https://learn.microsoft.com/en-us/archive/blogs/exchangesearch/how-is-apostrophe-indexed-and-searched-for-discovery-exchange-search
- [S51] A client that strips apostrophes: https://github.com/hherb/localmail/issues/373
- [S52] Hacker News threads on mail search: https://news.ycombinator.com/item?id=31998225, https://news.ycombinator.com/item?id=8755010
- Reddit (API blocked), the Google Community threads (pages didn't render) and Thunderbird's support articles (didn't render) couldn't be read. Where they would have been cited, source code was used instead.
